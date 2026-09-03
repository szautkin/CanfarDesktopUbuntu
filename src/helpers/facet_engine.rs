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
use std::collections::{HashMap, HashSet};

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
    /// This category's values in `m`, BORROWED.
    ///
    /// Returned `Vec<String>` until it was measured: every value here is a
    /// package name already owned by the manifest, and the facet builder asks
    /// for them twice per category per manifest — once to build the value
    /// universe, once to count. Eight categories over a 65-image cache whose
    /// largest manifest holds 1,275 packages meant a few hundred thousand
    /// `String` clones per keystroke, which was 9.4 ms of the 11.3 ms the
    /// dialog spent rebuilding its facets. Borrowing costs one boxed iterator.
    fn values_of(self, m: &ImageManifest) -> Box<dyn Iterator<Item = &str> + '_> {
        match self {
            Category::OsFamily => Box::new(opt_value(&m.os_family)),
            Category::OsVersion => Box::new(opt_value(&m.os_version)),
            Category::Python => Box::new(
                m.python
                    .iter()
                    .map(String::as_str)
                    .chain(m.python_by_env.values().flatten().map(String::as_str)),
            ),
            Category::R => Box::new(m.r_packages.iter().map(String::as_str)),
            Category::Dpkg => Box::new(m.dpkg.iter().map(String::as_str)),
            Category::Rpm => Box::new(m.rpm.iter().map(String::as_str)),
            Category::Apk => Box::new(m.apk.iter().map(String::as_str)),
            Category::Capabilities => Box::new(m.capabilities.iter().map(String::as_str)),
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
            Category::Python | Category::R | Category::Dpkg | Category::Rpm | Category::Apk => {
                s.packages.clear()
            }
        }
        s
    }

    /// Whether `value` is currently selected in `q` for this category. A selected
    /// value stays enabled even if it would otherwise collapse the result set.
    fn is_selected(self, q: &PackageQuery, value: &str) -> bool {
        match self {
            Category::OsFamily => opt_eq(&q.os_family, value),
            Category::OsVersion => opt_eq(&q.os_version, value),
            Category::Capabilities => q.capabilities.iter().any(|c| c.eq_ignore_ascii_case(value)),
            Category::Python | Category::R | Category::Dpkg | Category::Rpm | Category::Apk => {
                q.packages.iter().any(|p| p.eq_ignore_ascii_case(value))
            }
        }
    }
}

/// Build the full facet list from every cached *successful* manifest, with each
/// value counted across the images that contain it and every value enabled (no
/// query is applied). Empty categories are omitted.
///
/// Part of the public faceting contract (the dialog drives its live pane through
/// [`facets_for_query`]); retained for callers that want the unfiltered universe.
/// Recompute the facets against a live query: counts are scoped to the images
/// that match the OTHER active constraints, and any value that would drop the
/// result set to zero is marked `enabled = false` (unless it is already ticked).
/// The value universe stays the full catalogue so a filtered-out value can still
/// be un-ticked. Empty categories are omitted.
pub fn facets_for_query(store: &JsonManifestStore, q: &PackageQuery) -> Vec<Facet> {
    // Borrowed, under one lock. This used to list the ids and `get()` each one,
    // deep-copying every manifest twice over — 11.3 ms a call on a 65-image
    // cache, on every keystroke in the dialog.
    store.with_manifests(|manifests| facets_from(manifests, Some(q)))
}

// ---------------------------------------------------------------------------
// internals
// ---------------------------------------------------------------------------

/// Shared facet builder. With `query = None` every value is enabled and counted
/// across all manifests; with `query = Some(q)` counts are scoped to the
/// query-minus-this-category matches and unreachable values are greyed out.
fn facets_from(manifests: &[&ImageManifest], query: Option<&PackageQuery>) -> Vec<Facet> {
    let mut facets = Vec::new();
    // One scratch set, cleared per manifest rather than allocated per manifest.
    // The old shape built a fresh `HashSet` for every (category, manifest) pair
    // — 1,596 allocations on a 266-image cache, per call.
    let mut seen: HashSet<&str> = HashSet::new();

    for category in Category::ORDER {
        // The query with THIS category's own constraint dropped, so faceting a
        // value never greys itself out. `None` means no query: everything is
        // reachable and every value stays enabled.
        let scoped = query.map(|q| category.scoped_without(q));

        // Value universe and reachable counts in ONE pass.
        //
        // They used to be two: a pass to collect the universe, then another to
        // count. Both walk every value of every manifest, so the second was the
        // same work twice — 26.9 ms on this developer's cache, over a frame,
        // and growing with the catalogue.
        // A hash map, sorted once at the end rather than kept ordered on every
        // insert: a `BTreeMap<&str, _>` pays string comparisons on all 6,769
        // values, and the pane only needs them ordered once.
        let mut universe: HashMap<&str, usize> = HashMap::new();
        for m in manifests {
            let reachable = scoped.as_ref().map(|s| s.matches(m)).unwrap_or(true);
            seen.clear();
            for value in category.values_of(m) {
                let count = universe.entry(value).or_insert(0);
                // Each manifest counts once per value, however many times it
                // lists it — and only if the rest of the query admits it.
                if reachable && seen.insert(value) {
                    *count += 1;
                }
            }
        }
        if universe.is_empty() {
            continue;
        }

        let has_query = query.is_some();
        let mut ordered: Vec<(&str, usize)> = universe.into_iter().collect();
        ordered.sort_unstable_by_key(|(value, _)| *value);
        let values = ordered
            .into_iter()
            .map(|(value, count)| FacetValue {
                // A ticked value stays enabled even when it would collapse the
                // results, so it can be un-ticked.
                enabled: !has_query
                    || count > 0
                    || query
                        .map(|q| category.is_selected(q, value))
                        .unwrap_or(false),
                value: value.to_string(),
                count,
            })
            .collect();

        facets.push(Facet {
            category: category.label().to_string(),
            values,
        });
    }
    facets
}

/// A non-`unknown`, non-empty option as a zero- or one-element borrowed value.
fn opt_value(value: &Option<String>) -> impl Iterator<Item = &str> {
    value
        .as_deref()
        .filter(|v| !v.is_empty() && *v != "unknown")
        .into_iter()
}

fn opt_eq(actual: &Option<String>, expected: &str) -> bool {
    actual
        .as_deref()
        .map(|a| a.eq_ignore_ascii_case(expected))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {

    /// What one facet rebuild costs — the work every keystroke and every
    /// checkbox toggle in the find-by-package modal triggers.
    ///
    ///     cargo test --release facet_cost -- --ignored --nocapture
    ///
    /// Ignored because it measures rather than asserts: the number moves with
    /// the machine, and a threshold picked here would fail on someone else's.
    #[test]
    #[ignore = "measurement, not an assertion"]
    fn facet_cost_on_a_real_catalogue() {
        // 47 discovered images of ~320 packages each — the numbers the modal
        // reported against a live CANFAR account.
        let tmp = TempStore::new();
        let store = tmp.store();
        for i in 0..47 {
            let mut m = manifest(
                &format!("images.canfar.net/proj{}/img:{i}", i % 7),
                if i % 2 == 0 { "ubuntu" } else { "centos" },
                if i % 3 == 0 { "22.04" } else { "20.04" },
                &[],
            );
            m.dpkg = (0..250).map(|p| format!("libthing{p}")).collect();
            m.python = (0..70).map(|p| format!("pypkg{p}")).collect();
            store.set_manifest(&m.image_id.clone(), m, "2026-08-20T00:00:00Z".into());
        }

        for (name, query) in [
            ("no filters", PackageQuery::default()),
            (
                "one os family",
                PackageQuery {
                    os_family: Some("ubuntu".into()),
                    ..Default::default()
                },
            ),
            (
                "a package",
                PackageQuery {
                    packages: vec!["libthing7".into()],
                    ..Default::default()
                },
            ),
        ] {
            let started = std::time::Instant::now();
            let facets = facets_for_query(&store, &query);
            // What the pane RENDERS is capped per category by the dialog; the
            // engine still reports the full universe, which is what makes the
            // cap necessary.
            const RENDER_CAP: usize = 25;
            let values: usize = facets.iter().map(|f| f.values.len()).sum();
            let rendered: usize = facets.iter().map(|f| f.values.len().min(RENDER_CAP)).sum();
            println!(
                "{name:>14} -> {} categories, {values} values in {:?} \
                 ({rendered} checkbox widgets after the cap)",
                facets.len(),
                started.elapsed()
            );
        }

        // And the part every rebuild pays before faceting even starts. This was
        // `known_images()` + `get()` per id, which deep-copied every manifest
        // twice; it is now one lock and a pointer each.
        let started = std::time::Instant::now();
        let count = store.with_manifests(|m| m.len());
        println!(
            "{:>14} -> {count} manifests in {:?}",
            "loading them",
            started.elapsed()
        );
    }
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

        let facets = facets_for_query(&store, &PackageQuery::default());

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
            py.values
                .iter()
                .map(|v| v.value.clone())
                .collect::<Vec<_>>(),
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
        m.python_by_env = BTreeMap::from([("ml".to_string(), vec!["torch".to_string()])]);
        m.capabilities = vec!["gpu".to_string()];
        store.set_manifest("a:1", m, AT.into());

        let facets = facets_for_query(&store, &PackageQuery::default());
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
