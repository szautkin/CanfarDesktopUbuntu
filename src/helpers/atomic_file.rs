//! Writing a file so a reader never sees half of it.
//!
//! Three stores had grown their own copy of this — the workflow store, the AI
//! Guide, and now the proposal journal. Two of them differed in a way that
//! mattered: one derived its temp path with `with_extension("json.tmp")`, which
//! is the SAME path for every writer, so two saves racing each other could
//! rename each other's half-written file into place.
//!
//! Write to a uniquely named temp file beside the target, then rename. Rename
//! within a directory is atomic on every filesystem this app runs on, so a
//! reader sees either the old file or the new one.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

/// Distinguishes concurrent writers in this process; the pid distinguishes
/// processes.
static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// Write `text` to `path`, atomically.
///
/// The temp file is dot-prefixed and `.tmp`-suffixed so a directory listing
/// that filters by extension — the workflow store lists `*.workflow.md` — never
/// shows a partial write as an entry.
pub fn write(path: &Path, text: &str) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("{}: no parent directory", path.display()))?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;

    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("atomic-write");
    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp = parent.join(format!(".{}.{}.{}.tmp", file_name, std::process::id(), seq));

    std::fs::write(&tmp, text.as_bytes()).map_err(|e| e.to_string())?;
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            // Leaving the temp file behind would accumulate one per failure.
            let _ = std::fs::remove_file(&tmp);
            Err(e.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "verbinal-atomic-{}-{}-{name}",
            std::process::id(),
            TMP_SEQ.load(Ordering::Relaxed)
        ))
    }

    #[test]
    fn it_writes_the_whole_file_and_leaves_no_temp_behind() {
        let dir = scratch("dir");
        let path = dir.join("state.json");
        write(&path, "{\"a\":1}").expect("write");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "{\"a\":1}");

        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .expect("dir")
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".tmp"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "temp files left behind: {leftovers:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn it_replaces_an_existing_file_rather_than_appending() {
        let dir = scratch("replace");
        let path = dir.join("state.json");
        write(&path, "old and rather long").expect("write");
        write(&path, "new").expect("rewrite");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "new");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Two writers to the same path must not share a temp file.
    ///
    /// The AI Guide's copy used `with_extension("json.tmp")`, which is one name
    /// for every writer: two saves in flight could rename each other's
    /// half-written file into place.
    #[test]
    fn concurrent_writers_do_not_share_a_temp_path() {
        let dir = scratch("concurrent");
        let path = dir.join("state.json");
        std::fs::create_dir_all(&dir).expect("dir");

        let threads: Vec<_> = (0..8)
            .map(|i| {
                let path = path.clone();
                std::thread::spawn(move || write(&path, &format!("writer {i}")))
            })
            .collect();
        for t in threads {
            t.join().expect("thread").expect("write");
        }

        // Whoever won, the file is one complete write — never a mixture.
        let got = std::fs::read_to_string(&path).expect("read");
        assert!(got.starts_with("writer "), "torn write: {got:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
