//! Loads and persists [`NotebookSettings`] as JSON.
//!
//! Port of the persistence half of `Services/Notebook/NotebookSettings.cs`
//! (`Load` / `Save`). The file lives at `<data_dir>/notebook_settings.json`
//! using the same `ProjectDirs` root as the rest of Verbinal
//! (`net.canfar.Verbinal`), matching [`crate::helpers::notebook_autosave`].

use crate::models::notebook_settings::NotebookSettings;
use directories::ProjectDirs;
use std::path::PathBuf;

/// JSON-backed store for notebook preferences.
pub struct NotebookSettingsService {
    path: PathBuf,
}

impl Default for NotebookSettingsService {
    fn default() -> Self {
        Self::new()
    }
}

impl NotebookSettingsService {
    /// Create a service pointing at `<data_dir>/notebook_settings.json`.
    pub fn new() -> Self {
        let path = ProjectDirs::from("net", "canfar", "Verbinal")
            .map(|d| d.data_dir().join("notebook_settings.json"))
            .unwrap_or_else(|| PathBuf::from("notebook_settings.json"));
        Self { path }
    }

    /// Create a service backed by an explicit path (used by tests).
    #[cfg(test)]
    pub fn with_path(path: PathBuf) -> Self {
        Self { path }
    }

    /// Load settings, falling back to sanitized defaults if the file is
    /// missing or unparseable. Never fails — a broken file must not stop the
    /// notebook from opening.
    pub fn load(&self) -> NotebookSettings {
        let settings = match std::fs::read_to_string(&self.path) {
            Ok(contents) => serde_json::from_str(&contents).unwrap_or_default(),
            Err(_) => NotebookSettings::default(),
        };
        settings.sanitized()
    }

    /// Persist settings atomically-ish (write to a tmp sibling, then rename).
    pub fn save(&self, settings: &NotebookSettings) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let json = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
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
        std::env::temp_dir().join(format!("verbinal_nb_settings_{tag}_{n}.json"))
    }

    #[test]
    fn missing_file_loads_defaults() {
        let p = temp_path("missing");
        let _ = std::fs::remove_file(&p);
        let svc = NotebookSettingsService::with_path(p.clone());
        assert_eq!(svc.load(), NotebookSettings::default());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn save_then_load_round_trips() {
        let p = temp_path("round");
        let svc = NotebookSettingsService::with_path(p.clone());
        let settings = NotebookSettings {
            python_path: Some("/usr/bin/python3.12".to_string()),
            font_size: 15,
            tab_size: 2,
            word_wrap: false,
            autosave_enabled: false,
            autosave_interval_secs: 60,
            execution_timeout_secs: 120,
            show_toolbar: false,
            max_open_file_mb: 128,
            agent_image_max_dimension: 1024,
            agent_image_max_bytes_mb: 16,
            agent_result_max_kb: 64,
        };
        svc.save(&settings).expect("save");
        let back = svc.load();
        assert_eq!(back, settings);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn corrupt_file_falls_back_to_defaults() {
        let p = temp_path("corrupt");
        std::fs::write(&p, "{ not valid json ]").expect("write");
        let svc = NotebookSettingsService::with_path(p.clone());
        assert_eq!(svc.load(), NotebookSettings::default());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn load_sanitizes_out_of_range_values() {
        let p = temp_path("sanitize");
        std::fs::write(&p, r#"{"font_size":0,"tab_size":500}"#).expect("write");
        let svc = NotebookSettingsService::with_path(p.clone());
        let s = svc.load();
        assert_eq!(s.font_size, 6);
        assert_eq!(s.tab_size, 16);
        let _ = std::fs::remove_file(&p);
    }
}
