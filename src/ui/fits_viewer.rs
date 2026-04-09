use crate::helpers::fits_loader;
use crate::state::AppServices;
use crate::ui::fits_tab::FitsTab;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{self as gtk};

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

pub struct FitsViewer {
    widget: gtk::Box,
    notebook: gtk::Notebook,
    tabs: Rc<RefCell<Vec<Rc<FitsTab>>>>,
    shared_cursor: Rc<RefCell<Option<(f64, f64)>>>,
    status_label: gtk::Label,
    blink_active: Rc<RefCell<bool>>,
}

impl FitsViewer {
    pub fn new(_services: Arc<AppServices>) -> Rc<Self> {
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        widget.set_vexpand(true);
        widget.set_hexpand(true);

        // Toolbar
        let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        toolbar.set_margin_start(12);
        toolbar.set_margin_end(12);
        toolbar.set_margin_top(12);
        toolbar.set_margin_bottom(6);

        let open_btn = gtk::Button::with_label("Open FITS");
        open_btn.set_icon_name("document-open-symbolic");
        open_btn.add_css_class("suggested-action");
        toolbar.append(&open_btn);

        let blink_btn = gtk::ToggleButton::with_label("Blink");
        blink_btn.set_icon_name("view-refresh-symbolic");
        blink_btn.set_tooltip_text(Some("Blink comparison between first two tabs"));
        toolbar.append(&blink_btn);

        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        toolbar.append(&spacer);

        let status_label = gtk::Label::new(Some("No file loaded"));
        status_label.add_css_class("dim-label");
        status_label.add_css_class("caption");
        toolbar.append(&status_label);

        widget.append(&toolbar);
        widget.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

        // Notebook for tabs
        let notebook = gtk::Notebook::new();
        notebook.set_vexpand(true);
        notebook.set_scrollable(true);
        notebook.set_show_border(false);

        // Empty state
        let empty_box = gtk::Box::new(gtk::Orientation::Vertical, 12);
        empty_box.set_valign(gtk::Align::Center);
        empty_box.set_halign(gtk::Align::Center);
        let empty_icon = gtk::Image::from_icon_name("image-x-generic-symbolic");
        empty_icon.set_pixel_size(64);
        empty_icon.add_css_class("dim-label");
        empty_box.append(&empty_icon);
        let empty_label = gtk::Label::new(Some("Open a FITS file to get started"));
        empty_label.add_css_class("dim-label");
        empty_box.append(&empty_label);
        notebook.append_page(&empty_box, Some(&gtk::Label::new(Some("Welcome"))));

        widget.append(&notebook);

        let viewer = Rc::new(FitsViewer {
            widget,
            notebook,
            tabs: Rc::new(RefCell::new(Vec::new())),
            shared_cursor: Rc::new(RefCell::new(None)),
            status_label,
            blink_active: Rc::new(RefCell::new(false)),
        });

        // Open file button
        let v = viewer.clone();
        open_btn.connect_clicked(move |btn| {
            let v = v.clone();
            let btn = btn.clone();
            glib::spawn_future_local(async move {
                v.open_file_dialog(&btn).await;
            });
        });

        // Blink toggle
        let v = viewer.clone();
        blink_btn.connect_toggled(move |btn| {
            *v.blink_active.borrow_mut() = btn.is_active();
            if btn.is_active() {
                v.start_blink();
            }
        });

        viewer
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.widget
    }

    async fn open_file_dialog(&self, parent: &impl IsA<gtk::Widget>) {
        let root = parent.root().and_downcast::<gtk::Window>();
        let filter = gtk::FileFilter::new();
        filter.add_pattern("*.fits");
        filter.add_pattern("*.FITS");
        filter.add_pattern("*.fit");
        filter.add_pattern("*.fts");
        filter.set_name(Some("FITS Images"));

        let filters = gtk4::gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);

        let dialog = gtk::FileDialog::builder()
            .title("Open FITS File")
            .filters(&filters)
            .build();

        match dialog.open_future(root.as_ref()).await {
            Ok(file) => {
                if let Some(path) = file.path() {
                    self.load_file(&path);
                }
            }
            Err(_) => {} // User cancelled
        }
    }

    fn load_file(&self, path: &std::path::Path) {
        match fits_loader::load_fits_image(path) {
            Ok(data) => {
                let filename = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();

                let summary = fits_loader::fits_summary(&data);
                self.status_label.set_text(&summary);

                let tab = FitsTab::new(data, self.shared_cursor.clone());

                // Remove welcome page if it's the first real tab
                if self.tabs.borrow().is_empty() && self.notebook.n_pages() > 0 {
                    self.notebook.remove_page(Some(0));
                }

                // Add close button to tab label
                let tab_box = gtk::Box::new(gtk::Orientation::Horizontal, 4);
                let tab_label = gtk::Label::new(Some(&filename));
                tab_box.append(&tab_label);
                let close_btn = gtk::Button::from_icon_name("window-close-symbolic");
                close_btn.add_css_class("flat");
                close_btn.add_css_class("circular");
                tab_box.append(&close_btn);

                let page_num = self.notebook.append_page(tab.widget(), Some(&tab_box));
                self.notebook.set_current_page(Some(page_num));

                // Close button handler
                let notebook = self.notebook.clone();
                let tabs = self.tabs.clone();
                close_btn.connect_clicked(move |_| {
                    // Find which page this tab is on
                    let n = notebook.n_pages();
                    for i in 0..n {
                        if let Some(page) = notebook.nth_page(Some(i)) {
                            if let Some(tab_label_widget) = notebook.tab_label(&page) {
                                if tab_label_widget.eq(&tab_box) {
                                    notebook.remove_page(Some(i));
                                    let mut t = tabs.borrow_mut();
                                    if (i as usize) < t.len() {
                                        t.remove(i as usize);
                                    }
                                    break;
                                }
                            }
                        }
                    }
                });

                self.tabs.borrow_mut().push(tab);
            }
            Err(e) => {
                self.status_label.set_text(&format!("Error: {}", e));
            }
        }
    }

    fn start_blink(&self) {
        let notebook = self.notebook.clone();
        let blink_active = self.blink_active.clone();

        glib::timeout_add_local(std::time::Duration::from_millis(500), move || {
            if !*blink_active.borrow() {
                return glib::ControlFlow::Break;
            }
            if notebook.n_pages() < 2 {
                return glib::ControlFlow::Break;
            }
            let current = notebook.current_page().unwrap_or(0);
            let next = if current == 0 { 1 } else { 0 };
            notebook.set_current_page(Some(next));
            glib::ControlFlow::Continue
        });
    }

    /// Load a FITS file from a path (used by VOSpace integration).
    pub fn load_from_path(&self, path: &std::path::Path) {
        self.load_file(path);
    }
}
