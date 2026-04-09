use crate::state::AppServices;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

pub struct VoSpaceBrowser {
    widget: gtk::Box,
    services: Arc<AppServices>,
    current_path: Rc<RefCell<String>>,
    file_list_box: gtk::ListBox,
    breadcrumb_label: gtk::Label,
    status_label: gtk::Label,
}

impl VoSpaceBrowser {
    pub fn new(services: Arc<AppServices>) -> Rc<Self> {
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        widget.set_vexpand(true);
        widget.set_hexpand(true);

        // Toolbar
        let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        toolbar.set_margin_start(12);
        toolbar.set_margin_end(12);
        toolbar.set_margin_top(12);
        toolbar.set_margin_bottom(6);

        let breadcrumb_label = gtk::Label::new(Some("/"));
        breadcrumb_label.add_css_class("title-4");
        breadcrumb_label.set_hexpand(true);
        breadcrumb_label.set_halign(gtk::Align::Start);
        toolbar.append(&breadcrumb_label);

        let up_btn = gtk::Button::from_icon_name("go-up-symbolic");
        up_btn.set_tooltip_text(Some("Go Up"));
        toolbar.append(&up_btn);

        let new_folder_btn = gtk::Button::from_icon_name("folder-new-symbolic");
        new_folder_btn.set_tooltip_text(Some("New Folder"));
        toolbar.append(&new_folder_btn);

        let refresh_btn = gtk::Button::from_icon_name("view-refresh-symbolic");
        refresh_btn.set_tooltip_text(Some("Refresh"));
        toolbar.append(&refresh_btn);

        widget.append(&toolbar);

        // Separator
        widget.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

        // File list
        let scrolled = gtk::ScrolledWindow::new();
        scrolled.set_vexpand(true);
        scrolled.set_hexpand(true);

        let file_list_box = gtk::ListBox::new();
        file_list_box.set_selection_mode(gtk::SelectionMode::Single);
        file_list_box.add_css_class("boxed-list");
        file_list_box.set_margin_start(12);
        file_list_box.set_margin_end(12);
        file_list_box.set_margin_top(6);
        file_list_box.set_margin_bottom(12);

        // Placeholder
        file_list_box.set_placeholder(Some(
            &gtk::Label::builder()
                .label("Login to browse your VOSpace files")
                .css_classes(vec!["dim-label".to_string()])
                .margin_top(24)
                .margin_bottom(24)
                .build(),
        ));

        scrolled.set_child(Some(&file_list_box));
        widget.append(&scrolled);

        // Status bar
        let status_label = gtk::Label::new(None);
        status_label.add_css_class("dim-label");
        status_label.add_css_class("caption");
        status_label.set_margin_start(12);
        status_label.set_margin_bottom(6);
        status_label.set_halign(gtk::Align::Start);
        widget.append(&status_label);

        let browser = Rc::new(VoSpaceBrowser {
            widget,
            services,
            current_path: Rc::new(RefCell::new(String::new())),
            file_list_box,
            breadcrumb_label,
            status_label,
        });

        // Wire up buttons
        let b = browser.clone();
        refresh_btn.connect_clicked(move |_| {
            let b = b.clone();
            glib::spawn_future_local(async move {
                b.refresh().await;
            });
        });

        let b = browser.clone();
        up_btn.connect_clicked(move |_| {
            let b = b.clone();
            glib::spawn_future_local(async move {
                b.go_up().await;
            });
        });

        let b = browser.clone();
        new_folder_btn.connect_clicked(move |_| {
            let b = b.clone();
            glib::spawn_future_local(async move {
                b.create_folder_dialog().await;
            });
        });

        // Double-click to navigate into folders
        let b = browser.clone();
        browser.file_list_box.connect_row_activated(move |_, row| {
            let b = b.clone();
            let idx = row.index() as usize;
            glib::spawn_future_local(async move {
                b.on_row_activated(idx).await;
            });
        });

        browser
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.widget
    }

    pub async fn refresh(&self) {
        let svc = self.services.clone();
        let path = self.current_path.borrow().clone();

        let result = self
            .services
            .spawn(async move {
                let token = svc.get_token().await;
                let username = svc.get_username().await;
                match (token, username) {
                    (Some(token), Some(username)) => {
                        svc.vospace.list_nodes(&token, &username, &path).await
                    }
                    _ => Err(crate::services::ApiError::Unauthorized),
                }
            })
            .await;

        // Clear existing items
        while let Some(child) = self.file_list_box.first_child() {
            self.file_list_box.remove(&child);
        }

        match result {
            Ok(nodes) => {
                let count = nodes.len();
                for node in &nodes {
                    let row = self.make_file_row(node);
                    self.file_list_box.append(&row);
                }
                let path = self.current_path.borrow().clone();
                self.breadcrumb_label.set_text(&format!("/{}", path));
                self.status_label.set_text(&format!("{} items", count));
            }
            Err(e) => {
                self.status_label.set_text(&format!("Error: {}", e));
            }
        }
    }

    fn make_file_row(&self, node: &crate::models::VoSpaceNode) -> adw::ActionRow {
        let icon_name = if node.is_container() {
            "folder-symbolic"
        } else {
            match node.content_type.as_deref() {
                Some(ct) if ct.contains("fits") => "image-x-generic-symbolic",
                Some(ct) if ct.contains("image") => "image-x-generic-symbolic",
                Some(ct) if ct.contains("text") => "text-x-generic-symbolic",
                _ => "text-x-generic-symbolic",
            }
        };

        let row = adw::ActionRow::builder()
            .title(&node.name)
            .subtitle(&if node.is_container() {
                "Folder".to_string()
            } else {
                node.size_display()
            })
            .activatable(true)
            .build();

        row.add_prefix(&gtk::Image::from_icon_name(icon_name));

        if !node.is_container() {
            let download_btn = gtk::Button::from_icon_name("folder-download-symbolic");
            download_btn.set_tooltip_text(Some("Download"));
            download_btn.set_valign(gtk::Align::Center);
            download_btn.add_css_class("flat");
            row.add_suffix(&download_btn);
        }

        let delete_btn = gtk::Button::from_icon_name("user-trash-symbolic");
        delete_btn.set_tooltip_text(Some("Delete"));
        delete_btn.set_valign(gtk::Align::Center);
        delete_btn.add_css_class("flat");
        row.add_suffix(&delete_btn);

        row
    }

    async fn go_up(&self) {
        let path = self.current_path.borrow().clone();
        if path.is_empty() {
            return;
        }
        let new_path = match path.rsplit_once('/') {
            Some((parent, _)) => parent.to_string(),
            None => String::new(),
        };
        *self.current_path.borrow_mut() = new_path;
        self.refresh().await;
    }

    async fn on_row_activated(&self, _idx: usize) {
        // Navigation into folders will be wired up with node data
        // For now this is a placeholder
    }

    async fn create_folder_dialog(&self) {
        // Will show a dialog to create a new folder
    }
}
