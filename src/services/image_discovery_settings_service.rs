//! Persists [`ImageDiscoverySettings`] and the registry secret.
//!
//! Port of `Services/ImageDiscovery/ImageDiscoverySettingsService.cs`. The
//! non-secret knobs (registry host/repository, username, inspector image) are
//! stored as JSON at `ProjectDirs("net","canfar","Verbinal").data_dir()/
//! image_discovery_settings.json`. The registry secret is stored in the OS
//! keychain via [`keyring`], keyed `host:username` (so multi-account users keep
//! distinct credentials) — mirroring [`crate::services::token_storage`].
//!
//! The service hands the resolved inspector image and the
//! `x-skaha-registry-auth` header to the discovery coordinator, and can verify
//! the stored credentials against the registry
//! ([`ImageDiscoverySettingsService::test_registry_credentials`]).

use crate::helpers::registry_credential_test::{test_registry_credentials, CredTestResult};
use crate::models::image_discovery_settings::{
    ImageDiscoverySettings, DEFAULT_INSPECTOR_IMAGE, DEFAULT_REGISTRY_HOST,
};
use directories::ProjectDirs;
use keyring::Entry;
use std::path::PathBuf;

/// Keychain service name for image-discovery registry secrets (kept distinct
/// from the CADC auth secrets in `token_storage`).
const KEYRING_SERVICE: &str = "canfar-verbinal-image-discovery";

/// JSON + keychain backed store for image-discovery preferences.
pub struct ImageDiscoverySettingsService {
    path: PathBuf,
    settings: ImageDiscoverySettings,
}

impl Default for ImageDiscoverySettingsService {
    fn default() -> Self {
        Self::new()
    }
}

impl ImageDiscoverySettingsService {
    /// Create a service pointing at `<data_dir>/image_discovery_settings.json`,
    /// loading any persisted settings (defaults on missing/corrupt file).
    pub fn new() -> Self {
        let path = ProjectDirs::from("net", "canfar", "Verbinal")
            .map(|d| d.data_dir().join("image_discovery_settings.json"))
            .unwrap_or_else(|| PathBuf::from("image_discovery_settings.json"));
        let settings = Self::load_from(&path);
        Self { path, settings }
    }

    /// Create a service backed by an explicit path (used by tests). The secret
    /// still lives in the OS keychain.
    #[cfg(test)]
    pub fn with_path(path: PathBuf) -> Self {
        let settings = Self::load_from(&path);
        Self { path, settings }
    }

    /// The current (non-secret) settings.
    pub fn settings(&self) -> &ImageDiscoverySettings {
        &self.settings
    }

    /// The inspector host image to launch, expanded to a full registry
    /// reference (short names are prefixed with the configured host/repository).
    pub fn resolve_inspector_image(&self) -> String {
        self.settings.resolve_inspector_image()
    }

    /// The `x-skaha-registry-auth` header value (`base64(username:secret)`), or
    /// `None` when no username/secret is configured.
    pub fn current_auth_header(&self) -> Option<String> {
        if self.settings.username.is_empty() {
            return None;
        }
        let secret = self.read_secret()?;
        Some(ImageDiscoverySettings::build_auth_header(
            &self.settings.username,
            &secret,
        ))
    }

    /// True when a non-empty secret is stored for the current host+username.
    pub fn has_secret(&self) -> bool {
        self.read_secret().is_some()
    }

    /// Set the inspector image (blank resets it to the default).
    pub fn set_inspector_image(&mut self, value: &str) {
        let v = value.trim();
        self.settings.inspector_image = if v.is_empty() {
            DEFAULT_INSPECTOR_IMAGE.to_string()
        } else {
            v.to_string()
        };
        let _ = self.save();
    }

    /// Set the registry host (blank resets it to the default).
    pub fn set_registry_host(&mut self, value: &str) {
        let v = value.trim();
        self.settings.registry_host = if v.is_empty() {
            DEFAULT_REGISTRY_HOST.to_string()
        } else {
            v.to_string()
        };
        let _ = self.save();
    }

    /// Set the registry repository/project (surrounding slashes trimmed).
    pub fn set_registry_repository(&mut self, value: &str) {
        self.settings.registry_repository = value.trim().trim_matches('/').to_string();
        let _ = self.save();
    }

    /// Set the registry username.
    pub fn set_username(&mut self, value: &str) {
        self.settings.username = value.trim().to_string();
        let _ = self.save();
    }

    /// Store (or, when blank, clear) the registry secret for the current
    /// host+username. Errors if a non-empty secret is set with no username, or
    /// if the OS keychain is unavailable.
    pub fn set_secret(&self, value: &str) -> Result<(), String> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            self.clear_secret();
            return Ok(());
        }
        if self.settings.username.is_empty() {
            return Err("Set a registry username before storing a secret.".to_string());
        }
        let entry = self
            .secret_entry(&self.settings.username)
            .ok_or_else(|| "OS keychain is unavailable.".to_string())?;
        entry.set_password(trimmed).map_err(|e| e.to_string())
    }

    /// Remove the stored secret for the current host+username (best-effort).
    pub fn clear_secret(&self) {
        if let Some(entry) = self.secret_entry(&self.settings.username) {
            let _ = entry.delete_credential();
        }
    }

    /// Clear the secret and reset all settings to defaults (persisting them).
    pub fn reset_to_defaults(&mut self) {
        self.clear_secret();
        self.settings = ImageDiscoverySettings::default();
        let _ = self.save();
    }

    /// Verify the stored credentials against the configured registry
    /// (Docker V2 token-auth). Uses a plain client — never the CADC token.
    pub async fn test_registry_credentials(&self) -> CredTestResult {
        let secret = self.read_secret().unwrap_or_default();
        test_registry_credentials(
            &self.settings.registry_host,
            &self.settings.username,
            &secret,
        )
        .await
    }

    // -- internals ----------------------------------------------------------

    /// Keychain entry for `host:username`, or `None` if the keychain is
    /// unavailable or the username is empty.
    fn secret_entry(&self, username: &str) -> Option<Entry> {
        if username.is_empty() {
            return None;
        }
        let account = format!("{}:{}", self.settings.registry_host, username);
        Entry::new(KEYRING_SERVICE, &account).ok()
    }

    /// Read the stored secret for the current host+username, filtering empties.
    fn read_secret(&self) -> Option<String> {
        let entry = self.secret_entry(&self.settings.username)?;
        entry.get_password().ok().filter(|s| !s.is_empty())
    }

    fn load_from(path: &PathBuf) -> ImageDiscoverySettings {
        match std::fs::read_to_string(path) {
            Ok(contents) => serde_json::from_str(&contents).unwrap_or_default(),
            Err(_) => ImageDiscoverySettings::default(),
        }
    }

    /// Persist non-secret settings atomically-ish (write tmp sibling, rename).
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("verbinal_img_disc_settings_{tag}_{n}.json"))
    }

    #[test]
    fn missing_file_loads_defaults() {
        let p = temp_path("missing");
        let _ = std::fs::remove_file(&p);
        let svc = ImageDiscoverySettingsService::with_path(p.clone());
        assert_eq!(*svc.settings(), ImageDiscoverySettings::default());
        assert!(svc.settings().is_all_defaults());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn setters_persist_and_round_trip() {
        let p = temp_path("round");
        let mut svc = ImageDiscoverySettingsService::with_path(p.clone());
        svc.set_registry_host("harbor.example.org");
        svc.set_registry_repository("/team/");
        svc.set_username("  alice  ");
        svc.set_inspector_image("skaha/astroml:24.07");

        // Reload from disk into a fresh service.
        let svc2 = ImageDiscoverySettingsService::with_path(p.clone());
        let s = svc2.settings();
        assert_eq!(s.registry_host, "harbor.example.org");
        assert_eq!(s.registry_repository, "team"); // slashes trimmed
        assert_eq!(s.username, "alice"); // whitespace trimmed
        assert_eq!(s.inspector_image, "skaha/astroml:24.07");
        assert!(!s.is_all_defaults());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn blank_setters_restore_defaults() {
        let p = temp_path("blank");
        let mut svc = ImageDiscoverySettingsService::with_path(p.clone());
        svc.set_registry_host("");
        svc.set_inspector_image("   ");
        let s = svc.settings();
        assert_eq!(s.registry_host, DEFAULT_REGISTRY_HOST);
        assert_eq!(s.inspector_image, DEFAULT_INSPECTOR_IMAGE);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn corrupt_file_falls_back_to_defaults() {
        let p = temp_path("corrupt");
        std::fs::write(&p, "{ not valid json ]").expect("write");
        let svc = ImageDiscoverySettingsService::with_path(p.clone());
        assert_eq!(*svc.settings(), ImageDiscoverySettings::default());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn no_username_means_no_auth_header() {
        let p = temp_path("noauth");
        let svc = ImageDiscoverySettingsService::with_path(p.clone());
        // Default settings have an empty username → never touches the keychain.
        assert!(svc.current_auth_header().is_none());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn reset_to_defaults_restores_settings() {
        let p = temp_path("reset");
        let mut svc = ImageDiscoverySettingsService::with_path(p.clone());
        svc.set_username("bob");
        svc.set_registry_repository("skaha");
        assert!(!svc.settings().is_all_defaults());
        svc.reset_to_defaults();
        assert_eq!(*svc.settings(), ImageDiscoverySettings::default());
        assert!(svc.settings().is_all_defaults());
        // Persisted file is defaults too.
        let reloaded = ImageDiscoverySettingsService::with_path(p.clone());
        assert!(reloaded.settings().is_all_defaults());
        let _ = std::fs::remove_file(&p);
    }
}
