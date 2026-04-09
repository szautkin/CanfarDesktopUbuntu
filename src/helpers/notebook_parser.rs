use crate::models::notebook_document::{CellSource, NotebookCell, NotebookDocument};
use once_cell::sync::Lazy;
use regex::Regex;
use std::path::Path;

/// Matches `import <module>` at the start of a line.
static RE_IMPORT: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^\s*import\s+([\w.]+)").unwrap());

/// Matches `from <module> import ...` at the start of a line.
static RE_FROM: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^\s*from\s+([\w.]+)\s+import\s+").unwrap());

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
pub fn load_notebook(path: &Path) -> Result<NotebookDocument, String> {
    let contents = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {}", path.display(), e))?;

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
pub fn save_notebook(doc: &NotebookDocument, path: &Path) -> Result<(), String> {
    // Ensure the parent directory exists.
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create directory {}: {}", parent.display(), e))?;
    }

    let json = serde_json::to_string_pretty(doc)
        .map_err(|e| format!("serialisation error: {}", e))?;

    // Reindent: serde_json uses 2-space indent by default; Jupyter uses 1.
    // Rebuild the string with a 1-space indent rather than pulling in an
    // extra dependency.
    let json = reindent_json(&json);

    // Atomic write: write to <path>.tmp, then rename.
    let tmp_path = path.with_extension("ipynb.tmp");
    std::fs::write(&tmp_path, &json)
        .map_err(|e| format!("cannot write {}: {}", tmp_path.display(), e))?;
    std::fs::rename(&tmp_path, path)
        .map_err(|e| format!("cannot rename {} to {}: {}", tmp_path.display(), path.display(), e))?;

    Ok(())
}

/// Wrap a plain `.py` Python source file as a notebook with a single code cell.
///
/// This lets the UI open `.py` files in the notebook viewer without requiring
/// a real Jupyter kernel to be running.
pub fn load_python_as_notebook(path: &Path) -> Result<NotebookDocument, String> {
    let source = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {}", path.display(), e))?;

    let mut nb = NotebookDocument::create_empty();
    nb.cells[0].source = CellSource::Single(source);
    Ok(nb)
}

/// Wrap a `.md` Markdown file as a notebook with a single markdown cell.
pub fn load_markdown_as_notebook(path: &Path) -> Result<NotebookDocument, String> {
    let source = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {}", path.display(), e))?;

    let mut nb = NotebookDocument::create_empty();
    nb.cells[0].cell_type = "markdown".to_string();
    nb.cells[0].source = CellSource::Single(source);
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
    use crate::models::notebook_document::{CellOutput, OutputData};
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
        let cells_json: String = (0..10_001)
            .map(|_| cell)
            .collect::<Vec<_>>()
            .join(",");
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
            source: CellSource::Single(
                "# import requests\nimport matplotlib".to_string(),
            ),
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
        assert_eq!(imports, sorted, "extract_imports must return sorted results");
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
