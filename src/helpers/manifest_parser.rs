//! Parses the JSON output of the in-container probe into an [`ImageManifest`].
//!
//! Ported from `Services/ImageDiscovery/ManifestParser.cs`. Empty package sets (an image with no
//! dpkg/rpm/apk/pip/conda) are a SUCCESS, not a failure — only unparseable, mis-schema'd, or
//! identity-less manifests are errors. The parser never panics: every failure path is a typed
//! [`ManifestError`].

use crate::models::image_manifest::ImageManifest;
use std::fmt;

/// Maximum `schema_version` this build understands (kept in lockstep with the probe script's
/// `schemaVersion`). A manifest declaring a newer schema is rejected as
/// [`ManifestError::UnsupportedSchema`] so the caller can treat the image as not-yet-discovered
/// rather than mis-reading fields it does not understand.
pub const MAX_SUPPORTED_SCHEMA_VERSION: u32 = 3;

/// Typed reasons [`parse_manifest`] can reject probe output. Mirrors the C#
/// `ManifestParseKind` (`Empty` / `Malformed` / `UnknownSchema`), with the missing-identity case
/// split out as its own variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestError {
    /// No bytes to parse (empty or all-whitespace input).
    Empty,
    /// Truncated, unreadable, or type-mismatched JSON. Carries the underlying serde message.
    BadJson(String),
    /// `schema_version` is newer than [`MAX_SUPPORTED_SCHEMA_VERSION`].
    UnsupportedSchema { version: u32, max: u32 },
    /// A required field was absent or blank (currently only `image_id`).
    MissingField(String),
}

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ManifestError::Empty => write!(f, "empty manifest"),
            ManifestError::BadJson(msg) => write!(f, "malformed manifest json: {msg}"),
            ManifestError::UnsupportedSchema { version, max } => {
                write!(f, "unknown schema version {version} (max supported {max})")
            }
            ManifestError::MissingField(field) => write!(f, "missing required field: {field}"),
        }
    }
}

impl std::error::Error for ManifestError {}

/// Parse probe JSON into an [`ImageManifest`].
///
/// Accepts both the canonical serialized form and the raw probe form (camelCase keys,
/// `{ "name": ... }` package objects) — see [`ImageManifest`]'s deserializer. The parsed manifest
/// is left as-written; call [`ImageManifest::sanitize`] to normalize it for matching.
///
/// # Errors
/// - [`ManifestError::Empty`] when `json` is empty or all whitespace.
/// - [`ManifestError::BadJson`] when the bytes are not valid manifest JSON.
/// - [`ManifestError::MissingField`] when `image_id` is absent or blank.
/// - [`ManifestError::UnsupportedSchema`] when `schema_version` exceeds
///   [`MAX_SUPPORTED_SCHEMA_VERSION`].
pub fn parse_manifest(json: &str) -> Result<ImageManifest, ManifestError> {
    if json.trim().is_empty() {
        return Err(ManifestError::Empty);
    }

    let manifest: ImageManifest =
        serde_json::from_str(json).map_err(|e| ManifestError::BadJson(e.to_string()))?;

    if manifest.image_id.trim().is_empty() {
        return Err(ManifestError::MissingField("image_id".to_string()));
    }

    if manifest.schema_version > MAX_SUPPORTED_SCHEMA_VERSION {
        return Err(ManifestError::UnsupportedSchema {
            version: manifest.schema_version,
            max: MAX_SUPPORTED_SCHEMA_VERSION,
        });
    }

    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_valid_manifest() {
        let m = parse_manifest(r#"{"schemaVersion":3,"imageID":"images.canfar.net/skaha/base:1.0"}"#)
            .unwrap();
        assert_eq!(m.image_id, "images.canfar.net/skaha/base:1.0");
        assert_eq!(m.schema_version, 3);
    }

    #[test]
    fn empty_input_is_empty_error() {
        assert_eq!(parse_manifest(""), Err(ManifestError::Empty));
        assert_eq!(parse_manifest("   \n\t "), Err(ManifestError::Empty));
    }

    #[test]
    fn empty_package_sets_are_success_not_failure() {
        let json = r#"{
            "schemaVersion": 3,
            "imageID": "img:1",
            "dpkgPackages": [], "rpmPackages": [], "apkPackages": [],
            "pythonPackages": [], "rPackages": [], "condaEnvs": [], "capabilities": []
        }"#;
        let m = parse_manifest(json).unwrap();
        assert!(m.dpkg.is_empty());
        assert!(m.all_package_names().is_empty());
    }

    #[test]
    fn malformed_json_is_bad_json_error() {
        match parse_manifest("{not json") {
            Err(ManifestError::BadJson(_)) => {}
            other => panic!("expected BadJson, got {other:?}"),
        }
        // Type mismatch (schemaVersion as a string) is also malformed.
        match parse_manifest(r#"{"schemaVersion":"three","imageID":"x:1"}"#) {
            Err(ManifestError::BadJson(_)) => {}
            other => panic!("expected BadJson, got {other:?}"),
        }
    }

    #[test]
    fn missing_image_id_is_missing_field_error() {
        assert_eq!(
            parse_manifest(r#"{"schemaVersion":1}"#),
            Err(ManifestError::MissingField("image_id".to_string()))
        );
        assert_eq!(
            parse_manifest(r#"{"imageID":"   "}"#),
            Err(ManifestError::MissingField("image_id".to_string()))
        );
    }

    #[test]
    fn newer_schema_is_rejected() {
        let json = r#"{"schemaVersion":99,"imageID":"img:1"}"#;
        assert_eq!(
            parse_manifest(json),
            Err(ManifestError::UnsupportedSchema {
                version: 99,
                max: MAX_SUPPORTED_SCHEMA_VERSION,
            })
        );
    }

    #[test]
    fn max_supported_schema_is_accepted() {
        let json = format!(
            r#"{{"schemaVersion":{MAX_SUPPORTED_SCHEMA_VERSION},"imageID":"img:1"}}"#
        );
        assert!(parse_manifest(&json).is_ok());
    }

    #[test]
    fn error_display_is_human_readable() {
        assert_eq!(ManifestError::Empty.to_string(), "empty manifest");
        assert_eq!(
            ManifestError::UnsupportedSchema { version: 9, max: 3 }.to_string(),
            "unknown schema version 9 (max supported 3)"
        );
        assert_eq!(
            ManifestError::MissingField("image_id".to_string()).to_string(),
            "missing required field: image_id"
        );
    }
}
