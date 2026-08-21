//! AI Compute models: the persisted settings for the agent `run_code` tool, the
//! pure file-drop RPC contract shared with the external `verbinal-execution`
//! watcher container, and the request/result wire records.
//!
//! Port of `Models/AICompute/AIComputeSettings.cs` +
//! `Services/AICompute/{RunCodeContract,RunCodeModels}.cs`. Everything here is
//! pure (no I/O) so it is fully unit-testable; the actual file-drop / session
//! plumbing lives in [`crate::services::ai_compute_service`].

use crate::models::image_discovery_settings::resolve_registry_image;
use serde::{Deserialize, Serialize};

/// Default registry host (Canfar's Harbor instance) — kept in sync with the
/// Image Discovery default.
pub const DEFAULT_REGISTRY_HOST: &str = "images.canfar.net";

// ─────────────────────────────────────────────────────────────────────────────
// Settings
// ─────────────────────────────────────────────────────────────────────────────

/// User-configurable settings for the agent `run_code` tool: the compute
/// container image (an EMPTY image DISABLES run_code), the instance size, and the
/// registry credentials to pull a private compute image. The secret itself lives
/// in the OS keychain (never in this struct); [`has_secret`] is populated by the
/// service on load and is not persisted. Mirrors the C# `AIComputeSettings`.
///
/// [`has_secret`]: AIComputeSettings::has_secret
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AIComputeSettings {
    /// The compute container image (a `verbinal-execution` watcher). Empty ⇒
    /// run_code disabled. A short name is expanded with the configured
    /// host/repository; a full `host/project/name:tag` is used as-is.
    pub image: String,
    pub cores: u32,
    pub ram: u32,
    pub registry_host: String,
    /// Registry repository/project (e.g. `project`); prefixes a short compute
    /// image name. Empty when unset.
    pub registry_repository: String,
    pub registry_username: String,
    /// True when a secret is stored for the current (host, username). Populated
    /// from the keychain by the service; NOT serialized.
    #[serde(skip)]
    pub has_secret: bool,
}

impl Default for AIComputeSettings {
    fn default() -> Self {
        Self {
            image: String::new(),
            cores: RunCodeContract::DEFAULT_CORES,
            ram: RunCodeContract::DEFAULT_RAM,
            registry_host: DEFAULT_REGISTRY_HOST.to_string(),
            registry_repository: String::new(),
            registry_username: String::new(),
            has_secret: false,
        }
    }
}

impl AIComputeSettings {
    /// The image reference to launch, from whichever field actually carries it,
    /// with the repository to qualify it by — or `None` when nothing does.
    ///
    /// Normally that is the Compute image field. But "Registry repository
    /// (project)" sits right above it and reads like the place a full
    /// `project/name:tag` belongs, so that is where one gets typed — leaving
    /// the image field empty, run_code silently off, and every other row on the
    /// screen saying the configuration is fine, credentials included.
    ///
    /// A repository carrying a TAG is a complete reference and is taken as one.
    /// A bare `private-test` has no tag and is left alone, so the ordinary
    /// host + repository + image composition is untouched.
    fn image_source(&self) -> Option<(&str, &str)> {
        let image = self.image.trim();
        if !image.is_empty() {
            return Some((image, self.registry_repository.trim()));
        }
        let repository = self.registry_repository.trim();
        // Qualifying it by itself would double the project segment.
        (repository.contains(':')).then_some((repository, ""))
    }

    /// run_code / start_compute are enabled only when a compute image is set.
    pub fn is_enabled(&self) -> bool {
        self.image_source().is_some()
    }

    /// The compute image to launch, expanded to a full registry reference from
    /// the configured host + repository. Empty when run_code is disabled.
    pub fn resolve_image(&self) -> String {
        match self.image_source() {
            Some((image, repository)) => {
                resolve_registry_image(image, &self.registry_host, repository)
            }
            None => String::new(),
        }
    }

    /// The clamped (cores, ram) for the lazy compute launch.
    pub fn resolve_resources(&self) -> (u32, u32) {
        (
            RunCodeContract::clamp_cores(self.cores),
            RunCodeContract::clamp_ram(self.ram),
        )
    }

    #[cfg(test)]
    /// True when nothing user-configured is meaningfully set (the settings UI
    /// shows/hides the Reset affordance on this). Mirrors the C# `IsAllDefaults`.
    pub fn is_all_defaults(&self) -> bool {
        self.image.is_empty()
            && self.cores == RunCodeContract::DEFAULT_CORES
            && self.ram == RunCodeContract::DEFAULT_RAM
            && self.registry_username.is_empty()
            && self.registry_repository.is_empty()
            && !self.has_secret
            && (self.registry_host == DEFAULT_REGISTRY_HOST || self.registry_host.is_empty())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// File-drop RPC contract (pure — matches the watcher byte-for-byte)
// ─────────────────────────────────────────────────────────────────────────────

/// The file-drop RPC contract shared with the external `verbinal-execution`
/// watcher container. These literals MUST match the watcher (and the macOS /
/// Windows `RunCodeContract`): the app launches one reusable `contributed`
/// session named [`RunCodeContract::SESSION_NAME`], PUTs a request to the shared
/// `/arc` inbox, and polls the out dir for the result. Pure — no I/O.
pub struct RunCodeContract;

impl RunCodeContract {
    pub const SESSION_NAME: &'static str = "verbinal-compute";
    pub const SESSION_TYPE: &'static str = "contributed";
    pub const INBOX_DIR: &'static str = ".verbinal/exec/inbox";
    pub const OUT_DIR: &'static str = ".verbinal/exec/out";

    /// Bounded read of the result file (the watcher caps output at this size).
    pub const MAX_RESULT_BYTES: usize = 1024 * 1024;

    pub const LANGUAGES: [&'static str; 2] = ["python", "bash"];

    pub const DEFAULT_TIMEOUT_SECONDS: i64 = 60;
    pub const MAX_TIMEOUT_SECONDS: i64 = 900;
    pub const DEFAULT_CORES: u32 = 1;
    pub const DEFAULT_RAM: u32 = 1;
    pub const MAX_CORES: u32 = 64;
    pub const MAX_RAM: u32 = 256;

    const UNSAFE_ID_CHARS: [char; 9] = ['/', ':', '\\', '?', '*', '<', '>', '|', '"'];

    pub fn clamp_timeout(seconds: i64) -> i64 {
        let base = if seconds <= 0 {
            Self::DEFAULT_TIMEOUT_SECONDS
        } else {
            seconds
        };
        base.clamp(1, Self::MAX_TIMEOUT_SECONDS)
    }

    pub fn clamp_cores(cores: u32) -> u32 {
        let base = if cores == 0 {
            Self::DEFAULT_CORES
        } else {
            cores
        };
        base.clamp(1, Self::MAX_CORES)
    }

    pub fn clamp_ram(ram: u32) -> u32 {
        let base = if ram == 0 { Self::DEFAULT_RAM } else { ram };
        base.clamp(1, Self::MAX_RAM)
    }

    /// Normalize the requested language to a supported one (default `python`).
    pub fn normalize_language(language: Option<&str>) -> String {
        let l = language.unwrap_or("").trim().to_lowercase();
        if Self::LANGUAGES.contains(&l.as_str()) {
            l
        } else {
            "python".to_string()
        }
    }

    /// Replace filesystem-unsafe characters in an execution id so it is a valid
    /// file name.
    pub fn sanitize_id(id: &str) -> String {
        id.chars()
            .map(|c| {
                if Self::UNSAFE_ID_CHARS.contains(&c) {
                    '_'
                } else {
                    c
                }
            })
            .collect()
    }

    /// Request file, relative to the user's VOSpace home:
    /// `.verbinal/exec/inbox/<id>.json`.
    ///
    /// This is the ONLY form, on purpose.
    ///
    /// The reference builds `<username>/.verbinal/…` because its storage layer
    /// roots at `/home/`. Ours roots at `/home/<username>/`, so the same string
    /// handed to `vospace_files_url` produced
    /// `/home/szautkin/szautkin/.verbinal/exec/inbox/…` — a directory one level
    /// below one that exists. Every run_code failed on upload, and every result
    /// read 404 and looked like "not ready yet" rather than a wrong address.
    ///
    /// A `inbox_path(username, id)` mirroring the reference used to live beside
    /// this. It was correct, tested, and had exactly one use: being passed to a
    /// function that adds the username itself. A function whose only caller is
    /// a mistake is a mistake, so it is gone and the compiler now refuses what
    /// the tests could not see — the fault was never in the path, it was at the
    /// seam, and no test of the path alone can reach a seam.
    ///
    /// The literals (`INBOX_DIR`, `OUT_DIR`) still match the watcher and the
    /// other two apps, which is what the shared contract actually requires.
    pub fn inbox_relpath(id: &str) -> String {
        format!("{}/{}.json", Self::INBOX_DIR, Self::sanitize_id(id))
    }

    /// Result file, relative to the user's VOSpace home:
    /// `.verbinal/exec/out/<id>.json`.
    pub fn out_relpath(id: &str) -> String {
        format!("{}/{}.json", Self::OUT_DIR, Self::sanitize_id(id))
    }

    /// The inbox folder tree to create one level at a time (create_folder rejects
    /// creating a multi-level path whose parents don't yet exist).
    pub fn inbox_tree_levels() -> [&'static str; 3] {
        [".verbinal", ".verbinal/exec", ".verbinal/exec/inbox"]
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Wire records
// ─────────────────────────────────────────────────────────────────────────────

/// The request file dropped in the inbox: `{id, language, code, timeout_seconds}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunCodeRequest {
    pub id: String,
    pub language: String,
    pub code: String,
    pub timeout_seconds: i64,
}

impl RunCodeRequest {
    pub fn new(
        id: impl Into<String>,
        language: impl Into<String>,
        code: impl Into<String>,
        timeout_seconds: i64,
    ) -> Self {
        Self {
            id: id.into(),
            language: language.into(),
            code: code.into(),
            timeout_seconds,
        }
    }
}

/// The result file the watcher writes. `status` is authoritative
/// (`ok|error|timeout`); stdout/stderr are utf8 unless the matching
/// `*_encoding` is `base64`. `artifacts` lists any files the watcher published.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct RunCodeResult {
    pub status: Option<String>,
    pub exit_code: Option<i64>,
    pub stdout: Option<String>,
    pub stdout_encoding: Option<String>,
    pub stderr: Option<String>,
    pub stderr_encoding: Option<String>,
    pub duration_ms: Option<i64>,
    pub truncated: Option<bool>,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    /// VOSpace paths (or names) of any artifacts the run produced.
    pub artifacts: Option<Vec<String>>,
}

impl RunCodeResult {
    pub fn decoded_stdout(&self) -> Option<String> {
        Self::decode(self.stdout.as_deref(), self.stdout_encoding.as_deref())
    }

    pub fn decoded_stderr(&self) -> Option<String> {
        Self::decode(self.stderr.as_deref(), self.stderr_encoding.as_deref())
    }

    /// base64-decode when the encoding says so, else pass the value through. A
    /// malformed base64 payload falls back to the raw value (never panics).
    fn decode(value: Option<&str>, encoding: Option<&str>) -> Option<String> {
        let value = value?;
        if encoding
            .map(|e| e.eq_ignore_ascii_case("base64"))
            .unwrap_or(false)
        {
            use base64::Engine as _;
            match base64::engine::general_purpose::STANDARD.decode(value) {
                Ok(bytes) => Some(String::from_utf8_lossy(&bytes).into_owned()),
                Err(_) => Some(value.to_string()),
            }
        } else {
            Some(value.to_string())
        }
    }
}

/// Pure (de)serialization of the run_code wire files — snake_case to match the
/// watcher contract.
pub struct RunCodeJson;

impl RunCodeJson {
    pub fn serialize_request(request: &RunCodeRequest) -> Result<String, String> {
        serde_json::to_string(request).map_err(|e| e.to_string())
    }

    /// Lenient parse of the result file; `None` when absent/blank/incomplete
    /// (read-after-write lag).
    pub fn try_parse_result(json: &str) -> Option<RunCodeResult> {
        if json.trim().is_empty() {
            return None;
        }
        serde_json::from_str::<RunCodeResult>(json).ok()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_disabled_and_all_defaults() {
        let s = AIComputeSettings::default();
        assert!(!s.is_enabled(), "an empty image disables run_code");
        assert!(s.is_all_defaults());
        assert_eq!(s.cores, 1);
        assert_eq!(s.ram, 1);
        assert_eq!(s.registry_host, DEFAULT_REGISTRY_HOST);
        assert_eq!(s.resolve_image(), "");
    }

    #[test]
    fn a_full_reference_in_the_repository_field_still_launches() {
        // What a real configuration looked like: everything filled, credentials
        // verified, run_code off — because the whole reference had been typed
        // into "Registry repository (project)", one row above the field that is
        // actually checked, and nothing said so.
        let s = AIComputeSettings {
            image: String::new(),
            registry_host: "images.canfar.net".to_string(),
            registry_repository: "private-test/verbinal-execution:0.0.1".to_string(),
            ..Default::default()
        };
        assert!(s.is_enabled());
        assert_eq!(
            s.resolve_image(),
            "images.canfar.net/private-test/verbinal-execution:0.0.1",
            "the project segment was doubled"
        );
    }

    #[test]
    fn a_bare_project_in_the_repository_field_is_still_just_a_project() {
        // No tag, so it is what the label says and composition is unchanged.
        let s = AIComputeSettings {
            image: String::new(),
            registry_repository: "private-test".to_string(),
            ..Default::default()
        };
        assert!(!s.is_enabled());
        assert_eq!(s.resolve_image(), "");

        let s = AIComputeSettings {
            image: "verbinal-compute:1.0".to_string(),
            registry_host: "images.canfar.net".to_string(),
            registry_repository: "private-test".to_string(),
            ..Default::default()
        };
        assert_eq!(
            s.resolve_image(),
            "images.canfar.net/private-test/verbinal-compute:1.0"
        );
    }

    #[test]
    fn a_project_qualified_image_beside_its_own_project_is_not_doubled() {
        // The configuration that works today, exactly as it sits on disk: the
        // full reference in "Compute image", the project on its own in
        // "Registry repository". Both name `private-test`, and a naive
        // host + repository + image composition would launch
        // `images.canfar.net/private-test/private-test/verbinal-execution:0.0.1`
        // — a pull failure that reads like a missing image rather than a
        // mangled name.
        let s = AIComputeSettings {
            image: "private-test/verbinal-execution:0.0.1".to_string(),
            registry_host: "images.canfar.net".to_string(),
            registry_repository: "private-test".to_string(),
            ..Default::default()
        };
        assert!(s.is_enabled());
        assert_eq!(
            s.resolve_image(),
            "images.canfar.net/private-test/verbinal-execution:0.0.1",
            "the project segment was doubled"
        );
    }

    #[test]
    fn the_image_field_wins_when_both_are_filled() {
        let s = AIComputeSettings {
            image: "chosen:1.0".to_string(),
            registry_host: "images.canfar.net".to_string(),
            registry_repository: "proj/other:2.0".to_string(),
            ..Default::default()
        };
        assert_eq!(
            s.resolve_image(),
            "images.canfar.net/proj/other:2.0/chosen:1.0"
        );
    }

    #[test]
    fn is_enabled_when_image_set_and_resolves_short_name() {
        let s = AIComputeSettings {
            image: "verbinal-compute:1.0".to_string(),
            registry_repository: "project".to_string(),
            ..Default::default()
        };
        assert!(s.is_enabled());
        assert!(!s.is_all_defaults());
        assert_eq!(
            s.resolve_image(),
            "images.canfar.net/project/verbinal-compute:1.0"
        );
    }

    #[test]
    fn resolve_resources_clamps() {
        let s = AIComputeSettings {
            cores: 0,
            ram: 9999,
            ..Default::default()
        };
        // cores 0 → default 1; ram over max → clamped to 256.
        assert_eq!(s.resolve_resources(), (1, 256));
    }

    #[test]
    fn settings_serde_skips_secret_and_partial_json_defaults() {
        let s = AIComputeSettings {
            image: "img:1".to_string(),
            has_secret: true,
            ..Default::default()
        };
        let json = serde_json::to_string(&s).unwrap();
        assert!(!json.contains("has_secret"), "has_secret is not persisted");
        // Missing fields fall back to defaults.
        let partial: AIComputeSettings = serde_json::from_str(r#"{"image":"x:1"}"#).unwrap();
        assert_eq!(partial.image, "x:1");
        assert_eq!(partial.cores, 1);
        assert_eq!(partial.registry_host, DEFAULT_REGISTRY_HOST);
        assert!(!partial.has_secret);
    }

    #[test]
    fn contract_clamps() {
        assert_eq!(RunCodeContract::clamp_timeout(0), 60);
        assert_eq!(RunCodeContract::clamp_timeout(-5), 60);
        assert_eq!(RunCodeContract::clamp_timeout(30), 30);
        assert_eq!(RunCodeContract::clamp_timeout(99999), 900);
        assert_eq!(RunCodeContract::clamp_cores(0), 1);
        assert_eq!(RunCodeContract::clamp_cores(128), 64);
        assert_eq!(RunCodeContract::clamp_ram(0), 1);
        assert_eq!(RunCodeContract::clamp_ram(9999), 256);
    }

    #[test]
    fn normalize_language_defaults_to_python() {
        assert_eq!(RunCodeContract::normalize_language(Some("  BASH ")), "bash");
        assert_eq!(
            RunCodeContract::normalize_language(Some("python")),
            "python"
        );
        assert_eq!(RunCodeContract::normalize_language(Some("ruby")), "python");
        assert_eq!(RunCodeContract::normalize_language(None), "python");
    }

    #[test]
    fn sanitize_id_replaces_unsafe_chars() {
        assert_eq!(RunCodeContract::sanitize_id("a/b:c*d"), "a_b_c_d");
        assert_eq!(RunCodeContract::sanitize_id("clean-id_123"), "clean-id_123");
    }

    /// The URL run_code actually PUTs to names the user exactly once.
    ///
    /// This is the test that was missing. The old one asserted
    /// `inbox_path("alice", ...) == "alice/.verbinal/…"`, which was correct and
    /// proved nothing: the fault was not in the path, it was in handing that
    /// path to a URL builder that inserts `/home/<username>/` itself. The
    /// result was `/home/szautkin/szautkin/.verbinal/exec/inbox/…` — every
    /// run_code failed on upload, and every result read 404 and looked like
    /// "not ready yet" rather than a wrong address.
    ///
    /// So this asserts against the composed URL, which is the only place the
    /// two halves meet.
    #[test]
    fn the_url_run_code_uploads_to_names_the_user_once() {
        let endpoints = crate::config::ApiEndpoints::new(crate::config::AppConfig::default());

        for url in [
            endpoints.vospace_files_url("szautkin", &RunCodeContract::inbox_relpath("abc")),
            endpoints.vospace_files_url("szautkin", &RunCodeContract::out_relpath("abc")),
        ] {
            assert_eq!(
                url.matches("szautkin").count(),
                1,
                "the username is doubled in {url}"
            );
            assert!(
                url.ends_with("/home/szautkin/.verbinal/exec/inbox/abc.json")
                    || url.ends_with("/home/szautkin/.verbinal/exec/out/abc.json"),
                "{url}"
            );
        }
    }

    /// The tree that gets CREATED is the tree the files are written into.
    ///
    /// `ensure_inbox_tree` walks home-relative levels while the upload used a
    /// username-rooted path, so the app created `.verbinal/exec/inbox` and then
    /// wrote one directory deeper. Each was defensible alone.
    #[test]
    fn the_inbox_tree_is_the_parent_of_the_inbox_file() {
        let deepest = RunCodeContract::inbox_tree_levels()
            .last()
            .copied()
            .expect("levels");
        let file = RunCodeContract::inbox_relpath("abc");
        assert_eq!(
            file,
            format!("{deepest}/abc.json"),
            "the file is not written into the folder the app creates"
        );
    }

    #[test]
    fn inbox_and_out_paths_are_built_under_dot_verbinal() {
        // An id with a slash is sanitized, so it cannot climb out of the inbox.
        assert_eq!(
            RunCodeContract::inbox_relpath("abc/def"),
            ".verbinal/exec/inbox/abc_def.json"
        );
        assert_eq!(
            RunCodeContract::out_relpath("abc"),
            ".verbinal/exec/out/abc.json"
        );
        assert_eq!(
            RunCodeContract::inbox_tree_levels(),
            [".verbinal", ".verbinal/exec", ".verbinal/exec/inbox"]
        );
    }

    #[test]
    fn serialize_request_is_snake_case() {
        let req = RunCodeRequest::new("id1", "python", "print(1)", 60);
        let json = RunCodeJson::serialize_request(&req).unwrap();
        assert!(json.contains("\"timeout_seconds\":60"));
        assert!(json.contains("\"language\":\"python\""));
        assert!(json.contains("\"code\":\"print(1)\""));
    }

    #[test]
    fn try_parse_result_is_lenient() {
        assert!(RunCodeJson::try_parse_result("").is_none());
        assert!(RunCodeJson::try_parse_result("   ").is_none());
        assert!(RunCodeJson::try_parse_result("{ not json").is_none());
        // A partial result parses; missing fields become None.
        let r = RunCodeJson::try_parse_result(r#"{"status":"ok","exit_code":0,"stdout":"hi"}"#)
            .unwrap();
        assert_eq!(r.status.as_deref(), Some("ok"));
        assert_eq!(r.exit_code, Some(0));
        assert_eq!(r.stdout.as_deref(), Some("hi"));
        assert_eq!(r.stderr, None);
        assert_eq!(r.artifacts, None);
    }

    #[test]
    fn result_decodes_base64_stdout_and_passes_through_utf8() {
        // base64("hi") == "aGk="
        let r = RunCodeJson::try_parse_result(
            r#"{"stdout":"aGk=","stdout_encoding":"base64","stderr":"plain","artifacts":["out/a.png"]}"#,
        )
        .unwrap();
        assert_eq!(r.decoded_stdout().as_deref(), Some("hi"));
        assert_eq!(r.decoded_stderr().as_deref(), Some("plain"));
        assert_eq!(r.artifacts.as_deref(), Some(&["out/a.png".to_string()][..]));
    }

    #[test]
    fn result_decode_bad_base64_falls_back_to_raw() {
        let r = RunCodeJson::try_parse_result(
            r#"{"stdout":"!!!notbase64!!!","stdout_encoding":"base64"}"#,
        )
        .unwrap();
        assert_eq!(r.decoded_stdout().as_deref(), Some("!!!notbase64!!!"));
    }
}
