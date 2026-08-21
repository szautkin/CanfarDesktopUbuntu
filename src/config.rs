use serde::{Deserialize, Serialize};
use std::sync::RwLock;

/// Single source of truth for the eight editable CANFAR/CADC service bases.
///
/// Mirrors `Helpers/ApiEndpointDefaults.cs` in CanfarDesktop. Every request URL
/// in the app derives from one of these bases so that a user can repoint the
/// app at a staging/alternate deployment from Settings.
pub mod api_endpoint_defaults {
    /// CADC login/whoami (`ac`). Login + session identity live here.
    pub const LOGIN_BASE: &str = "https://ws-cadc.canfar.net/ac";
    /// Skaha science-platform API (sessions, images, context, stats).
    pub const SKAHA_BASE: &str = "https://ws-uv.canfar.net/skaha";
    /// Access-control / user-info service (`ac`) on the UV host.
    pub const AC_BASE: &str = "https://ws-uv.canfar.net/ac";
    /// VOSpace metadata nodes (`arc/nodes`).
    pub const ARC_NODES: &str = "https://ws-uv.canfar.net/arc/nodes";
    /// VOSpace file transfer (`arc/files`).
    pub const ARC_FILES: &str = "https://ws-uv.canfar.net/arc/files";
    /// TAP archive-search service (`argus`).
    pub const TAP_BASE: &str = "https://ws.cadc-ccda.hia-iha.nrc-cnrc.gc.ca/argus";
    /// CAOM2 operations (DataLink + package download).
    pub const CAOM2OPS_BASE: &str = "https://ws.cadc-ccda.hia-iha.nrc-cnrc.gc.ca/caom2ops";
    /// CADC target-name resolver.
    pub const RESOLVER_BASE: &str =
        "https://ws.cadc-ccda.hia-iha.nrc-cnrc.gc.ca/cadc-target-resolver";
    /// VizieR TAP mirrors, tried in order, whitespace-separated.
    ///
    /// A list rather than one base because VizieR is one operator with several
    /// front doors, and they do not fail together — a search that finds the
    /// first two down should still answer. TLS hosts first; the plain-HTTP
    /// mirror is last, for callers who cannot complete a handshake at all.
    ///
    /// Editable in Settings like every other endpoint here: these hostnames
    /// have moved before. Two of the four shipped previously
    /// (`tap.cds.unistra.fr`, `tapvizier.esac.esa.int`) had become NXDOMAIN,
    /// and being a constant in the binary meant nobody could route around it.
    pub const VIZIER_MIRRORS: &str = "https://tapvizier.cds.unistra.fr/TAPVizieR/tap/sync \
                                      https://tapvizier.u-strasbg.fr/TAPVizieR/tap/sync \
                                      http://vizier.china-vo.org/tap/sync";
}

// ---- per-field serde defaults (so old settings.json keeps loading) ----------
fn d_vizier_mirrors() -> String {
    api_endpoint_defaults::VIZIER_MIRRORS
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn d_login_base() -> String {
    api_endpoint_defaults::LOGIN_BASE.to_string()
}
fn d_skaha_base() -> String {
    api_endpoint_defaults::SKAHA_BASE.to_string()
}
fn d_ac_base() -> String {
    api_endpoint_defaults::AC_BASE.to_string()
}
fn d_arc_nodes() -> String {
    api_endpoint_defaults::ARC_NODES.to_string()
}
fn d_arc_files() -> String {
    api_endpoint_defaults::ARC_FILES.to_string()
}
fn d_tap_base() -> String {
    api_endpoint_defaults::TAP_BASE.to_string()
}
fn d_caom2ops_base() -> String {
    api_endpoint_defaults::CAOM2OPS_BASE.to_string()
}
fn d_resolver_base() -> String {
    api_endpoint_defaults::RESOLVER_BASE.to_string()
}
fn d_theme() -> String {
    "System".to_string()
}
fn d_session_type() -> String {
    "notebook".to_string()
}
fn d_cores() -> u32 {
    2
}
fn d_ram() -> u32 {
    8
}
fn d_language() -> String {
    "system".to_string()
}

/// Persisted application configuration.
///
/// The container-level `#[serde(default)]` plus per-field defaults are load-bearing:
/// without them, shipping *any* new field would make an older `settings.json` fail
/// to deserialize and silently reset every user setting. With them, unknown legacy
/// keys are ignored and missing keys fall back to their default — a clean migration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    // --- editable service endpoints (8 bases) ---
    #[serde(default = "d_login_base")]
    pub login_base: String,
    #[serde(default = "d_skaha_base")]
    pub skaha_base: String,
    #[serde(default = "d_ac_base")]
    pub ac_base: String,
    #[serde(default = "d_arc_nodes")]
    pub arc_nodes: String,
    #[serde(default = "d_arc_files")]
    pub arc_files: String,
    #[serde(default = "d_tap_base")]
    pub tap_base: String,
    #[serde(default = "d_caom2ops_base")]
    pub caom2ops_base: String,
    #[serde(default = "d_resolver_base")]
    pub resolver_base: String,
    /// VizieR TAP mirrors, tried in order (whitespace-separated).
    #[serde(default = "d_vizier_mirrors")]
    pub vizier_mirrors: String,

    // --- appearance / session defaults ---
    #[serde(default = "d_theme")]
    pub theme: String,
    #[serde(default = "d_session_type")]
    pub default_session_type: String,
    #[serde(default = "d_cores")]
    pub default_cores: u32,
    #[serde(default = "d_ram")]
    pub default_ram: u32,

    /// UI language: "system", "en", or "fr".
    #[serde(default = "d_language")]
    pub language: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        AppConfig {
            login_base: d_login_base(),
            skaha_base: d_skaha_base(),
            ac_base: d_ac_base(),
            arc_nodes: d_arc_nodes(),
            arc_files: d_arc_files(),
            tap_base: d_tap_base(),
            caom2ops_base: d_caom2ops_base(),
            resolver_base: d_resolver_base(),
            vizier_mirrors: d_vizier_mirrors(),
            theme: d_theme(),
            default_session_type: d_session_type(),
            default_cores: d_cores(),
            default_ram: d_ram(),
            language: d_language(),
        }
    }
}

impl AppConfig {
    /// Restore only the eight endpoint fields to their defaults, leaving theme,
    /// language and session defaults untouched (mirrors `SettingsService.ResetEndpoints`).
    pub fn reset_endpoints(&mut self) {
        self.login_base = d_login_base();
        self.skaha_base = d_skaha_base();
        self.ac_base = d_ac_base();
        self.arc_nodes = d_arc_nodes();
        self.arc_files = d_arc_files();
        self.tap_base = d_tap_base();
        self.caom2ops_base = d_caom2ops_base();
        self.resolver_base = d_resolver_base();
    }
}

/// A validated snapshot of the eight service bases used to build request URLs.
#[derive(Debug, Clone)]
pub struct EndpointBases {
    pub login_base: String,
    pub skaha_base: String,
    pub ac_base: String,
    pub arc_nodes: String,
    pub arc_files: String,
    pub tap_base: String,
    pub caom2ops_base: String,
    pub resolver_base: String,
    /// VizieR TAP sync URLs, in the order they should be tried.
    pub vizier_mirrors: Vec<String>,
}

/// The VizieR mirrors to try, in order.
///
/// Each entry is validated like any other base; anything that is not an
/// absolute http/https URL is dropped rather than silently prefixed onto a
/// request. An empty or wholly invalid setting falls back to the shipped list,
/// because a user who clears the field wants the defaults back, not a tool that
/// has no mirrors at all.
fn sanitize_mirrors(value: &str) -> Vec<String> {
    let parsed: Vec<String> = value
        .split_whitespace()
        .map(|m| m.trim_end_matches('/'))
        .filter(|m| m.starts_with("http://") || m.starts_with("https://"))
        .map(str::to_string)
        .collect();
    if parsed.is_empty() {
        return api_endpoint_defaults::VIZIER_MIRRORS
            .split_whitespace()
            .map(str::to_string)
            .collect();
    }
    parsed
}

/// Validate a single base URL. Trims whitespace and a trailing '/', and requires
/// an absolute http/https URL — otherwise falls back to the provided default
/// (matches `SettingsService.ApplyEndpointsTo` sanitisation).
fn sanitize_base(value: &str, default: &str) -> String {
    let trimmed = value.trim().trim_end_matches('/');
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed.to_string()
    } else {
        default.to_string()
    }
}

impl EndpointBases {
    pub fn defaults() -> Self {
        Self::from_config(&AppConfig::default())
    }

    pub fn from_config(cfg: &AppConfig) -> Self {
        use api_endpoint_defaults as d;
        EndpointBases {
            login_base: sanitize_base(&cfg.login_base, d::LOGIN_BASE),
            skaha_base: sanitize_base(&cfg.skaha_base, d::SKAHA_BASE),
            ac_base: sanitize_base(&cfg.ac_base, d::AC_BASE),
            arc_nodes: sanitize_base(&cfg.arc_nodes, d::ARC_NODES),
            arc_files: sanitize_base(&cfg.arc_files, d::ARC_FILES),
            tap_base: sanitize_base(&cfg.tap_base, d::TAP_BASE),
            caom2ops_base: sanitize_base(&cfg.caom2ops_base, d::CAOM2OPS_BASE),
            resolver_base: sanitize_base(&cfg.resolver_base, d::RESOLVER_BASE),
            vizier_mirrors: sanitize_mirrors(&cfg.vizier_mirrors),
        }
    }
}

/// Builds every request URL from the eight editable bases.
///
/// The bases live behind a `RwLock` (interior mutability) so that editing an
/// endpoint in Settings takes effect on the *next request* without a restart —
/// every service holds an `Arc<ApiEndpoints>` clone and observes the change.
/// `config` is an immutable startup snapshot used for non-endpoint settings
/// (session launch defaults).
pub struct ApiEndpoints {
    bases: RwLock<EndpointBases>,
    config: AppConfig,
}

impl ApiEndpoints {
    pub fn new(config: AppConfig) -> Self {
        let bases = EndpointBases::from_config(&config);
        ApiEndpoints {
            bases: RwLock::new(bases),
            config,
        }
    }

    /// Re-apply (validated) endpoint bases from a config. Takes effect on the next
    /// request built by any service holding this `ApiEndpoints`.
    pub fn apply_from(&self, config: &AppConfig) {
        let next = EndpointBases::from_config(config);
        *self.bases.write().unwrap() = next;
    }

    /// Restore the eight bases to their defaults (endpoints only).
    pub fn reset_endpoints(&self) {
        *self.bases.write().unwrap() = EndpointBases::defaults();
    }

    /// A clone of the current bases (for the Settings editor / self-test).
    pub fn bases_snapshot(&self) -> EndpointBases {
        self.bases.read().unwrap().clone()
    }

    // ---- auth / identity (login_base) --------------------------------------
    pub fn login_url(&self) -> String {
        format!("{}/login", self.bases.read().unwrap().login_base)
    }

    pub fn whoami_url(&self) -> String {
        format!("{}/whoami", self.bases.read().unwrap().login_base)
    }

    // ---- skaha science platform (skaha_base) -------------------------------
    pub fn sessions_url(&self) -> String {
        format!("{}/v1/session", self.bases.read().unwrap().skaha_base)
    }

    pub fn session_url(&self, session_id: &str) -> String {
        format!(
            "{}/v1/session/{}",
            self.bases.read().unwrap().skaha_base,
            session_id
        )
    }

    pub fn session_renew_url(&self, session_id: &str) -> String {
        format!(
            "{}/v1/session/{}?action=renew",
            self.bases.read().unwrap().skaha_base,
            session_id
        )
    }

    pub fn session_events_url(&self, session_id: &str) -> String {
        format!(
            "{}/v1/session/{}?view=events",
            self.bases.read().unwrap().skaha_base,
            session_id
        )
    }

    pub fn session_logs_url(&self, session_id: &str) -> String {
        format!(
            "{}/v1/session/{}?view=logs",
            self.bases.read().unwrap().skaha_base,
            session_id
        )
    }

    pub fn repository_url(&self) -> String {
        format!("{}/v1/repository", self.bases.read().unwrap().skaha_base)
    }

    pub fn images_url(&self) -> String {
        format!("{}/v1/image", self.bases.read().unwrap().skaha_base)
    }

    pub fn context_url(&self) -> String {
        format!("{}/v1/context", self.bases.read().unwrap().skaha_base)
    }

    pub fn stats_url(&self) -> String {
        format!(
            "{}/v1/session?view=stats",
            self.bases.read().unwrap().skaha_base
        )
    }

    // ---- VOSpace storage (arc_nodes / arc_files) ---------------------------
    pub fn storage_url(&self, username: &str) -> String {
        format!(
            "{}/home/{}?limit=0",
            self.bases.read().unwrap().arc_nodes,
            username
        )
    }

    pub fn vospace_nodes_url(&self, username: &str, path: &str) -> String {
        let arc_nodes = self.bases.read().unwrap().arc_nodes.clone();
        if path.is_empty() {
            format!("{}/home/{}", arc_nodes, username)
        } else {
            format!("{}/home/{}/{}", arc_nodes, username, path)
        }
    }

    /// The VizieR TAP mirrors to try, in order. Reflects the user's setting.
    pub fn vizier_mirrors(&self) -> Vec<String> {
        self.bases.read().unwrap().vizier_mirrors.clone()
    }

    pub fn vospace_files_url(&self, username: &str, path: &str) -> String {
        format!(
            "{}/home/{}/{}",
            self.bases.read().unwrap().arc_files,
            username,
            path
        )
    }

    /// The `vos://` node URI required as the `uri` attribute of a `setNode` body.
    /// (The metadata *URL* from [`vospace_nodes_url`] is a different thing.)
    pub fn vospace_node_uri(&self, username: &str, path: &str) -> String {
        if path.is_empty() {
            format!("vos://cadc.nrc.ca~arc/home/{}", username)
        } else {
            format!("vos://cadc.nrc.ca~arc/home/{}/{}", username, path)
        }
    }

    // ---- TAP + resolver ----------------------------------------------------
    pub fn tap_sync_url(&self) -> String {
        format!("{}/sync", self.bases.read().unwrap().tap_base)
    }

    pub fn resolver_find_url(&self) -> String {
        format!("{}/find", self.bases.read().unwrap().resolver_base)
    }

    // ---- CAOM2 ops (DataLink + package download) ---------------------------
    pub fn datalink_base_url(&self) -> String {
        format!("{}/datalink", self.bases.read().unwrap().caom2ops_base)
    }

    pub fn pkg_url(&self) -> String {
        format!("{}/pkg", self.bases.read().unwrap().caom2ops_base)
    }

    /// CAOM2 metadata document URL: `caom2ops/meta?ID=caom:{collection}/{obsID}`.
    pub fn caom2_meta_url(&self, observation_uri: &str) -> String {
        format!(
            "{}/meta?ID={}",
            self.bases.read().unwrap().caom2ops_base,
            urlencoding::encode(observation_uri)
        )
    }

    pub fn config(&self) -> &AppConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    /// A settings.json written before `vizier_mirrors` existed still loads.
    ///
    /// Every endpoint here is `#[serde(default)]` for exactly this reason, and
    /// a new field is the moment that stops being theoretical: an existing
    /// install has a file with eight endpoint keys and no ninth. Deserialising
    /// it must yield the shipped mirrors, not an empty list and a tool with
    /// nowhere to ask.
    #[test]
    fn a_settings_file_from_before_this_field_still_loads() {
        // A real-shaped file: customised endpoints, no vizier_mirrors.
        let old = r#"{
            "login_base": "https://example.org/ac",
            "skaha_base": "https://example.org/skaha",
            "theme": "dark"
        }"#;
        let cfg: AppConfig = serde_json::from_str(old).expect("old settings must still parse");

        // The customisation survives …
        assert_eq!(cfg.login_base, "https://example.org/ac");
        assert_eq!(cfg.theme, "dark");
        // … and the new field arrives with the shipped list, not empty.
        assert!(!cfg.vizier_mirrors.trim().is_empty());
        assert_eq!(
            ApiEndpoints::new(cfg).vizier_mirrors(),
            api_endpoint_defaults::VIZIER_MIRRORS
                .split_whitespace()
                .map(str::to_string)
                .collect::<Vec<_>>()
        );
    }

    /// Editing the setting takes effect without a restart.
    ///
    /// `apply_from` is what Settings calls on save; a field it does not rebuild
    /// is a field the user can change and watch do nothing.
    #[test]
    fn changing_the_mirrors_applies_without_a_restart() {
        let endpoints = ApiEndpoints::new(AppConfig::default());
        let before = endpoints.vizier_mirrors();

        let edited = AppConfig {
            vizier_mirrors: "https://mirror.example.org/tap/sync".to_string(),
            ..Default::default()
        };
        endpoints.apply_from(&edited);

        assert_ne!(endpoints.vizier_mirrors(), before);
        assert_eq!(
            endpoints.vizier_mirrors(),
            vec!["https://mirror.example.org/tap/sync".to_string()]
        );
    }

    use super::*;

    fn endpoints() -> ApiEndpoints {
        ApiEndpoints::new(AppConfig::default())
    }

    #[test]
    fn sessions_url() {
        assert_eq!(
            endpoints().sessions_url(),
            "https://ws-uv.canfar.net/skaha/v1/session"
        );
    }

    #[test]
    fn session_url() {
        assert_eq!(
            endpoints().session_url("abc123"),
            "https://ws-uv.canfar.net/skaha/v1/session/abc123"
        );
    }

    #[test]
    fn session_renew_url() {
        assert_eq!(
            endpoints().session_renew_url("abc123"),
            "https://ws-uv.canfar.net/skaha/v1/session/abc123?action=renew"
        );
    }

    #[test]
    fn session_events_url() {
        assert_eq!(
            endpoints().session_events_url("abc123"),
            "https://ws-uv.canfar.net/skaha/v1/session/abc123?view=events"
        );
    }

    #[test]
    fn session_logs_url() {
        assert_eq!(
            endpoints().session_logs_url("abc123"),
            "https://ws-uv.canfar.net/skaha/v1/session/abc123?view=logs"
        );
    }

    #[test]
    fn images_url() {
        assert_eq!(
            endpoints().images_url(),
            "https://ws-uv.canfar.net/skaha/v1/image"
        );
    }

    #[test]
    fn repository_url() {
        assert_eq!(
            endpoints().repository_url(),
            "https://ws-uv.canfar.net/skaha/v1/repository"
        );
    }

    #[test]
    fn storage_url() {
        assert_eq!(
            endpoints().storage_url("testuser"),
            "https://ws-uv.canfar.net/arc/nodes/home/testuser?limit=0"
        );
    }

    #[test]
    fn vospace_nodes_url_paths() {
        assert_eq!(
            endpoints().vospace_nodes_url("testuser", ""),
            "https://ws-uv.canfar.net/arc/nodes/home/testuser"
        );
        assert_eq!(
            endpoints().vospace_nodes_url("testuser", "sub/dir"),
            "https://ws-uv.canfar.net/arc/nodes/home/testuser/sub/dir"
        );
    }

    #[test]
    fn vospace_files_url() {
        assert_eq!(
            endpoints().vospace_files_url("testuser", "file.fits"),
            "https://ws-uv.canfar.net/arc/files/home/testuser/file.fits"
        );
    }

    /// A `vos://` URI is built in exactly one place.
    ///
    /// Creating a folder sent `uri="vos://cadc.nrc.ca~arc/{path}"` while the URL
    /// it PUT to rooted the same path under `home/{username}/`. Two spellings of
    /// one address, and the service rejected every folder with "invalid URI".
    /// Both Copy-path actions had the same defect, so what reached the clipboard
    /// was a URI `vcp` refuses.
    ///
    /// The scheme appears inside a string literal only in this module. Everywhere else must ask [`ApiEndpoints::vospace_node_uri`],
    /// which is tested above and roots the path the way the URL does.
    #[test]
    fn only_this_module_spells_out_a_vospace_uri() {
        let mut offenders: Vec<String> = Vec::new();
        for (path, text) in crate::testing::rust_sources() {
            if path.ends_with("config.rs") {
                continue; // the one place that may
            }
            let code = crate::testing::code(&text);
            // The scheme inside a STRING LITERAL: that is a URI being
            // assembled. Prose naming it — this test's own docs, the comment in
            // `create_folder` recording what went wrong — is not.
            for (at, _) in code.match_indices("\"vos://cadc.nrc.ca") {
                let line = code[..at].lines().count();
                offenders.push(format!("{}:{line}", path.display()));
            }
        }
        offenders.sort();
        assert!(
            offenders.is_empty(),
            "a vos:// URI is being assembled outside ApiEndpoints — it will \
             disagree with the URL that addresses the same node: {offenders:#?}"
        );
    }

    #[test]
    fn vospace_node_uri_paths() {
        let e = endpoints();
        assert_eq!(
            e.vospace_node_uri("alice", ""),
            "vos://cadc.nrc.ca~arc/home/alice"
        );
        assert_eq!(
            e.vospace_node_uri("alice", "shared/data.fits"),
            "vos://cadc.nrc.ca~arc/home/alice/shared/data.fits"
        );
    }

    #[test]
    fn login_and_whoami_derive_from_login_base() {
        assert_eq!(
            endpoints().login_url(),
            "https://ws-cadc.canfar.net/ac/login"
        );
        assert_eq!(
            endpoints().whoami_url(),
            "https://ws-cadc.canfar.net/ac/whoami"
        );
    }

    #[test]
    fn tap_resolver_datalink_urls() {
        let e = endpoints();
        assert_eq!(
            e.tap_sync_url(),
            "https://ws.cadc-ccda.hia-iha.nrc-cnrc.gc.ca/argus/sync"
        );
        assert_eq!(
            e.resolver_find_url(),
            "https://ws.cadc-ccda.hia-iha.nrc-cnrc.gc.ca/cadc-target-resolver/find"
        );
        assert_eq!(
            e.datalink_base_url(),
            "https://ws.cadc-ccda.hia-iha.nrc-cnrc.gc.ca/caom2ops/datalink"
        );
        assert_eq!(
            e.pkg_url(),
            "https://ws.cadc-ccda.hia-iha.nrc-cnrc.gc.ca/caom2ops/pkg"
        );
        assert_eq!(
            e.caom2_meta_url("caom:CFHT/1234567"),
            "https://ws.cadc-ccda.hia-iha.nrc-cnrc.gc.ca/caom2ops/meta?ID=caom%3ACFHT%2F1234567"
        );
    }

    #[test]
    fn edited_endpoint_applies_live() {
        let e = endpoints();
        // Trailing slash is trimmed by `apply_from`.
        let cfg = AppConfig {
            skaha_base: "https://staging.example.net/skaha/".to_string(),
            ..Default::default()
        };
        e.apply_from(&cfg);
        assert_eq!(
            e.sessions_url(),
            "https://staging.example.net/skaha/v1/session"
        );
        e.reset_endpoints();
        assert_eq!(
            e.sessions_url(),
            "https://ws-uv.canfar.net/skaha/v1/session"
        );
    }

    #[test]
    fn invalid_endpoint_falls_back_to_default() {
        let cfg = AppConfig {
            tap_base: "not-a-url".to_string(),
            ..Default::default()
        };
        let e = ApiEndpoints::new(cfg);
        assert_eq!(
            e.tap_sync_url(),
            "https://ws.cadc-ccda.hia-iha.nrc-cnrc.gc.ca/argus/sync"
        );
    }

    #[test]
    fn reset_endpoints_keeps_other_settings() {
        let mut cfg = AppConfig {
            theme: "Dark".to_string(),
            default_cores: 8,
            tap_base: "https://staging.example.net/tap".to_string(),
            ..Default::default()
        };
        cfg.reset_endpoints();
        assert_eq!(cfg.tap_base, api_endpoint_defaults::TAP_BASE);
        assert_eq!(cfg.theme, "Dark");
        assert_eq!(cfg.default_cores, 8);
    }

    #[test]
    fn default_config_values() {
        let cfg = AppConfig::default();
        assert_eq!(cfg.default_cores, 2);
        assert_eq!(cfg.default_ram, 8);
        assert_eq!(cfg.default_session_type, "notebook");
        assert_eq!(cfg.language, "system");
    }

    #[test]
    fn legacy_settings_json_still_loads() {
        // Old file: single api_base_url + relative paths, no endpoint bases.
        let legacy = r#"{
            "api_base_url": "https://ws-uv.canfar.net",
            "skaha_api_path": "/skaha/v1",
            "theme": "Dark",
            "default_session_type": "desktop",
            "default_cores": 4,
            "default_ram": 16
        }"#;
        let cfg: AppConfig = serde_json::from_str(legacy).unwrap();
        // Preserved settings survive:
        assert_eq!(cfg.theme, "Dark");
        assert_eq!(cfg.default_session_type, "desktop");
        assert_eq!(cfg.default_cores, 4);
        assert_eq!(cfg.default_ram, 16);
        // New endpoint fields take their defaults (unknown legacy keys ignored):
        assert_eq!(cfg.skaha_base, api_endpoint_defaults::SKAHA_BASE);
        assert_eq!(cfg.login_base, api_endpoint_defaults::LOGIN_BASE);
    }
}
