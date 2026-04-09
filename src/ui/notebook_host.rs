//! Tab host for multiple open notebooks in Verbinal.
//!
//! [`NotebookTabHost`] wraps a `gtk::Notebook` (tab strip) and manages
//! [`NotebookPage`] instances.  When no notebooks are open it shows a welcome
//! empty-state page with a list of recently-opened files.

use crate::helpers::notebook_parser;
use crate::helpers::python_discovery;
use crate::services::notebook_store::NotebookStore;
use crate::state::AppServices;
use crate::ui::notebook_page::NotebookPage;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use libadwaita as adw;
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
    /// Persistent recent-notebooks store.
    store: Rc<NotebookStore>,
    /// Resolved Python interpreter path (may be `None` if Python not found).
    python_path: Rc<RefCell<Option<PathBuf>>>,
    /// App services (for kernel bridging).
    services: Arc<AppServices>,
    /// Stack that switches between the empty-state and the tab notebook.
    content_stack: gtk::Stack,
    /// Status dot for kernel (coloured circle).
    #[allow(dead_code)]
    kernel_dot: gtk::Label,
    /// Python path label in toolbar.
    #[allow(dead_code)]
    python_label: gtk::Label,
    /// Toast overlay for surfacing errors to the user.
    toast_overlay: adw::ToastOverlay,
}

impl NotebookTabHost {
    /// Create a new, empty tab host and resolve Python.
    pub fn new(services: Arc<AppServices>) -> Rc<Self> {
        // ── Python discovery ─────────────────────────────────────────────────
        let python_path = python_discovery::find_python(None);
        let python_label_text = python_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "Python not found".to_string());

        // ── Root ─────────────────────────────────────────────────────────────
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        widget.set_vexpand(true);
        widget.set_hexpand(true);

        // ── Toolbar ──────────────────────────────────────────────────────────
        let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        toolbar.set_margin_start(8);
        toolbar.set_margin_end(8);
        toolbar.set_margin_top(6);
        toolbar.set_margin_bottom(6);

        let open_btn = gtk::Button::with_label("Open");
        open_btn.set_icon_name("document-open-symbolic");
        open_btn.add_css_class("flat");
        open_btn.set_tooltip_text(Some("Open notebook"));
        toolbar.append(&open_btn);

        let save_btn = gtk::Button::with_label("Save");
        save_btn.set_icon_name("document-save-symbolic");
        save_btn.add_css_class("flat");
        save_btn.set_tooltip_text(Some("Save notebook (Ctrl+S)"));
        toolbar.append(&save_btn);

        toolbar.append(&gtk::Separator::new(gtk::Orientation::Vertical));

        let run_all_btn = gtk::Button::with_label("Run All");
        run_all_btn.set_icon_name("media-seek-forward-symbolic");
        run_all_btn.add_css_class("flat");
        run_all_btn.set_tooltip_text(Some("Run all cells"));
        toolbar.append(&run_all_btn);

        let restart_btn = gtk::Button::with_label("Restart");
        restart_btn.set_icon_name("view-refresh-symbolic");
        restart_btn.add_css_class("flat");
        restart_btn.set_tooltip_text(Some("Restart kernel"));
        toolbar.append(&restart_btn);

        let interrupt_btn = gtk::Button::with_label("Interrupt");
        interrupt_btn.set_icon_name("media-playback-stop-symbolic");
        interrupt_btn.add_css_class("flat");
        interrupt_btn.set_tooltip_text(Some("Interrupt kernel"));
        toolbar.append(&interrupt_btn);

        // Spacer
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        toolbar.append(&spacer);

        // Kernel dot
        let kernel_dot = gtk::Label::new(Some("●"));
        kernel_dot.add_css_class("dim-label");
        kernel_dot.set_tooltip_text(Some("Kernel status"));
        toolbar.append(&kernel_dot);

        // Python path label
        let python_label = gtk::Label::new(Some(&python_label_text));
        python_label.add_css_class("dim-label");
        python_label.add_css_class("caption");
        toolbar.append(&python_label);

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

        let host = Rc::new(NotebookTabHost {
            widget,
            tab_view,
            pages: Rc::new(RefCell::new(Vec::new())),
            store,
            python_path: Rc::new(RefCell::new(python_path)),
            services,
            content_stack,
            kernel_dot,
            python_label,
            toast_overlay,
        });

        // Wire toolbar buttons
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
                h.save_current();
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

        // Populate recent notebooks in empty state
        host.populate_recent_list(&empty_page);

        host
    }

    // ── Public API ────────────────────────────────────────────────────────────

    /// Return the root widget.
    pub fn widget(&self) -> &gtk::Box {
        &self.widget
    }

    /// Open a file chooser dialog and load the selected notebook.
    async fn open_file_dialog(&self, parent: &gtk::Widget) {
        let root = parent.root().and_downcast::<gtk::Window>();

        let filter = gtk::FileFilter::new();
        filter.set_name(Some("Notebooks"));
        filter.add_pattern("*.ipynb");
        filter.add_pattern("*.py");

        let filters = gtk::gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);

        let dialog = gtk::FileDialog::builder()
            .title("Open Notebook")
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
    /// Supports `.ipynb` and `.py` files.
    pub fn load_from_path(&self, path: &Path) {
        let load_result = if path.extension().and_then(|e| e.to_str()) == Some("py") {
            notebook_parser::load_python_as_notebook(path)
        } else {
            notebook_parser::load_notebook(path)
        };

        let doc = match load_result {
            Ok(d) => d,
            Err(e) => {
                self.toast_overlay
                    .add_toast(adw::Toast::new(&format!("Failed to load: {}", e)));
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
            python_path,
            doc,
            Some(path.to_path_buf()),
        );

        self.add_tab(page, &name);
    }

    // ── Private helpers ───────────────────────────────────────────────────────

    /// Add a [`NotebookPage`] as a new tab.
    fn add_tab(&self, page: Rc<NotebookPage>, title: &str) {
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

        self.pages.borrow_mut().push(page.clone());

        // Close button handler — use a stable index captured at creation time.
        // This is a simplified approach: the index may shift if earlier tabs
        // are closed, so we search by widget pointer instead.
        {
            let tab_view = self.tab_view.clone();
            let pages = self.pages.clone();
            let content_stack = self.content_stack.clone();
            let page_ptr = page.widget().clone();
            close_btn.connect_clicked(move |_| {
                let n = tab_view.n_pages();
                for i in 0..n {
                    if let Some(child) = tab_view.nth_page(Some(i)) {
                        // Compare widget pointer identity
                        if child == page_ptr.clone().upcast::<gtk::Widget>() {
                            tab_view.remove_page(Some(i));
                            let idx_usize = i as usize;
                            if idx_usize < pages.borrow().len() {
                                pages.borrow_mut().remove(idx_usize);
                            }
                            if pages.borrow().is_empty() {
                                content_stack.set_visible_child_name("empty");
                            }
                            return;
                        }
                    }
                }
                // Fallback: remove by original index
                if tab_index < tab_view.n_pages() {
                    tab_view.remove_page(Some(tab_index));
                    let idx_usize = tab_index as usize;
                    if idx_usize < pages.borrow().len() {
                        pages.borrow_mut().remove(idx_usize);
                    }
                    if pages.borrow().is_empty() {
                        content_stack.set_visible_child_name("empty");
                    }
                }
            });
        }

        // Switch to tabs view
        self.content_stack.set_visible_child_name("tabs");
        self.tab_view.set_current_page(Some(tab_index));
    }

    /// Return the currently-active [`NotebookPage`], if any.
    fn current_page(&self) -> Option<Rc<NotebookPage>> {
        let current_tab = self.tab_view.current_page()?;
        self.pages.borrow().get(current_tab as usize).cloned()
    }

    /// Save the currently-active notebook.
    fn save_current(&self) {
        if let Some(page) = self.current_page() {
            if let Err(e) = page.save() {
                self.toast_overlay
                    .add_toast(adw::Toast::new(&format!("Save failed: {}", e)));
            }
        }
    }

    /// Populate the recent-notebooks list inside the empty state widget.
    fn populate_recent_list(&self, empty_page: &gtk::Box) {
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

        // Connect row activation to open the notebook
        {
            let store_entries: Vec<String> = recent.iter().map(|e| e.path.clone()).collect();
            let host_pages = self.pages.clone();
            let host_services = self.services.clone();
            let host_python = self.python_path.clone();
            let tab_view = self.tab_view.clone();
            let content_stack = self.content_stack.clone();
            let store = self.store.clone();

            list_box.connect_row_activated(move |_, row| {
                let idx = row.index() as usize;
                if let Some(path_str) = store_entries.get(idx) {
                    let path = PathBuf::from(path_str);
                    let load_result = if path.extension().and_then(|e| e.to_str()) == Some("py") {
                        notebook_parser::load_python_as_notebook(&path)
                    } else {
                        notebook_parser::load_notebook(&path)
                    };

                    if let Ok(doc) = load_result {
                        let name = path
                            .file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_else(|| path_str.clone());

                        let _ = store.add(path_str, &name);

                        let python_path = host_python
                            .borrow()
                            .clone()
                            .unwrap_or_else(|| PathBuf::from("/usr/bin/python3"));

                        let page = NotebookPage::load_from_document(
                            host_services.clone(),
                            python_path,
                            doc,
                            Some(path.clone()),
                        );

                        // Build tab header
                        let tab_box = gtk::Box::new(gtk::Orientation::Horizontal, 4);
                        let tab_label_w = gtk::Label::new(Some(&name));
                        tab_label_w.set_max_width_chars(20);
                        tab_label_w
                            .set_ellipsize(gtk::pango::EllipsizeMode::End);
                        let close_btn =
                            gtk::Button::from_icon_name("window-close-symbolic");
                        close_btn.add_css_class("flat");
                        close_btn.add_css_class("circular");
                        tab_box.append(&tab_label_w);
                        tab_box.append(&close_btn);

                        let page_widget = page.widget().clone().upcast::<gtk::Widget>();
                        let page_ptr = page.widget().clone();
                        let tab_index =
                            tab_view.append_page(&page_widget, Some(&tab_box));
                        host_pages.borrow_mut().push(page.clone());

                        let tv2 = tab_view.clone();
                        let pages2 = host_pages.clone();
                        let cs2 = content_stack.clone();
                        close_btn.connect_clicked(move |_| {
                            let n = tv2.n_pages();
                            for i in 0..n {
                                if let Some(child) = tv2.nth_page(Some(i)) {
                                    if child
                                        == page_ptr.clone().upcast::<gtk::Widget>()
                                    {
                                        tv2.remove_page(Some(i));
                                        let iu = i as usize;
                                        if iu < pages2.borrow().len() {
                                            pages2.borrow_mut().remove(iu);
                                        }
                                        if pages2.borrow().is_empty() {
                                            cs2.set_visible_child_name("empty");
                                        }
                                        return;
                                    }
                                }
                            }
                            if tab_index < tv2.n_pages() {
                                tv2.remove_page(Some(tab_index));
                                let iu = tab_index as usize;
                                if iu < pages2.borrow().len() {
                                    pages2.borrow_mut().remove(iu);
                                }
                                if pages2.borrow().is_empty() {
                                    cs2.set_visible_child_name("empty");
                                }
                            }
                        });

                        content_stack.set_visible_child_name("tabs");
                        tab_view.set_current_page(Some(tab_index));
                    }
                }
            });
        }
    }
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

    let title = gtk::Label::new(Some("Notebook"));
    title.add_css_class("title-2");
    page.append(&title);

    let subtitle = gtk::Label::new(Some(
        "Open a Jupyter notebook (.ipynb) or Python (.py) file",
    ));
    subtitle.add_css_class("dim-label");
    subtitle.set_justify(gtk::Justification::Center);
    page.append(&subtitle);

    // Recent notebooks section
    let recent_label = gtk::Label::new(Some("Recent Notebooks"));
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
