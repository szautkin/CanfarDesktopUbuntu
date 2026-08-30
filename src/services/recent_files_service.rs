//! Persists the list of recently opened files as JSON under the app data
//! directory, capped at 8 entries. Ported one-to-one from
//! `Services/CubeViewer/RecentCubesService.cs` and modelled on the sibling
//! [`crate::services::recent_launch_service::RecentLaunchService`] (stateless:
//! each call reads/mutates/writes the file, no in-memory cache). Entries whose
//! file no longer exists are dropped on load.
//!
//! One implementation, one file per [`RecentKind`]. The cube viewer had this
//! and the FITS viewer wanted the same thing; the alternative was a second copy
//! that would drift, since this one already holds the cap and the
//! move-to-the-top rule the other would have had to re-derive. The lists stay
//! separate on disk so neither viewer's recents are polluted by the other's —
//! and the cube keeps its original file name, so nothing a user already has is
//! lost.

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Maximum number of recent cubes retained (matches Windows `MaxRecent`).
const MAX_RECENT: usize = 8;

/// What kind of file a list holds, which is also its file name on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecentKind {
    Cube,
    Fits,
}

impl RecentKind {
    /// The store's file name. `recent_cubes.json` is what the cube viewer has
    /// always written, and it keeps it.
    fn file_name(self) -> &'static str {
        match self {
            RecentKind::Cube => "recent_cubes.json",
            RecentKind::Fits => "recent_fits.json",
        }
    }
}

/// One recently opened file: full path, display name (file name), last-opened
/// time. Mirrors the Windows `RecentCubeEntry` record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecentFileEntry {
    pub path: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub opened_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// JSON-backed store of recently opened files, under
/// `ProjectDirs("net","canfar","Verbinal").data_dir()`.
pub struct RecentFilesService {
    file_path: PathBuf,
}

impl RecentFilesService {
    pub fn new(kind: RecentKind) -> Self {
        let name = kind.file_name();
        let file_path = ProjectDirs::from("net", "canfar", "Verbinal")
            .map(|dirs| dirs.data_dir().join(name))
            .unwrap_or_else(|| PathBuf::from(name));
        RecentFilesService { file_path }
    }

    /// Test-only constructor pointing at an explicit JSON file.
    #[cfg(test)]
    fn with_file(file_path: PathBuf) -> Self {
        RecentFilesService { file_path }
    }

    /// Read + deserialize the store, dropping entries whose file no longer exists.
    fn load_entries(&self) -> Vec<RecentFileEntry> {
        if !self.file_path.exists() {
            return Vec::new();
        }
        match std::fs::read_to_string(&self.file_path) {
            Ok(contents) => {
                let loaded: Vec<RecentFileEntry> =
                    serde_json::from_str(&contents).unwrap_or_default();
                // Files deleted/moved since the last session would just produce
                // dead entries — drop them (same as the Windows Load()).
                loaded
                    .into_iter()
                    .filter(|e| !e.path.is_empty() && Path::new(&e.path).exists())
                    .collect()
            }
            Err(_) => Vec::new(),
        }
    }

    fn save_entries(&self, entries: &[RecentFileEntry]) -> Result<(), String> {
        if let Some(parent) = self.file_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let json = serde_json::to_string_pretty(entries).map_err(|e| e.to_string())?;
        std::fs::write(&self.file_path, json).map_err(|e| e.to_string())
    }

    /// Recently opened cube paths, most recent first. Missing files are excluded.
    pub fn list(&self) -> Vec<PathBuf> {
        self.load_entries()
            .into_iter()
            .map(|e| PathBuf::from(e.path))
            .collect()
    }

    /// Record an opened cube, moving an existing path to the top. Capped at 8.
    pub fn add(&self, p: &Path) {
        let path_str = p.to_string_lossy().into_owned();
        let mut entries = self.load_entries();
        // Linux paths are case-sensitive (unlike the Windows OrdinalIgnoreCase).
        entries.retain(|e| e.path != path_str);
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path_str.clone());
        entries.insert(
            0,
            RecentFileEntry {
                path: path_str,
                name,
                opened_at: Some(chrono::Utc::now()),
            },
        );
        entries.truncate(MAX_RECENT);
        let _ = self.save_entries(&entries);
    }

    /// Drop one path from the store.
    ///
    /// Was test-only: nothing in the app ever forgot a recent, because neither
    /// empty state offered it. The shared recents list does.
    pub fn remove(&self, p: &Path) {
        let path_str = p.to_string_lossy().into_owned();
        let mut entries = self.load_entries();
        let before = entries.len();
        entries.retain(|e| e.path != path_str);
        if entries.len() != before {
            let _ = self.save_entries(&entries);
        }
    }

    #[cfg(test)]
    /// Forget everything in this store.
    pub fn clear(&self) {
        if self.file_path.exists() {
            let _ = std::fs::remove_file(&self.file_path);
        }
    }
}

impl Default for RecentFilesService {
    /// The cube list, which is the one that existed before there were two.
    fn default() -> Self {
        Self::new(RecentKind::Cube)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    /// Create a fresh, isolated temp directory for a single test.
    fn temp_dir() -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "verbinal_recent_cubes_{}_{}",
            std::process::id(),
            n
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Create an empty file so `Path::exists()` succeeds and return its path.
    fn touch(dir: &Path, name: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, b"").unwrap();
        p
    }

    #[test]
    fn add_then_list_most_recent_first() {
        let dir = temp_dir();
        let svc = RecentFilesService::with_file(dir.join("recent.json"));
        let a = touch(&dir, "a.fits");
        let b = touch(&dir, "b.fits");

        svc.add(&a);
        svc.add(&b);

        let list = svc.list();
        assert_eq!(list, vec![b.clone(), a.clone()]);
    }

    #[test]
    fn re_adding_moves_to_top_without_duplicates() {
        let dir = temp_dir();
        let svc = RecentFilesService::with_file(dir.join("recent.json"));
        let a = touch(&dir, "a.fits");
        let b = touch(&dir, "b.fits");

        svc.add(&a);
        svc.add(&b);
        svc.add(&a); // move a back to the top

        let list = svc.list();
        assert_eq!(list, vec![a, b]);
    }

    #[test]
    fn capped_at_eight() {
        let dir = temp_dir();
        let svc = RecentFilesService::with_file(dir.join("recent.json"));
        let mut paths = Vec::new();
        for i in 0..12 {
            let p = touch(&dir, &format!("cube{i}.fits"));
            svc.add(&p);
            paths.push(p);
        }
        let list = svc.list();
        assert_eq!(list.len(), MAX_RECENT);
        // Most-recent-first: last added (cube11) is at the front.
        assert_eq!(list[0], paths[11]);
        assert_eq!(list[MAX_RECENT - 1], paths[12 - MAX_RECENT]);
    }

    #[test]
    fn missing_files_dropped_on_list() {
        let dir = temp_dir();
        let svc = RecentFilesService::with_file(dir.join("recent.json"));
        let a = touch(&dir, "a.fits");
        let b = touch(&dir, "b.fits");
        svc.add(&a);
        svc.add(&b);

        std::fs::remove_file(&a).unwrap();

        let list = svc.list();
        assert_eq!(list, vec![b]);
    }

    #[test]
    fn remove_and_clear() {
        let dir = temp_dir();
        let svc = RecentFilesService::with_file(dir.join("recent.json"));
        let a = touch(&dir, "a.fits");
        let b = touch(&dir, "b.fits");
        svc.add(&a);
        svc.add(&b);

        svc.remove(&a);
        assert_eq!(svc.list(), vec![b]);

        svc.clear();
        assert!(svc.list().is_empty());
    }

    #[test]
    fn list_on_empty_store() {
        let dir = temp_dir();
        let svc = RecentFilesService::with_file(dir.join("does_not_exist.json"));
        assert!(svc.list().is_empty());
    }
}
