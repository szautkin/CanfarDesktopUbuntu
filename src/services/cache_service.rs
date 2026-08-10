use chrono::{DateTime, Utc};
use directories::ProjectDirs;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

/// Wrapper stored on disk for every cached API response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry<T> {
    pub cached_at: DateTime<Utc>,
    pub data: T,
}

/// How fresh a cached entry is relative to its TTL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freshness {
    /// Within fresh_duration — no network call needed.
    Fresh,
    /// Past fresh_duration but within max_stale — serve but revalidate.
    Stale,
    /// Past max_stale or missing — must fetch from network.
    Expired,
}

/// Typed cache keys so callers don't fumble with strings.
#[derive(Debug, Clone)]
pub enum CacheKey {
    DataTrainRows,
    ResolverResult { target: String, service: String },
    ContainerImages,
    SessionContext,
    VoSpaceNodes { path: String },
    Sessions,
    StorageQuotaCached { username: String },
}

impl CacheKey {
    /// Relative file path under the cache root.
    fn to_path(&self) -> PathBuf {
        match self {
            CacheKey::DataTrainRows => PathBuf::from("data_train.json"),
            CacheKey::ResolverResult { target, service } => {
                let safe = sanitize_filename(&format!("{}_{}", target, service));
                PathBuf::from("resolver").join(format!("{}.json", safe))
            }
            CacheKey::ContainerImages => PathBuf::from("images.json"),
            CacheKey::SessionContext => PathBuf::from("context.json"),
            CacheKey::VoSpaceNodes { path } => {
                let safe = sanitize_filename(path);
                let safe = if safe.is_empty() {
                    "_root_".to_string()
                } else {
                    safe
                };
                PathBuf::from("vospace").join(format!("{}.json", safe))
            }
            CacheKey::Sessions => PathBuf::from("sessions.json"),
            CacheKey::StorageQuotaCached { username } => {
                PathBuf::from(format!("quota_{}.json", sanitize_filename(username)))
            }
        }
    }

    /// (fresh_duration, max_stale) for this key type.
    fn ttl(&self) -> (Duration, Option<Duration>) {
        match self {
            CacheKey::DataTrainRows => (
                Duration::from_secs(24 * 3600),            // 24h fresh
                Some(Duration::from_secs(30 * 24 * 3600)), // 30 days max stale
            ),
            CacheKey::ResolverResult { .. } => (
                Duration::from_secs(7 * 24 * 3600), // 7 days fresh
                None,                               // never expire
            ),
            CacheKey::ContainerImages => (
                Duration::from_secs(3600),                // 1h fresh
                Some(Duration::from_secs(7 * 24 * 3600)), // 7 days max stale
            ),
            CacheKey::SessionContext => (
                Duration::from_secs(6 * 3600),            // 6h fresh
                Some(Duration::from_secs(7 * 24 * 3600)), // 7 days max stale
            ),
            CacheKey::VoSpaceNodes { .. } => (
                Duration::from_secs(300),        // 5 min fresh
                Some(Duration::from_secs(3600)), // 1h max stale
            ),
            CacheKey::Sessions => (
                Duration::from_secs(30),        // 30s fresh
                Some(Duration::from_secs(600)), // 10min max stale
            ),
            CacheKey::StorageQuotaCached { .. } => (
                Duration::from_secs(900),            // 15min fresh
                Some(Duration::from_secs(6 * 3600)), // 6h max stale
            ),
        }
    }
}

/// XDG-compliant disk cache under `~/.cache/verbinal/api_cache/`.
pub struct CacheService {
    cache_dir: PathBuf,
}

impl CacheService {
    pub fn new() -> Self {
        let cache_dir = ProjectDirs::from("net", "canfar", "Verbinal")
            .map(|dirs| dirs.cache_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from("/tmp/verbinal-cache"))
            .join("api_cache");

        CacheService { cache_dir }
    }

    /// Read a cached entry. Returns None if missing or corrupt.
    pub fn read<T: DeserializeOwned>(&self, key: &CacheKey) -> Option<CacheEntry<T>> {
        let path = self.cache_dir.join(key.to_path());
        let bytes = std::fs::read(&path).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    /// Check freshness of a cached entry without deserialising the full payload.
    pub fn freshness_of<T: DeserializeOwned>(&self, key: &CacheKey) -> Freshness {
        match self.read::<T>(key) {
            None => Freshness::Expired,
            Some(entry) => self.entry_freshness(key, &entry),
        }
    }

    /// Compute freshness for an already-loaded entry.
    pub fn entry_freshness<T>(&self, key: &CacheKey, entry: &CacheEntry<T>) -> Freshness {
        let age = Utc::now()
            .signed_duration_since(entry.cached_at)
            .to_std()
            .unwrap_or(Duration::ZERO);
        let (fresh, max_stale) = key.ttl();
        if age <= fresh {
            Freshness::Fresh
        } else if max_stale.is_none_or(|ms| age <= ms) {
            Freshness::Stale
        } else {
            Freshness::Expired
        }
    }

    /// Write a value to cache. Atomic: writes to `.tmp` then renames.
    pub fn write<T: Serialize>(&self, key: &CacheKey, data: &T) {
        let path = self.cache_dir.join(key.to_path());
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let entry = CacheEntry {
            cached_at: Utc::now(),
            data,
        };
        let tmp = path.with_extension("tmp");
        if let Ok(json) = serde_json::to_string(&entry) {
            if std::fs::write(&tmp, &json).is_ok() {
                let _ = std::fs::rename(&tmp, &path);
            }
        }
    }

    /// Human-readable timestamp for a cached entry (e.g. "14:32").
    pub fn cached_time_label(&self, key: &CacheKey) -> Option<String> {
        let entry = self.read::<serde_json::Value>(key)?;
        let local: DateTime<chrono::Local> = entry.cached_at.into();
        Some(local.format("%H:%M").to_string())
    }
}

fn sanitize_filename(input: &str) -> String {
    input
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect::<String>()
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_round_trip() {
        let svc = CacheService {
            cache_dir: std::env::temp_dir().join("verbinal_test_cache"),
        };
        let key = CacheKey::DataTrainRows;
        let data: Vec<String> = vec!["a".into(), "b".into()];
        svc.write(&key, &data);

        let entry: CacheEntry<Vec<String>> = svc.read(&key).expect("should read back");
        assert_eq!(entry.data, vec!["a".to_string(), "b".to_string()]);

        let freshness = svc.entry_freshness(&key, &entry);
        assert_eq!(freshness, Freshness::Fresh);

        // Cleanup
        let _ = std::fs::remove_dir_all(svc.cache_dir);
    }

    #[test]
    fn missing_key_returns_none() {
        let svc = CacheService {
            cache_dir: std::env::temp_dir().join("verbinal_test_cache_miss"),
        };
        let result: Option<CacheEntry<Vec<String>>> = svc.read(&CacheKey::ContainerImages);
        assert!(result.is_none());
    }

    #[test]
    fn sanitize_filename_works() {
        assert_eq!(sanitize_filename("M31_ALL"), "m31_all");
        assert_eq!(sanitize_filename("ivo://cadc/foo"), "ivo___cadc_foo");
    }
}
