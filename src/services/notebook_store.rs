use chrono::{DateTime, Utc};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Maximum number of entries kept in the recent-notebooks list.
const MAX_RECENT: usize = 15;

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// A single entry in the recent-notebooks list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecentNotebook {
    /// Absolute file-system path to the `.ipynb` (or `.py` / `.md`) file.
    pub path: String,
    /// Human-readable display name (usually the filename without its path).
    pub name: String,
    /// When the file was last opened in Verbinal.
    pub opened_at: DateTime<Utc>,
}

impl RecentNotebook {
    /// Create a new entry stamped with the current UTC time.
    pub fn new(path: impl Into<String>, name: impl Into<String>) -> Self {
        RecentNotebook {
            path: path.into(),
            name: name.into(),
            opened_at: Utc::now(),
        }
    }
}

// ---------------------------------------------------------------------------
// Service
// ---------------------------------------------------------------------------

/// Persistent store for the 15 most recently opened notebook files.
///
/// Data is stored as JSON at
/// `~/.local/share/net.canfar/Verbinal/recent_notebooks.json`.
/// The same `directories::ProjectDirs` convention is used throughout Verbinal.
pub struct NotebookStore {
    data_path: PathBuf,
}

impl NotebookStore {
    /// Create a new `NotebookStore` pointing at the canonical data path.
    pub fn new() -> Self {
        let data_path = ProjectDirs::from("net", "canfar", "Verbinal")
            .map(|dirs| dirs.data_dir().join("recent_notebooks.json"))
            .unwrap_or_else(|| PathBuf::from("recent_notebooks.json"));
        NotebookStore { data_path }
    }

    /// Load the list of recent notebooks from disk.
    ///
    /// Returns an empty `Vec` if the file does not exist or cannot be parsed.
    pub fn load(&self) -> Vec<RecentNotebook> {
        if !self.data_path.exists() {
            return Vec::new();
        }
        match std::fs::read_to_string(&self.data_path) {
            Ok(json) => serde_json::from_str(&json).unwrap_or_default(),
            Err(_) => Vec::new(),
        }
    }

    /// Record that `path` / `name` was opened, moving it to the top of the
    /// list.  Duplicate paths are removed before inserting.  The list is
    /// trimmed to [`MAX_RECENT`] entries after each update.
    pub fn add(&self, path: &str, name: &str) -> Result<(), String> {
        let mut entries = self.load();

        // Dedup by path so the same file only appears once.
        entries.retain(|e| e.path != path);

        entries.insert(0, RecentNotebook::new(path, name));
        entries.truncate(MAX_RECENT);

        self.write(&entries)
    }

    /// Remove the entry at `index` from the list (0-based).
    ///
    /// Returns `Err` if the index is out of range.
    pub fn remove(&self, index: usize) -> Result<(), String> {
        let mut entries = self.load();
        if index >= entries.len() {
            return Err(format!(
                "index {} out of range (list has {} entries)",
                index,
                entries.len()
            ));
        }
        entries.remove(index);
        self.write(&entries)
    }

    /// Clear the entire list.
    pub fn clear(&self) -> Result<(), String> {
        self.write(&[])
    }

    /// Return the number of recent-notebook entries currently stored.
    pub fn len(&self) -> usize {
        self.load().len()
    }

    /// Return `true` if there are no recent-notebook entries.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    // ---------------------------------------------------------------------------
    // Private helpers
    // ---------------------------------------------------------------------------

    fn write(&self, entries: &[RecentNotebook]) -> Result<(), String> {
        if let Some(parent) = self.data_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let json = serde_json::to_string_pretty(entries).map_err(|e| e.to_string())?;
        // Atomic write: write to a .tmp sibling then rename to avoid data
        // corruption on crash or NFS partial writes.
        let tmp = self.data_path.with_extension("json.tmp");
        std::fs::write(&tmp, &json).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &self.data_path).map_err(|e| e.to_string())
    }
}

impl Default for NotebookStore {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Counter to generate unique temp-dir names within a test run.
    static COUNTER: AtomicU32 = AtomicU32::new(0);

    /// Build a `NotebookStore` backed by a unique subdirectory of `$TMPDIR`.
    /// The directory is created automatically by the first `add`/`write` call.
    fn make_store() -> (NotebookStore, PathBuf) {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("verbinal_nbstore_test_{}", n));
        let store = NotebookStore {
            data_path: dir.join("recent_notebooks.json"),
        };
        (store, dir)
    }

    #[test]
    fn load_empty_when_no_file() {
        let (store, _dir) = make_store();
        assert!(store.load().is_empty());
    }

    #[test]
    fn add_single_entry() {
        let (store, dir) = make_store();
        store.add("/home/user/nb.ipynb", "nb.ipynb").expect("add");
        let entries = store.load();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, "/home/user/nb.ipynb");
        assert_eq!(entries[0].name, "nb.ipynb");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn add_deduplicates_by_path() {
        let (store, dir) = make_store();
        store.add("/home/user/nb.ipynb", "nb.ipynb").expect("add");
        store.add("/home/user/nb.ipynb", "nb.ipynb").expect("add");
        assert_eq!(store.load().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn add_moves_existing_to_top() {
        let (store, dir) = make_store();
        store
            .add("/home/user/first.ipynb", "first.ipynb")
            .expect("add");
        store
            .add("/home/user/second.ipynb", "second.ipynb")
            .expect("add");
        store
            .add("/home/user/first.ipynb", "first.ipynb")
            .expect("add");
        let entries = store.load();
        assert_eq!(entries[0].path, "/home/user/first.ipynb");
        assert_eq!(entries[1].path, "/home/user/second.ipynb");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn add_caps_at_max_recent() {
        let (store, dir) = make_store();
        for i in 0..(MAX_RECENT + 5) {
            store
                .add(
                    &format!("/home/user/nb{}.ipynb", i),
                    &format!("nb{}.ipynb", i),
                )
                .expect("add");
        }
        assert_eq!(store.load().len(), MAX_RECENT);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn remove_by_index() {
        let (store, dir) = make_store();
        store.add("/home/user/a.ipynb", "a.ipynb").expect("add");
        store.add("/home/user/b.ipynb", "b.ipynb").expect("add");
        // b is at index 0 (most recent first).
        store.remove(0).expect("remove");
        let entries = store.load();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "a.ipynb");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn remove_out_of_range_returns_err() {
        let (store, dir) = make_store();
        store.add("/nb.ipynb", "nb.ipynb").expect("add");
        assert!(store.remove(5).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn clear_empties_the_list() {
        let (store, dir) = make_store();
        store.add("/a.ipynb", "a.ipynb").expect("add");
        store.add("/b.ipynb", "b.ipynb").expect("add");
        store.clear().expect("clear");
        assert!(store.load().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn len_and_is_empty() {
        let (store, dir) = make_store();
        assert!(store.is_empty());
        store.add("/nb.ipynb", "nb.ipynb").expect("add");
        assert_eq!(store.len(), 1);
        assert!(!store.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_corrupt_json_returns_empty() {
        let (store, dir) = make_store();
        // Manually create the data file with corrupt content.
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(&store.data_path, "not json at all").expect("write");
        assert!(store.load().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn persists_across_instances() {
        let (store, dir) = make_store();
        store.add("/nb.ipynb", "nb.ipynb").expect("add");
        // Drop `store`, create a second instance at the same path.
        let store2 = NotebookStore {
            data_path: dir.join("recent_notebooks.json"),
        };
        assert_eq!(store2.load().len(), 1);
        assert_eq!(store2.load()[0].name, "nb.ipynb");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn recent_notebook_new_sets_opened_at() {
        let before = Utc::now();
        let entry = RecentNotebook::new("/nb.ipynb", "nb.ipynb");
        let after = Utc::now();
        assert!(entry.opened_at >= before);
        assert!(entry.opened_at <= after);
    }

    #[test]
    fn round_trip_serialisation() {
        let entry = RecentNotebook::new("/some/path/notebook.ipynb", "notebook.ipynb");
        let json = serde_json::to_string(&entry).expect("serialise");
        let back: RecentNotebook = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(back.path, entry.path);
        assert_eq!(back.name, entry.name);
    }
}
