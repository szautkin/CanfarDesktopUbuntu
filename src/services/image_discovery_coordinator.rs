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

use crate::helpers::embedded_probe_scripts::{
    inspector_script, inspector_script_name, manifest_path, probe_script, probe_script_name,
    HOME_SUBDIR,
};
use crate::helpers::job_diagnostics::{tail, MAX_REASON_CHARS};
use crate::helpers::manifest_parser::parse_manifest;
use crate::models::image_manifest::{DiscoveryOutcome, ImageManifest};
use crate::models::job_record::{JobOrigin, JobOutcome, JobRecord};
use crate::models::SessionLaunchParams;
use crate::services::image_discovery_settings_service::ImageDiscoverySettingsService;
use crate::services::job_history_store::JobHistoryStore;
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
    /// Where finished probe jobs are remembered. The coordinator deletes its
    /// own jobs the moment they finish, so without this a failed inspection
    /// leaves nothing behind: no job, no logs, no events, and a cache entry
    /// that gets overwritten by the next attempt on the same image.
    history: Arc<JobHistoryStore>,
    settings: Mutex<ImageDiscoverySettingsService>,
    /// Image ids with a probe currently in flight (coalescing gate).
    in_flight: Mutex<HashSet<String>>,
    /// Script filenames already uploaded this run (mirrors the reference's
    /// `_probeUploaded` / `_inspectorUploaded` flags). The name is a hash of
    /// the body, so this is a cache, never a staleness risk.
    uploaded: Mutex<HashSet<String>>,
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
    pub fn new(store: Arc<JsonManifestStore>, history: Arc<JobHistoryStore>) -> Self {
        ImageDiscoveryCoordinator {
            store,
            history,
            settings: Mutex::new(ImageDiscoverySettingsService::new()),
            in_flight: Mutex::new(HashSet::new()),
            uploaded: Mutex::new(HashSet::new()),
            race_backoffs: vec![
                Duration::from_secs(3),
                Duration::from_secs(7),
                Duration::from_secs(15),
            ],
            poll_delay: Duration::from_secs(3),
            max_polls: 200,
        }
    }

    #[cfg(test)]
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

        self.run_discovery(services, image_id, force).await
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

    async fn run_discovery(
        &self,
        services: &AppServices,
        image_id: &str,
        force: bool,
    ) -> DiscoveryOutcome {
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

        // Everything below needs the account name; resolve it once. A failure
        // here is not fatal on its own — only the upload truly requires it, and
        // it reports its own error.
        let username = Self::username(services, &token).await;

        // A manifest a previous probe published is a probe we do not have to
        // run: cheaper, faster, and it works across machines and reinstalls in
        // a way the local cache cannot. Skipped when forced, since forcing is
        // how you ask for a fresh look.
        if !force {
            if let Ok(ref user) = username {
                if let Some(manifest) = self
                    .fetch_manifest_if_present(services, &token, user, image_id)
                    .await
                {
                    let now = chrono::Utc::now().to_rfc3339();
                    self.store.set_manifest(image_id, manifest.clone(), now);
                    return DiscoveryOutcome::Manifest(manifest);
                }
            }
        }

        let username = match username {
            Ok(user) => user,
            Err(msg) => {
                return self.fail(
                    image_id,
                    category::JOB_SUBMIT_FAILED,
                    &format!("Probe submit failed: could not stage the probe script — {msg}"),
                    None,
                );
            }
        };

        // Upload the script and launch `bash <path>`.
        //
        // This used to pass the script INLINE as `bash -c <body>`, to skip the
        // upload. Skaha reads a single `args` value, so of the two form fields
        // we sent only `-c` arrived, and every probe died with
        // "/bin/bash: -c: option requires an argument". There is no inline form
        // that survives: one `args` value holding the whole script would be
        // split on whitespace at the far end. The reference uploads and passes
        // a path — one argument, no spaces in it — which is why the script
        // names have been content-hashed all along.
        let script_path = match self
            .ensure_uploaded(services, &token, &username, strategy, script_body)
            .await
        {
            Ok(path) => path,
            Err(msg) => {
                return self.fail(
                    image_id,
                    category::JOB_SUBMIT_FAILED,
                    &format!("Probe submit failed: could not stage the probe script — {msg}"),
                    None,
                );
            }
        };

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
            cmd: Some("bash".to_string()),
            // The id the script keys off of, as the reference passes it.
            env: Some(format!("{env_name}={image_id}")),
            registry_username,
            registry_secret,
            args: Some(vec![script_path]),
            replicas: Some(1),
        };

        // (4)+(5-launch) Launch with the Skaha submit-race retry.
        let job_id = match self.launch_with_retry(services, &token, &params).await {
            Ok(id) => id,
            Err(msg) => {
                let reason = format!("Probe submit failed: {msg}");
                self.remember(image_id, &params, "", JobOutcome::Failed, &reason);
                return self.fail(image_id, category::JOB_SUBMIT_FAILED, &reason, None);
            }
        };

        // (5) Poll until terminal, tolerating the informer-cache visibility race.
        if let Err(msg) = self.poll_until_terminal(services, &token, &job_id).await {
            // The job may have published just after our last poll — the
            // reference calls this a "late manifest fetch", and it turns the
            // most expensive failure mode (a slow probe, marked failed, job
            // deleted) into a success.
            if let Some(manifest) = self
                .fetch_manifest_if_present(services, &token, &username, image_id)
                .await
            {
                self.best_effort_delete(services, &token, &job_id).await;
                self.remember(image_id, &params, &job_id, JobOutcome::Succeeded, "");
                let now = chrono::Utc::now().to_rfc3339();
                self.store.set_manifest(image_id, manifest.clone(), now);
                return DiscoveryOutcome::Manifest(manifest);
            }

            // Read the job's own account of itself BEFORE deleting it. This used
            // to report "job ended in failed state: Failed" and then destroy the
            // only copy of the logs and events that said why — leaving a status
            // word where a reason should be.
            let diagnosis = self.diagnose(services, &token, &job_id, &msg).await;
            self.best_effort_delete(services, &token, &job_id).await;
            let category = if msg.contains("timed out") {
                category::JOB_TIMED_OUT
            } else {
                category::UNKNOWN
            };
            self.remember(image_id, &params, &job_id, JobOutcome::Failed, &diagnosis);
            return self.fail(image_id, category, &diagnosis, Some(job_id));
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
                // The logs are in hand and are the only evidence of what went
                // wrong; reporting their absence of JSON while discarding their
                // contents told the user nothing they could act on.
                let reason = self
                    .diagnose(
                        services,
                        &token,
                        &job_id,
                        "Manifest fetch failed: job produced no manifest JSON in its logs.",
                    )
                    .await;
                self.best_effort_delete(services, &token, &job_id).await;
                self.remember(image_id, &params, &job_id, JobOutcome::Failed, &reason);
                return self.fail(
                    image_id,
                    category::MANIFEST_FETCH_FAILED,
                    &reason,
                    Some(job_id),
                );
            }
        };

        let manifest = match parse_manifest(&json) {
            Ok(m) => m,
            Err(e) => {
                let reason = format!("Manifest parse failed: {e}");
                self.best_effort_delete(services, &token, &job_id).await;
                self.remember(image_id, &params, &job_id, JobOutcome::Failed, &reason);
                return self.fail(
                    image_id,
                    category::MANIFEST_PARSE_FAILED,
                    &reason,
                    Some(job_id),
                );
            }
        };

        // A "stub" manifest is the placeholder a failed probe writes (no packages
        // + a probeNotes reason). Refuse to cache it — surface the reason instead.
        if is_stub_manifest(&manifest, probe_notes_of(&json).as_deref()) {
            self.best_effort_delete(services, &token, &job_id).await;
            let notes = probe_notes_of(&json).unwrap_or_else(|| "no software detected".to_string());
            let reason = format!("Manifest fetch failed: probe wrote a stub manifest — {notes}");
            self.remember(image_id, &params, &job_id, JobOutcome::Failed, &reason);
            return self.fail(
                image_id,
                category::MANIFEST_FETCH_FAILED,
                &reason,
                Some(job_id),
            );
        }

        // (7) Success — best-effort reap the job, then cache and return.
        self.best_effort_delete(services, &token, &job_id).await;
        self.remember(image_id, &params, &job_id, JobOutcome::Succeeded, "");
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

    /// The signed-in account name, or why it could not be had.
    ///
    /// Every VOSpace path is `/arc/home/{username}/…`; an empty username
    /// collapses it to a directory that is not the user's, so an absent name is
    /// an error rather than a blank.
    async fn username(services: &AppServices, token: &str) -> Result<String, String> {
        services
            .auth
            .get_user_info(token)
            .await
            .map_err(|e| format!("could not resolve the CANFAR username: {e}"))?
            .username
            .filter(|name| !name.trim().is_empty())
            .ok_or_else(|| "the CANFAR username is empty".to_string())
    }

    /// The manifest a previous probe left in the user's VOSpace, if there is a
    /// usable one.
    ///
    /// The scripts have always published to `~/.verbinal/manifests/`; on CANFAR
    /// that is `/arc/home/<user>`, real storage. Nothing here read it until
    /// now, so a probe that finished after we stopped watching was a total loss
    /// — we recorded a failure, deleted the job, and its manifest sat unread.
    /// It is also the only DURABLE copy: we recover from the job's stdout and
    /// then delete the job, and the logs go with it.
    ///
    /// `None` on anything doubtful, so the caller simply launches a probe.
    /// Both references apply the same two checks:
    ///
    /// * the manifest at the path must be FOR this image — two launches with
    ///   mismatched id env vars would otherwise cross-contaminate;
    /// * a stub is what a FAILED probe writes, and caching one turns a
    ///   transient failure into a permanent wrong answer.
    async fn fetch_manifest_if_present(
        &self,
        services: &AppServices,
        token: &str,
        username: &str,
        image_id: &str,
    ) -> Option<ImageManifest> {
        let path = manifest_path(image_id);
        let bytes = services
            .vospace
            .download_bytes(token, username, &path)
            .await
            .ok()?;
        usable_manifest(&String::from_utf8(bytes).ok()?, image_id)
    }

    /// Put the script in the user's home and return its absolute path.
    ///
    /// Uploaded once per script per app run — the name is a hash of the body,
    /// so a copy already sitting there from a previous run is the same copy,
    /// and editing the script produces a different name rather than a stale
    /// one being reused.
    async fn ensure_uploaded(
        &self,
        services: &AppServices,
        token: &str,
        username: &str,
        strategy: ProbeStrategy,
        body: &str,
    ) -> Result<String, String> {
        let file_name = match strategy {
            ProbeStrategy::InTarget => probe_script_name(),
            ProbeStrategy::Inspector => inspector_script_name(),
        };
        let relative = format!("{HOME_SUBDIR}/{file_name}");
        let absolute = format!("/arc/home/{username}/{relative}");

        if self
            .uploaded
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&file_name)
        {
            return Ok(absolute);
        }

        // The directory may not exist yet; a failure here is not fatal on its
        // own, because the usual cause is that it already does.
        let _ = services
            .vospace
            .create_folder(token, username, HOME_SUBDIR)
            .await;

        services
            .vospace
            .upload_file(
                token,
                username,
                &relative,
                body.as_bytes().to_vec(),
                "text/x-shellscript",
            )
            .await
            .map_err(|e| e.to_string())?;

        self.uploaded
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(file_name);
        Ok(absolute)
    }

    /// A failed job's own account of itself: our diagnosis, then its evidence.
    ///
    /// Must be called BEFORE the job is deleted — afterwards Skaha has neither
    /// its logs nor its events.
    async fn diagnose(
        &self,
        services: &AppServices,
        token: &str,
        job_id: &str,
        summary: &str,
    ) -> String {
        format!(
            "{summary}\n\n{}",
            services.sessions.get_diagnostics(token, job_id).await
        )
    }

    /// Remember a finished probe job, so it outlives the deletion on the very
    /// next line.
    fn remember(
        &self,
        image_id: &str,
        params: &SessionLaunchParams,
        job_id: &str,
        outcome: JobOutcome,
        reason: &str,
    ) {
        let now = chrono::Utc::now().to_rfc3339();
        let record = JobRecord {
            // A submit that never got an id still deserves a row; key it by the
            // name we asked for, which is unique per attempt.
            id: if job_id.is_empty() {
                params.name.clone()
            } else {
                job_id.to_string()
            },
            name: params.name.clone(),
            image: params.image.clone(),
            origin: JobOrigin::ImageProbe,
            outcome,
            status: match outcome {
                JobOutcome::Succeeded => "Succeeded".to_string(),
                JobOutcome::Failed => "Failed".to_string(),
            },
            started_at: now.clone(),
            finished_at: now,
            failure_reason: match outcome {
                JobOutcome::Failed => Some(tail(reason, MAX_REASON_CHARS)),
                JobOutcome::Succeeded => None,
            },
            target_image: Some(image_id.to_string()),
        };
        let _ = self.history.record(record);
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

/// Whether a published manifest may stand in for a fresh probe.
///
/// Pure, so the rules can be tested without a VOSpace; the fetch above does the
/// I/O and this does the deciding. Both references apply the same two:
///
/// * the manifest must be FOR the image asked about — two launches with
///   mismatched id env vars would otherwise cross-contaminate;
/// * a stub is what a FAILED probe writes, and caching one turns a transient
///   failure into a permanent wrong answer.
fn usable_manifest(json: &str, image_id: &str) -> Option<ImageManifest> {
    let manifest = parse_manifest(json).ok()?;
    if manifest.image_id != image_id {
        return None;
    }
    if is_stub_manifest(&manifest, probe_notes_of(json).as_deref()) {
        return None;
    }
    Some(manifest)
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
        let coord = ImageDiscoveryCoordinator::new(Arc::clone(&store), history_in(&dir));

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

    /// A job history pointed at a scratch directory, never the user's own.
    fn history_in(dir: &std::path::Path) -> Arc<JobHistoryStore> {
        Arc::new(JobHistoryStore::with_dir(dir.to_path_buf()))
    }

    #[test]
    fn in_flight_guard_coalesces_then_releases() {
        let dir = std::env::temp_dir().join("verbinal_coord_inflight_test");
        let store = Arc::new(JsonManifestStore::with_dir(dir.clone()));
        let coord = ImageDiscoveryCoordinator::new(store, history_in(&dir));
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

#[cfg(test)]
mod published_manifest_rules {
    //! What a manifest found in the user's VOSpace has to satisfy before it is
    //! allowed to stand in for running a probe.

    use super::*;

    const IMAGE: &str = "images.canfar.net/skaha/astroml:1.0";

    fn manifest_json(image_id: &str, extra: &str) -> String {
        format!(
            r#"{{"schemaVersion":3,"imageID":"{image_id}","osFamily":"ubuntu",
               "dpkgPackages":[{{"name":"bash","version":"5.2"}}],
               "pythonPackages":[{{"name":"numpy","version":"1.26"}}]{extra}}}"#
        )
    }

    #[test]
    fn a_good_manifest_is_accepted() {
        let manifest = usable_manifest(&manifest_json(IMAGE, ""), IMAGE).expect("accepted");
        assert_eq!(manifest.image_id, IMAGE);
    }

    #[test]
    fn a_manifest_for_another_image_is_refused() {
        // Two launches with mismatched id env vars would write to the same
        // path. Trusting it would describe one image with another's packages —
        // wrong in a way nothing downstream could detect.
        assert!(usable_manifest(&manifest_json("images.canfar.net/other:2", ""), IMAGE).is_none());
    }

    #[test]
    fn a_stub_left_by_a_failed_probe_is_refused() {
        // Caching a stub turns a transient failure — no network egress, syft
        // missing — into a permanent answer of "this image contains nothing".
        let stub = format!(
            r#"{{"schemaVersion":3,"imageID":"{IMAGE}","dpkgPackages":[],
               "rpmPackages":[],"apkPackages":[],"pythonPackages":[],
               "probeNotes":"syft installation failed"}}"#
        );
        assert!(usable_manifest(&stub, IMAGE).is_none());
    }

    #[test]
    fn unparseable_content_is_refused_rather_than_fatal() {
        // A truncated or half-written file is a reason to launch a probe, not
        // a reason to fail the inspection.
        assert!(usable_manifest("not json at all", IMAGE).is_none());
        assert!(usable_manifest("", IMAGE).is_none());
    }
}

#[cfg(test)]
mod real_probe_output {
    //! The recovery path, end to end, against output the probe script actually
    //! produced.
    //!
    //! `tests/fixtures/probe_job_logs.txt` is a real run of
    //! `src/resources/imagedisc/probe.sh` — its status line on stderr, its
    //! manifest on stdout, interleaved as the job's logs deliver them — with
    //! the package lists trimmed and the paths made generic.
    //!
    //! Every synthetic test in this file feeds `extract_manifest_json` a string
    //! someone wrote by hand, which is how the scripts could publish their
    //! manifest to a FILE and echo only `ok: $OUT` while the suite stayed
    //! green. Nothing here could see that stdout carried no JSON, because
    //! nothing here had ever seen the real stdout.

    use super::*;

    const LOGS: &str = include_str!("../../tests/fixtures/probe_job_logs.txt");

    #[test]
    fn a_real_probe_run_yields_a_manifest() {
        let json = extract_manifest_json(LOGS).expect(
            "no manifest JSON in the job logs — the probe is writing it \
             somewhere the app cannot read",
        );
        let manifest = parse_manifest(&json).expect("the manifest did not parse");
        assert_eq!(manifest.image_id, "images.canfar.net/skaha/astroml:1.0");
        assert_eq!(manifest.os_family.as_deref(), Some("ubuntu"));
        assert!(
            !manifest.dpkg.is_empty(),
            "no dpkg packages survived parsing"
        );
        assert!(manifest.has_python(), "python went missing");
    }

    #[test]
    fn a_real_probe_run_is_not_mistaken_for_a_stub() {
        // A stub is what a FAILED probe writes. Reading a good manifest as one
        // would turn every successful inspection into a reported failure.
        let json = extract_manifest_json(LOGS).expect("manifest");
        let manifest = parse_manifest(&json).expect("parsed");
        assert!(!is_stub_manifest(
            &manifest,
            probe_notes_of(&json).as_deref()
        ));
    }

    #[test]
    fn the_status_line_does_not_confuse_the_extractor() {
        // The probe prints a path on stderr and JSON on stdout, and the job's
        // logs interleave them. The extractor has to pick the object out.
        assert!(
            LOGS.contains("ok: /arc/home"),
            "the fixture lost its status line"
        );
        let json = extract_manifest_json(LOGS).expect("manifest");
        assert!(json.starts_with('{') && json.ends_with('}'), "{json:.60}");
    }
}

#[cfg(test)]
mod failure_reporting_guards {
    //! Source guards for the two rules the failure path has to keep.
    //!
    //! Neither can be tested by running the coordinator: both concern a live
    //! Skaha job, and the round trip needs a real service. What CAN be checked
    //! is the shape of the code, and the shape is where both bugs lived.

    const SOURCE: &str = include_str!("image_discovery_coordinator.rs");

    /// The body of `run_discovery`, which is where every failure path lives.
    ///
    /// Scanned from the raw source rather than through `testing::code`: that
    /// helper cuts at the first `#[cfg(test)]`, and this file has one on a real
    /// item — the `store()` accessor — a hundred lines above `run_discovery`.
    /// The slice below ends at the function's own closing brace, well before
    /// any test module, so a guard still cannot find itself.
    fn run_discovery(code: &str) -> &str {
        let at = code
            .find("async fn run_discovery")
            .expect("run_discovery is gone");
        let end = code[at..]
            .find("\n    }\n")
            .map(|e| at + e)
            .unwrap_or(code.len());
        &code[at..end]
    }

    #[test]
    fn a_jobs_reason_is_read_before_the_job_is_deleted() {
        // The coordinator deletes its own probe jobs, and Skaha takes the logs
        // and events with them. Reporting "job ended in failed state: Failed"
        // and THEN deleting the only record of why is how an inspection failure
        // became unexplainable.
        let body = run_discovery(SOURCE);
        let diagnose = body
            .find("self.diagnose(")
            .expect("a failed job is no longer diagnosed");
        let delete = body[diagnose..]
            .find("best_effort_delete")
            .map(|e| diagnose + e)
            .expect("the diagnosed job is never deleted, so it leaks");
        assert!(
            diagnose < delete,
            "the job is deleted before its logs are read"
        );
    }

    #[test]
    fn every_outcome_that_reached_skaha_is_remembered() {
        // A failure the history does not record is a failure nobody can look up
        // once the cache entry is overwritten by the next attempt on the same
        // image; a success it does not record leaves "did the inspection ever
        // run?" unanswerable.
        //
        // Asserted as a property, not a count: every terminal return after the
        // launch parameters exist must have a `self.remember(` between it and
        // the previous one. A formula would have to be edited each time a path
        // is added, which is precisely when it should be failing instead.
        //
        // Only the paths after the launch parameters. The ones before them —
        // not signed in, no inspector image, a manifest recovered from VOSpace
        // — are not jobs.
        let body = run_discovery(SOURCE);
        let at = body
            .find("let params = SessionLaunchParams")
            .expect("the launch parameters are gone");
        let attempted = &body[at..];

        let terminals: Vec<usize> = attempted
            .match_indices("return self.fail(")
            .chain(attempted.match_indices("return DiscoveryOutcome::Manifest("))
            .map(|(i, _)| i)
            .collect();
        assert!(
            terminals.len() >= 6,
            "only {} terminal paths after launch — did they move?",
            terminals.len()
        );

        let mut sorted = terminals;
        sorted.sort_unstable();
        let mut previous = 0usize;
        for terminal in sorted {
            assert!(
                attempted[previous..terminal].contains("self.remember("),
                "a job outcome at byte {terminal} of run_discovery is never \
                 recorded in the history"
            );
            previous = terminal;
        }
    }

    #[test]
    fn the_probe_is_launched_by_path_not_inline() {
        // Skaha reads a single `args` value, so `bash -c <script>` arrives as
        // `bash -c` and dies with "option requires an argument". There is no
        // inline form that survives — one value holding the whole script would
        // be split on whitespace at the far end. The reference uploads the
        // script and passes its path, which is one argument with no spaces in
        // it, and is why the script names have been content-hashed all along.
        let body = run_discovery(SOURCE);
        assert!(
            body.contains("ensure_uploaded("),
            "the probe script is no longer staged before launch"
        );
        assert!(
            !body.contains(r#""-c""#),
            "the probe is being passed inline again"
        );
        assert!(
            body.contains("env: Some(format!(\"{env_name}={image_id}\"))"),
            "the script's image id is no longer passed through the environment"
        );
    }

    #[test]
    fn a_staged_script_is_uploaded_once_per_body() {
        // The filename is a hash of the body, so a copy already in the user's
        // home is the same copy — re-uploading it on every inspection is a
        // round trip per image for no gain, and editing the script produces a
        // different name rather than a stale one being reused.
        let code = SOURCE;
        let at = code
            .find("async fn ensure_uploaded")
            .expect("ensure_uploaded is gone");
        let body = &code[at..(at + 2500).min(code.len())];
        assert!(body.contains("self.uploaded"), "the upload is never cached");
        assert!(
            body.contains("probe_script_name()") && body.contains("inspector_script_name()"),
            "the staged name is no longer content-hashed"
        );
    }

    #[test]
    fn a_published_manifest_is_looked_for_before_a_job_is_launched() {
        // The scripts have always published to ~/.verbinal/manifests/; on CANFAR
        // that is real storage. Launching a job without looking spends a
        // headless slot to recompute something already sitting in the user's
        // home — and, across machines or after a reinstall, something the local
        // cache cannot know about.
        let body = run_discovery(SOURCE);
        let fetch = body
            .find("fetch_manifest_if_present(")
            .expect("the published manifest is never looked for");
        let launch = body
            .find("let params = SessionLaunchParams")
            .expect("the launch parameters are gone");
        assert!(fetch < launch, "the job is launched before the cheap check");
        assert!(
            body[..launch].contains("if !force"),
            "the pre-launch recovery ignores `force`, so Refresh cannot refresh"
        );
    }

    #[test]
    fn a_late_manifest_rescues_a_job_that_looked_failed() {
        // A probe that published just after our last poll used to be a total
        // loss: failure recorded, job deleted, manifest unread. The check has
        // to come BEFORE the diagnosis, or a success is reported as a failure
        // that merely happens to have a manifest.
        let body = run_discovery(SOURCE);
        let at = body
            .find("if let Err(msg) = self.poll_until_terminal")
            .expect("the poll failure path is gone");
        let tail = &body[at..];
        let fetch = tail
            .find("fetch_manifest_if_present(")
            .expect("no late manifest fetch");
        let diagnose = tail.find("self.diagnose(").expect("no diagnosis");
        assert!(
            fetch < diagnose,
            "the job is written off before being asked"
        );
    }

    #[test]
    fn the_evidence_is_bounded() {
        // A probe that printed a megabyte of progress bars must not write a
        // megabyte into the history file.
        assert!(
            SOURCE.contains("MAX_REASON_CHARS"),
            "the failure reason is stored untrimmed"
        );
    }
}
