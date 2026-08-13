//! Embedded probe / inspector scripts and their content-hashed upload names.
//!
//! Port of `Services/ImageDiscovery/EmbeddedProbeScripts.cs`. The script bodies
//! are compiled into the binary via [`include_str!`]; their VOSpace upload
//! filenames are content-hashed (`probe-<12hex>.sh`) so editing a script
//! automatically busts the previously-uploaded copy.

#[cfg(test)]
use sha2::{Digest, Sha256};

#[cfg(test)]
/// Home-relative subdirectory the probe writes manifests into (`~/.verbinal`).
///
/// Mirrors `EmbeddedProbeScripts.HomeSubdirectory` and the `.verbinal` path the
/// scripts themselves hard-code.
pub const HOME_SUBDIR: &str = ".verbinal";

/// In-container probe script (runs inside the target image; emits a manifest).
const PROBE_SCRIPT: &str = include_str!("../resources/imagedisc/probe.sh");

/// Out-of-band inspector script (syft-based static scan of a target image from a
/// known-good headless container).
const INSPECTOR_SCRIPT: &str = include_str!("../resources/imagedisc/inspector.sh");

/// Body of the in-container probe script.
pub fn probe_script() -> &'static str {
    PROBE_SCRIPT
}

/// Body of the out-of-band inspector script.
pub fn inspector_script() -> &'static str {
    INSPECTOR_SCRIPT
}

#[cfg(test)]
/// Lowercase hex SHA-256 digest of `text`.
fn sha256_hex(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}

#[cfg(test)]
/// Content-hashed upload filename for the probe script (`probe-<first-12-hex>.sh`).
///
/// Mirrors `EmbeddedProbeScripts.ProbeUploadFileName`.
pub fn probe_script_name() -> String {
    format!("probe-{}.sh", &sha256_hex(PROBE_SCRIPT)[..12])
}

#[cfg(test)]
/// Content-hashed upload filename for the inspector script
/// (`inspector-<first-12-hex>.sh`). Mirrors `InspectorUploadFileName`.
pub fn inspector_script_name() -> String {
    format!("inspector-{}.sh", &sha256_hex(INSPECTOR_SCRIPT)[..12])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_are_shebanged_and_carry_their_env_contract() {
        assert!(probe_script().starts_with("#!/usr/bin/env bash"));
        assert!(inspector_script().starts_with("#!/usr/bin/env bash"));
        // Each script keys off a launcher-supplied env var.
        assert!(probe_script().contains("IMAGE_ID"));
        assert!(inspector_script().contains("TARGET_IMAGE"));
    }

    #[test]
    fn sha256_hex_matches_known_vector() {
        assert_eq!(
            sha256_hex("hello"),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
        assert_eq!(&sha256_hex("hello")[..12], "2cf24dba5fb0");
    }

    #[test]
    fn probe_name_is_content_hashed_stable_and_well_formed() {
        let name = probe_script_name();
        assert!(name.starts_with("probe-"), "got {name}");
        assert!(name.ends_with(".sh"), "got {name}");
        // "probe-" (6) + 12 hex + ".sh" (3) == 21
        assert_eq!(name.len(), 21, "got {name}");
        // Deterministic across calls.
        assert_eq!(name, probe_script_name());
    }

    #[test]
    fn inspector_name_is_well_formed_and_distinct_from_probe() {
        let name = inspector_script_name();
        assert!(name.starts_with("inspector-"), "got {name}");
        assert!(name.ends_with(".sh"), "got {name}");
        // "inspector-" (10) + 12 hex + ".sh" (3) == 25
        assert_eq!(name.len(), 25, "got {name}");
        // Different bodies -> different hashes.
        assert_ne!(
            &probe_script_name()[6..18],
            &inspector_script_name()[10..22]
        );
    }

    #[test]
    fn home_subdir_agrees_with_script_output_path() {
        assert_eq!(HOME_SUBDIR, ".verbinal");
        assert!(probe_script().contains(".verbinal"));
        assert!(inspector_script().contains(".verbinal"));
    }
}
