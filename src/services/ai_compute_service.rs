//! AI Compute service: runs agent-authored code on remote compute via the
//! file-drop RPC the external `verbinal-execution` watcher consumes.
//!
//! Port of `Services/AICompute/{AIComputeService,AIComputeSettingsService}.cs`,
//! folded into one Rust service (there is no separate settings service file):
//! the non-secret knobs persist as JSON at
//! `ProjectDirs("net","canfar","Verbinal").data_dir()/ai_compute_settings.json`,
//! and the registry secret lives in the OS keychain under a service name
//! DISTINCT from Image Discovery, so the two credential sets never collide.
//!
//! The runtime flow reuses the existing session + VOSpace services (no new HTTP
//! plumbing): reuse (or lazily launch, without waiting for Running) one
//! `contributed` session named `verbinal-compute`, PUT the request JSON to the
//! shared `/arc` inbox, and poll the out file. The service holds no live session
//! state — the warm session is discovered on Skaha BY NAME each call, so a fresh
//! instance is always correct.

use crate::models::ai_compute::{
    AIComputeSettings, RunCodeContract, RunCodeJson, RunCodeRequest, RunCodeResult,
};
use crate::models::{Session, SessionLaunchParams};
use crate::services::api_error::ApiError;
use crate::state::AppServices;
use directories::ProjectDirs;
use keyring::Entry;
use std::path::PathBuf;

/// Keychain service name for AI-compute registry secrets (kept DISTINCT from the
/// Image Discovery secrets in `image_discovery_settings_service`).
const KEYRING_SERVICE: &str = "canfar-verbinal-ai-compute";

/// JSON + keychain backed AI-compute settings plus the run_code file-drop RPC.
pub struct AIComputeService {
    path: PathBuf,
    settings: AIComputeSettings,
}

impl Default for AIComputeService {
    fn default() -> Self {
        Self::new()
    }
}

impl AIComputeService {
    /// Create a service pointing at `<data_dir>/ai_compute_settings.json`, loading
    /// any persisted settings (defaults on missing/corrupt file) and refreshing
    /// [`AIComputeSettings::has_secret`] from the keychain.
    pub fn new() -> Self {
        let path = ProjectDirs::from("net", "canfar", "Verbinal")
            .map(|d| d.data_dir().join("ai_compute_settings.json"))
            .unwrap_or_else(|| PathBuf::from("ai_compute_settings.json"));
        Self::with_path(path)
    }

    /// Create a service backed by an explicit path (used by tests). The secret
    /// still lives in the OS keychain.
    pub fn with_path(path: PathBuf) -> Self {
        let mut settings = Self::load_from(&path);
        settings.has_secret =
            Self::read_secret(&settings.registry_host, &settings.registry_username).is_some();
        Self { path, settings }
    }

    // -- settings accessors -------------------------------------------------

    pub fn settings(&self) -> &AIComputeSettings {
        &self.settings
    }

    /// The compute image to launch, expanded to a full registry reference. Empty
    /// when run_code is disabled.
    pub fn resolve_image(&self) -> String {
        self.settings.resolve_image()
    }

    /// The clamped (cores, ram) for the lazy compute launch.
    pub fn resolve_resources(&self) -> (u32, u32) {
        self.settings.resolve_resources()
    }

    /// The (username, secret) for the contributed-session registry auth, or empty
    /// strings when none.
    pub fn registry_credentials(&self) -> (String, String) {
        let secret = Self::read_secret(
            &self.settings.registry_host,
            &self.settings.registry_username,
        )
        .unwrap_or_default();
        (self.settings.registry_username.clone(), secret)
    }

    // -- settings setters ---------------------------------------------------

    pub fn set_image(&mut self, value: &str) {
        self.settings.image = value.trim().to_string();
        let _ = self.save();
    }

    pub fn set_cores(&mut self, value: u32) {
        self.settings.cores = RunCodeContract::clamp_cores(value);
        let _ = self.save();
    }

    pub fn set_ram(&mut self, value: u32) {
        self.settings.ram = RunCodeContract::clamp_ram(value);
        let _ = self.save();
    }

    pub fn set_registry_host(&mut self, value: &str) {
        let v = value.trim();
        self.settings.registry_host = if v.is_empty() {
            crate::models::ai_compute::DEFAULT_REGISTRY_HOST.to_string()
        } else {
            v.to_string()
        };
        self.settings.has_secret = Self::read_secret(
            &self.settings.registry_host,
            &self.settings.registry_username,
        )
        .is_some();
        let _ = self.save();
    }

    pub fn set_registry_repository(&mut self, value: &str) {
        self.settings.registry_repository = value.trim().trim_matches('/').to_string();
        let _ = self.save();
    }

    pub fn set_username(&mut self, value: &str) {
        self.settings.registry_username = value.trim().to_string();
        self.settings.has_secret = Self::read_secret(
            &self.settings.registry_host,
            &self.settings.registry_username,
        )
        .is_some();
        let _ = self.save();
    }

    /// Store (or, when blank, clear) the registry secret for the current
    /// host+username. Errors if a non-empty secret is set with no username, or if
    /// the OS keychain is unavailable.
    pub fn set_secret(&mut self, value: &str) -> Result<(), String> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            self.clear_secret();
            return Ok(());
        }
        if self.settings.registry_username.is_empty() {
            return Err("Set a registry username before storing a secret.".to_string());
        }
        let entry = self
            .secret_entry(&self.settings.registry_username)
            .ok_or_else(|| "OS keychain is unavailable.".to_string())?;
        entry.set_password(trimmed).map_err(|e| e.to_string())?;
        self.settings.has_secret = true;
        Ok(())
    }

    /// Remove the stored secret for the current host+username (best-effort).
    pub fn clear_secret(&mut self) {
        if let Some(entry) = self.secret_entry(&self.settings.registry_username) {
            let _ = entry.delete_credential();
        }
        self.settings.has_secret = false;
    }

    /// Clear the secret and reset all settings to defaults (persisting them).
    pub fn reset_to_defaults(&mut self) {
        self.clear_secret();
        self.settings = AIComputeSettings::default();
        let _ = self.save();
    }

    // -- runtime: file-drop RPC --------------------------------------------

    /// Reuse the warm `verbinal-compute` session, or launch one at the configured
    /// size. Does NOT wait for Running (a contributed launch routinely takes
    /// 60–90s; the watcher re-scans the inbox on boot). Errors when no compute
    /// image is configured or the user is not signed in.
    pub async fn ensure_session(&self, services: &AppServices) -> Result<(), String> {
        let image = self.resolve_image();
        if image.is_empty() {
            return Err(
                "No AI compute image configured. Set one in Settings ▸ AI compute.".to_string(),
            );
        }
        let token = services.get_token().await.ok_or_else(sign_in_msg)?;

        if self.find_warm_session(services, &token).await?.is_some() {
            return Ok(());
        }

        let (cores, ram) = self.resolve_resources();
        let (reg_user, reg_secret) = self.registry_credentials();
        let params = SessionLaunchParams {
            name: RunCodeContract::SESSION_NAME.to_string(),
            image,
            session_type: RunCodeContract::SESSION_TYPE.to_string(),
            cores,
            ram,
            gpus: 0,
            cmd: None,
            env: None,
            // The interactive launch path honours registry_username/secret.
            registry_username: (!reg_user.is_empty()).then(|| reg_user.clone()),
            registry_secret: (!reg_secret.is_empty()).then(|| reg_secret.clone()),
            args: None,
            replicas: None,
        };
        services
            .sessions
            .launch_session(&token, &params)
            .await
            .map(|_| ())
    }

    /// Ensure the compute session, then drop the request file in the inbox.
    /// Returns the `job_ref` (execution id) without waiting for a result — the
    /// caller polls [`fetch_out`](Self::fetch_out).
    pub async fn submit(
        &self,
        services: &AppServices,
        request: &RunCodeRequest,
    ) -> Result<String, String> {
        let token = services.get_token().await.ok_or_else(sign_in_msg)?;
        let user = services.get_username().await.ok_or_else(sign_in_msg)?;

        self.ensure_session(services).await?;
        self.ensure_inbox_tree(services, &token, &user).await?;

        let json = RunCodeJson::serialize_request(request)?;
        let path = RunCodeContract::inbox_path(&user, &request.id);
        services
            .vospace
            .upload_file(&token, &user, &path, json.into_bytes(), "application/json")
            .await
            .map_err(|e| e.to_string())?;
        Ok(request.id.clone())
    }

    /// Read + parse the result file for an execution id; `Ok(None)` when it isn't
    /// ready yet (absent, 404, or a transient error — the caller polls again).
    /// Only an auth failure surfaces as `Err`.
    pub async fn fetch_out(
        &self,
        services: &AppServices,
        id: &str,
    ) -> Result<Option<RunCodeResult>, String> {
        let token = services.get_token().await.ok_or_else(sign_in_msg)?;
        let user = services.get_username().await.ok_or_else(sign_in_msg)?;
        let path = RunCodeContract::out_path(&user, id);
        match services.vospace.download_bytes(&token, &user, &path).await {
            Ok(bytes) => {
                let text = bounded_utf8(&bytes, RunCodeContract::MAX_RESULT_BYTES);
                Ok(RunCodeJson::try_parse_result(&text))
            }
            // 404 / server / network are all "not ready yet" (mirrors the C#
            // HttpRequestException → null); only auth expiry propagates.
            Err(ApiError::Unauthorized) => Err(ApiError::Unauthorized.to_string()),
            Err(_) => Ok(None),
        }
    }

    /// Stop the warm compute session (idempotent — `Ok(false)` when none is
    /// running).
    pub async fn stop(&self, services: &AppServices) -> Result<bool, String> {
        let token = services.get_token().await.ok_or_else(sign_in_msg)?;
        match self.find_warm_session(services, &token).await? {
            Some(s) => services
                .sessions
                .delete_session(&token, &s.id)
                .await
                .map(|_| true),
            None => Ok(false),
        }
    }

    // -- internals ----------------------------------------------------------

    /// Reuse by NAME + TYPE (not image — survives registry-prefix normalization);
    /// count Pending so rapid cold-start calls don't spawn duplicates.
    async fn find_warm_session(
        &self,
        services: &AppServices,
        token: &str,
    ) -> Result<Option<Session>, String> {
        let sessions = services
            .sessions
            .get_sessions(token)
            .await
            .map_err(|e| e.to_string())?;
        Ok(sessions.into_iter().find(|s| {
            s.session_type
                .eq_ignore_ascii_case(RunCodeContract::SESSION_TYPE)
                && s.name == RunCodeContract::SESSION_NAME
                && is_live(&s.status)
        }))
    }

    /// Create the inbox folder tree one level at a time, tolerating an
    /// already-exists 409.
    async fn ensure_inbox_tree(
        &self,
        services: &AppServices,
        token: &str,
        user: &str,
    ) -> Result<(), String> {
        for level in RunCodeContract::inbox_tree_levels() {
            // `ensure_folder` owns the already-exists rule. This had its own
            // copy, matching on the 409 status alone — so a service that
            // signalled DuplicateNode with any other status would have failed
            // the whole tree.
            services
                .vospace
                .ensure_folder(token, user, level)
                .await
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    fn secret_entry(&self, username: &str) -> Option<Entry> {
        if username.is_empty() {
            return None;
        }
        let account = format!("{}:{}", self.settings.registry_host, username);
        Entry::new(KEYRING_SERVICE, &account).ok()
    }

    fn read_secret(host: &str, username: &str) -> Option<String> {
        if username.is_empty() {
            return None;
        }
        let account = format!("{host}:{username}");
        Entry::new(KEYRING_SERVICE, &account)
            .ok()?
            .get_password()
            .ok()
            .filter(|s| !s.is_empty())
    }

    fn load_from(path: &PathBuf) -> AIComputeSettings {
        match std::fs::read_to_string(path) {
            Ok(contents) => serde_json::from_str(&contents).unwrap_or_default(),
            Err(_) => AIComputeSettings::default(),
        }
    }

    /// Persist non-secret settings (write tmp sibling, rename).
    fn save(&self) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let json = serde_json::to_string_pretty(&self.settings).map_err(|e| e.to_string())?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, json).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &self.path).map_err(|e| e.to_string())
    }
}

/// The status strings that count as a live (reusable) session.
fn is_live(status: &str) -> bool {
    status.eq_ignore_ascii_case("Running") || status.eq_ignore_ascii_case("Pending")
}

fn sign_in_msg() -> String {
    "Sign in to CANFAR before using run_code.".to_string()
}

/// UTF-8 (lossy) decode of at most `max_bytes` of `bytes` — the watcher caps the
/// result file, and a mid-write read never over-reads.
fn bounded_utf8(bytes: &[u8], max_bytes: usize) -> String {
    let end = bytes.len().min(max_bytes);
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests (pure bits only — the network flow can't be live-tested here)
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn temp_path(tag: &str) -> PathBuf {
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("verbinal_ai_compute_{tag}_{n}.json"))
    }

    #[test]
    fn missing_file_loads_disabled_defaults() {
        let p = temp_path("missing");
        let _ = std::fs::remove_file(&p);
        let svc = AIComputeService::with_path(p.clone());
        assert!(!svc.settings().is_enabled());
        assert!(svc.settings().is_all_defaults());
        assert_eq!(svc.resolve_image(), "");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn setters_persist_and_round_trip() {
        let p = temp_path("round");
        let mut svc = AIComputeService::with_path(p.clone());
        svc.set_image("  verbinal-compute:1.0  ");
        svc.set_cores(0); // clamps to 1
        svc.set_ram(9999); // clamps to 256
        svc.set_registry_repository("/project/");
        svc.set_username("  alice  ");

        let svc2 = AIComputeService::with_path(p.clone());
        let s = svc2.settings();
        assert_eq!(s.image, "verbinal-compute:1.0");
        assert_eq!(s.cores, 1);
        assert_eq!(s.ram, 256);
        assert_eq!(s.registry_repository, "project");
        assert_eq!(s.registry_username, "alice");
        assert!(s.is_enabled());
        assert_eq!(
            svc2.resolve_image(),
            "images.canfar.net/project/verbinal-compute:1.0"
        );
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn corrupt_file_falls_back_to_defaults() {
        let p = temp_path("corrupt");
        std::fs::write(&p, "{ not valid json ]").unwrap();
        let svc = AIComputeService::with_path(p.clone());
        assert!(svc.settings().is_all_defaults());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn blank_registry_host_restores_default() {
        let p = temp_path("host");
        let mut svc = AIComputeService::with_path(p.clone());
        svc.set_registry_host("   ");
        assert_eq!(
            svc.settings().registry_host,
            crate::models::ai_compute::DEFAULT_REGISTRY_HOST
        );
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn is_live_only_running_or_pending() {
        assert!(is_live("Running"));
        assert!(is_live("pending"));
        assert!(!is_live("Terminating"));
        assert!(!is_live("Failed"));
    }

    #[test]
    fn bounded_utf8_caps_length() {
        let bytes = b"hello world";
        assert_eq!(bounded_utf8(bytes, 5), "hello");
        assert_eq!(bounded_utf8(bytes, 100), "hello world");
    }
}
