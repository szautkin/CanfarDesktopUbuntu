//! Per-image JSON cache of container-image discovery outcomes.
//!
//! Ported from `Services/ImageDiscovery/JsonManifestStore.cs`. One file per image
//! id lives at `<data_dir>/ImageManifests/<sanitized-id>.json` holding the full
//! [`LastOutcome`] (success manifest or typed failure), mirrored in memory behind
//! a `Mutex` for fast intersect-style queries. Writes are atomic (temp + rename);
//! reads hydrate lazily on first access. Per-image granularity lets concurrent
//! probes persist without contending on a single shared file.
//!
//! `<data_dir>` is `ProjectDirs::from("net","canfar","Verbinal").data_dir()`
//! (e.g. `~/.local/share/net.canfar/Verbinal`).

use crate::helpers::embedded_probe_scripts::sanitize_image_id;
use crate::models::image_manifest::{DiscoveryOutcome, ImageManifest, LastOutcome, PackageQuery};
use directories::ProjectDirs;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

/// One near-miss image: how much of the query it satisfies, and what it lacks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartialMatch {
    pub image_id: String,
    /// Count of satisfied constraint terms (the caller turns this into a 0..1
    /// fraction against the query's total).
    pub satisfied_terms: u32,
    /// Labels of the constraints this image does NOT satisfy, in query order.
    pub missing: Vec<String>,
}

/// What a list row needs to know about one image, without its package lists.
///
/// The lists are the expensive part of a manifest — a few hundred to a few
/// thousand `String`s — and no collapsed row displays them; it shows a status,
/// a count and a date. Keeping those separate is what lets a rebuild read every
/// image's state without copying every package name in the cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowSummary {
    /// A manifest was recorded (as opposed to a failure).
    pub discovered: bool,
    /// Total packages across every ecosystem, or 0 for a failure.
    pub package_count: usize,
    pub os_family: Option<String>,
    pub os_version: Option<String>,
    /// RFC-3339, when this outcome was recorded.
    pub discovered_at: String,
    /// Failure category + message; `None` for a success.
    pub failure: Option<(String, String)>,
}

impl RowSummary {
    /// Summarise one cached outcome. `pub(crate)` so the UI tests can build
    /// a summary from a `LastOutcome` and keep asserting on real records.
    pub(crate) fn of(outcome: &LastOutcome) -> Self {
        match &outcome.outcome {
            DiscoveryOutcome::Manifest(m) => RowSummary {
                discovered: true,
                package_count: crate::helpers::discovery_formatting::package_count(m),
                os_family: m.os_family.clone(),
                os_version: m.os_version.clone(),
                discovered_at: outcome.discovered_at.clone(),
                failure: None,
            },
            DiscoveryOutcome::Failure {
                category, message, ..
            } => RowSummary {
                discovered: false,
                package_count: 0,
                os_family: None,
                os_version: None,
                discovered_at: outcome.discovered_at.clone(),
                failure: Some((category.clone(), message.clone())),
            },
        }
    }
}

/// In-memory mirror of the on-disk cache, hydrated lazily.
#[derive(Default)]
struct Inner {
    loaded: HashMap<String, LastOutcome>,
    hydrated: bool,
}

/// JSON-backed, per-image discovery cache. Clone-free; share via `Arc`.
pub struct JsonManifestStore {
    directory: PathBuf,
    state: Mutex<Inner>,
}

impl JsonManifestStore {
    /// Open the store at the platform data directory
    /// (`<data_dir>/ImageManifests`). Nothing is read until the first query.
    pub fn new() -> Self {
        let directory = ProjectDirs::from("net", "canfar", "Verbinal")
            .map(|dirs| dirs.data_dir().join("ImageManifests"))
            .unwrap_or_else(|| PathBuf::from("ImageManifests"));
        Self::with_dir(directory)
    }

    /// Open the store at an explicit directory (used by tests and callers that
    /// need an isolated cache location).
    pub fn with_dir(directory: PathBuf) -> Self {
        JsonManifestStore {
            directory,
            state: Mutex::new(Inner::default()),
        }
    }

    /// Record a successful discovery: persist and mirror the manifest for
    /// `image_id`, timestamped `discovered_at` (RFC-3339, caller-supplied).
    pub fn set_manifest(&self, image_id: &str, m: ImageManifest, discovered_at: String) {
        let outcome = LastOutcome {
            image_id: image_id.to_string(),
            outcome: DiscoveryOutcome::Manifest(m),
            discovered_at,
        };
        self.put(image_id, outcome);
    }

    /// Record a failed discovery attempt for `image_id`.
    pub fn set_failure(
        &self,
        image_id: &str,
        category: &str,
        message: &str,
        job_id: Option<String>,
        at: String,
    ) {
        let outcome = LastOutcome {
            image_id: image_id.to_string(),
            outcome: DiscoveryOutcome::Failure {
                category: category.to_string(),
                message: message.to_string(),
                job_id,
            },
            discovered_at: at,
        };
        self.put(image_id, outcome);
    }

    /// The last recorded outcome for `image_id`, or `None` if never discovered.
    pub fn get(&self, image_id: &str) -> Option<LastOutcome> {
        let mut inner = self.lock();
        self.ensure_hydrated(&mut inner);
        inner.loaded.get(image_id).cloned()
    }

    /// Every cached image id (successes *and* failures), sorted.
    pub fn known_images(&self) -> Vec<String> {
        let mut inner = self.lock();
        self.ensure_hydrated(&mut inner);
        let mut ids: Vec<String> = inner.loaded.keys().cloned().collect();
        ids.sort();
        ids
    }

    /// Run `f` over every cached successful manifest, borrowed.
    ///
    /// The facet pane needs all of them at once, and the only way to get them
    /// used to be `known_images()` + `get()` per id — which takes the mutex once
    /// per image and DEEP-COPIES each outcome, manifest and all, only to read
    /// it and drop it. On this developer's cache (65 outcomes, the largest
    /// carrying 1,275 packages) that was 11.3 ms per call, and the dialog calls
    /// it on every keystroke. Handing out references costs one lock and a
    /// pointer per manifest.
    ///
    /// The closure runs with the store locked, so it must not call back into
    /// the store.
    pub fn with_manifests<R>(&self, f: impl FnOnce(&[&ImageManifest]) -> R) -> R {
        let mut inner = self.lock();
        self.ensure_hydrated(&mut inner);
        let manifests: Vec<&ImageManifest> = inner
            .loaded
            .values()
            .filter_map(|o| match &o.outcome {
                DiscoveryOutcome::Manifest(m) => Some(m),
                DiscoveryOutcome::Failure { .. } => None,
            })
            .collect();
        f(&manifests)
    }

    /// Everything the two image lists show per row, in one locked pass.
    ///
    /// A row needs a status, a package count and a date — none of which require
    /// the package LISTS. Both surfaces were calling `get()` three times per
    /// image per rebuild (filter, row build, subtitle), deep-copying a full
    /// manifest each time to read a boolean off it.
    pub fn row_summaries(&self) -> HashMap<String, RowSummary> {
        let mut inner = self.lock();
        self.ensure_hydrated(&mut inner);
        inner
            .loaded
            .iter()
            .map(|(id, o)| (id.clone(), RowSummary::of(o)))
            .collect()
    }

    /// Distinct package names across every cached *successful* manifest, sorted.
    /// Feeds the discovery facet pane.
    pub fn all_packages(&self) -> Vec<String> {
        let mut inner = self.lock();
        self.ensure_hydrated(&mut inner);
        let mut names: Vec<String> = Vec::new();
        for outcome in inner.loaded.values() {
            if let DiscoveryOutcome::Manifest(m) = &outcome.outcome {
                names.extend(m.all_package_names());
            }
        }
        names.sort();
        names.dedup();
        names
    }

    /// Package names containing `needle` (case-insensitive), with how many
    /// images carry each, most-common first.
    ///
    /// The vocabulary itself, which nothing could ask for before. Searching for
    /// images by package assumes you know the package's name — and the failure
    /// when you do not is silent and wrong: `spectroscopy` matches nothing, so
    /// the honest-looking answer is "no image does that", while nine images
    /// carry `specutils`. This is what turns a subject into the names to search
    /// for, the same way `describe_tap_schema` turns a table into its columns.
    ///
    /// The count is the useful half. `spec` matches 60-odd names here, and it
    /// is the ones present in 30 images rather than 1 that say what the
    /// platform actually supports.
    pub fn packages_matching(&self, needle: &str, limit: usize) -> Vec<(String, usize)> {
        let needle = needle.trim().to_lowercase();
        let mut inner = self.lock();
        self.ensure_hydrated(&mut inner);

        let mut counts: HashMap<String, usize> = HashMap::new();
        for outcome in inner.loaded.values() {
            if let DiscoveryOutcome::Manifest(m) = &outcome.outcome {
                // Per image, not per occurrence: a package listed in three
                // conda envs is still one image that has it.
                let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
                for name in m.all_package_names() {
                    if !needle.is_empty() && !name.to_lowercase().contains(&needle) {
                        continue;
                    }
                    if seen.insert(name.clone()) {
                        *counts.entry(name).or_insert(0) += 1;
                    }
                }
            }
        }

        let mut out: Vec<(String, usize)> = counts.into_iter().collect();
        // Names that START with the term first, then commonest, then
        // alphabetical for a stable order between ties.
        //
        // Count alone is the wrong lead. Searching "spec" over this cache puts
        // `jsonschema-specifications` (71 images), `fsspec` (56) and `pathspec`
        // (28) above `specutils` (9) and `spectral-cube` (8) — the packages in
        // most images are Python plumbing every image happens to carry, which
        // is exactly what does NOT distinguish one image from another. What the
        // caller is looking for is nearly always the word itself, at the front
        // of the name.
        out.sort_by(|a, b| {
            let a_leads = crate::helpers::discovery_formatting::leads_with(&a.0, &needle);
            let b_leads = crate::helpers::discovery_formatting::leads_with(&b.0, &needle);
            b_leads
                .cmp(&a_leads)
                .then_with(|| b.1.cmp(&a.1))
                .then_with(|| a.0.cmp(&b.0))
        });
        out.truncate(limit);
        out
    }

    /// Image ids whose manifest satisfies **all** of `q`, ranked by score
    /// (descending), ties broken by image id (ascending). Failures never match.
    /// An empty query returns every successful image id.
    pub fn search(&self, q: &PackageQuery) -> Vec<String> {
        let mut inner = self.lock();
        self.ensure_hydrated(&mut inner);
        let mut hits: Vec<(String, u32)> = Vec::new();
        for (id, outcome) in inner.loaded.iter() {
            if let DiscoveryOutcome::Manifest(m) = &outcome.outcome {
                if q.matches(m) {
                    hits.push((id.clone(), q.score(m)));
                }
            }
        }
        sort_ranked(&mut hits);
        hits.into_iter().map(|(id, _)| id).collect()
    }

    /// Ranked partial matches for every successful manifest that satisfies at
    /// least one term of `q`, best score first (ties by image id). An empty
    /// query returns nothing (there is no partial coverage to rank).
    pub fn search_partial(&self, q: &PackageQuery) -> Vec<PartialMatch> {
        let mut inner = self.lock();
        self.ensure_hydrated(&mut inner);
        if q.is_empty() {
            return Vec::new();
        }
        let mut results: Vec<PartialMatch> = Vec::new();
        for (id, outcome) in inner.loaded.iter() {
            if let DiscoveryOutcome::Manifest(m) = &outcome.outcome {
                let satisfied_terms = q.score(m);
                if satisfied_terms > 0 {
                    results.push(PartialMatch {
                        image_id: id.clone(),
                        satisfied_terms,
                        // Computed here, where the manifest is still in hand.
                        // The caller only sees ids, so it could not work out
                        // WHICH constraints failed on its own.
                        missing: q.unmet(m),
                    });
                }
            }
        }
        results.sort_by(|a, b| {
            b.satisfied_terms
                .cmp(&a.satisfied_terms)
                .then_with(|| a.image_id.cmp(&b.image_id))
        });
        results
    }

    #[cfg(test)]
    /// Forget `image_id`: drop the in-memory entry and delete its cache file.
    pub fn invalidate(&self, image_id: &str) {
        let mut inner = self.lock();
        self.ensure_hydrated(&mut inner);
        inner.loaded.remove(image_id);
        let path = self.file_path(image_id);
        if path.exists() {
            let _ = std::fs::remove_file(&path);
        }
    }

    #[cfg(test)]
    /// Forget everything: empty the mirror and delete every cache file.
    pub fn clear(&self) {
        let mut inner = self.lock();
        self.ensure_hydrated(&mut inner);
        inner.loaded.clear();
        if let Ok(entries) = std::fs::read_dir(&self.directory) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("json") {
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
    }

    #[cfg(test)]
    /// Number of cached outcomes (successes and failures).
    pub fn count(&self) -> usize {
        let mut inner = self.lock();
        self.ensure_hydrated(&mut inner);
        inner.loaded.len()
    }

    // ---------------------------------------------------------------------
    // Internals
    // ---------------------------------------------------------------------

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        // A poisoned lock still yields usable state; the cache is best-effort.
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Insert-or-update: persist to disk, then mirror in memory.
    fn put(&self, image_id: &str, outcome: LastOutcome) {
        let mut inner = self.lock();
        self.ensure_hydrated(&mut inner);
        self.persist(image_id, &outcome);
        inner.loaded.insert(image_id.to_string(), outcome);
    }

    /// Load every `*.json` cache file into the mirror on first access. Never
    /// throws: unreadable or malformed files are skipped.
    fn ensure_hydrated(&self, inner: &mut Inner) {
        if inner.hydrated {
            return;
        }
        inner.hydrated = true;

        let entries = match std::fs::read_dir(&self.directory) {
            Ok(e) => e,
            Err(_) => return, // directory missing yet — nothing cached
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Ok(raw) = std::fs::read_to_string(&path) else {
                continue;
            };
            if let Ok(outcome) = serde_json::from_str::<LastOutcome>(&raw) {
                if !outcome.image_id.is_empty() {
                    inner.loaded.insert(outcome.image_id.clone(), outcome);
                }
            }
        }
    }

    /// Atomically write one outcome to `<dir>/<sanitized-id>.json`
    /// (temp sibling + rename). Best-effort: I/O errors are swallowed so a
    /// failed cache write never breaks discovery.
    fn persist(&self, image_id: &str, outcome: &LastOutcome) {
        if std::fs::create_dir_all(&self.directory).is_err() {
            return;
        }
        let json = match serde_json::to_string_pretty(outcome) {
            Ok(j) => j,
            Err(_) => return,
        };
        let path = self.file_path(image_id);
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, json.as_bytes()).is_err() {
            return;
        }
        if std::fs::rename(&tmp, &path).is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
    }

    /// Where `image_id`'s outcome lives on disk.
    ///
    /// The same naming rule the probe scripts use, so the local cache and the
    /// VOSpace copy the scripts publish are addressed identically. In-memory
    /// keys always use the real id, so a filename collision between two exotic
    /// ids is harmless.
    fn file_path(&self, image_id: &str) -> PathBuf {
        self.directory
            .join(format!("{}.json", sanitize_image_id(image_id)))
    }
}

impl Default for JsonManifestStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Rank `(id, score)` pairs: score descending, then id ascending.
fn sort_ranked(v: &mut [(String, u32)]) {
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// A unique temp directory per test, removed on drop.
    struct TempDir {
        path: PathBuf,
    }
    impl TempDir {
        fn new() -> Self {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "verbinal_md_test_{}_{}_{}",
                std::process::id(),
                nanos,
                n
            ));
            TempDir { path }
        }
        fn store(&self) -> JsonManifestStore {
            JsonManifestStore::with_dir(self.path.clone())
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    const AT: &str = "2026-07-07T00:00:00Z";

    fn manifest(id: &str, os_family: &str, python: &[&str]) -> ImageManifest {
        ImageManifest {
            image_id: id.to_string(),
            os_family: Some(os_family.to_string()),
            os_version: Some("22.04".to_string()),
            python: python.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    fn py_query(names: &[&str]) -> PackageQuery {
        PackageQuery {
            packages: names.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn the_vocabulary_can_be_searched_by_a_fragment() {
        // The whole point: an agent asked about "spectra" does not know that
        // the package is called `specutils`. Searching for a subject word finds
        // nothing and reads as a definitive no.
        let tmp = TempDir::new();
        let store = tmp.store();
        store.set_manifest(
            "img:1",
            manifest("img:1", "ubuntu", &["specutils", "astropy"]),
            AT.into(),
        );
        store.set_manifest(
            "img:2",
            manifest("img:2", "ubuntu", &["specreduce", "numpy"]),
            AT.into(),
        );

        let hits: Vec<String> = store
            .packages_matching("spec", 10)
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        assert!(hits.contains(&"specutils".to_string()), "{hits:?}");
        assert!(hits.contains(&"specreduce".to_string()), "{hits:?}");
        assert!(!hits.contains(&"numpy".to_string()));
    }

    #[test]
    fn the_word_itself_beats_a_package_that_merely_contains_it() {
        // Measured against the real cache: searching "spec" ranks
        // `jsonschema-specifications` (71 images), `fsspec` (56) and `pathspec`
        // (28) above `specutils` (9). Those are Python plumbing that nearly
        // every image carries, so they are the names that distinguish nothing —
        // and they crowd out the one the caller meant.
        let tmp = TempDir::new();
        let store = tmp.store();
        for id in ["img:1", "img:2", "img:3"] {
            store.set_manifest(
                id,
                manifest(id, "ubuntu", &["fsspec", "pathspec"]),
                AT.into(),
            );
        }
        store.set_manifest(
            "img:4",
            manifest("img:4", "ubuntu", &["specutils", "fsspec"]),
            AT.into(),
        );

        let ranked = store.packages_matching("spec", 10);
        assert_eq!(
            ranked[0].0, "specutils",
            "a name merely containing the term outranked the term itself: {ranked:?}"
        );
    }

    #[test]
    fn the_commonest_package_leads() {
        // A name in most images is what the platform supports; one in a single
        // image is somebody's pin. An agent choosing an image needs that
        // ordering to pick a sensible default.
        let tmp = TempDir::new();
        let store = tmp.store();
        for id in ["img:1", "img:2", "img:3"] {
            store.set_manifest(id, manifest(id, "ubuntu", &["astropy"]), AT.into());
        }
        store.set_manifest(
            "img:4",
            manifest("img:4", "ubuntu", &["astroquery", "astropy"]),
            AT.into(),
        );

        let ranked = store.packages_matching("astro", 10);
        assert_eq!(ranked[0], ("astropy".to_string(), 4));
        assert_eq!(ranked[1], ("astroquery".to_string(), 1));
    }

    #[test]
    fn a_package_counts_once_per_image_however_often_it_is_listed() {
        // The same name appears in the flat python list and again in each conda
        // env. Counting occurrences would rank an image's private env above a
        // package the whole platform ships.
        let tmp = TempDir::new();
        let store = tmp.store();
        let mut m = manifest("img:1", "ubuntu", &["astropy"]);
        m.python_by_env = BTreeMap::from([
            ("base".to_string(), vec!["astropy".to_string()]),
            ("dev".to_string(), vec!["astropy".to_string()]),
        ]);
        store.set_manifest("img:1", m, AT.into());

        assert_eq!(store.packages_matching("astropy", 10)[0].1, 1);
    }

    #[test]
    fn an_empty_term_returns_the_commonest_packages() {
        // "What does this platform generally have" is a reasonable opening
        // question, and it must not be an error or an empty list.
        let tmp = TempDir::new();
        let store = tmp.store();
        store.set_manifest("img:1", manifest("img:1", "ubuntu", &["numpy"]), AT.into());
        assert_eq!(store.packages_matching("", 10).len(), 1);
    }

    #[test]
    fn the_vocabulary_search_is_case_insensitive() {
        let tmp = TempDir::new();
        let store = tmp.store();
        store.set_manifest("img:1", manifest("img:1", "ubuntu", &["AstroPy"]), AT.into());
        assert_eq!(store.packages_matching("astropy", 10).len(), 1);
    }

    #[test]
    fn a_failed_image_contributes_no_vocabulary() {
        // It has no manifest, so it has no packages — and counting it would
        // inflate a name's image count above the number that actually have it.
        let tmp = TempDir::new();
        let store = tmp.store();
        store.set_manifest("img:1", manifest("img:1", "ubuntu", &["numpy"]), AT.into());
        store.set_failure("img:2", "timeout", "took too long", None, AT.into());
        assert_eq!(store.packages_matching("numpy", 10)[0].1, 1);
    }

    #[test]
    fn set_manifest_then_get_roundtrips() {
        let tmp = TempDir::new();
        let store = tmp.store();
        assert!(store.get("img:1").is_none());

        store.set_manifest(
            "img:1",
            manifest("img:1", "ubuntu", &["astropy"]),
            AT.into(),
        );

        let outcome = store.get("img:1").expect("outcome should exist");
        assert!(outcome.is_success());
        assert_eq!(outcome.image_id, "img:1");
        assert_eq!(outcome.discovered_at, AT);
        let m = outcome.manifest().expect("manifest");
        assert_eq!(m.os_family.as_deref(), Some("ubuntu"));
        assert!(m.python.contains(&"astropy".to_string()));
        assert_eq!(store.count(), 1);
    }

    #[test]
    fn set_failure_then_get_is_failure() {
        let tmp = TempDir::new();
        let store = tmp.store();
        store.set_failure(
            "img:2",
            "JobTimedOut",
            "timed out",
            Some("job-9".into()),
            AT.into(),
        );

        let outcome = store.get("img:2").expect("outcome should exist");
        assert!(!outcome.is_success());
        match outcome.outcome {
            DiscoveryOutcome::Failure {
                category,
                message,
                job_id,
            } => {
                assert_eq!(category, "JobTimedOut");
                assert_eq!(message, "timed out");
                assert_eq!(job_id.as_deref(), Some("job-9"));
            }
            _ => panic!("expected failure"),
        }
    }

    #[test]
    fn persists_across_reload() {
        let tmp = TempDir::new();
        tmp.store().set_manifest(
            "keep:1",
            manifest("keep:1", "ubuntu", &["numpy"]),
            AT.into(),
        );

        // A fresh store over the same directory hydrates from disk.
        let reopened = tmp.store();
        let outcome = reopened
            .get("keep:1")
            .expect("outcome should survive reload");
        assert!(outcome.is_success());
        assert!(outcome
            .manifest()
            .unwrap()
            .python
            .contains(&"numpy".to_string()));
        assert_eq!(reopened.count(), 1);
    }

    #[test]
    fn invalidate_and_clear() {
        let tmp = TempDir::new();
        let store = tmp.store();
        store.set_manifest("a:1", manifest("a:1", "ubuntu", &[]), AT.into());
        store.set_manifest("b:1", manifest("b:1", "ubuntu", &[]), AT.into());

        store.invalidate("a:1");
        assert!(store.get("a:1").is_none());
        assert_eq!(store.count(), 1);
        // Deletion reached disk too: a reopened store does not see a:1.
        assert!(tmp.store().get("a:1").is_none());

        store.clear();
        assert_eq!(store.count(), 0);
        assert!(tmp.store().get("b:1").is_none());
    }

    #[test]
    fn search_intersection_and_ranking() {
        let tmp = TempDir::new();
        let store = tmp.store();
        store.set_manifest(
            "a:1",
            manifest("a:1", "ubuntu", &["astropy", "numpy"]),
            AT.into(),
        );
        store.set_manifest("b:1", manifest("b:1", "ubuntu", &["astropy"]), AT.into());
        store.set_failure("c:1", "Unknown", "x", None, AT.into());

        // Only a:1 has both astropy AND numpy.
        assert_eq!(
            store.search(&py_query(&["astropy", "numpy"])),
            vec!["a:1".to_string()]
        );

        // Partial: both a:1 and b:1 have astropy; a:1 outscores b:1.
        let partial = store.search_partial(&py_query(&["astropy", "numpy"]));
        assert_eq!(partial.len(), 2);
        assert_eq!(partial[0].image_id, "a:1");
        assert!(partial[0].satisfied_terms > partial[1].satisfied_terms);
        assert_eq!(partial[1].image_id, "b:1");

        // Failures appear in known_images but never in search results.
        assert_eq!(
            store.known_images(),
            vec!["a:1".to_string(), "b:1".to_string(), "c:1".to_string()]
        );
        assert_eq!(store.count(), 3);
    }

    #[test]
    fn empty_query_matches_all_successes_partial_matches_none() {
        let tmp = TempDir::new();
        let store = tmp.store();
        store.set_manifest("a:1", manifest("a:1", "ubuntu", &[]), AT.into());
        store.set_failure("c:1", "Unknown", "x", None, AT.into());

        let empty = PackageQuery::default();
        assert_eq!(store.search(&empty), vec!["a:1".to_string()]); // failures excluded
        assert!(store.search_partial(&empty).is_empty());
    }

    #[test]
    fn all_packages_unions_across_manifests() {
        let tmp = TempDir::new();
        let store = tmp.store();
        let mut alma = manifest("b:1", "almalinux", &["scipy"]);
        alma.rpm = vec!["gcc".to_string()];
        alma.python_by_env = BTreeMap::from([("ml".to_string(), vec!["torch".to_string()])]);
        store.set_manifest("a:1", manifest("a:1", "ubuntu", &["astropy"]), AT.into());
        store.set_manifest("b:1", alma, AT.into());

        let all = store.all_packages();
        for expected in ["astropy", "gcc", "scipy", "torch"] {
            assert!(all.contains(&expected.to_string()), "missing {expected}");
        }
        // Sorted and de-duplicated.
        let mut sorted = all.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(all, sorted);
    }

    #[test]
    fn the_cache_filename_is_the_one_the_scripts_publish_under() {
        // Not a second rule: `sanitize_image_id` is the scripts' rule, checked
        // against their own `tr` set. Asserted here because the local cache and
        // the VOSpace copy have to be addressed identically for one to stand in
        // for the other.
        let store = JsonManifestStore::with_dir(PathBuf::from("/cache"));
        assert_eq!(
            store.file_path("images.canfar.net/skaha/astroml:24.07"),
            PathBuf::from("/cache/images.canfar.net_skaha_astroml_24.07.json")
        );
    }
}
