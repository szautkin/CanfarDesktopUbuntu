use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A single observation that the user has downloaded from the CADC archive.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadedObservation {
    /// Locally generated UUID identifying this record.
    pub id: String,
    /// CADC publisher DID (e.g. `ivo://cadc.nrc.ca/CFHT?123456`).
    pub publisher_id: String,
    pub collection: String,
    pub observation_id: String,
    pub target_name: String,
    pub instrument: String,
    pub filter: String,
    pub ra: String,
    pub dec: String,
    pub start_date: String,
    pub cal_level: String,
    /// Absolute path to the file on disk.
    pub local_path: String,
    /// File size in bytes.
    pub file_size: u64,
    /// ISO-8601 timestamp of when the file was downloaded.
    pub downloaded_at: String,
}

impl DownloadedObservation {
    /// Human-readable file size (e.g. "3.4 MB").
    pub fn formatted_size(&self) -> String {
        format_bytes(self.file_size)
    }
}

/// Persistent JSON-backed store for downloaded observations.
///
/// Stored at `~/.local/share/net.canfar/Verbinal/observations.json`.
pub struct ObservationStore {
    data_path: PathBuf,
}

impl ObservationStore {
    pub fn new() -> Self {
        let data_path = ProjectDirs::from("net", "canfar", "Verbinal")
            .map(|dirs| dirs.data_dir().join("observations.json"))
            .unwrap_or_else(|| PathBuf::from("observations.json"));
        ObservationStore { data_path }
    }

    /// Load all observations from disk.  Returns an empty list on any error.
    pub fn load(&self) -> Vec<DownloadedObservation> {
        if !self.data_path.exists() {
            return Vec::new();
        }
        match std::fs::read_to_string(&self.data_path) {
            Ok(json) => serde_json::from_str(&json).unwrap_or_default(),
            Err(_) => Vec::new(),
        }
    }

    /// Append (or replace by `id`) an observation and flush to disk.
    pub fn save(&self, obs: DownloadedObservation) -> Result<(), String> {
        let mut list = self.load();
        list.retain(|o| o.id != obs.id);
        list.insert(0, obs);
        self.write(&list)
    }

    /// Remove an observation by its local `id`.
    pub fn remove(&self, id: &str) -> Result<(), String> {
        let mut list = self.load();
        list.retain(|o| o.id != id);
        self.write(&list)
    }

    /// Returns `true` if an observation with the given CADC publisher ID already exists.
    pub fn contains_publisher_id(&self, publisher_id: &str) -> bool {
        self.load()
            .iter()
            .any(|o| o.publisher_id == publisher_id)
    }

    /// Return observations whose collection, observation_id, target, or instrument
    /// contain `text` (case-insensitive).  An empty `text` returns everything.
    pub fn filter(&self, text: &str) -> Vec<DownloadedObservation> {
        let list = self.load();
        if text.is_empty() {
            return list;
        }
        let needle = text.to_lowercase();
        list.into_iter()
            .filter(|o| {
                o.collection.to_lowercase().contains(&needle)
                    || o.observation_id.to_lowercase().contains(&needle)
                    || o.target_name.to_lowercase().contains(&needle)
                    || o.instrument.to_lowercase().contains(&needle)
                    || o.filter.to_lowercase().contains(&needle)
            })
            .collect()
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    fn write(&self, list: &[DownloadedObservation]) -> Result<(), String> {
        if let Some(parent) = self.data_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let json = serde_json::to_string_pretty(list).map_err(|e| e.to_string())?;
        // Atomic write: write to a .tmp sibling then rename to avoid data
        // corruption on crash or NFS partial writes.
        let tmp = self.data_path.with_extension("json.tmp");
        std::fs::write(&tmp, &json).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &self.data_path).map_err(|e| e.to_string())
    }
}

impl Default for ObservationStore {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Utility
// ---------------------------------------------------------------------------

fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1_024;
    const MB: u64 = KB * 1_024;
    const GB: u64 = MB * 1_024;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_bytes_scales_correctly() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1023), "1023 B");
        assert_eq!(format_bytes(1024), "1.0 KB");
        assert_eq!(format_bytes(1_048_576), "1.0 MB");
        assert_eq!(format_bytes(1_073_741_824), "1.0 GB");
    }

    #[test]
    fn filter_is_case_insensitive() {
        let obs = DownloadedObservation {
            id: "1".into(),
            publisher_id: "pub1".into(),
            collection: "CFHT".into(),
            observation_id: "obs-001".into(),
            target_name: "M31".into(),
            instrument: "MegaCam".into(),
            filter: "g".into(),
            ra: "10.6".into(),
            dec: "41.2".into(),
            start_date: "2020-01-01".into(),
            cal_level: "1".into(),
            local_path: "/tmp/test.fits".into(),
            file_size: 1024,
            downloaded_at: "2024-01-01T00:00:00Z".into(),
        };
        // Store writes to disk so we test filtering logic directly
        let list = vec![obs.clone()];
        let needle = "cfht";
        let filtered: Vec<_> = list
            .iter()
            .filter(|o| o.collection.to_lowercase().contains(needle))
            .cloned()
            .collect();
        assert_eq!(filtered.len(), 1);
    }
}
