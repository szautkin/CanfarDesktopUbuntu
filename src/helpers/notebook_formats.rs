//! The three files this app opens as a notebook, and how they map to cells.
//!
//! `.ipynb` is the native format and always was. `.py` and `.md` could be
//! opened too — and each arrived as ONE cell holding the whole file, so a
//! 500-line script was a single block you could only run all at once. That is
//! not what any other tool does, and it is not what the file means.
//!
//! Two problems, and the second is worse than the first.
//!
//! **Cells were never found.** Every editor that runs Python interactively —
//! Jupyter via jupytext, VS Code, Spyder, PyCharm — splits a `.py` on `# %%`
//! marker lines. The convention is twenty years old between them and it is what
//! a scientist's script already contains. Reading it costs a scan of the lines.
//!
//! **Saving destroyed the file.** `save_notebook` writes nbformat JSON to
//! whatever path it is handed. Open `analysis.py`, press Ctrl+S, and the script
//! is replaced by a JSON document. Same for a `.md`. The file could be one you
//! opened to read.
//!
//! So format is a value here, decided once from the path, and each format can
//! both read cells and write them back. Everything is a pure function over
//! strings: no filesystem, no GTK, and every case below is testable.

use crate::models::notebook_document::{NotebookCell, NotebookDocument};

/// A file this app can open as a notebook.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotebookFormat {
    /// nbformat JSON — the native format.
    Ipynb,
    /// A Python script with `# %%` cell markers (jupytext "percent").
    PercentPython,
    /// Markdown, where fenced code blocks are code cells.
    Markdown,
    /// Plain text: one cell, written back as it came.
    ///
    /// Not a notebook format anywhere, and it does not pretend to be. It is
    /// here because observing notes, instrument logs and READMEs live next to
    /// the data as `.txt`, and being able to open one and add a code cell under
    /// it is the whole point of a notebook. Nothing is parsed out of it.
    PlainText,
    /// A file this app will not open as a notebook.
    ///
    /// Named rather than left to fail as malformed JSON, so the refusal can say
    /// what the file is and what to do with it instead.
    Unsupported,
}

impl NotebookFormat {
    /// The format of the file at `path`, from its extension.
    ///
    /// Anything unrecognised is `Ipynb`: that is what the loader has always
    /// assumed, and a file with no extension that a user saved from here is far
    /// more likely to be a notebook than a script.
    pub fn for_path(path: &std::path::Path) -> Self {
        match path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("py") => Self::PercentPython,
            Some("md" | "markdown") => Self::Markdown,
            Some("txt" | "text" | "log") => Self::PlainText,
            // Export formats and documents, not notebooks. `.html` in
            // particular is what a notebook is converted TO — the conversion is
            // one way, and no tool reads it back. Refusing by name lets the
            // message say so; falling through to `Ipynb` produced "invalid
            // notebook JSON", which describes the parser's disappointment
            // rather than the user's problem.
            Some("html" | "htm" | "pdf" | "docx" | "odt" | "rtf" | "tex") => Self::Unsupported,
            _ => Self::Ipynb,
        }
    }

    /// The word `list_notebooks` reports for this format.
    ///
    /// Derived from the format rather than from a second reading of the
    /// extension. The MCP layer had its own copy of that mapping and it had
    /// ALREADY drifted within a day: `.txt` became openable and the copy still
    /// answered `"other"` for it, so the tool reported a file as unsupported
    /// while the editor opened it happily.
    pub fn kind(self) -> &'static str {
        match self {
            Self::Ipynb => "notebook",
            Self::PercentPython => "python",
            Self::Markdown => "markdown",
            Self::PlainText => "text",
            Self::Unsupported => "other",
        }
    }

    /// Glob patterns for every openable format, for a file chooser.
    ///
    /// The Open dialog listed its own patterns, which is how `.txt` came to be
    /// openable by path but not offered in the dialog that exists to find it.
    pub fn open_patterns() -> &'static [&'static str] {
        &["*.ipynb", "*.py", "*.md", "*.markdown", "*.txt", "*.log"]
    }

    /// Why this file cannot be opened, and what to do instead.
    pub fn refusal(path: &std::path::Path) -> Option<String> {
        if Self::for_path(path) != Self::Unsupported {
            return None;
        }
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let name = path.display();
        Some(match ext.as_str() {
            "html" | "htm" => format!(
                "{name} is an exported document, not a notebook. Converting a notebook to \
                 HTML is one way — the cells, their outputs and their order are not recoverable \
                 from it. Open the .ipynb it was made from."
            ),
            other => format!(
                "{name} is a .{other} document, not a notebook. This editor opens .ipynb \
                 notebooks, .py scripts (split on `# %%`), .md documents (fenced python \
                 becomes code cells) and .txt notes."
            ),
        })
    }
}

/// Whether `line` opens a new percent cell, and what kind it declares.
///
/// `# %%`, `#%%`, `# %% [markdown]`, `# %% [raw]`, and the form carrying a
/// title — `# %% Load the data` — which jupytext writes and which people write
/// by hand more often than the bare marker.
fn percent_marker(line: &str) -> Option<&'static str> {
    let trimmed = line.trim_start();
    let rest = trimmed
        .strip_prefix("# %%")
        .or_else(|| trimmed.strip_prefix("#%%"))?;
    // `# %%%` is not a marker; the character after must end it or separate it.
    if rest.starts_with('%') {
        return None;
    }
    let lowered = rest.to_ascii_lowercase();
    if lowered.contains("[markdown]") {
        Some("markdown")
    } else if lowered.contains("[raw]") {
        Some("raw")
    } else {
        Some("code")
    }
}

/// Strip the comment prefix jupytext puts on every line of a markdown cell.
fn uncomment(text: &str) -> String {
    text.lines()
        .map(|l| {
            l.strip_prefix("# ")
                .or_else(|| l.strip_prefix('#'))
                .unwrap_or(l)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Comment out a markdown cell's text so the file stays valid Python.
fn comment(text: &str) -> String {
    text.lines()
        .map(|l| {
            if l.is_empty() {
                "#".to_string()
            } else {
                format!("# {l}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Split a percent-format Python file into cells.
///
/// A file with no markers is one code cell — which is both the old behaviour
/// and the right answer for an ordinary script.
pub fn split_percent(source: &str) -> Vec<NotebookCell> {
    let normalised = source.replace("\r\n", "\n").replace('\r', "\n");
    let mut cells: Vec<NotebookCell> = Vec::new();
    let mut kind = "code";
    let mut buffer: Vec<&str> = Vec::new();

    let push = |kind: &str, buffer: &mut Vec<&str>, cells: &mut Vec<NotebookCell>| {
        let text = buffer.join("\n");
        buffer.clear();
        if text.trim().is_empty() {
            return;
        }
        let source = if kind == "markdown" {
            uncomment(text.trim_matches('\n'))
        } else {
            text.trim_matches('\n').to_string()
        };
        cells.push(NotebookCell::with_source(kind, source));
    };

    for line in normalised.lines() {
        if let Some(next_kind) = percent_marker(line) {
            push(kind, &mut buffer, &mut cells);
            kind = next_kind;
            continue;
        }
        buffer.push(line);
    }
    push(kind, &mut buffer, &mut cells);

    if cells.is_empty() {
        cells.push(NotebookCell::new("code"));
    }
    cells
}

/// Write a document back as a percent-format Python file.
pub fn to_percent(doc: &NotebookDocument) -> String {
    let mut out = String::new();
    for cell in &doc.cells {
        let body = cell.source.joined();
        match cell.cell_type.as_str() {
            "markdown" => {
                out.push_str("# %% [markdown]\n");
                out.push_str(&comment(&body));
            }
            "raw" => {
                out.push_str("# %% [raw]\n");
                out.push_str(&comment(&body));
            }
            _ => {
                out.push_str("# %%\n");
                out.push_str(&body);
            }
        }
        out.push_str("\n\n");
    }
    end_with_one_newline(out)
}

/// Read a plain-text file as one markdown cell.
///
/// Deliberately no parsing. A `.txt` has no cell convention to find, and
/// inventing one — splitting on blank lines, say — would take a file someone
/// wrote as prose and cut it into pieces they did not ask for.
pub fn split_plain_text(source: &str) -> Vec<NotebookCell> {
    let text = source.replace("\r\n", "\n").replace('\r', "\n");
    if text.trim().is_empty() {
        return vec![NotebookCell::new("markdown")];
    }
    vec![NotebookCell::with_source(
        "markdown",
        text.trim_end_matches('\n').to_string(),
    )]
}

/// Write a document back as plain text.
///
/// Code cells keep a `# %%` marker so a notes file that grew some analysis can
/// be reopened with its cells intact, and so the code is visibly code rather
/// than silently run together with the prose.
pub fn to_plain_text(doc: &NotebookDocument) -> String {
    let mut out = String::new();
    for cell in &doc.cells {
        let body = cell.source.joined();
        if body.trim().is_empty() {
            continue;
        }
        if cell.cell_type == "code" {
            out.push_str("# %%\n");
        }
        out.push_str(body.trim_end());
        out.push_str("\n\n");
    }
    end_with_one_newline(out)
}

/// The language tags on a fenced block that mean "this is a code cell".
///
/// The kernel is Python, so only Python blocks become runnable cells. A shell
/// or JSON block in a document is illustration, and turning it into a code cell
/// would offer to run text that was never meant to be run.
const CODE_FENCE_LANGUAGES: &[&str] = &["python", "python3", "py", "ipython", "ipython3"];

/// Split a Markdown file into markdown cells and fenced Python code cells.
pub fn split_markdown(source: &str) -> Vec<NotebookCell> {
    let normalised = source.replace("\r\n", "\n").replace('\r', "\n");
    let mut cells: Vec<NotebookCell> = Vec::new();
    let mut prose: Vec<&str> = Vec::new();
    let mut code: Vec<&str> = Vec::new();
    let mut in_code = false;
    // Remembered so the closing fence must match the opening one; a nested
    // ``` inside a ~~~~ block does not end it.
    let mut fence = String::new();

    fn flush(kind: &str, buffer: &mut Vec<&str>, cells: &mut Vec<NotebookCell>) {
        let text = buffer.join("\n");
        buffer.clear();
        if text.trim().is_empty() {
            return;
        }
        cells.push(NotebookCell::with_source(
            kind,
            text.trim_matches('\n').to_string(),
        ));
    }

    for line in normalised.lines() {
        let trimmed = line.trim_start();
        if !in_code {
            if let Some(info) = trimmed
                .strip_prefix("```")
                .filter(|_| trimmed.starts_with("```"))
            {
                let language = info.trim().to_ascii_lowercase();
                let language = language.split_whitespace().next().unwrap_or("");
                if CODE_FENCE_LANGUAGES.contains(&language) {
                    flush("markdown", &mut prose, &mut cells);
                    in_code = true;
                    fence = "```".to_string();
                    continue;
                }
            }
            prose.push(line);
        } else {
            if trimmed.starts_with(&fence) && trimmed.trim_end() == fence {
                flush("code", &mut code, &mut cells);
                in_code = false;
                continue;
            }
            code.push(line);
        }
    }
    // An unterminated fence: keep the text rather than dropping the tail.
    if in_code {
        flush("code", &mut code, &mut cells);
    }
    flush("markdown", &mut prose, &mut cells);

    if cells.is_empty() {
        cells.push(NotebookCell::new("markdown"));
    }
    cells
}

/// Write a document back as Markdown, code cells as fenced Python blocks.
pub fn to_markdown(doc: &NotebookDocument) -> String {
    let mut out = String::new();
    for cell in &doc.cells {
        let body = cell.source.joined();
        if body.trim().is_empty() {
            continue;
        }
        match cell.cell_type.as_str() {
            "code" => {
                out.push_str("```python\n");
                out.push_str(body.trim_end());
                out.push_str("\n```\n\n");
            }
            _ => {
                out.push_str(body.trim_end());
                out.push_str("\n\n");
            }
        }
    }
    end_with_one_newline(out)
}

/// Exactly one trailing newline.
///
/// Writing a blank line at the end grew the file by one line on every save.
/// These are files people keep in git, and a diff that appears whenever the
/// notebook is opened and closed is a diff that trains people to ignore diffs.
fn end_with_one_newline(mut text: String) -> String {
    while text.ends_with('\n') || text.ends_with('\r') {
        text.pop();
    }
    text.push('\n');
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(cells: &[NotebookCell]) -> Vec<&str> {
        cells.iter().map(|c| c.cell_type.as_str()).collect()
    }
    fn sources(cells: &[NotebookCell]) -> Vec<String> {
        cells.iter().map(|c| c.source.joined()).collect()
    }

    #[test]
    fn the_format_comes_from_the_extension() {
        use std::path::Path;
        assert_eq!(
            NotebookFormat::for_path(Path::new("a.ipynb")),
            NotebookFormat::Ipynb
        );
        assert_eq!(
            NotebookFormat::for_path(Path::new("a.py")),
            NotebookFormat::PercentPython
        );
        assert_eq!(
            NotebookFormat::for_path(Path::new("a.md")),
            NotebookFormat::Markdown
        );
        assert_eq!(
            NotebookFormat::for_path(Path::new("A.MD")),
            NotebookFormat::Markdown
        );
        // No extension: treated as a notebook, which is what the loader has
        // always assumed and what a file saved from here actually is.
        assert_eq!(
            NotebookFormat::for_path(Path::new("untitled")),
            NotebookFormat::Ipynb
        );
    }

    /// The percent format, as VS Code, Spyder, PyCharm and jupytext write it.
    #[test]
    fn a_percent_script_splits_into_its_cells() {
        let src = "# %% [markdown]\n\
                   # A heading\n\
                   # and prose.\n\
                   \n\
                   # %%\n\
                   import numpy as np\n\
                   print(np.pi)\n\
                   \n\
                   # %%\n\
                   2 + 2\n";
        let cells = split_percent(src);
        assert_eq!(kinds(&cells), ["markdown", "code", "code"]);
        // The comment prefix is jupytext's encoding, not the author's text.
        assert_eq!(sources(&cells)[0], "A heading\nand prose.");
        assert!(sources(&cells)[1].contains("import numpy as np"));
        assert_eq!(sources(&cells)[2], "2 + 2");
    }

    /// An ordinary script has no markers, and is one cell — as before.
    #[test]
    fn a_plain_script_is_still_a_single_cell() {
        let cells = split_percent("import sys\nprint(sys.version)\n");
        assert_eq!(kinds(&cells), ["code"]);
        assert_eq!(sources(&cells)[0], "import sys\nprint(sys.version)");
    }

    /// Code above the first marker belongs to a cell of its own.
    ///
    /// Imports at the top of a file, before anyone thought to write a marker.
    #[test]
    fn a_preamble_before_the_first_marker_is_kept() {
        let cells = split_percent("import os\n\n# %%\nprint(os.name)\n");
        assert_eq!(kinds(&cells), ["code", "code"]);
        assert_eq!(sources(&cells)[0], "import os");
        assert_eq!(sources(&cells)[1], "print(os.name)");
    }

    #[test]
    fn the_marker_is_recognised_in_the_forms_people_write_it() {
        // No space, a title after it, and the raw kind.
        let cells =
            split_percent("#%%\na = 1\n\n# %% Load the data\nb = 2\n\n# %% [raw]\n# note\n");
        assert_eq!(kinds(&cells), ["code", "code", "raw"]);
        assert_eq!(sources(&cells)[1], "b = 2");

        // And something that only looks like one.
        let cells = split_percent("# %%% not a marker\nx = 1\n");
        assert_eq!(cells.len(), 1, "a triple-percent comment split the file");
    }

    /// Round-tripping must not change the cells.
    ///
    /// This is the property that matters: saving a `.py` used to replace it with
    /// nbformat JSON, so the file stopped being a script at all.
    #[test]
    fn a_percent_file_survives_a_round_trip() {
        let src = "# %% [markdown]\n# Heading\n\n# %%\nimport numpy as np\n\n# %%\n2 + 2\n";
        let cells = split_percent(src);
        let mut doc = NotebookDocument::create_empty();
        doc.cells = cells.clone();

        let written = to_percent(&doc);
        assert!(
            !written.trim_start().starts_with('{'),
            "a .py was written as JSON: {written}"
        );
        let reread = split_percent(&written);
        assert_eq!(kinds(&reread), kinds(&cells));
        assert_eq!(sources(&reread), sources(&cells));
    }

    /// Fenced Python in Markdown becomes runnable cells.
    #[test]
    fn markdown_splits_prose_from_fenced_python() {
        let src = "# Title\n\nSome prose.\n\n```python\nimport numpy\n```\n\nMore prose.\n";
        let cells = split_markdown(src);
        assert_eq!(kinds(&cells), ["markdown", "code", "markdown"]);
        assert_eq!(sources(&cells)[1], "import numpy");
        assert!(sources(&cells)[0].contains("# Title"));
        assert_eq!(sources(&cells)[2], "More prose.");
    }

    /// A block in another language is illustration, not something to run.
    #[test]
    fn a_non_python_fence_stays_prose() {
        let src = "Run this:\n\n```bash\nrm -rf /tmp/x\n```\n";
        let cells = split_markdown(src);
        assert_eq!(kinds(&cells), ["markdown"]);
        assert!(
            sources(&cells)[0].contains("rm -rf"),
            "the block's text was lost: {:?}",
            sources(&cells)
        );
    }

    /// A document with no code is one markdown cell, as it always was.
    #[test]
    fn plain_markdown_is_one_markdown_cell() {
        let cells = split_markdown("# Just a document\n\nWith prose.\n");
        assert_eq!(kinds(&cells), ["markdown"]);
    }

    /// An unterminated fence keeps its text.
    #[test]
    fn an_unclosed_fence_does_not_swallow_the_file() {
        let cells = split_markdown("Intro\n\n```python\nx = 1\n");
        let joined = sources(&cells).join("\n");
        assert!(joined.contains("Intro"), "{joined}");
        assert!(joined.contains("x = 1"), "{joined}");
    }

    #[test]
    fn a_markdown_file_survives_a_round_trip() {
        let src = "# Title\n\nProse.\n\n```python\nimport numpy\n```\n";
        let cells = split_markdown(src);
        let mut doc = NotebookDocument::create_empty();
        doc.cells = cells.clone();

        let written = to_markdown(&doc);
        assert!(
            !written.trim_start().starts_with('{'),
            "a .md was written as JSON: {written}"
        );
        let reread = split_markdown(&written);
        assert_eq!(kinds(&reread), kinds(&cells));
        assert_eq!(sources(&reread), sources(&cells));
    }

    /// Saving an unchanged file leaves it byte for byte as it was.
    ///
    /// These live in git. A save that appended a blank line meant opening a
    /// notebook and closing it produced a diff, which teaches people to stop
    /// reading diffs.
    #[test]
    fn saving_an_unchanged_file_changes_nothing() {
        let py = "# %% [markdown]\n# A heading\n\n# %%\nimport numpy as np\n\n# %%\n2 + 2\n";
        let mut doc = NotebookDocument::create_empty();
        doc.cells = split_percent(py);
        assert_eq!(to_percent(&doc), py);

        let md = "# Title\n\nProse.\n\n```python\nimport numpy\n```\n\nMore.\n";
        let mut doc = NotebookDocument::create_empty();
        doc.cells = split_markdown(md);
        assert_eq!(to_markdown(&doc), md);
    }

    /// Every openable pattern really opens, and nothing else claims to.
    ///
    /// The dialog's patterns and the loader's extensions were separate lists.
    /// This ties them: a pattern offered by the chooser must map to a format
    /// that is not `Unsupported`.
    #[test]
    fn the_open_dialog_offers_exactly_what_can_be_opened() {
        use std::path::Path;
        for pattern in NotebookFormat::open_patterns() {
            let name = pattern.replace('*', "file");
            let format = NotebookFormat::for_path(Path::new(&name));
            assert_ne!(
                format,
                NotebookFormat::Unsupported,
                "the chooser offers {pattern} but the loader refuses it"
            );
            assert_ne!(format.kind(), "other", "{pattern} has no kind");
        }
    }

    /// `kind` follows the format, so one change updates every reader.
    #[test]
    fn the_reported_kind_follows_the_format() {
        use std::path::Path;
        for (name, kind) in [
            ("a.ipynb", "notebook"),
            ("a.py", "python"),
            ("a.md", "markdown"),
            ("a.txt", "text"),
            ("a.html", "other"),
        ] {
            assert_eq!(
                NotebookFormat::for_path(Path::new(name)).kind(),
                kind,
                "{name}"
            );
        }
    }

    /// A `.txt` opens as its text, and comes back as its text.
    #[test]
    fn plain_text_opens_and_saves_as_itself() {
        use std::path::Path;
        assert_eq!(
            NotebookFormat::for_path(Path::new("notes.txt")),
            NotebookFormat::PlainText
        );

        let notes = "Observing notes\n\nTarget M31, seeing 0.8\n";
        let cells = split_plain_text(notes);
        assert_eq!(kinds(&cells), ["markdown"]);
        // Nothing is parsed out of it: a `.txt` has no cell convention, and
        // splitting on blank lines would cut prose the author wrote whole.
        assert_eq!(cells.len(), 1);

        let mut doc = NotebookDocument::create_empty();
        doc.cells = cells;
        assert_eq!(to_plain_text(&doc), notes);
    }

    /// Analysis added under some notes survives a save.
    #[test]
    fn code_added_to_a_text_file_is_marked_and_kept() {
        let mut doc = NotebookDocument::create_empty();
        doc.cells = vec![
            NotebookCell::with_source("markdown", "Seeing was poor."),
            NotebookCell::with_source("code", "import numpy as np"),
        ];
        let written = to_plain_text(&doc);
        assert!(written.contains("Seeing was poor."), "{written}");
        assert!(
            written.contains("# %%\nimport numpy as np"),
            "code was written without a marker, so reopening loses it: {written}"
        );
    }

    /// An export format is refused by name, with the reason.
    #[test]
    fn an_exported_document_is_refused_rather_than_mis_parsed() {
        use std::path::Path;
        assert_eq!(
            NotebookFormat::for_path(Path::new("report.html")),
            NotebookFormat::Unsupported
        );

        let message = NotebookFormat::refusal(Path::new("report.html")).expect("a refusal");
        // It must say what the file is and where the notebook actually is —
        // the old path answered "invalid notebook JSON in report.html".
        assert!(message.contains("one way"), "{message}");
        assert!(message.contains(".ipynb"), "{message}");
        assert!(!message.contains("invalid notebook JSON"), "{message}");

        // A PDF gets the general form, which lists what CAN be opened.
        let message = NotebookFormat::refusal(Path::new("paper.pdf")).expect("a refusal");
        assert!(
            message.contains(".ipynb") && message.contains(".txt"),
            "{message}"
        );

        // And a file that opens fine has nothing to refuse.
        assert!(NotebookFormat::refusal(Path::new("a.ipynb")).is_none());
        assert!(NotebookFormat::refusal(Path::new("notes.txt")).is_none());
    }

    /// Windows line endings do not produce a file of one enormous cell.
    #[test]
    fn crlf_files_split_the_same_way() {
        let cells = split_percent("# %%\r\na = 1\r\n\r\n# %%\r\nb = 2\r\n");
        assert_eq!(kinds(&cells), ["code", "code"]);
        assert_eq!(sources(&cells)[0], "a = 1");
    }
}
