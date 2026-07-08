//! Notebook autosave + crash-recovery checkpoint files.
//!
//! Port of `Services/Notebook/AutoSaveService.cs` + `RecoveryService.cs`. A
//! checkpoint of every open, dirty notebook is written to a dedicated AutoSave
//! directory on a timer (atomic tmp+rename), skipped when clean, and deleted on a
//! clean close. Orphaned files left by a crash are surfaced on next launch.

use crate::models::notebook_document::NotebookDocument;
use directories::ProjectDirs;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

const SUFFIX: &str = ".autosave.ipynb";

/// The directory holding autosave checkpoints.
pub fn autosave_dir() -> PathBuf {
    ProjectDirs::from("net", "canfar", "Verbinal")
        .map(|d| d.data_dir().join("AutoSave"))
        .unwrap_or_else(|| PathBuf::from("verbinal-autosave"))
}

fn short_hash(key: &str) -> String {
    let mut h = DefaultHasher::new();
    key.hash(&mut h);
    format!("{:08x}", (h.finish() & 0xFFFF_FFFF) as u32)
}

fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect()
}

/// A stable per-notebook checkpoint path. `key` (the original path, or a unique id
/// for never-saved notebooks) keeps same-name files in different dirs distinct.
pub fn autosave_path_for(key: &str, display_name: &str) -> PathBuf {
    autosave_dir().join(format!(
        "{}.{}{}",
        sanitize(display_name),
        short_hash(key),
        SUFFIX
    ))
}

/// Write a checkpoint atomically (tmp + rename). Creates the dir on demand.
pub fn write_autosave(doc: &NotebookDocument, path: &Path) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("tmp");
    crate::helpers::notebook_parser::save_notebook(doc, &tmp)?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

/// Delete a checkpoint (called on a clean close). Best-effort.
pub fn delete_autosave(path: &Path) {
    let _ = std::fs::remove_file(path);
}

/// A recoverable orphaned autosave file left by a previous (crashed) session.
#[derive(Debug, Clone)]
pub struct RecoveryCandidate {
    pub path: PathBuf,
    /// The notebook's display name (autosave suffix + hash stripped).
    pub display_name: String,
}

/// Scan the AutoSave directory for orphaned checkpoints.
pub fn detect_orphans() -> Vec<RecoveryCandidate> {
    let dir = autosave_dir();
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return out,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) if n.ends_with(SUFFIX) => n.to_string(),
            _ => continue,
        };
        // "<display>.<hash8>.autosave.ipynb" → display
        let stem = &name[..name.len() - SUFFIX.len()];
        let display = strip_hash_suffix(stem);
        out.push(RecoveryCandidate {
            path,
            display_name: display,
        });
    }
    out
}

/// Delete a single recovery candidate.
pub fn discard(path: &Path) {
    let _ = std::fs::remove_file(path);
}

/// Delete every orphaned checkpoint.
pub fn discard_all() {
    for c in detect_orphans() {
        let _ = std::fs::remove_file(&c.path);
    }
}

/// Load a checkpoint back into a document for recovery.
pub fn load_autosave(path: &Path) -> Result<NotebookDocument, String> {
    crate::helpers::notebook_parser::load_notebook(path)
}

/// Strip a trailing ".<8 hex>" hash segment, leaving the display name.
fn strip_hash_suffix(stem: &str) -> String {
    if let Some((head, tail)) = stem.rsplit_once('.') {
        if tail.len() == 8 && tail.chars().all(|c| c.is_ascii_hexdigit()) {
            return head.to_string();
        }
    }
    stem.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_is_stable_and_distinct() {
        let a = autosave_path_for("/home/u/a.ipynb", "a.ipynb");
        let b = autosave_path_for("/other/a.ipynb", "a.ipynb");
        assert_eq!(a, autosave_path_for("/home/u/a.ipynb", "a.ipynb"));
        assert_ne!(a, b); // same name, different dir → different checkpoint
        assert!(a.to_string_lossy().ends_with(".autosave.ipynb"));
    }

    #[test]
    fn display_name_strips_hash() {
        assert_eq!(strip_hash_suffix("my_nb.1a2b3c4d"), "my_nb");
        assert_eq!(strip_hash_suffix("no_hash"), "no_hash");
    }
}
