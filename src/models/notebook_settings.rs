//! Persisted notebook preferences.
//!
//! Port of `Services/Notebook/NotebookSettings.cs`. A plain serde struct with
//! sensible defaults, saved as JSON by
//! [`crate::services::notebook_settings_service::NotebookSettingsService`].
//!
//! `#[serde(default)]` on the container means a settings file written by an
//! older build (missing newer fields) still loads cleanly — each absent field
//! falls back to its [`Default`] value rather than failing the whole parse.

use serde::{Deserialize, Serialize};

/// User-configurable notebook editor / execution preferences.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NotebookSettings {
    /// Explicit Python interpreter path. `None` means auto-detect.
    pub python_path: Option<String>,
    /// Code-cell font size in points/pixels.
    pub font_size: u32,
    /// Tab width in spaces.
    pub tab_size: u32,
    /// Whether code cells wrap long lines.
    pub word_wrap: bool,
    /// Whether the periodic autosave checkpoint is enabled.
    pub autosave_enabled: bool,
    /// Autosave interval in seconds.
    pub autosave_interval_secs: u32,
    /// Execution timeout warning threshold in seconds (`0` means never warn).
    pub execution_timeout_secs: u32,
    /// Whether the notebook toolbar is shown.
    pub show_toolbar: bool,
}

impl Default for NotebookSettings {
    fn default() -> Self {
        // Mirrors the defaults in the C# reference.
        Self {
            python_path: None,
            font_size: 13,
            tab_size: 4,
            word_wrap: true,
            autosave_enabled: true,
            autosave_interval_secs: 30,
            execution_timeout_secs: 60,
            show_toolbar: true,
        }
    }
}

impl NotebookSettings {
    /// Return a copy with all numeric fields clamped to sane ranges, so a
    /// hand-edited or corrupt settings file can never push absurd values into
    /// the UI (e.g. a 0px font). Applied on load.
    pub fn sanitized(mut self) -> Self {
        self.font_size = self.font_size.clamp(6, 48);
        self.tab_size = self.tab_size.clamp(1, 16);
        // autosave interval: at least 5s, at most an hour.
        self.autosave_interval_secs = self.autosave_interval_secs.clamp(5, 3600);
        // execution timeout: 0 (never) is allowed, otherwise cap at an hour.
        if self.execution_timeout_secs != 0 {
            self.execution_timeout_secs = self.execution_timeout_secs.clamp(5, 3600);
        }
        // Normalise an all-whitespace python path to None.
        if let Some(p) = &self.python_path {
            if p.trim().is_empty() {
                self.python_path = None;
            }
        }
        self
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_reference() {
        let s = NotebookSettings::default();
        assert_eq!(s.font_size, 13);
        assert_eq!(s.tab_size, 4);
        assert!(s.word_wrap);
        assert!(s.autosave_enabled);
        assert_eq!(s.autosave_interval_secs, 30);
        assert_eq!(s.execution_timeout_secs, 60);
        assert!(s.show_toolbar);
        assert_eq!(s.python_path, None);
    }

    #[test]
    fn serde_round_trip_preserves_all_fields() {
        let s = NotebookSettings {
            python_path: Some("/opt/py/bin/python3".to_string()),
            font_size: 16,
            tab_size: 2,
            word_wrap: false,
            autosave_enabled: false,
            autosave_interval_secs: 60,
            execution_timeout_secs: 0,
            show_toolbar: false,
        };
        let json = serde_json::to_string_pretty(&s).expect("serialise");
        let back: NotebookSettings = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(s, back);
    }

    #[test]
    fn partial_json_fills_defaults() {
        // Only two fields present — the rest must fall back to defaults.
        let json = r#"{"font_size":20,"word_wrap":false}"#;
        let s: NotebookSettings = serde_json::from_str(json).expect("deserialise");
        assert_eq!(s.font_size, 20);
        assert!(!s.word_wrap);
        // Untouched fields keep their defaults.
        assert_eq!(s.tab_size, 4);
        assert_eq!(s.autosave_interval_secs, 30);
        assert!(s.show_toolbar);
    }

    #[test]
    fn empty_json_object_is_all_defaults() {
        let s: NotebookSettings = serde_json::from_str("{}").expect("deserialise");
        assert_eq!(s, NotebookSettings::default());
    }

    #[test]
    fn sanitized_clamps_absurd_values() {
        let s = NotebookSettings {
            font_size: 0,
            tab_size: 999,
            autosave_interval_secs: 1,
            execution_timeout_secs: 100_000,
            python_path: Some("   ".to_string()),
            ..Default::default()
        }
        .sanitized();
        assert_eq!(s.font_size, 6);
        assert_eq!(s.tab_size, 16);
        assert_eq!(s.autosave_interval_secs, 5);
        assert_eq!(s.execution_timeout_secs, 3600);
        assert_eq!(s.python_path, None);
    }

    #[test]
    fn sanitized_allows_never_timeout() {
        let s = NotebookSettings {
            execution_timeout_secs: 0,
            ..Default::default()
        }
        .sanitized();
        assert_eq!(s.execution_timeout_secs, 0);
    }
}
