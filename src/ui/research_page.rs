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

/// The Research page — shows all locally downloaded CADC observations and
/// lets the user open them in the FITS viewer or delete them.
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
    /// Stack that switches between the list and empty-state views.
    content_stack: gtk::Stack,
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
        // Column header
        // ----------------------------------------------------------------
        let col_header = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        col_header.set_margin_start(18);
        col_header.set_margin_end(18);
        col_header.set_margin_top(4);
        col_header.set_margin_bottom(2);
        col_header.add_css_class("dim-label");
        col_header.add_css_class("caption");

        let lbl_obs = gtk::Label::new(Some("Observation / Collection"));
        lbl_obs.set_hexpand(true);
        lbl_obs.set_halign(gtk::Align::Start);
        col_header.append(&lbl_obs);

        let lbl_inst = gtk::Label::new(Some("Instrument"));
        lbl_inst.set_size_request(120, -1);
        lbl_inst.set_halign(gtk::Align::Start);
        col_header.append(&lbl_inst);

        let lbl_size = gtk::Label::new(Some("Size"));
        lbl_size.set_size_request(80, -1);
        lbl_size.set_halign(gtk::Align::End);
        col_header.append(&lbl_size);

        // Reserve space for the two action buttons (open + delete)
        let lbl_actions = gtk::Label::new(None);
        lbl_actions.set_size_request(88, -1);
        col_header.append(&lbl_actions);

        widget.append(&col_header);
        widget.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

        // ----------------------------------------------------------------
        // Content stack — list ↔ empty state
        // ----------------------------------------------------------------
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

        let list_box = gtk::ListBox::new();
        list_box.set_selection_mode(gtk::SelectionMode::None);
        list_box.add_css_class("boxed-list");
        list_box.set_margin_start(12);
        list_box.set_margin_end(12);
        list_box.set_margin_top(6);
        list_box.set_margin_bottom(12);
        scrolled.set_child(Some(&list_box));
        content_stack.add_named(&scrolled, Some("list"));

        widget.append(&content_stack);

        // ----------------------------------------------------------------
        // Status bar
        // ----------------------------------------------------------------
        let count_label = gtk::Label::new(Some("0 observations"));
        count_label.add_css_class("dim-label");
        count_label.add_css_class("caption");
        count_label.set_margin_start(12);
        count_label.set_margin_bottom(6);
        count_label.set_halign(gtk::Align::Start);
        widget.append(&count_label);

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
        // Clear existing rows
        while let Some(child) = self.list_box.first_child() {
            self.list_box.remove(&child);
        }

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

        let subtitle = build_subtitle(obs);

        let row = adw::ActionRow::builder()
            .title(&title)
            .subtitle(&subtitle)
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

        // Kind badge: "Bookmarked" (bookmarked) or "FITS {size}" (downloaded)
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
                kind_badge.set_text(&format!("FITS · {}", size_text));
            }
            kind_badge.add_css_class("badge-fits");
        }
        row.add_suffix(&kind_badge);

        // "Open in FITS Viewer" button — only sensitive when file exists on disk
        let view_btn = gtk::Button::from_icon_name("image-x-generic-symbolic");
        view_btn.set_tooltip_text(Some("Open in FITS Viewer"));
        view_btn.add_css_class("flat");
        view_btn.set_valign(gtk::Align::Center);
        view_btn.set_sensitive(
            !obs.is_bookmarked() && std::path::Path::new(&obs.local_path).exists(),
        );

        let local_path = obs.local_path.clone();
        let app_ref = Rc::clone(&self.application);
        let services_for_view = self.services.clone();
        view_btn.connect_clicked(move |_btn| {
            if !std::path::Path::new(&local_path).exists() {
                services_for_view
                    .toast
                    .toast("File not found — it may have been moved or deleted");
                return;
            }
            if let Some(app) = app_ref.borrow().as_ref() {
                let action_group: &gtk::gio::ActionGroup = app.upcast_ref();
                action_group.activate_action(
                    "open-fits-file",
                    Some(&glib::Variant::from(local_path.as_str())),
                );
            }
        });
        row.add_suffix(&view_btn);

        // Context menu button — replaces the single Delete button with a
        // richer set of actions.
        let more_btn = gtk::MenuButton::new();
        more_btn.set_icon_name("view-more-symbolic");
        more_btn.add_css_class("flat");
        more_btn.set_valign(gtk::Align::Center);
        more_btn.set_tooltip_text(Some("More actions"));

        let menu = gtk::gio::Menu::new();
        menu.append(Some("Delete from list"), Some("row.delete-list"));
        if !obs.is_bookmarked() {
            menu.append(Some("Delete from disk"), Some("row.delete-disk"));
        }
        menu.append(Some("Copy Publisher ID"), Some("row.copy-id"));
        if !obs.is_bookmarked() {
            menu.append(Some("Show in File Manager"), Some("row.show-in-fm"));
        }

        let popover = gtk::PopoverMenu::from_model(Some(&menu));
        more_btn.set_popover(Some(&popover));

        // Per-row action group wiring
        let action_group = gtk::gio::SimpleActionGroup::new();

        // row.delete-list — remove from store only
        {
            let obs_id = obs.id.clone();
            let services = self.services.clone();
            let current_list = Rc::clone(&self.current_list);
            let list_box = self.list_box.clone();
            let content_stack = self.content_stack.clone();
            let count_label = self.count_label.clone();
            let row_weak = row.downgrade();
            let action = gtk::gio::SimpleAction::new("delete-list", None);
            action.connect_activate(move |_, _| {
                let obs_id = obs_id.clone();
                let services = services.clone();
                let current_list = Rc::clone(&current_list);
                let list_box = list_box.clone();
                let content_stack = content_stack.clone();
                let count_label = count_label.clone();
                let row_weak = row_weak.clone();
                glib::spawn_future_local(async move {
                    let svc = services.clone();
                    let id = obs_id.clone();
                    let _ = services
                        .spawn(async move {
                            svc.observation_store.remove_async(&id).await
                        })
                        .await;

                    let mut list = current_list.borrow_mut();
                    list.retain(|o| o.id != obs_id);
                    let n = list.len();
                    drop(list);

                    if let Some(row) = row_weak.upgrade() {
                        list_box.remove(&row);
                    }
                    if n == 0 {
                        content_stack.set_visible_child_name("empty");
                        count_label.set_text("No observations");
                    } else {
                        count_label.set_text(&format!(
                            "{} observation{}",
                            n,
                            if n == 1 { "" } else { "s" }
                        ));
                    }
                });
            });
            action_group.add_action(&action);
        }

        // row.delete-disk — remove from store AND delete file on disk (with confirmation)
        {
            let obs_id = obs.id.clone();
            let local_path = obs.local_path.clone();
            let target_name = obs.target_name.clone();
            let services = self.services.clone();
            let current_list = Rc::clone(&self.current_list);
            let list_box = self.list_box.clone();
            let content_stack = self.content_stack.clone();
            let count_label = self.count_label.clone();
            let row_weak = row.downgrade();
            let popover_ref = popover.clone();
            let action = gtk::gio::SimpleAction::new("delete-disk", None);
            action.connect_activate(move |_, _| {
                let obs_id = obs_id.clone();
                let local_path = local_path.clone();
                let target_name = target_name.clone();
                let services = services.clone();
                let current_list = Rc::clone(&current_list);
                let list_box = list_box.clone();
                let content_stack = content_stack.clone();
                let count_label = count_label.clone();
                let row_weak = row_weak.clone();
                let popover_ref = popover_ref.clone();
                glib::spawn_future_local(async move {
                    let parent_widget = match row_weak.upgrade() {
                        Some(r) => r,
                        None => return,
                    };
                    if !confirm_delete_from_disk(&parent_widget, &target_name, &local_path)
                        .await
                    {
                        return;
                    }
                    popover_ref.popdown();

                    // Delete the file on disk (blocking, but offloaded)
                    let lp = local_path.clone();
                    let _ = services
                        .spawn(async move {
                            tokio::task::spawn_blocking(move || {
                                std::fs::remove_file(&lp)
                            })
                            .await
                        })
                        .await;

                    // Remove the store record
                    let svc = services.clone();
                    let id = obs_id.clone();
                    let _ = services
                        .spawn(async move {
                            svc.observation_store.remove_async(&id).await
                        })
                        .await;

                    // Update UI
                    let mut list = current_list.borrow_mut();
                    list.retain(|o| o.id != obs_id);
                    let n = list.len();
                    drop(list);
                    if let Some(row) = row_weak.upgrade() {
                        list_box.remove(&row);
                    }
                    if n == 0 {
                        content_stack.set_visible_child_name("empty");
                        count_label.set_text("No observations");
                    } else {
                        count_label.set_text(&format!(
                            "{} observation{}",
                            n,
                            if n == 1 { "" } else { "s" }
                        ));
                    }
                    services.toast.toast("File deleted");
                });
            });
            action_group.add_action(&action);
        }

        // row.copy-id — copy publisher ID to clipboard
        {
            let pub_id = obs.publisher_id.clone();
            let services = self.services.clone();
            let popover_ref = popover.clone();
            let action = gtk::gio::SimpleAction::new("copy-id", None);
            action.connect_activate(move |_, _| {
                let display = gtk::gdk::Display::default();
                if let Some(d) = display {
                    d.clipboard().set_text(&pub_id);
                    services.toast.toast("Publisher ID copied");
                }
                popover_ref.popdown();
            });
            action_group.add_action(&action);
        }

        // row.show-in-fm — launch default file manager on the containing directory
        {
            let local_path = obs.local_path.clone();
            let services = self.services.clone();
            let popover_ref = popover.clone();
            let action = gtk::gio::SimpleAction::new("show-in-fm", None);
            action.connect_activate(move |_, _| {
                let path = std::path::Path::new(&local_path);
                let dir = match path.parent() {
                    Some(d) => d,
                    None => {
                        services.toast.toast("Unable to locate parent directory");
                        return;
                    }
                };
                let uri = format!("file://{}", dir.to_string_lossy());
                if let Err(e) = gtk::gio::AppInfo::launch_default_for_uri(
                    &uri,
                    gtk::gio::AppLaunchContext::NONE,
                ) {
                    services
                        .toast
                        .toast(&format!("Could not open file manager: {}", e));
                }
                popover_ref.popdown();
            });
            action_group.add_action(&action);
        }

        row.insert_action_group("row", Some(&action_group));
        row.add_suffix(&more_btn);

        row
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn is_fits_path(path: &str) -> bool {
    let lower = path.to_lowercase();
    lower.ends_with(".fits") || lower.ends_with(".fit") || lower.ends_with(".fts")
}

fn build_subtitle(obs: &DownloadedObservation) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if !obs.collection.is_empty() {
        parts.push(&obs.collection);
    }
    if !obs.target_name.is_empty() {
        parts.push(&obs.target_name);
    }
    if !obs.filter.is_empty() {
        parts.push(&obs.filter);
    }
    if !obs.cal_level.is_empty() {
        parts.push(&obs.cal_level);
    }
    if !obs.start_date.is_empty() {
        parts.push(&obs.start_date);
    }
    parts.join("  ·  ")
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
