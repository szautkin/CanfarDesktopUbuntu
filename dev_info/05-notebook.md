# 05 - Notebook Module Specification

## Purpose

The Notebook module is a native Jupyter notebook engine that opens, edits, and executes `.ipynb` files locally using a Python subprocess. It provides a cell-based editing and execution experience comparable to Jupyter Notebook/Lab, without requiring a Jupyter server.

## Architecture

```
NotebookTabHost (singleton, lives in ViewStack)
    |
    +-- Tab 1: NotebookPage
    |       |-- CellList (ScrolledWindow + ListBox)
    |       |     |-- CodeCell 0
    |       |     |-- MarkdownCell 1
    |       |     |-- CodeCell 2
    |       |     +-- ...
    |       +-- LocalKernelService (Python subprocess)
    |
    +-- Tab 2: NotebookPage
    |       |-- CellList
    |       +-- LocalKernelService
    |
    +-- (empty state / welcome)
```

### NotebookTabHost

Singleton widget registered in the main `ViewStack` under the name `"notebook"`. Contains a `gtk::Notebook` (tab widget) for managing multiple open notebooks.

```rust
pub struct NotebookTabHost {
    widget: gtk::Box,
    notebook: gtk::Notebook,          // GTK tab container
    tabs: Rc<RefCell<Vec<Rc<NotebookPage>>>>,
    recent_notebooks: Rc<RefCell<Vec<RecentNotebook>>>,
    status_label: gtk::Label,
}
```

Toolbar (above tabs):
- **Open** button (`document-open-symbolic`): File chooser for `.ipynb`, `.py`, `.md`.
- **Save** button (`document-save-symbolic`): Save current tab's notebook.
- **Run All** button (`media-playback-start-symbolic`): Execute all cells in order.
- **Restart Kernel** button (`view-refresh-symbolic`): Kill and restart the Python subprocess.
- **Interrupt** button (`process-stop-symbolic`): Cancel currently executing cell.
- Spacer.
- Kernel status indicator: `gtk::Label` showing `"Idle"`, `"Busy"`, `"Dead"`, or `"Starting..."`.
- Python path label: `gtk::Label` showing the resolved Python interpreter path.

Empty state (when no tabs are open): centered icon + label "Open a notebook to get started", same pattern as FITS viewer welcome.

### NotebookPage

One per open notebook file. Manages cells, kernel, and document state.

```rust
pub struct NotebookPage {
    widget: gtk::Box,
    cells: Rc<RefCell<Vec<Rc<Cell>>>>,
    kernel: Rc<RefCell<LocalKernelService>>,
    document: Rc<RefCell<NotebookDocument>>,
    active_cell_index: Rc<RefCell<usize>>,
    mode: Rc<RefCell<CellMode>>,       // Command or Edit
    undo_stack: Rc<RefCell<UndoStack>>,
    file_path: Option<PathBuf>,
    modified: Rc<RefCell<bool>>,
    autosave_source: Rc<RefCell<Option<glib::SourceId>>>,
}

#[derive(Clone, Copy, PartialEq)]
pub enum CellMode {
    Command,  // Navigation mode, keyboard shortcuts active
    Edit,     // Text editing inside a cell
}
```

## Notebook Document Format

### nbformat 4.x JSON

The `.ipynb` file format is nbformat version 4, minor version 0-5. Verbinal reads and writes this format.

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotebookDocument {
    pub nbformat: u32,                      // Always 4
    pub nbformat_minor: u32,                // 0-5
    pub metadata: NotebookMetadata,
    pub cells: Vec<NotebookCell>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotebookMetadata {
    pub kernelspec: Option<KernelSpec>,
    pub language_info: Option<LanguageInfo>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KernelSpec {
    pub name: String,                       // e.g., "python3"
    pub display_name: String,               // e.g., "Python 3"
    pub language: Option<String>,           // e.g., "python"
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LanguageInfo {
    pub name: String,                       // "python"
    pub version: Option<String>,            // e.g., "3.10.12"
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotebookCell {
    pub cell_type: String,                  // "code" or "markdown"
    pub source: Vec<String>,                // Lines of source text (per-line list)
    #[serde(default)]
    pub metadata: serde_json::Map<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<CellOutput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_count: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "output_type")]
pub enum CellOutput {
    #[serde(rename = "stream")]
    Stream {
        name: String,                       // "stdout" or "stderr"
        text: Vec<String>,
    },
    #[serde(rename = "execute_result")]
    ExecuteResult {
        execution_count: u32,
        data: OutputData,
        metadata: serde_json::Map<String, serde_json::Value>,
    },
    #[serde(rename = "display_data")]
    DisplayData {
        data: OutputData,
        metadata: serde_json::Map<String, serde_json::Value>,
    },
    #[serde(rename = "error")]
    Error {
        ename: String,
        evalue: String,
        traceback: Vec<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputData {
    #[serde(rename = "text/plain", default, skip_serializing_if = "Vec::is_empty")]
    pub text_plain: Vec<String>,
    #[serde(rename = "text/html", default, skip_serializing_if = "Vec::is_empty")]
    pub text_html: Vec<String>,
    #[serde(rename = "image/png", default, skip_serializing_if = "Option::is_none")]
    pub image_png: Option<String>,          // Base64-encoded PNG
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}
```

### Source Line Convention

In nbformat 4, the `source` field is a list of strings where each string is a line of text. Lines include trailing newlines except (optionally) the last line.

```json
"source": [
    "import numpy as np\n",
    "x = np.arange(10)\n",
    "print(x)"
]
```

When loading, join into a single string for editing: `source.join("")`. When saving, split back: split on `\n`, re-add `\n` to each line except the last.

### Loading Non-Notebook Files

- **`.py` files**: Create a single code cell with the file contents as source. Set `nbformat: 4`, `nbformat_minor: 5`, kernel info for Python.
- **`.md` files**: Create a single markdown cell with the file contents as source.
- Save always writes `.ipynb` format. If the original file was `.py` or `.md`, suggest saving as `.ipynb` (change extension in save dialog).

## Cell Types

### Code Cell

```
+--------------------------------------------------+
| [In 3]:  import numpy as np                      |
|          x = np.arange(100)                      |
|          plt.plot(x, np.sin(x))                  |
|          plt.show()                               |
+--------------------------------------------------+
| [Out 3]: <matplotlib figure PNG>                  |
+--------------------------------------------------+
```

UI structure:
- Container: `gtk::Box` vertical with `card` CSS class and colored left border (blue when active, gray otherwise).
- Header: `gtk::Box` horizontal with execution count label (`"In [3]:"` / `"In [ ]:"` for unexecuted) and a run button (`media-playback-start-symbolic`, flat).
- Source editor: `gtk::TextView` with monospace font, line numbers (via custom draw on the gutter or prefix labels), and basic syntax highlighting.
- Output area: `gtk::Box` vertical, dynamically populated based on `CellOutput` types. Hidden when no outputs.

### Syntax Highlighting

Implement a minimal Python syntax highlighter using `gtk::TextTag` applied to the `gtk::TextBuffer`:

| Token Type | Color (dark theme) | Color (light theme) |
|-----------|-------------------|---------------------|
| Keyword (`def`, `class`, `if`, `for`, `import`, etc.) | `#c678dd` | `#a626a4` |
| String (single/double/triple quoted) | `#98c379` | `#50a14f` |
| Comment (`#...`) | `#5c6370` | `#a0a1a7` |
| Number | `#d19a66` | `#986801` |
| Built-in (`print`, `len`, `range`, `True`, `False`, `None`) | `#e5c07b` | `#c18401` |
| Default | inherit | inherit |

Apply highlighting on `TextBuffer::connect_changed` with a debounce of 100ms to avoid highlighting on every keystroke.

### Markdown Cell

Two modes:
- **Edit mode**: `gtk::TextView` with the raw markdown source visible.
- **Preview mode**: Rendered markdown displayed in a `gtk::Label` with markup enabled (`set_use_markup(true)`) or a `gtk::TextView` with styled text tags.

Toggle between modes: double-click to enter edit, Escape or Ctrl+Enter to render and switch to preview.

Markdown rendering: Convert a subset of Markdown to Pango markup:
- `# Heading` -> `<span size="xx-large" weight="bold">Heading</span>`
- `## Heading` -> `<span size="x-large" weight="bold">Heading</span>`
- `**bold**` -> `<b>bold</b>`
- `*italic*` -> `<i>italic</i>`
- `` `code` `` -> `<tt>code</tt>`
- `- item` -> `\u2022 item`
- Code blocks (triple backtick) -> monospace font section

For a production-quality implementation, consider using a Markdown-to-Pango library or rendering to HTML and displaying in a WebKit view. For the initial implementation, the Pango markup approach is sufficient.

## Kernel: LocalKernelService

### Architecture

```rust
pub struct LocalKernelService {
    state: KernelState,
    process: Option<Child>,             // tokio::process::Child
    stdin: Option<ChildStdin>,
    stdout_reader: Option<BufReader<ChildStdout>>,
    python_path: PathBuf,
    exec_count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum KernelState {
    Dead,
    Starting,
    Idle,
    Busy,
    Error,
}
```

### State Machine

```
Dead ---start()--> Starting ---ready--> Idle
                                         |
                                    execute()
                                         |
                                         v
                                       Busy ---done--> Idle
                                         |
                                    interrupt()
                                         |
                                         v
                                       Dead ---start()--> Starting
                                         
Error (any state can transition here on unexpected process death)
  |
  +---restart()--> Starting
```

### Python Subprocess

The kernel spawns a Python process that reads JSON commands on stdin and writes JSON responses on stdout. A harness script is embedded in the Rust binary and written to a temp file at startup.

```rust
fn start_kernel(&mut self) -> Result<(), String> {
    self.state = KernelState::Starting;
    
    // Write harness script to temp file
    let harness_dir = std::env::temp_dir().join("verbinal_kernel");
    std::fs::create_dir_all(&harness_dir)?;
    let harness_path = harness_dir.join("kernel_harness.py");
    std::fs::write(&harness_path, KERNEL_HARNESS_SCRIPT)?;
    
    let child = Command::new(&self.python_path)
        .arg(&harness_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    
    self.process = Some(child);
    self.state = KernelState::Idle;
    Ok(())
}
```

### Protocol

Communication is JSON over stdin/stdout with a sentinel boundary marker to delimit message frames.

**Boundary sentinel**: `\x04__CANFAR_EXEC_BOUNDARY__\x04` (ETX character + fixed string + ETX character). This appears on its own line after each complete response sequence.

**Request format** (Rust -> Python):
```json
{"type": "execute", "code": "print('hello')", "exec_count": 1}
```

**Response format** (Python -> Rust): Multiple JSON lines followed by the boundary sentinel.

```json
{"msg_type": "status", "state": "busy"}
{"msg_type": "stream", "name": "stdout", "text": "hello\n"}
{"msg_type": "execute_reply", "status": "ok", "exec_count": 1}
\x04__CANFAR_EXEC_BOUNDARY__\x04
```

Response message types:

| msg_type | Fields | Description |
|----------|--------|-------------|
| `status` | `state`: `"busy"` or `"idle"` | Kernel state change |
| `stream` | `name`: `"stdout"` / `"stderr"`, `text`: string | Print output |
| `execute_result` | `data`: `{mime_type: content}`, `exec_count` | Expression result |
| `display_data` | `data`: `{mime_type: content}` | Rich display output (images, HTML) |
| `error` | `ename`, `evalue`, `traceback`: list of strings | Exception |
| `execute_reply` | `status`: `"ok"` / `"error"`, `exec_count` | Execution complete |

### Harness Script

The embedded Python harness script (approximately 150-200 lines):

```python
#!/usr/bin/env python3
"""Verbinal kernel harness - executes code blocks and streams results as JSON."""

import sys
import json
import io
import traceback
import ast
import base64

BOUNDARY = "\x04__CANFAR_EXEC_BOUNDARY__\x04"

class VerbinalKernel:
    def __init__(self):
        self.namespace = {"__name__": "__main__", "__builtins__": __builtins__}
        self._setup_matplotlib()
        self._setup_colab_compat()
    
    def _setup_matplotlib(self):
        """Configure matplotlib to use Agg backend and capture figures."""
        try:
            import matplotlib
            matplotlib.use('Agg')
        except ImportError:
            pass
    
    def _setup_colab_compat(self):
        """Mock google.colab for Colab notebook compatibility."""
        # Create a fake google.colab module
        import types
        colab = types.ModuleType("google.colab")
        colab.drive = types.ModuleType("google.colab.drive")
        colab.drive.mount = lambda *a, **kw: None
        google = types.ModuleType("google")
        google.colab = colab
        sys.modules["google"] = google
        sys.modules["google.colab"] = colab
        sys.modules["google.colab.drive"] = colab.drive
    
    def send(self, msg):
        """Send a JSON message to stdout."""
        print(json.dumps(msg), flush=True)
    
    def execute(self, code, exec_count):
        self.send({"msg_type": "status", "state": "busy"})
        
        # Redirect stdout/stderr
        old_stdout, old_stderr = sys.stdout, sys.stderr
        captured_stdout = io.StringIO()
        captured_stderr = io.StringIO()
        sys.stdout = captured_stdout
        sys.stderr = captured_stderr
        
        try:
            # Parse to separate last expression for result display
            tree = ast.parse(code)
            last_expr = None
            if tree.body and isinstance(tree.body[-1], ast.Expr):
                last_expr = ast.Expression(tree.body.pop())
                ast.fix_missing_locations(last_expr)
            
            # Execute statements
            if tree.body:
                exec(compile(tree, "<cell>", "exec"), self.namespace)
            
            # Evaluate last expression
            if last_expr:
                result = eval(compile(last_expr, "<cell>", "eval"), self.namespace)
                if result is not None:
                    # Check for matplotlib figures
                    # Check for rich display (_repr_html_, _repr_png_, etc.)
                    # Fall back to text/plain repr
                    self._send_result(result, exec_count)
            
            # Flush captured output
            self._flush_streams(captured_stdout, captured_stderr, old_stdout)
            
            # Capture matplotlib figures
            self._capture_figures(old_stdout)
            
            sys.stdout, sys.stderr = old_stdout, old_stderr
            self.send({"msg_type": "execute_reply", "status": "ok", "exec_count": exec_count})
        
        except Exception:
            sys.stdout, sys.stderr = old_stdout, old_stderr
            self._flush_streams(captured_stdout, captured_stderr, old_stdout)
            tb = traceback.format_exception(*sys.exc_info())
            self.send({
                "msg_type": "error",
                "ename": type(sys.exc_info()[1]).__name__,
                "evalue": str(sys.exc_info()[1]),
                "traceback": tb
            })
            self.send({"msg_type": "execute_reply", "status": "error", "exec_count": exec_count})
        
        self.send({"msg_type": "status", "state": "idle"})
        # Print boundary on raw stdout (bypassing capture)
        old_stdout.write(BOUNDARY + "\n")
        old_stdout.flush()
    
    def _send_result(self, result, exec_count):
        """Send execute_result with appropriate MIME types."""
        data = {"text/plain": repr(result)}
        if hasattr(result, '_repr_html_'):
            html = result._repr_html_()
            if html:
                data["text/html"] = html
        if hasattr(result, '_repr_png_'):
            png = result._repr_png_()
            if png:
                data["image/png"] = base64.b64encode(png).decode()
        sys.stdout = sys.__stdout__  # temporarily restore for send
        self.send({
            "msg_type": "execute_result",
            "data": data,
            "exec_count": exec_count
        })
    
    def _flush_streams(self, captured_stdout, captured_stderr, real_stdout):
        """Send any captured stdout/stderr as stream messages."""
        stdout_text = captured_stdout.getvalue()
        stderr_text = captured_stderr.getvalue()
        sys.stdout = real_stdout
        if stdout_text:
            self.send({"msg_type": "stream", "name": "stdout", "text": stdout_text})
        if stderr_text:
            self.send({"msg_type": "stream", "name": "stderr", "text": stderr_text})
    
    def _capture_figures(self, real_stdout):
        """Capture any open matplotlib figures as PNG."""
        try:
            import matplotlib.pyplot as plt
            for fig_num in plt.get_fignums():
                fig = plt.figure(fig_num)
                buf = io.BytesIO()
                fig.savefig(buf, format='png', bbox_inches='tight', dpi=100)
                buf.seek(0)
                png_b64 = base64.b64encode(buf.read()).decode()
                sys.stdout = real_stdout
                self.send({
                    "msg_type": "display_data",
                    "data": {"image/png": png_b64, "text/plain": f"<Figure {fig_num}>"}
                })
            plt.close('all')
        except ImportError:
            pass

def main():
    kernel = VerbinalKernel()
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            msg = json.loads(line)
            if msg.get("type") == "execute":
                kernel.execute(msg["code"], msg.get("exec_count", 0))
        except json.JSONDecodeError:
            pass

if __name__ == "__main__":
    main()
```

### Execution Flow (Rust Side)

```rust
async fn execute_cell(&self, code: &str) -> Vec<CellOutput> {
    self.exec_count += 1;
    let request = serde_json::json!({
        "type": "execute",
        "code": code,
        "exec_count": self.exec_count
    });
    
    // Write request to stdin
    let stdin = self.stdin.as_mut().unwrap();
    stdin.write_all(request.to_string().as_bytes()).await?;
    stdin.write_all(b"\n").await?;
    stdin.flush().await?;
    
    // Read responses until boundary
    let mut outputs = Vec::new();
    let reader = self.stdout_reader.as_mut().unwrap();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).await?;
        let line = line.trim();
        
        if line.contains("__CANFAR_EXEC_BOUNDARY__") {
            break;
        }
        
        if let Ok(msg) = serde_json::from_str::<serde_json::Value>(line) {
            match msg["msg_type"].as_str() {
                Some("stream") => {
                    outputs.push(CellOutput::Stream {
                        name: msg["name"].as_str().unwrap_or("stdout").to_string(),
                        text: vec![msg["text"].as_str().unwrap_or("").to_string()],
                    });
                }
                Some("execute_result") => { /* parse and push */ }
                Some("display_data") => { /* parse and push */ }
                Some("error") => { /* parse and push */ }
                Some("status") => { /* update kernel state */ }
                Some("execute_reply") => { /* execution done, continue reading to boundary */ }
                _ => {}
            }
        }
    }
    
    outputs
}
```

### Interrupt

On Linux, send SIGINT to the Python subprocess:

```rust
fn interrupt(&mut self) {
    if let Some(ref child) = self.process {
        // Send SIGINT via nix or libc
        unsafe {
            libc::kill(child.id() as i32, libc::SIGINT);
        }
    }
    // If interrupt doesn't work within 5 seconds, kill and restart
}
```

Fallback: kill the process and restart the kernel. This loses the execution namespace.

## Output Rendering

### text/plain

Display in a `gtk::Label` or `gtk::TextView` with monospace font, inside the output area. Selectable text.

### image/png

Decode base64 string to bytes, create `gdk_pixbuf::Pixbuf` from bytes, create `gdk::Texture`, display in `gtk::Picture`. Scale to fit cell width while maintaining aspect ratio.

```rust
fn render_png_output(base64_data: &str) -> gtk::Picture {
    let bytes = base64::engine::general_purpose::STANDARD.decode(base64_data).unwrap();
    let gbytes = glib::Bytes::from_owned(bytes);
    let stream = gio::MemoryInputStream::from_bytes(&gbytes);
    let pixbuf = gdk_pixbuf::Pixbuf::from_stream(&stream, gio::Cancellable::NONE).unwrap();
    let texture = gdk::Texture::for_pixbuf(&pixbuf);
    let picture = gtk::Picture::for_paintable(Some(&texture));
    picture.set_content_fit(gtk::ContentFit::Contain);
    picture.set_can_shrink(true);
    picture
}
```

### text/html

For pandas DataFrame HTML tables: parse the `<table>` HTML and render as a `gtk::Grid` with styled cells. Alternatively, use a simplified table renderer:

```rust
fn render_html_table(html: &str) -> gtk::ScrolledWindow {
    // Parse <table> from HTML
    // Extract <thead> <th> headers and <tbody> <tr> <td> cells
    // Build a gtk::Grid or gtk::ColumnView
    // Style with monospace font and borders
}
```

For other HTML content: fall back to displaying the `text/plain` representation.

### error

Display traceback lines in a `gtk::TextView` with monospace font and red/orange coloring. Strip ANSI escape codes or convert them to Pango markup:

- `\x1b[0;31m` (red) -> error name highlighting
- `\x1b[0;32m` (green) -> filename highlighting
- `\x1b[0m` (reset) -> back to default

## Python Discovery

Search for a valid Python interpreter at startup:

```rust
fn find_python() -> Option<PathBuf> {
    let candidates = [
        // 1. Explicit config setting (if user has set a Python path)
        // 2. PATH search
        "python3",
        "python",
        // 3. pyenv
        // ~/.pyenv/shims/python3
        // 4. conda
        // ~/miniconda3/bin/python3, ~/anaconda3/bin/python3
        // 5. Common install locations
        "/usr/bin/python3",
        "/usr/local/bin/python3",
    ];
    
    for candidate in candidates {
        if let Ok(output) = Command::new(candidate)
            .arg("--version")
            .output()
        {
            if output.status.success() {
                let version_str = String::from_utf8_lossy(&output.stdout);
                if parse_version(&version_str) >= (3, 8) {
                    return Some(PathBuf::from(candidate));
                }
            }
        }
    }
    None
}
```

Minimum required version: Python 3.8. Display warning if not found: `"Python 3.8+ is required for notebook execution. Install Python or set the path in Settings."`.

## Magic Commands

Intercept magic commands before sending to the Python kernel:

| Command | Behavior |
|---------|----------|
| `%pip install <pkg>` | Run `{python} -m pip install <pkg>` as subprocess, stream output |
| `%conda install <pkg>` | Run `conda install <pkg>` as subprocess, stream output |
| `!<command>` | Run `<command>` in a shell subprocess, stream stdout/stderr |
| `%matplotlib inline` | Already handled (Agg backend captures figures) |
| `%time <code>` | Wrap code in timing and report elapsed time |

Magic command detection: Check if the first non-whitespace character of the code is `%` or `!`.

## Keyboard Shortcuts

### Command Mode (cell selected, not editing)

| Key | Action |
|-----|--------|
| `Enter` | Enter edit mode for selected cell |
| `Escape` | Stay in command mode (no-op) |
| `A` | Insert new code cell above |
| `B` | Insert new code cell below |
| `D`, `D` (double press within 500ms) | Delete selected cell |
| `C` | Copy selected cell |
| `V` | Paste cell below |
| `X` | Cut selected cell |
| `M` | Change cell type to Markdown |
| `Y` | Change cell type to Code |
| `Up` / `K` | Select previous cell |
| `Down` / `J` | Select next cell |
| `Ctrl+Enter` | Run selected cell, stay on it |
| `Shift+Enter` | Run selected cell, advance to next (create new if last) |
| `Ctrl+S` | Save notebook |
| `Z` | Undo last structural change |
| `Shift+Z` | Redo |

### Edit Mode (typing inside a cell)

| Key | Action |
|-----|--------|
| `Escape` | Exit to command mode |
| `Ctrl+Enter` | Run cell, exit to command mode |
| `Shift+Enter` | Run cell, advance to next cell |
| `Tab` | Indent (insert spaces based on tab_size setting) |
| `Shift+Tab` | Dedent |
| `Up` (at first line of cell) | Exit edit mode, select cell above |
| `Down` (at last line of cell) | Exit edit mode, select cell below |

### Implementation

Use `gtk::EventControllerKey` on the main NotebookPage widget. In command mode, intercept single key presses. In edit mode, most keys pass through to the `gtk::TextView`.

```rust
fn setup_keyboard_shortcuts(page: &NotebookPage) {
    let controller = gtk::EventControllerKey::new();
    let mode = page.mode.clone();
    let cells = page.cells.clone();
    let active = page.active_cell_index.clone();
    
    controller.connect_key_pressed(move |_, key, _code, modifiers| {
        let mode = *mode.borrow();
        let ctrl = modifiers.contains(gdk::ModifierType::CONTROL_MASK);
        let shift = modifiers.contains(gdk::ModifierType::SHIFT_MASK);
        
        match mode {
            CellMode::Command => {
                match key {
                    gdk::Key::Return => { enter_edit_mode(); Propagation::Stop }
                    gdk::Key::a => { insert_cell_above(); Propagation::Stop }
                    gdk::Key::b => { insert_cell_below(); Propagation::Stop }
                    // ... etc
                    _ => Propagation::Proceed
                }
            }
            CellMode::Edit => {
                if key == gdk::Key::Escape {
                    exit_to_command_mode();
                    Propagation::Stop
                } else if ctrl && key == gdk::Key::Return {
                    run_current_cell();
                    Propagation::Stop
                } else {
                    Propagation::Proceed  // Let TextView handle it
                }
            }
        }
    });
    
    page.widget.add_controller(controller);
}
```

## Autosave

### Timer

Start a `glib::timeout_add_local` with 30-second interval when a notebook is modified:

```rust
fn schedule_autosave(&self) {
    // Cancel existing autosave timer if any
    if let Some(source_id) = self.autosave_source.borrow_mut().take() {
        source_id.remove();
    }
    
    let document = self.document.clone();
    let file_path = self.file_path.clone();
    let modified = self.modified.clone();
    
    let source_id = glib::timeout_add_local(
        std::time::Duration::from_secs(30),
        move || {
            if *modified.borrow() {
                if let Some(ref path) = file_path {
                    let autosave_path = autosave_path_for(path);
                    let doc = document.borrow();
                    let json = serde_json::to_string_pretty(&*doc).unwrap_or_default();
                    let _ = std::fs::write(&autosave_path, json);
                }
            }
            glib::ControlFlow::Continue
        },
    );
    
    *self.autosave_source.borrow_mut() = Some(source_id);
}
```

### Autosave Path

```rust
fn autosave_path_for(original: &Path) -> PathBuf {
    let dir = ProjectDirs::from("net", "canfar", "Verbinal")
        .map(|dirs| dirs.data_dir().join("autosave"))
        .unwrap_or_else(|| PathBuf::from("/tmp/verbinal_autosave"));
    std::fs::create_dir_all(&dir).ok();
    
    // Use a hash of the original path as the autosave filename
    let hash = format!("{:x}", md5_or_simple_hash(original.to_string_lossy().as_bytes()));
    dir.join(format!("{}.ipynb", hash))
}
```

### Recovery

On startup (or when opening a file), check if an autosave file exists for the path. If it does and is newer than the original file:

1. Show dialog: "An autosave recovery was found for this notebook. Would you like to recover it?"
2. If yes: load the autosave file instead of the original.
3. If no: delete the autosave file and load the original.

### Cleanup

On normal close (user closes tab or saves), delete the autosave file for that notebook.

## Undo/Redo

### State Snapshots

Track structural changes (cell add, delete, move, type change) with full cell state snapshots.

```rust
pub struct UndoStack {
    undo: Vec<CellSnapshot>,
    redo: Vec<CellSnapshot>,
    max_levels: usize,              // Default: 50
}

#[derive(Clone)]
pub struct CellSnapshot {
    pub cells: Vec<NotebookCell>,   // Full copy of all cells
    pub active_index: usize,        // Which cell was active
    pub description: String,        // e.g., "Delete cell", "Add cell below"
}

impl UndoStack {
    pub fn push(&mut self, snapshot: CellSnapshot) {
        self.undo.push(snapshot);
        if self.undo.len() > self.max_levels {
            self.undo.remove(0);
        }
        self.redo.clear();  // Clear redo stack on new action
    }
    
    pub fn undo(&mut self) -> Option<CellSnapshot> {
        let snapshot = self.undo.pop()?;
        // Push current state to redo before restoring
        // (caller must provide current state)
        Some(snapshot)
    }
    
    pub fn redo(&mut self) -> Option<CellSnapshot> {
        self.redo.pop()
    }
}
```

Push a snapshot before every structural change:
- Insert cell (A/B keys)
- Delete cell (DD key)
- Cut cell (X key)
- Change cell type (M/Y keys)
- Move cell (future drag-and-drop)

Text editing within cells is NOT tracked in the undo stack (the `gtk::TextBuffer` has its own undo).

## Settings

Notebook-specific settings stored in the main `AppConfig`:

```rust
pub struct NotebookSettings {
    pub font_size: u32,             // Default: 13
    pub tab_size: u32,              // Default: 4
    pub word_wrap: bool,            // Default: true
    pub autosave_interval_secs: u32, // Default: 30
    pub execution_timeout_secs: u32, // Default: 300 (5 minutes)
    pub python_path: Option<String>, // Default: None (auto-discover)
}
```

These settings are editable in the Settings page under a new "Notebook" preferences group.

## Dependency Scanner

When opening a notebook, scan all code cells for import statements:

```rust
fn extract_imports(cells: &[NotebookCell]) -> Vec<String> {
    let mut imports = Vec::new();
    for cell in cells {
        if cell.cell_type != "code" { continue; }
        let source = cell.source.join("");
        for line in source.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("import ") {
                // "import numpy" -> "numpy"
                // "import numpy as np" -> "numpy"
                let pkg = trimmed.strip_prefix("import ").unwrap()
                    .split_whitespace().next().unwrap()
                    .split('.').next().unwrap();
                imports.push(pkg.to_string());
            } else if trimmed.starts_with("from ") {
                // "from matplotlib import pyplot" -> "matplotlib"
                let pkg = trimmed.strip_prefix("from ").unwrap()
                    .split_whitespace().next().unwrap()
                    .split('.').next().unwrap();
                imports.push(pkg.to_string());
            }
        }
    }
    imports.sort();
    imports.dedup();
    imports
}
```

Check which imports are not installed by running `{python} -c "import {pkg}"` for each. For missing packages, show an info bar at the top of the notebook: `"Missing packages: numpy, matplotlib. [Install]"`. The Install button runs `%pip install numpy matplotlib` in a new cell.

## Recent Notebooks

Track recently opened notebooks in a persistent JSON file.

```rust
pub struct RecentNotebook {
    pub path: String,
    pub name: String,
    pub opened_at: String,          // ISO 8601 timestamp
}
```

Storage: `{ProjectDirs::data_dir()}/recent_notebooks.json`. Maximum 15 entries. MRU order (most recent first). On open, add/move to front. On display, filter out entries where the file no longer exists.

Display in the NotebookTabHost empty state as a list of clickable `adw::ActionRow` items.

## Module Files to Create

| File | Purpose |
|------|---------|
| `src/models/notebook_document.rs` | `NotebookDocument`, `NotebookCell`, `CellOutput`, `OutputData` structs |
| `src/services/kernel_service.rs` | `LocalKernelService`, Python subprocess management, protocol handling |
| `src/services/notebook_store.rs` | Recent notebooks persistence |
| `src/ui/notebook_host.rs` | `NotebookTabHost` -- multi-tab container, toolbar, empty state |
| `src/ui/notebook_page.rs` | `NotebookPage` -- per-tab cell list, keyboard shortcuts, undo/redo |
| `src/ui/notebook_cell.rs` | `CodeCell`, `MarkdownCell` UI widgets |
| `src/helpers/python_discovery.rs` | Python interpreter search and validation |
| `src/helpers/notebook_parser.rs` | Load/save .ipynb, .py, .md files |
| `data/kernel_harness.py` | Embedded Python harness script (also stored as `include_str!` in Rust) |

Update `src/models/mod.rs`, `src/services/mod.rs`, `src/helpers/mod.rs`, `src/ui/mod.rs` to include new modules. Replace the placeholder page in `main_window.rs` with the real `NotebookTabHost`.

## GTK4/Adwaita Widget Mapping

| Concept | Widget |
|---------|--------|
| Tab container | `gtk::Notebook` with closable tabs |
| Cell list | `gtk::ListBox` inside `gtk::ScrolledWindow` |
| Code editor | `gtk::TextView` with monospace font and custom tags |
| Markdown preview | `gtk::Label` with Pango markup |
| Output area | `gtk::Box` vertical with dynamic children |
| Image output | `gtk::Picture` |
| Table output | `gtk::Grid` or `gtk::ColumnView` |
| Error output | `gtk::TextView` with colored text tags |
| Kernel status | `gtk::Label` in toolbar |
| Missing packages bar | `adw::Banner` or `gtk::InfoBar` |
| Settings | `adw::PreferencesGroup` entries in Settings page |

## Error Handling

- **Python not found**: Show persistent warning in toolbar. Disable Run/Execute buttons. Allow editing but not execution.
- **Kernel crash**: Set state to `Error`, show toast `"Kernel died unexpectedly"`. Offer restart button.
- **Execution timeout**: After `execution_timeout_secs`, kill the cell execution. Show error output `"Execution timed out after {N} seconds"`.
- **Invalid .ipynb JSON**: Show error dialog with parse error details. Do not open the file.
- **File write error on save**: Show toast with error message. Do not mark as saved.
- **Missing packages**: Non-blocking info bar. User can dismiss or click Install.
