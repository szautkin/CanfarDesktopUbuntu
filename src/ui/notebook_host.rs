//! Tab host for multiple open notebooks in Verbinal.
//!
//! [`NotebookTabHost`] wraps a `gtk::Notebook` (tab strip) and manages
//! [`NotebookPage`] instances.  When no notebooks are open it shows a welcome
//! empty-state page with a list of recently-opened files.

use crate::helpers::notebook_parser;
use crate::helpers::python_discovery;
use crate::models::notebook_document::{CellOutput, NotebookDocument};
use crate::models::notebook_settings::NotebookSettings;
use crate::services::notebook_settings_service::NotebookSettingsService;
use crate::services::notebook_store::NotebookStore;
use crate::state::AppServices;
use crate::ui::notebook_page::NotebookPage;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// NotebookTabHost
// ---------------------------------------------------------------------------

/// Top-level widget that owns the toolbar, the tab strip, and all open
/// [`NotebookPage`]s.
pub struct NotebookTabHost {
    /// Root widget exposed to `main_window`.
    widget: gtk::Box,
    /// GTK Notebook used as the tab container.
    tab_view: gtk::Notebook,
    /// Parallel Vec of page wrappers (matches tab order).
    pages: Rc<RefCell<Vec<Rc<NotebookPage>>>>,
    /// Tab title labels, parallel to `pages` (for the `*` unsaved marker).
    tab_labels: Rc<RefCell<Vec<gtk::Label>>>,
    /// Per-page autosave checkpoint paths, parallel to `pages`.
    autosave_paths: Rc<RefCell<Vec<PathBuf>>>,
    /// Monotonic id for never-saved notebooks (stable autosave keys).
    untitled_seq: std::cell::Cell<u64>,
    /// Persistent recent-notebooks store.
    store: Rc<NotebookStore>,
    /// Resolved Python interpreter path (may be `None` if Python not found).
    python_path: Rc<RefCell<Option<PathBuf>>>,
    /// App services (for kernel bridging).
    services: Arc<AppServices>,
    /// Stack that switches between the empty-state and the tab notebook.
    content_stack: gtk::Stack,
    /// Status dot for kernel (coloured circle).
    kernel_dot: gtk::Label,
    /// Python path label in toolbar.
    python_label: gtk::Label,
    /// Toast overlay for surfacing errors to the user.
    toast_overlay: adw::ToastOverlay,
    /// The cell/exec toolbar (kept so `show_toolbar` can toggle its visibility).
    toolbar: gtk::Box,
    /// Persisted notebook preferences (font/tab/wrap/python path/…).
    settings: Rc<RefCell<NotebookSettings>>,
    /// JSON store backing [`Self::settings`].
    settings_service: Rc<NotebookSettingsService>,
    /// Global CSS provider that applies the configured code-cell font size to
    /// every open notebook's code cells (`.code-cell-source`).
    font_provider: gtk::CssProvider,
}

/// Push the open notebook tabs + active index into the MCP view state.
///
/// See `fits_viewer::publish_fits_tabs` — `list_open_tabs` had no publisher, so
/// it reported nothing regardless of what was open. An unsaved notebook has no
/// path, so it is listed by its title rather than dropped.
fn publish_notebook_tabs(tab_view: &gtk::Notebook, pages: &Rc<RefCell<Vec<Rc<NotebookPage>>>>) {
    let paths: Vec<String> = pages
        .borrow()
        .iter()
        .map(|p| {
            p.file_path
                .borrow()
                .as_ref()
                .map(|x| x.display().to_string())
                .unwrap_or_else(|| p.title())
        })
        .collect();
    let active = tab_view
        .current_page()
        .map(|i| i as usize)
        .filter(|i| *i < paths.len());
    crate::mcp::view_state::set_open_notebooks(paths, active);
}

impl NotebookTabHost {
    /// Create a new, empty tab host and resolve Python.
    pub fn new(services: Arc<AppServices>) -> Rc<Self> {
        // ── Notebook settings ────────────────────────────────────────────────
        let settings_service = Rc::new(NotebookSettingsService::new());
        let settings = settings_service.load();

        // ── Python discovery ─────────────────────────────────────────────────
        // Prefer the explicitly-configured interpreter, falling back to
        // auto-discovery (find_python already tries the configured path first).
        let python_path = python_discovery::find_python(settings.python_path.as_deref());
        let python_label_text = python_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| crate::tr_en!("Python not found").to_string());

        // ── Root ─────────────────────────────────────────────────────────────
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        widget.set_vexpand(true);
        widget.set_hexpand(true);

        // ── Toolbar ──────────────────────────────────────────────────────────
        // GNOME HIG: frequent actions inline (file ops, add cell, run/stop);
        // structural and kernel housekeeping grouped behind labelled menu buttons.
        let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        toolbar.add_css_class("toolbar");

        // A left-aligned, menu-like popover item.
        let menu_item = |label: &str, tooltip: &str| {
            let btn = gtk::Button::new();
            let l = gtk::Label::new(Some(label));
            l.set_xalign(0.0);
            btn.set_child(Some(&l));
            btn.add_css_class("flat");
            btn.set_tooltip_text(Some(tooltip));
            btn
        };

        // File group: New, Open, Save, Save As
        let new_btn = gtk::Button::from_icon_name("document-new-symbolic");
        new_btn.add_css_class("flat");
        new_btn.set_tooltip_text(Some(crate::tr_en!("New Notebook (Ctrl+N)")));
        toolbar.append(&new_btn);

        let open_btn = gtk::Button::from_icon_name("document-open-symbolic");
        open_btn.add_css_class("flat");
        open_btn.set_tooltip_text(Some(crate::tr_en!("Open Notebook (Ctrl+O)")));
        toolbar.append(&open_btn);

        let save_btn = gtk::Button::from_icon_name("document-save-symbolic");
        save_btn.add_css_class("flat");
        save_btn.set_tooltip_text(Some(crate::tr_en!("Save Notebook (Ctrl+S)")));
        toolbar.append(&save_btn);

        let save_as_btn = gtk::Button::from_icon_name("document-save-as-symbolic");
        save_as_btn.add_css_class("flat");
        save_as_btn.set_tooltip_text(Some(crate::tr_en!("Save As… (Ctrl+Shift+S)")));
        toolbar.append(&save_as_btn);

        toolbar.append(&gtk::Separator::new(gtk::Orientation::Vertical));

        // Add-cell group (frequent): Code, Markdown (icon + label)
        let add_code_btn = gtk::Button::new();
        let add_code_content = adw::ButtonContent::new();
        add_code_content.set_icon_name("list-add-symbolic");
        add_code_content.set_label(crate::tr_en!("Code"));
        add_code_btn.set_child(Some(&add_code_content));
        add_code_btn.add_css_class("flat");
        add_code_btn.set_tooltip_text(Some(crate::tr_en!("Add Code Cell")));
        toolbar.append(&add_code_btn);

        let add_md_btn = gtk::Button::new();
        let add_md_content = adw::ButtonContent::new();
        add_md_content.set_icon_name("format-text-rich-symbolic");
        add_md_content.set_label(crate::tr_en!("Markdown"));
        add_md_btn.set_child(Some(&add_md_content));
        add_md_btn.add_css_class("flat");
        add_md_btn.set_tooltip_text(Some(crate::tr_en!("Add Markdown Cell")));
        toolbar.append(&add_md_btn);

        // Run group (frequent): Run Cell, Run All, Interrupt
        let run_cell_btn = gtk::Button::from_icon_name("media-playback-start-symbolic");
        run_cell_btn.add_css_class("flat");
        run_cell_btn.set_tooltip_text(Some(crate::tr_en!("Run Cell (Ctrl+Enter)")));
        toolbar.append(&run_cell_btn);

        let run_all_btn = gtk::Button::new();
        let run_all_content = adw::ButtonContent::new();
        run_all_content.set_icon_name("media-seek-forward-symbolic");
        run_all_content.set_label(crate::tr_en!("Run All"));
        run_all_btn.set_child(Some(&run_all_content));
        run_all_btn.add_css_class("flat");
        run_all_btn.set_tooltip_text(Some(crate::tr_en!("Run all cells")));
        toolbar.append(&run_all_btn);

        let interrupt_btn = gtk::Button::from_icon_name("media-playback-stop-symbolic");
        interrupt_btn.add_css_class("flat");
        interrupt_btn.set_tooltip_text(Some(crate::tr_en!("Interrupt kernel")));
        toolbar.append(&interrupt_btn);

        // ── "Cell" menu: structural operations ───────────────────────────────
        let move_up_btn = menu_item(crate::tr_en!("Move up"), crate::tr_en!("Move Cell Up"));
        let move_down_btn = menu_item(crate::tr_en!("Move down"), crate::tr_en!("Move Cell Down"));
        let delete_cell_btn = menu_item(crate::tr_en!("Delete cell"), crate::tr_en!("Delete Cell"));
        let split_btn = menu_item(
            crate::tr_en!("Split at cursor"),
            crate::tr_en!("Split Cell at Cursor (Ctrl+Shift+Minus)"),
        );
        let merge_btn = menu_item(
            crate::tr_en!("Merge with below"),
            crate::tr_en!("Merge Cell Below (Shift+M)"),
        );

        let cell_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
        cell_box.set_margin_start(6);
        cell_box.set_margin_end(6);
        cell_box.set_margin_top(6);
        cell_box.set_margin_bottom(6);
        cell_box.set_size_request(180, -1);
        cell_box.append(&move_up_btn);
        cell_box.append(&move_down_btn);
        cell_box.append(&split_btn);
        cell_box.append(&merge_btn);
        cell_box.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        cell_box.append(&delete_cell_btn);

        let cell_pop = gtk::Popover::new();
        cell_pop.set_child(Some(&cell_box));
        let cell_menu_btn = gtk::MenuButton::new();
        cell_menu_btn.set_label(crate::tr_en!("Cell"));
        cell_menu_btn.add_css_class("flat");
        cell_menu_btn.set_tooltip_text(Some(crate::tr_en!(
            "Cell operations — move, split, merge, delete"
        )));
        cell_menu_btn.set_popover(Some(&cell_pop));
        toolbar.append(&cell_menu_btn);

        // ── "Kernel" menu: restart + clear outputs ───────────────────────────
        let restart_btn = menu_item(
            crate::tr_en!("Restart kernel"),
            crate::tr_en!("Restart kernel"),
        );
        let clear_outputs_btn = menu_item(
            crate::tr_en!("Clear all outputs"),
            crate::tr_en!("Clear All Outputs"),
        );

        let kernel_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
        kernel_box.set_margin_start(6);
        kernel_box.set_margin_end(6);
        kernel_box.set_margin_top(6);
        kernel_box.set_margin_bottom(6);
        kernel_box.set_size_request(180, -1);
        kernel_box.append(&restart_btn);
        kernel_box.append(&clear_outputs_btn);

        let kernel_pop = gtk::Popover::new();
        kernel_pop.set_child(Some(&kernel_box));
        let kernel_menu_btn = gtk::MenuButton::new();
        kernel_menu_btn.set_label(crate::tr_en!("Kernel"));
        kernel_menu_btn.add_css_class("flat");
        kernel_menu_btn.set_tooltip_text(Some(crate::tr_en!("Kernel — restart, clear outputs")));
        kernel_menu_btn.set_popover(Some(&kernel_pop));
        toolbar.append(&kernel_menu_btn);

        // Menus close on activation (standard menu behaviour).
        for (btn, pop) in [
            (&move_up_btn, &cell_pop),
            (&move_down_btn, &cell_pop),
            (&split_btn, &cell_pop),
            (&merge_btn, &cell_pop),
            (&delete_cell_btn, &cell_pop),
            (&restart_btn, &kernel_pop),
            (&clear_outputs_btn, &kernel_pop),
        ] {
            let pop = pop.clone();
            btn.connect_clicked(move |_| pop.popdown());
        }

        // Spacer
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        toolbar.append(&spacer);

        // Status indicators at the trailing edge: python path, kernel dot, settings.
        let python_label = gtk::Label::new(Some(&python_label_text));
        python_label.add_css_class("dim-label");
        python_label.add_css_class("caption");
        toolbar.append(&python_label);

        let kernel_dot = gtk::Label::new(Some("●"));
        kernel_dot.add_css_class("kernel-dot");
        kernel_dot.add_css_class("dim-label");
        kernel_dot.set_tooltip_text(Some(crate::tr_en!("Kernel status: not started")));
        toolbar.append(&kernel_dot);

        let settings_btn = gtk::Button::from_icon_name("emblem-system-symbolic");
        settings_btn.add_css_class("flat");
        settings_btn.set_tooltip_text(Some(crate::tr_en!("Notebook Settings")));
        toolbar.append(&settings_btn);

        widget.append(&toolbar);
        widget.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

        // ── Content stack: empty state or notebook tabs ───────────────────────
        let content_stack = gtk::Stack::new();
        content_stack.set_transition_type(gtk::StackTransitionType::None);
        content_stack.set_vexpand(true);
        content_stack.set_hexpand(true);

        // Empty state
        let empty_page = build_empty_state();
        content_stack.add_named(&empty_page, Some("empty"));

        // Notebook (tab strip)
        let tab_view = gtk::Notebook::new();
        tab_view.set_scrollable(true);
        tab_view.set_show_border(false);
        tab_view.set_vexpand(true);
        tab_view.set_hexpand(true);
        content_stack.add_named(&tab_view, Some("tabs"));

        content_stack.set_visible_child_name("empty");

        // Wrap content stack in a toast overlay so errors are visible in-widget.
        let toast_overlay = adw::ToastOverlay::new();
        toast_overlay.set_child(Some(&content_stack));
        toast_overlay.set_vexpand(true);
        toast_overlay.set_hexpand(true);
        widget.append(&toast_overlay);

        let store = Rc::new(NotebookStore::new());

        // Apply the persisted "show toolbar" preference at startup.
        toolbar.set_visible(settings.show_toolbar);

        // Global font provider: applies the configured code-cell font size to
        // every notebook's code cells (`.code-cell-source`). Added to the
        // display once and re-loaded when the font size changes.
        let font_provider = gtk::CssProvider::new();
        apply_font_css(&font_provider, settings.font_size);
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &font_provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }

        let host = Rc::new(NotebookTabHost {
            widget,
            tab_view,
            pages: Rc::new(RefCell::new(Vec::new())),
            tab_labels: Rc::new(RefCell::new(Vec::new())),
            autosave_paths: Rc::new(RefCell::new(Vec::new())),
            untitled_seq: std::cell::Cell::new(0),
            store,
            python_path: Rc::new(RefCell::new(python_path)),
            services,
            content_stack,
            kernel_dot,
            python_label,
            toast_overlay,
            toolbar,
            settings: Rc::new(RefCell::new(settings)),
            settings_service,
            font_provider,
        });

        // Autosave tick: every 30s, checkpoint any dirty notebook (atomic write).
        {
            let h = host.clone();
            glib::timeout_add_local(std::time::Duration::from_secs(30), move || {
                h.autosave_tick();
                glib::ControlFlow::Continue
            });
        }

        // Crash recovery: surface orphaned autosave checkpoints from a prior session.
        {
            let h = host.clone();
            glib::spawn_future_local(async move {
                h.check_recovery().await;
            });
        }

        // Wire toolbar buttons
        {
            let h = host.clone();
            new_btn.connect_clicked(move |_| {
                h.trigger_new();
            });
        }
        {
            let h = host.clone();
            open_btn.connect_clicked(move |btn| {
                let h = h.clone();
                let parent = btn.clone().upcast::<gtk::Widget>();
                glib::spawn_future_local(async move {
                    h.open_file_dialog(&parent).await;
                });
            });
        }
        {
            let h = host.clone();
            save_btn.connect_clicked(move |_| {
                h.trigger_save();
            });
        }
        {
            let h = host.clone();
            save_as_btn.connect_clicked(move |btn| {
                let h = h.clone();
                let parent = btn.clone().upcast::<gtk::Widget>();
                glib::spawn_future_local(async move {
                    h.trigger_save_as_widget(&parent).await;
                });
            });
        }
        {
            let h = host.clone();
            add_code_btn.connect_clicked(move |_| {
                if let Some(page) = h.current_page() {
                    let idx = page.active_cell_index() + 1;
                    page.insert_cell(idx, "code");
                }
            });
        }
        {
            let h = host.clone();
            add_md_btn.connect_clicked(move |_| {
                if let Some(page) = h.current_page() {
                    let idx = page.active_cell_index() + 1;
                    page.insert_cell(idx, "markdown");
                }
            });
        }
        {
            let h = host.clone();
            move_up_btn.connect_clicked(move |_| {
                if let Some(page) = h.current_page() {
                    let i = page.active_cell_index();
                    if i > 0 {
                        page.move_cell(i, i - 1);
                    }
                }
            });
        }
        {
            let h = host.clone();
            move_down_btn.connect_clicked(move |_| {
                if let Some(page) = h.current_page() {
                    let i = page.active_cell_index();
                    if i + 1 < page.cell_count() {
                        page.move_cell(i, i + 1);
                    }
                }
            });
        }
        {
            let h = host.clone();
            delete_cell_btn.connect_clicked(move |_| {
                if let Some(page) = h.current_page() {
                    page.delete_cell(page.active_cell_index());
                }
            });
        }
        {
            let h = host.clone();
            run_cell_btn.connect_clicked(move |_| {
                if let Some(page) = h.current_page() {
                    page.run_cell(page.active_cell_index());
                }
            });
        }
        {
            let h = host.clone();
            run_all_btn.connect_clicked(move |_| {
                if let Some(page) = h.current_page() {
                    page.run_all();
                }
            });
        }
        {
            let h = host.clone();
            clear_outputs_btn.connect_clicked(move |_| {
                if let Some(page) = h.current_page() {
                    page.clear_all_outputs();
                }
            });
        }
        {
            let h = host.clone();
            restart_btn.connect_clicked(move |_| {
                if let Some(page) = h.current_page() {
                    page.restart_kernel();
                }
            });
        }
        {
            let h = host.clone();
            interrupt_btn.connect_clicked(move |_| {
                if let Some(page) = h.current_page() {
                    page.interrupt_kernel();
                }
            });
        }
        {
            let h = host.clone();
            split_btn.connect_clicked(move |_| {
                if let Some(page) = h.current_page() {
                    page.split_active_cell();
                }
            });
        }
        {
            let h = host.clone();
            merge_btn.connect_clicked(move |_| {
                if let Some(page) = h.current_page() {
                    page.merge_cell_below();
                }
            });
        }
        {
            let h = host.clone();
            settings_btn.connect_clicked(move |btn| {
                let parent = btn.clone().upcast::<gtk::Widget>();
                h.open_settings_dialog(&parent);
            });
        }

        // Ctrl+comma reopens settings even when the toolbar is hidden, so the
        // "Show toolbar" preference can never lock the user out of settings.
        {
            let controller = gtk::EventControllerKey::new();
            let h = host.clone();
            controller.connect_key_pressed(move |_, key, _code, modifier| {
                if modifier.contains(gtk::gdk::ModifierType::CONTROL_MASK)
                    && key == gtk::gdk::Key::comma
                {
                    let parent = h.widget.clone().upcast::<gtk::Widget>();
                    h.open_settings_dialog(&parent);
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            });
            host.widget.add_controller(controller);
        }

        // Wire tab-switch to update the kernel dot from the newly-active tab
        {
            let h = host.clone();
            host.tab_view.connect_switch_page(move |_, _, _| {
                if let Some(page) = h.current_page() {
                    let label = page.current_kernel_status_label();
                    h.update_kernel_dot_from_label(&label);
                } else {
                    h.update_kernel_dot("dead");
                }
                // The active index moved, so the MCP tab snapshot is stale.
                publish_notebook_tabs(&h.tab_view, &h.pages);
            });
        }

        // Populate recent notebooks in empty state
        host.populate_recent_list(&empty_page);

        host
    }

    // ── Public API ────────────────────────────────────────────────────────────

    /// Return the root widget.
    pub fn widget(&self) -> &gtk::Box {
        &self.widget
    }

    /// Handle a live MCP viewer command (`op` + JSON `args`) against the open
    /// notebooks. Runs on the GTK main thread. Read ops return a snapshot;
    /// mutation ops go through the page's existing mutators and return the
    /// resulting notebook state. Ops target the active notebook unless an
    /// `index`/`path`/`notebook` selector picks another open tab.
    pub async fn handle_viewer_command(
        self: &Rc<Self>,
        op: &str,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        use serde_json::json;

        match op {
            "list_open_notebooks" => {
                let active = self.tab_view.current_page();
                let pages = self.pages.borrow();
                let items: Vec<serde_json::Value> = pages
                    .iter()
                    .enumerate()
                    .map(|(i, p)| {
                        let fp = p
                            .file_path
                            .borrow()
                            .as_ref()
                            .map(|x| x.display().to_string());
                        json!({
                            "notebookId": fp.clone().unwrap_or_else(|| format!("notebook-{i}")),
                            "title": p.title(),
                            "filePath": fp,
                            "isActive": active == Some(i as u32),
                            "isDirty": p.is_modified(),
                            "cellCount": p.cell_count(),
                            "kernelState": kernel_keyword(&p.current_kernel_status_label()),
                        })
                    })
                    .collect();
                Ok(json!({ "count": items.len(), "notebooks": items }))
            }

            "get_notebook" => {
                let page = self.resolve_page(args).ok_or_else(no_notebook)?;
                let id = self.page_id(&page);
                Ok(notebook_state_json(&page, &id))
            }

            "get_cell_output" => {
                let page = self.resolve_page(args).ok_or_else(no_notebook)?;
                let idx = arg_index(args, "cell")
                    .or_else(|| arg_index(args, "index"))
                    .ok_or_else(|| "cell (index) is required".to_string())?;
                let doc = page.snapshot_document();
                let cell = doc
                    .cells
                    .get(idx)
                    .ok_or_else(|| format!("cell index {idx} out of range"))?;
                let outputs: Vec<serde_json::Value> =
                    cell.outputs.iter().map(cell_output_json).collect();
                Ok(json!({
                    "index": idx,
                    "type": cell.cell_type,
                    "executionCount": cell.execution_count,
                    "outputCount": outputs.len(),
                    "outputs": outputs,
                }))
            }

            "get_kernel_state" => {
                let page = self.resolve_page(args).ok_or_else(no_notebook)?;
                let label = page.current_kernel_status_label();
                let doc = page.snapshot_document();
                let kernel_name = doc
                    .metadata
                    .kernelspec
                    .as_ref()
                    .map(|k| k.name.clone())
                    .unwrap_or_else(|| "python3".to_string());
                Ok(json!({
                    "state": kernel_keyword(&label),
                    "statusText": label,
                    "kernelName": kernel_name,
                }))
            }

            "add_cell" => {
                let page = self.resolve_page(args).ok_or_else(no_notebook)?;
                let cell_type = cell_type_arg(
                    crate::mcp::tools::arg(args, "cellType")
                        .or_else(|| crate::mcp::tools::arg(args, "type")),
                )?;
                let count = page.cell_count();
                let idx = arg_index(args, "index").unwrap_or(count).min(count);
                page.insert_cell(idx, &cell_type);
                Ok(self.state_of(&page))
            }

            "edit_cell" => {
                let page = self.resolve_page(args).ok_or_else(no_notebook)?;
                let idx =
                    arg_index(args, "index").ok_or_else(|| "index is required".to_string())?;
                let source = crate::mcp::tools::arg(args, "source")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| "source is required".to_string())?;
                if idx >= page.cell_count() {
                    return Err(format!("cell index {idx} out of range"));
                }
                page.set_cell_source(idx, source);
                Ok(self.state_of(&page))
            }

            "delete_cell" => {
                let page = self.resolve_page(args).ok_or_else(no_notebook)?;
                let idx =
                    arg_index(args, "index").ok_or_else(|| "index is required".to_string())?;
                if idx >= page.cell_count() {
                    return Err(format!("cell index {idx} out of range"));
                }
                page.delete_cell(idx);
                Ok(self.state_of(&page))
            }

            "change_cell_type" => {
                let page = self.resolve_page(args).ok_or_else(no_notebook)?;
                let idx =
                    arg_index(args, "index").ok_or_else(|| "index is required".to_string())?;
                // `cellType` FIRST — it is the name the schema advertises and
                // requires. This read `cell_type` only, so the documented call
                // was rejected and the tool was unusable as specified; the two
                // older spellings stay as tolerated aliases.
                let ct = crate::mcp::tools::arg(args, "cellType")
                    .or_else(|| crate::mcp::tools::arg(args, "cell_type"))
                    .or_else(|| crate::mcp::tools::arg(args, "type"))
                    .and_then(|v| v.as_str());
                let ct = match ct {
                    Some(s) if s.eq_ignore_ascii_case("code") => "code",
                    Some(s) if s.eq_ignore_ascii_case("markdown") => "markdown",
                    _ => return Err("cellType must be 'code' or 'markdown'".to_string()),
                };
                if idx >= page.cell_count() {
                    return Err(format!("cell index {idx} out of range"));
                }
                page.change_cell_type(idx, ct);
                Ok(self.state_of(&page))
            }

            "move_cell" => {
                let page = self.resolve_page(args).ok_or_else(no_notebook)?;
                let idx =
                    arg_index(args, "index").ok_or_else(|| "index is required".to_string())?;
                let count = page.cell_count();
                if idx >= count {
                    return Err(format!("cell index {idx} out of range"));
                }
                let to = if let Some(to) = arg_index(args, "to") {
                    to
                } else {
                    match crate::mcp::tools::arg(args, "direction").and_then(|v| v.as_str()) {
                        Some("up") => {
                            if idx == 0 {
                                return Err("cell already at top".to_string());
                            }
                            idx - 1
                        }
                        Some("down") => {
                            if idx + 1 >= count {
                                return Err("cell already at bottom".to_string());
                            }
                            idx + 1
                        }
                        _ => return Err("direction must be 'up' or 'down'".to_string()),
                    }
                };
                if to >= count {
                    return Err(format!("target index {to} out of range"));
                }
                page.move_cell(idx, to);
                Ok(self.state_of(&page))
            }

            "run_cell" => {
                let page = self.resolve_page(args).ok_or_else(no_notebook)?;
                let idx =
                    arg_index(args, "index").ok_or_else(|| "index is required".to_string())?;
                if idx >= page.cell_count() {
                    return Err(format!("cell index {idx} out of range"));
                }
                page.run_cell(idx);
                Ok(self.state_of(&page))
            }

            // Op names are the MCP tool names verbatim (see `mcp::tools::notebook`),
            // so there is one vocabulary rather than a translation table to drift.
            "run_all_cells" => {
                let page = self.resolve_page(args).ok_or_else(no_notebook)?;
                page.run_all();
                Ok(self.state_of(&page))
            }

            "clear_cell_outputs" => {
                let page = self.resolve_page(args).ok_or_else(no_notebook)?;
                page.clear_all_outputs();
                Ok(self.state_of(&page))
            }

            "start_kernel" => {
                let page = self.resolve_page(args).ok_or_else(no_notebook)?;
                page.start_kernel_now().await;
                Ok(self.state_of(&page))
            }

            "interrupt_kernel" => {
                let page = self.resolve_page(args).ok_or_else(no_notebook)?;
                page.interrupt_kernel();
                Ok(self.state_of(&page))
            }

            "restart_kernel" => {
                let page = self.resolve_page(args).ok_or_else(no_notebook)?;
                page.restart_kernel();
                Ok(self.state_of(&page))
            }

            "save_notebook" => {
                let page = self.resolve_page(args).ok_or_else(no_notebook)?;
                let path = crate::mcp::tools::arg(args, "path")
                    .and_then(|v| v.as_str())
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty());
                match path {
                    Some(p) => page.save_as(PathBuf::from(p)),
                    None => page.save(),
                }
                .map_err(|e| format!("save failed: {e}"))?;
                self.refresh_tab_title(&page);
                Ok(self.state_of(&page))
            }

            "open_notebook" => {
                let path = crate::mcp::tools::arg(args, "path")
                    .and_then(|v| v.as_str())
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| "path is required".to_string())?;
                self.load_from_path(&PathBuf::from(path));
                let page = self
                    .current_page()
                    .ok_or_else(|| "failed to open notebook".to_string())?;
                Ok(self.state_of(&page))
            }

            "create_notebook" => {
                self.trigger_new();
                let page = self
                    .current_page()
                    .ok_or_else(|| "failed to create notebook".to_string())?;
                Ok(self.state_of(&page))
            }

            other => Err(format!("notebook op '{other}' is not supported")),
        }
    }

    /// Resolve the target page: the `notebook` selector (open-tab index, id, file
    /// path, or title) when present, otherwise the active tab.
    fn resolve_page(&self, args: &serde_json::Value) -> Option<Rc<NotebookPage>> {
        if let Some(sel) = crate::mcp::tools::arg(args, "notebook").and_then(|v| v.as_str()) {
            let sel = sel.trim();
            if !sel.is_empty() {
                // Bare numeric selector → tab index.
                if let Ok(i) = sel.parse::<usize>() {
                    if let Some(p) = self.pages.borrow().get(i).cloned() {
                        return Some(p);
                    }
                }
                // "notebook-N" synthetic id → tab index.
                if let Some(i) = sel
                    .strip_prefix("notebook-")
                    .and_then(|s| s.parse::<usize>().ok())
                {
                    if let Some(p) = self.pages.borrow().get(i).cloned() {
                        return Some(p);
                    }
                }
                // Otherwise match by file path or title.
                let pages = self.pages.borrow();
                for p in pages.iter() {
                    let matches_path = p
                        .file_path
                        .borrow()
                        .as_ref()
                        .map(|x| x.display().to_string())
                        .as_deref()
                        == Some(sel);
                    if matches_path || p.title() == sel {
                        return Some(p.clone());
                    }
                }
                return None;
            }
        }
        self.current_page()
    }

    /// A stable-ish selector id for `page`: its file path, or `notebook-<index>`.
    fn page_id(&self, page: &Rc<NotebookPage>) -> String {
        if let Some(p) = page.file_path.borrow().as_ref() {
            return p.display().to_string();
        }
        let idx = self.pages.borrow().iter().position(|p| Rc::ptr_eq(p, page));
        match idx {
            Some(i) => format!("notebook-{i}"),
            None => "notebook-?".to_string(),
        }
    }

    /// Snapshot `page` into the shared notebook-state JSON shape.
    fn state_of(&self, page: &Rc<NotebookPage>) -> serde_json::Value {
        let id = self.page_id(page);
        notebook_state_json(page, &id)
    }

    /// Open a file chooser dialog and load the selected notebook.
    async fn open_file_dialog(self: Rc<Self>, parent: &gtk::Widget) {
        let root = parent.root().and_downcast::<gtk::Window>();

        let filter = gtk::FileFilter::new();
        filter.set_name(Some(crate::tr_en!("Notebooks")));
        filter.add_pattern("*.ipynb");
        filter.add_pattern("*.py");
        // Markdown too, as the reference does (`NotebookFileMode.Markdown`).
        // `load_markdown_as_notebook` has always been able to read one; the
        // filter simply never offered it, so the file was unselectable.
        filter.add_pattern("*.md");

        let filters = gtk::gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);

        let dialog = gtk::FileDialog::builder()
            .title(crate::tr_en!("Open Notebook"))
            .modal(true)
            .filters(&filters)
            .build();

        if let Ok(file) = dialog.open_future(root.as_ref()).await {
            if let Some(path) = file.path() {
                self.load_from_path(&path);
            }
        }
    }

    /// Load a notebook from the given path and open it in a new tab.
    ///
    /// Supports `.ipynb`, `.py` and `.md` files.
    pub fn load_from_path(self: &Rc<Self>, path: &Path) {
        let load_result = match path.extension().and_then(|e| e.to_str()) {
            Some("py") => notebook_parser::load_python_as_notebook(path),
            Some("md") | Some("markdown") => notebook_parser::load_markdown_as_notebook(path),
            _ => notebook_parser::load_notebook(path),
        };

        let doc = match load_result {
            Ok(d) => d,
            Err(e) => {
                self.toast_overlay
                    .add_toast(adw::Toast::new(&crate::tr_fmt!("Failed to load: {}", e)));
                return;
            }
        };

        // Record in recent store
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.display().to_string());
        let _ = self.store.add(&path.display().to_string(), &name);

        let python_path = self
            .python_path
            .borrow()
            .clone()
            .unwrap_or_else(|| PathBuf::from("/usr/bin/python3"));

        let page = NotebookPage::load_from_document(
            self.services.clone(),
            python_path.clone(),
            doc.clone(),
            Some(path.to_path_buf()),
        );

        self.add_tab(page, &name);
        self.check_dependencies(&doc, python_path);
    }

    /// Ask the interpreter which of the notebook's imports are missing, and
    /// offer to install them — the reference's `CheckDependenciesAsync`.
    ///
    /// A notebook that opens and then fails on its first cell with
    /// `ModuleNotFoundError` is a notebook the app could have warned about
    /// before the user pressed Run. The scanner half of this shipped long ago
    /// (`extract_imports`) and nothing ever called it, so the seven strings for
    /// it have been sitting translated and unused in the catalog.
    fn check_dependencies(self: &Rc<Self>, doc: &NotebookDocument, python: PathBuf) {
        use crate::helpers::dependency_scanner as deps;

        let imports = notebook_parser::extract_imports(&doc.cells);
        if imports.is_empty() {
            return;
        }

        let host = self.clone();
        glib::spawn_future_local(async move {
            let script = deps::probe_script(&imports);
            let py = python.clone();
            let probe = host
                .services
                .spawn(async move {
                    tokio::process::Command::new(&py)
                        .arg("-c")
                        .arg(script)
                        .output()
                        .await
                        .ok()
                })
                .await;
            let Some(out) = probe else { return };

            let missing: Vec<String> =
                deps::missing_from_probe(&String::from_utf8_lossy(&out.stdout))
                    .iter()
                    .map(|m| deps::pip_name(m).to_string())
                    .collect();
            if missing.is_empty() {
                return;
            }

            let listed = missing
                .iter()
                .map(|p| format!("  - {p}"))
                .collect::<Vec<_>>()
                .join("\n");
            let root = host.widget.root().and_downcast::<gtk::Window>();
            let dialog = adw::MessageDialog::new(
                root.as_ref(),
                Some(&crate::i18n::tr_args(
                    "Nb_MissingPkgTitle",
                    &[&missing.len().to_string()],
                )),
                Some(&crate::i18n::tr_args("Nb_MissingPkgBody", &[&listed])),
            );
            dialog.add_response("skip", crate::i18n::tr("Nb_SkipButton"));
            dialog.add_response("install", crate::i18n::tr("Nb_InstallAllButton"));
            dialog.set_response_appearance("install", adw::ResponseAppearance::Suggested);
            dialog.set_default_response(Some("install"));
            dialog.set_close_response("skip");
            if dialog.choose_future().await != "install" {
                return;
            }

            host.toast_overlay
                .add_toast(adw::Toast::new(&crate::i18n::tr_args(
                    "Nb_InstallingPkgs",
                    &[&missing.len().to_string()],
                )));
            let args = deps::install_args(&missing);
            let installed = host
                .services
                .spawn(async move {
                    tokio::process::Command::new(&python)
                        .args(&args)
                        .output()
                        .await
                        .map(|o| o.status.success())
                        .unwrap_or(false)
                })
                .await;

            host.toast_overlay.add_toast(adw::Toast::new(if installed {
                crate::i18n::tr("Nb_DepsAllInstalled")
            } else {
                crate::tr_en!("Install failed — see the kernel log")
            }));
        });
    }

    // ── Private helpers ───────────────────────────────────────────────────────

    /// Add a [`NotebookPage`] as a new tab.
    fn add_tab(self: &Rc<Self>, page: Rc<NotebookPage>, title: &str) {
        // Build tab header: label + close button
        let tab_box = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        let tab_label = gtk::Label::new(Some(title));
        tab_label.set_max_width_chars(20);
        tab_label.set_ellipsize(gtk::pango::EllipsizeMode::End);

        let close_btn = gtk::Button::from_icon_name("window-close-symbolic");
        close_btn.add_css_class("flat");
        close_btn.add_css_class("circular");
        close_btn.set_icon_name("window-close-symbolic");

        tab_box.append(&tab_label);
        tab_box.append(&close_btn);

        let page_widget = page.widget().clone().upcast::<gtk::Widget>();
        let tab_index = self.tab_view.append_page(&page_widget, Some(&tab_box));

        // A stable autosave key: the file path, or a per-session untitled id.
        let key = match page.file_path.borrow().as_ref() {
            Some(p) => p.to_string_lossy().to_string(),
            None => {
                let n = self.untitled_seq.get() + 1;
                self.untitled_seq.set(n);
                format!("untitled-{n}")
            }
        };
        let autosave_path = crate::helpers::notebook_autosave::autosave_path_for(&key, title);

        self.pages.borrow_mut().push(page.clone());
        self.tab_labels.borrow_mut().push(tab_label.clone());
        self.autosave_paths.borrow_mut().push(autosave_path);
        publish_notebook_tabs(&self.tab_view, &self.pages);

        // Wire kernel state callback to update the host's dot
        {
            let h = self.clone();
            page.set_on_kernel_state_changed(Rc::new(move |state| {
                h.update_kernel_dot(state);
            }));
        }

        // Show a `*` in the tab title the moment the document becomes dirty.
        {
            let h = self.clone();
            let p = page.clone();
            page.set_on_modified(move || h.refresh_tab_title(&p));
        }

        // Close button — guard unsaved changes (Save / Discard / Cancel).
        {
            let h = self.clone();
            let page = page.clone();
            close_btn.connect_clicked(move |_| {
                let h = h.clone();
                let page = page.clone();
                glib::spawn_future_local(async move {
                    if !h.confirm_close(&page).await {
                        return;
                    }
                    h.remove_page_for(&page);
                });
            });
        }

        // Switch to tabs view
        self.content_stack.set_visible_child_name("tabs");
        self.tab_view.set_current_page(Some(tab_index));
    }

    /// Update a page's tab title, adding a `*` when it has unsaved changes.
    fn refresh_tab_title(&self, page: &Rc<NotebookPage>) {
        let pages = self.pages.borrow();
        let labels = self.tab_labels.borrow();
        if let Some(idx) = pages.iter().position(|p| Rc::ptr_eq(p, page)) {
            if let Some(label) = labels.get(idx) {
                let base = page.title();
                if page.is_modified() {
                    label.set_text(&format!("{base} *"));
                } else {
                    label.set_text(&base);
                }
            }
        }
    }

    /// Ask the user before closing a modified notebook. Returns `true` to proceed
    /// with closing (saved or discarded), `false` to cancel.
    async fn confirm_close(self: &Rc<Self>, page: &Rc<NotebookPage>) -> bool {
        if !page.is_modified() {
            return true;
        }
        let root = self.widget.root().and_downcast::<gtk::Window>();
        let dialog = adw::MessageDialog::new(
            root.as_ref(),
            Some(crate::tr_en!("Save changes?")),
            Some(&crate::tr_fmt!(
                "“{}” has unsaved changes. Save them before closing?",
                page.title()
            )),
        );
        dialog.add_response("cancel", crate::tr_en!("Cancel"));
        dialog.add_response("discard", crate::tr_en!("Discard"));
        dialog.add_response("save", crate::tr_en!("Save"));
        dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);
        dialog.set_response_appearance("discard", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("save"));
        dialog.set_close_response("cancel");
        match dialog.choose_future().await.as_str() {
            "save" => {
                if page.file_path.borrow().is_none() {
                    // Never saved — must pick a path; treat cancel-of-save as cancel-close.
                    self.toast_overlay.add_toast(adw::Toast::new(crate::tr_en!(
                        "Use Save As to choose a file path, then close"
                    )));
                    false
                } else if let Err(e) = page.save() {
                    self.toast_overlay
                        .add_toast(adw::Toast::new(&crate::tr_fmt!("Save failed: {}", e)));
                    false
                } else {
                    true
                }
            }
            "discard" => true,
            _ => false, // cancel
        }
    }

    /// Remove a page's tab + tracking entries, and delete its autosave checkpoint.
    fn remove_page_for(&self, page: &Rc<NotebookPage>) {
        let idx = self.pages.borrow().iter().position(|p| Rc::ptr_eq(p, page));
        let Some(idx) = idx else { return };
        if (idx as u32) < self.tab_view.n_pages() {
            self.tab_view.remove_page(Some(idx as u32));
        }
        self.pages.borrow_mut().remove(idx);
        if idx < self.tab_labels.borrow().len() {
            self.tab_labels.borrow_mut().remove(idx);
        }
        if idx < self.autosave_paths.borrow().len() {
            let path = self.autosave_paths.borrow_mut().remove(idx);
            crate::helpers::notebook_autosave::delete_autosave(&path);
        }
        if self.pages.borrow().is_empty() {
            self.content_stack.set_visible_child_name("empty");
        }
    }

    /// Timer callback: write an autosave checkpoint for every dirty notebook.
    fn autosave_tick(&self) {
        let pages = self.pages.borrow();
        let paths = self.autosave_paths.borrow();
        for (page, path) in pages.iter().zip(paths.iter()) {
            if page.is_modified() {
                let doc = page.snapshot_document();
                let _ = crate::helpers::notebook_autosave::write_autosave(&doc, path);
            }
        }
    }

    /// On startup, offer to recover orphaned autosave checkpoints from a crash.
    async fn check_recovery(self: &Rc<Self>) {
        let orphans = crate::helpers::notebook_autosave::detect_orphans();
        if orphans.is_empty() {
            return;
        }
        let root = self.widget.root().and_downcast::<gtk::Window>();
        let dialog = adw::MessageDialog::new(
            root.as_ref(),
            Some(crate::tr_en!("Recover notebooks?")),
            Some(&crate::tr_fmt!(
                "{} unsaved notebook checkpoint(s) from a previous session were found. Recover them?",
                orphans.len()
            )),
        );
        dialog.add_response("later", crate::tr_en!("Later"));
        dialog.add_response("discard", crate::tr_en!("Discard All"));
        dialog.add_response("recover", crate::tr_en!("Recover"));
        dialog.set_response_appearance("recover", adw::ResponseAppearance::Suggested);
        dialog.set_response_appearance("discard", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("recover"));
        dialog.set_close_response("later");
        match dialog.choose_future().await.as_str() {
            "recover" => {
                let python_path = self
                    .python_path
                    .borrow()
                    .clone()
                    .unwrap_or_else(|| PathBuf::from("/usr/bin/python3"));
                for cand in &orphans {
                    if let Ok(doc) = crate::helpers::notebook_autosave::load_autosave(&cand.path) {
                        let page = NotebookPage::load_from_document(
                            self.services.clone(),
                            python_path.clone(),
                            doc,
                            None,
                        );
                        // Recovered content is unsaved by definition.
                        page.set_modified(true);
                        self.add_tab(
                            page.clone(),
                            &crate::tr_fmt!("{} (recovered)", cand.display_name),
                        );
                        self.refresh_tab_title(&page);
                    }
                    crate::helpers::notebook_autosave::discard(&cand.path);
                }
            }
            "discard" => crate::helpers::notebook_autosave::discard_all(),
            _ => {} // later: leave the checkpoints in place
        }
    }

    /// Return the currently-active [`NotebookPage`], if any.
    fn current_page(&self) -> Option<Rc<NotebookPage>> {
        let current_tab = self.tab_view.current_page()?;
        self.pages.borrow().get(current_tab as usize).cloned()
    }

    /// Save the currently-active notebook.
    fn save_current(&self) {
        if let Some(page) = self.current_page() {
            if page.file_path.borrow().is_none() {
                // No path set — show toast and do nothing; user should use Save As
                self.toast_overlay.add_toast(adw::Toast::new(crate::tr_en!(
                    "Use Save As to choose a file path"
                )));
                return;
            }
            if let Err(e) = page.save() {
                self.toast_overlay
                    .add_toast(adw::Toast::new(&crate::tr_fmt!("Save failed: {}", e)));
            } else {
                // Clean save → clear the `*` marker and drop the autosave checkpoint.
                self.refresh_tab_title(&page);
                if let Some(idx) = self
                    .pages
                    .borrow()
                    .iter()
                    .position(|p| Rc::ptr_eq(p, &page))
                {
                    if let Some(path) = self.autosave_paths.borrow().get(idx) {
                        crate::helpers::notebook_autosave::delete_autosave(path);
                    }
                }
                self.toast_overlay
                    .add_toast(adw::Toast::new(crate::tr_en!("Saved")));
            }
        }
    }

    // ── Public entry points (for toolbar and keyboard shortcuts) ─────────────

    /// Create a new untitled notebook in a new tab.
    pub fn trigger_new(self: &Rc<Self>) {
        let doc = NotebookDocument::create_empty();
        let python_path = self
            .python_path
            .borrow()
            .clone()
            .unwrap_or_else(|| PathBuf::from("/usr/bin/python3"));

        let page = NotebookPage::load_from_document(self.services.clone(), python_path, doc, None);
        self.add_tab(page, crate::tr_en!("Untitled"));
    }

    /// Trigger "Open" via keyboard shortcut. Uses the host's widget as parent.
    pub fn trigger_open(self: &Rc<Self>) {
        let h = self.clone();
        let parent = h.widget.clone().upcast::<gtk::Widget>();
        glib::spawn_future_local(async move {
            h.open_file_dialog(&parent).await;
        });
    }

    /// Trigger "Save" for the currently-active notebook.
    pub fn trigger_save(&self) {
        self.save_current();
    }

    /// Trigger "Save As" for the currently-active notebook.
    pub fn trigger_save_as(self: &Rc<Self>) {
        let h = self.clone();
        let parent = h.widget.clone().upcast::<gtk::Widget>();
        glib::spawn_future_local(async move {
            h.trigger_save_as_widget(&parent).await;
        });
    }

    /// "Save As" async implementation, parameterised by a parent widget.
    async fn trigger_save_as_widget(self: Rc<Self>, parent: &gtk::Widget) {
        let page = match self.current_page() {
            Some(p) => p,
            None => return,
        };
        let root = parent.root().and_downcast::<gtk::Window>();

        let filter = gtk::FileFilter::new();
        filter.set_name(Some(crate::tr_en!("Jupyter Notebook")));
        filter.add_pattern("*.ipynb");
        let filters = gtk::gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);

        let dialog = gtk::FileDialog::builder()
            .title(crate::tr_en!("Save Notebook As"))
            .modal(true)
            .filters(&filters)
            .build();

        if let Ok(file) = dialog.save_future(root.as_ref()).await {
            if let Some(path) = file.path() {
                match page.save_as(path) {
                    Ok(()) => {
                        self.toast_overlay
                            .add_toast(adw::Toast::new(crate::tr_en!("Saved")));
                    }
                    Err(e) => {
                        self.toast_overlay
                            .add_toast(adw::Toast::new(&crate::tr_fmt!("Save failed: {}", e)));
                    }
                }
            }
        }
    }

    // ── Kernel status dot ────────────────────────────────────────────────────

    /// Update the kernel dot based on a short state keyword.
    fn update_kernel_dot(&self, state_kw: &str) {
        // Remove all known state classes
        for cls in &["idle", "busy", "starting", "dead", "error"] {
            self.kernel_dot.remove_css_class(cls);
        }
        self.kernel_dot.remove_css_class("dim-label");

        match state_kw {
            "idle" => {
                self.kernel_dot.add_css_class("idle");
                self.kernel_dot
                    .set_tooltip_text(Some(crate::tr_en!("Kernel status: idle")));
            }
            "busy" => {
                self.kernel_dot.add_css_class("busy");
                self.kernel_dot
                    .set_tooltip_text(Some(crate::tr_en!("Kernel status: busy")));
            }
            "starting" => {
                self.kernel_dot.add_css_class("starting");
                self.kernel_dot
                    .set_tooltip_text(Some(crate::tr_en!("Kernel status: starting")));
            }
            "error" => {
                self.kernel_dot.add_css_class("error");
                self.kernel_dot
                    .set_tooltip_text(Some(crate::tr_en!("Kernel status: error")));
            }
            _ => {
                self.kernel_dot.add_css_class("dim-label");
                self.kernel_dot
                    .set_tooltip_text(Some(crate::tr_en!("Kernel status: not started")));
            }
        }
    }

    /// Update the kernel dot from a full status label (e.g. "Kernel: idle").
    fn update_kernel_dot_from_label(&self, label: &str) {
        let kw = if label.contains("idle") {
            "idle"
        } else if label.contains("busy") {
            "busy"
        } else if label.contains("starting") || label.contains("restarting") {
            "starting"
        } else if label.contains("failed") || label.contains("error") {
            "error"
        } else {
            "dead"
        };
        self.update_kernel_dot(kw);
    }

    /// Populate the recent-notebooks list inside the empty state widget.
    fn populate_recent_list(self: &Rc<Self>, empty_page: &gtk::Box) {
        let recent = self.store.load();
        if recent.is_empty() {
            return;
        }

        // Scan empty_page's children to find the ListBox appended by
        // `build_empty_state`.
        let mut target: Option<gtk::ListBox> = None;
        let mut child = empty_page.first_child();
        while let Some(w) = child {
            if let Some(lb) = w.downcast_ref::<gtk::ListBox>() {
                target = Some(lb.clone());
                break;
            }
            child = w.next_sibling();
        }

        let list_box = match target {
            Some(lb) => lb,
            None => return,
        };

        for entry in &recent {
            let row = gtk::ListBoxRow::new();
            let row_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            row_box.set_margin_start(8);
            row_box.set_margin_end(8);
            row_box.set_margin_top(6);
            row_box.set_margin_bottom(6);

            let icon = gtk::Image::from_icon_name("accessories-text-editor-symbolic");
            row_box.append(&icon);

            let label = gtk::Label::new(Some(&entry.name));
            label.set_halign(gtk::Align::Start);
            label.set_hexpand(true);
            row_box.append(&label);

            row.set_child(Some(&row_box));
            list_box.append(&row);
        }

        // Connect row activation to open the notebook via the unified load_from_path
        let store_entries: Vec<String> = recent.iter().map(|e| e.path.clone()).collect();
        let h = self.clone();
        list_box.connect_row_activated(move |_, row| {
            let idx = row.index() as usize;
            if let Some(path_str) = store_entries.get(idx) {
                h.load_from_path(&PathBuf::from(path_str));
            }
        });
    }

    // ── Notebook settings ──────────────────────────────────────────────────────

    /// Open the notebook settings window (an `adw::Window`, share_dialog idiom).
    /// Every row persists on change and applies live via [`Self::update_settings`].
    fn open_settings_dialog(self: &Rc<Self>, parent: &gtk::Widget) {
        let dialog = adw::Window::builder()
            .title(crate::tr_en!("Notebook Settings"))
            .default_width(480)
            .default_height(560)
            .modal(true)
            .build();
        if let Some(root) = parent.root().and_downcast::<gtk::Window>() {
            dialog.set_transient_for(Some(&root));
        }

        let toolbar_view = adw::ToolbarView::new();
        let header = adw::HeaderBar::new();
        toolbar_view.add_top_bar(&header);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 16);
        content.set_margin_start(18);
        content.set_margin_end(18);
        content.set_margin_top(12);
        content.set_margin_bottom(18);

        let cur = self.settings.borrow().clone();

        // ── Editor ────────────────────────────────────────────────────────────
        let editor_group = adw::PreferencesGroup::new();
        editor_group.set_title(crate::tr_en!("Editor"));

        let font_row = adw::SpinRow::new(
            Some(&gtk::Adjustment::new(
                cur.font_size as f64,
                6.0,
                48.0,
                1.0,
                2.0,
                0.0,
            )),
            1.0,
            0,
        );
        font_row.set_title(crate::tr_en!("Font size"));
        editor_group.add(&font_row);

        let tab_row = adw::SpinRow::new(
            Some(&gtk::Adjustment::new(
                cur.tab_size as f64,
                1.0,
                16.0,
                1.0,
                2.0,
                0.0,
            )),
            1.0,
            0,
        );
        tab_row.set_title(crate::tr_en!("Tab size (spaces)"));
        editor_group.add(&tab_row);

        let wrap_row = adw::SwitchRow::new();
        wrap_row.set_title(crate::tr_en!("Word wrap"));
        wrap_row.set_active(cur.word_wrap);
        editor_group.add(&wrap_row);
        content.append(&editor_group);

        // ── Saving ────────────────────────────────────────────────────────────
        let saving_group = adw::PreferencesGroup::new();
        saving_group.set_title(crate::tr_en!("Saving"));

        let autosave_row = adw::SwitchRow::new();
        autosave_row.set_title(crate::tr_en!("Autosave enabled"));
        autosave_row.set_active(cur.autosave_enabled);
        saving_group.add(&autosave_row);

        let interval_row = adw::SpinRow::new(
            Some(&gtk::Adjustment::new(
                cur.autosave_interval_secs as f64,
                5.0,
                600.0,
                5.0,
                15.0,
                0.0,
            )),
            1.0,
            0,
        );
        interval_row.set_title(crate::tr_en!("Autosave interval (seconds)"));
        saving_group.add(&interval_row);
        content.append(&saving_group);

        // ── Execution ───────────────────────────────────────────────────────────
        let exec_group = adw::PreferencesGroup::new();
        exec_group.set_title(crate::tr_en!("Execution"));

        let timeout_row = adw::SpinRow::new(
            Some(&gtk::Adjustment::new(
                cur.execution_timeout_secs as f64,
                0.0,
                3600.0,
                5.0,
                30.0,
                0.0,
            )),
            1.0,
            0,
        );
        timeout_row.set_title(crate::tr_en!("Execution timeout (seconds, 0 = never)"));
        exec_group.add(&timeout_row);

        let py_row = adw::EntryRow::new();
        py_row.set_title(crate::tr_en!("Python path (blank = auto-detect)"));
        py_row.set_text(cur.python_path.as_deref().unwrap_or(""));
        py_row.set_show_apply_button(true);
        let browse_btn = gtk::Button::from_icon_name("document-open-symbolic");
        browse_btn.add_css_class("flat");
        browse_btn.set_valign(gtk::Align::Center);
        browse_btn.set_tooltip_text(Some(crate::tr_en!("Browse for interpreter")));
        py_row.add_suffix(&browse_btn);
        exec_group.add(&py_row);
        content.append(&exec_group);

        // ── Interface ───────────────────────────────────────────────────────────
        let ui_group = adw::PreferencesGroup::new();
        ui_group.set_title(crate::tr_en!("Interface"));
        let toolbar_row = adw::SwitchRow::new();
        toolbar_row.set_title(crate::tr_en!("Show toolbar"));
        toolbar_row.set_subtitle(crate::tr_en!("Reopen settings with Ctrl+comma"));
        toolbar_row.set_active(cur.show_toolbar);
        ui_group.add(&toolbar_row);

        // Diagnostics: the kernel log is the only durable record of a start
        // failure or an unexplained kernel death, so give the user a way to reach
        // it without knowing the platform's data directory.
        let log_row = adw::ActionRow::new();
        log_row.set_title(crate::tr_en!("Kernel log"));
        log_row.set_subtitle(crate::tr_en!(
            "Diagnostics for kernel start failures and unexpected exits"
        ));
        let log_btn = gtk::Button::with_label(crate::tr_en!("Open folder"));
        log_btn.set_valign(gtk::Align::Center);
        log_btn.add_css_class("flat");
        {
            let h = self.clone();
            log_btn.connect_clicked(move |_| {
                match crate::helpers::notebook_logger::log_dir() {
                    Some(dir) => {
                        // Create it first: the folder does not exist until
                        // something is logged, and opening a missing path just
                        // fails silently.
                        let _ = std::fs::create_dir_all(&dir);
                        let uri = format!("file://{}", dir.display());
                        gtk::gio::AppInfo::launch_default_for_uri(
                            &uri,
                            None::<&gtk::gio::AppLaunchContext>,
                        )
                        .unwrap_or_else(|e| {
                            h.services
                                .toast
                                .toast(crate::tr_fmt!("Could not open the log folder: {}", e));
                        });
                    }
                    None => h
                        .services
                        .toast
                        .toast(crate::tr_en!("No log folder is available on this system")),
                }
            });
        }
        log_row.add_suffix(&log_btn);
        ui_group.add(&log_row);

        content.append(&ui_group);

        let scrolled = gtk::ScrolledWindow::new();
        scrolled.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scrolled.set_vexpand(true);
        scrolled.set_child(Some(&content));
        toolbar_view.set_content(Some(&scrolled));
        dialog.set_content(Some(&toolbar_view));

        // Read every row → persist + apply. Shared by all change signals.
        let persist: Rc<dyn Fn()> = Rc::new({
            let h = self.clone();
            let font_row = font_row.clone();
            let tab_row = tab_row.clone();
            let wrap_row = wrap_row.clone();
            let autosave_row = autosave_row.clone();
            let interval_row = interval_row.clone();
            let timeout_row = timeout_row.clone();
            let py_row = py_row.clone();
            let toolbar_row = toolbar_row.clone();
            move || {
                let py = py_row.text().trim().to_string();
                let new = NotebookSettings {
                    python_path: if py.is_empty() { None } else { Some(py) },
                    font_size: font_row.value().round() as u32,
                    tab_size: tab_row.value().round() as u32,
                    word_wrap: wrap_row.is_active(),
                    autosave_enabled: autosave_row.is_active(),
                    autosave_interval_secs: interval_row.value().round() as u32,
                    execution_timeout_secs: timeout_row.value().round() as u32,
                    show_toolbar: toolbar_row.is_active(),
                };
                h.update_settings(new);
            }
        });

        {
            let p = persist.clone();
            font_row.connect_value_notify(move |_| p());
        }
        {
            let p = persist.clone();
            tab_row.connect_value_notify(move |_| p());
        }
        {
            let p = persist.clone();
            interval_row.connect_value_notify(move |_| p());
        }
        {
            let p = persist.clone();
            timeout_row.connect_value_notify(move |_| p());
        }
        {
            let p = persist.clone();
            wrap_row.connect_active_notify(move |_| p());
        }
        {
            let p = persist.clone();
            autosave_row.connect_active_notify(move |_| p());
        }
        {
            let p = persist.clone();
            toolbar_row.connect_active_notify(move |_| p());
        }
        {
            let p = persist.clone();
            py_row.connect_apply(move |_| p());
        }

        // Browse for a Python interpreter with a file chooser.
        {
            let py_row = py_row.clone();
            let persist = persist.clone();
            let dialog = dialog.clone();
            browse_btn.connect_clicked(move |_| {
                let py_row = py_row.clone();
                let persist = persist.clone();
                let root = dialog.clone().upcast::<gtk::Window>();
                glib::spawn_future_local(async move {
                    let file_dialog = gtk::FileDialog::builder()
                        .title(crate::tr_en!("Select Python Interpreter"))
                        .modal(true)
                        .build();
                    if let Ok(file) = file_dialog.open_future(Some(&root)).await {
                        if let Some(path) = file.path() {
                            py_row.set_text(&path.display().to_string());
                            persist();
                        }
                    }
                });
            });
        }

        dialog.present();
    }

    /// Persist `new`, then apply it live: font size (global CSS), tab width /
    /// wrap (every open page), toolbar visibility, and the Python preference.
    fn update_settings(self: &Rc<Self>, new: NotebookSettings) {
        let new = new.sanitized();
        let old_python = self.settings.borrow().python_path.clone();

        if let Err(e) = self.settings_service.save(&new) {
            self.toast_overlay
                .add_toast(adw::Toast::new(&crate::tr_fmt!(
                    "Settings save failed: {}",
                    e
                )));
        }

        // Font size → global provider (restyles every open notebook's cells).
        apply_font_css(&self.font_provider, new.font_size);

        // Tab width + wrap → each open page's code cells.
        for page in self.pages.borrow().iter() {
            page.apply_editor_settings(new.font_size, new.tab_size, new.word_wrap);
        }

        // Toolbar visibility.
        self.toolbar.set_visible(new.show_toolbar);

        // Re-resolve Python only when the configured path actually changed —
        // avoids spawning `python --version` on every font/tab tweak.
        if old_python != new.python_path {
            let resolved = python_discovery::find_python(new.python_path.as_deref());
            let label = resolved
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| crate::tr_en!("Python not found").to_string());
            self.python_label.set_text(&label);
            *self.python_path.borrow_mut() = resolved;
        }

        *self.settings.borrow_mut() = new;
    }
}

/// Load the code-cell font-size rule into `provider`. Targets the
/// `.code-cell-source` node used by every [`NotebookPage`] code cell so a
/// single global provider restyles all open notebooks at once.
fn apply_font_css(provider: &gtk::CssProvider, font_size: u32) {
    let css = format!(".code-cell-source, .code-cell-source text {{ font-size: {font_size}px; }}");
    provider.load_from_string(&css);
}

// ---------------------------------------------------------------------------
// Live MCP notebook-command helpers
// ---------------------------------------------------------------------------

/// Error string returned when no notebook is open.
fn no_notebook() -> String {
    "no notebook open — use open_notebook or create_notebook first".to_string()
}

/// Read a non-negative integer argument as a `usize`.
fn arg_index(args: &serde_json::Value, key: &str) -> Option<usize> {
    crate::mcp::tools::arg(args, key)
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
}

/// Normalise a cell-type argument, defaulting to "code".
/// Resolve an `add_cell` cell type, refusing anything the schema does not offer.
///
/// The old rule was "markdown, else code", so `cellType: "raw"` — a real Jupyter
/// cell type — silently produced a CODE cell, and so did a capitalised
/// `"Markdown"`. An absent value still means code, which is what the schema
/// documents as the default.
fn cell_type_arg(v: Option<&serde_json::Value>) -> Result<String, String> {
    match v.and_then(|x| x.as_str()) {
        None => Ok("code".to_string()),
        Some(s) if s.eq_ignore_ascii_case("markdown") => Ok("markdown".to_string()),
        Some(s) if s.eq_ignore_ascii_case("code") => Ok("code".to_string()),
        Some(other) => Err(format!(
            "cellType must be 'code' or 'markdown', got '{other}'"
        )),
    }
}

/// Map a kernel status label ("Kernel: idle") to a short state keyword.
fn kernel_keyword(label: &str) -> &'static str {
    if label.contains("idle") {
        "idle"
    } else if label.contains("busy") {
        "busy"
    } else if label.contains("starting") || label.contains("restarting") {
        "starting"
    } else if label.contains("failed") || label.contains("error") {
        "error"
    } else {
        "dead"
    }
}

/// Cap `s` to at most `max` chars, returning the (possibly truncated) text and a
/// flag indicating whether truncation occurred.
fn cap(s: &str, max: usize) -> (String, bool) {
    if s.chars().count() > max {
        (s.chars().take(max).collect(), true)
    } else {
        (s.to_string(), false)
    }
}

/// Character cap applied to cell sources and output text for transport.
const TEXT_CAP: usize = 10_000;

/// Serialise a single code-cell output into the transport JSON shape (binary
/// image data is flagged via `has_image`, never inlined here).
fn cell_output_json(out: &CellOutput) -> serde_json::Value {
    use serde_json::json;
    match out {
        CellOutput::Stream { name, text } => {
            let (t, tr) = cap(&text.joined(), TEXT_CAP);
            json!({
                "outputType": "stream", "name": name, "text": t, "textTruncated": tr,
                "isError": false, "errorName": "", "traceback": "", "tracebackTruncated": false,
                "hasImage": false, "hasHtml": false,
            })
        }
        CellOutput::ExecuteResult {
            execution_count,
            data,
            ..
        } => {
            let (t, tr) = cap(&data.plain_text().unwrap_or_default(), TEXT_CAP);
            json!({
                "outputType": "execute_result", "executionCount": execution_count,
                "text": t, "textTruncated": tr, "isError": false, "errorName": "",
                "traceback": "", "tracebackTruncated": false,
                "hasImage": data.has_image(), "hasHtml": data.text_html.is_some(),
            })
        }
        CellOutput::DisplayData { data, .. } => {
            let (t, tr) = cap(&data.plain_text().unwrap_or_default(), TEXT_CAP);
            json!({
                "outputType": "display_data", "text": t, "textTruncated": tr,
                "isError": false, "errorName": "", "traceback": "", "tracebackTruncated": false,
                "hasImage": data.has_image(), "hasHtml": data.text_html.is_some(),
            })
        }
        CellOutput::Error {
            ename,
            evalue,
            traceback,
        } => {
            let (t, tr) = cap(evalue, TEXT_CAP);
            let (tb, tbtr) = cap(&traceback.join("\n"), TEXT_CAP);
            json!({
                "outputType": "error", "text": t, "textTruncated": tr,
                "isError": true, "errorName": ename, "traceback": tb, "tracebackTruncated": tbtr,
                "hasImage": false, "hasHtml": false,
            })
        }
    }
}

/// Build the full notebook-state JSON (metadata + cell list + kernel) for `page`.
fn notebook_state_json(page: &Rc<NotebookPage>, id: &str) -> serde_json::Value {
    use serde_json::json;
    let doc = page.snapshot_document();
    let label = page.current_kernel_status_label();
    let file_path = page
        .file_path
        .borrow()
        .as_ref()
        .map(|p| p.display().to_string());
    let cells: Vec<serde_json::Value> = doc
        .cells
        .iter()
        .enumerate()
        .map(|(i, cell)| {
            let (src, trunc) = cap(&cell.source.joined(), TEXT_CAP);
            json!({
                "index": i,
                "type": cell.cell_type,
                "source": src,
                "sourceTruncated": trunc,
                "executionCount": cell.execution_count,
                "outputCount": cell.outputs.len(),
            })
        })
        .collect();
    json!({
        "loaded": true,
        "notebookId": id,
        "title": page.title(),
        "filePath": file_path,
        "isDirty": page.is_modified(),
        "kernelState": kernel_keyword(&label),
        "kernelStatusText": label,
        "selectedIndex": page.active_cell_index(),
        "cellCount": doc.cells.len(),
        "cells": cells,
    })
}

// ---------------------------------------------------------------------------
// Empty state widget
// ---------------------------------------------------------------------------

fn build_empty_state() -> gtk::Box {
    let page = gtk::Box::new(gtk::Orientation::Vertical, 24);
    page.set_vexpand(true);
    page.set_valign(gtk::Align::Center);
    page.set_halign(gtk::Align::Center);
    page.set_margin_top(48);
    page.set_margin_bottom(48);

    let icon = gtk::Image::from_icon_name("accessories-text-editor-symbolic");
    icon.set_pixel_size(64);
    icon.add_css_class("dim-label");
    page.append(&icon);

    let title = gtk::Label::new(Some(crate::tr_en!("Notebook")));
    title.add_css_class("title-2");
    page.append(&title);

    let subtitle = gtk::Label::new(Some(crate::tr_en!(
        "Open a Jupyter notebook (.ipynb) or Python (.py) file"
    )));
    subtitle.add_css_class("dim-label");
    subtitle.set_justify(gtk::Justification::Center);
    page.append(&subtitle);

    // Recent notebooks section
    let recent_label = gtk::Label::new(Some(crate::tr_en!("Recent Notebooks")));
    recent_label.add_css_class("heading");
    recent_label.set_margin_top(12);
    page.append(&recent_label);

    // The ListBox that populate_recent_list will fill
    let list_box = gtk::ListBox::new();
    list_box.add_css_class("boxed-list");
    list_box.set_width_request(480);
    page.append(&list_box);

    page
}
