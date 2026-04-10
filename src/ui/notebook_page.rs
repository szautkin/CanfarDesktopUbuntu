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
use crate::models::notebook_document::{CellOutput, CellSource, NotebookCell, NotebookDocument};
use crate::services::kernel_service::LocalKernelService;
use crate::state::AppServices;
use crate::ui::notebook_cell::{CellWidget, CodeCellWidget, MarkdownCellWidget};
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

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
    on_state_changed: RefCell<Option<Rc<dyn Fn(&str)>>>,
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

        let kernel_status = gtk::Label::new(Some("Kernel: not started"));
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
    pub fn run_cell(self: &Rc<Self>, index: usize) {
        let doc_len = self.document.borrow().cells.len();
        if index >= doc_len {
            return;
        }

        let cell_type = self.document.borrow().cells[index].cell_type.clone();
        if cell_type != "code" {
            return;
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

        // Clone what we need to move into the async block
        let services = self.services.clone();
        let kernel = self.kernel.clone();
        let code_str = source.clone();
        let page = self.clone();

        glib::spawn_future_local(async move {
            // Run the blocking kernel call on the tokio thread pool
            let result = services
                .spawn(async move {
                    let mut k = kernel.lock().await;
                    k.execute(&code_str).await
                })
                .await;

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

                    *page.modified.borrow_mut() = true;
                }
                Err(e) => {
                    let error_output = CellOutput::Error {
                        ename: "KernelError".to_string(),
                        evalue: e.clone(),
                        traceback: vec![],
                    };
                    let widgets = page.cell_widgets.borrow();
                    if let Some(CellWidget::Code(code)) = widgets.get(index) {
                        code.set_executing(false);
                        code.set_outputs(&[error_output]);
                    }
                }
            }

            // Refresh kernel status after execution
            let kernel_state = {
                let k = page.kernel.try_lock();
                k.map(|k| format!("{:?}", k.state())).unwrap_or_else(|_| "busy".to_string())
            };
            let _ = kernel_state; // just update the label
            page.update_kernel_status_label("Kernel: idle");
        });
    }

    /// Execute all code cells in order.
    pub fn run_all(self: &Rc<Self>) {
        let count = self.document.borrow().cells.len();
        let page = self.clone();
        glib::spawn_future_local(async move {
            for i in 0..count {
                let cell_type = page
                    .document
                    .borrow()
                    .cells
                    .get(i)
                    .map(|c| c.cell_type.clone());
                if cell_type.as_deref() == Some("code") {
                    page.run_cell(i);
                    // Small yield so GTK can process events between cells
                    glib::timeout_future(std::time::Duration::from_millis(50)).await;
                }
            }
        });
    }

    /// Insert a new cell of `cell_type` ("code" or "markdown") at `index`.
    pub fn insert_cell(self: &Rc<Self>, index: usize, cell_type: &str) {
        let new_cell = NotebookCell {
            cell_type: cell_type.to_string(),
            source: CellSource::Single(String::new()),
            outputs: Vec::new(),
            execution_count: None,
            id: Some(NotebookDocument::generate_cell_id()),
            metadata: serde_json::Map::new(),
        };

        let insert_at = index.min(self.document.borrow().cells.len());
        self.document
            .borrow_mut()
            .cells
            .insert(insert_at, new_cell);
        *self.modified.borrow_mut() = true;
        self.rebuild_cell_list();
        self.set_active_cell(insert_at);
    }

    /// Delete the cell at `index`.
    pub fn delete_cell(self: &Rc<Self>, index: usize) {
        let len = self.document.borrow().cells.len();
        if len == 0 || index >= len {
            return;
        }

        self.document.borrow_mut().cells.remove(index);

        // Ensure at least one cell remains
        if self.document.borrow().cells.is_empty() {
            self.insert_cell(0, "code");
            return;
        }

        *self.modified.borrow_mut() = true;
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
        let cell = self.document.borrow_mut().cells.remove(from);
        self.document.borrow_mut().cells.insert(to, cell);
        *self.modified.borrow_mut() = true;
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
                Err(e) => page
                    .update_kernel_status_label(&format!("Kernel: error — {}", e)),
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
        *self.modified.borrow_mut() = true;
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
            Err(e) => self
                .update_kernel_status_label(&format!("Kernel: failed — {}", e)),
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
                    // Z → undo placeholder
                    gtk::gdk::Key::z if !ctrl => glib::Propagation::Stop,
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
                        }
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

    /// Change the cell type at `index` and rebuild the list.
    fn change_cell_type(self: &Rc<Self>, index: usize, new_type: &str) {
        let current_source = {
            let widgets = self.cell_widgets.borrow();
            widgets
                .get(index)
                .map(|w| w.get_source())
                .unwrap_or_default()
        };

        {
            let mut doc = self.document.borrow_mut();
            if let Some(cell) = doc.cells.get_mut(index) {
                if cell.cell_type != new_type {
                    cell.cell_type = new_type.to_string();
                    cell.source = CellSource::Single(current_source);
                    cell.outputs.clear();
                }
            }
        }

        *self.modified.borrow_mut() = true;
        self.rebuild_cell_list();
        self.set_active_cell(index);
    }
}
