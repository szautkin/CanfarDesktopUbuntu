//! Persists Terms-of-Use acceptance (version + UTC timestamp) to a small JSON
//! file under the app config dir.
//!
//! Port of `Services/LegalAgreementService.cs`. Where the Windows client used
//! `ApplicationData.LocalSettings`, this uses a `legal_acceptance.json` file
//! alongside `settings.json` (same `ProjectDirs` layout as [`SettingsService`]).
//! Degrades gracefully: an unreadable/absent file simply means "not yet
//! accepted", and write failures never surface to the caller (the gate will
//! re-appear next launch rather than crash).
//!
//! [`SettingsService`]: crate::services::settings_service::SettingsService

use crate::helpers::legal_terms;
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct LegalRecord {
    /// Highest terms version the user has accepted, if any.
    #[serde(default)]
    accepted_version: Option<u32>,
    /// UTC timestamp (RFC 3339) of the acceptance, for audit / support.
    #[serde(default)]
    accepted_utc: Option<String>,
}

/// Reads and records Terms-of-Use acceptance.
pub struct LegalAgreementService {
    path: PathBuf,
}

impl LegalAgreementService {
    /// Construct pointing at `<config>/legal_acceptance.json`.
    pub fn new() -> Self {
        let path = ProjectDirs::from("net", "canfar", "Verbinal")
            .map(|dirs| dirs.config_dir().join("legal_acceptance.json"))
            .unwrap_or_else(|| PathBuf::from("legal_acceptance.json"));
        Self { path }
    }

    /// Construct against an explicit path (used by tests).
    #[cfg(test)]
    pub fn with_path(path: PathBuf) -> Self {
        Self { path }
    }

    /// The version the running build asks users to accept.
    #[allow(dead_code)]
    pub fn current_version(&self) -> u32 {
        legal_terms::CURRENT_VERSION
    }

    fn read(&self) -> LegalRecord {
        std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    fn accepted_version(&self) -> Option<u32> {
        self.read().accepted_version
    }

    /// True if a stored acceptance covers the current terms version.
    pub fn has_accepted_current(&self) -> bool {
        legal_terms::is_accepted(self.accepted_version())
    }

    /// True when the blocking gate must be shown: no acceptance on record, or the
    /// terms have been bumped past the previously-accepted version.
    pub fn needs_acceptance(&self) -> bool {
        !self.has_accepted_current()
    }

    /// Record acceptance of the current terms version (with a UTC timestamp).
    /// Best-effort: I/O errors are swallowed so the UI flow always proceeds.
    pub fn accept(&self) {
        let record = LegalRecord {
            accepted_version: Some(legal_terms::CURRENT_VERSION),
            accepted_utc: Some(chrono::Utc::now().to_rfc3339()),
        };
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(&record) {
            let _ = std::fs::write(&self.path, json);
        }
    }
}

impl Default for LegalAgreementService {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(tag: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        let uniq = format!(
            "verbinal_legal_test_{}_{}_{:?}.json",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        p.push(uniq);
        p
    }

    #[test]
    fn fresh_install_needs_acceptance() {
        let path = temp_path("fresh");
        let _ = std::fs::remove_file(&path);
        let svc = LegalAgreementService::with_path(path.clone());
        assert!(svc.needs_acceptance());
        assert!(!svc.has_accepted_current());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn accept_persists_and_clears_gate() {
        let path = temp_path("accept");
        let _ = std::fs::remove_file(&path);
        let svc = LegalAgreementService::with_path(path.clone());
        svc.accept();
        assert!(svc.has_accepted_current());
        assert!(!svc.needs_acceptance());

        // A brand-new instance over the same file sees the persisted acceptance.
        let reopened = LegalAgreementService::with_path(path.clone());
        assert!(reopened.has_accepted_current());

        // The persisted record carries a timestamp.
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("accepted_utc"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn version_bump_reprompts() {
        let path = temp_path("bump");
        // Simulate an acceptance recorded against an older terms version.
        let stale = format!(
            "{{\"accepted_version\": {}, \"accepted_utc\": \"2020-01-01T00:00:00Z\"}}",
            legal_terms::CURRENT_VERSION.saturating_sub(1)
        );
        std::fs::write(&path, stale).unwrap();
        let svc = LegalAgreementService::with_path(path.clone());
        if legal_terms::CURRENT_VERSION > 0 {
            assert!(
                svc.needs_acceptance(),
                "an acceptance older than the current terms version must re-prompt"
            );
        }
        // Accepting brings it current again.
        svc.accept();
        assert!(!svc.needs_acceptance());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn corrupt_file_is_treated_as_unaccepted() {
        let path = temp_path("corrupt");
        std::fs::write(&path, "not json {{{").unwrap();
        let svc = LegalAgreementService::with_path(path.clone());
        assert!(svc.needs_acceptance());
        let _ = std::fs::remove_file(&path);
    }
}
