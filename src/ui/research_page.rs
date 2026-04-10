use crate::services::observation_store::DownloadedObservation;
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
// ResearchPage
// ---------------------------------------------------------------------------

/// The Research page — shows all locally saved CADC observations in a
/// master-detail layout: the list is on the left, the currently selected
/// observation's full metadata, preview image, and action buttons are on
/// the right.  Matches the Windows CanfarDesktop layout.
pub struct ResearchPage {
    widget: gtk::Box,
    services: Arc<AppServices>,
    /// The currently displayed list (may be filtered).
    current_list: Rc<RefCell<Vec<DownloadedObservation>>>,
    list_box: gtk::ListBox,
    filter_entry: gtk::SearchEntry,
    count_label: gtk::Label,
    /// Running application — needed to activate `app.open-fits-file`.
    application: Rc<RefCell<Option<adw::Application>>>,
    /// Outer stack for the list pane (list ↔ empty state).
    content_stack: gtk::Stack,
    /// Detail pane stack (empty placeholder ↔ detail view).
    detail_stack: gtk::Stack,
    /// Container for the currently rendered detail view.  Cleared and
    /// rebuilt on every selection change.
    detail_container: gtk::Box,
}

impl ResearchPage {
    pub fn new(services: Arc<AppServices>) -> Rc<Self> {
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        widget.set_vexpand(true);
        widget.set_hexpand(true);

        // ----------------------------------------------------------------
        // Toolbar / filter bar
        // ----------------------------------------------------------------
        let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        toolbar.set_margin_start(12);
        toolbar.set_margin_end(12);
        toolbar.set_margin_top(12);
        toolbar.set_margin_bottom(6);

        let filter_entry = gtk::SearchEntry::new();
        filter_entry.set_placeholder_text(Some("Search by collection, target, instrument…"));
        filter_entry.set_hexpand(true);
        toolbar.append(&filter_entry);

        let refresh_btn = gtk::Button::from_icon_name("view-refresh-symbolic");
        refresh_btn.set_tooltip_text(Some("Refresh list"));
        toolbar.append(&refresh_btn);

        widget.append(&toolbar);
        widget.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

        // ----------------------------------------------------------------
        // Master-detail split: list on the left (320px), detail on the right
        // ----------------------------------------------------------------
        let paned = gtk::Paned::new(gtk::Orientation::Horizontal);
        paned.set_vexpand(true);
        paned.set_hexpand(true);
        paned.set_position(320);
        paned.set_shrink_start_child(false);
        paned.set_shrink_end_child(false);
        paned.set_resize_start_child(false);
        paned.set_resize_end_child(true);

        // ── Left pane: list + empty state + status ─────────────────────
        let left_pane = gtk::Box::new(gtk::Orientation::Vertical, 0);
        left_pane.set_size_request(280, -1);

        let content_stack = gtk::Stack::new();
        content_stack.set_vexpand(true);
        content_stack.set_hexpand(true);
        content_stack.set_transition_type(gtk::StackTransitionType::Crossfade);

        // Empty state — copy includes bookmarked-only entries
        let empty_status = adw::StatusPage::new();
        empty_status.set_icon_name(Some("document-open-recent-symbolic"));
        empty_status.set_title("No Saved Observations");
        empty_status.set_description(Some(
            "Search the CADC archive, then save or download observations \
             to see them here.",
        ));

        // CTA button → jumps to the Search page
        let go_to_search_btn = gtk::Button::with_label("Go to Search");
        go_to_search_btn.add_css_class("suggested-action");
        go_to_search_btn.add_css_class("pill");
        go_to_search_btn.set_halign(gtk::Align::Center);
        go_to_search_btn.connect_clicked(|btn| {
            if let Some(root) = btn.root().and_then(|r| r.downcast::<gtk::Window>().ok()) {
                if let Some(app) = root.application() {
                    let ag: &gtk::gio::ActionGroup = app.upcast_ref();
                    ag.activate_action("navigate-search", None);
                }
            }
        });
        empty_status.set_child(Some(&go_to_search_btn));

        content_stack.add_named(&empty_status, Some("empty"));

        // Scrollable list
        let scrolled = gtk::ScrolledWindow::new();
        scrolled.set_vexpand(true);
        scrolled.set_hexpand(true);
        scrolled.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);

        let list_box = gtk::ListBox::new();
        list_box.set_selection_mode(gtk::SelectionMode::Single);
        list_box.add_css_class("navigation-sidebar");
        list_box.set_margin_start(0);
        list_box.set_margin_end(0);
        list_box.set_margin_top(0);
        list_box.set_margin_bottom(0);
        scrolled.set_child(Some(&list_box));
        content_stack.add_named(&scrolled, Some("list"));

        left_pane.append(&content_stack);

        // Count label — thin status bar at the bottom of the left pane
        let count_label = gtk::Label::new(Some("0 observations"));
        count_label.add_css_class("dim-label");
        count_label.add_css_class("caption");
        count_label.set_margin_start(12);
        count_label.set_margin_end(12);
        count_label.set_margin_top(4);
        count_label.set_margin_bottom(6);
        count_label.set_halign(gtk::Align::Start);
        left_pane.append(&count_label);

        paned.set_start_child(Some(&left_pane));

        // ── Right pane: detail view or empty placeholder ───────────────
        let detail_stack = gtk::Stack::new();
        detail_stack.set_vexpand(true);
        detail_stack.set_hexpand(true);
        detail_stack.set_transition_type(gtk::StackTransitionType::Crossfade);
        detail_stack.set_transition_duration(150);

        // Empty placeholder shown when nothing is selected
        let detail_empty = adw::StatusPage::new();
        detail_empty.set_icon_name(Some("document-open-symbolic"));
        detail_empty.set_title("Select an observation");
        detail_empty.set_description(Some(
            "Saved observations from CADC archive searches appear on the left.",
        ));
        detail_stack.add_named(&detail_empty, Some("empty"));

        // Scrollable detail view — `detail_container` is cleared and
        // rebuilt every time the user selects a different observation.
        let detail_scroll = gtk::ScrolledWindow::new();
        detail_scroll.set_vexpand(true);
        detail_scroll.set_hexpand(true);

        let detail_container = gtk::Box::new(gtk::Orientation::Vertical, 12);
        detail_container.set_margin_start(24);
        detail_container.set_margin_end(24);
        detail_container.set_margin_top(24);
        detail_container.set_margin_bottom(24);
        detail_scroll.set_child(Some(&detail_container));
        detail_stack.add_named(&detail_scroll, Some("detail"));

        detail_stack.set_visible_child_name("empty");
        paned.set_end_child(Some(&detail_stack));

        widget.append(&paned);

        // ----------------------------------------------------------------
        // Assemble
        // ----------------------------------------------------------------
        let page = Rc::new(ResearchPage {
            widget,
            services,
            current_list: Rc::new(RefCell::new(Vec::new())),
            list_box,
            filter_entry,
            count_label,
            application: Rc::new(RefCell::new(None)),
            content_stack,
            detail_stack,
            detail_container,
        });

        // Wire signals
        {
            let p = Rc::clone(&page);
            page.filter_entry.connect_search_changed(move |entry| {
                let p = Rc::clone(&p);
                let text = entry.text().to_string();
                glib::spawn_future_local(async move {
                    p.apply_filter_async(&text).await;
                });
            });
        }

        {
            let p = Rc::clone(&page);
            refresh_btn.connect_clicked(move |_| {
                p.reload();
            });
        }

        // Row-selection → populate the detail pane on the right
        {
            let p = Rc::clone(&page);
            page.list_box.connect_row_selected(move |_, row_opt| {
                match row_opt {
                    None => p.clear_detail(),
                    Some(row) => {
                        let idx = row.index() as usize;
                        let list = p.current_list.borrow();
                        if let Some(obs) = list.get(idx).cloned() {
                            drop(list);
                            p.show_detail(&obs);
                        }
                    }
                }
            });
        }

        // Initial load
        page.reload();

        page
    }

    /// Return the root widget to embed in the view stack.
    pub fn widget(&self) -> &gtk::Box {
        &self.widget
    }

    /// Provide the running application so we can activate `app.open-fits-file`.
    pub fn set_application(&self, app: &adw::Application) {
        *self.application.borrow_mut() = Some(app.clone());
    }

    // -----------------------------------------------------------------------
    // Data management
    // -----------------------------------------------------------------------

    /// Reload from disk and refresh the displayed list.  Disk I/O is
    /// offloaded to the tokio blocking pool so this is non-blocking on the
    /// GLib main thread.
    pub fn reload(self: &Rc<Self>) {
        let page = Rc::clone(self);
        glib::spawn_future_local(async move {
            let text = page.filter_entry.text().to_string();
            page.apply_filter_async(&text).await;
        });
    }

    async fn apply_filter_async(self: &Rc<Self>, text: &str) {
        // Offloaded disk read — avoids blocking the main loop on slow disks
        let svc = self.services.clone();
        let full = self
            .services
            .spawn(async move { svc.observation_store.load_async().await })
            .await;

        // Case-insensitive filter in memory
        let filtered: Vec<DownloadedObservation> = if text.is_empty() {
            full
        } else {
            let needle = text.to_lowercase();
            full.into_iter()
                .filter(|o| {
                    o.collection.to_lowercase().contains(&needle)
                        || o.observation_id.to_lowercase().contains(&needle)
                        || o.target_name.to_lowercase().contains(&needle)
                        || o.instrument.to_lowercase().contains(&needle)
                        || o.filter.to_lowercase().contains(&needle)
                })
                .collect()
        };

        *self.current_list.borrow_mut() = filtered.clone();
        self.rebuild_rows(&filtered);
    }

    fn rebuild_rows(&self, observations: &[DownloadedObservation]) {
        // Clear existing rows and reset the detail pane to the empty state.
        while let Some(child) = self.list_box.first_child() {
            self.list_box.remove(&child);
        }
        self.clear_detail();

        if observations.is_empty() {
            self.content_stack.set_visible_child_name("empty");
            self.count_label.set_text("No observations");
            return;
        }

        self.content_stack.set_visible_child_name("list");
        let n = observations.len();
        self.count_label.set_text(&format!(
            "{} observation{}",
            n,
            if n == 1 { "" } else { "s" }
        ));

        for obs in observations {
            let row = self.build_row(obs);
            self.list_box.append(&row);
        }
    }

    /// Reset the right-side detail pane to the empty placeholder.
    fn clear_detail(&self) {
        while let Some(child) = self.detail_container.first_child() {
            self.detail_container.remove(&child);
        }
        self.detail_stack.set_visible_child_name("empty");
    }

    /// Build a compact list row showing only an icon, title, subtitle, and
    /// kind badge.  All actions (Open / Delete / Copy / etc.) live in the
    /// right-side detail pane, matching the Windows master-detail layout.
    fn build_row(&self, obs: &DownloadedObservation) -> adw::ActionRow {
        // Title: prefer target name, fall back to observation ID, then publisher DID
        let title = if !obs.target_name.is_empty() {
            obs.target_name.clone()
        } else if !obs.observation_id.is_empty() {
            obs.observation_id.clone()
        } else {
            obs.publisher_id
                .split('?')
                .nth(1)
                .unwrap_or(&obs.publisher_id)
                .to_string()
        };

        // Subtitle: "Collection — Instrument" (matches Windows 2-line template)
        let mut parts: Vec<&str> = Vec::new();
        if !obs.collection.is_empty() {
            parts.push(&obs.collection);
        }
        if !obs.instrument.is_empty() {
            parts.push(&obs.instrument);
        }
        let subtitle = parts.join(" — ");

        let row = adw::ActionRow::builder()
            .title(&title)
            .subtitle(&subtitle)
            .activatable(true)
            .build();

        // Leading icon — bookmark / FITS / generic
        let icon_name = if obs.is_bookmarked() {
            "bookmark-symbolic"
        } else if is_fits_path(&obs.local_path) {
            "image-x-generic-symbolic"
        } else {
            "document-open-recent-symbolic"
        };
        let lead_icon = gtk::Image::from_icon_name(icon_name);
        lead_icon.set_pixel_size(24);
        row.add_prefix(&lead_icon);

        // Kind badge: "Bookmarked" or "FITS · size"
        let kind_badge = gtk::Label::new(None);
        kind_badge.set_valign(gtk::Align::Center);
        kind_badge.set_margin_end(6);
        if obs.is_bookmarked() {
            kind_badge.set_text("Bookmarked");
            kind_badge.add_css_class("badge-bookmarked");
        } else {
            let size_text = obs.formatted_size();
            if size_text.is_empty() {
                kind_badge.set_text("FITS");
            } else {
                kind_badge.set_text(&size_text);
            }
            kind_badge.add_css_class("badge-fits");
        }
        row.add_suffix(&kind_badge);

        row
    }

    // -----------------------------------------------------------------------
    // Detail pane
    // -----------------------------------------------------------------------

    /// Populate the right-side detail pane for the selected observation.
    /// Clears any previous content, then builds a fresh view containing:
    /// preview image (async-loaded), title + subtitle, contextual action
    /// buttons, observation metadata group, and file info group.
    fn show_detail(self: &Rc<Self>, obs: &DownloadedObservation) {
        // Clear previous detail
        while let Some(child) = self.detail_container.first_child() {
            self.detail_container.remove(&child);
        }
        self.detail_stack.set_visible_child_name("detail");

        // ── Preview image (conditional on having thumbnail/preview URL) ──
        let preview_url = if !obs.thumbnail_url.is_empty() {
            Some(obs.thumbnail_url.clone())
        } else if !obs.preview_url.is_empty() {
            Some(obs.preview_url.clone())
        } else {
            None
        };

        if let Some(url) = preview_url {
            let frame = gtk::Frame::new(None);
            frame.add_css_class("card");
            frame.set_halign(gtk::Align::Start);

            let stack = gtk::Stack::new();
            stack.set_transition_type(gtk::StackTransitionType::Crossfade);
            stack.set_size_request(420, 260);

            let loading = gtk::Spinner::new();
            loading.set_size_request(32, 32);
            loading.set_halign(gtk::Align::Center);
            loading.set_valign(gtk::Align::Center);
            loading.start();
            stack.add_named(&loading, Some("loading"));

            let err_page = adw::StatusPage::new();
            err_page.set_icon_name(Some("image-missing-symbolic"));
            err_page.set_title("Preview unavailable");
            stack.add_named(&err_page, Some("error"));

            stack.set_visible_child_name("loading");
            frame.set_child(Some(&stack));
            self.detail_container.append(&frame);

            // Async load
            let svc = self.services.clone();
            let stack_ref = stack.clone();
            glib::spawn_future_local(async move {
                let svc2 = svc.clone();
                let url_clone = url.clone();
                let bytes_result = svc
                    .spawn(async move {
                        let token = svc2.get_token().await;
                        svc2.datalink
                            .download_image(&url_clone, token.as_deref())
                            .await
                    })
                    .await;
                let bytes = match bytes_result {
                    Ok(b) => b,
                    Err(_) => {
                        stack_ref.set_visible_child_name("error");
                        return;
                    }
                };
                let gbytes = gtk::glib::Bytes::from(&bytes);
                let stream = gtk::gio::MemoryInputStream::from_bytes(&gbytes);
                let pixbuf = match gtk::gdk_pixbuf::Pixbuf::from_stream_future(&stream).await {
                    Ok(p) => p,
                    Err(_) => {
                        stack_ref.set_visible_child_name("error");
                        return;
                    }
                };
                let texture = gtk::gdk::Texture::for_pixbuf(&pixbuf);
                let picture = gtk::Picture::for_paintable(&texture);
                picture.set_content_fit(gtk::ContentFit::Contain);
                picture.set_size_request(420, 260);
                stack_ref.add_named(&picture, Some("image"));
                stack_ref.set_visible_child_name("image");
            });
        }

        // ── Title + subtitle ───────────────────────────────────────────
        let title_text = if !obs.target_name.is_empty() {
            obs.target_name.clone()
        } else if !obs.observation_id.is_empty() {
            obs.observation_id.clone()
        } else {
            "Observation".to_string()
        };
        let title_label = gtk::Label::new(Some(&title_text));
        title_label.add_css_class("title-2");
        title_label.set_halign(gtk::Align::Start);
        title_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        title_label.set_margin_top(4);
        self.detail_container.append(&title_label);

        let mut sub_parts: Vec<String> = Vec::new();
        if !obs.collection.is_empty() {
            sub_parts.push(obs.collection.clone());
        }
        if !obs.observation_id.is_empty() && obs.observation_id != title_text {
            sub_parts.push(obs.observation_id.clone());
        }
        if !sub_parts.is_empty() {
            let subtitle = gtk::Label::new(Some(&sub_parts.join(" — ")));
            subtitle.add_css_class("caption");
            subtitle.add_css_class("dim-label");
            subtitle.set_halign(gtk::Align::Start);
            self.detail_container.append(&subtitle);
        }

        // ── Action bar ─────────────────────────────────────────────────
        let action_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        action_row.set_halign(gtk::Align::Start);
        action_row.set_margin_top(8);
        action_row.set_margin_bottom(8);

        let file_exists = !obs.is_bookmarked()
            && std::path::Path::new(&obs.local_path).exists();

        // Open File (shown when file exists)
        if file_exists {
            let open_btn = make_icon_button(
                "document-open-symbolic",
                "Open",
                "Open the FITS file in the viewer",
                Some("suggested-action"),
            );
            let local_path = obs.local_path.clone();
            let app_ref = Rc::clone(&self.application);
            let svc = self.services.clone();
            open_btn.connect_clicked(move |_| {
                if !std::path::Path::new(&local_path).exists() {
                    svc.toast
                        .toast("File not found — it may have been moved or deleted");
                    return;
                }
                if let Some(app) = app_ref.borrow().as_ref() {
                    let ag: &gtk::gio::ActionGroup = app.upcast_ref();
                    ag.activate_action(
                        "open-fits-file",
                        Some(&glib::Variant::from(local_path.as_str())),
                    );
                }
            });
            action_row.append(&open_btn);

            // Show in File Manager
            let show_btn = make_icon_button(
                "folder-symbolic",
                "Show in Files",
                "Open the containing folder in the file manager",
                None,
            );
            let local_path = obs.local_path.clone();
            let svc = self.services.clone();
            show_btn.connect_clicked(move |_| {
                let dir = std::path::Path::new(&local_path).parent();
                if let Some(d) = dir {
                    let uri = format!("file://{}", d.to_string_lossy());
                    if let Err(e) = gtk::gio::AppInfo::launch_default_for_uri(
                        &uri,
                        gtk::gio::AppLaunchContext::NONE,
                    ) {
                        svc.toast
                            .toast(&format!("Could not open file manager: {}", e));
                    }
                } else {
                    svc.toast.toast("Unable to locate parent directory");
                }
            });
            action_row.append(&show_btn);
        } else if !obs.is_bookmarked() {
            // File expected but missing from disk
            let missing_lbl = gtk::Label::new(Some("File missing from disk"));
            missing_lbl.add_css_class("warning");
            missing_lbl.add_css_class("caption");
            missing_lbl.set_margin_end(8);
            action_row.append(&missing_lbl);
        }

        // Copy Publisher ID
        let copy_btn = make_icon_button(
            "edit-copy-symbolic",
            "Copy ID",
            "Copy the publisher DID to the clipboard",
            None,
        );
        {
            let pub_id = obs.publisher_id.clone();
            let svc = self.services.clone();
            copy_btn.connect_clicked(move |btn| {
                let display = btn.display();
                display.clipboard().set_text(&pub_id);
                svc.toast.toast("Publisher ID copied");
            });
        }
        action_row.append(&copy_btn);

        // Spacer to push Delete to the right
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        action_row.append(&spacer);

        // Delete button (menu when both list/disk options exist)
        if obs.is_bookmarked() {
            // Bookmarked-only: single "Remove" button (no file to delete)
            let del_btn = make_icon_button(
                "user-trash-symbolic",
                "Remove",
                "Remove this bookmark from the library",
                Some("destructive-action"),
            );
            let this = Rc::clone(self);
            let obs_id = obs.id.clone();
            del_btn.connect_clicked(move |_| {
                let this = Rc::clone(&this);
                let obs_id = obs_id.clone();
                glib::spawn_future_local(async move {
                    this.delete_from_list(&obs_id).await;
                });
            });
            action_row.append(&del_btn);
        } else {
            // Downloaded: menu with "Remove from list" / "Delete from disk"
            let del_menu_btn = gtk::MenuButton::new();
            let del_hbox = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            del_hbox.append(&gtk::Image::from_icon_name("user-trash-symbolic"));
            del_hbox.append(&gtk::Label::new(Some("Delete")));
            del_menu_btn.set_child(Some(&del_hbox));
            del_menu_btn.add_css_class("destructive-action");
            del_menu_btn.set_tooltip_text(Some("Delete options"));

            let menu = gtk::gio::Menu::new();
            menu.append(Some("Remove from list"), Some("detail.delete-list"));
            menu.append(Some("Delete from disk"), Some("detail.delete-disk"));
            let popover = gtk::PopoverMenu::from_model(Some(&menu));
            del_menu_btn.set_popover(Some(&popover));

            // Action group for this row
            let ag = gtk::gio::SimpleActionGroup::new();
            {
                let this = Rc::clone(self);
                let obs_id = obs.id.clone();
                let action = gtk::gio::SimpleAction::new("delete-list", None);
                action.connect_activate(move |_, _| {
                    let this = Rc::clone(&this);
                    let obs_id = obs_id.clone();
                    glib::spawn_future_local(async move {
                        this.delete_from_list(&obs_id).await;
                    });
                });
                ag.add_action(&action);
            }
            {
                let this = Rc::clone(self);
                let obs_id = obs.id.clone();
                let local_path = obs.local_path.clone();
                let target_name = obs.target_name.clone();
                let action = gtk::gio::SimpleAction::new("delete-disk", None);
                action.connect_activate(move |_, _| {
                    let this = Rc::clone(&this);
                    let obs_id = obs_id.clone();
                    let local_path = local_path.clone();
                    let target_name = target_name.clone();
                    glib::spawn_future_local(async move {
                        this.delete_from_disk(&obs_id, &target_name, &local_path)
                            .await;
                    });
                });
                ag.add_action(&action);
            }
            del_menu_btn.insert_action_group("detail", Some(&ag));
            action_row.append(&del_menu_btn);
        }

        self.detail_container.append(&action_row);
        self.detail_container
            .append(&gtk::Separator::new(gtk::Orientation::Horizontal));

        // ── Observation Metadata group ─────────────────────────────────
        let metadata_group = adw::PreferencesGroup::new();
        metadata_group.set_title("Observation Metadata");

        let add_row = |group: &adw::PreferencesGroup, label: &str, value: &str| {
            if !value.is_empty() {
                let row = adw::ActionRow::builder()
                    .title(label)
                    .subtitle(value)
                    .subtitle_selectable(true)
                    .build();
                group.add(&row);
            }
        };

        add_row(&metadata_group, "Collection", &obs.collection);
        add_row(&metadata_group, "Observation ID", &obs.observation_id);
        add_row(&metadata_group, "Target Name", &obs.target_name);
        add_row(&metadata_group, "Instrument", &obs.instrument);
        add_row(&metadata_group, "Filter", &obs.filter);
        add_row(&metadata_group, "RA (J2000)", &obs.ra);
        add_row(&metadata_group, "Dec (J2000)", &obs.dec);
        add_row(&metadata_group, "Start Date", &obs.start_date);
        add_row(&metadata_group, "Calibration Level", &obs.cal_level);
        self.detail_container.append(&metadata_group);

        // ── File Info group ────────────────────────────────────────────
        let file_group = adw::PreferencesGroup::new();
        file_group.set_title("File Info");
        file_group.set_margin_top(12);

        if obs.is_bookmarked() {
            let row = adw::ActionRow::builder()
                .title("Status")
                .subtitle("Bookmarked (metadata only — no file downloaded)")
                .build();
            file_group.add(&row);
        } else {
            add_row(&file_group, "Path", &obs.local_path);
            let size_str = obs.formatted_size();
            if !size_str.is_empty() {
                add_row(&file_group, "Size", &size_str);
            }
            let exists_str = if file_exists {
                "Yes"
            } else {
                "Missing — file not found on disk"
            };
            add_row(&file_group, "File exists", exists_str);
        }
        add_row(&file_group, "Saved at", &format_rfc3339(&obs.downloaded_at));
        add_row(&file_group, "Publisher ID", &obs.publisher_id);
        self.detail_container.append(&file_group);
    }

    async fn delete_from_list(self: &Rc<Self>, obs_id: &str) {
        let svc = self.services.clone();
        let id = obs_id.to_string();
        let _ = self
            .services
            .spawn(async move { svc.observation_store.remove_async(&id).await })
            .await;

        let mut list = self.current_list.borrow_mut();
        list.retain(|o| o.id != obs_id);
        let remaining = list.clone();
        drop(list);

        self.rebuild_rows(&remaining);
        self.services.toast.toast("Removed from list");
    }

    async fn delete_from_disk(
        self: &Rc<Self>,
        obs_id: &str,
        target_name: &str,
        local_path: &str,
    ) {
        if !confirm_delete_from_disk(&self.widget, target_name, local_path).await {
            return;
        }
        let lp = local_path.to_string();
        let _ = self
            .services
            .spawn(async move {
                tokio::task::spawn_blocking(move || std::fs::remove_file(&lp)).await
            })
            .await;

        let svc = self.services.clone();
        let id = obs_id.to_string();
        let _ = self
            .services
            .spawn(async move { svc.observation_store.remove_async(&id).await })
            .await;

        let mut list = self.current_list.borrow_mut();
        list.retain(|o| o.id != obs_id);
        let remaining = list.clone();
        drop(list);

        self.rebuild_rows(&remaining);
        self.services.toast.toast("File deleted");
    }
}

/// Build a standard `Icon + Label` button used in the detail pane action bar.
fn make_icon_button(icon: &str, label: &str, tooltip: &str, css: Option<&str>) -> gtk::Button {
    let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    hbox.append(&gtk::Image::from_icon_name(icon));
    hbox.append(&gtk::Label::new(Some(label)));
    let btn = gtk::Button::new();
    btn.set_child(Some(&hbox));
    btn.set_tooltip_text(Some(tooltip));
    if let Some(c) = css {
        btn.add_css_class(c);
    }
    btn
}

/// Format an RFC-3339 timestamp as "YYYY-MM-DD HH:MM" in local time.
fn format_rfc3339(rfc3339: &str) -> String {
    match chrono::DateTime::parse_from_rfc3339(rfc3339) {
        Ok(dt) => dt
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M")
            .to_string(),
        Err(_) => rfc3339.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn is_fits_path(path: &str) -> bool {
    let lower = path.to_lowercase();
    lower.ends_with(".fits") || lower.ends_with(".fit") || lower.ends_with(".fts")
}

/// Show an `AdwMessageDialog` confirming permanent file deletion from disk.
/// Returns `true` iff the user clicked "Delete".
async fn confirm_delete_from_disk(
    widget: &impl IsA<gtk::Widget>,
    target_name: &str,
    local_path: &str,
) -> bool {
    let root = match widget.root().and_then(|r| r.downcast::<gtk::Window>().ok()) {
        Some(w) => w,
        None => return false,
    };

    let filename = std::path::Path::new(local_path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| local_path.to_string());

    let body = format!(
        "This will permanently remove {} from your computer.\n\nThis cannot be undone.",
        if !target_name.is_empty() { target_name } else { &filename }
    );

    let dialog = adw::MessageDialog::new(
        Some(&root),
        Some("Delete file from disk?"),
        Some(&body),
    );
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("delete", "Delete");
    dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");

    let result = Rc::new(std::cell::RefCell::new(false));
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let tx = Rc::new(std::cell::RefCell::new(Some(tx)));

    {
        let result = result.clone();
        let tx = tx.clone();
        dialog.connect_response(None, move |_, response| {
            *result.borrow_mut() = response == "delete";
            if let Some(tx) = tx.borrow_mut().take() {
                let _ = tx.send(());
            }
        });
    }

    dialog.present();
    let _ = rx.await;
    let val = *result.borrow();
    val
}
