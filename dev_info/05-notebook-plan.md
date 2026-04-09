# Notebook — Implementation Plan

## Current State

No notebook code exists. The `main_window.rs` adds a placeholder page with "Notebook module is coming soon." at ViewStack name `"notebook"`. Everything must be built from scratch.

## Cargo.toml Changes Required

```toml
tokio = { version = "1", features = ["rt-multi-thread", "macros", "time", "sync", "process", "io-util"] }
libc = "0.2"
```

---

## Step 1: NotebookDocument Model

**File:** `src/models/notebook_document.rs` (NEW)
**Register:** `src/models/mod.rs` — add `pub mod notebook_document;`
**Dependencies:** None

```rust
// Key structs (serde-based nbformat 4.x):
pub struct NotebookDocument {
    pub nbformat: u32,
    pub nbformat_minor: u32,
    pub metadata: NotebookMetadata,
    pub cells: Vec<NotebookCell>,
}

pub struct NotebookCell {
    pub cell_type: String,          // "code" or "markdown"
    pub source: Vec<String>,        // per-line per nbformat
    pub outputs: Vec<CellOutput>,   // code cells only
    pub execution_count: Option<u32>,
    pub id: Option<String>,
    pub metadata: serde_json::Map<String, serde_json::Value>,
}

// CellOutput as tagged enum: Stream, ExecuteResult, DisplayData, Error
// OutputData with text/plain, text/html, image/png fields
// RecentNotebook { path, name, opened_at }
```

Helper methods:
- `NotebookDocument::create_empty() -> Self`
- `NotebookDocument::generate_cell_id() -> String` (8-char hex)
- `NotebookDocument::source_joined(cell) -> String`
- `NotebookDocument::source_split(text) -> Vec<String>`

---

## Step 2: Notebook Parser

**File:** `src/helpers/notebook_parser.rs` (NEW)
**Register:** `src/helpers/mod.rs`
**Dependencies:** Step 1

```rust
pub fn load_notebook(path: &Path) -> Result<NotebookDocument, String>;
pub fn save_notebook(doc: &NotebookDocument, path: &Path) -> Result<(), String>;
pub fn load_python_as_notebook(path: &Path) -> Result<NotebookDocument, String>;
pub fn load_markdown_as_notebook(path: &Path) -> Result<NotebookDocument, String>;
```

- `load_notebook`: Read JSON, deserialize, normalize cells (assign IDs, cap 10K).
- `save_notebook`: Atomic write (write `.tmp`, rename). JSON with 1-space indent per Jupyter convention.
- `.py` → single code cell, `.md` → single markdown cell.

---

## Step 3: Python Discovery

**File:** `src/helpers/python_discovery.rs` (NEW)
**Register:** `src/helpers/mod.rs`
**Dependencies:** None

```rust
pub fn find_python(configured_path: Option<&str>) -> Option<PathBuf>;
pub fn validate_python(path: &Path) -> Option<(u32, u32)>; // (major, minor)
```

Search order (Linux):
1. User-configured path from settings
2. `python3` on PATH
3. `python` on PATH (validate >= 3.8)
4. `~/.pyenv/shims/python3`
5. `~/miniconda3/bin/python3`, `~/anaconda3/bin/python3`
6. `/usr/bin/python3`, `/usr/local/bin/python3`

Validate via `Command::new(path).arg("--version")`, parse "Python 3.X.Y".

---

## Step 4: Kernel Harness Python Script

**File:** `data/kernel_harness.py` (NEW)
**Embedded via:** `include_str!("../../data/kernel_harness.py")` in kernel_service.rs
**Dependencies:** None

~200 lines of Python implementing:
- JSON stdin/stdout protocol
- Request: `{"type": "execute", "code": "...", "exec_count": N}` or `{"type": "quit"}`
- Response: stream, execute_result, display_data, error messages (one JSON per line)
- Boundary sentinel: `\x04__CANFAR_EXEC_BOUNDARY__\x04`
- User namespace execution (compile as eval first, fallback to exec)
- stdout/stderr capture via StringIO redirect
- Matplotlib Agg backend → PNG base64 figure capture
- Magic commands: `%pip install X`, `%conda install X`, `!shell command`
- Colab compat: mock `google.colab`, `/content/` path rewrite
- Traceback formatting with harness frames stripped

---

## Step 5: Kernel Service

**File:** `src/services/kernel_service.rs` (NEW)
**Register:** `src/services/mod.rs`
**Dependencies:** Steps 3, 4

```rust
pub enum KernelState { Dead, Starting, Idle, Busy, Error }

pub struct LocalKernelService {
    state: KernelState,
    process: Option<tokio::process::Child>,
    stdin: Option<tokio::process::ChildStdin>,
    stdout_reader: Option<BufReader<tokio::process::ChildStdout>>,
    python_path: PathBuf,
    exec_count: u32,
}

impl LocalKernelService {
    pub fn new(python_path: PathBuf) -> Self;
    pub async fn start(&mut self) -> Result<(), String>;
    pub async fn execute(&mut self, code: &str) -> Result<(Vec<CellOutput>, u32), String>;
    pub fn interrupt(&mut self);  // libc::kill(pid, SIGINT)
    pub async fn restart(&mut self) -> Result<(), String>;
    pub fn shutdown(&mut self);
    pub fn state(&self) -> KernelState;
}
```

- `start`: Write harness to temp dir, spawn `python3 -u harness.py` with piped stdio.
- `execute`: Write JSON to stdin, read lines until boundary, parse each as CellOutput.
- `interrupt`: `libc::kill(pid, libc::SIGINT)`. If no response after 5s, kill and restart.
- State transitions: Dead→Starting→Idle on start, Idle→Busy→Idle on execute.

---

## Step 6: Notebook Store

**File:** `src/services/notebook_store.rs` (NEW)
**Register:** `src/services/mod.rs`
**Dependencies:** Step 1

```rust
pub struct NotebookStore { file_path: PathBuf }

impl NotebookStore {
    pub fn new() -> Self;
    pub fn load(&self) -> Vec<RecentNotebook>;
    pub fn add(&self, path: &str, name: &str) -> Result<(), String>;
    pub fn remove(&self, index: usize) -> Result<(), String>;
    pub fn clear(&self) -> Result<(), String>;
}
```

Max 15 entries. Storage: `~/.local/share/net.canfar/Verbinal/recent_notebooks.json`.

---

## Step 7: UI — Cell Widgets

**File:** `src/ui/notebook_cell.rs` (NEW)
**Register:** `src/ui/mod.rs`
**Dependencies:** Step 1

### CodeCellWidget
```rust
pub struct CodeCellWidget {
    widget: gtk::Box,
    text_view: gtk::TextView,
    output_box: gtk::Box,
    exec_label: gtk::Label,      // "[ ]", "[*]", "[N]"
    run_button: gtk::Button,
    active: Rc<RefCell<bool>>,
}
```

Methods: `set_source`, `get_source`, `set_execution_count`, `set_executing`, `set_outputs`, `clear_outputs`, `set_active`, `grab_focus`.

**Syntax highlighting:** Apply `gtk::TextTag` objects for Python keywords (35), builtins (50), strings, comments, numbers. Debounce 100ms via `glib::timeout_add_local_once`.

**Output rendering:**
- `CellOutput::Stream` → `gtk::Label` (monospace, red for stderr)
- `CellOutput::ExecuteResult/DisplayData` → check `image/png` first (base64 → Pixbuf → `gtk::Picture`), then `text/plain`
- `CellOutput::Error` → `gtk::TextView` (monospace, red, strip ANSI codes)

### MarkdownCellWidget
```rust
pub struct MarkdownCellWidget {
    widget: gtk::Box,
    text_view: gtk::TextView,     // edit mode
    preview_label: gtk::Label,    // preview mode (Pango markup)
    editing: Rc<RefCell<bool>>,
}
```

Methods: `set_source`, `get_source`, `enter_edit_mode`, `enter_preview_mode`, `set_active`, `grab_focus`.

Markdown→Pango: `# H` → `<span size="xx-large"><b>H</b></span>`, `**bold**` → `<b>bold</b>`, `*italic*` → `<i>italic</i>`, `` `code` `` → `<tt>code</tt>`.

---

## Step 8: UI — Notebook Page

**File:** `src/ui/notebook_page.rs` (NEW)
**Register:** `src/ui/mod.rs`
**Dependencies:** Steps 1, 2, 5, 7

```rust
pub struct NotebookPage {
    widget: gtk::Box,
    cell_list_box: gtk::ListBox,
    cell_widgets: Rc<RefCell<Vec<CellWidget>>>,
    kernel: Rc<RefCell<LocalKernelService>>,
    document: Rc<RefCell<NotebookDocument>>,
    active_cell: Rc<RefCell<usize>>,
    mode: Rc<RefCell<CellMode>>,          // Command or Edit
    undo_stack: Rc<RefCell<UndoStack>>,
    file_path: Rc<RefCell<Option<PathBuf>>>,
    modified: Rc<RefCell<bool>>,
    kernel_status_label: gtk::Label,
}
```

Methods: `new`, `widget`, `save`, `save_as`, `is_modified`, `run_cell`, `run_all`, `interrupt`, `restart_kernel`, `insert_cell`, `delete_cell`, `move_cell`, `select_cell`, `undo`, `redo`.

**UndoStack:** `Vec<CellSnapshot>` (max 50), push before structural changes.

**Autosave:** `glib::timeout_add_local` every 30s, write to `~/.local/share/net.canfar/Verbinal/autosave/{hash}.ipynb`, atomic write, delete on clean close.

---

## Step 9: UI — Tab Host

**File:** `src/ui/notebook_host.rs` (NEW)
**Register:** `src/ui/mod.rs`
**Dependencies:** Steps 2, 3, 6, 8

```rust
pub struct NotebookTabHost {
    widget: gtk::Box,
    notebook: gtk::Notebook,
    tabs: Rc<RefCell<Vec<Rc<NotebookPage>>>>,
    store: NotebookStore,
    python_path: Rc<RefCell<Option<PathBuf>>>,
}
```

Methods: `new`, `widget`, `open_file`, `load_from_path` (integration), `add_tab`, `close_tab`, `save_current`, `run_all_current`.

Toolbar: Open, Save, separator, Run All, Restart, Interrupt, spacer, kernel status, python path.

Empty state: centered icon + "Open a notebook to get started" + recent notebooks list.

---

## Step 10: Keyboard Shortcuts

Implemented inside `notebook_page.rs` via `gtk::EventControllerKey`.

**Command mode** (when no cell is in edit):
| Key | Action |
|-----|--------|
| A | Insert cell above |
| B | Insert cell below |
| D D | Delete cell (two presses within 500ms) |
| C | Copy cell |
| V | Paste cell below |
| M | Change to markdown |
| Y | Change to code |
| Enter | Enter edit mode |
| Up | Select previous cell |
| Down | Select next cell |
| Ctrl+Enter | Run cell, stay |
| Shift+Enter | Run cell, advance |
| Z | Undo |
| Ctrl+Z | Undo |
| Ctrl+Shift+Z | Redo |
| Ctrl+S | Save |

**Edit mode** (cell's TextView has focus):
| Key | Action |
|-----|--------|
| Escape | Exit to command mode |
| Ctrl+Enter | Run cell, stay |
| Shift+Enter | Run cell, advance |
| Up (at line 1) | Exit edit, select previous cell |
| Down (at last line) | Exit edit, select next cell |

---

## Step 11: Autosave + Recovery

Part of `notebook_page.rs`:
- Timer via `glib::timeout_add_local` (30s configurable from settings)
- Write to `{data_dir}/autosave/{filename_hash}.autosave.ipynb`
- Atomic: write `.tmp` then rename
- Delete autosave on clean save or close
- On open: check for orphaned `.autosave.ipynb` files, offer recovery dialog

---

## Step 12: Undo/Redo

Part of `notebook_page.rs`:
```rust
struct UndoStack {
    undo: Vec<CellSnapshot>,
    redo: Vec<CellSnapshot>,
}
struct CellSnapshot {
    cells: Vec<(String, String, Option<u32>)>, // (cell_type, source, exec_count)
    active_index: usize,
}
```
- Push BEFORE structural changes (add/delete/move/type-change/merge/split)
- Max 50 undo levels
- New action clears redo stack

---

## Step 13: Dependency Scanner

Add to `src/helpers/notebook_parser.rs`:
```rust
pub fn extract_imports(cells: &[NotebookCell]) -> Vec<String>;
pub async fn check_missing(python: &Path, packages: &[String]) -> Vec<String>;
pub async fn install_packages(python: &Path, packages: &[String]) -> Result<String, String>;
```

- Extract: regex `^\s*import\s+(\w+)` and `^\s*from\s+(\w+)` on code cells
- Filter out stdlib (60+ known modules)
- Map import names to pip names (PIL→Pillow, cv2→opencv-python, etc.)
- Check: `python3 -c "import {pkg}"` per package
- Install: `python3 -m pip install {pkg1} {pkg2} ...`
- Show in NotebookPage as `adw::Banner` at top: "Missing: numpy, matplotlib [Install]"

---

## Step 14: Integration

**Files to modify:**
- `src/ui/main_window.rs` — Replace notebook placeholder with `NotebookTabHost::new()`
- `src/ui/mod.rs` — Add `pub mod notebook_host; pub mod notebook_page; pub mod notebook_cell;`
- `src/models/mod.rs` — Add `pub mod notebook_document;`
- `src/helpers/mod.rs` — Add `pub mod notebook_parser; pub mod python_discovery;`
- `src/services/mod.rs` — Add `pub mod kernel_service; pub mod notebook_store;`
- `src/config.rs` — Add `NotebookSettings` to `AppConfig`:
  ```rust
  pub notebook_font_size: u32,        // default 13
  pub notebook_tab_size: u32,         // default 4
  pub notebook_word_wrap: bool,       // default true
  pub notebook_autosave_secs: u32,    // default 30
  pub notebook_python_path: Option<String>,
  ```
- `src/ui/settings_page.rs` — Add "Notebook" preferences group (font, tab size, wrap, autosave, python path)
- `Cargo.toml` — Add `tokio/process`, `tokio/io-util` features, add `libc = "0.2"`

Wire VOSpace "Open in Notebook" for .ipynb files → `notebook_host.load_from_path(path)`.

---

## Implementation Order

| Phase | Step | Effort | Description |
|-------|------|--------|-------------|
| 1 | Step 1 | 1 day | NotebookDocument model with serde |
| 1 | Step 2 | 0.5 day | Parser (load/save JSON, .py, .md) |
| 1 | Step 3 | 0.5 day | Python discovery |
| 2 | Step 4 | 1 day | Kernel harness Python script |
| 2 | Step 5 | 2 days | Kernel service (subprocess, protocol) |
| 2 | Step 6 | 0.5 day | Notebook store (recent files) |
| 3 | Step 7 | 2 days | Cell widgets (code + markdown + outputs) |
| 3 | Step 8 | 2 days | Notebook page (cell list, execution, save) |
| 4 | Step 9 | 1 day | Tab host (multi-tab, toolbar, empty state) |
| 4 | Step 10 | 1 day | Keyboard shortcuts (command/edit modes) |
| 4 | Step 11 | 0.5 day | Autosave + recovery |
| 4 | Step 12 | 0.5 day | Undo/redo |
| 5 | Step 13 | 0.5 day | Dependency scanner |
| 5 | Step 14 | 0.5 day | Integration + settings |
| **Total** | | **~13 days** | |
