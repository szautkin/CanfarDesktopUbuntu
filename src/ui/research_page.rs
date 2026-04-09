use crate::services::observation_store::{DownloadedObservation, ObservationStore};
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

// ---------------------------------------------------------------------------
// ResearchPage
// ---------------------------------------------------------------------------

/// The Research page — shows all locally downloaded CADC observations and
/// lets the user open them in the FITS viewer or delete them.
pub struct ResearchPage {
    widget: gtk::Box,
    store: Rc<ObservationStore>,
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
    pub fn new() -> Rc<Self> {
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

        // Empty state
        let empty_status = adw::StatusPage::new();
        empty_status.set_icon_name(Some("document-open-recent-symbolic"));
        empty_status.set_title("No Downloaded Observations");
        empty_status.set_description(Some(
            "Search the CADC archive and download files to see them here.",
        ));
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
            store: Rc::new(ObservationStore::new()),
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
                p.apply_filter(entry.text().as_ref());
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

    /// Reload from disk and refresh the displayed list.
    pub fn reload(&self) {
        let text = self.filter_entry.text();
        self.apply_filter(text.as_ref());
    }

    fn apply_filter(&self, text: &str) {
        let list = self.store.filter(text);
        *self.current_list.borrow_mut() = list.clone();
        self.rebuild_rows(&list);
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
        // Title: ObservationID   Subtitle: Collection | Target | Filter | Date
        let title = if obs.observation_id.is_empty() {
            obs.publisher_id
                .split('?')
                .nth(1)
                .unwrap_or(&obs.publisher_id)
                .to_string()
        } else {
            obs.observation_id.clone()
        };

        let subtitle = build_subtitle(obs);

        let row = adw::ActionRow::builder()
            .title(&title)
            .subtitle(&subtitle)
            .build();

        // Leading icon — FITS vs generic
        let icon_name = if is_fits_path(&obs.local_path) {
            "image-x-generic-symbolic"
        } else {
            "document-open-recent-symbolic"
        };
        let lead_icon = gtk::Image::from_icon_name(icon_name);
        lead_icon.set_pixel_size(24);
        row.add_prefix(&lead_icon);

        // Instrument + size in a compact suffix box
        let meta_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
        meta_box.set_valign(gtk::Align::Center);
        meta_box.set_margin_end(4);

        let size_lbl = gtk::Label::new(Some(&obs.formatted_size()));
        size_lbl.add_css_class("dim-label");
        size_lbl.add_css_class("caption");
        size_lbl.set_halign(gtk::Align::End);
        meta_box.append(&size_lbl);

        if !obs.instrument.is_empty() {
            let inst_lbl = gtk::Label::new(Some(&obs.instrument));
            inst_lbl.add_css_class("dim-label");
            inst_lbl.add_css_class("caption");
            inst_lbl.set_halign(gtk::Align::End);
            meta_box.append(&inst_lbl);
        }
        row.add_suffix(&meta_box);

        // "Open in FITS Viewer" button
        let view_btn = gtk::Button::from_icon_name("image-x-generic-symbolic");
        view_btn.set_tooltip_text(Some("Open in FITS Viewer"));
        view_btn.add_css_class("flat");
        view_btn.set_valign(gtk::Align::Center);

        let local_path = obs.local_path.clone();
        let app_ref = Rc::clone(&self.application);
        view_btn.connect_clicked(move |btn| {
            if !std::path::Path::new(&local_path).exists() {
                show_error_dialog(
                    btn,
                    "File Not Found",
                    &format!("The file no longer exists at:\n{}", local_path),
                );
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

        // "Delete" button
        let del_btn = gtk::Button::from_icon_name("user-trash-symbolic");
        del_btn.set_tooltip_text(Some("Remove from list"));
        del_btn.add_css_class("flat");
        del_btn.add_css_class("destructive-action");
        del_btn.set_valign(gtk::Align::Center);

        let obs_id = obs.id.clone();
        let store = Rc::clone(&self.store);
        let current_list = Rc::clone(&self.current_list);
        let list_box = self.list_box.clone();
        let content_stack = self.content_stack.clone();
        let count_label = self.count_label.clone();
        let row_weak = row.downgrade();

        del_btn.connect_clicked(move |_btn| {
            let obs_id = obs_id.clone();
            let store = Rc::clone(&store);
            let current_list = Rc::clone(&current_list);
            let list_box = list_box.clone();
            let content_stack = content_stack.clone();
            let count_label = count_label.clone();
            let row_weak = row_weak.clone();

            // Remove from store synchronously — it's disk I/O but the file is tiny
            let _ = store.remove(&obs_id);

            // Update in-memory list
            let mut list = current_list.borrow_mut();
            list.retain(|o| o.id != obs_id);
            let n = list.len();
            drop(list);

            // Remove the GTK row
            if let Some(row) = row_weak.upgrade() {
                list_box.remove(&row);
            }

            // Update status
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
        row.add_suffix(&del_btn);

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

/// Show a simple non-blocking error window anchored to `widget`.
fn show_error_dialog(widget: &impl IsA<gtk::Widget>, heading: &str, body: &str) {
    let Some(root) = widget.root() else { return };
    let Some(window) = root.downcast_ref::<gtk::Window>() else {
        return;
    };

    let dialog = adw::Window::builder()
        .title(heading)
        .default_width(360)
        .modal(true)
        .transient_for(window)
        .build();

    let toolbar_view = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    toolbar_view.add_top_bar(&header);

    let content_box = gtk::Box::new(gtk::Orientation::Vertical, 12);
    content_box.set_margin_start(24);
    content_box.set_margin_end(24);
    content_box.set_margin_top(12);
    content_box.set_margin_bottom(24);

    let body_label = gtk::Label::new(Some(body));
    body_label.set_wrap(true);
    body_label.set_halign(gtk::Align::Start);
    content_box.append(&body_label);

    let ok_btn = gtk::Button::with_label("OK");
    ok_btn.add_css_class("suggested-action");
    let dialog_weak = dialog.downgrade();
    ok_btn.connect_clicked(move |_| {
        if let Some(d) = dialog_weak.upgrade() {
            d.close();
        }
    });
    content_box.append(&ok_btn);

    toolbar_view.set_content(Some(&content_box));
    dialog.set_content(Some(&toolbar_view));
    dialog.present();
}
