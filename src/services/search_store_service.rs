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
