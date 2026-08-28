use crate::models::notebook_document::{NotebookCell, NotebookDocument};
use once_cell::sync::Lazy;
use regex::Regex;
use std::path::Path;

/// Matches `import <module>` at the start of a line.
static RE_IMPORT: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\s*import\s+([\w.]+)").unwrap());

/// Matches `from <module> import ...` at the start of a line.
static RE_FROM: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\s*from\s+([\w.]+)\s+import\s+").unwrap());

// ---------------------------------------------------------------------------
// Python stdlib module names (incomplete but covers the vast majority).
// Used to filter `extract_imports` so only third-party deps are returned.
// ---------------------------------------------------------------------------

static STDLIB_MODULES: &[&str] = &[
    "__future__",
    "_thread",
    "abc",
    "aifc",
    "argparse",
    "array",
    "ast",
    "asynchat",
    "asyncio",
    "asyncore",
    "atexit",
    "audioop",
    "base64",
    "bdb",
    "binascii",
    "binhex",
    "bisect",
    "builtins",
    "bz2",
    "calendar",
    "cgi",
    "cgitb",
    "chunk",
    "cmath",
    "cmd",
    "code",
    "codecs",
    "codeop",
    "colorsys",
    "compileall",
    "concurrent",
    "configparser",
    "contextlib",
    "contextvars",
    "copy",
    "copyreg",
    "cProfile",
    "csv",
    "ctypes",
    "curses",
    "dataclasses",
    "datetime",
    "dbm",
    "decimal",
    "difflib",
    "dis",
    "distutils",
    "doctest",
    "email",
    "encodings",
    "enum",
    "errno",
    "faulthandler",
    "fcntl",
    "filecmp",
    "fileinput",
    "fnmatch",
    "fractions",
    "ftplib",
    "functools",
    "gc",
    "getopt",
    "getpass",
    "gettext",
    "glob",
    "grp",
    "gzip",
    "hashlib",
    "heapq",
    "hmac",
    "html",
    "http",
    "idlelib",
    "imaplib",
    "importlib",
    "inspect",
    "io",
    "ipaddress",
    "itertools",
    "json",
    "keyword",
    "lib2to3",
    "linecache",
    "locale",
    "logging",
    "lzma",
    "mailbox",
    "marshal",
    "math",
    "mimetypes",
    "mmap",
    "modulefinder",
    "multiprocessing",
    "netrc",
    "nis",
    "nntplib",
    "numbers",
    "operator",
    "optparse",
    "os",
    "ossaudiodev",
    "pathlib",
    "pdb",
    "pickle",
    "pickletools",
    "pipes",
    "pkgutil",
    "platform",
    "plistlib",
    "poplib",
    "posix",
    "posixpath",
    "pprint",
    "profile",
    "pstats",
    "pty",
    "pwd",
    "py_compile",
    "pyclbr",
    "pydoc",
    "queue",
    "quopri",
    "random",
    "re",
    "readline",
    "reprlib",
    "resource",
    "rlcompleter",
    "runpy",
    "sched",
    "secrets",
    "select",
    "selectors",
    "shelve",
    "shlex",
    "shutil",
    "signal",
    "site",
    "smtpd",
    "smtplib",
    "sndhdr",
    "socket",
    "socketserver",
    "spwd",
    "sqlite3",
    "sre_compile",
    "sre_constants",
    "sre_parse",
    "ssl",
    "stat",
    "statistics",
    "string",
    "stringprep",
    "struct",
    "subprocess",
    "sunau",
    "symtable",
    "sys",
    "sysconfig",
    "syslog",
    "tabnanny",
    "tarfile",
    "telnetlib",
    "tempfile",
    "termios",
    "test",
    "textwrap",
    "threading",
    "time",
    "timeit",
    "tkinter",
    "token",
    "tokenize",
    "tomllib",
    "trace",
    "traceback",
    "tracemalloc",
    "tty",
    "turtle",
    "turtledemo",
    "types",
    "typing",
    "unicodedata",
    "unittest",
    "urllib",
    "uu",
    "uuid",
    "venv",
    "warnings",
    "wave",
    "weakref",
    "webbrowser",
    "winreg",
    "winsound",
    "wsgiref",
    "xdrlib",
    "xml",
    "xmlrpc",
    "zipapp",
    "zipfile",
    "zipimport",
    "zlib",
    "zoneinfo",
    // IPython / Jupyter builtins that are not "third-party" in the usual sense.
    "IPython",
    "ipykernel",
    "ipywidgets",
];

fn is_stdlib(module: &str) -> bool {
    // Compare the top-level package name only (e.g. "os.path" → "os").
    let top = module.split('.').next().unwrap_or(module);
    STDLIB_MODULES.contains(&top)
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Load a `.ipynb` notebook from disk and return a parsed `NotebookDocument`.
///
/// The function enforces a maximum of 10 000 cells and assigns fresh cell IDs
/// to any cells that are missing one (as required by nbformat 4.5+).
/// Read a file the notebook is about to open, refusing one that is too large.
///
/// `read_to_string` pulls the whole file into memory before anything can look
/// at it. The only limit was on the number of CELLS, which a `.txt` or a `.py`
/// reaches long after the bytes are already in RAM — and in an astronomy folder
/// the file most likely to be enormous is exactly the kind now openable: a
/// source catalogue saved as `.txt` beside the notes it belongs to. Asking the
/// filesystem for the size first costs one `stat`.
///
/// The ceiling is [`NotebookSettings::max_open_file_mb`], so a workstation can
/// raise it.
pub fn read_within_limit(path: &Path) -> Result<String, String> {
    let limit_mb = crate::services::notebook_settings_service::NotebookSettingsService::new()
        .load()
        .max_open_file_mb;
    read_within(path, limit_mb)
}

/// The same, with the limit passed in.
///
/// Split from the setting lookup so the decision can be tested. Reading the
/// user's settings file is I/O against a global, and a rule that can only be
/// exercised by writing that file is a rule nobody exercises — this one shipped
/// with no test at all.
fn read_within(path: &Path, limit_mb: u32) -> Result<String, String> {
    if let Ok(meta) = std::fs::metadata(path) {
        if let Some(refusal) = too_large(path, meta.len(), limit_mb) {
            return Err(refusal);
        }
    }
    std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {}", path.display(), e))
}

/// Why `size` is too large to open, or `None` if it is not.
///
/// The message names the setting, because the remedy is a setting and a user
/// told only "too large" has nowhere to go.
fn too_large(path: &Path, size: u64, limit_mb: u32) -> Option<String> {
    let limit = u64::from(limit_mb) * 1024 * 1024;
    (size > limit).then(|| {
        format!(
            "{} is {:.1} MB, over the {} MB limit for opening a notebook. Raise \
             \"Largest file to open\" in the notebook settings if this is really a notebook.",
            path.display(),
            size as f64 / (1024.0 * 1024.0),
            limit_mb
        )
    })
}

pub fn load_notebook(path: &Path) -> Result<NotebookDocument, String> {
    let contents = read_within_limit(path)?;

    let mut doc: NotebookDocument = serde_json::from_str(&contents)
        .map_err(|e| format!("invalid notebook JSON in {}: {}", path.display(), e))?;

    // Cap cells to prevent OOM on pathological inputs.
    const MAX_CELLS: usize = 10_000;
    if doc.cells.len() > MAX_CELLS {
        doc.cells.truncate(MAX_CELLS);
    }

    // Assign IDs to cells that lack them (older nbformat versions).
    for cell in &mut doc.cells {
        if cell.id.is_none() {
            cell.id = Some(NotebookDocument::generate_cell_id());
        }
    }

    Ok(doc)
}

/// Persist a `NotebookDocument` to disk using an atomic write (write to a
/// `.tmp` file then rename).
///
/// The JSON is indented with a single space to match Jupyter's own output.
/// `path` with an `.ipynb` extension, unless it already carries one.
///
/// A file chooser returns exactly what was typed. A notebook saved as
/// `analysis` is still a notebook, but nothing else will treat it as one: our
/// own Open dialog filters for `*.ipynb`, and so does Jupyter's.
pub fn with_ipynb_extension(path: std::path::PathBuf) -> std::path::PathBuf {
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) if ext.eq_ignore_ascii_case("ipynb") => path,
        // A dotted name that is not an extension ("run.v2") keeps its dot and
        // gains the real one, as every editor does.
        _ => {
            let mut s = path.into_os_string();
            s.push(".ipynb");
            std::path::PathBuf::from(s)
        }
    }
}

pub fn save_notebook(doc: &NotebookDocument, path: &Path) -> Result<(), String> {
    // Write the file back in the format it IS.
    //
    // This wrote nbformat JSON to whatever path it was given. Open
    // `analysis.py`, press Ctrl+S, and the script was replaced by a JSON
    // document — the same for a `.md`, which could be a document someone opened
    // only to read. The app has offered `*.py` and `*.md` in its Open dialog
    // the whole time, so the way to lose a file was to use a feature.
    match crate::helpers::notebook_formats::NotebookFormat::for_path(path) {
        crate::helpers::notebook_formats::NotebookFormat::PercentPython => {
            return write_atomically(
                path,
                &crate::helpers::notebook_formats::to_percent(doc),
                "py",
            );
        }
        crate::helpers::notebook_formats::NotebookFormat::Markdown => {
            return write_atomically(
                path,
                &crate::helpers::notebook_formats::to_markdown(doc),
                "md",
            );
        }
        crate::helpers::notebook_formats::NotebookFormat::PlainText => {
            return write_atomically(
                path,
                &crate::helpers::notebook_formats::to_plain_text(doc),
                "txt",
            );
        }
        // Never reached through the UI, which refuses to open these — but a
        // path can arrive from an MCP caller, and writing nbformat JSON over
        // someone's PDF is the failure this whole match exists to prevent.
        crate::helpers::notebook_formats::NotebookFormat::Unsupported => {
            return Err(
                crate::helpers::notebook_formats::NotebookFormat::refusal(path)
                    .unwrap_or_else(|| format!("cannot save to {}", path.display())),
            );
        }
        crate::helpers::notebook_formats::NotebookFormat::Ipynb => {}
    }

    // Stamp the kernel this app actually runs, unless the notebook already
    // names one. A notebook with no kernelspec opens in Jupyter as "select a
    // kernel" — true, but every notebook we write is a Python 3 notebook, and
    // saying so is the difference between a file that runs and a file that asks.
    let mut doc = doc.clone();
    if doc.metadata.kernelspec.is_none() {
        doc.metadata.kernelspec = Some(crate::models::notebook_document::KernelSpec {
            name: "python3".to_string(),
            display_name: "Python 3".to_string(),
            language: Some("python".to_string()),
        });
    }
    if doc.metadata.language_info.is_none() {
        doc.metadata.language_info = Some(crate::models::notebook_document::LanguageInfo {
            name: "python".to_string(),
            version: None,
            extra: Default::default(),
        });
    }
    let doc = &doc;

    // Ensure the parent directory exists.
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create directory {}: {}", parent.display(), e))?;
    }

    let json =
        serde_json::to_string_pretty(doc).map_err(|e| format!("serialisation error: {}", e))?;

    // Reindent: serde_json uses 2-space indent by default; Jupyter uses 1.
    // Rebuild the string with a 1-space indent rather than pulling in an
    // extra dependency.
    let json = reindent_json(&json);

    write_atomically(path, &json, "ipynb")
}

/// Write `contents` to `path` via a temporary file and a rename.
///
/// The rename is what makes it atomic: a reader either sees the old file or the
/// new one, never a half-written one. `extension` names the temporary so two
/// formats saving the same stem cannot collide.
fn write_atomically(path: &Path, contents: &str, extension: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create directory {}: {}", parent.display(), e))?;
    }
    let tmp_path = path.with_extension(format!("{extension}.tmp"));
    std::fs::write(&tmp_path, contents)
        .map_err(|e| format!("cannot write {}: {}", tmp_path.display(), e))?;
    std::fs::rename(&tmp_path, path).map_err(|e| {
        format!(
            "cannot rename {} to {}: {}",
            tmp_path.display(),
            path.display(),
            e
        )
    })?;
    Ok(())
}

/// Wrap a plain `.py` Python source file as a notebook with a single code cell.
///
/// This lets the UI open `.py` files in the notebook viewer without requiring
/// a real Jupyter kernel to be running.
pub fn load_python_as_notebook(path: &Path) -> Result<NotebookDocument, String> {
    let source = read_within_limit(path)?;

    // Split on `# %%`, the marker jupytext, VS Code, Spyder and PyCharm all
    // write. The whole file used to become ONE cell, so a script was a single
    // block that could only be run end to end — and its own cell structure,
    // sitting there in the comments, was ignored.
    let mut nb = NotebookDocument::create_empty();
    nb.cells = crate::helpers::notebook_formats::split_percent(&source);
    Ok(nb)
}

/// Wrap a `.md` Markdown file as a notebook with a single markdown cell.
pub fn load_markdown_as_notebook(path: &Path) -> Result<NotebookDocument, String> {
    let source = read_within_limit(path)?;

    // Fenced Python blocks become runnable cells and the prose around them
    // becomes markdown cells, so a written-up analysis can be executed where it
    // is read. The whole document used to arrive as one markdown cell.
    let mut nb = NotebookDocument::create_empty();
    nb.cells = crate::helpers::notebook_formats::split_markdown(&source);
    Ok(nb)
}

/// Read a `.txt` as a notebook: one markdown cell holding the text.
pub fn load_text_as_notebook(path: &Path) -> Result<NotebookDocument, String> {
    let source = read_within_limit(path)?;
    let mut nb = NotebookDocument::create_empty();
    nb.cells = crate::helpers::notebook_formats::split_plain_text(&source);
    Ok(nb)
}

/// Extract unique third-party module names from the `import` statements in all
/// code cells.
///
/// Both `import X` and `from X import Y` forms are recognised.  Standard
/// library modules (and IPython/ipywidgets builtins) are filtered out.
/// The returned list is sorted and deduplicated.
pub fn extract_imports(cells: &[NotebookCell]) -> Vec<String> {
    let mut found: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();

    for cell in cells {
        // Only scan code cells.
        if cell.cell_type != "code" {
            continue;
        }

        let source = cell.source.joined();
        for line in source.lines() {
            // Skip comment lines quickly.
            let trimmed = line.trim_start();
            if trimmed.starts_with('#') {
                continue;
            }

            let module = RE_IMPORT
                .captures(line)
                .map(|cap| cap[1].to_string())
                .or_else(|| RE_FROM.captures(line).map(|cap| cap[1].to_string()));

            if let Some(module) = module {
                // Top-level package only.
                let top = module.split('.').next().unwrap_or(&module).to_string();
                if !is_stdlib(&top) {
                    found.insert(top);
                }
            }
        }
    }

    found.into_iter().collect()
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Convert a 2-space-indented JSON string to a 1-space-indented one.
///
/// This is a simple line-by-line approach that counts leading spaces and
/// replaces each pair with a single space.  It avoids re-parsing the JSON.
fn reindent_json(json: &str) -> String {
    let mut out = String::with_capacity(json.len());
    for line in json.lines() {
        let leading = line.len() - line.trim_start().len();
        // Each indent level is 2 spaces in serde_json output.
        let level = leading / 2;
        for _ in 0..level {
            out.push(' ');
        }
        out.push_str(line.trim_start());
        out.push('\n');
    }
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::notebook_document::{CellOutput, CellSource, OutputData};
    use std::path::PathBuf;

    /// Write `content` to a uniquely-named temp file with the given `suffix`
    /// (e.g. `".ipynb"`).  Returns the `PathBuf` — the caller is responsible
    /// for cleanup, but the OS will reclaim the file on process exit.
    fn write_temp(content: &str, suffix: &str) -> PathBuf {
        let id = NotebookDocument::generate_cell_id();
        let path = std::env::temp_dir().join(format!("verbinal_test_{}{}", id, suffix));
        std::fs::write(&path, content).expect("write temp file");
        path
    }

    // ---------------------------------------------------------------------------
    // load_notebook
    // ---------------------------------------------------------------------------

    #[test]
    fn load_valid_notebook() {
        let json = r#"{
          "nbformat": 4,
          "nbformat_minor": 5,
          "metadata": {},
          "cells": [
            {"cell_type":"code","source":"x=1","outputs":[],"metadata":{}}
          ]
        }"#;
        let p = write_temp(json, ".ipynb");
        let nb = load_notebook(&p).expect("load");
        assert_eq!(nb.nbformat, 4);
        assert_eq!(nb.cells.len(), 1);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn load_notebook_assigns_missing_ids() {
        let json = r#"{
          "nbformat": 4,
          "nbformat_minor": 5,
          "metadata": {},
          "cells": [
            {"cell_type":"code","source":"x=1","outputs":[],"metadata":{}}
          ]
        }"#;
        let p = write_temp(json, ".ipynb");
        let nb = load_notebook(&p).expect("load");
        assert!(nb.cells[0].id.is_some());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn load_notebook_invalid_json() {
        let p = write_temp("not json at all", ".ipynb");
        assert!(load_notebook(&p).is_err());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn load_notebook_missing_file() {
        let result = load_notebook(Path::new("/tmp/this_does_not_exist_canfar_xyz.ipynb"));
        assert!(result.is_err());
    }

    #[test]
    fn load_notebook_caps_at_10000_cells() {
        let cell = r#"{"cell_type":"code","source":"x=1","outputs":[],"metadata":{}}"#;
        let cells_json: String = (0..10_001).map(|_| cell).collect::<Vec<_>>().join(",");
        let json = format!(
            r#"{{"nbformat":4,"nbformat_minor":5,"metadata":{{}},"cells":[{}]}}"#,
            cells_json
        );
        let p = write_temp(&json, ".ipynb");
        let nb = load_notebook(&p).expect("load");
        assert_eq!(nb.cells.len(), 10_000);
        let _ = std::fs::remove_file(&p);
    }

    // ---------------------------------------------------------------------------
    // save_notebook / round-trip
    // ---------------------------------------------------------------------------

    #[test]
    fn save_and_reload_round_trip() {
        let nb = NotebookDocument::create_empty();
        let id = NotebookDocument::generate_cell_id();
        let path = std::env::temp_dir().join(format!("verbinal_rt_{}.ipynb", id));
        save_notebook(&nb, &path).expect("save");
        let back = load_notebook(&path).expect("reload");
        assert_eq!(back.nbformat, 4);
        assert_eq!(back.cells.len(), 1);
        let _ = std::fs::remove_file(&path);
    }

    /// A file is saved in the format it IS, not always as nbformat JSON.
    ///
    /// This wrote JSON to whatever path it was handed. The Open dialog offers
    /// `*.py` and `*.md`, so opening a script and pressing Ctrl+S replaced it
    /// with a JSON document — the way to destroy a file was to use a feature.
    #[test]
    fn saving_does_not_turn_a_script_into_json() {
        let source = "# %% [markdown]\n# Notes\n\n# %%\nimport numpy as np\n";
        let path = write_temp(source, ".py");
        let doc = load_python_as_notebook(&path).expect("load");
        assert_eq!(doc.cells.len(), 2, "the `# %%` markers were not read");

        save_notebook(&doc, &path).expect("save");
        let written = std::fs::read_to_string(&path).expect("re-read");
        assert!(
            !written.trim_start().starts_with('{'),
            "the script was replaced by JSON: {written}"
        );
        assert!(written.contains("import numpy as np"), "{written}");
        assert!(
            written.contains("# %%"),
            "cell markers were lost: {written}"
        );
        // And it still loads as the same notebook.
        assert_eq!(
            load_python_as_notebook(&path).expect("reload").cells.len(),
            2
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn saving_does_not_turn_a_markdown_document_into_json() {
        let source = "# Title\n\nProse.\n\n```python\nimport numpy\n```\n";
        let path = write_temp(source, ".md");
        let doc = load_markdown_as_notebook(&path).expect("load");
        assert_eq!(doc.cells.len(), 2, "the fenced block was not read as code");

        save_notebook(&doc, &path).expect("save");
        let written = std::fs::read_to_string(&path).expect("re-read");
        assert!(
            !written.trim_start().starts_with('{'),
            "the document was replaced by JSON: {written}"
        );
        assert!(written.contains("# Title"), "{written}");
        assert!(
            written.contains("```python"),
            "the code fence was lost: {written}"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// A file over the limit is refused, and the message names the remedy.
    #[test]
    fn a_file_over_the_limit_is_refused_by_name_and_size() {
        let path = std::path::Path::new("/tmp/catalogue.txt");
        // 64 MB limit, 100 MB file.
        let refusal = too_large(path, 100 * 1024 * 1024, 64).expect("should refuse");
        assert!(refusal.contains("catalogue.txt"), "{refusal}");
        assert!(refusal.contains("100.0 MB"), "{refusal}");
        assert!(refusal.contains("64 MB"), "{refusal}");
        // The remedy is a setting; saying only "too large" leaves nowhere to go.
        assert!(refusal.contains("Largest file to open"), "{refusal}");
    }

    #[test]
    fn a_file_within_the_limit_is_not_refused() {
        let path = std::path::Path::new("/tmp/notes.txt");
        assert!(too_large(path, 1024, 64).is_none());
        // Exactly at the limit is allowed — the rule is "over", not "at".
        assert!(too_large(path, 64 * 1024 * 1024, 64).is_none());
        assert!(too_large(path, 64 * 1024 * 1024 + 1, 64).is_some());
    }

    /// The limit is applied to a real read, not only computed.
    #[test]
    fn the_limit_is_actually_enforced_when_reading() {
        let path = write_temp(&"x".repeat(3 * 1024 * 1024), ".txt");
        // 1 MB limit against a 3 MB file.
        assert!(
            read_within(&path, 1).is_err(),
            "a 3 MB file passed a 1 MB limit"
        );
        // ...and the same file is fine under a larger one.
        assert!(read_within(&path, 8).is_ok());
        let _ = std::fs::remove_file(&path);
    }

    /// A `.txt` is written back as text, not as JSON.
    #[test]
    fn saving_does_not_turn_notes_into_json() {
        let source = "Observing notes\n\nTarget M31, seeing 0.8\n";
        let path = write_temp(source, ".txt");
        let doc = load_text_as_notebook(&path).expect("load");
        assert_eq!(doc.cells.len(), 1);

        save_notebook(&doc, &path).expect("save");
        let written = std::fs::read_to_string(&path).expect("re-read");
        assert!(
            !written.trim_start().starts_with('{'),
            "the notes were replaced by JSON: {written}"
        );
        assert_eq!(written, source, "an unchanged file should be unchanged");
        let _ = std::fs::remove_file(&path);
    }

    /// Saving to a format we refuse to OPEN must not overwrite the file.
    ///
    /// The UI cannot reach this — it will not open a `.pdf` — but a path can
    /// arrive from an MCP caller, and writing nbformat JSON over someone's
    /// paper is precisely the failure the format dispatch exists to prevent.
    /// The arm was written and never exercised, which is how the original
    /// version of this bug shipped.
    #[test]
    fn saving_refuses_to_overwrite_a_document_it_cannot_open() {
        let original = "%PDF-1.7 not really, but not a notebook either\n";
        let path = write_temp(original, ".pdf");
        let doc = NotebookDocument::create_empty();

        let err = save_notebook(&doc, &path).expect_err("saving to a .pdf must fail");
        assert!(
            err.contains(".pdf") || err.contains("not a notebook"),
            "{err}"
        );
        assert_eq!(
            std::fs::read_to_string(&path).expect("re-read"),
            original,
            "the file was modified despite the refusal"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// An `.ipynb` is still nbformat JSON — the change must not go the other way.
    #[test]
    fn a_notebook_is_still_saved_as_json() {
        let path = write_temp("", ".ipynb");
        let doc = NotebookDocument::create_empty();
        save_notebook(&doc, &path).expect("save");
        let written = std::fs::read_to_string(&path).expect("re-read");
        assert!(
            written.trim_start().starts_with('{'),
            "an .ipynb stopped being JSON: {written}"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn save_notebook_atomic_creates_file() {
        let id = NotebookDocument::generate_cell_id();
        let path = std::env::temp_dir().join(format!("verbinal_atomic_{}.ipynb", id));
        let nb = NotebookDocument::create_empty();
        save_notebook(&nb, &path).expect("save");
        assert!(path.exists());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn save_notebook_no_tmp_leftover() {
        let id = NotebookDocument::generate_cell_id();
        let path = std::env::temp_dir().join(format!("verbinal_notmp_{}.ipynb", id));
        let nb = NotebookDocument::create_empty();
        save_notebook(&nb, &path).expect("save");
        assert!(!path.with_extension("ipynb.tmp").exists());
        let _ = std::fs::remove_file(&path);
    }

    // ---------------------------------------------------------------------------
    // load_python_as_notebook / load_markdown_as_notebook
    // ---------------------------------------------------------------------------

    #[test]
    fn load_python_as_notebook_creates_code_cell() {
        let p = write_temp("import numpy as np\nprint('hello')", ".py");
        let nb = load_python_as_notebook(&p).expect("load");
        assert_eq!(nb.cells.len(), 1);
        assert_eq!(nb.cells[0].cell_type, "code");
        assert!(nb.cells[0].source.joined().contains("numpy"));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn load_markdown_as_notebook_creates_markdown_cell() {
        let p = write_temp("# Hello\nWorld", ".md");
        let nb = load_markdown_as_notebook(&p).expect("load");
        assert_eq!(nb.cells.len(), 1);
        assert_eq!(nb.cells[0].cell_type, "markdown");
        assert!(nb.cells[0].source.joined().contains("Hello"));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn load_python_missing_file() {
        assert!(load_python_as_notebook(Path::new("/no/such/file_canfar.py")).is_err());
    }

    // ---------------------------------------------------------------------------
    // extract_imports
    // ---------------------------------------------------------------------------

    #[test]
    fn extract_imports_basic() {
        let cells = vec![NotebookCell {
            cell_type: "code".to_string(),
            source: CellSource::Single(
                "import numpy as np\nimport pandas\nfrom scipy import stats".to_string(),
            ),
            outputs: vec![],
            execution_count: None,
            id: None,
            metadata: serde_json::Map::new(),
        }];
        let imports = extract_imports(&cells);
        assert!(imports.contains(&"numpy".to_string()));
        assert!(imports.contains(&"pandas".to_string()));
        assert!(imports.contains(&"scipy".to_string()));
    }

    #[test]
    fn extract_imports_filters_stdlib() {
        let cells = vec![NotebookCell {
            cell_type: "code".to_string(),
            source: CellSource::Single(
                "import os\nimport sys\nimport numpy\nfrom math import pi".to_string(),
            ),
            outputs: vec![],
            execution_count: None,
            id: None,
            metadata: serde_json::Map::new(),
        }];
        let imports = extract_imports(&cells);
        // stdlib modules must be absent.
        assert!(!imports.contains(&"os".to_string()));
        assert!(!imports.contains(&"sys".to_string()));
        assert!(!imports.contains(&"math".to_string()));
        // third-party must be present.
        assert!(imports.contains(&"numpy".to_string()));
    }

    #[test]
    fn extract_imports_skips_markdown_cells() {
        let cells = vec![NotebookCell {
            cell_type: "markdown".to_string(),
            source: CellSource::Single("import requests".to_string()),
            outputs: vec![],
            execution_count: None,
            id: None,
            metadata: serde_json::Map::new(),
        }];
        let imports = extract_imports(&cells);
        assert!(imports.is_empty());
    }

    #[test]
    fn extract_imports_deduplicates() {
        let cells = vec![
            NotebookCell {
                cell_type: "code".to_string(),
                source: CellSource::Single("import numpy".to_string()),
                outputs: vec![],
                execution_count: None,
                id: None,
                metadata: serde_json::Map::new(),
            },
            NotebookCell {
                cell_type: "code".to_string(),
                source: CellSource::Single("import numpy as np".to_string()),
                outputs: vec![],
                execution_count: None,
                id: None,
                metadata: serde_json::Map::new(),
            },
        ];
        let imports = extract_imports(&cells);
        assert_eq!(imports.iter().filter(|m| m.as_str() == "numpy").count(), 1);
    }

    #[test]
    fn extract_imports_submodule_normalised() {
        // `from astropy.io import fits` should register just `astropy`.
        let cells = vec![NotebookCell {
            cell_type: "code".to_string(),
            source: CellSource::Single("from astropy.io import fits".to_string()),
            outputs: vec![],
            execution_count: None,
            id: None,
            metadata: serde_json::Map::new(),
        }];
        let imports = extract_imports(&cells);
        assert!(imports.contains(&"astropy".to_string()));
        assert!(!imports.contains(&"astropy.io".to_string()));
    }

    #[test]
    fn extract_imports_skips_comment_lines() {
        let cells = vec![NotebookCell {
            cell_type: "code".to_string(),
            source: CellSource::Single("# import requests\nimport matplotlib".to_string()),
            outputs: vec![],
            execution_count: None,
            id: None,
            metadata: serde_json::Map::new(),
        }];
        let imports = extract_imports(&cells);
        assert!(!imports.contains(&"requests".to_string()));
        assert!(imports.contains(&"matplotlib".to_string()));
    }

    #[test]
    fn extract_imports_sorted() {
        let cells = vec![NotebookCell {
            cell_type: "code".to_string(),
            source: CellSource::Single(
                "import scipy\nimport astropy\nimport matplotlib".to_string(),
            ),
            outputs: vec![],
            execution_count: None,
            id: None,
            metadata: serde_json::Map::new(),
        }];
        let imports = extract_imports(&cells);
        let mut sorted = imports.clone();
        sorted.sort();
        assert_eq!(
            imports, sorted,
            "extract_imports must return sorted results"
        );
    }

    // ---------------------------------------------------------------------------
    // reindent_json
    // ---------------------------------------------------------------------------

    #[test]
    fn reindent_json_reduces_indent() {
        let two_space = "{\n  \"key\": [\n    \"val\"\n  ]\n}";
        let one_space = reindent_json(two_space);
        assert!(one_space.contains(" \"key\""));
        assert!(!one_space.contains("   \"key\"")); // not 3 spaces
    }

    // ---------------------------------------------------------------------------
    // Full notebook save/load with outputs
    // ---------------------------------------------------------------------------

    #[test]
    fn save_load_notebook_with_outputs() {
        let mut nb = NotebookDocument::create_empty();
        nb.cells[0].outputs.push(CellOutput::ExecuteResult {
            execution_count: Some(1),
            data: OutputData {
                text_plain: Some(CellSource::Single("42".to_string())),
                ..Default::default()
            },
            metadata: serde_json::Map::new(),
        });
        let id = NotebookDocument::generate_cell_id();
        let path = std::env::temp_dir().join(format!("verbinal_out_{}.ipynb", id));
        save_notebook(&nb, &path).expect("save");
        let back = load_notebook(&path).expect("load");
        assert_eq!(back.cells[0].outputs.len(), 1);
        let _ = std::fs::remove_file(&path);
    }
}

#[cfg(test)]
mod nbformat_conformance {
    //! What we write has to be a notebook Jupyter will open.
    //!
    //! Three things it was not, all invisible from inside the app because our
    //! own reader is forgiving of them:
    //!
    //! * `"kernelspec": null` / `"language_info": null` — the schema types both
    //!   as objects, so a validator rejects the file and Jupyter has no kernel
    //!   to offer.
    //! * `"outputs": []` and `"execution_count": null` on every MARKDOWN cell —
    //!   the 4.5 schema sets `additionalProperties: false` on markdown and raw
    //!   cells, so those two keys make the file invalid.
    //! * No kernelspec at all, so a notebook this app wrote opened elsewhere as
    //!   "select a kernel" when every one of them is Python 3.
    use super::*;
    use crate::models::notebook_document::{CellSource, NotebookCell, NotebookDocument};
    use serde_json::Value;

    fn write_and_read(doc: &NotebookDocument) -> Value {
        let path = std::env::temp_dir().join(format!(
            "verbinal_nbfmt_{}.ipynb",
            NotebookDocument::generate_cell_id()
        ));
        save_notebook(doc, &path).expect("save");
        let text = std::fs::read_to_string(&path).expect("read back");
        let _ = std::fs::remove_file(&path);
        serde_json::from_str(&text).expect("what we wrote is JSON")
    }

    fn markdown(text: &str) -> NotebookCell {
        NotebookCell {
            cell_type: "markdown".to_string(),
            source: CellSource::Single(text.to_string()),
            outputs: Vec::new(),
            execution_count: None,
            id: Some(NotebookDocument::generate_cell_id()),
            metadata: serde_json::Map::new(),
        }
    }

    #[test]
    fn a_markdown_cell_carries_no_code_cell_keys() {
        let mut doc = NotebookDocument::create_empty();
        doc.cells.push(markdown("# Title"));
        let json = write_and_read(&doc);
        let cell = &json["cells"][1];
        let keys: Vec<&str> = cell
            .as_object()
            .unwrap()
            .keys()
            .map(|s| s.as_str())
            .collect();
        for forbidden in ["outputs", "execution_count"] {
            assert!(
                !keys.contains(&forbidden),
                "a markdown cell carries `{forbidden}`, which the 4.5 schema forbids: {keys:?}"
            );
        }
        for required in ["cell_type", "metadata", "source"] {
            assert!(keys.contains(&required), "{required} missing: {keys:?}");
        }
    }

    #[test]
    fn a_code_cell_keeps_the_keys_the_schema_requires() {
        // Both are REQUIRED for a code cell, and `execution_count` is null until
        // it runs — the one null in the format that is correct.
        let doc = NotebookDocument::create_empty();
        let json = write_and_read(&doc);
        let cell = &json["cells"][0];
        assert!(cell.get("outputs").is_some_and(|v| v.is_array()));
        assert!(cell.get("execution_count").is_some(), "must be present");
        assert!(cell["execution_count"].is_null(), "null until it has run");
        assert!(cell.get("id").is_some(), "4.5 requires a cell id");
    }

    #[test]
    fn the_metadata_names_the_kernel_and_holds_no_nulls() {
        let doc = NotebookDocument::create_empty();
        let json = write_and_read(&doc);
        let md = &json["metadata"];
        assert_eq!(md["kernelspec"]["name"], "python3", "{md}");
        assert_eq!(md["language_info"]["name"], "python", "{md}");
        // A null where the schema wants an object or a string is what made the
        // file invalid in the first place.
        fn nulls(v: &Value, path: String, out: &mut Vec<String>) {
            match v {
                Value::Null => out.push(path),
                Value::Object(m) => {
                    for (k, v) in m {
                        nulls(v, format!("{path}.{k}"), out);
                    }
                }
                Value::Array(a) => {
                    for (i, v) in a.iter().enumerate() {
                        nulls(v, format!("{path}[{i}]"), out);
                    }
                }
                _ => {}
            }
        }
        let mut found = Vec::new();
        nulls(md, "metadata".into(), &mut found);
        assert!(found.is_empty(), "nulls in metadata: {found:?}");
    }

    #[test]
    fn a_notebook_this_app_wrote_is_one_it_can_read_back() {
        let mut doc = NotebookDocument::create_empty();
        doc.cells.push(markdown("## Notes"));
        doc.cells[0].source = CellSource::Single("print(1)".into());
        let json = write_and_read(&doc);
        let reread: NotebookDocument =
            serde_json::from_value(json).expect("our own reader accepts our own file");
        assert_eq!(reread.cells.len(), 2);
        assert_eq!(reread.cells[0].source.joined(), "print(1)");
        assert_eq!(reread.cells[1].cell_type, "markdown");
        assert_eq!(reread.nbformat, 4);
        assert_eq!(reread.nbformat_minor, 5);
    }
}

#[cfg(test)]
mod extension_tests {
    use super::with_ipynb_extension;
    use std::path::PathBuf;

    #[test]
    fn a_name_without_an_extension_gains_one() {
        // A chooser returns exactly what was typed. "analysis" is a notebook
        // nothing will offer to open — not our Open dialog, not Jupyter's.
        assert_eq!(
            with_ipynb_extension(PathBuf::from("/tmp/analysis")),
            PathBuf::from("/tmp/analysis.ipynb")
        );
    }

    #[test]
    fn a_name_that_already_has_one_is_left_alone() {
        assert_eq!(
            with_ipynb_extension(PathBuf::from("/tmp/analysis.ipynb")),
            PathBuf::from("/tmp/analysis.ipynb")
        );
        // Case is the file system's business, not ours to rewrite.
        assert_eq!(
            with_ipynb_extension(PathBuf::from("/tmp/A.IPYNB")),
            PathBuf::from("/tmp/A.IPYNB")
        );
    }

    #[test]
    fn a_dotted_name_keeps_its_dot_and_gains_the_real_extension() {
        assert_eq!(
            with_ipynb_extension(PathBuf::from("/tmp/run.v2")),
            PathBuf::from("/tmp/run.v2.ipynb")
        );
    }
}
