//! Persisted MCP UI preferences + Portal session-launch defaults.
//!
//! Linux port of `Mcp/McpSettingsService.cs` (the agent-autonomy knobs) plus the
//! Portal-tab defaults that live in `Services/SettingsService.cs` on Windows
//! (`DefaultResourceType` / `DefaultGpus`). The Windows app keeps both in
//! `LocalSettings`; here each is a small JSON document under the shared
//! `ProjectDirs("net","canfar","Verbinal").data_dir()` root, mirroring
//! [`crate::services::notebook_settings_service`] and
//! [`crate::mcp::client_approval`].
//!
//! Two independent stores are exposed:
//!
//! * [`McpSettingsService`] → `mcp_settings.json`
//!   `{ "auto_apply_enabled": true, "follow_activity_enabled": true, "show_ai_guide_tile": true }`
//! * [`PortalDefaultsService`] → `portal_defaults.json`
//!   `{ "default_resource_type": "none", "default_gpus": 0 }`
//!
//! The Portal defaults for session type / cores / RAM already live in
//! [`crate::config::AppConfig`]; `AppConfig` has no `default_resource_type` /
//! `default_gpus` fields (grepped), so per the integration note those two knobs
//! are persisted here, adjacent to the MCP settings, rather than by widening
//! `AppConfig`.
//!
//! Both services hold their state behind a `RefCell` (single UI thread; shared
//! via `Rc`) and persist best-effort on every setter — a write error leaves the
//! in-memory value authoritative, matching the C# void property setters.

use directories::ProjectDirs;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Shared JSON helpers
// ---------------------------------------------------------------------------

/// Resolve `<data_dir>/<file>`, falling back to a relative path when the
/// platform dirs can't be resolved (headless/test).
fn data_path(file: &str) -> PathBuf {
    ProjectDirs::from("net", "canfar", "Verbinal")
        .map(|d| d.data_dir().join(file))
        .unwrap_or_else(|| PathBuf::from(file))
}

/// Read + deserialize `path`, falling back to `Default` on a missing or corrupt
/// file. Never fails — a broken preferences file must not stop settings loading.
fn read_or_default<T: DeserializeOwned + Default>(path: &PathBuf) -> T {
    match std::fs::read_to_string(path) {
        Ok(raw) => serde_json::from_str(&raw).unwrap_or_default(),
        Err(_) => T::default(),
    }
}

/// Best-effort atomic persist (write to a `.tmp` sibling, then rename).
fn write_json<T: Serialize>(path: &PathBuf, value: &T) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// MCP agent-autonomy settings
// ---------------------------------------------------------------------------

/// The persisted MCP UI knobs. Every field defaults to `true`, matching the C#
/// `McpSettingsService` property defaults (`#[serde(default)]` fills a missing
/// field from this `Default`, so partial documents round-trip cleanly).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct McpSettings {
    /// Agent write proposals auto-apply (vs. queue for review). Default true.
    pub auto_apply_enabled: bool,
    /// Navigate the user to the touched view after an applied write. Default true.
    pub follow_activity_enabled: bool,
    /// Show the AI-Guide tile on the landing launchpad. Default true.
    pub show_ai_guide_tile: bool,
    /// Whether the MCP server was left enabled — so it is auto-started on the next
    /// app launch (an AI client can then connect without the user re-enabling it).
    /// Default false (opt-in).
    pub server_enabled: bool,
}

impl Default for McpSettings {
    fn default() -> Self {
        McpSettings {
            auto_apply_enabled: true,
            follow_activity_enabled: true,
            show_ai_guide_tile: true,
            server_enabled: false,
        }
    }
}

/// JSON-backed store for [`McpSettings`]. Cheap to wrap in an `Rc` and clone into
/// the settings-page signal closures; each setter persists immediately.
pub struct McpSettingsService {
    path: PathBuf,
    state: RefCell<McpSettings>,
}

impl Default for McpSettingsService {
    fn default() -> Self {
        Self::new()
    }
}

impl McpSettingsService {
    /// Load from `<data_dir>/mcp_settings.json`.
    pub fn new() -> Self {
        Self::with_path(data_path("mcp_settings.json"))
    }

    /// Load from an explicit path (a test seam; also usable for a custom file).
    pub fn with_path(path: PathBuf) -> Self {
        let state = RefCell::new(read_or_default(&path));
        McpSettingsService { path, state }
    }

    /// A clone of the current settings.
    pub fn settings(&self) -> McpSettings {
        self.state.borrow().clone()
    }

    pub fn auto_apply_enabled(&self) -> bool {
        self.state.borrow().auto_apply_enabled
    }

    pub fn set_auto_apply_enabled(&self, value: bool) {
        self.state.borrow_mut().auto_apply_enabled = value;
        self.save();
    }

    pub fn follow_activity_enabled(&self) -> bool {
        self.state.borrow().follow_activity_enabled
    }

    pub fn set_follow_activity_enabled(&self, value: bool) {
        self.state.borrow_mut().follow_activity_enabled = value;
        self.save();
    }

    pub fn server_enabled(&self) -> bool {
        self.state.borrow().server_enabled
    }

    pub fn set_server_enabled(&self, value: bool) {
        self.state.borrow_mut().server_enabled = value;
        self.save();
    }

    pub fn show_ai_guide_tile(&self) -> bool {
        self.state.borrow().show_ai_guide_tile
    }

    pub fn set_show_ai_guide_tile(&self, value: bool) {
        self.state.borrow_mut().show_ai_guide_tile = value;
        self.save();
    }

    /// Persist the current state. Best-effort: errors are swallowed (the in-memory
    /// value stays authoritative), mirroring the C# void setters.
    fn save(&self) {
        let _ = write_json(&self.path, &*self.state.borrow());
    }
}

// ---------------------------------------------------------------------------
// Portal session-launch defaults (resource preset + GPUs)
// ---------------------------------------------------------------------------

/// The Portal defaults not covered by [`crate::config::AppConfig`]: the resource
/// preset (`none` / `flexible` / `fixed`) and the default GPU count. Session type,
/// cores and RAM continue to live in `AppConfig`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PortalDefaults {
    /// One of `"none"`, `"flexible"`, `"fixed"`. Default `"none"`.
    pub default_resource_type: String,
    /// Default GPU count for a fixed-resource launch. Default 0.
    pub default_gpus: u32,
}

impl Default for PortalDefaults {
    fn default() -> Self {
        PortalDefaults {
            default_resource_type: "none".to_string(),
            default_gpus: 0,
        }
    }
}

/// JSON-backed store for [`PortalDefaults`], sibling to [`McpSettingsService`].
pub struct PortalDefaultsService {
    path: PathBuf,
    state: RefCell<PortalDefaults>,
}

impl Default for PortalDefaultsService {
    fn default() -> Self {
        Self::new()
    }
}

impl PortalDefaultsService {
    /// Load from `<data_dir>/portal_defaults.json`.
    pub fn new() -> Self {
        Self::with_path(data_path("portal_defaults.json"))
    }

    /// Load from an explicit path (a test seam).
    pub fn with_path(path: PathBuf) -> Self {
        let state = RefCell::new(read_or_default(&path));
        PortalDefaultsService { path, state }
    }

    /// A clone of the current defaults.
    pub fn defaults(&self) -> PortalDefaults {
        self.state.borrow().clone()
    }

    pub fn resource_type(&self) -> String {
        self.state.borrow().default_resource_type.clone()
    }

    pub fn set_resource_type(&self, value: &str) {
        self.state.borrow_mut().default_resource_type = value.to_string();
        self.save();
    }

    pub fn gpus(&self) -> u32 {
        self.state.borrow().default_gpus
    }

    pub fn set_gpus(&self, value: u32) {
        self.state.borrow_mut().default_gpus = value;
        self.save();
    }

    /// Restore the built-in Portal defaults (`none` preset, 0 GPUs), matching the
    /// macOS/Windows "Clear all defaults" action for these two knobs.
    pub fn clear(&self) {
        *self.state.borrow_mut() = PortalDefaults::default();
        self.save();
    }

    fn save(&self) {
        let _ = write_json(&self.path, &*self.state.borrow());
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn temp_path(tag: &str) -> PathBuf {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!(
            "verbinal_mcp_settings_test_{tag}_{}_{nanos}_{n}.json",
            std::process::id()
        ))
    }

    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
            let _ = std::fs::remove_file(self.0.with_extension("json.tmp"));
        }
    }

    #[test]
    fn mcp_defaults_are_all_true() {
        let s = McpSettings::default();
        assert!(s.auto_apply_enabled);
        assert!(s.follow_activity_enabled);
        assert!(s.show_ai_guide_tile);
    }

    #[test]
    fn missing_file_loads_defaults() {
        let p = temp_path("missing");
        let _cleanup = Cleanup(p.clone());
        let svc = McpSettingsService::with_path(p);
        assert_eq!(svc.settings(), McpSettings::default());
    }

    #[test]
    fn mcp_setters_persist_and_reload() {
        let p = temp_path("mcp_round");
        let _cleanup = Cleanup(p.clone());
        {
            let svc = McpSettingsService::with_path(p.clone());
            svc.set_auto_apply_enabled(false);
            svc.set_follow_activity_enabled(false);
            svc.set_show_ai_guide_tile(false);
            assert!(!svc.auto_apply_enabled());
        }
        let reloaded = McpSettingsService::with_path(p);
        assert!(!reloaded.auto_apply_enabled());
        assert!(!reloaded.follow_activity_enabled());
        assert!(!reloaded.show_ai_guide_tile());
    }

    #[test]
    fn partial_json_fills_missing_fields_from_defaults() {
        let p = temp_path("partial");
        let _cleanup = Cleanup(p.clone());
        std::fs::write(&p, r#"{"auto_apply_enabled":false}"#).unwrap();
        let svc = McpSettingsService::with_path(p);
        // The one present field wins; the absent ones fall back to (true) defaults.
        assert!(!svc.auto_apply_enabled());
        assert!(svc.follow_activity_enabled());
        assert!(svc.show_ai_guide_tile());
    }

    #[test]
    fn corrupt_file_falls_back_to_defaults() {
        let p = temp_path("corrupt");
        let _cleanup = Cleanup(p.clone());
        std::fs::write(&p, b"{ not json ]").unwrap();
        let svc = McpSettingsService::with_path(p);
        assert_eq!(svc.settings(), McpSettings::default());
    }

    #[test]
    fn portal_defaults_are_none_and_zero() {
        let d = PortalDefaults::default();
        assert_eq!(d.default_resource_type, "none");
        assert_eq!(d.default_gpus, 0);
    }

    #[test]
    fn portal_setters_persist_and_clear_restores_defaults() {
        let p = temp_path("portal_round");
        let _cleanup = Cleanup(p.clone());
        {
            let svc = PortalDefaultsService::with_path(p.clone());
            svc.set_resource_type("fixed");
            svc.set_gpus(4);
        }
        let svc = PortalDefaultsService::with_path(p);
        assert_eq!(svc.resource_type(), "fixed");
        assert_eq!(svc.gpus(), 4);

        svc.clear();
        assert_eq!(svc.resource_type(), "none");
        assert_eq!(svc.gpus(), 0);
        // The cleared state persisted, too.
        assert_eq!(svc.defaults(), PortalDefaults::default());
    }
}
