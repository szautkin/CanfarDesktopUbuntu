//! Owns the two synchronous workflow tiers: read-only built-in templates
//! (embedded at compile time) and the user's local working copies under
//! `<data_dir>/workflows/*.workflow.md` — the ONLY tier where check-off state is
//! written (the file itself IS the state). VOSpace workflows are a remote tier
//! handled elsewhere, so this type stays synchronous, simple, and unit-testable
//! against a temp directory.
//!
//! Ported one-to-one from `Services/Workflows/WorkflowStore.cs`. Mirrors
//! `observation_store.rs` for the `ProjectDirs` location and the write-to-temp-
//! then-rename atomic write idiom. Local files are always written as UTF-8 with
//! no BOM.

use crate::helpers::workflow_format::{self, FILE_EXTENSION};
use crate::models::workflow::{WorkflowInfo, WorkflowSource};
use directories::ProjectDirs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Id prefix for read-only bundled templates (`builtin:<slug>`).
pub const BUILTIN_PREFIX: &str = "builtin:";
/// Id prefix for user working copies on disk (`local:<slug>`).
pub const LOCAL_PREFIX: &str = "local:";

/// The 7 bundled templates in a fixed slug order, embedded at compile time.
/// The order here is authoritative for `list_built_in`.
const BUILT_INS: &[(&str, &str)] = &[
    (
        "cfht-imaging-recon",
        include_str!("../../assets/workflows/cfht-imaging-recon.workflow.md"),
    ),
    (
        "variable-star-photometry",
        include_str!("../../assets/workflows/variable-star-photometry.workflow.md"),
    ),
    (
        "jcmt-cube-kinematics",
        include_str!("../../assets/workflows/jcmt-cube-kinematics.workflow.md"),
    ),
    (
        "dao-espadons-spectroscopy",
        include_str!("../../assets/workflows/dao-espadons-spectroscopy.workflow.md"),
    ),
    (
        "vizier-cadc-crossmatch",
        include_str!("../../assets/workflows/vizier-cadc-crossmatch.workflow.md"),
    ),
    (
        "canfar-batch-reprocessing",
        include_str!("../../assets/workflows/canfar-batch-reprocessing.workflow.md"),
    ),
    (
        "proposal-due-diligence",
        include_str!("../../assets/workflows/proposal-due-diligence.workflow.md"),
    ),
];

/// Monotonic counter to keep atomic-write temp filenames unique within a
/// process (combined with the pid) even under concurrent writes to one file.
static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// Persistent, synchronous store of built-in + local workflows.
pub struct WorkflowStore {
    /// Directory holding the user's local `*.workflow.md` working copies.
    directory: PathBuf,
}

impl WorkflowStore {
    /// Local dir = `ProjectDirs("net","canfar","Verbinal").data_dir()/workflows`.
    pub fn new() -> Self {
        let directory = ProjectDirs::from("net", "canfar", "Verbinal")
            .map(|dirs| dirs.data_dir().join("workflows"))
            .unwrap_or_else(|| PathBuf::from("workflows"));
        WorkflowStore { directory }
    }

    /// Construct a store rooted at an explicit directory (tests only).
    #[cfg(test)]
    fn with_dir(directory: PathBuf) -> Self {
        WorkflowStore { directory }
    }

    // ── Listing / reading ──────────────────────────────────────────────────

    /// The bundled read-only templates, returned in the fixed embedded slug
    /// order. Each parses its embedded text into a `WorkflowDoc`.
    pub fn list_built_in(&self) -> Vec<WorkflowInfo> {
        BUILT_INS
            .iter()
            .map(|(slug, text)| built_in_info(slug, text))
            .collect()
    }

    /// The user's local working copies, parsed from disk and sorted by title
    /// (case-insensitive) for a deterministic UI order. Unreadable files are
    /// skipped rather than breaking the whole list. Empty when the directory
    /// does not exist yet.
    pub fn list_local(&self) -> Vec<WorkflowInfo> {
        let entries = match std::fs::read_dir(&self.directory) {
            Ok(e) => e,
            Err(_) => return Vec::new(),
        };
        let mut list: Vec<WorkflowInfo> = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if !is_workflow_file(&path) {
                continue;
            }
            let text = match std::fs::read_to_string(&path) {
                Ok(t) => t,
                Err(_) => continue, // unreadable file — skip, don't break the list
            };
            let slug = slug_of(&path);
            list.push(WorkflowInfo {
                id: format!("{}{}", LOCAL_PREFIX, slug),
                source: WorkflowSource::Local,
                doc: workflow_format::parse(&text),
                raw_text: text,
            });
        }
        list.sort_by(|a, b| a.doc.title.to_lowercase().cmp(&b.doc.title.to_lowercase()));
        list
    }

    /// Resolve any id (`builtin:<slug>` or `local:<slug>`) to its info, or
    /// `None` if unknown / missing / an unrecognized prefix.
    pub fn get(&self, id: &str) -> Option<WorkflowInfo> {
        if let Some(slug) = id.strip_prefix(BUILTIN_PREFIX) {
            return BUILT_INS
                .iter()
                .find(|(s, _)| s.eq_ignore_ascii_case(slug))
                .map(|(s, text)| built_in_info(s, text));
        }
        if let Some(slug) = id.strip_prefix(LOCAL_PREFIX) {
            let path = self.path_of_slug(slug);
            let text = std::fs::read_to_string(&path).ok()?;
            return Some(WorkflowInfo {
                id: id.to_string(),
                source: WorkflowSource::Local,
                doc: workflow_format::parse(&text),
                raw_text: text,
            });
        }
        None
    }

    // ── Mutations (local tier only) ────────────────────────────────────────

    /// Create a new local workflow from raw text. The id is derived from
    /// `name` by slugifying and de-duplicating (`-2`, `-3`, …) against existing
    /// files. Writes UTF-8 (no BOM) atomically and returns the new info.
    pub fn save_new(&self, name: &str, text: &str) -> Result<WorkflowInfo, String> {
        std::fs::create_dir_all(&self.directory).map_err(|e| e.to_string())?;
        let slug = slugify(name);
        let mut candidate = slug.clone();
        let mut n = 2u32;
        while self.path_of_slug(&candidate).exists() {
            candidate = format!("{}-{}", slug, n);
            n += 1;
        }
        let path = self.path_of_slug(&candidate);
        write_atomic(&path, text)?;
        Ok(WorkflowInfo {
            id: format!("{}{}", LOCAL_PREFIX, candidate),
            source: WorkflowSource::Local,
            doc: workflow_format::parse(text),
            raw_text: text.to_string(),
        })
    }

    /// Replace a local workflow's full text (editor / update flow). Errors on
    /// a built-in id or a missing local file.
    pub fn update_text(&self, id: &str, text: &str) -> Result<(), String> {
        let path = self.require_local_path(id)?;
        write_atomic(&path, text)
    }

    /// Flip one step's done-marker in place — only the checkbox character
    /// changes (see `workflow_format::with_step_done`). Reads, rewrites
    /// atomically, and returns the reparsed info. Errors on a built-in id, a
    /// missing local file, or an out-of-range step index.
    pub fn set_step_done(
        &self,
        id: &str,
        step_index: usize,
        done: bool,
    ) -> Result<WorkflowInfo, String> {
        let path = self.require_local_path(id)?;
        let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        let new_text = workflow_format::with_step_done(&text, step_index, done)?;
        write_atomic(&path, &new_text)?;
        Ok(WorkflowInfo {
            id: id.to_string(),
            source: WorkflowSource::Local,
            doc: workflow_format::parse(&new_text),
            raw_text: new_text,
        })
    }

    /// Delete a local workflow. Errors on a built-in id or a missing file.
    pub fn delete(&self, id: &str) -> Result<(), String> {
        let path = self.require_local_path(id)?;
        std::fs::remove_file(&path).map_err(|e| e.to_string())
    }

    // ── Helpers ────────────────────────────────────────────────────────────

    /// Absolute path of the local file for a bare slug.
    fn path_of_slug(&self, slug: &str) -> PathBuf {
        self.directory.join(format!("{}{}", slug, FILE_EXTENSION))
    }

    /// Resolve a local id to an existing file path, or produce the same
    /// human-readable errors the C# original does for the built-in / unknown
    /// / missing cases.
    fn require_local_path(&self, id: &str) -> Result<PathBuf, String> {
        if let Some(slug) = id.strip_prefix(LOCAL_PREFIX) {
            let path = self.path_of_slug(slug);
            if path.exists() {
                return Ok(path);
            }
            return Err(format!(
                "No local workflow '{}'. Call list_workflows for ids.",
                id
            ));
        }
        if id.starts_with(BUILTIN_PREFIX) {
            return Err(format!(
                "'{}' is a read-only template — call use_workflow to make a local working copy first.",
                id
            ));
        }
        Err(format!(
            "No local workflow '{}'. Call list_workflows for ids.",
            id
        ))
    }
}

impl Default for WorkflowStore {
    fn default() -> Self {
        Self::new()
    }
}

// ── Free functions ─────────────────────────────────────────────────────────

/// Build a `WorkflowInfo` for a built-in slug + embedded text.
fn built_in_info(slug: &str, text: &str) -> WorkflowInfo {
    WorkflowInfo {
        id: format!("{}{}", BUILTIN_PREFIX, slug),
        source: WorkflowSource::BuiltIn,
        doc: workflow_format::parse(text),
        raw_text: text.to_string(),
    }
}

/// True when `path` is a regular file whose name ends with `.workflow.md`
/// (case-insensitive).
fn is_workflow_file(path: &Path) -> bool {
    path.is_file()
        && path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.to_ascii_lowercase().ends_with(FILE_EXTENSION))
            .unwrap_or(false)
}

/// The slug of a `<slug>.workflow.md` path (the file name minus the extension).
fn slug_of(path: &Path) -> String {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    // The extension is ASCII, so `len - ext.len()` is always a valid char
    // boundary when the (case-insensitive) suffix matches.
    if name.to_ascii_lowercase().ends_with(FILE_EXTENSION) {
        name[..name.len() - FILE_EXTENSION.len()].to_string()
    } else {
        name.to_string()
    }
}

/// Slugify a display name: trim, lowercase, map every non-alphanumeric char to
/// `-`, collapse runs of `-` (dropping empties), and clamp to 60 chars. An
/// all-punctuation / empty name becomes `"workflow"`. Mirrors
/// `WorkflowStore.Slugify` in C#.
fn slugify(name: &str) -> String {
    let mut mapped = String::new();
    for c in name.trim().to_lowercase().chars() {
        mapped.push(if c.is_alphanumeric() { c } else { '-' });
    }
    let slug = mapped
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if slug.is_empty() {
        "workflow".to_string()
    } else {
        slug.chars().take(60).collect()
    }
}

/// Atomically write `text` (UTF-8, no BOM) to `path`: create the parent dir,
/// write to a uniquely named temp sibling in the SAME directory, then rename
/// over the target so a crash or partial write never corrupts the file.
fn write_atomic(path: &Path, text: &str) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "invalid workflow path: no parent directory".to_string())?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;

    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("workflow");
    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    // Leading dot + `.tmp` suffix keeps the temp file hidden and out of
    // `list_local` (it does not end with `.workflow.md`).
    let tmp = parent.join(format!(".{}.{}.{}.tmp", file_name, std::process::id(), seq));

    std::fs::write(&tmp, text.as_bytes()).map_err(|e| e.to_string())?;
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Per-test unique temp dir seed, so parallel tests never share state.
    static TEST_SEQ: AtomicU64 = AtomicU64::new(0);

    /// Create a fresh, empty temp directory and a store rooted there. The dir
    /// name derives from the pid + a monotonic counter (no RNG).
    fn temp_store() -> (WorkflowStore, PathBuf) {
        let seq = TEST_SEQ.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("verbinal-wf-test-{}-{}", std::process::id(), seq));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        (WorkflowStore::with_dir(dir.clone()), dir)
    }

    const SAMPLE: &str = "# Sample Protocol\n> A tiny test protocol.\nTags: test\n\n## Steps\n\n- [ ] **First step** — do the thing.\n- [ ] **Second step** — do another thing.\n";

    #[test]
    fn slugify_basic_and_collapse() {
        assert_eq!(
            slugify("Variable Star Photometry"),
            "variable-star-photometry"
        );
        // Punctuation and repeated separators collapse to single dashes.
        assert_eq!(slugify("Hello, World!"), "hello-world");
        assert_eq!(slugify("  Leading/trailing  --  "), "leading-trailing");
    }

    #[test]
    fn slugify_empty_and_all_punctuation_fallback() {
        assert_eq!(slugify(""), "workflow");
        assert_eq!(slugify("   "), "workflow");
        assert_eq!(slugify("!!!///"), "workflow");
    }

    #[test]
    fn slugify_clamps_to_60_chars() {
        let name = "a".repeat(100);
        let slug = slugify(&name);
        assert_eq!(slug.chars().count(), 60);
    }

    #[test]
    fn built_in_list_has_all_seven_in_fixed_order() {
        let (store, _dir) = temp_store();
        let builtins = store.list_built_in();
        assert_eq!(builtins.len(), 7);
        // First is the fixed leading slug, all ids carry the builtin prefix.
        assert_eq!(builtins[0].id, "builtin:cfht-imaging-recon");
        assert!(builtins.iter().all(|w| w.id.starts_with(BUILTIN_PREFIX)));
        assert!(builtins.iter().all(|w| w.source == WorkflowSource::BuiltIn));
        // get() dispatches to the same built-in.
        let got = store.get("builtin:cfht-imaging-recon").unwrap();
        assert_eq!(got.id, "builtin:cfht-imaging-recon");
    }

    #[test]
    fn save_new_dedups_with_numeric_suffix() {
        let (store, _dir) = temp_store();
        let a = store.save_new("My Protocol", SAMPLE).unwrap();
        let b = store.save_new("My Protocol", SAMPLE).unwrap();
        let c = store.save_new("My Protocol", SAMPLE).unwrap();
        assert_eq!(a.id, "local:my-protocol");
        assert_eq!(b.id, "local:my-protocol-2");
        assert_eq!(c.id, "local:my-protocol-3");
        assert_eq!(store.list_local().len(), 3);
    }

    #[test]
    fn set_step_done_round_trip_persists() {
        let (store, _dir) = temp_store();
        let info = store.save_new("Round Trip", SAMPLE).unwrap();
        assert_eq!(info.doc.done_count(), 0);

        // Check step 1 (0-based) — returned info reflects it, and re-reading
        // from disk agrees.
        let after = store.set_step_done(&info.id, 1, true).unwrap();
        assert_eq!(after.doc.done_count(), 1);
        assert!(after.doc.steps[1].done);
        assert!(!after.doc.steps[0].done);
        let reread = store.get(&info.id).unwrap();
        assert_eq!(reread.doc.done_count(), 1);
        assert!(reread.raw_text.contains("- [x] **Second step**"));

        // Uncheck it again — back to zero.
        let back = store.set_step_done(&info.id, 1, false).unwrap();
        assert_eq!(back.doc.done_count(), 0);
        assert!(store
            .get(&info.id)
            .unwrap()
            .raw_text
            .contains("- [ ] **Second step**"));
    }

    #[test]
    fn set_step_done_out_of_range_errors() {
        let (store, _dir) = temp_store();
        let info = store.save_new("Bounds", SAMPLE).unwrap();
        assert!(store.set_step_done(&info.id, 9, true).is_err());
    }

    #[test]
    fn mutations_reject_builtin_ids() {
        let (store, _dir) = temp_store();
        assert!(store
            .update_text("builtin:cfht-imaging-recon", SAMPLE)
            .is_err());
        assert!(store
            .set_step_done("builtin:cfht-imaging-recon", 0, true)
            .is_err());
        assert!(store.delete("builtin:cfht-imaging-recon").is_err());
    }

    #[test]
    fn update_and_delete_local_round_trip() {
        let (store, _dir) = temp_store();
        let info = store.save_new("Editable", SAMPLE).unwrap();
        store
            .update_text(&info.id, "# Renamed\n\n- [ ] **Only step**\n")
            .unwrap();
        let got = store.get(&info.id).unwrap();
        assert_eq!(got.doc.title, "Renamed");
        assert_eq!(got.doc.steps.len(), 1);

        store.delete(&info.id).unwrap();
        assert!(store.get(&info.id).is_none());
        assert!(store.list_local().is_empty());
        // Deleting a now-missing file errors.
        assert!(store.delete(&info.id).is_err());
    }

    #[test]
    fn get_unknown_prefix_is_none() {
        let (store, _dir) = temp_store();
        assert!(store.get("vospace:/foo").is_none());
        assert!(store.get("local:does-not-exist").is_none());
    }
}
