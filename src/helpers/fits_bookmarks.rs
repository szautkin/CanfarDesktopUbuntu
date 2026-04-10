//! Persistent storage for FITS coordinate bookmarks.
//!
//! Bookmarks are saved as JSON at `~/.local/share/verbinal/fits_bookmarks.json`
//! via the `directories` crate. Each bookmark captures a saved sky position
//! with a user-chosen label and the source FITS filename.

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FitsBookmark {
    /// Unix timestamp at creation time, used as a stable id for removal.
    pub id: u64,
    pub label: String,
    pub ra_deg: f64,
    pub dec_deg: f64,
    pub source_file: String,
}

/// Return the on-disk path where bookmarks are stored.
fn bookmarks_path() -> Option<PathBuf> {
    ProjectDirs::from("net", "canfar", "Verbinal")
        .map(|dirs| dirs.data_dir().join("fits_bookmarks.json"))
}

/// Load all bookmarks. Returns empty Vec on any error (missing/corrupt file).
pub fn load_bookmarks() -> Vec<FitsBookmark> {
    let path = match bookmarks_path() {
        Some(p) => p,
        None => return Vec::new(),
    };
    let bytes = match fs::read(&path) {
        Ok(b) => b,
        Err(_) => return Vec::new(),
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

/// Persist the bookmark list to disk atomically.
pub fn save_bookmarks(bookmarks: &[FitsBookmark]) -> Result<(), String> {
    let path = bookmarks_path().ok_or_else(|| "Cannot resolve data directory".to_string())?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("Cannot create data dir: {}", e))?;
    }
    let json = serde_json::to_string_pretty(bookmarks)
        .map_err(|e| format!("Cannot serialize bookmarks: {}", e))?;
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, &json).map_err(|e| format!("Cannot write bookmarks: {}", e))?;
    fs::rename(&tmp, &path).map_err(|e| format!("Cannot rename bookmarks file: {}", e))?;
    Ok(())
}

/// Add a new bookmark to the list (generating an id) and persist.
pub fn add_bookmark(
    label: String,
    ra_deg: f64,
    dec_deg: f64,
    source_file: String,
) -> Result<Vec<FitsBookmark>, String> {
    let mut list = load_bookmarks();
    let id = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    list.push(FitsBookmark {
        id,
        label,
        ra_deg,
        dec_deg,
        source_file,
    });
    save_bookmarks(&list)?;
    Ok(list)
}

/// Remove a bookmark by id and persist.
pub fn remove_bookmark(id: u64) -> Result<Vec<FitsBookmark>, String> {
    let mut list = load_bookmarks();
    list.retain(|b| b.id != id);
    save_bookmarks(&list)?;
    Ok(list)
}
