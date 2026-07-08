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
//!     Skaha account).
//!
//! [`registry_repository`]: ImageDiscoverySettings::registry_repository
//! [`username`]: ImageDiscoverySettings::username
//! [`inspector_image`]: ImageDiscoverySettings::inspector_image

use serde::{Deserialize, Serialize};

/// Default registry host (Canfar's Harbor instance).
pub const DEFAULT_REGISTRY_HOST: &str = "images.canfar.net";
/// Default inspector host image (short `project/name:tag` form).
pub const DEFAULT_INSPECTOR_IMAGE: &str = "skaha/terminal:1.1.2";

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
}

impl Default for ImageDiscoverySettings {
    fn default() -> Self {
        Self {
            registry_host: DEFAULT_REGISTRY_HOST.to_string(),
            registry_repository: String::new(),
            username: String::new(),
            inspector_image: DEFAULT_INSPECTOR_IMAGE.to_string(),
        }
    }
}

impl ImageDiscoverySettings {
    /// The inspector host image to launch, expanded to a full registry
    /// reference from the configured host + repository (see
    /// [`resolve_registry_image`]).
    pub fn resolve_inspector_image(&self) -> String {
        resolve_registry_image(&self.inspector_image, &self.registry_host, &self.registry_repository)
    }

    /// True when nothing user-configured is meaningfully set (the settings UI
    /// shows/hides the Reset affordance on this). Mirrors the C# `IsAllDefaults`
    /// minus the secret check (secret presence is a service-layer concern).
    pub fn is_all_defaults(&self) -> bool {
        self.username.is_empty()
            && self.inspector_image == DEFAULT_INSPECTOR_IMAGE
            && self.registry_repository.is_empty()
            && (self.registry_host == DEFAULT_REGISTRY_HOST || self.registry_host.is_empty())
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
    fn is_all_defaults_flips_when_configured() {
        let mut s = ImageDiscoverySettings::default();
        s.username = "alice".to_string();
        assert!(!s.is_all_defaults());

        let mut s2 = ImageDiscoverySettings::default();
        s2.inspector_image = "images.canfar.net/skaha/astroml:24.07".to_string();
        assert!(!s2.is_all_defaults());

        // Empty host is still considered "default".
        let mut s3 = ImageDiscoverySettings::default();
        s3.registry_host = String::new();
        assert!(s3.is_all_defaults());
    }

    #[test]
    fn serde_round_trip_and_partial_json_defaults() {
        let s = ImageDiscoverySettings {
            registry_host: "images.canfar.net".to_string(),
            registry_repository: "skaha".to_string(),
            username: "bob".to_string(),
            inspector_image: "skaha/terminal:1.1.2".to_string(),
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
        assert_eq!(
            ImageDiscoverySettings::build_auth_header("u", ""),
            {
                use base64::Engine as _;
                base64::engine::general_purpose::STANDARD.encode("u:")
            }
        );
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
        assert_eq!(resolve_registry_image(full, "images.canfar.net", "skaha"), full);
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
