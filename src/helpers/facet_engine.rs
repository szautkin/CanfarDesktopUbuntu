//! Pure faceting for the find-by-package dialog's left filter pane.
//!
//! Port of `Helpers/ImageDiscovery/FacetEngine.cs` (+ the section-ordering bits
//! of `ImageDiscoveryViewModel.BuildSections`). Given the discovery cache
//! ([`JsonManifestStore`]) it groups every value present in the discovered
//! manifests into the pinned facet categories (OS family, OS version, Python, R,
//! dpkg, rpm, apk, Capabilities) with per-value result counts, and — for a live
//! [`PackageQuery`] — recomputes those counts and greys out (marks
//! `enabled = false`) any value that would collapse the result set to zero.
//!
//! WinUI/GTK-free so the whole interaction model stays headlessly unit-testable
//! (mirrors the reason the reference keeps `FacetEngine` free of WinUI).

use crate::models::image_manifest::{ImageManifest, PackageQuery};
use crate::services::manifest_store::JsonManifestStore;
use std::collections::{BTreeMap, HashSet};

/// One selectable value inside a [`Facet`], with the number of discovered images
/// it would still match (`count`) and whether ticking it keeps results non-empty
/// (`enabled` — `false` renders the checkbox greyed out).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FacetValue {
    pub value: String,
    pub count: usize,
    pub enabled: bool,
}

/// One category section in the filter pane (e.g. `"Python"`), with its values in
/// ascending order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Facet {
    pub category: String,
    pub values: Vec<FacetValue>,
}

/// The pinned facet categories, in the exact order the left pane presents them
/// (mirrors `ImageDiscoveryViewModel.SectionOrder`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Category {
    OsFamily,
    OsVersion,
    Python,
    R,
    Dpkg,
    Rpm,
    Apk,
    Capabilities,
}

impl Category {
    const ORDER: [Category; 8] = [
        Category::OsFamily,
        Category::OsVersion,
        Category::Python,
        Category::R,
        Category::Dpkg,
        Category::Rpm,
        Category::Apk,
        Category::Capabilities,
    ];

    /// Display label for the section header.
    fn label(self) -> &'static str {
        match self {
            Category::OsFamily => "OS family",
            Category::OsVersion => "OS version",
            Category::Python => "Python",
            Category::R => "R",
            Category::Dpkg => "System (apt / dpkg)",
            Category::Rpm => "System (rpm)",
            Category::Apk => "System (apk)",
            Category::Capabilities => "Capabilities",
        }
    }

    /// The distinct values this manifest contributes to this category. OS
    /// family/version skip the sentinel `"unknown"`; Python unions the flat pip
    /// snapshot with every per-conda-env snapshot (matching the store's package
    /// universe).
    fn values_of(self, m: &ImageManifest) -> Vec<String> {
        match self {
            Category::OsFamily => opt_value(&m.os_family),
            Category::OsVersion => opt_value(&m.os_version),
            Category::Python => {
                let mut v: Vec<String> = m.python.clone();
                for pkgs in m.python_by_env.values() {
                    v.extend(pkgs.iter().cloned());
                }
                v
            }
            Category::R => m.r_packages.clone(),
            Category::Dpkg => m.dpkg.clone(),
            Category::Rpm => m.rpm.clone(),
            Category::Apk => m.apk.clone(),
            Category::Capabilities => m.capabilities.clone(),
        }
    }

    /// A copy of `q` with this category's own constraint removed, so faceting a
    /// value never greys itself out. The five package families share the flat
    /// [`PackageQuery::packages`] list, so dropping any of them clears the whole
    /// list (the honest scope given the name-only query model).
    fn scoped_without(self, q: &PackageQuery) -> PackageQuery {
        let mut s = q.clone();
        match self {
            Category::OsFamily => s.os_family = None,
            Category::OsVersion => s.os_version = None,
            Category::Capabilities => s.capabilities.clear(),
            Category::Python
            | Category::R
            | Category::Dpkg
            | Category::Rpm
            | Category::Apk => s.packages.clear(),
        }
        s
    }

    /// Whether `value` is currently selected in `q` for this category. A selected
    /// value stays enabled even if it would otherwise collapse the result set.
    fn is_selected(self, q: &PackageQuery, value: &str) -> bool {
        match self {
            Category::OsFamily => opt_eq(&q.os_family, value),
            Category::OsVersion => opt_eq(&q.os_version, value),
            Category::Capabilities => {
                q.capabilities.iter().any(|c| c.eq_ignore_ascii_case(value))
            }
            Category::Python
            | Category::R
            | Category::Dpkg
            | Category::Rpm
            | Category::Apk => q.packages.iter().any(|p| p.eq_ignore_ascii_case(value)),
        }
    }
}

/// Build the full facet list from every cached *successful* manifest, with each
/// value counted across the images that contain it and every value enabled (no
/// query is applied). Empty categories are omitted.
///
/// Part of the public faceting contract (the dialog drives its live pane through
/// [`facets_for_query`]); retained for callers that want the unfiltered universe.
#[allow(dead_code)]
pub fn build_facets(store: &JsonManifestStore) -> Vec<Facet> {
    let manifests = discovered_manifests(store);
    facets_from(&manifests, None)
}

/// Recompute the facets against a live query: counts are scoped to the images
/// that match the OTHER active constraints, and any value that would drop the
/// result set to zero is marked `enabled = false` (unless it is already ticked).
/// The value universe stays the full catalogue so a filtered-out value can still
/// be un-ticked. Empty categories are omitted.
pub fn facets_for_query(store: &JsonManifestStore, q: &PackageQuery) -> Vec<Facet> {
    let manifests = discovered_manifests(store);
    facets_from(&manifests, Some(q))
}

// ---------------------------------------------------------------------------
// internals
// ---------------------------------------------------------------------------

/// Every cached successful manifest (failures carry no manifest and are skipped).
fn discovered_manifests(store: &JsonManifestStore) -> Vec<ImageManifest> {
    store
        .known_images()
        .into_iter()
        .filter_map(|id| store.get(&id).and_then(|o| o.manifest().cloned()))
        .collect()
}

/// Shared facet builder. With `query = None` every value is enabled and counted
/// across all manifests; with `query = Some(q)` counts are scoped to the
/// query-minus-this-category matches and unreachable values are greyed out.
fn facets_from(manifests: &[ImageManifest], query: Option<&PackageQuery>) -> Vec<Facet> {
    let mut facets = Vec::new();
    for category in Category::ORDER {
        // The full value universe for this category (all discovered manifests),
        // so an already-ticked value is always present to un-tick.
        let mut universe: BTreeMap<String, ()> = BTreeMap::new();
        for m in manifests {
            for v in category.values_of(m) {
                universe.insert(v, ());
            }
        }
        if universe.is_empty() {
            continue;
        }

        // Which manifests + values are reachable given the rest of the query.
        let (reachable_counts, has_query) = match query {
            Some(q) => (scoped_counts(category, q, manifests), true),
            None => (unscoped_counts(category, manifests), false),
        };

        let mut values = Vec::with_capacity(universe.len());
        for value in universe.into_keys() {
            let count = reachable_counts.get(&value).copied().unwrap_or(0);
            let enabled = if has_query {
                count > 0 || query.map(|q| category.is_selected(q, &value)).unwrap_or(false)
            } else {
                true
            };
            values.push(FacetValue {
                value,
                count,
                enabled,
            });
        }

        facets.push(Facet {
            category: category.label().to_string(),
            values,
        });
    }
    facets
}

/// Per-value image counts with no query applied: how many manifests contain each
/// value of this category.
fn unscoped_counts(category: Category, manifests: &[ImageManifest]) -> BTreeMap<String, usize> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for m in manifests {
        for v in distinct(category.values_of(m)) {
            *counts.entry(v).or_insert(0) += 1;
        }
    }
    counts
}

/// Per-value image counts scoped to the manifests that satisfy `q` with this
/// category's own constraint dropped — the count a checkbox would narrow to.
fn scoped_counts(
    category: Category,
    q: &PackageQuery,
    manifests: &[ImageManifest],
) -> BTreeMap<String, usize> {
    let scoped = category.scoped_without(q);
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for m in manifests {
        if !scoped.matches(m) {
            continue;
        }
        for v in distinct(category.values_of(m)) {
            *counts.entry(v).or_insert(0) += 1;
        }
    }
    counts
}

/// De-duplicate one manifest's contribution so each image counts a value once.
fn distinct(values: Vec<String>) -> HashSet<String> {
    values.into_iter().collect()
}

/// A non-`unknown`, non-empty option as a single-element value list.
fn opt_value(value: &Option<String>) -> Vec<String> {
    match value {
        Some(v) if !v.is_empty() && v != "unknown" => vec![v.clone()],
        _ => Vec::new(),
    }
}

fn opt_eq(actual: &Option<String>, expected: &str) -> bool {
    actual
        .as_deref()
        .map(|a| a.eq_ignore_ascii_case(expected))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct TempStore {
        path: PathBuf,
    }
    impl TempStore {
        fn new() -> Self {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "verbinal_facet_test_{}_{}_{}",
                std::process::id(),
                nanos,
                n
            ));
            TempStore { path }
        }
        fn store(&self) -> JsonManifestStore {
            JsonManifestStore::with_dir(self.path.clone())
        }
    }
    impl Drop for TempStore {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    const AT: &str = "2026-07-07T00:00:00Z";

    fn manifest(id: &str, os_family: &str, os_version: &str, python: &[&str]) -> ImageManifest {
        ImageManifest {
            image_id: id.to_string(),
            os_family: Some(os_family.to_string()),
            os_version: Some(os_version.to_string()),
            python: python.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    fn facet<'a>(facets: &'a [Facet], category: &str) -> &'a Facet {
        facets
            .iter()
            .find(|f| f.category == category)
            .unwrap_or_else(|| panic!("missing facet {category}"))
    }

    fn value<'a>(f: &'a Facet, v: &str) -> &'a FacetValue {
        f.values
            .iter()
            .find(|fv| fv.value == v)
            .unwrap_or_else(|| panic!("missing value {v}"))
    }

    #[test]
    fn build_facets_groups_values_with_counts() {
        let tmp = TempStore::new();
        let store = tmp.store();
        store.set_manifest(
            "a:1",
            manifest("a:1", "ubuntu", "22.04", &["numpy", "astropy"]),
            AT.into(),
        );
        store.set_manifest(
            "b:1",
            manifest("b:1", "ubuntu", "24.04", &["numpy"]),
            AT.into(),
        );
        // A failure contributes nothing to the facets.
        store.set_failure("c:1", "Unknown", "boom", None, AT.into());

        let facets = build_facets(&store);

        // OS family: a single "ubuntu" value present in both images.
        let os = facet(&facets, "OS family");
        assert_eq!(os.values.len(), 1);
        assert_eq!(value(os, "ubuntu").count, 2);
        assert!(value(os, "ubuntu").enabled);

        // OS version: two distinct versions, one image each.
        let ver = facet(&facets, "OS version");
        assert_eq!(value(ver, "22.04").count, 1);
        assert_eq!(value(ver, "24.04").count, 1);

        // Python: numpy in both, astropy in one; sorted ascending.
        let py = facet(&facets, "Python");
        assert_eq!(
            py.values.iter().map(|v| v.value.clone()).collect::<Vec<_>>(),
            vec!["astropy".to_string(), "numpy".to_string()]
        );
        assert_eq!(value(py, "numpy").count, 2);
        assert_eq!(value(py, "astropy").count, 1);

        // Categories with no values are omitted.
        assert!(facets.iter().all(|f| f.category != "System (rpm)"));
    }

    #[test]
    fn build_facets_unions_python_by_env() {
        let tmp = TempStore::new();
        let store = tmp.store();
        let mut m = manifest("a:1", "ubuntu", "22.04", &["numpy"]);
        m.python_by_env =
            BTreeMap::from([("ml".to_string(), vec!["torch".to_string()])]);
        m.capabilities = vec!["gpu".to_string()];
        store.set_manifest("a:1", m, AT.into());

        let facets = build_facets(&store);
        let py = facet(&facets, "Python");
        assert!(py.values.iter().any(|v| v.value == "torch"));
        assert!(py.values.iter().any(|v| v.value == "numpy"));

        let caps = facet(&facets, "Capabilities");
        assert_eq!(value(caps, "gpu").count, 1);
    }

    #[test]
    fn facets_for_query_greys_out_unreachable_values() {
        let tmp = TempStore::new();
        let store = tmp.store();
        store.set_manifest(
            "a:1",
            manifest("a:1", "ubuntu", "22.04", &["numpy", "astropy"]),
            AT.into(),
        );
        store.set_manifest(
            "b:1",
            manifest("b:1", "rockylinux", "9", &["numpy"]),
            AT.into(),
        );

        // Constrain to ubuntu: rockylinux's os_version "9" is now unreachable and
        // greyed, while ubuntu's "22.04" stays enabled.
        let q = PackageQuery {
            os_family: Some("ubuntu".to_string()),
            ..Default::default()
        };
        let facets = facets_for_query(&store, &q);

        let ver = facet(&facets, "OS version");
        let v22 = value(ver, "22.04");
        assert!(v22.enabled);
        assert_eq!(v22.count, 1);

        let v9 = value(ver, "9");
        assert!(!v9.enabled, "rockylinux-only version should be greyed out");
        assert_eq!(v9.count, 0);

        // astropy only exists on the ubuntu image, so it stays reachable; both
        // OS families remain visible in their own facet (self-category dropped).
        let py = facet(&facets, "Python");
        assert!(value(py, "astropy").enabled);
        let os = facet(&facets, "OS family");
        assert!(value(os, "ubuntu").enabled);
        assert!(value(os, "rockylinux").enabled);
    }

    #[test]
    fn facets_for_query_keeps_selected_value_enabled() {
        let tmp = TempStore::new();
        let store = tmp.store();
        store.set_manifest(
            "a:1",
            manifest("a:1", "ubuntu", "22.04", &["numpy"]),
            AT.into(),
        );
        let mut rocky = manifest("b:1", "rockylinux", "9", &[]);
        rocky.rpm = vec!["gcc".to_string()];
        store.set_manifest("b:1", rocky, AT.into());

        // Require an ubuntu OS AND the gcc package — but gcc only exists on the
        // rockylinux image, so within the OS-family facet (self-category dropped,
        // gcc kept) only rockylinux is reachable. "ubuntu" therefore scopes to
        // zero, yet stays enabled because it is the selected family (so the user
        // can un-tick it out of the dead-end).
        let q = PackageQuery {
            os_family: Some("ubuntu".to_string()),
            packages: vec!["gcc".to_string()],
            ..Default::default()
        };
        let facets = facets_for_query(&store, &q);
        let os = facet(&facets, "OS family");
        let ubuntu = value(os, "ubuntu");
        assert_eq!(ubuntu.count, 0, "no gcc-bearing ubuntu image exists");
        assert!(ubuntu.enabled, "selected value stays enabled");
        // rockylinux is reachable (it carries gcc) and stays enabled.
        assert!(value(os, "rockylinux").enabled);
    }
}
