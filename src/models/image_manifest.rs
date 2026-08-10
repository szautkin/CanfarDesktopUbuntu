//! Shared image-discovery types (Agent A's canonical deliverable).
//!
//! Ported from the CanfarDesktop (Windows/WPF) reference:
//!   Models/ImageDiscovery/ImageManifest.cs   (manifest + capability keys)
//!   Models/ImageDiscovery/PackageQuery.cs     (intersection match + coverage score)
//!   Models/ImageDiscovery/LastOutcome.cs       (success / typed-failure cache record)
//!
//! This is the fixed contract imported by the manifest store, the discovery coordinator, and the
//! facet/search UI. Package lists are flat, name-only vectors — the Windows reference keeps
//! `(name, version)` records, but the Linux port matches on names only, which is all the query
//! surface needs. The deserializer stays compatible with the in-container probe, which emits
//! camelCase keys and `{ "name": ..., "version": ... }` package objects (see
//! `Resources/ImageDiscovery/probe.sh`): both the object form and bare-string form are accepted.

use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeMap;

/// Structured snapshot of what is installed inside a Skaha container image, produced by the
/// in-container probe. Every field carries a serde default so manifests written by older/newer
/// probe versions still deserialize (forward/backward compatible).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ImageManifest {
    #[serde(alias = "schemaVersion")]
    pub schema_version: u32,
    #[serde(alias = "imageID")]
    pub image_id: String,
    pub os_family: Option<String>,
    pub os_version: Option<String>,
    pub os_release: Option<String>,
    pub kernel: Option<String>,
    #[serde(alias = "dpkgPackages", deserialize_with = "de_names")]
    pub dpkg: Vec<String>,
    #[serde(alias = "rpmPackages", deserialize_with = "de_names")]
    pub rpm: Vec<String>,
    #[serde(alias = "apkPackages", deserialize_with = "de_names")]
    pub apk: Vec<String>,
    #[serde(alias = "pythonPackages", deserialize_with = "de_names")]
    pub python: Vec<String>,
    /// Per-conda-env python package names (env name -> package names).
    pub python_by_env: BTreeMap<String, Vec<String>>,
    #[serde(alias = "rPackages", deserialize_with = "de_names")]
    pub r_packages: Vec<String>,
    /// Conda environment names present in the image.
    #[serde(alias = "condaEnvs", deserialize_with = "de_names")]
    pub conda_envs: Vec<String>,
    pub capabilities: Vec<String>,
    pub shells: Vec<String>,
}

impl Default for ImageManifest {
    fn default() -> Self {
        // Schema version defaults to 1 (parity with the C# record's `SchemaVersion = 1`), so a
        // manifest that omits the field is treated as the oldest known schema, not "0".
        ImageManifest {
            schema_version: 1,
            image_id: String::new(),
            os_family: None,
            os_version: None,
            os_release: None,
            kernel: None,
            dpkg: Vec::new(),
            rpm: Vec::new(),
            apk: Vec::new(),
            python: Vec::new(),
            python_by_env: BTreeMap::new(),
            r_packages: Vec::new(),
            conda_envs: Vec::new(),
            capabilities: Vec::new(),
            shells: Vec::new(),
        }
    }
}

impl ImageManifest {
    /// Normalize in place: trim + lowercase every entry, drop blanks, then sort and de-duplicate
    /// every package/capability list (including each per-env python list) and the OS/kernel
    /// strings. Idempotent — re-sanitizing a normalized manifest is a no-op. Produces a canonical,
    /// case-insensitively comparable manifest (the reference `ImageManifest.Sanitize` intent).
    pub fn sanitize(&mut self) {
        normalize_opt(&mut self.os_family);
        normalize_opt(&mut self.os_version);
        normalize_opt(&mut self.os_release);
        normalize_opt(&mut self.kernel);
        self.image_id = self.image_id.trim().to_string();

        normalize_list(&mut self.dpkg);
        normalize_list(&mut self.rpm);
        normalize_list(&mut self.apk);
        normalize_list(&mut self.python);
        normalize_list(&mut self.r_packages);
        normalize_list(&mut self.conda_envs);
        normalize_list(&mut self.capabilities);
        normalize_list(&mut self.shells);

        // Re-key python_by_env under trimmed/lowercased env names, merging any collisions.
        let mut merged: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (env, pkgs) in std::mem::take(&mut self.python_by_env) {
            let key = env.trim().to_lowercase();
            if key.is_empty() {
                continue;
            }
            merged.entry(key).or_default().extend(pkgs);
        }
        for pkgs in merged.values_mut() {
            normalize_list(pkgs);
        }
        self.python_by_env = merged;
    }

    /// Every distinct package name across all package families (dpkg, rpm, apk, python — flat and
    /// per-env — and R), sorted and de-duplicated. This is the name universe a
    /// [`PackageQuery::packages`] term is matched against.
    pub fn all_package_names(&self) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        for group in [
            &self.dpkg,
            &self.rpm,
            &self.apk,
            &self.python,
            &self.r_packages,
        ] {
            names.extend(group.iter().cloned());
        }
        for v in self.python_by_env.values() {
            names.extend(v.iter().cloned());
        }
        names.sort();
        names.dedup();
        names
    }

    /// Whether the image ships a usable Python (a flat pip snapshot, a per-env snapshot, or the
    /// `python3` capability flag).
    pub fn has_python(&self) -> bool {
        !self.python.is_empty()
            || self.python_by_env.values().any(|v| !v.is_empty())
            || self.has_capability(capability::PYTHON3)
    }

    /// Whether the image ships R (installed R packages or the `rscript` capability flag).
    pub fn has_r(&self) -> bool {
        !self.r_packages.is_empty() || self.has_capability(capability::RSCRIPT)
    }

    /// True when the image advertises `cap`, case-insensitively.
    pub fn has_capability(&self, cap: &str) -> bool {
        self.capabilities
            .iter()
            .any(|c| c.eq_ignore_ascii_case(cap))
    }

    /// True when some installed package name contains `needle` case-insensitively. This covers
    /// exact matches (a string contains itself) as well as substring queries such as `cfitsio`
    /// hitting `libcfitsio-dev`.
    fn contains_package(&self, needle: &str) -> bool {
        let needle = needle.trim().to_lowercase();
        if needle.is_empty() {
            return true;
        }
        let hit = |group: &[String]| group.iter().any(|n| n.to_lowercase().contains(&needle));
        hit(&self.dpkg)
            || hit(&self.rpm)
            || hit(&self.apk)
            || hit(&self.python)
            || hit(&self.r_packages)
            || self
                .python_by_env
                .values()
                .any(|v| v.iter().any(|n| n.to_lowercase().contains(&needle)))
    }
}

/// Canonical behavioural capability keys the probe tests for. Mirrors `ImageCapability` in the
/// Windows reference, plus `jupyter` for the notebook-launch facet.
pub mod capability {
    pub const FITSIO: &str = "fitsio";
    pub const PHOTUTILS_ITERATIVE_PSF: &str = "photutils-iterative-psf";
    pub const GPU: &str = "gpu";
    pub const JUPYTER: &str = "jupyter";
    pub const PYTHON3: &str = "python3";
    pub const CONDA: &str = "conda";
    pub const RSCRIPT: &str = "rscript";

    /// All known capability keys, in a stable order.
    pub const ALL: &[&str] = &[
        FITSIO,
        PHOTUTILS_ITERATIVE_PSF,
        GPU,
        JUPYTER,
        PYTHON3,
        CONDA,
        RSCRIPT,
    ];
}

/// Search criteria for finding images that contain a set of required packages / capabilities.
/// Name-only intersection: an image matches when its manifest satisfies *every* populated
/// constraint. [`PackageQuery::score`] additionally reports partial coverage for near-miss images.
///
/// Package terms match case-insensitively by substring-or-exact (a friendlier superset of the
/// C# exact-membership `IsSubsetOf`); capability and OS terms match case-insensitively exact.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PackageQuery {
    /// Package names required across any family (dpkg/rpm/apk/python/R).
    pub packages: Vec<String>,
    /// Required capability keys (see [`capability`]).
    pub capabilities: Vec<String>,
    /// Required OS family (case-insensitive match against the manifest's `os_family`).
    pub os_family: Option<String>,
    /// Required OS version (case-insensitive match against the manifest's `os_version`).
    pub os_version: Option<String>,
    /// `Some(true)` requires python present, `Some(false)` requires it absent.
    pub python: Option<bool>,
    /// `Some(true)` requires R present, `Some(false)` requires it absent.
    pub r: Option<bool>,
}

impl PackageQuery {
    /// True when no constraint is populated. An empty query trivially matches every manifest
    /// (satisfied and total terms are both zero).
    pub fn is_empty(&self) -> bool {
        self.packages.is_empty()
            && self.capabilities.is_empty()
            && self.os_family.is_none()
            && self.os_version.is_none()
            && self.python.is_none()
            && self.r.is_none()
    }

    /// `(satisfied, total)` individual constraint terms for `m`. Each package name and each
    /// capability counts as one term; the OS/python/R filters each count as one term when
    /// populated.
    fn tally(&self, m: &ImageManifest) -> (u32, u32) {
        let mut satisfied = 0u32;
        let mut total = 0u32;

        if let Some(fam) = &self.os_family {
            total += 1;
            if opt_eq_ignore_case(&m.os_family, fam) {
                satisfied += 1;
            }
        }
        if let Some(ver) = &self.os_version {
            total += 1;
            if opt_eq_ignore_case(&m.os_version, ver) {
                satisfied += 1;
            }
        }
        for pkg in &self.packages {
            total += 1;
            if m.contains_package(pkg) {
                satisfied += 1;
            }
        }
        for cap in &self.capabilities {
            total += 1;
            if m.has_capability(cap) {
                satisfied += 1;
            }
        }
        if let Some(want) = self.python {
            total += 1;
            if m.has_python() == want {
                satisfied += 1;
            }
        }
        if let Some(want) = self.r {
            total += 1;
            if m.has_r() == want {
                satisfied += 1;
            }
        }

        (satisfied, total)
    }

    /// True when the manifest satisfies every populated constraint.
    pub fn matches(&self, m: &ImageManifest) -> bool {
        let (satisfied, total) = self.tally(m);
        satisfied == total
    }

    /// Number of individually satisfied constraint terms (0..=`total_terms`).
    pub fn score(&self, m: &ImageManifest) -> u32 {
        self.tally(m).0
    }

    /// Total number of populated terms (the denominator for a coverage fraction).
    pub fn total_terms(&self) -> u32 {
        let mut total = self.packages.len() as u32 + self.capabilities.len() as u32;
        total += self.os_family.is_some() as u32;
        total += self.os_version.is_some() as u32;
        total += self.python.is_some() as u32;
        total += self.r.is_some() as u32;
        total
    }
}

/// What the cache last knew about an image: a successful manifest, or a typed failure carrying an
/// optional job id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DiscoveryOutcome {
    Manifest(ImageManifest),
    Failure {
        category: String,
        message: String,
        job_id: Option<String>,
    },
}

impl DiscoveryOutcome {
    /// True when this outcome carries a manifest.
    pub fn is_success(&self) -> bool {
        matches!(self, DiscoveryOutcome::Manifest(_))
    }

    /// The manifest, if this outcome is a success.
    pub fn manifest(&self) -> Option<&ImageManifest> {
        match self {
            DiscoveryOutcome::Manifest(m) => Some(m),
            DiscoveryOutcome::Failure { .. } => None,
        }
    }
}

/// The last recorded discovery outcome for one image, with the RFC-3339 timestamp of when it was
/// recorded (passed in by the caller — this module is time-agnostic).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LastOutcome {
    pub image_id: String,
    pub outcome: DiscoveryOutcome,
    pub discovered_at: String,
}

impl LastOutcome {
    /// Record a successful discovery for `manifest`, timestamped `discovered_at` (RFC-3339).
    pub fn success(manifest: ImageManifest, discovered_at: impl Into<String>) -> Self {
        LastOutcome {
            image_id: manifest.image_id.clone(),
            outcome: DiscoveryOutcome::Manifest(manifest),
            discovered_at: discovered_at.into(),
        }
    }

    /// Record a typed failure for `image_id`, timestamped `discovered_at` (RFC-3339).
    pub fn failure(
        image_id: impl Into<String>,
        category: impl Into<String>,
        message: impl Into<String>,
        job_id: Option<String>,
        discovered_at: impl Into<String>,
    ) -> Self {
        LastOutcome {
            image_id: image_id.into(),
            outcome: DiscoveryOutcome::Failure {
                category: category.into(),
                message: message.into(),
                job_id,
            },
            discovered_at: discovered_at.into(),
        }
    }

    /// The successful manifest, if this outcome is a success.
    pub fn manifest(&self) -> Option<&ImageManifest> {
        self.outcome.manifest()
    }

    /// True when this outcome carries a manifest.
    pub fn is_success(&self) -> bool {
        self.outcome.is_success()
    }
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn normalize_list(list: &mut Vec<String>) {
    for s in list.iter_mut() {
        *s = s.trim().to_lowercase();
    }
    list.retain(|s| !s.is_empty());
    list.sort();
    list.dedup();
}

fn normalize_opt(value: &mut Option<String>) {
    if let Some(v) = value {
        let normalized = v.trim().to_lowercase();
        if normalized.is_empty() {
            *value = None;
        } else {
            *v = normalized;
        }
    }
}

fn opt_eq_ignore_case(actual: &Option<String>, expected: &str) -> bool {
    match actual {
        Some(a) => a.trim().eq_ignore_ascii_case(expected.trim()),
        None => false,
    }
}

/// Deserialize a package list that may be either bare strings (the canonical Rust form) or the
/// probe's `{ "name": ..., ... }` / `{ "Name": ..., ... }` objects — extra object fields are
/// ignored.
fn de_names<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    struct Named {
        #[serde(alias = "Name")]
        name: String,
    }

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum NameOrObj {
        Bare(String),
        Named(Named),
    }

    let items = Vec::<NameOrObj>::deserialize(deserializer)?;
    Ok(items
        .into_iter()
        .map(|item| match item {
            NameOrObj::Bare(s) => s,
            NameOrObj::Named(n) => n.name,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ubuntu_manifest() -> ImageManifest {
        ImageManifest {
            image_id: "images.canfar.net/skaha/astroml:24.07".to_string(),
            os_family: Some("ubuntu".to_string()),
            os_version: Some("22.04".to_string()),
            dpkg: vec!["libcfitsio-dev".to_string(), "gcc".to_string()],
            python: vec!["numpy".to_string(), "astropy".to_string()],
            r_packages: vec!["ggplot2".to_string()],
            capabilities: vec![
                capability::GPU.to_string(),
                capability::PYTHON3.to_string(),
                capability::JUPYTER.to_string(),
            ],
            ..Default::default()
        }
    }

    #[test]
    fn matches_all_populated_constraints() {
        let m = ubuntu_manifest();
        let q = PackageQuery {
            packages: vec!["numpy".to_string(), "gcc".to_string()],
            capabilities: vec![capability::GPU.to_string()],
            os_family: Some("Ubuntu".to_string()), // case-insensitive
            python: Some(true),
            r: Some(true),
            ..Default::default()
        };
        assert!(q.matches(&m));
        assert_eq!(q.total_terms(), 6);
        assert_eq!(q.score(&m), 6);
    }

    #[test]
    fn matches_substring_and_case_insensitive() {
        let m = ubuntu_manifest();
        // "cfitsio" is a substring of "libcfitsio-dev"; "NUMPY" differs only in case.
        let q = PackageQuery {
            packages: vec!["cfitsio".to_string(), "NUMPY".to_string()],
            ..Default::default()
        };
        assert!(q.matches(&m));
        assert_eq!(q.score(&m), 2);
    }

    #[test]
    fn fails_when_one_package_missing() {
        let m = ubuntu_manifest();
        let q = PackageQuery {
            packages: vec!["numpy".to_string(), "tensorflow".to_string()],
            ..Default::default()
        };
        assert!(!q.matches(&m));
        // numpy satisfied, tensorflow not.
        assert_eq!(q.score(&m), 1);
        assert_eq!(q.total_terms(), 2);
    }

    #[test]
    fn fails_on_wrong_os_or_capability() {
        let m = ubuntu_manifest();
        let bad_os = PackageQuery {
            os_family: Some("rockylinux".to_string()),
            ..Default::default()
        };
        assert!(!bad_os.matches(&m));
        assert_eq!(bad_os.score(&m), 0);

        let bad_cap = PackageQuery {
            capabilities: vec![capability::CONDA.to_string()],
            ..Default::default()
        };
        assert!(!bad_cap.matches(&m));
        assert_eq!(bad_cap.score(&m), 0);
    }

    #[test]
    fn python_and_r_presence_flags() {
        let m = ubuntu_manifest();
        assert!(m.has_python());
        assert!(m.has_r());

        let bare = ImageManifest {
            image_id: "x".to_string(),
            ..Default::default()
        };
        assert!(!bare.has_python());
        assert!(!bare.has_r());

        let want_no_python = PackageQuery {
            python: Some(false),
            ..Default::default()
        };
        assert!(want_no_python.matches(&bare));
        assert!(!want_no_python.matches(&m));
    }

    #[test]
    fn empty_query_matches_but_scores_zero() {
        let m = ubuntu_manifest();
        let q = PackageQuery::default();
        assert!(q.is_empty());
        assert!(q.matches(&m));
        assert_eq!(q.score(&m), 0);
        assert_eq!(q.total_terms(), 0);
    }

    #[test]
    fn sanitize_trims_lowercases_dedups_sorts() {
        let mut m = ImageManifest {
            image_id: "  images.canfar.net/skaha/astroml:24.07  ".to_string(),
            os_family: Some("  Ubuntu  ".to_string()),
            os_version: Some("   ".to_string()), // becomes None
            dpkg: vec![
                "  GCC  ".to_string(),
                "gcc".to_string(),
                "".to_string(),
                "Zlib".to_string(),
            ],
            capabilities: vec!["GPU".to_string(), "gpu".to_string()],
            ..Default::default()
        };
        m.python_by_env.insert(
            "  Base  ".to_string(),
            vec!["NumPy".to_string(), "numpy".to_string()],
        );
        m.sanitize();

        assert_eq!(m.image_id, "images.canfar.net/skaha/astroml:24.07");
        assert_eq!(m.os_family.as_deref(), Some("ubuntu"));
        assert_eq!(m.os_version, None);
        assert_eq!(m.dpkg, vec!["gcc".to_string(), "zlib".to_string()]);
        assert_eq!(m.capabilities, vec!["gpu".to_string()]);
        assert_eq!(
            m.python_by_env.get("base"),
            Some(&vec!["numpy".to_string()])
        );

        // Idempotent.
        let mut again = m.clone();
        again.sanitize();
        assert_eq!(again, m);
    }

    #[test]
    fn deserializes_probe_object_form() {
        // Probe emits camelCase keys and {name, version} objects.
        let json = r#"{
            "schemaVersion": 3,
            "imageID": "images.canfar.net/skaha/base:1.0",
            "osFamily": "ubuntu",
            "dpkgPackages": [{"name":"gcc","version":"12"},{"name":"make","version":"4"}],
            "pythonPackages": [{"name":"numpy","version":"1.26","source":"pip","env":"base"}],
            "rPackages": [],
            "capabilities": ["gpu","python3"]
        }"#;
        let m: ImageManifest = serde_json::from_str(json).unwrap();
        assert_eq!(m.schema_version, 3);
        assert_eq!(m.image_id, "images.canfar.net/skaha/base:1.0");
        assert_eq!(m.dpkg, vec!["gcc".to_string(), "make".to_string()]);
        assert_eq!(m.python, vec!["numpy".to_string()]);
    }

    #[test]
    fn round_trips_canonical_string_form() {
        let m = ubuntu_manifest();
        let json = serde_json::to_string(&m).unwrap();
        let back: ImageManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(m, back);
    }

    #[test]
    fn missing_schema_version_defaults_to_one() {
        let m: ImageManifest = serde_json::from_str(r#"{"imageID":"x/y:1"}"#).unwrap();
        assert_eq!(m.schema_version, 1);
    }

    #[test]
    fn last_outcome_success_and_failure() {
        let ok = LastOutcome::success(ubuntu_manifest(), "2026-07-07T00:00:00Z");
        assert!(ok.is_success());
        assert_eq!(ok.image_id, "images.canfar.net/skaha/astroml:24.07");
        assert!(ok.manifest().is_some());

        let bad = LastOutcome::failure(
            "img:1",
            "ManifestParseFailed",
            "boom",
            Some("job-42".to_string()),
            "2026-07-07T00:00:00Z",
        );
        assert!(!bad.is_success());
        assert!(bad.manifest().is_none());

        // Round-trips through JSON.
        let json = serde_json::to_string(&bad).unwrap();
        let back: LastOutcome = serde_json::from_str(&json).unwrap();
        assert_eq!(bad, back);
    }
}
