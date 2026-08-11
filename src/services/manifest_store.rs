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

/// Convert an image id such as `images.canfar.net/skaha/astroml:24.07` into a
/// filesystem-safe stub for the cache filename. Filesystem-hostile characters
/// (path separators, drive/registry punctuation, wildcard/quote glyphs and
/// whitespace) collapse to `_`. In-memory keys always use the real id, so a
/// filename collision between two exotic ids is harmless.
fn sanitize_image_id(image_id: &str) -> String {
    image_id
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '@' | '?' | '*' | '<' | '>' | '|' | '"' => '_',
            c if c.is_whitespace() || c.is_control() => '_',
            c => c,
        })
        .collect()
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
    fn sanitize_image_id_replaces_hostile_chars() {
        assert_eq!(
            sanitize_image_id("images.canfar.net/skaha/astroml:24.07"),
            "images.canfar.net_skaha_astroml_24.07"
        );
        assert!(!sanitize_image_id("a@b:c/d").contains(['/', ':', '@']));
    }
}
