use crate::models::search_result::{RecentSearch, SavedQuery};
use directories::ProjectDirs;
use std::collections::HashMap;
use std::path::PathBuf;

const MAX_RECENT: usize = 20;

pub struct SearchStoreService {
    data_dir: PathBuf,
}

impl SearchStoreService {
    pub fn new() -> Self {
        let data_dir = ProjectDirs::from("net", "canfar", "Verbinal")
            .map(|dirs| dirs.data_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
        SearchStoreService { data_dir }
    }

    /// Point the store at an arbitrary directory.
    ///
    /// Exists so tests never touch the real user data dir — a test that cleared
    /// the live store would delete the developer's own saved work.
    pub fn with_data_dir(data_dir: PathBuf) -> Self {
        SearchStoreService { data_dir }
    }

    // --- Recent Searches ---

    pub fn load_recent(&self) -> Vec<RecentSearch> {
        let path = self.data_dir.join("recent_searches.json");
        if path.exists() {
            match std::fs::read_to_string(&path) {
                Ok(json) => serde_json::from_str(&json).unwrap_or_default(),
                Err(_) => Vec::new(),
            }
        } else {
            Vec::new()
        }
    }

    pub fn save_recent(&self, entry: RecentSearch) -> Result<(), String> {
        let mut entries = self.load_recent();
        // Dedup by ADQL
        entries.retain(|e| e.adql != entry.adql);
        entries.insert(0, entry);
        entries.truncate(MAX_RECENT);
        self.write_recent(&entries)
    }

    /// Replace the whole recent-search list in one write.
    ///
    /// The alternative — clear, then re-save each entry — is O(n) file writes for
    /// one deletion, and `save_recent` dedups by ADQL and prepends, so replaying a
    /// list through it also REVERSES the order and silently merges two searches
    /// that happen to share ADQL. Anything editing the list wholesale (an agent
    /// removing one entry, the sidebar's delete button) must come through here.
    pub fn save_all_recent(&self, entries: &[RecentSearch]) -> Result<(), String> {
        let mut capped = entries.to_vec();
        capped.truncate(MAX_RECENT);
        self.write_recent(&capped)
    }

    pub fn clear_recent(&self) -> Result<(), String> {
        self.write_recent(&[])
    }

    fn write_recent(&self, entries: &[RecentSearch]) -> Result<(), String> {
        std::fs::create_dir_all(&self.data_dir).map_err(|e| e.to_string())?;
        let path = self.data_dir.join("recent_searches.json");
        let json = serde_json::to_string_pretty(entries).map_err(|e| e.to_string())?;
        std::fs::write(path, json).map_err(|e| e.to_string())
    }

    // --- Saved Queries ---

    pub fn load_saved(&self) -> Vec<SavedQuery> {
        let path = self.data_dir.join("saved_queries.json");
        if path.exists() {
            match std::fs::read_to_string(&path) {
                Ok(json) => serde_json::from_str(&json).unwrap_or_default(),
                Err(_) => Vec::new(),
            }
        } else {
            Vec::new()
        }
    }

    pub fn save_query(&self, query: SavedQuery) -> Result<(), String> {
        let mut queries = self.load_saved();
        // Upsert by name
        queries.retain(|q| q.name != query.name);
        queries.insert(0, query);
        self.write_saved(&queries)
    }

    pub fn delete_saved(&self, name: &str) -> Result<(), String> {
        let mut queries = self.load_saved();
        queries.retain(|q| q.name != name);
        self.write_saved(&queries)
    }

    fn write_saved(&self, queries: &[SavedQuery]) -> Result<(), String> {
        std::fs::create_dir_all(&self.data_dir).map_err(|e| e.to_string())?;
        let path = self.data_dir.join("saved_queries.json");
        let json = serde_json::to_string_pretty(queries).map_err(|e| e.to_string())?;
        std::fs::write(path, json).map_err(|e| e.to_string())
    }

    // --- Column display units ---
    //
    // Per-column display-unit choices for the search results grid (cleaned column
    // key → unit id, e.g. `"ra(j20000)" → "degrees"`). Persisted so choices survive
    // restarts. Mirrors the Windows `LocalSettingsColumnUnitStore` (`search.col.unit.*`
    // keys); here the whole map is stored as one JSON object.

    pub fn load_column_units(&self) -> HashMap<String, String> {
        let path = self.data_dir.join("column_units.json");
        if path.exists() {
            match std::fs::read_to_string(&path) {
                Ok(json) => serde_json::from_str(&json).unwrap_or_default(),
                Err(_) => HashMap::new(),
            }
        } else {
            HashMap::new()
        }
    }

    pub fn save_column_units(&self, units: &HashMap<String, String>) -> Result<(), String> {
        std::fs::create_dir_all(&self.data_dir).map_err(|e| e.to_string())?;
        let path = self.data_dir.join("column_units.json");
        let json = serde_json::to_string_pretty(units).map_err(|e| e.to_string())?;
        std::fs::write(path, json).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A store rooted in a unique temp dir, cleaned up on drop.
    struct TempStore {
        dir: PathBuf,
        svc: SearchStoreService,
    }

    impl TempStore {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "verbinal_search_store_{}_{}_{}",
                tag,
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            ));
            let svc = SearchStoreService::with_data_dir(dir.clone());
            TempStore { dir, svc }
        }
    }

    impl Drop for TempStore {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn recent(adql: &str, summary: &str) -> RecentSearch {
        RecentSearch {
            adql: adql.to_string(),
            summary: summary.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn save_all_recent_preserves_order_and_duplicate_adql() {
        // Replaying a list through `save_recent` reversed it (each entry is
        // prepended) and merged entries sharing ADQL (it dedups). A wholesale
        // write must do neither.
        let t = TempStore::new("order");
        let entries = vec![
            recent("SELECT 1", "first"),
            recent("SELECT 1", "second, same adql"),
            recent("SELECT 2", "third"),
        ];
        t.svc.save_all_recent(&entries).unwrap();

        let back = t.svc.load_recent();
        assert_eq!(back.len(), 3, "no entry may be dropped or merged");
        assert_eq!(back[0].summary, "first");
        assert_eq!(back[1].summary, "second, same adql");
        assert_eq!(back[2].summary, "third");
    }

    #[test]
    fn save_all_recent_enforces_the_cap() {
        let t = TempStore::new("cap");
        let entries: Vec<RecentSearch> = (0..MAX_RECENT + 5)
            .map(|i| recent(&format!("SELECT {i}"), &format!("q{i}")))
            .collect();
        t.svc.save_all_recent(&entries).unwrap();
        assert_eq!(t.svc.load_recent().len(), MAX_RECENT);
    }

    #[test]
    fn removing_one_entry_leaves_the_rest_in_order() {
        let t = TempStore::new("remove");
        t.svc
            .save_all_recent(&[
                recent("SELECT 1", "a"),
                recent("SELECT 2", "b"),
                recent("SELECT 3", "c"),
            ])
            .unwrap();

        let mut all = t.svc.load_recent();
        all.retain(|r| r.summary != "b");
        t.svc.save_all_recent(&all).unwrap();

        let back = t.svc.load_recent();
        let summaries: Vec<&str> = back.iter().map(|r| r.summary.as_str()).collect();
        assert_eq!(summaries, vec!["a", "c"]);
    }
}
