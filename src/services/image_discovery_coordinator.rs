//! Orchestrates per-image package discovery.
//!
//! Port of `Services/ImageDiscovery/ImageDiscoveryCoordinator.cs` (+ the pure
//! decision helpers from `Helpers/DiscoveryHeuristics.cs`). For one target image
//! the coordinator: picks a probe strategy → launches a headless Skaha job that
//! runs the embedded probe/inspector script → polls until the job reaches a
//! terminal state (tolerating Skaha's informer-cache "job not visible yet" race)
//! → reads the manifest JSON the probe printed to the job logs → parses + caches
//! it. Successful manifests short-circuit future calls unless `force`. Concurrent
//! callers for the same image coalesce: the first claims an in-flight slot and
//! the rest get a transient `Busy` failure rather than launching a duplicate job.
//!
//! Unlike the Windows reference (which round-trips the manifest through VOSpace),
//! this Linux port passes the probe script *inline* via `bash -c` and recovers
//! the manifest from the job's stdout logs — no VOSpace upload/download step.
//!
//! Only the pure decision helpers (strategy pick, log→JSON extraction,
//! terminal-state detection, job-name/auth-header munging) are unit-tested; the
//! headless round-trip itself needs a live Skaha and is exercised in the app.

use crate::helpers::embedded_probe_scripts::{inspector_script, probe_script};
use crate::helpers::manifest_parser::parse_manifest;
use crate::models::image_manifest::{DiscoveryOutcome, ImageManifest};
use crate::models::SessionLaunchParams;
use crate::services::image_discovery_settings_service::ImageDiscoverySettingsService;
use crate::services::manifest_store::JsonManifestStore;
use crate::state::AppServices;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Stable failure categories persisted in the discovery cache (mirrors the C#
/// `FailureCategory` enum). Kept as `&str` constants because the store's failure
/// record carries a free-form category string.
mod category {
    pub const JOB_SUBMIT_FAILED: &str = "JobSubmitFailed";
    pub const JOB_TIMED_OUT: &str = "JobTimedOut";
    pub const MANIFEST_FETCH_FAILED: &str = "ManifestFetchFailed";
    pub const MANIFEST_PARSE_FAILED: &str = "ManifestParseFailed";
    pub const UNKNOWN: &str = "Unknown";
    /// Not a persisted outcome — signals a coalesced concurrent probe.
    pub const BUSY: &str = "Busy";
}

/// Which probe strategy applies to a target image (port of `ProbeStrategy`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProbeStrategy {
    /// Run `probe.sh` inside the target image itself (image supports headless).
    InTarget,
    /// Launch a known-good headless host that introspects the target via syft.
    Inspector,
}

/// Orchestrates discovery for one image at a time, coalescing concurrent probes
/// of the same id. Clone-free; share via `Arc`.
pub struct ImageDiscoveryCoordinator {
    store: Arc<JsonManifestStore>,
    settings: Mutex<ImageDiscoverySettingsService>,
    /// Image ids with a probe currently in flight (coalescing gate).
    in_flight: Mutex<HashSet<String>>,
    /// Backoff schedule for Skaha's informer-cache "job not visible yet" race.
    race_backoffs: Vec<Duration>,
    /// Delay between steady-state poll iterations.
    poll_delay: Duration,
    /// Maximum poll iterations before declaring a timeout.
    max_polls: usize,
}

impl ImageDiscoveryCoordinator {
    /// Create a coordinator over `store`, with a freshly-loaded settings service
    /// and the default Skaha timing schedule (3s/7s/15s race backoffs, 3s poll
    /// interval, 200 poll cap ≈ 10 min).
    pub fn new(store: Arc<JsonManifestStore>) -> Self {
        ImageDiscoveryCoordinator {
            store,
            settings: Mutex::new(ImageDiscoverySettingsService::new()),
            in_flight: Mutex::new(HashSet::new()),
            race_backoffs: vec![
                Duration::from_secs(3),
                Duration::from_secs(7),
                Duration::from_secs(15),
            ],
            poll_delay: Duration::from_secs(3),
            max_polls: 200,
        }
    }

    /// The backing manifest cache (shared `Arc`), for the search/facet UI.
    pub fn store(&self) -> Arc<JsonManifestStore> {
        Arc::clone(&self.store)
    }

    /// Discover packages for one image. A cached *successful* manifest short-
    /// circuits unless `force`; cached failures always re-run. Concurrent probes
    /// of the same id coalesce — the loser gets a transient `Busy` failure.
    pub async fn discover_image(
        &self,
        services: &AppServices,
        image_id: &str,
        force: bool,
    ) -> DiscoveryOutcome {
        // (1) Cache short-circuit — successes only.
        if !force {
            if let Some(cached) = self.cached_success(image_id) {
                return DiscoveryOutcome::Manifest(cached);
            }
        }

        // (2) In-flight coalescing. The guard removes the id on drop.
        let _guard = match self.claim_in_flight(image_id) {
            Some(g) => g,
            None => {
                return DiscoveryOutcome::Failure {
                    category: category::BUSY.to_string(),
                    message: format!("A probe for {image_id} is already running"),
                    job_id: None,
                };
            }
        };

        self.run_discovery(services, image_id).await
    }

    // -- cache / coalescing -------------------------------------------------

    /// The cached manifest for `image_id`, if the last outcome was a success.
    fn cached_success(&self, image_id: &str) -> Option<ImageManifest> {
        match self.store.get(image_id)?.outcome {
            DiscoveryOutcome::Manifest(m) => Some(m),
            DiscoveryOutcome::Failure { .. } => None,
        }
    }

    /// Claim the in-flight slot for `image_id`. Returns `None` if a probe is
    /// already running for it (the caller should report `Busy`).
    fn claim_in_flight(&self, image_id: &str) -> Option<InFlightGuard<'_>> {
        let mut set = self.in_flight.lock().unwrap_or_else(|e| e.into_inner());
        if set.contains(image_id) {
            return None;
        }
        set.insert(image_id.to_string());
        Some(InFlightGuard {
            set: &self.in_flight,
            id: image_id.to_string(),
        })
    }

    // -- the discovery pipeline --------------------------------------------

    async fn run_discovery(&self, services: &AppServices, image_id: &str) -> DiscoveryOutcome {
        // Auth: every headless launch and log fetch needs a bearer token, and a
        // blank username fails opaquely deeper in the stack — reject clearly.
        let token = match services.get_token().await {
            Some(t) if !t.trim().is_empty() => t,
            _ => {
                return self.fail(
                    image_id,
                    category::JOB_SUBMIT_FAILED,
                    "Probe submit failed: not signed in — sign in to CANFAR before inspecting images.",
                    None,
                );
            }
        };

        // Strategy: headless-capable target → in-target probe, else inspector.
        let types = self.lookup_image_types(services, &token, image_id).await;
        let strategy = strategy(types.as_deref());

        // Registry auth + inspector image. Re-load the persisted settings FRESH
        // here (rather than using the copy cached at app startup) so an inspector
        // image / registry credentials the user just set in Settings are honoured
        // — otherwise the coordinator's stale copy would fall back to the default
        // `skaha/terminal` inspector. (Extracted before any await so the std Mutex
        // guard is never held across a suspension point.)
        let (inspector_image, auth_header) = {
            let fresh = ImageDiscoverySettingsService::new();
            let mut cached = self.settings.lock().unwrap_or_else(|e| e.into_inner());
            *cached = fresh;
            (
                cached.resolve_inspector_image(),
                cached.current_auth_header(),
            )
        };
        let (registry_username, registry_secret) = match auth_header.as_deref() {
            Some(h) => match decode_auth_header(h) {
                Some((u, p)) => (Some(u), Some(p)),
                None => (None, None),
            },
            None => (None, None),
        };

        let (launch_image, env_name, script_body) = match strategy {
            ProbeStrategy::InTarget => (image_id.to_string(), "IMAGE_ID", probe_script()),
            ProbeStrategy::Inspector => (inspector_image, "TARGET_IMAGE", inspector_script()),
        };
        if launch_image.trim().is_empty() {
            return self.fail(
                image_id,
                category::JOB_SUBMIT_FAILED,
                "Probe submit failed: no image to launch (inspector image unresolved)",
                None,
            );
        }

        // Pass the script inline via `bash -c`, exporting the id env var the
        // script keys off of so we never depend on Skaha's env-field handling.
        let inline = format!(
            "export {env_name}={}\n{script_body}",
            shell_single_quote(image_id)
        );
        let prefix = if strategy == ProbeStrategy::InTarget {
            "vp"
        } else {
            "vi"
        };
        let params = SessionLaunchParams {
            name: make_job_name(prefix, image_id, &new_job_suffix()),
            image: launch_image,
            session_type: "headless".to_string(),
            cores: 1,
            ram: 1,
            gpus: 0,
            cmd: Some("/bin/bash".to_string()),
            env: None,
            registry_username,
            registry_secret,
            args: Some(vec!["-c".to_string(), inline]),
            replicas: Some(1),
        };

        // (4)+(5-launch) Launch with the Skaha submit-race retry.
        let job_id = match self.launch_with_retry(services, &token, &params).await {
            Ok(id) => id,
            Err(msg) => {
                return self.fail(
                    image_id,
                    category::JOB_SUBMIT_FAILED,
                    &format!("Probe submit failed: {msg}"),
                    None,
                );
            }
        };

        // (5) Poll until terminal, tolerating the informer-cache visibility race.
        if let Err(msg) = self.poll_until_terminal(services, &token, &job_id).await {
            self.best_effort_delete(services, &token, &job_id).await;
            let category = if msg.contains("timed out") {
                category::JOB_TIMED_OUT
            } else {
                category::UNKNOWN
            };
            return self.fail(image_id, category, &msg, Some(job_id));
        }

        // (6) Recover the manifest JSON from the job's stdout logs.
        let logs = services
            .sessions
            .get_logs(&token, &job_id)
            .await
            .unwrap_or_default();
        let json = match extract_manifest_json(&logs) {
            Some(j) => j,
            None => {
                self.best_effort_delete(services, &token, &job_id).await;
                return self.fail(
                    image_id,
                    category::MANIFEST_FETCH_FAILED,
                    "Manifest fetch failed: job produced no manifest JSON in its logs",
                    Some(job_id),
                );
            }
        };

        let manifest = match parse_manifest(&json) {
            Ok(m) => m,
            Err(e) => {
                self.best_effort_delete(services, &token, &job_id).await;
                return self.fail(
                    image_id,
                    category::MANIFEST_PARSE_FAILED,
                    &format!("Manifest parse failed: {e}"),
                    Some(job_id),
                );
            }
        };

        // A "stub" manifest is the placeholder a failed probe writes (no packages
        // + a probeNotes reason). Refuse to cache it — surface the reason instead.
        if is_stub_manifest(&manifest, probe_notes_of(&json).as_deref()) {
            self.best_effort_delete(services, &token, &job_id).await;
            let reason =
                probe_notes_of(&json).unwrap_or_else(|| "no software detected".to_string());
            return self.fail(
                image_id,
                category::MANIFEST_FETCH_FAILED,
                &format!("Manifest fetch failed: probe wrote a stub manifest — {reason}"),
                Some(job_id),
            );
        }

        // (7) Success — best-effort reap the job, then cache and return.
        self.best_effort_delete(services, &token, &job_id).await;
        let now = chrono::Utc::now().to_rfc3339();
        self.store.set_manifest(image_id, manifest.clone(), now);
        DiscoveryOutcome::Manifest(manifest)
    }

    /// The Skaha session types advertised for `image_id`, or `None` if the images
    /// listing is unavailable or the image is unknown (→ inspector strategy).
    async fn lookup_image_types(
        &self,
        services: &AppServices,
        token: &str,
        image_id: &str,
    ) -> Option<Vec<String>> {
        let images = services.images.get_images(token).await.ok()?;
        images
            .into_iter()
            .find(|img| img.id == image_id)
            .map(|img| img.types)
    }

    /// Submit the headless job, retrying only on Skaha's informer-cache submit
    /// race ("jobs.batch … not found"). Other errors fail fast.
    async fn launch_with_retry(
        &self,
        services: &AppServices,
        token: &str,
        params: &SessionLaunchParams,
    ) -> Result<String, String> {
        let attempts = self.race_backoffs.len() + 1;
        for (i, &backoff) in self.race_backoffs.iter().enumerate() {
            match services.sessions.launch_session(token, params).await {
                Ok(id) if !id.trim().is_empty() => return Ok(id),
                Ok(_) => return Err("Skaha returned no job id".to_string()),
                Err(msg) if is_skaha_job_not_found_race(&msg) => {
                    let _ = i;
                    tokio::time::sleep(backoff).await;
                }
                Err(msg) => return Err(msg),
            }
        }
        // Final attempt after the backoff schedule is exhausted.
        match services.sessions.launch_session(token, params).await {
            Ok(id) if !id.trim().is_empty() => Ok(id),
            Ok(_) => Err("Skaha returned no job id".to_string()),
            Err(msg) if is_skaha_job_not_found_race(&msg) => Err(format!(
                "Skaha couldn't see the job it just created after {attempts} attempts \
                 (informer-cache lag, not a quota issue): {msg}"
            )),
            Err(msg) => Err(msg),
        }
    }

    /// Poll the sessions listing until `job_id` reaches a terminal state. A job
    /// that was seen and then vanished from the listing is treated as reaped
    /// after completion (success). A job that is *never* visible is retried on
    /// the informer-cache backoff schedule before giving up.
    async fn poll_until_terminal(
        &self,
        services: &AppServices,
        token: &str,
        job_id: &str,
    ) -> Result<(), String> {
        let mut ever_seen = false;
        let mut race_attempt = 0usize;

        for _ in 0..self.max_polls {
            let jobs = match services.sessions.get_sessions(token).await {
                Ok(j) => j,
                Err(e) => return Err(format!("poll: {e}")),
            };

            match jobs.iter().find(|s| s.id == job_id) {
                Some(session) => {
                    ever_seen = true;
                    if is_terminal(&session.status) {
                        if is_failed(&session.status) {
                            return Err(format!("job ended in failed state: {}", session.status));
                        }
                        return Ok(());
                    }
                    tokio::time::sleep(self.poll_delay).await;
                }
                None if ever_seen => {
                    // Dropped from the listing after we'd seen it → reaped on
                    // completion; the log fetch validates the outcome.
                    return Ok(());
                }
                None => {
                    // Not visible yet — informer-cache race. Back off, then retry.
                    match self.race_backoffs.get(race_attempt) {
                        Some(&backoff) => {
                            race_attempt += 1;
                            tokio::time::sleep(backoff).await;
                        }
                        None => {
                            return Err(format!(
                                "Skaha never surfaced job {job_id} after \
                                 {} visibility retries (informer-cache lag)",
                                self.race_backoffs.len()
                            ));
                        }
                    }
                }
            }
        }
        Err("Probe timed out".to_string())
    }

    /// Delete a finished probe job — best effort, never surfaces an error.
    async fn best_effort_delete(&self, services: &AppServices, token: &str, job_id: &str) {
        let _ = services.sessions.delete_session(token, job_id).await;
    }

    /// Persist a typed failure (best effort) and return it as the outcome.
    fn fail(
        &self,
        image_id: &str,
        category: &str,
        message: &str,
        job_id: Option<String>,
    ) -> DiscoveryOutcome {
        let now = chrono::Utc::now().to_rfc3339();
        self.store
            .set_failure(image_id, category, message, job_id.clone(), now);
        DiscoveryOutcome::Failure {
            category: category.to_string(),
            message: message.to_string(),
            job_id,
        }
    }
}

/// RAII release of an in-flight coalescing slot.
struct InFlightGuard<'a> {
    set: &'a Mutex<HashSet<String>>,
    id: String,
}

impl Drop for InFlightGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut set) = self.set.lock() {
            set.remove(&self.id);
        }
    }
}

// ---------------------------------------------------------------------------
// Pure decision helpers (ported from DiscoveryHeuristics.cs — no I/O, testable)
// ---------------------------------------------------------------------------

/// Strategy from the image's session types: headless-capable → in-target probe;
/// unknown/null or any non-headless set → inspector (safer for private/unknown
/// images). Faithful port of `DiscoveryHeuristics.Strategy`.
fn strategy(types: Option<&[String]>) -> ProbeStrategy {
    match types {
        None => ProbeStrategy::Inspector,
        Some(t) if t.iter().any(|s| s.eq_ignore_ascii_case("headless")) => ProbeStrategy::InTarget,
        Some(_) => ProbeStrategy::Inspector,
    }
}

/// A Skaha job status that no longer changes (mirrors the adapter's `IsTerminal`).
fn is_terminal(status: &str) -> bool {
    ["succeeded", "completed", "failed", "error", "terminating"]
        .iter()
        .any(|s| status.eq_ignore_ascii_case(s))
}

/// A terminal status that indicates the job failed (mirrors `IsFailed`).
fn is_failed(status: &str) -> bool {
    ["failed", "error"]
        .iter()
        .any(|s| status.eq_ignore_ascii_case(s))
}

/// Match the specific Skaha error from the K8s informer-cache submit race (the
/// POST created the job but the immediate GET 404'd). Faithful port of
/// `DiscoveryHeuristics.IsSkahaJobNotFoundRace`.
fn is_skaha_job_not_found_race(message: &str) -> bool {
    let msg = message.to_lowercase();
    msg.contains("jobs.batch") && msg.contains("not found")
}

/// A "stub" manifest: a failed probe's placeholder — no packages of any kind
/// AND a non-empty probe-notes reason. Port of `DiscoveryHeuristics.IsStubManifest`
/// (the `probeNotes` field lives only in the raw JSON, so it is supplied here).
fn is_stub_manifest(m: &ImageManifest, probe_notes: Option<&str>) -> bool {
    let has_packages = !m.dpkg.is_empty()
        || !m.rpm.is_empty()
        || !m.apk.is_empty()
        || !m.python.is_empty()
        || !m.r_packages.is_empty()
        || !m.conda_envs.is_empty()
        || m.python_by_env.values().any(|v| !v.is_empty());
    if has_packages {
        return false;
    }
    matches!(probe_notes, Some(n) if !n.trim().is_empty())
}

/// Extract the `probeNotes` string from a raw manifest JSON payload, if present.
fn probe_notes_of(json: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(json)
        .ok()?
        .get("probeNotes")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// A fresh 8-char lowercase-hex job-name suffix (port of `NewJobSuffix`).
fn new_job_suffix() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..8].to_lowercase()
}

/// Build a Skaha session name safe for K8s DNS-1123 labels (≤63 chars, lowercase
/// ASCII alphanumerics + hyphens, no leading/trailing or consecutive hyphens).
/// Port of `DiscoveryHeuristics.MakeJobName` (`suffix` is caller-supplied so the
/// result is deterministic in tests).
fn make_job_name(prefix: &str, image_id: &str, suffix: &str) -> String {
    // "<prefix>-<middle>-<suffix>"
    let budget = 63i32 - prefix.len() as i32 - 1 - suffix.len() as i32 - 1;
    let safe = image_id.to_lowercase();
    let slice_len = budget.max(0).min(safe.chars().count() as i32) as usize;
    let middle: String = safe.chars().take(slice_len).collect();

    let mut sb = String::with_capacity(middle.len());
    for ch in middle.chars() {
        let c = if ch.is_ascii_alphanumeric() || ch == '-' {
            ch
        } else {
            '-'
        };
        if c == '-' && sb.ends_with('-') {
            continue; // collapse runs
        }
        sb.push(c);
    }
    let trimmed = sb.trim_matches('-');
    if trimmed.is_empty() {
        format!("{prefix}-{suffix}")
    } else {
        format!("{prefix}-{trimmed}-{suffix}")
    }
}

/// Decode a `base64(username:secret)` `x-skaha-registry-auth` header back into
/// `(username, secret)`. Returns `None` on non-base64 / non-UTF-8 / no-colon.
fn decode_auth_header(header: &str) -> Option<(String, String)> {
    use base64::Engine as _;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(header.trim())
        .ok()?;
    let text = String::from_utf8(decoded).ok()?;
    let (user, secret) = text.split_once(':')?;
    Some((user.to_string(), secret.to_string()))
}

/// Single-quote a value for safe interpolation into a POSIX shell command.
fn shell_single_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if c == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(c);
        }
    }
    out.push('\'');
    out
}

/// Extract the manifest JSON object the probe printed to its stdout logs. Scans
/// the log text for balanced, string-aware top-level `{…}` objects and returns
/// the last one that looks like a manifest (carries `imageID`/`schemaVersion`),
/// falling back to the last balanced object. `None` when no object is present.
fn extract_manifest_json(logs: &str) -> Option<String> {
    let objects = balanced_json_objects(logs);
    for obj in objects.iter().rev() {
        if obj.contains("\"imageID\"") || obj.contains("\"schemaVersion\"") {
            return Some(obj.clone());
        }
    }
    objects.into_iter().next_back()
}

/// Every balanced, top-level `{…}` object substring in `text`, in order. String
/// literals (and their `\"` escapes) are respected so braces inside strings
/// don't confuse the depth counter. Non-ASCII bytes are passed through untouched
/// (slices always start/end on ASCII `{`/`}`, so UTF-8 boundaries stay valid).
fn balanced_json_objects(text: &str) -> Vec<String> {
    let bytes = text.as_bytes();
    let n = bytes.len();
    let mut objects = Vec::new();
    let mut i = 0;
    while i < n {
        if bytes[i] != b'{' {
            i += 1;
            continue;
        }
        let start = i;
        let mut depth = 0i32;
        let mut in_string = false;
        let mut escaped = false;
        let mut j = i;
        let mut closed = false;
        while j < n {
            let c = bytes[j];
            if in_string {
                if escaped {
                    escaped = false;
                } else if c == b'\\' {
                    escaped = true;
                } else if c == b'"' {
                    in_string = false;
                }
            } else {
                match c {
                    b'"' => in_string = true,
                    b'{' => depth += 1,
                    b'}' => {
                        depth -= 1;
                        if depth == 0 {
                            closed = true;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            j += 1;
        }
        if closed {
            if let Ok(s) = std::str::from_utf8(&bytes[start..=j]) {
                objects.push(s.to_string());
            }
            i = j + 1;
        } else {
            // This `{` never balances (e.g. an incidental brace in a log line).
            // Skip just it and keep scanning for a real object further on.
            i += 1;
        }
    }
    objects
}

// ---------------------------------------------------------------------------
// Tests (pure logic only)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn strategy_picks_in_target_only_for_headless() {
        assert_eq!(strategy(None), ProbeStrategy::Inspector);
        assert_eq!(
            strategy(Some(&["headless".to_string()])),
            ProbeStrategy::InTarget
        );
        assert_eq!(
            strategy(Some(&["notebook".to_string(), "HEADLESS".to_string()])),
            ProbeStrategy::InTarget // case-insensitive
        );
        assert_eq!(
            strategy(Some(&["notebook".to_string(), "desktop".to_string()])),
            ProbeStrategy::Inspector
        );
        assert_eq!(strategy(Some(&[])), ProbeStrategy::Inspector);
    }

    #[test]
    fn terminal_and_failed_state_detection() {
        for ok in ["Succeeded", "Completed", "succeeded"] {
            assert!(is_terminal(ok));
            assert!(!is_failed(ok));
        }
        for bad in ["Failed", "Error", "error"] {
            assert!(is_terminal(bad));
            assert!(is_failed(bad));
        }
        assert!(is_terminal("Terminating"));
        assert!(!is_failed("Terminating"));
        for running in ["Running", "Pending", "Queued", ""] {
            assert!(!is_terminal(running), "{running} should not be terminal");
            assert!(!is_failed(running));
        }
    }

    #[test]
    fn skaha_race_matcher_is_specific() {
        assert!(is_skaha_job_not_found_race(
            "500: jobs.batch \"skaha-abc\" not found"
        ));
        assert!(is_skaha_job_not_found_race(
            "Error from server (NotFound): jobs.batch not found"
        ));
        // Other faults must not trigger a retry.
        assert!(!is_skaha_job_not_found_race(
            "403 Forbidden: quota exceeded"
        ));
        assert!(!is_skaha_job_not_found_race("pods not found"));
        assert!(!is_skaha_job_not_found_race("jobs.batch already exists"));
    }

    fn manifest_with(python: &[&str]) -> ImageManifest {
        ImageManifest {
            image_id: "img:1".to_string(),
            python: python.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn stub_manifest_needs_no_packages_and_a_note() {
        // No packages + a note → stub.
        assert!(is_stub_manifest(&manifest_with(&[]), Some("syft failed")));
        // A note but real packages → not a stub.
        assert!(!is_stub_manifest(
            &manifest_with(&["numpy"]),
            Some("syft failed")
        ));
        // No packages, no note → not a stub (a legitimately-empty image).
        assert!(!is_stub_manifest(&manifest_with(&[]), None));
        assert!(!is_stub_manifest(&manifest_with(&[]), Some("   ")));
        // Per-env python counts as packages.
        let mut m = manifest_with(&[]);
        m.python_by_env = BTreeMap::from([("base".to_string(), vec!["scipy".to_string()])]);
        assert!(!is_stub_manifest(&m, Some("note")));
    }

    #[test]
    fn probe_notes_extracted_from_raw_json() {
        assert_eq!(
            probe_notes_of(r#"{"imageID":"x:1","probeNotes":"syft failed (rc=1)"}"#).as_deref(),
            Some("syft failed (rc=1)")
        );
        assert_eq!(probe_notes_of(r#"{"imageID":"x:1"}"#), None);
        assert_eq!(probe_notes_of("not json"), None);
    }

    #[test]
    fn make_job_name_is_dns_safe_and_deterministic() {
        let name = make_job_name("vp", "images.canfar.net/skaha/astroml:24.07", "deadbeef");
        assert!(name.starts_with("vp-"));
        assert!(name.ends_with("-deadbeef"));
        assert!(name.len() <= 63);
        // Only lowercase alphanumerics + hyphens; no consecutive hyphens.
        assert!(name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'));
        assert!(!name.contains("--"));
        assert!(!name.starts_with('-') && !name.ends_with('-'));
        // Deterministic for a fixed suffix.
        assert_eq!(
            name,
            make_job_name("vp", "images.canfar.net/skaha/astroml:24.07", "deadbeef")
        );
    }

    #[test]
    fn make_job_name_respects_63_char_budget_and_empty_middle() {
        let long = "a".repeat(200);
        let name = make_job_name("vi", &long, "cafef00d");
        assert!(name.len() <= 63, "got {} chars", name.len());
        // An id that sanitizes to nothing still yields a valid name.
        assert_eq!(make_job_name("vp", "///", "abcd1234"), "vp-abcd1234");
    }

    #[test]
    fn new_job_suffix_is_8_lowercase_hex() {
        let s = new_job_suffix();
        assert_eq!(s.len(), 8);
        assert!(s
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    }

    #[test]
    fn decode_auth_header_round_trips_user_secret() {
        use base64::Engine as _;
        let header = base64::engine::general_purpose::STANDARD.encode("alice:s3cr3t");
        assert_eq!(
            decode_auth_header(&header),
            Some(("alice".to_string(), "s3cr3t".to_string()))
        );
        // Secret may itself contain a colon; only the first is the delimiter.
        let h2 = base64::engine::general_purpose::STANDARD.encode("bob:a:b:c");
        assert_eq!(
            decode_auth_header(&h2),
            Some(("bob".to_string(), "a:b:c".to_string()))
        );
        // Empty secret is preserved.
        let h3 = base64::engine::general_purpose::STANDARD.encode("u:");
        assert_eq!(
            decode_auth_header(&h3),
            Some(("u".to_string(), String::new()))
        );
        // Garbage / colonless input rejected.
        assert_eq!(decode_auth_header("!!!not base64!!!"), None);
        let no_colon = base64::engine::general_purpose::STANDARD.encode("nocolon");
        assert_eq!(decode_auth_header(&no_colon), None);
    }

    #[test]
    fn shell_single_quote_escapes_embedded_quotes() {
        assert_eq!(
            shell_single_quote("images.canfar.net/x:1"),
            "'images.canfar.net/x:1'"
        );
        assert_eq!(shell_single_quote("a'b"), "'a'\\''b'");
    }

    #[test]
    fn extract_manifest_json_from_probe_logs() {
        let logs = "some setup noise\n\
            checking python...\n\
            {\"schemaVersion\":3,\"imageID\":\"images.canfar.net/skaha/base:1.0\",\"dpkgPackages\":[]}\n\
            ok: /arc/home/alice/.verbinal/manifests/x.json\n";
        let json = extract_manifest_json(logs).expect("manifest json present");
        let m = parse_manifest(&json).expect("parses");
        assert_eq!(m.image_id, "images.canfar.net/skaha/base:1.0");
    }

    #[test]
    fn extract_manifest_json_handles_pretty_printed_and_braces_in_strings() {
        let logs = "log line with a brace } and a { fragment\n\
            {\n  \"schemaVersion\": 3,\n  \"imageID\": \"x:1\",\n  \"probeNotes\": \"has } and { in it\"\n}\n\
            trailing\n";
        let json = extract_manifest_json(logs).expect("json");
        assert!(json.contains("\"imageID\""));
        assert_eq!(probe_notes_of(&json).as_deref(), Some("has } and { in it"));
    }

    #[test]
    fn extract_manifest_json_prefers_manifest_over_incidental_objects() {
        // An earlier non-manifest object should not be chosen over the manifest.
        let logs = "{\"unrelated\":true}\n\
            {\"schemaVersion\":3,\"imageID\":\"pick:me\"}\n\
            {\"also\":\"trailing non-manifest\"}\n";
        let json = extract_manifest_json(logs).unwrap();
        assert!(json.contains("\"imageID\""));
        assert!(json.contains("pick:me"));
    }

    #[test]
    fn extract_manifest_json_none_when_absent() {
        assert_eq!(extract_manifest_json(""), None);
        assert_eq!(extract_manifest_json("no json here at all"), None);
        // An unbalanced brace tail yields nothing.
        assert_eq!(extract_manifest_json("{\"oops\": unterminated"), None);
    }

    #[test]
    fn coordinator_reports_cached_success_and_shares_store() {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "verbinal_coord_test_{}_{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let store = Arc::new(JsonManifestStore::with_dir(dir.clone()));
        let coord = ImageDiscoveryCoordinator::new(Arc::clone(&store));

        assert!(coord.cached_success("img:1").is_none());
        store.set_manifest(
            "img:1",
            manifest_with(&["numpy"]),
            "2026-07-07T00:00:00Z".to_string(),
        );
        // Reads through the shared store.
        assert!(coord.cached_success("img:1").is_some());
        assert_eq!(coord.store().count(), 1);

        // A failure outcome is not a cached success.
        store.set_failure(
            "img:2",
            category::UNKNOWN,
            "boom",
            None,
            "2026-07-07T00:00:00Z".to_string(),
        );
        assert!(coord.cached_success("img:2").is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn in_flight_guard_coalesces_then_releases() {
        let store = Arc::new(JsonManifestStore::with_dir(
            std::env::temp_dir().join("verbinal_coord_inflight_test"),
        ));
        let coord = ImageDiscoveryCoordinator::new(store);
        let g1 = coord.claim_in_flight("img:1");
        assert!(g1.is_some(), "first claim succeeds");
        assert!(
            coord.claim_in_flight("img:1").is_none(),
            "second concurrent claim is refused (Busy)"
        );
        // A different image is independent.
        assert!(coord.claim_in_flight("img:2").is_some());
        drop(g1);
        // Slot released → reclaimable.
        assert!(coord.claim_in_flight("img:1").is_some());
    }
}
