//! JSON-backed store for per-observation research notes/ratings/tags, keyed by
//! publisher ID.
//!
//! Ported from `Services/Database/ObservationNoteStore.cs`.  The Windows
//! reference is SQLite + FTS5; the Linux port keeps a small JSON map at
//! `~/.local/share/net.canfar/Verbinal/observation_notes.json` and does a
//! plain case-insensitive substring scan over note text + tags as the
//! full-text-search substitute.  Writes are atomic (tmp + rename) and mirror
//! `services::observation_store`.
//!
//! Saving an *empty* note (blank text, unrated, no tags) removes the entry —
//! matching the reference `Upsert` delete-on-empty behavior so the file never
//! accumulates blank rows.

use crate::models::observation_note::ObservationNote;
use directories::ProjectDirs;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Persistent JSON-backed store mapping `publisher_id -> ObservationNote`.
pub struct ObservationNoteStore {
    data_path: PathBuf,
}

impl ObservationNoteStore {
    pub fn new() -> Self {
        let data_path = ProjectDirs::from("net", "canfar", "Verbinal")
            .map(|dirs| dirs.data_dir().join("observation_notes.json"))
            .unwrap_or_else(|| PathBuf::from("observation_notes.json"));
        ObservationNoteStore { data_path }
    }

    /// The note for `publisher_id`, or `None` if there isn't one.
    pub fn get(&self, publisher_id: &str) -> Option<ObservationNote> {
        self.load_map().remove(publisher_id)
    }

    /// Insert or update a note.  An empty note (see [`ObservationNote::is_empty`])
    /// removes the entry instead.  Blocking disk I/O — the note file is tiny so
    /// callers invoke this directly from the UI thread on a debounce.
    pub fn save(&self, note: ObservationNote) -> Result<(), String> {
        let mut map = self.load_map();
        if note.is_empty() {
            map.remove(&note.publisher_id);
        } else {
            map.insert(note.publisher_id.clone(), note);
        }
        self.write_map(&map)
    }

    /// All stored notes (order unspecified).
    pub fn all(&self) -> Vec<ObservationNote> {
        self.load_map().into_values().collect()
    }

    /// Publisher IDs whose note text OR any tag contains `query`
    /// (case-insensitive substring).  An empty/whitespace query returns an
    /// empty list (matches the reference, which returns nothing for a blank
    /// FTS query rather than everything).
    pub fn search(&self, query: &str) -> Vec<String> {
        let needle = query.trim().to_lowercase();
        if needle.is_empty() {
            return Vec::new();
        }
        self.load_map()
            .into_iter()
            .filter(|(_, n)| {
                n.note.to_lowercase().contains(&needle)
                    || n.tags.iter().any(|t| t.to_lowercase().contains(&needle))
            })
            .map(|(id, _)| id)
            .collect()
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    fn load_map(&self) -> BTreeMap<String, ObservationNote> {
        if !self.data_path.exists() {
            return BTreeMap::new();
        }
        match std::fs::read_to_string(&self.data_path) {
            Ok(raw) => serde_json::from_str(&raw).unwrap_or_default(),
            Err(_) => BTreeMap::new(),
        }
    }

    fn write_map(&self, map: &BTreeMap<String, ObservationNote>) -> Result<(), String> {
        if let Some(parent) = self.data_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let json = serde_json::to_string_pretty(map).map_err(|e| e.to_string())?;
        // Atomic write: write to a .tmp sibling then rename to avoid data
        // corruption on crash or partial write.
        let tmp = self.data_path.with_extension("json.tmp");
        std::fs::write(&tmp, &json).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &self.data_path).map_err(|e| e.to_string())
    }
}

impl Default for ObservationNoteStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    // Test-only constructor pointing the store at an arbitrary path so tests
    // never touch the user's real notes file.
    impl ObservationNoteStore {
        fn with_path(path: PathBuf) -> Self {
            ObservationNoteStore { data_path: path }
        }
    }

    /// A unique temp path per test, cleaned up on drop.
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
                "verbinal_notes_test_{}_{}_{}.json",
                std::process::id(),
                nanos,
                n
            ));
            TempStore { path }
        }

        fn store(&self) -> ObservationNoteStore {
            ObservationNoteStore::with_path(self.path.clone())
        }
    }

    impl Drop for TempStore {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
            let _ = std::fs::remove_file(self.path.with_extension("json.tmp"));
        }
    }

    fn note(pub_id: &str, rating: u8, text: &str, tags: &[&str]) -> ObservationNote {
        ObservationNote {
            publisher_id: pub_id.to_string(),
            rating,
            note: text.to_string(),
            tags: tags.iter().map(|s| s.to_string()).collect(),
            updated: "2024-01-01T00:00:00Z".to_string(),
            agent_attribution: None,
        }
    }

    #[test]
    fn save_then_get_roundtrips() {
        let tmp = TempStore::new();
        let store = tmp.store();
        assert!(store.get("ivo://cadc/CFHT?1").is_none());

        store
            .save(note(
                "ivo://cadc/CFHT?1",
                4,
                "Nice galaxy",
                &["galaxy", "deep"],
            ))
            .unwrap();

        let got = store.get("ivo://cadc/CFHT?1").expect("note should exist");
        assert_eq!(got.rating, 4);
        assert_eq!(got.note, "Nice galaxy");
        assert_eq!(got.tags, vec!["galaxy".to_string(), "deep".to_string()]);
    }

    #[test]
    fn save_replaces_existing_by_publisher_id() {
        let tmp = TempStore::new();
        let store = tmp.store();
        store.save(note("id1", 1, "first", &["a"])).unwrap();
        store.save(note("id1", 5, "second", &["b"])).unwrap();

        let got = store.get("id1").unwrap();
        assert_eq!(got.rating, 5);
        assert_eq!(got.note, "second");
        assert_eq!(got.tags, vec!["b".to_string()]);
        // Still a single entry.
        assert_eq!(store.all().len(), 1);
    }

    #[test]
    fn saving_empty_note_removes_entry() {
        let tmp = TempStore::new();
        let store = tmp.store();
        store.save(note("id1", 3, "keep", &["x"])).unwrap();
        assert!(store.get("id1").is_some());

        // Blank text, unrated, no tags => delete.
        store.save(note("id1", 0, "   ", &[])).unwrap();
        assert!(store.get("id1").is_none());
        assert!(store.all().is_empty());
    }

    #[test]
    fn search_matches_note_text_and_tags_case_insensitively() {
        let tmp = TempStore::new();
        let store = tmp.store();
        store
            .save(note("id1", 2, "Spiral arms visible", &["morphology"]))
            .unwrap();
        store
            .save(note("id2", 0, "faint blob", &["Transient", "followup"]))
            .unwrap();

        // Note-text hit (case-insensitive).
        let hits = store.search("SPIRAL");
        assert_eq!(hits, vec!["id1".to_string()]);

        // Tag hit (case-insensitive).
        let hits = store.search("transient");
        assert_eq!(hits, vec!["id2".to_string()]);

        // No match.
        assert!(store.search("supernova").is_empty());

        // Blank query returns nothing (not everything).
        assert!(store.search("   ").is_empty());
    }

    #[test]
    fn all_returns_every_stored_note() {
        let tmp = TempStore::new();
        let store = tmp.store();
        store.save(note("id1", 1, "one", &[])).unwrap();
        store.save(note("id2", 2, "two", &[])).unwrap();
        store.save(note("id3", 3, "three", &[])).unwrap();

        let mut ids: Vec<String> = store.all().into_iter().map(|n| n.publisher_id).collect();
        ids.sort();
        assert_eq!(ids, vec!["id1", "id2", "id3"]);
    }
}
