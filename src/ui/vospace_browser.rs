use crate::models::VoSpaceNode;
use crate::state::AppServices;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Sort state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SortColumn {
    Name,
    Size,
    Modified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SortOrder {
    Ascending,
    Descending,
}

#[derive(Debug, Clone)]
struct SortState {
    column: SortColumn,
    order: SortOrder,
}

impl Default for SortState {
    fn default() -> Self {
        SortState {
            column: SortColumn::Name,
            order: SortOrder::Ascending,
        }
    }
}

impl SortState {
    fn toggle(&mut self, col: SortColumn) {
        if self.column == col {
            self.order = match self.order {
                SortOrder::Ascending => SortOrder::Descending,
                SortOrder::Descending => SortOrder::Ascending,
            };
        } else {
            self.column = col;
            self.order = SortOrder::Ascending;
        }
    }

    fn indicator(&self, col: SortColumn) -> &'static str {
        if self.column == col {
            match self.order {
                SortOrder::Ascending => " ▲",
                SortOrder::Descending => " ▼",
            }
        } else {
            ""
        }
    }
}

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

fn is_fits_file(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.ends_with(".fits") || lower.ends_with(".fit") || lower.ends_with(".fts")
}

fn is_notebook_file(name: &str) -> bool {
    name.to_lowercase().ends_with(".ipynb")
}

fn guess_content_type(filename: &str) -> &'static str {
    let lower = filename.to_lowercase();
    if lower.ends_with(".fits") || lower.ends_with(".fit") || lower.ends_with(".fts") {
        "application/fits"
    } else if lower.ends_with(".png") {
        "image/png"
    } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        "image/jpeg"
    } else if lower.ends_with(".gif") {
        "image/gif"
    } else if lower.ends_with(".txt") || lower.ends_with(".log") {
        "text/plain"
    } else if lower.ends_with(".xml") {
        "application/xml"
    } else if lower.ends_with(".json") {
        "application/json"
    } else if lower.ends_with(".csv") {
        "text/csv"
    } else if lower.ends_with(".pdf") {
        "application/pdf"
    } else {
        "application/octet-stream"
    }
}

// ---------------------------------------------------------------------------
// VoSpaceBrowser
// ---------------------------------------------------------------------------

pub struct VoSpaceBrowser {
    widget: gtk::Box,
    services: Arc<AppServices>,
    /// Current directory path within the user's VOSpace home (empty = root).
    current_path: Rc<RefCell<String>>,
    file_list_box: gtk::ListBox,
    breadcrumb_label: gtk::Label,
    status_label: gtk::Label,
    /// Cached and sorted node list — row index → node.
    nodes: Rc<RefCell<Vec<VoSpaceNode>>>,
    /// Active sort state.
    sort_state: Rc<RefCell<SortState>>,
    /// Sort header buttons — kept so we can update their labels.
    sort_btn_name: gtk::Button,
    sort_btn_size: gtk::Button,
    sort_btn_modified: gtk::Button,
    /// Toast overlay injected by the parent window after construction.
    toast_overlay: Rc<RefCell<Option<adw::ToastOverlay>>>,
    /// The transfer strip: hidden until something is moving.
    transfer_row: gtk::Box,
    transfer_label: gtk::Label,
    transfer_bar: gtk::ProgressBar,
    /// The token for the transfer in flight, so Cancel has something to trip.
    transfer_cancel: Rc<RefCell<Option<crate::services::transfer::Cancel>>>,
}

impl VoSpaceBrowser {
    pub fn new(services: Arc<AppServices>) -> Rc<Self> {
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        widget.set_vexpand(true);
        widget.set_hexpand(true);

        // ----------------------------------------------------------------
        // Toolbar
        // ----------------------------------------------------------------
        let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
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
        up_btn.set_tooltip_text(Some(crate::tr_en!("Go Up")));
        toolbar.append(&up_btn);

        let new_folder_btn = gtk::Button::from_icon_name("folder-new-symbolic");
        new_folder_btn.set_tooltip_text(Some(crate::tr_en!("New Folder")));
        toolbar.append(&new_folder_btn);

        let upload_btn = gtk::Button::from_icon_name("document-send-symbolic");
        upload_btn.set_tooltip_text(Some(crate::tr_en!("Upload Files")));
        toolbar.append(&upload_btn);

        let copy_path_btn = gtk::Button::from_icon_name("edit-copy-symbolic");
        copy_path_btn.set_tooltip_text(Some(crate::tr_en!("Copy Current Path")));
        toolbar.append(&copy_path_btn);

        let refresh_btn = gtk::Button::from_icon_name("view-refresh-symbolic");
        refresh_btn.set_tooltip_text(Some(crate::tr_en!("Refresh")));
        toolbar.append(&refresh_btn);

        widget.append(&toolbar);
        widget.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

        // ----------------------------------------------------------------
        // Transfer strip — what is moving, how far it has got, and a way out
        // ----------------------------------------------------------------
        let transfer_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        transfer_row.set_margin_start(12);
        transfer_row.set_margin_end(12);
        transfer_row.set_margin_top(6);
        transfer_row.set_margin_bottom(6);
        transfer_row.set_visible(false);

        let transfer_label = gtk::Label::new(None);
        transfer_label.add_css_class("caption");
        transfer_label.set_halign(gtk::Align::Start);
        transfer_label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        transfer_row.append(&transfer_label);

        let transfer_bar = gtk::ProgressBar::new();
        transfer_bar.set_hexpand(true);
        transfer_bar.set_valign(gtk::Align::Center);
        transfer_row.append(&transfer_bar);

        let transfer_cancel_btn = gtk::Button::from_icon_name("process-stop-symbolic");
        transfer_cancel_btn.add_css_class("flat");
        transfer_cancel_btn.set_tooltip_text(Some(crate::tr_en!("Cancel the transfer")));
        transfer_row.append(&transfer_cancel_btn);

        widget.append(&transfer_row);

        // ----------------------------------------------------------------
        // Column sort header
        // ----------------------------------------------------------------
        let sort_header = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        sort_header.set_margin_start(12);
        sort_header.set_margin_end(12);
        sort_header.set_margin_top(6);
        sort_header.set_margin_bottom(6);
        sort_header.add_css_class("dim-label");

        let sort_btn_name = gtk::Button::with_label(crate::tr_en!("Name ▲"));
        sort_btn_name.add_css_class("flat");
        sort_btn_name.set_hexpand(true);
        sort_btn_name.set_halign(gtk::Align::Start);
        sort_header.append(&sort_btn_name);

        let sort_btn_size = gtk::Button::with_label(crate::tr_en!("Size"));
        sort_btn_size.add_css_class("flat");
        sort_btn_size.set_size_request(110, -1);
        sort_header.append(&sort_btn_size);

        let sort_btn_modified = gtk::Button::with_label(crate::tr_en!("Modified"));
        sort_btn_modified.add_css_class("flat");
        sort_btn_modified.set_size_request(180, -1);
        sort_header.append(&sort_btn_modified);

        widget.append(&sort_header);
        widget.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

        // ----------------------------------------------------------------
        // File list
        // ----------------------------------------------------------------
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

        file_list_box.set_placeholder(Some(
            &gtk::Label::builder()
                .label(crate::tr_en!("Login to browse your VOSpace files"))
                .css_classes(vec!["dim-label".to_string()])
                .margin_top(24)
                .margin_bottom(24)
                .build(),
        ));

        scrolled.set_child(Some(&file_list_box));
        widget.append(&scrolled);

        // ----------------------------------------------------------------
        // Status bar
        // ----------------------------------------------------------------
        let status_label = gtk::Label::new(None);
        status_label.add_css_class("dim-label");
        status_label.add_css_class("caption");
        status_label.set_margin_start(12);
        status_label.set_margin_end(12);
        status_label.set_margin_bottom(6);
        status_label.set_halign(gtk::Align::Start);
        widget.append(&status_label);

        // ----------------------------------------------------------------
        // Assemble
        // ----------------------------------------------------------------
        let browser = Rc::new(VoSpaceBrowser {
            widget,
            services,
            current_path: Rc::new(RefCell::new(String::new())),
            file_list_box,
            breadcrumb_label,
            status_label,
            nodes: Rc::new(RefCell::new(Vec::new())),
            sort_state: Rc::new(RefCell::new(SortState::default())),
            sort_btn_name,
            sort_btn_size,
            sort_btn_modified,
            toast_overlay: Rc::new(RefCell::new(None)),
            transfer_row: transfer_row.clone(),
            transfer_label: transfer_label.clone(),
            transfer_bar: transfer_bar.clone(),
            transfer_cancel: Rc::new(RefCell::new(None)),
        });

        // ----------------------------------------------------------------
        // Toolbar signal connections
        // ----------------------------------------------------------------
        {
            let b = browser.clone();
            transfer_cancel_btn.connect_clicked(move |_| {
                if let Some(c) = b.transfer_cancel.borrow().as_ref() {
                    c.cancel();
                }
                // The strip stays up until the transfer notices and unwinds, so
                // the user sees that the request landed rather than a control
                // that vanished with the work still running.
                b.transfer_label.set_text(crate::tr_en!("Cancelling…"));
            });
        }
        {
            let b = browser.clone();
            refresh_btn.connect_clicked(move |_| {
                let b = b.clone();
                // Reload, not refresh: pressing Refresh and being handed the
                // cached listing is the one answer this button must never give.
                glib::spawn_future_local(async move { b.reload().await });
            });
        }
        {
            let b = browser.clone();
            up_btn.connect_clicked(move |_| {
                let b = b.clone();
                glib::spawn_future_local(async move { b.go_up().await });
            });
        }
        {
            let b = browser.clone();
            new_folder_btn.connect_clicked(move |_| {
                let b = b.clone();
                glib::spawn_future_local(async move { b.create_folder_dialog().await });
            });
        }
        {
            let b = browser.clone();
            upload_btn.connect_clicked(move |btn| {
                let b = b.clone();
                let btn = btn.clone();
                glib::spawn_future_local(async move { b.upload_files_dialog(&btn).await });
            });
        }
        {
            let b = browser.clone();
            copy_path_btn.connect_clicked(move |btn| {
                let b = b.clone();
                let btn = btn.clone();
                glib::spawn_future_local(async move {
                    let current = b.current_path.borrow().clone();
                    if let Some(vos_path) = b.node_uri(&current).await {
                        btn.display().clipboard().set_text(&vos_path);
                        b.show_toast(&crate::tr_fmt!("Copied: {}", vos_path));
                    }
                });
            });
        }

        // ----------------------------------------------------------------
        // Sort column header connections
        // ----------------------------------------------------------------
        {
            let b = browser.clone();
            browser.sort_btn_name.connect_clicked(move |_| {
                b.sort_state.borrow_mut().toggle(SortColumn::Name);
                let b = b.clone();
                glib::spawn_future_local(async move { b.redisplay_sorted() });
            });
        }
        {
            let b = browser.clone();
            browser.sort_btn_size.connect_clicked(move |_| {
                b.sort_state.borrow_mut().toggle(SortColumn::Size);
                let b = b.clone();
                glib::spawn_future_local(async move { b.redisplay_sorted() });
            });
        }
        {
            let b = browser.clone();
            browser.sort_btn_modified.connect_clicked(move |_| {
                b.sort_state.borrow_mut().toggle(SortColumn::Modified);
                let b = b.clone();
                glib::spawn_future_local(async move { b.redisplay_sorted() });
            });
        }

        // Drag-and-drop upload: accept gdk::FileList on the file list box
        {
            let b = browser.clone();
            let drop_target = gtk::DropTarget::new(
                gtk::gdk::FileList::static_type(),
                gtk::gdk::DragAction::COPY,
            );
            drop_target.connect_drop(move |_, value, _, _| {
                if let Ok(file_list) = value.get::<gtk::gdk::FileList>() {
                    let paths: Vec<std::path::PathBuf> =
                        file_list.files().iter().filter_map(|f| f.path()).collect();
                    if !paths.is_empty() {
                        let b = b.clone();
                        glib::spawn_future_local(async move {
                            b.upload_local_paths(paths).await;
                        });
                        return true;
                    }
                }
                false
            });
            browser.file_list_box.add_controller(drop_target);
        }

        browser
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.widget
    }

    /// Inject the application's ToastOverlay so the browser can show notifications.
    pub fn set_toast_overlay(&self, overlay: adw::ToastOverlay) {
        *self.toast_overlay.borrow_mut() = Some(overlay);
    }

    // -----------------------------------------------------------------------
    // Refresh / listing
    // -----------------------------------------------------------------------

    /// Show the transfer strip for `label` and hand back the token that stops it.
    ///
    /// One place decides what a transfer looks like, so an upload and a download
    /// cannot drift into looking like different features.
    fn begin_transfer(&self, label: &str) -> crate::services::transfer::Cancel {
        let cancel = crate::services::transfer::Cancel::new();
        *self.transfer_cancel.borrow_mut() = Some(cancel.clone());
        self.transfer_label.set_text(label);
        self.transfer_bar.set_fraction(0.0);
        self.transfer_bar.set_text(None);
        self.transfer_row.set_visible(true);
        cancel
    }

    /// Move the bar. `total` is `None` when the server never said how big it is,
    /// in which case the bar pulses rather than lying about a fraction.
    fn report_transfer(&self, done: u64, total: Option<u64>) {
        match total {
            Some(t) if t > 0 => {
                self.transfer_bar
                    .set_fraction((done as f64 / t as f64).min(1.0));
            }
            _ => self.transfer_bar.pulse(),
        }
        self.transfer_bar.set_show_text(true);
        self.transfer_bar
            .set_text(Some(&crate::services::transfer::format_download_amount(
                done, total,
            )));
    }

    fn end_transfer(&self) {
        *self.transfer_cancel.borrow_mut() = None;
        self.transfer_row.set_visible(false);
        self.transfer_bar.set_fraction(0.0);
    }

    /// Re-read this folder from the service, ignoring anything cached.
    ///
    /// What every mutation must call. `refresh` is allowed to answer from a
    /// 5-minute-fresh cache — right for navigating back into a folder, wrong
    /// immediately after changing one: the listing it serves is the listing
    /// from before the change.
    pub async fn reload(self: &Rc<Self>) {
        let path = self.current_path.borrow().clone();
        self.services
            .cache
            .forget(&crate::services::cache_service::CacheKey::VoSpaceNodes { path });
        self.refresh().await;
    }

    pub async fn refresh(self: &Rc<Self>) {
        use crate::services::cache_service::{CacheKey, Freshness};
        use crate::services::health_tracker::{ServiceName, ServiceStatus};

        let svc = self.services.clone();
        let path = self.current_path.borrow().clone();
        let cache_key = CacheKey::VoSpaceNodes { path: path.clone() };

        // Serve fresh cache without hitting the network
        if let Some(entry) = self
            .services
            .cache
            .read::<Vec<crate::models::VoSpaceNode>>(&cache_key)
        {
            if self.services.cache.entry_freshness(&cache_key, &entry) == Freshness::Fresh {
                *self.nodes.borrow_mut() = entry.data;
                self.redisplay_sorted();
                self.services
                    .health
                    .set(ServiceName::VoSpace, ServiceStatus::Reachable);
                return;
            }
        }

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

        match result {
            Ok(nodes) => {
                self.services.cache.write(&cache_key, &nodes);
                *self.nodes.borrow_mut() = nodes;
                self.redisplay_sorted();
                self.services
                    .health
                    .set(ServiceName::VoSpace, ServiceStatus::Reachable);
            }
            Err(e) => {
                // Network failed — serve stale cache if available
                if let Some(entry) = self
                    .services
                    .cache
                    .read::<Vec<crate::models::VoSpaceNode>>(&cache_key)
                {
                    let time_label = self
                        .services
                        .cache
                        .cached_time_label(&cache_key)
                        .unwrap_or_else(|| "unknown".into());
                    *self.nodes.borrow_mut() = entry.data;
                    self.redisplay_sorted();
                    self.status_label
                        .set_text(&crate::tr_fmt!("Cached listing from {}", time_label));
                    self.services.toast.toast(crate::tr_fmt!(
                        "VOSpace unreachable — showing cached listing from {}",
                        time_label
                    ));
                } else {
                    self.status_label.set_text(&crate::tr_fmt!("Error: {}", e));
                }
                self.services.health.set(
                    ServiceName::VoSpace,
                    ServiceStatus::Unreachable {
                        since: chrono::Utc::now(),
                        reason: e.to_string(),
                    },
                );
            }
        }
    }

    // -----------------------------------------------------------------------
    // Sort and display
    // -----------------------------------------------------------------------

    /// Sort the cached nodes and repopulate the list box without fetching.
    fn redisplay_sorted(self: &Rc<Self>) {
        let mut nodes = self.nodes.borrow().clone();
        let state = self.sort_state.borrow().clone();

        // Folders always before files; within each group apply the chosen sort.
        nodes.sort_by(|a, b| {
            let a_folder = a.is_container();
            let b_folder = b.is_container();
            if a_folder != b_folder {
                return if a_folder {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Greater
                };
            }
            let ord = match state.column {
                SortColumn::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                SortColumn::Size => a.size.cmp(&b.size),
                SortColumn::Modified => a
                    .date
                    .as_deref()
                    .unwrap_or("")
                    .cmp(b.date.as_deref().unwrap_or("")),
            };
            if state.order == SortOrder::Descending {
                ord.reverse()
            } else {
                ord
            }
        });

        // Update column header button labels
        self.sort_btn_name
            .set_label(&crate::tr_fmt!("Name{}", state.indicator(SortColumn::Name)));
        self.sort_btn_size
            .set_label(&crate::tr_fmt!("Size{}", state.indicator(SortColumn::Size)));
        self.sort_btn_modified.set_label(&crate::tr_fmt!(
            "Modified{}",
            state.indicator(SortColumn::Modified)
        ));

        // Repopulate
        while let Some(child) = self.file_list_box.first_child() {
            self.file_list_box.remove(&child);
        }

        let count = nodes.len();
        for (idx, node) in nodes.iter().enumerate() {
            let row = self.make_file_row(node, idx);
            self.file_list_box.append(&row);
        }

        let path = self.current_path.borrow().clone();
        self.breadcrumb_label.set_text(&format!("/{}", path));
        self.status_label
            .set_text(&crate::tr_fmt!("{} items", count));

        // Store sorted order so row index callbacks stay consistent
        *self.nodes.borrow_mut() = nodes;
    }

    // -----------------------------------------------------------------------
    // Row construction
    // -----------------------------------------------------------------------

    fn make_file_row(self: &Rc<Self>, node: &VoSpaceNode, idx: usize) -> gtk::ListBoxRow {
        let icon_name = if node.is_container() {
            "folder-symbolic"
        } else if is_fits_file(&node.name) {
            "image-x-generic-symbolic"
        } else {
            match node.content_type.as_deref() {
                Some(ct) if ct.contains("image") => "image-x-generic-symbolic",
                Some(ct) if ct.contains("text") => "text-x-generic-symbolic",
                _ => "text-x-generic-symbolic",
            }
        };

        // Row content box
        let row_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        row_box.set_margin_start(6);
        row_box.set_margin_end(6);
        row_box.set_margin_top(6);
        row_box.set_margin_bottom(6);

        let icon = gtk::Image::from_icon_name(icon_name);
        icon.set_pixel_size(16);
        row_box.append(&icon);

        let name_label = gtk::Label::new(Some(&node.name));
        name_label.set_hexpand(true);
        name_label.set_halign(gtk::Align::Start);
        name_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        row_box.append(&name_label);

        let size_text = if node.is_container() {
            crate::tr_en!("Folder").to_string()
        } else {
            node.size_display()
        };
        let size_label = gtk::Label::new(Some(&size_text));
        size_label.add_css_class("dim-label");
        size_label.set_width_chars(12);
        size_label.set_halign(gtk::Align::End);
        row_box.append(&size_label);

        let date_label = gtk::Label::new(Some(node.date.as_deref().unwrap_or("\u{2014}")));
        date_label.add_css_class("dim-label");
        date_label.set_width_chars(20);
        date_label.set_halign(gtk::Align::End);
        row_box.append(&date_label);

        // Download button (files only)
        if !node.is_container() {
            let dl_btn = gtk::Button::from_icon_name("folder-download-symbolic");
            dl_btn.set_tooltip_text(Some(crate::tr_en!("Download")));
            dl_btn.set_valign(gtk::Align::Center);
            dl_btn.add_css_class("flat");
            let b = self.clone();
            dl_btn.connect_clicked(move |_| {
                let b = b.clone();
                glib::spawn_future_local(async move { b.action_download(idx).await });
            });
            row_box.append(&dl_btn);
        }

        // Delete button
        {
            let del_btn = gtk::Button::from_icon_name("user-trash-symbolic");
            del_btn.set_tooltip_text(Some(crate::tr_en!("Delete")));
            del_btn.set_valign(gtk::Align::Center);
            del_btn.add_css_class("flat");
            let b = self.clone();
            let del_btn2 = del_btn.clone();
            del_btn.connect_clicked(move |_| {
                let b = b.clone();
                let del_btn2 = del_btn2.clone();
                glib::spawn_future_local(async move { b.action_delete(idx, &del_btn2).await });
            });
            row_box.append(&del_btn);
        }

        let row = gtk::ListBoxRow::new();
        row.set_child(Some(&row_box));
        // Disable the default activate-on-click; we handle via GestureClick
        row.set_activatable(false);

        // ----------------------------------------------------------------
        // Double-click gesture (button 1)
        // ----------------------------------------------------------------
        {
            let b = self.clone();
            let gc = gtk::GestureClick::new();
            gc.set_button(1);
            gc.connect_pressed(move |g, n_press, _x, _y| {
                if n_press == 2 {
                    g.set_state(gtk::EventSequenceState::Claimed);
                    let b = b.clone();
                    glib::spawn_future_local(async move { b.on_double_click(idx).await });
                }
            });
            row.add_controller(gc);
        }

        // ----------------------------------------------------------------
        // Right-click context menu (button 3)
        // ----------------------------------------------------------------
        {
            let b = self.clone();
            let row_ref = row.clone();
            let node_is_container = node.is_container();
            let node_is_fits = is_fits_file(&node.name);
            let node_is_notebook = is_notebook_file(&node.name);

            let gc = gtk::GestureClick::new();
            gc.set_button(3);
            gc.connect_pressed(move |g, _n_press, x, y| {
                g.set_state(gtk::EventSequenceState::Claimed);
                build_context_menu(
                    &b,
                    &row_ref,
                    idx,
                    x,
                    y,
                    node_is_container,
                    node_is_fits,
                    node_is_notebook,
                );
            });
            row.add_controller(gc);
        }

        row
    }

    // -----------------------------------------------------------------------
    // Navigation
    // -----------------------------------------------------------------------

    async fn go_up(self: &Rc<Self>) {
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

    async fn navigate_into(self: &Rc<Self>, idx: usize) {
        let node = self.nodes.borrow().get(idx).cloned();
        if let Some(node) = node {
            if node.is_container() {
                let current = self.current_path.borrow().clone();
                let new_path = if current.is_empty() {
                    node.name.clone()
                } else {
                    format!("{}/{}", current, node.name)
                };
                *self.current_path.borrow_mut() = new_path;
                self.refresh().await;
            }
        }
    }

    async fn on_double_click(self: &Rc<Self>, idx: usize) {
        let node = self.nodes.borrow().get(idx).cloned();
        if let Some(node) = node {
            if node.is_container() {
                self.navigate_into(idx).await;
            } else if is_fits_file(&node.name) {
                self.action_open_fits(idx).await;
            } else {
                self.action_download(idx).await;
            }
        }
    }

    // -----------------------------------------------------------------------
    // Actions
    // -----------------------------------------------------------------------

    async fn action_download(self: &Rc<Self>, idx: usize) {
        let node = match self.nodes.borrow().get(idx).cloned() {
            Some(n) if !n.is_container() => n,
            _ => return,
        };

        let remote_path = self.build_remote_path(&node.name);

        let dialog = gtk::FileDialog::builder()
            .title(crate::tr_en!("Save As"))
            .initial_name(&node.name)
            .build();

        let root = self.widget.root().and_downcast::<gtk::Window>();
        let dest_file = match dialog.save_future(root.as_ref()).await {
            Ok(f) => f,
            Err(_) => return,
        };
        let local_path = match dest_file.path() {
            Some(p) => p,
            None => return,
        };

        let svc = self.services.clone();
        let fname = node.name.clone();

        let cancel = self.begin_transfer(&crate::tr_fmt!("Downloading {}…", fname));
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(u64, Option<u64>)>();
        {
            let this = self.clone();
            glib::spawn_future_local(async move {
                while let Some((done, total)) = rx.recv().await {
                    this.report_transfer(done, total);
                }
            });
        }

        let cancel_task = cancel.clone();
        let toast = self.services.toast.clone();
        let label = fname.clone();
        let result = self
            .services
            .spawn(async move {
                let token = svc.get_token().await;
                let username = svc.get_username().await;
                let (Some(tok), Some(user)) = (token, username) else {
                    return Err("not signed in".to_string());
                };
                let url = svc.endpoints.vospace_files_url(&user, &remote_path);
                // The same streaming download every other page uses: bounded
                // memory, a `.tmp` that is renamed only once complete, and the
                // partial file removed if it does not.
                crate::services::transfer::download_with_progress(
                    &url,
                    Some(&tok),
                    &local_path,
                    &toast,
                    &label,
                    Some(tx),
                    &cancel_task,
                )
                .await
            })
            .await;
        self.end_transfer();

        match result {
            Ok(bytes) => self.show_toast(&crate::tr_fmt!("Downloaded {} ({} bytes)", fname, bytes)),
            Err(e) if cancel.is_cancelled() => {
                let _ = e;
                self.show_toast(&crate::tr_fmt!("Download cancelled: {}", fname));
            }
            Err(e) => self.show_toast(&crate::tr_fmt!("Download failed: {}", e)),
        }
    }

    /// Fetch `node` to a temp file so a viewer can open it — streamed, watched
    /// and cancellable like any other transfer.
    ///
    /// The three "open in …" actions each had their own copy of this, and each
    /// buffered the whole file into memory first. Opening a 3 GB cube from
    /// VOSpace is the case that most needs a progress bar and a way out, and it
    /// was the case with neither.
    async fn fetch_to_temp(self: &Rc<Self>, name: &str) -> Option<std::path::PathBuf> {
        let remote_path = self.build_remote_path(name);
        let local_path = std::env::temp_dir().join(name);
        let local_for_task = local_path.clone();
        let svc = self.services.clone();

        let cancel = self.begin_transfer(&crate::tr_fmt!("Downloading {}…", name));
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(u64, Option<u64>)>();
        {
            let this = self.clone();
            glib::spawn_future_local(async move {
                while let Some((done, total)) = rx.recv().await {
                    this.report_transfer(done, total);
                }
            });
        }

        let cancel_task = cancel.clone();
        let toast = self.services.toast.clone();
        let label = name.to_string();
        let result = self
            .services
            .spawn(async move {
                let token = svc.get_token().await;
                let username = svc.get_username().await;
                let (Some(tok), Some(user)) = (token, username) else {
                    return Err("not signed in".to_string());
                };
                let url = svc.endpoints.vospace_files_url(&user, &remote_path);
                crate::services::transfer::download_with_progress(
                    &url,
                    Some(&tok),
                    &local_for_task,
                    &toast,
                    &label,
                    Some(tx),
                    &cancel_task,
                )
                .await
            })
            .await;
        self.end_transfer();

        match result {
            Ok(_) => Some(local_path),
            Err(_) if cancel.is_cancelled() => {
                self.show_toast(&crate::tr_fmt!("Download cancelled: {}", name));
                None
            }
            Err(e) => {
                self.show_toast(&crate::tr_fmt!("Download failed: {}", e));
                None
            }
        }
    }

    /// Download a FITS file to a temp location and open it in the FITS Viewer tab.
    async fn action_open_fits(self: &Rc<Self>, idx: usize) {
        let node = match self.nodes.borrow().get(idx).cloned() {
            Some(n) if is_fits_file(&n.name) => n,
            _ => return,
        };

        let Some(local_path) = self.fetch_to_temp(&node.name).await else {
            return; // fetch_to_temp has already said why
        };

        if let Some(window) = self.widget.root().and_downcast::<adw::ApplicationWindow>() {
            if let Some(app) = window.application() {
                let path_str = local_path.to_string_lossy().to_string();
                let variant = glib::Variant::from(path_str.as_str());
                app.activate_action("open-fits-file", Some(&variant));
            }
        }
        self.show_toast(&crate::tr_fmt!("Opened {} in FITS Viewer", node.name));
    }

    /// Download a FITS cube to a temp location and open it in the Cube Viewer.
    async fn action_open_cube(self: &Rc<Self>, idx: usize) {
        let node = match self.nodes.borrow().get(idx).cloned() {
            Some(n) if is_fits_file(&n.name) => n,
            _ => return,
        };

        let Some(local_path) = self.fetch_to_temp(&node.name).await else {
            return; // fetch_to_temp has already said why
        };

        if let Some(window) = self.widget.root().and_downcast::<adw::ApplicationWindow>() {
            if let Some(app) = window.application() {
                let path_str = local_path.to_string_lossy().to_string();
                let variant = glib::Variant::from(path_str.as_str());
                app.activate_action("open-cube-file", Some(&variant));
            }
        }
        self.show_toast(&crate::tr_fmt!("Opened {} in Cube Viewer", node.name));
    }

    /// Download a notebook file to a temp location and open it in the Notebook tab.
    async fn action_open_notebook(self: &Rc<Self>, idx: usize) {
        let node = match self.nodes.borrow().get(idx).cloned() {
            Some(n) if is_notebook_file(&n.name) => n,
            _ => return,
        };

        let Some(local_path) = self.fetch_to_temp(&node.name).await else {
            return; // fetch_to_temp has already said why
        };

        if let Some(window) = self.widget.root().and_downcast::<adw::ApplicationWindow>() {
            if let Some(app) = window.application() {
                let path_str = local_path.to_string_lossy().to_string();
                let variant = glib::Variant::from(path_str.as_str());
                app.activate_action("open-notebook-file", Some(&variant));
            }
        }
        self.show_toast(&crate::tr_fmt!("Opened {} in Notebook", node.name));
    }

    fn action_copy_node_path(self: &Rc<Self>, idx: usize) {
        let node = match self.nodes.borrow().get(idx).cloned() {
            Some(n) => n,
            None => return,
        };
        let this = self.clone();
        glib::spawn_future_local(async move {
            let path = this.build_remote_path(&node.name);
            if let Some(vos_path) = this.node_uri(&path).await {
                this.widget.display().clipboard().set_text(&vos_path);
                this.show_toast(&crate::tr_fmt!("Copied: {}", vos_path));
            }
        });
    }

    /// The `vos://` URI for a path in this browser, or `None` when signed out.
    ///
    /// Both Copy-path actions used to build one inline, without the
    /// `home/<username>` root the address actually carries — so what landed on
    /// the clipboard was a URI `vcp` and `vls` reject. There is one way to name
    /// a node, and it lives with the endpoints.
    async fn node_uri(&self, path: &str) -> Option<String> {
        let username = self.services.get_username().await?;
        Some(
            self.services
                .endpoints
                .vospace_node_uri(&username, path.trim_matches('/')),
        )
    }

    async fn action_delete(self: &Rc<Self>, idx: usize, parent_widget: &impl IsA<gtk::Widget>) {
        let node = match self.nodes.borrow().get(idx).cloned() {
            Some(n) => n,
            None => return,
        };
        if !self.confirm_delete(parent_widget, &node.name).await {
            return;
        }
        self.do_delete(&node.name).await;
    }

    async fn action_delete_from_menu(self: &Rc<Self>, idx: usize) {
        let node = match self.nodes.borrow().get(idx).cloned() {
            Some(n) => n,
            None => return,
        };
        if !self.confirm_delete(&self.widget, &node.name).await {
            return;
        }
        self.do_delete(&node.name).await;
    }

    async fn do_delete(self: &Rc<Self>, name: &str) {
        let remote_path = self.build_remote_path(name);
        let svc = self.services.clone();
        let name_owned = name.to_string();

        let result = self
            .services
            .spawn(async move {
                let token = svc.get_token().await;
                let username = svc.get_username().await;
                match (token, username) {
                    (Some(tok), Some(user)) => {
                        svc.vospace.delete_node(&tok, &user, &remote_path).await
                    }
                    _ => Err(crate::services::ApiError::Unauthorized),
                }
            })
            .await;

        match result {
            Ok(()) => {
                self.show_toast(&crate::tr_fmt!("Deleted {}", name_owned));
                self.reload().await;
            }
            Err(e) => {
                self.show_toast(&crate::tr_fmt!("Delete failed: {}", e));
            }
        }
    }

    async fn action_share(self: &Rc<Self>, idx: usize) {
        let node = match self.nodes.borrow().get(idx).cloned() {
            Some(n) => n,
            None => return,
        };
        let path = self.build_remote_path(&node.name);

        // Fetch the node's current ACL (a directory listing may omit ACL props),
        // so the dialog prefills correctly and Save never silently revokes.
        let svc = self.services.clone();
        let path_for_get = path.clone();
        let fetched = self
            .services
            .spawn(async move {
                let token = svc.get_token().await;
                let username = svc.get_username().await;
                match (token, username) {
                    (Some(tok), Some(user)) => {
                        svc.vospace.get_node(&tok, &user, &path_for_get).await.ok()
                    }
                    _ => None,
                }
            })
            .await;
        let current = fetched.unwrap_or_else(|| node.clone());

        let result = match crate::ui::share_dialog::show_share_dialog(
            &self.widget,
            &node.name,
            current.is_public,
            &current.group_read,
            &current.group_write,
        )
        .await
        {
            Some(r) => r,
            None => return,
        };

        let node_type = node.node_type.clone();
        let svc = self.services.clone();
        let path_apply = path.clone();
        let apply = self
            .services
            .spawn(async move {
                let token = svc.get_token().await;
                let username = svc.get_username().await;
                match (token, username) {
                    (Some(tok), Some(user)) => {
                        svc.vospace
                            .set_node_acl(
                                &tok,
                                &user,
                                &path_apply,
                                &node_type,
                                Some(result.group_read),
                                Some(result.group_write),
                                Some(result.is_public),
                            )
                            .await
                    }
                    _ => Err(crate::services::ApiError::Unauthorized),
                }
            })
            .await;

        match apply {
            Ok(()) => {
                self.show_toast(&crate::tr_fmt!("Sharing updated for {}", node.name));
                self.reload().await;
            }
            Err(e) => self.show_toast(&crate::tr_fmt!("Share failed: {}", e)),
        }
    }

    async fn action_rename(self: &Rc<Self>, idx: usize) {
        let node = match self.nodes.borrow().get(idx).cloned() {
            Some(n) => n,
            None => return,
        };
        if node.is_container() {
            self.show_toast(crate::tr_en!("Rename not supported for folders yet"));
            return;
        }

        let new_name = match crate::ui::rename_dialog::show_rename_dialog(
            &self.widget,
            crate::tr_en!("Rename File"),
            &node.name,
        )
        .await
        {
            Some(name) => name,
            None => return,
        };

        let old_path = self.build_remote_path(&node.name);
        let svc = self.services.clone();
        let old_name = node.name.clone();
        let new_name_clone = new_name.clone();

        let result = self
            .services
            .spawn(async move {
                let token = svc.get_token().await;
                let username = svc.get_username().await;
                match (token, username) {
                    (Some(tok), Some(user)) => {
                        svc.vospace
                            .rename_file(&tok, &user, &old_path, &new_name_clone)
                            .await
                    }
                    _ => Err(crate::services::ApiError::Unauthorized),
                }
            })
            .await;

        match result {
            Ok(()) => {
                self.show_toast(&crate::tr_fmt!("Renamed {} → {}", old_name, new_name));
                self.reload().await;
            }
            Err(e) => {
                self.show_toast(&crate::tr_fmt!("Rename failed: {}", e));
            }
        }
    }

    // -----------------------------------------------------------------------
    // Create folder dialog
    // -----------------------------------------------------------------------

    async fn create_folder_dialog(self: &Rc<Self>) {
        let dialog = adw::Window::builder()
            .title(crate::tr_en!("New Folder"))
            .default_width(crate::ui::fit::PROMPT)
            .modal(true)
            .build();

        if let Some(root) = self.widget.root().and_downcast::<gtk::Window>() {
            dialog.set_transient_for(Some(&root));
        }

        let toolbar_view = adw::ToolbarView::new();
        let header = adw::HeaderBar::new();
        toolbar_view.add_top_bar(&header);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
        content.set_margin_start(24);
        content.set_margin_end(24);
        content.set_margin_top(12);
        content.set_margin_bottom(24);

        let entry = gtk::Entry::new();
        entry.set_placeholder_text(Some(crate::tr_en!("Folder name")));
        entry.set_activates_default(true);
        content.append(&entry);

        let btn_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        btn_row.set_halign(gtk::Align::End);
        let cancel_btn = gtk::Button::with_label(crate::tr_en!("Cancel"));
        let create_btn = gtk::Button::with_label(crate::tr_en!("Create"));
        create_btn.add_css_class("suggested-action");
        create_btn.set_receives_default(true);
        btn_row.append(&cancel_btn);
        btn_row.append(&create_btn);
        content.append(&btn_row);

        toolbar_view.set_content(Some(&content));
        dialog.set_content(Some(&toolbar_view));

        // Use a channel to wait for the dialog to close
        let (tx, rx) = tokio::sync::oneshot::channel::<Option<String>>();
        let tx = Rc::new(RefCell::new(Some(tx)));

        {
            let dialog = dialog.clone();
            let tx = tx.clone();
            cancel_btn.connect_clicked(move |_| {
                if let Some(tx) = tx.borrow_mut().take() {
                    let _ = tx.send(None);
                }
                dialog.close();
            });
        }
        {
            let dialog = dialog.clone();
            let entry = entry.clone();
            let tx = tx.clone();
            create_btn.connect_clicked(move |_| {
                let name = entry.text().to_string();
                if !name.is_empty() {
                    if let Some(tx) = tx.borrow_mut().take() {
                        let _ = tx.send(Some(name));
                    }
                    dialog.close();
                }
            });
        }
        {
            let tx = tx.clone();
            dialog.connect_close_request(move |_| {
                // If user closes with the window button without clicking Create/Cancel
                if let Some(tx) = tx.borrow_mut().take() {
                    let _ = tx.send(None);
                }
                glib::Propagation::Proceed
            });
        }

        dialog.present();

        if let Ok(Some(name)) = rx.await {
            let remote_path = self.build_remote_path(&name);
            let svc = self.services.clone();
            let name_owned = name.clone();

            let result = self
                .services
                .spawn(async move {
                    let token = svc.get_token().await;
                    let username = svc.get_username().await;
                    match (token, username) {
                        (Some(tok), Some(user)) => {
                            svc.vospace.create_folder(&tok, &user, &remote_path).await
                        }
                        _ => Err(crate::services::ApiError::Unauthorized),
                    }
                })
                .await;

            match result {
                Ok(()) => {
                    self.show_toast(&crate::tr_fmt!("Created folder '{}'", name_owned));
                    self.reload().await;
                }
                Err(e) => {
                    self.show_toast(&crate::tr_fmt!("Failed to create folder: {}", e));
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Upload
    // -----------------------------------------------------------------------

    async fn upload_files_dialog(self: &Rc<Self>, parent: &impl IsA<gtk::Widget>) {
        let root = parent.root().and_downcast::<gtk::Window>();

        let dialog = gtk::FileDialog::builder()
            .title(crate::tr_en!("Upload Files"))
            .build();

        let files = match dialog.open_multiple_future(root.as_ref()).await {
            Ok(f) => f,
            Err(_) => return,
        };

        let n = files.n_items();
        if n == 0 {
            return;
        }

        let mut paths = Vec::with_capacity(n as usize);
        for i in 0..n {
            if let Some(file) = files.item(i).and_downcast::<gtk::gio::File>() {
                if let Some(path) = file.path() {
                    paths.push(path);
                }
            }
        }

        self.upload_local_paths(paths).await;
    }

    /// Upload a list of local file paths to the current remote directory.
    /// Shared entry point used by both the Upload button and drag-drop.
    async fn upload_local_paths(self: &Rc<Self>, paths: Vec<std::path::PathBuf>) {
        if paths.is_empty() {
            return;
        }

        let current = self.current_path.borrow().clone();
        let total = paths.len();
        let mut any_error = false;

        for local_path in paths {
            let filename = local_path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();

            if filename.is_empty() {
                continue;
            }

            let remote_path = if current.is_empty() {
                filename.clone()
            } else {
                format!("{}/{}", current, filename)
            };

            let content_type = guess_content_type(&filename).to_string();
            let svc = self.services.clone();
            let fname = filename.clone();

            let cancel = self.begin_transfer(&crate::tr_fmt!("Uploading {}…", fname));
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(u64, Option<u64>)>();
            {
                // Drain progress on the GTK thread while the transfer runs on
                // tokio; the transfer never touches a widget.
                let this = self.clone();
                glib::spawn_future_local(async move {
                    while let Some((done, total)) = rx.recv().await {
                        this.report_transfer(done, total);
                    }
                });
            }

            let cancel_task = cancel.clone();
            let remote_for_cleanup = remote_path.clone();
            let result = self
                .services
                .spawn(async move {
                    let token = svc.get_token().await;
                    let username = svc.get_username().await;
                    let (Some(tok), Some(user)) = (token, username) else {
                        return Err(crate::services::transfer::TransferError::Failed(
                            "not signed in".into(),
                        ));
                    };
                    let outcome = svc
                        .vospace
                        .upload_file_streaming(
                            &tok,
                            &user,
                            &remote_path,
                            &local_path,
                            &content_type,
                            Some(tx),
                            &cancel_task,
                        )
                        .await;
                    // Graceful cleanup: an interrupted PUT can leave a node of
                    // the wrong length, and a truncated FITS that looks whole is
                    // worse than no file — the next reader gets a header
                    // promising data that is not there.
                    if outcome.is_err() {
                        let _ = svc
                            .vospace
                            .delete_node(&tok, &user, &remote_for_cleanup)
                            .await;
                    }
                    outcome
                })
                .await;
            self.end_transfer();

            match result {
                Ok(_) => self.show_toast(&crate::tr_fmt!("Uploaded {}", fname)),
                Err(crate::services::transfer::TransferError::Cancelled) => {
                    self.show_toast(&crate::tr_fmt!("Upload cancelled: {}", fname));
                    break;
                }
                Err(e) => {
                    self.show_toast(&crate::tr_fmt!("Upload failed for {}: {}", fname, e));
                    any_error = true;
                }
            }
        }

        if !any_error && total > 1 {
            self.show_toast(&crate::tr_fmt!("Uploaded {} files", total));
        }

        self.reload().await;
    }

    // -----------------------------------------------------------------------
    // Confirm delete
    // -----------------------------------------------------------------------

    async fn confirm_delete(&self, parent: &impl IsA<gtk::Widget>, name: &str) -> bool {
        let dialog = adw::MessageDialog::new(
            parent
                .root()
                .and_then(|r| r.downcast::<gtk::Window>().ok())
                .as_ref(),
            Some(crate::tr_en!("Delete Item")),
            Some(&crate::tr_fmt!(
                "Are you sure you want to delete '{}'? This cannot be undone.",
                name
            )),
        );
        dialog.add_response("cancel", crate::tr_en!("Cancel"));
        dialog.add_response("delete", crate::tr_en!("Delete"));
        dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");

        let result = std::rc::Rc::new(std::cell::RefCell::new(false));
        let (sender, receiver) = tokio::sync::oneshot::channel::<()>();
        let sender = std::rc::Rc::new(std::cell::RefCell::new(Some(sender)));

        {
            let result = result.clone();
            let sender = sender.clone();
            dialog.connect_response(None, move |_, response| {
                *result.borrow_mut() = response == "delete";
                if let Some(s) = sender.borrow_mut().take() {
                    let _ = s.send(());
                }
            });
        }

        dialog.present();
        let _ = receiver.await;
        let val = *result.borrow();
        val
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    /// Build a remote path by joining the current directory with `name`.
    fn build_remote_path(&self, name: &str) -> String {
        let current = self.current_path.borrow().clone();
        if current.is_empty() {
            name.to_string()
        } else {
            format!("{}/{}", current, name)
        }
    }

    fn show_toast(&self, message: &str) {
        let toast = adw::Toast::new(message);
        if let Some(ref overlay) = *self.toast_overlay.borrow() {
            overlay.add_toast(toast);
        }
    }
}

// ---------------------------------------------------------------------------
// Context menu (free function to avoid Rc<Self> in closure arguments)
// ---------------------------------------------------------------------------

fn build_context_menu(
    browser: &Rc<VoSpaceBrowser>,
    row: &gtk::ListBoxRow,
    idx: usize,
    x: f64,
    y: f64,
    node_is_container: bool,
    node_is_fits: bool,
    node_is_notebook: bool,
) {
    let menu = gtk::gio::Menu::new();
    if node_is_fits {
        menu.append(
            Some(crate::tr_en!("Open in FITS Viewer")),
            Some("row.open-fits"),
        );
        menu.append(
            Some(crate::tr_en!("Open in Cube Viewer")),
            Some("row.open-cube"),
        );
    }
    if node_is_notebook {
        menu.append(
            Some(crate::tr_en!("Open in Notebook")),
            Some("row.open-notebook"),
        );
    }
    if !node_is_container {
        menu.append(Some(crate::tr_en!("Download")), Some("row.download"));
    }
    menu.append(Some(crate::tr_en!("Copy Path")), Some("row.copy-path"));
    menu.append(Some(crate::tr_en!("Share…")), Some("row.share"));
    if !node_is_container {
        menu.append(Some(crate::tr_en!("Rename")), Some("row.rename"));
    }
    menu.append(Some(crate::tr_en!("Delete")), Some("row.delete"));

    let popover = gtk::PopoverMenu::from_model(Some(&menu));
    popover.set_parent(row);
    let rect = gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1);
    popover.set_pointing_to(Some(&rect));
    popover.set_has_arrow(false);

    let ag = gtk::gio::SimpleActionGroup::new();

    // open-fits
    {
        let b = browser.clone();
        let action = gtk::gio::SimpleAction::new("open-fits", None);
        action.set_enabled(node_is_fits);
        action.connect_activate(move |_, _| {
            let b = b.clone();
            glib::spawn_future_local(async move { b.action_open_fits(idx).await });
        });
        ag.add_action(&action);
    }

    // open-cube
    {
        let b = browser.clone();
        let action = gtk::gio::SimpleAction::new("open-cube", None);
        action.set_enabled(node_is_fits);
        action.connect_activate(move |_, _| {
            let b = b.clone();
            glib::spawn_future_local(async move { b.action_open_cube(idx).await });
        });
        ag.add_action(&action);
    }

    // open-notebook
    {
        let b = browser.clone();
        let action = gtk::gio::SimpleAction::new("open-notebook", None);
        action.set_enabled(node_is_notebook);
        action.connect_activate(move |_, _| {
            let b = b.clone();
            glib::spawn_future_local(async move { b.action_open_notebook(idx).await });
        });
        ag.add_action(&action);
    }

    // download
    {
        let b = browser.clone();
        let action = gtk::gio::SimpleAction::new("download", None);
        action.connect_activate(move |_, _| {
            let b = b.clone();
            glib::spawn_future_local(async move { b.action_download(idx).await });
        });
        ag.add_action(&action);
    }

    // copy-path
    {
        let b = browser.clone();
        let action = gtk::gio::SimpleAction::new("copy-path", None);
        action.connect_activate(move |_, _| {
            b.action_copy_node_path(idx);
        });
        ag.add_action(&action);
    }

    // share
    {
        let b = browser.clone();
        let popover_weak = popover.downgrade();
        let action = gtk::gio::SimpleAction::new("share", None);
        action.connect_activate(move |_, _| {
            if let Some(p) = popover_weak.upgrade() {
                p.popdown();
            }
            let b = b.clone();
            glib::spawn_future_local(async move { b.action_share(idx).await });
        });
        ag.add_action(&action);
    }

    // rename (files only)
    {
        let b = browser.clone();
        let popover_weak = popover.downgrade();
        let action = gtk::gio::SimpleAction::new("rename", None);
        action.set_enabled(!node_is_container);
        action.connect_activate(move |_, _| {
            if let Some(p) = popover_weak.upgrade() {
                p.popdown();
            }
            let b = b.clone();
            glib::spawn_future_local(async move { b.action_rename(idx).await });
        });
        ag.add_action(&action);
    }

    // delete
    {
        let b = browser.clone();
        let popover_weak = popover.downgrade();
        let action = gtk::gio::SimpleAction::new("delete", None);
        action.connect_activate(move |_, _| {
            if let Some(p) = popover_weak.upgrade() {
                p.popdown();
            }
            let b = b.clone();
            glib::spawn_future_local(async move { b.action_delete_from_menu(idx).await });
        });
        ag.add_action(&action);
    }

    row.insert_action_group("row", Some(&ag));
    popover.popup();
}

#[cfg(test)]
mod cache_tests {
    //! Creating a folder reported success and showed a directory the folder was
    //! not in — because `refresh` may answer from a 5-minute-fresh cache, and
    //! every mutation called it. Creating the same folder again then failed as
    //! already existing, which it was.

    const SOURCE: &str = include_str!("vospace_browser.rs");

    /// Every line that follows a "we just changed this folder" toast.
    fn lines_after_mutation_toasts() -> Vec<(usize, String)> {
        let code = crate::testing::code(SOURCE);
        let lines: Vec<&str> = code.lines().collect();
        let mut out = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            let is_mutation_toast =
                line.contains("show_toast") || (i > 0 && lines[i - 1].contains("show_toast"));
            if !is_mutation_toast {
                continue;
            }
            let window = lines[i..(i + 4).min(lines.len())].join("\n");
            if [
                "Created folder",
                "Deleted",
                "Renamed",
                "Sharing updated",
                "Uploaded",
            ]
            .iter()
            .any(|k| {
                lines[i.saturating_sub(1)..(i + 2).min(lines.len())]
                    .join(" ")
                    .contains(k)
            }) {
                out.push((i + 1, window));
            }
        }
        out
    }

    #[test]
    fn a_folder_that_was_just_changed_is_re_read_not_recalled() {
        let sites = lines_after_mutation_toasts();
        assert!(
            !sites.is_empty(),
            "found no mutation sites — scan is broken"
        );
        for (line, window) in sites {
            if window.contains("refresh().await") && !window.contains("reload().await") {
                panic!(
                    "vospace_browser.rs:{line}: this follows a change to the folder and \
                     calls refresh(), which may answer from cache — the user would be \
                     shown the listing from before their change. Use reload()."
                );
            }
        }
    }

    #[test]
    fn the_refresh_button_does_not_answer_from_cache() {
        let code = crate::testing::code(SOURCE);
        let at = code
            .find("refresh_btn.connect_clicked")
            .expect("the Refresh button is gone");
        let body = &code[at..(at + 400).min(code.len())];
        assert!(
            body.contains("reload().await"),
            "Refresh hands back the cached listing — the one answer that button \
             must never give"
        );
    }
}

#[cfg(test)]
mod transfer_tests {
    //! Storage moved files by reading them entirely into memory: `fs::read` up,
    //! `resp.bytes()` down. A 5 GB cube needed 5 GB of RAM, showed nothing until
    //! it finished, and could not be stopped. The streaming path with progress,
    //! `.tmp` staging and cleanup already existed — in the Search page, which
    //! the one screen whose whole job is moving files did not use.

    const SOURCE: &str = include_str!("vospace_browser.rs");

    #[test]
    fn no_transfer_reads_the_whole_file_into_memory() {
        let code = crate::testing::code(SOURCE);
        for buffered in ["std::fs::read(", "resp.bytes()"] {
            assert!(
                !code.contains(buffered),
                "a transfer is buffering the whole file ({buffered}) — peak memory \
                 becomes the file's size, and there is nothing to report progress from"
            );
        }
    }

    #[test]
    fn both_directions_can_be_watched_and_stopped() {
        let code = crate::testing::code(SOURCE);
        // Every transfer that raises the strip must lower it: a strip left up
        // over a finished transfer is a progress bar that never completes and a
        // Cancel button wired to nothing.
        let begun = code.matches("self.begin_transfer(").count();
        let ended = code.matches("self.end_transfer();").count();
        assert!(begun >= 3, "only {begun} transfers show the strip");
        assert_eq!(
            begun, ended,
            "{begun} transfers begin the strip, {ended} end it"
        );
        assert!(
            code.contains("transfer_cancel_btn.connect_clicked"),
            "the strip has no Cancel"
        );
    }

    #[test]
    fn a_failed_upload_does_not_leave_a_truncated_file_behind() {
        let code = crate::testing::code(SOURCE);
        let at = code
            .find("upload_file_streaming")
            .expect("the streaming upload is gone");
        let after = &code[at..(at + 1200).min(code.len())];
        assert!(
            after.contains("delete_node"),
            "an interrupted PUT can leave a node of the wrong length; a truncated \
             FITS that looks whole is worse than no file"
        );
    }
}
