//! The main notebook editing page for Verbinal.
//!
//! [`NotebookPage`] manages a list of [`CellWidget`]s, synchronises them with
//! a [`NotebookDocument`], and drives the [`LocalKernelService`] for execution.
//!
//! # Thread safety note
//!
//! The kernel subprocess is I/O-bound and must be awaited on the tokio runtime
//! (via `services.spawn`).  Because `services.spawn` requires `Send + 'static`,
//! the `LocalKernelService` is wrapped in an `Arc<tokio::sync::Mutex<>>` so it
//! can be moved into tokio tasks safely.  All GTK widget state remains
//! `Rc<RefCell<>>` on the GLib main thread.

use crate::helpers::notebook_parser;
use crate::helpers::notebook_undo::UndoRedoStack;
use crate::models::notebook_document::{CellOutput, CellSource, NotebookCell, NotebookDocument};
use crate::services::kernel_service::{KernelState, LocalKernelService};
use crate::services::notebook_settings_service::NotebookSettingsService;
use crate::state::AppServices;
use crate::ui::notebook_cell::{CellWidget, CodeCellWidget, MarkdownCellWidget};
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

thread_local! {
    /// Process-wide (GTK main-thread) cell clipboard for command-mode
    /// Copy (`C`) / Paste-below (`V`), shared across every open notebook tab —
    /// the equivalent of the reference's `static NotebookCell? _clipboardCell`.
    static CELL_CLIPBOARD: RefCell<Option<NotebookCell>> = const { RefCell::new(None) };
}

// ---------------------------------------------------------------------------
// Interaction mode
// ---------------------------------------------------------------------------

/// Notebook interaction mode, mirroring Jupyter's modal editing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellMode {
    /// Command mode: keyboard shortcuts act on cells as a whole.
    Command,
    /// Edit mode: keyboard input goes into the active cell's text view.
    Edit,
}

// ---------------------------------------------------------------------------
// NotebookPage
// ---------------------------------------------------------------------------

/// The main notebook editing widget.
///
/// Holds the [`NotebookDocument`] in memory, renders each cell as a
/// [`CellWidget`], and handles execution via a [`LocalKernelService`].
pub struct NotebookPage {
    /// Root widget exposed to the tab host.
    widget: gtk::Box,
    /// Scrollable list of cell rows.
    cell_list: gtk::ListBox,
    /// Parallel Vec of cell widgets (indexed identically to `document.cells`).
    cell_widgets: Rc<RefCell<Vec<CellWidget>>>,
    /// In-memory notebook document.
    document: Rc<RefCell<NotebookDocument>>,
    /// Index of the currently-active cell.
    active_cell: Rc<RefCell<usize>>,
    /// Current interaction mode.
    mode: Rc<RefCell<CellMode>>,
    /// The Python kernel — wrapped in Arc+Mutex so it can cross tokio task
    /// boundaries inside `services.spawn(...)`.
    kernel: Arc<tokio::sync::Mutex<LocalKernelService>>,
    /// File path if the notebook was loaded from or saved to disk.
    pub file_path: Rc<RefCell<Option<PathBuf>>>,
    /// Whether the document has unsaved changes.
    pub modified: Rc<RefCell<bool>>,
    /// Kernel status label shown in the toolbar area.
    pub kernel_status: gtk::Label,
    /// Optional callback invoked whenever the kernel state changes.
    /// The callback receives a short state keyword: "idle", "busy", "starting", "dead", "error".
    on_state_changed: crate::ui::CallbackSlot<dyn Fn(&str)>,
    /// Optional callback fired when the document transitions to modified — the host
    /// uses it to add a `*` to the tab title and notify the autosave timer.
    on_modified: RefCell<Option<Rc<dyn Fn()>>>,
    /// Undo/redo history of whole-document snapshots. Pushed before every
    /// structural cell mutation; drives `undo()` / `redo()`.
    undo_stack: Rc<RefCell<UndoRedoStack>>,
    /// Code-cell font size (points) — used to size tab stops. Font rendering
    /// itself is applied globally by the tab host via a shared CSS provider.
    editor_font_size: Cell<u32>,
    /// Code-cell tab width in spaces.
    editor_tab_size: Cell<u32>,
    /// Whether code cells wrap long lines.
    editor_word_wrap: Cell<bool>,
    /// Soft execution-timeout threshold in seconds (`0` = never warn). Read
    /// from `NotebookSettings.execution_timeout_secs`; when a running cell
    /// exceeds it we surface a non-fatal warning offering Interrupt — the
    /// kernel is never hard-killed (mirrors `RunSelectedCellAsync`).
    exec_timeout_secs: Cell<u32>,
    /// Re-entrancy guard so a second Run-All can't interleave with the first
    /// (mirrors the reference `_runAllInProgress`).
    run_all_in_progress: Cell<bool>,
    /// App services (needed to bridge tokio → glib).
    services: Arc<AppServices>,
}

impl NotebookPage {
    // ── Constructors ──────────────────────────────────────────────────────────

    /// Create a new, empty notebook page with a single empty code cell.
    pub fn new(services: Arc<AppServices>, python_path: PathBuf) -> Rc<Self> {
        let doc = NotebookDocument::create_empty();
        Self::from_document(services, python_path, doc, None)
    }

    /// Create a notebook page pre-populated from `doc`.
    pub fn load_from_document(
        services: Arc<AppServices>,
        python_path: PathBuf,
        doc: NotebookDocument,
        path: Option<PathBuf>,
    ) -> Rc<Self> {
        Self::from_document(services, python_path, doc, path)
    }

    // ── Private constructor ───────────────────────────────────────────────────

    fn from_document(
        services: Arc<AppServices>,
        python_path: PathBuf,
        doc: NotebookDocument,
        path: Option<PathBuf>,
    ) -> Rc<Self> {
        // ── root layout ──────────────────────────────────────────────────────
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        widget.set_vexpand(true);
        widget.set_hexpand(true);

        // ── kernel status bar ────────────────────────────────────────────────
        let status_bar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        status_bar.set_margin_start(12);
        status_bar.set_margin_end(12);
        status_bar.set_margin_top(4);
        status_bar.set_margin_bottom(4);

        let kernel_status = gtk::Label::new(Some(crate::tr_en!("Kernel: not started")));
        kernel_status.add_css_class("dim-label");
        kernel_status.add_css_class("caption");
        kernel_status.set_halign(gtk::Align::Start);
        status_bar.append(&kernel_status);

        widget.append(&status_bar);
        widget.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

        // ── scrolled list ────────────────────────────────────────────────────
        let scrolled = gtk::ScrolledWindow::new();
        scrolled.set_vexpand(true);
        scrolled.set_hexpand(true);
        scrolled.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);

        let cell_list = gtk::ListBox::new();
        cell_list.set_selection_mode(gtk::SelectionMode::None);
        cell_list.add_css_class("notebook-cell-list");
        cell_list.set_hexpand(true);

        scrolled.set_child(Some(&cell_list));
        widget.append(&scrolled);

        // ── assemble page ────────────────────────────────────────────────────
        let kernel = Arc::new(tokio::sync::Mutex::new(LocalKernelService::new(
            python_path,
        )));

        // Editor preferences (font/tab/wrap) come from the persisted notebook
        // settings. Font rendering is applied globally by the host; here we
        // only need the values to size tab stops and pick the wrap mode.
        let editor = NotebookSettingsService::new().load();

        let page = Rc::new(NotebookPage {
            widget,
            cell_list,
            cell_widgets: Rc::new(RefCell::new(Vec::new())),
            document: Rc::new(RefCell::new(doc)),
            active_cell: Rc::new(RefCell::new(0)),
            mode: Rc::new(RefCell::new(CellMode::Command)),
            kernel,
            file_path: Rc::new(RefCell::new(path)),
            modified: Rc::new(RefCell::new(false)),
            kernel_status,
            on_state_changed: RefCell::new(None),
            on_modified: RefCell::new(None),
            undo_stack: Rc::new(RefCell::new(UndoRedoStack::new())),
            editor_font_size: Cell::new(editor.font_size),
            editor_tab_size: Cell::new(editor.tab_size),
            editor_word_wrap: Cell::new(editor.word_wrap),
            exec_timeout_secs: Cell::new(editor.execution_timeout_secs),
            run_all_in_progress: Cell::new(false),
            services,
        });

        // Populate cells from document
        page.rebuild_cell_list();

        // Set up keyboard controller on the cell list
        page.setup_keyboard_shortcuts();

        // Start the kernel
        {
            let page = page.clone();
            glib::spawn_future_local(async move {
                page.start_kernel().await;
            });
        }

        page
    }

    // ── Public API ────────────────────────────────────────────────────────────

    /// Return the root widget.
    pub fn widget(&self) -> &gtk::Box {
        &self.widget
    }

    /// Execute the cell at `index` (must be a code cell).
    ///
    /// Fire-and-forget entry point used by the run button, Ctrl+Enter, and
    /// Shift+Enter; drives the shared awaitable
    /// [`run_cell_async`](Self::run_cell_async).
    pub fn run_cell(self: &Rc<Self>, index: usize) {
        let page = self.clone();
        glib::spawn_future_local(async move {
            page.run_cell_async(index).await;
        });
    }

    /// Read the kernel's current lifecycle state off the tokio runtime.
    async fn kernel_state(self: &Rc<Self>) -> KernelState {
        let kernel = self.kernel.clone();
        self.services
            .spawn(async move {
                let k = kernel.lock().await;
                k.state().clone()
            })
            .await
    }

    /// Execute the cell at `index`, **awaiting** completion.
    ///
    /// Returns `true` while the kernel remains usable afterwards, or `false`
    /// if the kernel died / errored — so sequential callers like
    /// [`run_all`](Self::run_all) can stop early. Mirrors the reference
    /// `RunSelectedCellAsync`, including the soft, non-fatal execution-timeout
    /// warning (the kernel is never hard-killed on timeout).
    async fn run_cell_async(self: &Rc<Self>, index: usize) -> bool {
        let doc_len = self.document.borrow().cells.len();
        if index >= doc_len {
            return true;
        }

        let cell_type = self.document.borrow().cells[index].cell_type.clone();
        if cell_type != "code" {
            return true;
        }

        // Read current source from widget
        let source = {
            let widgets = self.cell_widgets.borrow();
            widgets
                .get(index)
                .map(|w| w.get_source())
                .unwrap_or_default()
        };

        // Mark cell as executing
        {
            let widgets = self.cell_widgets.borrow();
            if let Some(CellWidget::Code(code)) = widgets.get(index) {
                code.set_executing(true);
            }
        }

        self.update_kernel_status_label("Kernel: busy");

        // ── Soft execution-timeout warning ────────────────────────────────
        // A configurable, NON-FATAL warning: if the cell is still running
        // after `execution_timeout_secs`, surface a hint to Interrupt —
        // without killing the kernel or forcing an Error state (mirrors the
        // reference `RunSelectedCellAsync` soft timeout). `0` disables it.
        let done = Rc::new(Cell::new(false));
        let timeout_secs = self.exec_timeout_secs.get();
        if timeout_secs > 0 {
            let page = self.clone();
            let done = done.clone();
            glib::spawn_future_local(async move {
                glib::timeout_future(std::time::Duration::from_secs(timeout_secs as u64)).await;
                if !done.get() {
                    page.update_kernel_status_label(&format!(
                        "Kernel: busy — cell running over {timeout_secs}s (press I,I to Interrupt)"
                    ));
                }
            });
        }

        // Clone what we need to move into the async block
        let services = self.services.clone();
        let kernel = self.kernel.clone();
        let code_str = source.clone();
        let page = self.clone();

        // ── Live output ──────────────────────────────────────────────────
        // The kernel runs on the tokio pool and cannot touch widgets, so it
        // publishes each output through a channel that the GTK main loop drains.
        // Without this a cell that prints as it works showed NOTHING until it
        // finished — indistinguishable, to the user, from a hung kernel.
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<serde_json::Value>();
        {
            let page = self.clone();
            glib::spawn_future_local(async move {
                // Clear once, on the first output: re-running a cell should not
                // blank its previous result until there is something to replace it.
                let mut cleared = false;
                while let Some(raw) = rx.recv().await {
                    let Ok(output) = serde_json::from_value::<CellOutput>(raw) else {
                        continue;
                    };
                    let widgets = page.cell_widgets.borrow();
                    if let Some(CellWidget::Code(code)) = widgets.get(index) {
                        if !cleared {
                            code.clear_outputs();
                            cleared = true;
                        }
                        code.append_output(&output);
                    }
                }
            });
        }

        // Run the blocking kernel call on the tokio thread pool
        let result = services
            .spawn(async move {
                let mut k = kernel.lock().await;
                k.execute_streaming(&code_str, tx).await
            })
            .await;

        // Execution finished — stop the soft-timeout warning from firing.
        done.set(true);

        match result {
            Ok((raw_outputs, exec_count)) => {
                // Parse raw JSON outputs into CellOutput
                let outputs: Vec<CellOutput> = raw_outputs
                    .iter()
                    .filter_map(|v| serde_json::from_value(v.clone()).ok())
                    .collect();

                // Update document
                {
                    let mut doc = page.document.borrow_mut();
                    if let Some(cell) = doc.cells.get_mut(index) {
                        cell.outputs = outputs.clone();
                        cell.execution_count = Some(exec_count);
                        cell.source = CellSource::Single(source);
                    }
                }

                // Update widget
                {
                    let widgets = page.cell_widgets.borrow();
                    if let Some(CellWidget::Code(code)) = widgets.get(index) {
                        code.set_executing(false);
                        code.set_execution_count(exec_count);
                        code.set_outputs(&outputs);
                    }
                }

                page.mark_modified();
                page.update_kernel_status_label("Kernel: idle");
                true
            }
            Err(e) => {
                let error_output = CellOutput::Error {
                    ename: "KernelError".to_string(),
                    evalue: e.clone(),
                    traceback: vec![],
                };
                {
                    let widgets = page.cell_widgets.borrow();
                    if let Some(CellWidget::Code(code)) = widgets.get(index) {
                        code.set_executing(false);
                        code.set_outputs(&[error_output]);
                    }
                }
                page.update_kernel_status_label(&format!("Kernel: error — {e}"));
                false
            }
        }
    }

    /// Execute all code cells in order.
    ///
    /// Runs cells sequentially, **awaiting** each cell's execution before
    /// starting the next, and stops early if the kernel dies. Guarded so a
    /// second Run-All can't overlap the first. Mirrors the reference
    /// `RunAllCellsAsync`.
    pub fn run_all(self: &Rc<Self>) {
        // Re-entrancy guard: a second Run-All would interleave with the first.
        if self.run_all_in_progress.get() {
            return;
        }
        self.run_all_in_progress.set(true);

        let page = self.clone();
        glib::spawn_future_local(async move {
            // Auto-start a dead kernel before the sweep (mirrors RunAllCellsAsync).
            if matches!(page.kernel_state().await, KernelState::Dead) {
                page.start_kernel().await;
            }

            let mut i = 0;
            loop {
                // Re-read the length each iteration — cells may be edited
                // between awaits (the reference re-evaluates `Cells.Count`).
                let count = page.document.borrow().cells.len();
                if i >= count {
                    break;
                }
                let is_code = page
                    .document
                    .borrow()
                    .cells
                    .get(i)
                    .map(|c| c.cell_type == "code")
                    .unwrap_or(false);
                if is_code {
                    page.set_active_cell(i);
                    // AWAIT this cell before starting the next one.
                    let kernel_ok = page.run_cell_async(i).await;
                    if !kernel_ok {
                        break; // kernel died / errored — stop early
                    }
                }
                i += 1;
            }

            page.run_all_in_progress.set(false);
        });
    }

    /// Insert a new cell of `cell_type` ("code" or "markdown") at `index`.
    pub fn insert_cell(self: &Rc<Self>, index: usize, cell_type: &str) {
        self.push_undo();
        self.insert_cell_inner(index, cell_type);
    }

    /// Insert a cell without recording an undo point (callers that already
    /// pushed an undo snapshot use this to avoid a spurious extra entry).
    fn insert_cell_inner(self: &Rc<Self>, index: usize, cell_type: &str) {
        let new_cell = NotebookCell {
            cell_type: cell_type.to_string(),
            source: CellSource::Single(String::new()),
            outputs: Vec::new(),
            execution_count: None,
            id: Some(NotebookDocument::generate_cell_id()),
            metadata: serde_json::Map::new(),
        };

        let insert_at = index.min(self.document.borrow().cells.len());
        self.document.borrow_mut().cells.insert(insert_at, new_cell);
        self.mark_modified();
        self.rebuild_cell_list();
        self.set_active_cell(insert_at);
    }

    /// Delete the cell at `index`.
    pub fn delete_cell(self: &Rc<Self>, index: usize) {
        let len = self.document.borrow().cells.len();
        if len == 0 || index >= len {
            return;
        }

        self.push_undo();
        self.document.borrow_mut().cells.remove(index);

        // Ensure at least one cell remains (Jupyter inserts a fresh code cell).
        // Use the no-undo insert so the single delete stays one undo step.
        if self.document.borrow().cells.is_empty() {
            self.insert_cell_inner(0, "code");
            return;
        }

        self.mark_modified();
        self.rebuild_cell_list();

        let new_active = index
            .saturating_sub(1)
            .min(self.document.borrow().cells.len() - 1);
        self.set_active_cell(new_active);
    }

    /// Move a cell from position `from` to position `to`.
    pub fn move_cell(self: &Rc<Self>, from: usize, to: usize) {
        let len = self.document.borrow().cells.len();
        if from >= len || to >= len || from == to {
            return;
        }
        self.push_undo();
        let cell = self.document.borrow_mut().cells.remove(from);
        self.document.borrow_mut().cells.insert(to, cell);
        self.mark_modified();
        self.rebuild_cell_list();
        self.set_active_cell(to);
    }

    /// Sync widget sources back to the document and save to disk.
    pub fn save(&self) -> Result<(), String> {
        // Sync widget text → document
        {
            let widgets = self.cell_widgets.borrow();
            let mut doc = self.document.borrow_mut();
            for (i, widget) in widgets.iter().enumerate() {
                if let Some(cell) = doc.cells.get_mut(i) {
                    cell.source = CellSource::Single(widget.get_source());
                }
            }
        }

        let path = self
            .file_path
            .borrow()
            .clone()
            .ok_or_else(|| "No file path set".to_string())?;

        notebook_parser::save_notebook(&self.document.borrow(), &path)?;
        *self.modified.borrow_mut() = false;
        Ok(())
    }

    /// Interrupt the running kernel (sends SIGINT).
    pub fn interrupt_kernel(&self) {
        // Use try_lock — if the kernel is in a tokio task, we cannot lock it
        // here, but interrupt() is signal-based and fine to call from any thread
        // through the Arc.
        if let Ok(mut k) = self.kernel.try_lock() {
            k.interrupt();
        }
    }

    /// Restart the kernel.
    pub fn restart_kernel(self: &Rc<Self>) {
        let page = self.clone();
        glib::spawn_future_local(async move {
            page.update_kernel_status_label("Kernel: restarting…");
            let kernel = page.kernel.clone();
            let result = page
                .services
                .spawn(async move {
                    let mut k = kernel.lock().await;
                    k.restart().await
                })
                .await;
            match result {
                Ok(()) => page.update_kernel_status_label("Kernel: idle"),
                Err(e) => page.update_kernel_status_label(&format!("Kernel: error — {}", e)),
            }
        });
    }

    /// Return the tab title (filename or "Untitled Notebook").
    pub fn tab_title(&self) -> String {
        self.file_path
            .borrow()
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "Untitled Notebook".to_string())
    }

    /// Return `true` if there are unsaved changes.
    pub fn is_modified(&self) -> bool {
        *self.modified.borrow()
    }

    /// Mark the document dirty and notify the host (tab `*` marker + autosave).
    fn mark_modified(&self) {
        *self.modified.borrow_mut() = true;
        if let Some(cb) = self.on_modified.borrow().as_ref() {
            cb();
        }
    }

    /// Register a callback fired when the document becomes modified.
    pub fn set_on_modified(&self, cb: impl Fn() + 'static) {
        *self.on_modified.borrow_mut() = Some(Rc::new(cb));
    }

    /// Explicitly set the modified flag (e.g. mark a recovered notebook dirty).
    pub fn set_modified(&self, value: bool) {
        *self.modified.borrow_mut() = value;
        if value {
            if let Some(cb) = self.on_modified.borrow().as_ref() {
                cb();
            }
        }
    }

    /// A display title for the tab — the file stem, or "Untitled".
    pub fn title(&self) -> String {
        self.file_path
            .borrow()
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "Untitled".to_string())
    }

    /// A consistent snapshot of the document with the live cell text synced in —
    /// used by autosave (does not touch `file_path` or the modified flag).
    pub fn snapshot_document(&self) -> NotebookDocument {
        {
            let widgets = self.cell_widgets.borrow();
            let mut doc = self.document.borrow_mut();
            for (i, widget) in widgets.iter().enumerate() {
                if let Some(cell) = doc.cells.get_mut(i) {
                    cell.source = CellSource::Single(widget.get_source());
                }
            }
        }
        self.document.borrow().clone()
    }

    // ── Private helpers ───────────────────────────────────────────────────────

    /// Clear and re-populate the `cell_list` from the current document.
    fn rebuild_cell_list(self: &Rc<Self>) {
        // Remove all existing rows
        while let Some(child) = self.cell_list.first_child() {
            self.cell_list.remove(&child);
        }

        let doc = self.document.borrow();
        let mut widgets: Vec<CellWidget> = Vec::new();

        for (i, cell) in doc.cells.iter().enumerate() {
            let cell_widget = if cell.cell_type == "markdown" {
                let md = MarkdownCellWidget::new();
                md.set_source(&cell.source.joined());
                CellWidget::Markdown(md)
            } else {
                let code = CodeCellWidget::new();
                code.set_source(&cell.source.joined());
                if let Some(n) = cell.execution_count {
                    code.set_execution_count(n);
                }
                code.set_outputs(&cell.outputs);
                self.apply_prefs_to_code_view(code.text_view());
                CellWidget::Code(code)
            };

            // Wrap in a ListBoxRow
            let row = gtk::ListBoxRow::new();
            row.set_selectable(false);
            row.set_activatable(false);
            row.set_child(Some(cell_widget.widget()));
            self.cell_list.append(&row);

            // Connect run button for code cells
            if let CellWidget::Code(ref code) = cell_widget {
                let page = self.clone();
                let idx = i;
                code.run_button().connect_clicked(move |_| {
                    page.run_cell(idx);
                });
            }

            // Click to activate cell
            {
                let gesture = gtk::GestureClick::new();
                let page = self.clone();
                let idx = i;
                gesture.connect_pressed(move |_, _, _, _| {
                    page.set_active_cell(idx);
                    *page.mode.borrow_mut() = CellMode::Edit;
                });
                cell_widget.widget().add_controller(gesture);
            }

            widgets.push(cell_widget);
        }

        drop(doc);
        *self.cell_widgets.borrow_mut() = widgets;

        // Re-apply active highlight
        let active = *self.active_cell.borrow();
        self.apply_active_highlight(active);
    }

    /// Set the active cell index and update UI highlights.
    fn set_active_cell(&self, index: usize) {
        let prev = *self.active_cell.borrow();
        let len = self.cell_widgets.borrow().len();
        let clamped = index.min(len.saturating_sub(1));

        *self.active_cell.borrow_mut() = clamped;
        self.apply_active_highlight(prev);
        self.apply_active_highlight(clamped);
    }

    fn apply_active_highlight(&self, index: usize) {
        let widgets = self.cell_widgets.borrow();
        let active = *self.active_cell.borrow();
        if let Some(w) = widgets.get(index) {
            w.set_active(index == active);
        }
    }

    /// Update the kernel status label text and notify observers.
    fn update_kernel_status_label(&self, text: &str) {
        self.kernel_status.set_label(text);

        // Derive a short state keyword for observers
        let state_kw = if text.contains("idle") {
            "idle"
        } else if text.contains("busy") {
            "busy"
        } else if text.contains("starting") || text.contains("restarting") {
            "starting"
        } else if text.contains("failed") || text.contains("error") {
            "error"
        } else if text.contains("not started") || text.contains("dead") {
            "dead"
        } else {
            "unknown"
        };

        if let Some(cb) = self.on_state_changed.borrow().as_ref() {
            cb(state_kw);
        }
    }

    /// Register a callback to be invoked whenever the kernel state changes.
    pub fn set_on_kernel_state_changed(&self, cb: Rc<dyn Fn(&str) + 'static>) {
        *self.on_state_changed.borrow_mut() = Some(cb);
    }

    /// Return the current kernel status label text.
    pub fn current_kernel_status_label(&self) -> String {
        self.kernel_status.label().to_string()
    }

    /// Return the index of the currently-active cell.
    pub fn active_cell_index(&self) -> usize {
        *self.active_cell.borrow()
    }

    /// Return the number of cells in the document.
    pub fn cell_count(&self) -> usize {
        self.document.borrow().cells.len()
    }

    /// Clear all cell outputs (both in the document and the widgets).
    pub fn clear_all_outputs(self: &Rc<Self>) {
        {
            let mut doc = self.document.borrow_mut();
            for cell in &mut doc.cells {
                cell.outputs.clear();
                cell.execution_count = None;
            }
        }
        // Clear widget outputs in place (avoids full rebuild)
        for widget in self.cell_widgets.borrow().iter() {
            if let CellWidget::Code(code) = widget {
                code.set_outputs(&[]);
                code.clear_execution_count();
            }
        }
        self.mark_modified();
    }

    /// Save the notebook to a new path ("Save As").
    pub fn save_as(&self, path: PathBuf) -> Result<(), String> {
        *self.file_path.borrow_mut() = Some(path);
        self.save()
    }

    /// Start the Python kernel subprocess.
    async fn start_kernel(self: &Rc<Self>) {
        self.update_kernel_status_label("Kernel: starting…");

        let kernel = self.kernel.clone();
        let result = self
            .services
            .spawn(async move {
                let mut k = kernel.lock().await;
                k.start().await
            })
            .await;

        match result {
            Ok(()) => self.update_kernel_status_label("Kernel: idle"),
            Err(e) => self.update_kernel_status_label(&format!("Kernel: failed — {}", e)),
        }
    }

    // ── Keyboard shortcuts ────────────────────────────────────────────────────

    fn setup_keyboard_shortcuts(self: &Rc<Self>) {
        let controller = gtk::EventControllerKey::new();
        let page = self.clone();

        controller.connect_key_pressed(move |_, key, _code, modifier| {
            let mode = *page.mode.borrow();
            let ctrl = modifier.contains(gtk::gdk::ModifierType::CONTROL_MASK);
            let shift = modifier.contains(gtk::gdk::ModifierType::SHIFT_MASK);

            match mode {
                CellMode::Command => match key {
                    // Enter → switch to edit mode
                    gtk::gdk::Key::Return if !ctrl && !shift => {
                        *page.mode.borrow_mut() = CellMode::Edit;
                        glib::Propagation::Stop
                    }
                    // A → insert above
                    gtk::gdk::Key::a if !ctrl => {
                        let idx = *page.active_cell.borrow();
                        page.insert_cell(idx, "code");
                        glib::Propagation::Stop
                    }
                    // B → insert below
                    gtk::gdk::Key::b if !ctrl => {
                        let idx = *page.active_cell.borrow() + 1;
                        page.insert_cell(idx, "code");
                        glib::Propagation::Stop
                    }
                    // M → change to markdown
                    gtk::gdk::Key::m if !ctrl => {
                        let idx = *page.active_cell.borrow();
                        page.change_cell_type(idx, "markdown");
                        glib::Propagation::Stop
                    }
                    // Y → change to code
                    gtk::gdk::Key::y if !ctrl => {
                        let idx = *page.active_cell.borrow();
                        page.change_cell_type(idx, "code");
                        glib::Propagation::Stop
                    }
                    // Up / K → navigate up
                    gtk::gdk::Key::Up | gtk::gdk::Key::k if !ctrl => {
                        let idx = *page.active_cell.borrow();
                        if idx > 0 {
                            page.set_active_cell(idx - 1);
                        }
                        glib::Propagation::Stop
                    }
                    // Down / J → navigate down
                    gtk::gdk::Key::Down | gtk::gdk::Key::j if !ctrl => {
                        let idx = *page.active_cell.borrow();
                        let max = page.cell_widgets.borrow().len().saturating_sub(1);
                        if idx < max {
                            page.set_active_cell(idx + 1);
                        }
                        glib::Propagation::Stop
                    }
                    // Ctrl+Enter → run current cell
                    gtk::gdk::Key::Return if ctrl => {
                        let idx = *page.active_cell.borrow();
                        page.run_cell(idx);
                        glib::Propagation::Stop
                    }
                    // Z → undo
                    gtk::gdk::Key::z if !ctrl && !shift => {
                        page.undo();
                        glib::Propagation::Stop
                    }
                    // Shift+Z → redo
                    gtk::gdk::Key::Z if !ctrl => {
                        page.redo();
                        glib::Propagation::Stop
                    }
                    // C → copy the active cell
                    gtk::gdk::Key::c if !ctrl => {
                        page.copy_active_cell();
                        glib::Propagation::Stop
                    }
                    // V → paste the clipboard cell below
                    gtk::gdk::Key::v if !ctrl => {
                        page.paste_cell_below();
                        glib::Propagation::Stop
                    }
                    // Shift+M → merge the active cell with the one below
                    gtk::gdk::Key::M if !ctrl => {
                        page.merge_cell_below();
                        glib::Propagation::Stop
                    }
                    _ => glib::Propagation::Proceed,
                },

                CellMode::Edit => match key {
                    // Escape → command mode
                    gtk::gdk::Key::Escape => {
                        let idx = *page.active_cell.borrow();
                        let widgets = page.cell_widgets.borrow();
                        if let Some(CellWidget::Markdown(md)) = widgets.get(idx) {
                            md.enter_preview_mode();
                        }
                        drop(widgets);
                        *page.mode.borrow_mut() = CellMode::Command;
                        glib::Propagation::Stop
                    }
                    // Ctrl+Enter → run and stay
                    gtk::gdk::Key::Return if ctrl && !shift => {
                        let idx = *page.active_cell.borrow();
                        page.run_cell(idx);
                        glib::Propagation::Stop
                    }
                    // Shift+Enter → run and advance
                    gtk::gdk::Key::Return if shift => {
                        let idx = *page.active_cell.borrow();
                        page.run_cell(idx);
                        let max = page.cell_widgets.borrow().len().saturating_sub(1);
                        if idx < max {
                            page.set_active_cell(idx + 1);
                        } else {
                            // Last cell: append a fresh empty code cell and
                            // advance to it (mirrors RunSelectedAndAdvanceAsync's
                            // AddCellBelow("code")). `insert_cell` selects it.
                            page.insert_cell(idx + 1, "code");
                        }
                        glib::Propagation::Stop
                    }
                    // Ctrl+Shift+Minus → split the cell at the cursor. Shift maps
                    // the `-` key to `_` on most layouts, so accept both.
                    gtk::gdk::Key::minus | gtk::gdk::Key::underscore if ctrl && shift => {
                        page.split_active_cell();
                        glib::Propagation::Stop
                    }
                    _ => glib::Propagation::Proceed,
                },
            }
        });

        self.widget.add_controller(controller);
        self.widget.set_focusable(true);
        self.widget.set_can_focus(true);
    }

    /// Replace the source text of the cell at `index` (used by the live MCP
    /// `edit_cell` tool). Goes through the same undo + `mark_modified` +
    /// `rebuild_cell_list` path as the interactive editors.
    pub fn set_cell_source(self: &Rc<Self>, index: usize, source: &str) {
        if index >= self.document.borrow().cells.len() {
            return;
        }
        self.push_undo();
        {
            let mut doc = self.document.borrow_mut();
            if let Some(cell) = doc.cells.get_mut(index) {
                cell.source = CellSource::Single(source.to_string());
            }
        }
        self.mark_modified();
        self.rebuild_cell_list();
        self.set_active_cell(index);
    }

    /// Start the Python kernel and await readiness (used by the live MCP
    /// `start_kernel` tool). Wraps the private constructor-time starter.
    pub async fn start_kernel_now(self: &Rc<Self>) {
        self.start_kernel().await;
    }

    /// Change the cell type at `index` and rebuild the list.
    pub fn change_cell_type(self: &Rc<Self>, index: usize, new_type: &str) {
        // No-op (and no undo point) when the type is already what was asked for.
        {
            let doc = self.document.borrow();
            match doc.cells.get(index) {
                Some(cell) if cell.cell_type == new_type => return,
                None => return,
                _ => {}
            }
        }

        let current_source = {
            let widgets = self.cell_widgets.borrow();
            widgets
                .get(index)
                .map(|w| w.get_source())
                .unwrap_or_default()
        };

        self.push_undo();

        {
            let mut doc = self.document.borrow_mut();
            if let Some(cell) = doc.cells.get_mut(index) {
                cell.cell_type = new_type.to_string();
                cell.source = CellSource::Single(current_source);
                cell.outputs.clear();
                cell.execution_count = None;
            }
        }

        self.mark_modified();
        self.rebuild_cell_list();
        self.set_active_cell(index);
    }

    // ── Undo / Redo ───────────────────────────────────────────────────────────

    /// Snapshot the current document (with live widget text synced in) onto the
    /// undo stack. Must be called *before* any structural cell mutation
    /// (add / delete / move / type-change / split / merge / paste).
    fn push_undo(&self) {
        let snapshot = self.snapshot_document();
        self.undo_stack.borrow_mut().push(snapshot);
    }

    /// Restore the previous document state, if any.
    pub fn undo(self: &Rc<Self>) {
        let current = self.snapshot_document();
        let restored = self.undo_stack.borrow_mut().undo(current);
        if let Some(doc) = restored {
            self.restore_document(doc);
        }
    }

    /// Re-apply the most recently undone state, if any.
    pub fn redo(self: &Rc<Self>) {
        let current = self.snapshot_document();
        let restored = self.undo_stack.borrow_mut().redo(current);
        if let Some(doc) = restored {
            self.restore_document(doc);
        }
    }

    /// Replace the live document with `doc`, clamp the active cell, rebuild the
    /// cell list, and mark the notebook modified (shared by undo/redo).
    fn restore_document(self: &Rc<Self>, doc: NotebookDocument) {
        let len = doc.cells.len();
        *self.document.borrow_mut() = doc;
        // Keep the active index inside the restored bounds.
        if len > 0 {
            let clamped = (*self.active_cell.borrow()).min(len - 1);
            *self.active_cell.borrow_mut() = clamped;
        } else {
            *self.active_cell.borrow_mut() = 0;
        }
        self.rebuild_cell_list();
        self.mark_modified();
    }

    // ── Cell operations (split / merge / copy / paste) ────────────────────────

    /// Split the active cell at the text cursor into two cells of the same
    /// type. The text before the cursor stays in the current cell; the text
    /// after moves into a new cell inserted directly below.
    pub fn split_active_cell(self: &Rc<Self>) {
        let idx = *self.active_cell.borrow();
        if idx >= self.document.borrow().cells.len() {
            return;
        }

        // Read the current source and cursor char-offset from the active widget.
        let (full, cursor) = {
            let widgets = self.cell_widgets.borrow();
            let (source, offset) = match widgets.get(idx) {
                Some(CellWidget::Code(c)) => {
                    (c.get_source(), c.text_view().buffer().cursor_position())
                }
                Some(CellWidget::Markdown(m)) => {
                    (m.get_source(), m.text_view().buffer().cursor_position())
                }
                None => return,
            };
            (source, offset.max(0) as usize)
        };

        // Convert the char offset to a byte index so we can split the String.
        let byte_idx = full
            .char_indices()
            .nth(cursor)
            .map(|(i, _)| i)
            .unwrap_or_else(|| full.len());
        let (top, bottom) = full.split_at(byte_idx);
        let top = top.to_string();
        let bottom = bottom.to_string();

        self.push_undo();

        let cell_type = self.document.borrow().cells[idx].cell_type.clone();
        {
            let mut doc = self.document.borrow_mut();
            if let Some(cell) = doc.cells.get_mut(idx) {
                cell.source = CellSource::Single(top);
            }
            let new_cell = NotebookCell {
                cell_type,
                source: CellSource::Single(bottom),
                outputs: Vec::new(),
                execution_count: None,
                id: Some(NotebookDocument::generate_cell_id()),
                metadata: serde_json::Map::new(),
            };
            doc.cells.insert(idx + 1, new_cell);
        }

        self.mark_modified();
        self.rebuild_cell_list();
        self.set_active_cell(idx + 1);
    }

    /// Merge the active cell with the cell directly below it, joining their
    /// sources with a newline. No-op when the active cell is the last one.
    pub fn merge_cell_below(self: &Rc<Self>) {
        let idx = *self.active_cell.borrow();
        if idx + 1 >= self.document.borrow().cells.len() {
            return;
        }

        // Pick up the latest edited text from both widgets before merging.
        let (top_src, below_src) = {
            let widgets = self.cell_widgets.borrow();
            let a = widgets.get(idx).map(|w| w.get_source()).unwrap_or_default();
            let b = widgets
                .get(idx + 1)
                .map(|w| w.get_source())
                .unwrap_or_default();
            (a, b)
        };

        self.push_undo();

        {
            let mut doc = self.document.borrow_mut();
            let merged = format!("{top_src}\n{below_src}");
            if let Some(cell) = doc.cells.get_mut(idx) {
                cell.source = CellSource::Single(merged);
            }
            doc.cells.remove(idx + 1);
        }

        self.mark_modified();
        self.rebuild_cell_list();
        self.set_active_cell(idx);
    }

    /// Copy the active cell (type + current source) to the shared clipboard.
    pub fn copy_active_cell(&self) {
        let idx = *self.active_cell.borrow();
        let source = {
            let widgets = self.cell_widgets.borrow();
            match widgets.get(idx) {
                Some(w) => w.get_source(),
                None => return,
            }
        };
        let cell_type = match self.document.borrow().cells.get(idx) {
            Some(c) => c.cell_type.clone(),
            None => return,
        };
        let clip = NotebookCell {
            cell_type,
            source: CellSource::Single(source),
            outputs: Vec::new(),
            execution_count: None,
            id: Some(NotebookDocument::generate_cell_id()),
            metadata: serde_json::Map::new(),
        };
        CELL_CLIPBOARD.with(|c| *c.borrow_mut() = Some(clip));
    }

    /// Paste the clipboard cell directly below the active cell. No-op when the
    /// clipboard is empty.
    pub fn paste_cell_below(self: &Rc<Self>) {
        let clip = CELL_CLIPBOARD.with(|c| c.borrow().clone());
        let Some(clip) = clip else {
            return;
        };

        self.push_undo();

        let insert_at = {
            let len = self.document.borrow().cells.len();
            (*self.active_cell.borrow() + 1).min(len)
        };
        let new_cell = NotebookCell {
            cell_type: clip.cell_type.clone(),
            source: CellSource::Single(clip.source.joined()),
            outputs: Vec::new(),
            execution_count: None,
            id: Some(NotebookDocument::generate_cell_id()),
            metadata: serde_json::Map::new(),
        };
        self.document.borrow_mut().cells.insert(insert_at, new_cell);

        self.mark_modified();
        self.rebuild_cell_list();
        self.set_active_cell(insert_at);
    }

    // ── Editor preferences ─────────────────────────────────────────────────────

    /// Apply editor preferences (font size, tab width, word wrap) live. Font
    /// rendering is handled globally by the tab host's CSS provider; here we
    /// update the tab stops (sized from the font) and wrap mode on every code
    /// cell's text view.
    pub fn apply_editor_settings(&self, font_size: u32, tab_size: u32, word_wrap: bool) {
        self.editor_font_size.set(font_size);
        self.editor_tab_size.set(tab_size);
        self.editor_word_wrap.set(word_wrap);
        let widgets = self.cell_widgets.borrow();
        for w in widgets.iter() {
            if let CellWidget::Code(code) = w {
                self.apply_prefs_to_code_view(code.text_view());
            }
        }
    }

    /// Set wrap mode + tab stops on a single code-cell text view from the
    /// current stored preferences.
    fn apply_prefs_to_code_view(&self, tv: &gtk::TextView) {
        let wrap = if self.editor_word_wrap.get() {
            gtk::WrapMode::Word
        } else {
            gtk::WrapMode::None
        };
        tv.set_wrap_mode(wrap);

        // Tab stops: monospace character width ≈ 0.6 × font size (px). We lay
        // down a run of evenly-spaced left tab stops so any tab lands on a
        // `tab_size`-column boundary.
        let tab_size = self.editor_tab_size.get().max(1) as f64;
        let char_px = (self.editor_font_size.get() as f64 * 0.6).max(4.0);
        let step = (char_px * tab_size).round().max(1.0) as i32;
        let stops: i32 = 48;
        let mut tabs = gtk::pango::TabArray::new(stops, true);
        for i in 0..stops {
            tabs.set_tab(i, gtk::pango::TabAlign::Left, step * (i + 1));
        }
        tv.set_tabs(&tabs);
    }
}
