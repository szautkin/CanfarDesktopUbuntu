use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A single observation that the user has either bookmarked (metadata only)
/// or downloaded (with a local FITS file) from the CADC archive.
///
/// When `local_path` is empty the entry is a bookmark; otherwise it has a
/// downloaded file on disk.  `thumbnail_url` / `preview_url` carry optional
/// DataLink preview URLs so the Research page can show a thumbnail without
/// re-hitting the network.
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
    /// Absolute path to the file on disk. Empty string means "bookmarked only".
    pub local_path: String,
    /// File size in bytes.  Zero when bookmarked only.
    pub file_size: u64,
    /// ISO-8601 timestamp of when the file was downloaded or bookmarked.
    pub downloaded_at: String,
    /// DataLink `#thumbnail` URL, if available. Optional for backwards compat.
    #[serde(default)]
    pub thumbnail_url: String,
    /// DataLink `#preview` URL, if available. Optional for backwards compat.
    #[serde(default)]
    pub preview_url: String,
}

impl DownloadedObservation {
    /// True when this record is metadata-only (no local file).
    pub fn is_bookmarked(&self) -> bool {
        self.local_path.is_empty()
    }

    /// Human-readable file size (e.g. "3.4 MB"). Returns empty string for bookmarks.
    pub fn formatted_size(&self) -> String {
        if self.is_bookmarked() {
            String::new()
        } else {
            format_bytes(self.file_size)
        }
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

    /// Load all observations from disk.
    ///
    /// Performs a load-time cleanup: entries with a non-empty `local_path`
    /// whose file no longer exists are purged from the returned list (and
    /// the JSON file is rewritten to reflect the cleanup).  Bookmarked-only
    /// entries (empty `local_path`) are always kept.
    ///
    /// Returns an empty list on any parse or I/O error.
    pub fn load(&self) -> Vec<DownloadedObservation> {
        if !self.data_path.exists() {
            return Vec::new();
        }
        let raw = match std::fs::read_to_string(&self.data_path) {
            Ok(json) => json,
            Err(_) => return Vec::new(),
        };
        let mut list: Vec<DownloadedObservation> =
            serde_json::from_str(&raw).unwrap_or_default();

        // Purge phantom entries: records whose on-disk file is gone.
        let before = list.len();
        list.retain(|obs| obs.is_bookmarked() || std::path::Path::new(&obs.local_path).exists());
        if list.len() != before {
            // Best-effort rewrite; ignore errors so a read-only disk still
            // returns a usable list.
            let _ = self.write(&list);
        }
        list
    }

    /// Append (or replace by `id`) an observation and flush to disk.
    ///
    /// This is a blocking call — prefer `save_async` from a tokio context.
    pub fn save(&self, obs: DownloadedObservation) -> Result<(), String> {
        let mut list = self.load();
        list.retain(|o| o.id != obs.id);
        list.insert(0, obs);
        self.write(&list)
    }

    /// Remove an observation by its local `id`.
    ///
    /// This is a blocking call — prefer `remove_async` from a tokio context.
    pub fn remove(&self, id: &str) -> Result<(), String> {
        let mut list = self.load();
        list.retain(|o| o.id != id);
        self.write(&list)
    }

    /// Async variant of `save` that offloads disk I/O to the tokio
    /// blocking thread pool.  Call this from any async context.
    pub async fn save_async(&self, obs: DownloadedObservation) -> Result<(), String> {
        let path = self.data_path.clone();
        tokio::task::spawn_blocking(move || {
            let tmp_store = ObservationStore { data_path: path };
            tmp_store.save(obs)
        })
        .await
        .unwrap_or_else(|e| Err(format!("blocking pool error: {e}")))
    }

    /// Async variant of `remove` that offloads disk I/O to the tokio
    /// blocking thread pool.
    pub async fn remove_async(&self, id: &str) -> Result<(), String> {
        let path = self.data_path.clone();
        let id = id.to_string();
        tokio::task::spawn_blocking(move || {
            let tmp_store = ObservationStore { data_path: path };
            tmp_store.remove(&id)
        })
        .await
        .unwrap_or_else(|e| Err(format!("blocking pool error: {e}")))
    }

    /// Async variant of `load` that offloads disk I/O.
    pub async fn load_async(&self) -> Vec<DownloadedObservation> {
        let path = self.data_path.clone();
        tokio::task::spawn_blocking(move || {
            let tmp_store = ObservationStore { data_path: path };
            tmp_store.load()
        })
        .await
        .unwrap_or_default()
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

    fn sample_obs() -> DownloadedObservation {
        DownloadedObservation {
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
            thumbnail_url: String::new(),
            preview_url: String::new(),
        }
    }

    #[test]
    fn filter_is_case_insensitive() {
        let obs = sample_obs();
        let list = vec![obs];
        let needle = "cfht";
        let filtered: Vec<_> = list
            .iter()
            .filter(|o| o.collection.to_lowercase().contains(needle))
            .cloned()
            .collect();
        assert_eq!(filtered.len(), 1);
    }

    #[test]
    fn is_bookmarked_by_empty_local_path() {
        let mut obs = sample_obs();
        assert!(!obs.is_bookmarked());
        obs.local_path = String::new();
        assert!(obs.is_bookmarked());
    }

    #[test]
    fn formatted_size_empty_for_bookmark() {
        let mut obs = sample_obs();
        obs.local_path = String::new();
        obs.file_size = 0;
        assert_eq!(obs.formatted_size(), "");
    }

    #[test]
    fn backwards_compat_json_without_preview_urls() {
        // Older JSON format without thumbnail_url/preview_url fields should
        // still deserialize thanks to #[serde(default)].
        let legacy_json = r#"[
            {
                "id": "1",
                "publisher_id": "pub1",
                "collection": "CFHT",
                "observation_id": "obs-001",
                "target_name": "M31",
                "instrument": "MegaCam",
                "filter": "g",
                "ra": "10.6",
                "dec": "41.2",
                "start_date": "2020-01-01",
                "cal_level": "1",
                "local_path": "/tmp/test.fits",
                "file_size": 1024,
                "downloaded_at": "2024-01-01T00:00:00Z"
            }
        ]"#;
        let parsed: Vec<DownloadedObservation> = serde_json::from_str(legacy_json).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].thumbnail_url, "");
        assert_eq!(parsed[0].preview_url, "");
    }
}
