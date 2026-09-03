//! User-configurable defaults for image discovery.
//!
//! Port of `Models/ImageDiscovery/ImageDiscoverySettings.cs`. Holds the
//! non-secret knobs the discovery coordinator needs:
//!   * [`ImageDiscoverySettings::registry_host`] + [`registry_repository`] +
//!     [`username`] — used (together with a secret kept in the OS keychain,
//!     never in this struct) to mint the `x-skaha-registry-auth` header so
//!     Skaha can pull private-namespace images;
//!   * [`inspector_image`] — the headless host image the syft inspector runs
//!     in (must ship bash + python3 + curl/wget and be pullable for the user's
//!     Skaha account);
//!   * [`inspector_cores`] + [`inspector_ram`] — how big that inspector job is.
//!
//! [`registry_repository`]: ImageDiscoverySettings::registry_repository
//! [`username`]: ImageDiscoverySettings::username
//! [`inspector_image`]: ImageDiscoverySettings::inspector_image
//! [`inspector_cores`]: ImageDiscoverySettings::inspector_cores
//! [`inspector_ram`]: ImageDiscoverySettings::inspector_ram

use serde::{Deserialize, Serialize};

/// Default registry host (Canfar's Harbor instance).
pub const DEFAULT_REGISTRY_HOST: &str = "images.canfar.net";
/// Default inspector host image (short `project/name:tag` form).
pub const DEFAULT_INSPECTOR_IMAGE: &str = "skaha/terminal:1.1.2";

/// Default CPU cores for the inspector job.
pub const DEFAULT_INSPECTOR_CORES: u32 = 2;

/// Default RAM (GB) for the inspector job.
///
/// Was hard-coded at 1 GB, and that is what broke image discovery: the
/// inspector runs `syft registry:<target>`, which pulls and unpacks the whole
/// target image, so on a large one syft was SIGKILLed by the cgroup. In this
/// user's cache the syft path failed 32% of the time (11 of 34) while the
/// in-target probe — same 1 GB, but no syft — failed 0 of 26. Across every
/// manifest ever published to their VOSpace the syft path stubbed 53%.
///
/// The two shapes that took are the same fault at different levels: syft killed
/// with `rc=137` (128+9, SIGKILL) leaving a stub, and the whole container killed
/// leaving nothing at all — no logs, no events, no manifest, which is why those
/// failures could never be explained from the record.
///
/// 8 GB is the app's own default job size ([`DEFAULT_RAM_GB`]), so a probe is
/// simply a normal small job rather than a uniquely cramped one.
///
/// [`DEFAULT_RAM_GB`]: crate::models::session_launch_params::DEFAULT_RAM_GB
pub const DEFAULT_INSPECTOR_RAM_GB: u32 = 8;

/// Upper bounds for the inspector job, matching the AI-compute spin rows so the
/// two "how big is this job" settings offer the same range.
pub const MAX_INSPECTOR_CORES: u32 = 64;
/// See [`MAX_INSPECTOR_CORES`].
pub const MAX_INSPECTOR_RAM_GB: u32 = 256;

/// Clamp a requested inspector core count into range; 0 means "unset" and
/// yields the default.
pub fn clamp_inspector_cores(cores: u32) -> u32 {
    let base = if cores == 0 {
        DEFAULT_INSPECTOR_CORES
    } else {
        cores
    };
    base.clamp(1, MAX_INSPECTOR_CORES)
}

/// Clamp a requested inspector RAM size (GB) into range; 0 means "unset" and
/// yields the default.
pub fn clamp_inspector_ram(ram: u32) -> u32 {
    let base = if ram == 0 {
        DEFAULT_INSPECTOR_RAM_GB
    } else {
        ram
    };
    base.clamp(1, MAX_INSPECTOR_RAM_GB)
}

/// Persistable image-discovery preferences. The registry secret lives in the
/// OS keychain and is intentionally **not** a field here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ImageDiscoverySettings {
    /// Registry host, e.g. `images.canfar.net`.
    pub registry_host: String,
    /// Registry repository/project (e.g. `skaha`); prefixes a short inspector
    /// image name. Empty when unset.
    pub registry_repository: String,
    /// Registry username (the account the stored secret belongs to).
    pub username: String,
    /// Inspector host image — a short name is expanded with the configured
    /// host/repository; a fully-qualified `host/project/name:tag` is used as-is.
    pub inspector_image: String,
    /// CPU cores for the inspector job. Applies to the syft inspector only —
    /// the in-target probe reads package databases already on disk and stays at
    /// [`crate::services::image_discovery_coordinator::IN_TARGET_PROBE_CORES`].
    pub inspector_cores: u32,
    /// RAM (GB) for the inspector job. See [`DEFAULT_INSPECTOR_RAM_GB`] for why
    /// this exists and why the default is what it is.
    pub inspector_ram: u32,
}

impl Default for ImageDiscoverySettings {
    fn default() -> Self {
        Self {
            registry_host: DEFAULT_REGISTRY_HOST.to_string(),
            registry_repository: String::new(),
            username: String::new(),
            inspector_image: DEFAULT_INSPECTOR_IMAGE.to_string(),
            inspector_cores: DEFAULT_INSPECTOR_CORES,
            inspector_ram: DEFAULT_INSPECTOR_RAM_GB,
        }
    }
}

impl ImageDiscoverySettings {
    /// The inspector host image to launch, expanded to a full registry
    /// reference from the configured host + repository (see
    /// [`resolve_registry_image`]).
    pub fn resolve_inspector_image(&self) -> String {
        resolve_registry_image(
            &self.inspector_image,
            &self.registry_host,
            &self.registry_repository,
        )
    }

    /// The inspector job size actually launched, clamped into range.
    ///
    /// Read through this rather than off the fields: the settings file is
    /// hand-editable, and a 0 or a 9999 in it reaches Skaha as a launch that is
    /// rejected — or, worse, silently accepted.
    pub fn resolved_inspector_resources(&self) -> (u32, u32) {
        (
            clamp_inspector_cores(self.inspector_cores),
            clamp_inspector_ram(self.inspector_ram),
        )
    }

    #[cfg(test)]
    /// True when nothing user-configured is meaningfully set (the settings UI
    /// shows/hides the Reset affordance on this). Mirrors the C# `IsAllDefaults`
    /// minus the secret check (secret presence is a service-layer concern).
    pub fn is_all_defaults(&self) -> bool {
        self.username.is_empty()
            && self.inspector_image == DEFAULT_INSPECTOR_IMAGE
            && self.registry_repository.is_empty()
            && (self.registry_host == DEFAULT_REGISTRY_HOST || self.registry_host.is_empty())
            && self.inspector_cores == DEFAULT_INSPECTOR_CORES
            && self.inspector_ram == DEFAULT_INSPECTOR_RAM_GB
    }

    /// Build the `x-skaha-registry-auth` value: `base64(username:secret)`.
    pub fn build_auth_header(username: &str, secret: &str) -> String {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(format!("{username}:{secret}"))
    }
}

/// Resolve a possibly-short image name to a full registry reference using the
/// configured host and repository/project. Faithful port of the shared
/// `Helpers/RegistryImageResolver.Resolve`:
///   * a bare name (no `/`) is prefixed with the host — and the repo/project
///     when set;
///   * a name that already contains a `/` is assumed already-qualified and is
///     returned unchanged;
///   * an empty image yields an empty string; a blank host leaves the bare
///     name untouched.
///
/// Pure and unit-tested for parity with `RegistryImageResolverTests`.
pub fn resolve_registry_image(image: &str, host: &str, repository: &str) -> String {
    let img = image.trim();
    if img.is_empty() {
        return String::new();
    }
    // Docker reference convention: the first path segment is a registry HOST only
    // if it contains a '.' or ':' (or is "localhost"). "skaha/terminal:1.1.2" is
    // project/name — NOT host-qualified — and must still get the host prefix,
    // otherwise Skaha rejects the probe ("session image must come from one of
    // [images.canfar.net]").
    let first = img.split('/').next().unwrap_or("");
    let host_qualified =
        img.contains('/') && (first.contains('.') || first.contains(':') || first == "localhost");
    if host_qualified {
        return img.to_string();
    }
    let h = host.trim().trim_end_matches('/');
    if h.is_empty() {
        return img.to_string(); // no host to prefix with
    }
    if img.contains('/') {
        // Already project-qualified (e.g. "skaha/terminal:1.1.2") — add the host
        // only; prepending the repository too would double the project segment.
        return format!("{h}/{img}");
    }
    let repo = repository.trim().trim_matches('/');
    if repo.is_empty() {
        format!("{h}/{img}")
    } else {
        format!("{h}/{repo}/{img}")
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_constants() {
        let s = ImageDiscoverySettings::default();
        assert_eq!(s.registry_host, DEFAULT_REGISTRY_HOST);
        assert_eq!(s.inspector_image, DEFAULT_INSPECTOR_IMAGE);
        assert!(s.registry_repository.is_empty());
        assert!(s.username.is_empty());
        assert!(s.is_all_defaults());
    }

    #[test]
    fn the_inspector_default_is_not_the_one_gigabyte_that_broke_discovery() {
        // The regression this guards is a number, so the test is the number.
        // At 1 GB `syft registry:<target>` was SIGKILLed on large images: 32%
        // of syft probes failed against 0% of in-target ones, and the container
        // kills left no logs, no events and no manifest to explain themselves.
        let s = ImageDiscoverySettings::default();
        assert!(
            s.inspector_ram >= 4,
            "the inspector unpacks whole container images; {} GB is back in \
             OOM territory",
            s.inspector_ram
        );
        assert_eq!(s.inspector_ram, DEFAULT_INSPECTOR_RAM_GB);
        assert_eq!(s.inspector_cores, DEFAULT_INSPECTOR_CORES);
        // The app already has a "normal small job" size; a probe is one of
        // those, and two different answers to the same question is how the
        // 1 GB got there.
        assert_eq!(
            (s.inspector_cores, s.inspector_ram),
            (
                crate::models::session_launch_params::DEFAULT_CORES,
                crate::models::session_launch_params::DEFAULT_RAM_GB
            ),
        );
    }

    #[test]
    fn inspector_resources_clamp_rather_than_reaching_skaha() {
        // The settings file is hand-editable and these values are launched.
        assert_eq!(clamp_inspector_cores(0), DEFAULT_INSPECTOR_CORES);
        assert_eq!(clamp_inspector_ram(0), DEFAULT_INSPECTOR_RAM_GB);
        assert_eq!(clamp_inspector_cores(9999), MAX_INSPECTOR_CORES);
        assert_eq!(clamp_inspector_ram(9999), MAX_INSPECTOR_RAM_GB);
        assert_eq!(clamp_inspector_cores(4), 4);
        assert_eq!(clamp_inspector_ram(16), 16);

        let s = ImageDiscoverySettings {
            inspector_cores: 0,
            inspector_ram: 9999,
            ..Default::default()
        };
        assert_eq!(
            s.resolved_inspector_resources(),
            (DEFAULT_INSPECTOR_CORES, MAX_INSPECTOR_RAM_GB)
        );
    }

    #[test]
    fn a_settings_file_written_before_these_knobs_existed_gets_the_new_default() {
        // Every existing install has a settings file with no inspector_cores /
        // inspector_ram in it. `#[serde(default)]` has to give those the NEW
        // default, or the fix ships and nobody who already used the app gets it.
        let old = r#"{
            "registry_host": "images.canfar.net",
            "registry_repository": "private-test",
            "username": "szautkin",
            "inspector_image": "private-test/verbinal-inspector:1.0.0"
        }"#;
        let s: ImageDiscoverySettings = serde_json::from_str(old).unwrap();
        assert_eq!(s.username, "szautkin");
        assert_eq!(s.inspector_cores, DEFAULT_INSPECTOR_CORES);
        assert_eq!(s.inspector_ram, DEFAULT_INSPECTOR_RAM_GB);
    }

    #[test]
    fn is_all_defaults_flips_when_configured() {
        let s = ImageDiscoverySettings {
            username: "alice".to_string(),
            ..Default::default()
        };
        assert!(!s.is_all_defaults());

        let s2 = ImageDiscoverySettings {
            inspector_image: "images.canfar.net/skaha/astroml:24.07".to_string(),
            ..Default::default()
        };
        assert!(!s2.is_all_defaults());

        // Empty host is still considered "default".
        let s3 = ImageDiscoverySettings {
            registry_host: String::new(),
            ..Default::default()
        };
        assert!(s3.is_all_defaults());
    }

    #[test]
    fn serde_round_trip_and_partial_json_defaults() {
        let s = ImageDiscoverySettings {
            registry_host: "images.canfar.net".to_string(),
            registry_repository: "skaha".to_string(),
            username: "bob".to_string(),
            inspector_image: "skaha/terminal:1.1.2".to_string(),
            inspector_cores: 4,
            inspector_ram: 16,
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: ImageDiscoverySettings = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);

        // Missing fields fall back to defaults thanks to `#[serde(default)]`.
        let partial: ImageDiscoverySettings =
            serde_json::from_str(r#"{"username":"carol"}"#).unwrap();
        assert_eq!(partial.username, "carol");
        assert_eq!(partial.registry_host, DEFAULT_REGISTRY_HOST);
        assert_eq!(partial.inspector_image, DEFAULT_INSPECTOR_IMAGE);
        assert!(partial.registry_repository.is_empty());
    }

    #[test]
    fn auth_header_is_base64_of_user_colon_secret() {
        // base64("user:pass") == "dXNlcjpwYXNz"
        assert_eq!(
            ImageDiscoverySettings::build_auth_header("user", "pass"),
            "dXNlcjpwYXNz"
        );
        // Empty secret still encodes the "user:" prefix.
        assert_eq!(ImageDiscoverySettings::build_auth_header("u", ""), {
            use base64::Engine as _;
            base64::engine::general_purpose::STANDARD.encode("u:")
        });
    }

    #[test]
    fn resolve_short_name_with_repo_is_host_repo_qualified() {
        assert_eq!(
            resolve_registry_image("verbinal-compute:1.0", "images.canfar.net", "skaha"),
            "images.canfar.net/skaha/verbinal-compute:1.0"
        );
    }

    #[test]
    fn resolve_short_name_without_repo_is_host_qualified() {
        assert_eq!(
            resolve_registry_image("verbinal-compute:1.0", "images.canfar.net", ""),
            "images.canfar.net/verbinal-compute:1.0"
        );
    }

    #[test]
    fn resolve_project_qualified_gets_host_prefix_only() {
        // "skaha/terminal:1.1.2" is project/name (first segment has no '.' / ':'),
        // so it must be HOST-prefixed — Skaha rejects unprefixed images — but the
        // repository must NOT be inserted (that would double the project segment).
        assert_eq!(
            resolve_registry_image("skaha/terminal:1.1.2", "images.canfar.net", "skaha"),
            "images.canfar.net/skaha/terminal:1.1.2"
        );
        assert_eq!(
            resolve_registry_image(
                "private-test/verbinal-inspector:1.0.0",
                "images.canfar.net",
                "private-test"
            ),
            "images.canfar.net/private-test/verbinal-inspector:1.0.0"
        );
        // A genuinely host-qualified reference is left unchanged.
        let full = "images.canfar.net/skaha/terminal:1.1.2";
        assert_eq!(
            resolve_registry_image(full, "images.canfar.net", "skaha"),
            full
        );
        assert_eq!(
            resolve_registry_image("localhost/x:1", "images.canfar.net", ""),
            "localhost/x:1"
        );
    }

    #[test]
    fn resolve_edge_cases() {
        assert_eq!(resolve_registry_image("", "images.canfar.net", "skaha"), "");
        assert_eq!(resolve_registry_image("myimg:1", "", "skaha"), "myimg:1");
        // Trims a trailing host slash and surrounding repo slashes.
        assert_eq!(resolve_registry_image("img", "h/", "/r/"), "h/r/img");
    }

    #[test]
    fn default_inspector_image_is_host_qualified() {
        // The default "skaha/terminal:1.1.2" must resolve with the registry host
        // prefix — Skaha requires fully-qualified session images.
        let s = ImageDiscoverySettings::default();
        assert_eq!(
            s.resolve_inspector_image(),
            format!("{DEFAULT_REGISTRY_HOST}/{DEFAULT_INSPECTOR_IMAGE}")
        );
    }
}
