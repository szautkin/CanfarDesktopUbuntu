use crate::services::observation_store::DownloadedObservation;
use crate::state::AppServices;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use crate::models::agent_attribution::AgentAttribution;
use crate::models::observation_note::ObservationNote;
use crate::services::observation_note_store::ObservationNoteStore;
use crate::ui::agent_badge::agent_badge;

/// Resolve the optional agent provenance stored on a record into a renderable
/// [`AgentAttribution`].  The stored string is either a JSON-serialised
/// `AgentAttribution` (full provenance, as written by MCP agent flows) or a
/// bare client label (e.g. `"Claude Desktop"`), in which case we synthesise an
/// attribution using the record's `downloaded_at` as the timestamp.  Returns
/// `None` when the record has no attribution (the common, non-agent case).
fn agent_attribution_from(obs: &DownloadedObservation) -> Option<AgentAttribution> {
    let raw = obs.agent_attribution.as_deref()?.trim();
    if raw.is_empty() {
        return None;
    }
    // Prefer a full JSON payload so client/tool/timestamp/fingerprint survive.
    if let Ok(attr) = serde_json::from_str::<AgentAttribution>(raw) {
        return Some(attr);
    }
    // Fall back to treating the value as a plain client label.
    Some(AgentAttribution::new(
        raw.to_string(),
        crate::tr_en!("mcp"),
        obs.downloaded_at.clone(),
    ))
}

/// Build the renderable badge model for a record whose provenance is stored as
/// the compact [`crate::helpers::agent_attribution::AgentAttribution`] stamp
/// (origin label + apply time).  The stamp doesn't record the tool dimension, so
/// it degrades to the generic "mcp" agent label — matching the bare-label
/// fallback in [`agent_attribution_from`].  Used for the notes surface, whose
/// [`ObservationNote`] carries the compact stamp rather than a JSON string.
fn badge_from_stamp(
    stamp: &crate::helpers::agent_attribution::AgentAttribution,
) -> AgentAttribution {
    AgentAttribution::new(
        stamp.origin.clone(),
        crate::tr_en!("mcp"),
        stamp.applied_at.clone(),
    )
}

// ---------------------------------------------------------------------------
// ResearchPage
// ---------------------------------------------------------------------------

/// The Research page — shows all locally saved CADC observations in a
/// How often the page checks whether the observation store changed underneath
/// it. One mutex read per tick; it reloads only when the sequence moved.
const LIBRARY_POLL_MS: u64 = 1500;

/// master-detail layout: the list is on the left, the currently selected
/// observation's full metadata, preview image, and action buttons are on
/// the right.  Matches the Windows CanfarDesktop layout.
pub struct ResearchPage {
    widget: gtk::Box,
    services: Arc<AppServices>,
    /// The currently displayed list (may be filtered).
    current_list: Rc<RefCell<Vec<DownloadedObservation>>>,
    /// Publisher id of the observation whose detail pane is open.
    ///
    /// Tracked by IDENTITY, not row index: a reload re-filters and re-sorts, so
    /// index 3 afterwards is rarely the record the user was reading. Without
    /// this, every reload — including the one fired on each navigation back to
    /// this page — closed the detail pane and sent the user back to the list.
    selected_publisher_id: RefCell<Option<String>>,
    /// True while `rebuild_rows` re-selects, so the row-deselected handler that
    /// fires as the old rows are removed does not clear the tracked selection.
    restoring_selection: RefCell<bool>,
    /// Observation-store sequence this page has already reflected.
    last_library_seq: RefCell<u64>,
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

    // ── Research-notes editor state ────────────────────────────────────
    // Notes are keyed by publisher ID; `note_edit_id` is the id the editor
    // currently holds.  Saving always writes under it, and we flush under the
    // OUTGOING id before loading a new one, so quick selection switches never
    // cross-contaminate notes (mirrors the Windows `_noteEditId` guard).
    note_store: ObservationNoteStore,
    note_edit_id: RefCell<Option<String>>,
    /// Provenance stamp of the note currently in the editor, if it was authored
    /// by an AI agent.  Seeded from the store on selection and re-applied on save
    /// so an autosave/user edit doesn't strip the "created by AI agent" badge.
    note_attribution: RefCell<Option<crate::helpers::agent_attribution::AgentAttribution>>,
    /// Suppresses autosave while the editor is being seeded from the store.
    note_suppress: Cell<bool>,
    /// Pending 700ms debounce timer; `None` when no edit is queued.
    note_debounce: RefCell<Option<glib::SourceId>>,
    /// Current in-editor rating (0–5), read on save.
    note_rating: Cell<u8>,
    /// Live refs to the editor widgets so save/flush can read their values.
    note_buffer: RefCell<Option<gtk::TextBuffer>>,
    note_tags_entry: RefCell<Option<gtk::Entry>>,
    star_buttons: RefCell<Vec<gtk::Button>>,
}

impl ResearchPage {
    pub fn new(services: Arc<AppServices>) -> Rc<Self> {
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        widget.set_vexpand(true);
        widget.set_hexpand(true);

        // ----------------------------------------------------------------
        // Toolbar / filter bar
        // ----------------------------------------------------------------
        let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        toolbar.add_css_class("toolbar");

        let filter_entry = gtk::SearchEntry::new();
        filter_entry.set_placeholder_text(Some(crate::tr_en!(
            "Search by collection, target, instrument…"
        )));
        filter_entry.set_hexpand(true);
        toolbar.append(&filter_entry);

        let export_btn = make_icon_button(
            "document-save-symbolic",
            crate::tr_en!("Export…"),
            crate::tr_en!("Export a research bundle (observations + notes) as a .zip"),
            None,
        );
        toolbar.append(&export_btn);

        let refresh_btn = gtk::Button::from_icon_name("view-refresh-symbolic");
        refresh_btn.set_tooltip_text(Some(crate::tr_en!("Refresh list")));
        toolbar.append(&refresh_btn);

        widget.append(&toolbar);
        widget.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

        // ----------------------------------------------------------------
        // Master-detail split: list on the left (320px), detail on the right
        // ----------------------------------------------------------------
        let paned = gtk::Paned::new(gtk::Orientation::Horizontal);
        paned.set_vexpand(true);
        paned.set_hexpand(true);
        paned.set_position(crate::ui::panel::LIST_WIDTH);
        paned.set_shrink_start_child(false);
        paned.set_shrink_end_child(false);
        paned.set_resize_start_child(false);
        paned.set_resize_end_child(true);

        // ── Left pane: list + empty state + status ─────────────────────
        let left_pane = gtk::Box::new(gtk::Orientation::Vertical, 0);
        // The list is the panel here, and it keeps its width: the detail
        // pane beside it takes the squeeze. At 320 the names it exists to
        // show were truncated — `panel_width_probe` measures the rows at a
        // 429 px median with truncation switched off — while the pane doing
        // the squeezing was showing a "select something" placeholder.
        crate::ui::panel::pin(&left_pane, crate::ui::panel::LIST_WIDTH);

        let content_stack = gtk::Stack::new();
        content_stack.set_vexpand(true);
        content_stack.set_hexpand(true);
        content_stack.set_transition_type(gtk::StackTransitionType::Crossfade);

        // Empty state — copy includes bookmarked-only entries
        let empty_status = adw::StatusPage::new();
        empty_status.set_icon_name(Some("document-open-recent-symbolic"));
        empty_status.set_title(crate::tr_en!("No Saved Observations"));
        empty_status.set_description(Some(crate::tr_en!(
            "Search the CADC archive, then save or download observations \
             to see them here."
        )));

        // CTA button → jumps to the Search page
        let go_to_search_btn = gtk::Button::with_label(crate::tr_en!("Go to Search"));
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
        list_box.add_css_class("boxed-list");
        list_box.set_margin_start(12);
        list_box.set_margin_end(12);
        list_box.set_margin_top(12);
        list_box.set_margin_bottom(12);
        scrolled.set_child(Some(&list_box));
        content_stack.add_named(&scrolled, Some("list"));

        left_pane.append(&content_stack);

        // Count label — thin status bar at the bottom of the left pane
        let count_label = gtk::Label::new(Some(crate::tr_en!("0 observations")));
        count_label.add_css_class("dim-label");
        count_label.add_css_class("caption");
        count_label.set_margin_start(12);
        count_label.set_margin_end(12);
        count_label.set_margin_top(6);
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
        detail_empty.set_title(crate::tr_en!("Select an observation"));
        detail_empty.set_description(Some(crate::tr_en!(
            "Saved observations from CADC archive searches appear on the left."
        )));
        detail_stack.add_named(&detail_empty, Some("empty"));

        // Scrollable detail view — `detail_container` is cleared and
        // rebuilt every time the user selects a different observation.
        let detail_scroll = gtk::ScrolledWindow::new();
        detail_scroll.set_vexpand(true);
        detail_scroll.set_hexpand(true);

        let detail_container = gtk::Box::new(gtk::Orientation::Vertical, 12);
        detail_container.set_margin_start(12);
        detail_container.set_margin_end(12);
        detail_container.set_margin_top(12);
        detail_container.set_margin_bottom(12);
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
            selected_publisher_id: RefCell::new(None),
            restoring_selection: RefCell::new(false),
            // Seeded with what has already happened, so opening the page does
            // not replay an old change as if it were new.
            last_library_seq: RefCell::new(crate::helpers::store_events::current_seq(
                crate::helpers::store_events::Store::Observations,
            )),
            list_box,
            filter_entry,
            count_label,
            application: Rc::new(RefCell::new(None)),
            content_stack,
            detail_stack,
            detail_container,
            note_store: ObservationNoteStore::new(),
            note_edit_id: RefCell::new(None),
            note_attribution: RefCell::new(None),
            note_suppress: Cell::new(false),
            note_debounce: RefCell::new(None),
            note_rating: Cell::new(0),
            note_buffer: RefCell::new(None),
            note_tags_entry: RefCell::new(None),
            star_buttons: RefCell::new(Vec::new()),
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

        {
            let p = Rc::clone(&page);
            export_btn.connect_clicked(move |_| {
                let p = Rc::clone(&p);
                glib::spawn_future_local(async move {
                    p.export_bundle().await;
                });
            });
        }

        // Row-selection → populate the detail pane on the right
        {
            let p = Rc::clone(&page);
            page.list_box
                .connect_row_selected(move |_, row_opt| match row_opt {
                    None => {
                        // Only a deliberate deselection forgets the selection;
                        // a rebuild restores it instead (see `rebuild_rows`).
                        if !*p.restoring_selection.borrow() {
                            *p.selected_publisher_id.borrow_mut() = None;
                        }
                        p.clear_detail();
                    }
                    Some(row) => {
                        let idx = row.index() as usize;
                        let list = p.current_list.borrow();
                        if let Some(obs) = list.get(idx).cloned() {
                            drop(list);
                            *p.selected_publisher_id.borrow_mut() = Some(obs.publisher_id.clone());
                            p.show_detail(&obs);
                        }
                    }
                });
        }
        // Redundant row-activated handler for activatable rows (clicks)
        {
            let p = Rc::clone(&page);
            page.list_box.connect_row_activated(move |_, row| {
                let idx = row.index() as usize;
                let list = p.current_list.borrow();
                if let Some(obs) = list.get(idx).cloned() {
                    drop(list);
                    *p.selected_publisher_id.borrow_mut() = Some(obs.publisher_id.clone());
                    p.show_detail(&obs);
                }
            });
        }

        // Initial load
        page.reload();

        // Follow library changes made elsewhere: an agent applying
        // download_observation, delete_downloaded_observation or a note update
        // writes the store directly, and the page would otherwise keep showing
        // the previous library until the user navigated away and back. Safe to
        // do now that a reload keeps the open observation selected. Weak, so the
        // timer dies with the page.
        {
            let weak = Rc::downgrade(&page);
            glib::timeout_add_local(
                std::time::Duration::from_millis(LIBRARY_POLL_MS),
                move || match weak.upgrade() {
                    Some(page) => {
                        page.follow_library_changes();
                        glib::ControlFlow::Continue
                    }
                    None => glib::ControlFlow::Break,
                },
            );
        }

        page
    }

    /// Reload when the observation store changed underneath the page.
    fn follow_library_changes(self: &Rc<Self>) {
        let seq = crate::helpers::store_events::current_seq(
            crate::helpers::store_events::Store::Observations,
        );
        if seq <= *self.last_library_seq.borrow() {
            return;
        }
        *self.last_library_seq.borrow_mut() = seq;
        self.reload();
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

        // Case-insensitive filter in memory. A record is shown when its METADATA
        // matches OR its note/tag text matches — mirroring the Windows
        // `ResearchViewModel.Refresh`, which unions the metadata match with
        // `_noteStore.SearchPublisherIds(FilterText)`.
        let filtered: Vec<DownloadedObservation> = if text.is_empty() {
            full
        } else {
            let needle = text.to_lowercase();
            let mut result: Vec<DownloadedObservation> = full
                .iter()
                .filter(|o| {
                    o.collection.to_lowercase().contains(&needle)
                        || o.observation_id.to_lowercase().contains(&needle)
                        || o.target_name.to_lowercase().contains(&needle)
                        || o.instrument.to_lowercase().contains(&needle)
                        || o.filter.to_lowercase().contains(&needle)
                })
                .cloned()
                .collect();

            // Union in observations whose stored note text OR any tag matches the
            // query (the notes file is tiny, so the scan is read inline like the
            // note editor's `get`). Skip records already matched by metadata.
            let note_ids: std::collections::HashSet<String> =
                self.note_store.search(text).into_iter().collect();
            if !note_ids.is_empty() {
                let already: std::collections::HashSet<&str> =
                    result.iter().map(|o| o.publisher_id.as_str()).collect();
                let by_note: Vec<DownloadedObservation> = full
                    .iter()
                    .filter(|o| {
                        note_ids.contains(&o.publisher_id)
                            && !already.contains(o.publisher_id.as_str())
                    })
                    .cloned()
                    .collect();
                result.extend(by_note);
            }
            result
        };

        *self.current_list.borrow_mut() = filtered.clone();
        self.rebuild_rows(&filtered);
    }

    /// Rebuild the list, keeping the user on the observation they were reading.
    ///
    /// The selection is restored by publisher id, not row index — a reload
    /// re-filters and re-sorts, so the row that was at index 3 is rarely the
    /// same record. This used to clear the detail pane unconditionally, and
    /// `main_window` reloads on every navigation INTO this page, so returning to
    /// Research always dropped you back at the list.
    fn rebuild_rows(&self, observations: &[DownloadedObservation]) {
        // Removing rows fires row-deselected; flag it so that handler does not
        // read it as the user deliberately closing the detail pane.
        *self.restoring_selection.borrow_mut() = true;
        while let Some(child) = self.list_box.first_child() {
            self.list_box.remove(&child);
        }

        if observations.is_empty() {
            *self.restoring_selection.borrow_mut() = false;
            self.clear_detail();
            self.content_stack.set_visible_child_name("empty");
            self.count_label.set_text(crate::tr_en!("No observations"));
            return;
        }

        self.content_stack.set_visible_child_name("list");
        let n = observations.len();
        self.count_label
            .set_text(&crate::tr_plural!(n, "{} observation", "{} observations"));

        for obs in observations {
            let row = self.build_row(obs);
            self.list_box.append(&row);
        }

        // Re-select the previously open observation if it survived the reload.
        let previously_open = self.selected_publisher_id.borrow().clone();
        match selection_index_after_rebuild(previously_open.as_deref(), observations) {
            Some(index) => {
                if let Some(row) = self.list_box.row_at_index(index as i32) {
                    self.list_box.select_row(Some(&row));
                }
            }
            // Either nothing was open, or it was deleted / filtered out — the
            // detail pane must not keep showing a record that is no longer here.
            None => {
                *self.selected_publisher_id.borrow_mut() = None;
                self.clear_detail();
            }
        }
        *self.restoring_selection.borrow_mut() = false;
    }

    /// Reset the right-side detail pane to the empty placeholder.
    fn clear_detail(&self) {
        // Persist any pending edit before we tear the editor widgets down.
        self.flush_note();
        *self.note_edit_id.borrow_mut() = None;
        *self.note_attribution.borrow_mut() = None;
        *self.note_buffer.borrow_mut() = None;
        *self.note_tags_entry.borrow_mut() = None;
        self.star_buttons.borrow_mut().clear();

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
        crate::ui::fit::fit_label(&kind_badge);
        kind_badge.set_valign(gtk::Align::Center);
        kind_badge.set_margin_end(6);
        if obs.is_bookmarked() {
            kind_badge.set_text(crate::tr_en!("Bookmarked"));
            kind_badge.add_css_class("badge-bookmarked");
        } else {
            let size_text = obs.formatted_size();
            if size_text.is_empty() {
                kind_badge.set_text(crate::tr_en!("FITS"));
            } else {
                kind_badge.set_text(&size_text);
            }
            kind_badge.add_css_class("badge-fits");
        }
        row.add_suffix(&kind_badge);

        // Agent provenance badge — shown only when an AI agent created this
        // record over MCP (matches ResearchPage.xaml's inline AgentBadge).
        if let Some(attr) = agent_attribution_from(obs) {
            row.add_suffix(&agent_badge(&attr));
        }

        row
    }

    // -----------------------------------------------------------------------
    // Detail pane
    // -----------------------------------------------------------------------

    /// Populate the right-side detail pane for the selected observation.
    fn show_detail(self: &Rc<Self>, obs: &DownloadedObservation) {
        // Persist the OUTGOING observation's note, then drop the stale editor
        // widget refs before rebuilding (mirrors the Windows FlushNote on
        // selection change).  Must flush BEFORE clearing the refs it reads.
        self.flush_note();
        *self.note_buffer.borrow_mut() = None;
        *self.note_tags_entry.borrow_mut() = None;
        self.star_buttons.borrow_mut().clear();

        // Clear previous detail
        while let Some(child) = self.detail_container.first_child() {
            self.detail_container.remove(&child);
        }
        self.detail_stack.set_visible_child_name("detail");

        // ── Preview image (fully offline — read from local disk) ────────
        // The Research page does NOT touch the network for previews.  The
        // image is loaded synchronously from `local_preview_path`.  If no
        // local preview exists we skip the preview frame entirely.
        if obs.has_local_preview() {
            let frame = gtk::Frame::new(None);
            frame.add_css_class("card");
            frame.set_halign(gtk::Align::Start);

            // gtk::Picture::for_filename decodes on the main thread but
            // thumbnails are small (< 100KB) so this is imperceptible.
            let picture = gtk::Picture::for_filename(&obs.local_preview_path);
            picture.set_content_fit(gtk::ContentFit::Contain);
            picture.set_size_request(420, 260);
            frame.set_child(Some(&picture));
            self.detail_container.append(&frame);
        } else if !obs.thumbnail_url.is_empty() || !obs.preview_url.is_empty() {
            // Legacy record saved before managed storage was introduced.
            // Show a subtle banner telling the user to re-save for offline access.
            let banner = gtk::Label::new(Some(crate::tr_en!(
                "Legacy record — re-save from the Search page to cache the preview locally."
            )));
            banner.add_css_class("dim-label");
            banner.add_css_class("caption");
            banner.set_wrap(true);
            banner.set_halign(gtk::Align::Start);
            self.detail_container.append(&banner);
        }

        // ── Title + subtitle ───────────────────────────────────────────
        let title_text = if !obs.target_name.is_empty() {
            obs.target_name.clone()
        } else if !obs.observation_id.is_empty() {
            obs.observation_id.clone()
        } else {
            crate::tr_en!("Observation").to_string()
        };
        let title_label = gtk::Label::new(Some(&title_text));
        title_label.add_css_class("title-2");
        title_label.set_halign(gtk::Align::Start);
        title_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        title_label.set_margin_top(6);
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

        // ── Agent provenance badge (only for agent-created records) ─────
        if let Some(attr) = agent_attribution_from(obs) {
            let attr_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            attr_row.set_halign(gtk::Align::Start);
            attr_row.set_margin_top(6);
            let caption = gtk::Label::new(Some(crate::tr_en!("Created by AI agent")));
            caption.add_css_class("caption");
            caption.add_css_class("dim-label");
            attr_row.append(&caption);
            attr_row.append(&agent_badge(&attr));
            self.detail_container.append(&attr_row);
        }

        // ── Action bar ─────────────────────────────────────────────────
        let action_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        action_row.set_halign(gtk::Align::Start);
        action_row.set_margin_top(6);
        action_row.set_margin_bottom(6);

        let file_exists = !obs.is_bookmarked() && std::path::Path::new(&obs.local_path).exists();

        // Open File (shown when file exists)
        if file_exists {
            let open_btn = make_icon_button(
                "document-open-symbolic",
                crate::tr_en!("Open (FITS)"),
                crate::tr_en!("Open the file in the 2D FITS viewer"),
                Some("suggested-action"),
            );
            let local_path = obs.local_path.clone();
            let app_ref = Rc::clone(&self.application);
            let svc = self.services.clone();
            open_btn.connect_clicked(move |_| {
                if !std::path::Path::new(&local_path).exists() {
                    svc.toast.toast(crate::tr_en!(
                        "File not found — it may have been moved or deleted"
                    ));
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

            // Open as Cube — offered for FITS-like files; the cube loader declines
            // non-cubes with a toast (matches the reference's dual-viewer choice).
            if is_cube_openable(&obs.local_path) {
                let cube_btn = make_icon_button(
                    "view-paged-symbolic",
                    crate::tr_en!("Open as Cube"),
                    crate::tr_en!("Open a spectral cube in the 3D Cube Viewer"),
                    None,
                );
                let local_path = obs.local_path.clone();
                let app_ref = Rc::clone(&self.application);
                let svc = self.services.clone();
                cube_btn.connect_clicked(move |_| {
                    if !std::path::Path::new(&local_path).exists() {
                        svc.toast.toast(crate::tr_en!(
                            "File not found — it may have been moved or deleted"
                        ));
                        return;
                    }
                    if let Some(app) = app_ref.borrow().as_ref() {
                        let ag: &gtk::gio::ActionGroup = app.upcast_ref();
                        ag.activate_action(
                            "open-cube-file",
                            Some(&glib::Variant::from(local_path.as_str())),
                        );
                    }
                });
                action_row.append(&cube_btn);

                // Sniff-driven recommendation hint. `fits_sniff::inspect` reads only
                // header metadata; run it off the GLib thread so a slow disk / large
                // header never stalls the UI. When it detects a spectral third axis
                // we reveal the hint so the user prefers the Cube Viewer. (Reference:
                // ResearchViewModel's post-download viewer recommendation.)
                let reco_label = gtk::Label::new(None);
                reco_label.add_css_class("caption");
                reco_label.add_css_class("accent");
                reco_label.set_valign(gtk::Align::Center);
                reco_label.set_visible(false);
                action_row.append(&reco_label);

                let path = obs.local_path.clone();
                let svc = self.services.clone();
                glib::spawn_future_local(async move {
                    let p = std::path::PathBuf::from(path);
                    let shape = svc
                        .spawn(async move {
                            tokio::task::spawn_blocking(move || {
                                crate::helpers::fits_sniff::inspect(&p)
                            })
                            .await
                            .ok()
                        })
                        .await;
                    if let Some(shape) = shape {
                        if shape.recommend_cube() {
                            reco_label.set_text(crate::tr_en!(
                                "Spectral cube detected — Open as Cube recommended"
                            ));
                            reco_label.set_visible(true);
                        }
                    }
                });
            }

            // Show in File Manager
            let show_btn = make_icon_button(
                "folder-symbolic",
                crate::tr_en!("Show in Files"),
                crate::tr_en!("Open the containing folder in the file manager"),
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
                            .toast(crate::tr_fmt!("Could not open file manager: {}", e));
                    }
                } else {
                    svc.toast
                        .toast(crate::tr_en!("Unable to locate parent directory"));
                }
            });
            action_row.append(&show_btn);
        } else if !obs.is_bookmarked() {
            // File expected but missing from disk. Offer a one-click re-download
            // that resolves the DataLink #this URL and streams it back into the
            // managed directory (mirrors ResearchViewModel.DownloadObservationFileAsync).
            let missing_lbl = gtk::Label::new(Some(crate::tr_en!("File missing from disk")));
            missing_lbl.add_css_class("warning");
            missing_lbl.add_css_class("caption");
            missing_lbl.set_margin_end(6);
            action_row.append(&missing_lbl);

            let download_btn = make_icon_button(
                "folder-download-symbolic",
                crate::tr_en!("Download FITS"),
                crate::tr_en!("Re-download the FITS file to the Research library"),
                Some("suggested-action"),
            );
            let this = Rc::clone(self);
            let obs_clone = obs.clone();
            download_btn.connect_clicked(move |btn| {
                // Guard against double-clicks — the streamed download can take a
                // while; disable the button for the duration.
                btn.set_sensitive(false);
                let this = Rc::clone(&this);
                let obs_clone = obs_clone.clone();
                glib::spawn_future_local(async move {
                    this.download_missing_file(&obs_clone).await;
                });
            });
            action_row.append(&download_btn);
        }

        // CAOM2 Observation Detail (metadata — works even without a local file)
        let details_btn = make_icon_button(
            "view-more-symbolic",
            crate::tr_en!("Details"),
            crate::tr_en!("View the full CAOM2 observation metadata"),
            None,
        );
        {
            let pub_id = obs.publisher_id.clone();
            let app_ref = Rc::clone(&self.application);
            details_btn.connect_clicked(move |_| {
                if let Some(app) = app_ref.borrow().as_ref() {
                    let ag: &gtk::gio::ActionGroup = app.upcast_ref();
                    ag.activate_action(
                        "open-observation-detail",
                        Some(&glib::Variant::from(pub_id.as_str())),
                    );
                }
            });
        }
        action_row.append(&details_btn);

        // Copy Publisher ID
        let copy_btn = make_icon_button(
            "edit-copy-symbolic",
            crate::tr_en!("Copy ID"),
            crate::tr_en!("Copy the publisher DID to the clipboard"),
            None,
        );
        {
            let pub_id = obs.publisher_id.clone();
            let svc = self.services.clone();
            copy_btn.connect_clicked(move |btn| {
                let display = btn.display();
                display.clipboard().set_text(&pub_id);
                svc.toast.toast(crate::tr_en!("Publisher ID copied"));
            });
        }
        action_row.append(&copy_btn);

        // Spacer to push Delete to the right
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        action_row.append(&spacer);

        // Delete button (menu when both list/disk options exist)
        if obs.is_bookmarked() {
            // Legacy metadata-only record: simple "Remove" button
            let del_btn = make_icon_button(
                "user-trash-symbolic",
                crate::tr_en!("Remove"),
                crate::tr_en!("Remove this observation from the library"),
                Some("destructive-action"),
            );
            let this = Rc::clone(self);
            let obs_id = obs.id.clone();
            let target_name = obs.target_name.clone();
            let parent = self.widget.clone();
            del_btn.connect_clicked(move |_| {
                let this = Rc::clone(&this);
                let obs_id = obs_id.clone();
                let target_name = target_name.clone();
                let parent = parent.clone();
                glib::spawn_future_local(async move {
                    if !confirm_delete(&parent, &target_name).await {
                        return;
                    }
                    this.delete_observation(&obs_id).await;
                });
            });
            action_row.append(&del_btn);
        } else {
            // Full record: single "Delete" button that removes both the
            // store entry AND the managed directory (preview + FITS file).
            let del_btn = make_icon_button(
                "user-trash-symbolic",
                crate::tr_en!("Delete"),
                crate::tr_en!("Remove from Research and delete the local files"),
                Some("destructive-action"),
            );
            let this = Rc::clone(self);
            let obs_id = obs.id.clone();
            let target_name = obs.target_name.clone();
            let parent = self.widget.clone();
            del_btn.connect_clicked(move |_| {
                let this = Rc::clone(&this);
                let obs_id = obs_id.clone();
                let target_name = target_name.clone();
                let parent = parent.clone();
                glib::spawn_future_local(async move {
                    if !confirm_delete(&parent, &target_name).await {
                        return;
                    }
                    this.delete_observation(&obs_id).await;
                });
            });
            action_row.append(&del_btn);
        }

        self.detail_container.append(&action_row);
        self.detail_container
            .append(&gtk::Separator::new(gtk::Orientation::Horizontal));

        // ── Observation Metadata group ─────────────────────────────────
        let metadata_group = adw::PreferencesGroup::new();
        metadata_group.set_title(crate::tr_en!("Observation Metadata"));

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

        add_row(
            &metadata_group,
            crate::tr_en!("Collection"),
            &obs.collection,
        );
        add_row(
            &metadata_group,
            crate::tr_en!("Observation ID"),
            &obs.observation_id,
        );
        add_row(
            &metadata_group,
            crate::tr_en!("Target Name"),
            &obs.target_name,
        );
        add_row(
            &metadata_group,
            crate::tr_en!("Instrument"),
            &obs.instrument,
        );
        add_row(&metadata_group, crate::tr_en!("Filter"), &obs.filter);
        add_row(&metadata_group, crate::tr_en!("RA (J2000)"), &obs.ra);
        add_row(&metadata_group, crate::tr_en!("Dec (J2000)"), &obs.dec);
        add_row(
            &metadata_group,
            crate::tr_en!("Start Date"),
            &obs.start_date,
        );
        add_row(
            &metadata_group,
            crate::tr_en!("Calibration Level"),
            &obs.cal_level,
        );
        self.detail_container.append(&metadata_group);

        // ── File Info group ────────────────────────────────────────────
        let file_group = adw::PreferencesGroup::new();
        file_group.set_title(crate::tr_en!("File Info"));
        file_group.set_margin_top(12);

        if obs.is_bookmarked() {
            let row = adw::ActionRow::builder()
                .title(crate::tr_en!("Status"))
                .subtitle(crate::tr_en!(
                    "Bookmarked (metadata only — no file downloaded)"
                ))
                .build();
            file_group.add(&row);
        } else {
            add_row(&file_group, crate::tr_en!("Path"), &obs.local_path);
            let size_str = obs.formatted_size();
            if !size_str.is_empty() {
                add_row(&file_group, crate::tr_en!("Size"), &size_str);
            }
            let exists_str = if file_exists {
                crate::tr_en!("Yes")
            } else {
                crate::tr_en!("Missing — file not found on disk")
            };
            add_row(&file_group, crate::tr_en!("File exists"), exists_str);
        }
        add_row(
            &file_group,
            crate::tr_en!("Saved at"),
            &format_rfc3339(&obs.downloaded_at),
        );
        add_row(
            &file_group,
            crate::tr_en!("Publisher ID"),
            &obs.publisher_id,
        );
        self.detail_container.append(&file_group);

        // ── Research Notes editor (rating + note + tags, debounced autosave) ─
        self.build_notes_editor(obs);
    }

    // -----------------------------------------------------------------------
    // Research-notes editor
    // -----------------------------------------------------------------------

    /// Build the rating/note/tags editor into the detail pane and seed it from
    /// the note store for `obs`.  Notes are keyed by publisher ID; without one
    /// there is nothing to persist against, so the editor is skipped.
    ///
    /// Ported from `ResearchPage.xaml.cs` `BuildNotesEditor`.
    fn build_notes_editor(self: &Rc<Self>, obs: &DownloadedObservation) {
        if obs.publisher_id.is_empty() {
            *self.note_edit_id.borrow_mut() = None;
            *self.note_attribution.borrow_mut() = None;
            return;
        }

        // Seed with events suppressed so setting the initial values does not
        // trigger an autosave.
        self.note_suppress.set(true);
        *self.note_edit_id.borrow_mut() = Some(obs.publisher_id.clone());

        let saved = self.note_store.get(&obs.publisher_id);

        // Remember any agent provenance so re-saving the note keeps its badge.
        let note_attr = saved.as_ref().and_then(|n| n.agent_attribution.clone());
        *self.note_attribution.borrow_mut() = note_attr.clone();

        // Section header — with an inline agent badge when the note was authored
        // by an AI agent over MCP (mirrors the observation surfaces above).
        let header_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        header_row.set_halign(gtk::Align::Start);
        header_row.set_margin_top(12);
        header_row.set_margin_bottom(6);
        let header = gtk::Label::new(Some(crate::tr_en!("Research Notes")));
        header.add_css_class("title-4");
        header.set_halign(gtk::Align::Start);
        header_row.append(&header);
        if let Some(stamp) = &note_attr {
            header_row.append(&agent_badge(&badge_from_stamp(stamp)));
        }
        self.detail_container.append(&header_row);

        // ── Rating row: five star buttons + a clear button ──────────────
        let rating_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        rating_row.set_halign(gtk::Align::Start);

        let rating_label = gtk::Label::new(Some(crate::tr_en!("Rating")));
        rating_label.add_css_class("dim-label");
        rating_row.append(&rating_label);

        let stars_box = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        let mut star_btns: Vec<gtk::Button> = Vec::with_capacity(5);
        for i in 0..5u8 {
            let btn = gtk::Button::new();
            btn.add_css_class("flat");
            btn.set_child(Some(&gtk::Image::from_icon_name("non-starred-symbolic")));
            btn.set_tooltip_text(Some(&crate::tr_plural!(i + 1, "{} star", "{} stars")));
            let this = Rc::clone(self);
            btn.connect_clicked(move |_| {
                let clicked = i + 1;
                // Clicking the current top star clears the rating (matches the
                // reference RatingControl's IsClearEnabled behavior).
                let new = if this.note_rating.get() == clicked {
                    0
                } else {
                    clicked
                };
                this.set_rating(new);
                this.schedule_note_save();
            });
            stars_box.append(&btn);
            star_btns.push(btn);
        }
        rating_row.append(&stars_box);
        *self.star_buttons.borrow_mut() = star_btns;

        let clear_btn = gtk::Button::from_icon_name("edit-clear-symbolic");
        clear_btn.add_css_class("flat");
        clear_btn.set_tooltip_text(Some(crate::tr_en!("Clear rating")));
        {
            let this = Rc::clone(self);
            clear_btn.connect_clicked(move |_| {
                this.set_rating(0);
                this.schedule_note_save();
            });
        }
        rating_row.append(&clear_btn);
        self.detail_container.append(&rating_row);

        // ── Multiline note ──────────────────────────────────────────────
        let note_scroll = gtk::ScrolledWindow::new();
        note_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        note_scroll.set_min_content_height(96);
        note_scroll.add_css_class("card");
        let note_view = gtk::TextView::new();
        note_view.set_wrap_mode(gtk::WrapMode::WordChar);
        note_view.set_left_margin(6);
        note_view.set_right_margin(6);
        note_view.set_top_margin(6);
        note_view.set_bottom_margin(6);
        let buffer = note_view.buffer();
        note_scroll.set_child(Some(&note_view));
        self.detail_container.append(&note_scroll);
        {
            let this = Rc::clone(self);
            buffer.connect_changed(move |_| this.schedule_note_save());
        }

        // ── Tags (comma-separated) ──────────────────────────────────────
        let tags_entry = gtk::Entry::new();
        tags_entry.set_placeholder_text(Some(crate::tr_en!("Tags (comma-separated)")));
        {
            let this = Rc::clone(self);
            tags_entry.connect_changed(move |_| this.schedule_note_save());
        }
        self.detail_container.append(&tags_entry);

        // Seed values from the store.
        let (rating, note_text, tags_text) = match saved {
            Some(n) => (n.rating, n.note, n.tags.join(", ")),
            None => (0, String::new(), String::new()),
        };
        buffer.set_text(&note_text);
        tags_entry.set_text(&tags_text);

        *self.note_buffer.borrow_mut() = Some(buffer);
        *self.note_tags_entry.borrow_mut() = Some(tags_entry);
        self.set_rating(rating);

        self.note_suppress.set(false);
    }

    /// Update the in-editor rating and refresh the star icons.
    fn set_rating(&self, rating: u8) {
        let r = rating.min(5);
        self.note_rating.set(r);
        for (i, btn) in self.star_buttons.borrow().iter().enumerate() {
            if let Some(img) = btn.child().and_then(|c| c.downcast::<gtk::Image>().ok()) {
                let icon = if (i as u8) < r {
                    "starred-symbolic"
                } else {
                    "non-starred-symbolic"
                };
                img.set_icon_name(Some(icon));
            }
        }
    }

    /// Restart the 700ms debounce on any edit (mirrors `OnNoteEdited`).
    fn schedule_note_save(self: &Rc<Self>) {
        if self.note_suppress.get() {
            return;
        }
        // Cancel a still-pending timer before arming a fresh one.
        if let Some(src) = self.note_debounce.borrow_mut().take() {
            src.remove();
        }
        let this = Rc::clone(self);
        let src = glib::timeout_add_local_once(std::time::Duration::from_millis(700), move || {
            // Clear our stored handle first so `save_note_now` / `flush_note`
            // never try to remove this already-fired source.
            this.note_debounce.borrow_mut().take();
            this.save_note_now();
        });
        *self.note_debounce.borrow_mut() = Some(src);
    }

    /// Persist the editor's current values immediately under the id it is
    /// editing (mirrors `SaveNoteNow`).
    fn save_note_now(&self) {
        // Stop any pending debounce.
        if let Some(src) = self.note_debounce.borrow_mut().take() {
            src.remove();
        }
        let edit_id = match self.note_edit_id.borrow().clone() {
            Some(id) => id,
            None => return,
        };
        let buffer_ref = self.note_buffer.borrow();
        let tags_ref = self.note_tags_entry.borrow();
        let (buffer, tags_entry) = match (buffer_ref.as_ref(), tags_ref.as_ref()) {
            (Some(b), Some(t)) => (b, t),
            _ => return,
        };

        let note_text = buffer
            .text(&buffer.start_iter(), &buffer.end_iter(), false)
            .to_string();
        let tags: Vec<String> = tags_entry
            .text()
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        let note = ObservationNote {
            publisher_id: edit_id,
            rating: self.note_rating.get(),
            note: note_text.trim().to_string(),
            tags,
            updated: chrono::Utc::now().to_rfc3339(),
            // Preserve any agent provenance seeded from the store; a purely
            // user-authored note carries `None` and shows no badge.
            agent_attribution: self.note_attribution.borrow().clone(),
        };
        // Blocking write of a tiny JSON file; ignore errors (read-only disk
        // must not crash the UI). An empty note removes the entry in `save`.
        let _ = self.note_store.save(note);
    }

    /// Flush any pending edit immediately (called before switching
    /// observations, mirrors `FlushNote`).
    fn flush_note(&self) {
        let pending = self.note_debounce.borrow_mut().take();
        if let Some(src) = pending {
            src.remove();
            self.save_note_now();
        }
    }

    /// Remove an observation from the Research library.  Deletes the
    /// record from `observations.json` AND removes the managed subdirectory
    /// containing the preview image and FITS file.  Offloaded via
    /// `spawn_blocking`.
    async fn delete_observation(self: &Rc<Self>, obs_id: &str) {
        let svc = self.services.clone();
        let id = obs_id.to_string();
        let _ = self
            .services
            .spawn(async move {
                // Remove the managed directory first, then the store record.
                let id2 = id.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    crate::services::delete_managed_dir(&id2);
                })
                .await;
                svc.observation_store.remove_async(&id).await
            })
            .await;

        let mut list = self.current_list.borrow_mut();
        list.retain(|o| o.id != obs_id);
        let remaining = list.clone();
        drop(list);

        self.rebuild_rows(&remaining);
        self.services
            .toast
            .toast(crate::tr_en!("Removed from Research"));
    }

    // -----------------------------------------------------------------------
    // Re-download a record whose local file went missing
    // -----------------------------------------------------------------------

    /// Resolve the observation's DataLink `#this` URL, stream the science file
    /// back into the managed directory, and update `local_path` / `file_size`
    /// on the store record and the in-memory list — then re-render the detail
    /// pane so the Open actions appear.  Reuses `search_page`'s streaming idiom
    /// (`transfer::download_to_file`).  Ported from
    /// `ResearchViewModel.DownloadObservationFileAsync`.
    async fn download_missing_file(self: &Rc<Self>, obs: &DownloadedObservation) {
        use crate::services::managed_dir_for;

        let publisher_id = obs.publisher_id.clone();
        if publisher_id.is_empty() {
            self.services.toast.toast(crate::tr_en!(
                "No publisher ID — cannot download this observation"
            ));
            self.show_detail(obs);
            return;
        }

        // ── Resolve DataLink for the #this science URL (off-thread) ────────
        self.services
            .toast
            .toast(crate::tr_en!("Resolving download link…"));
        let svc = self.services.clone();
        let pid = publisher_id.clone();
        let dl_result = self
            .services
            .spawn(async move {
                let token = svc.get_token().await;
                svc.datalink.resolve(&pid, token.as_deref()).await
            })
            .await;

        // Prefer the #this science file; fall back to the synthesised package URL.
        let (science_url, science_name) = match dl_result {
            Ok(dl) => match dl.files.iter().find(|f| f.is_science_data()).cloned() {
                Some(f) => (f.url.clone(), Some(f.filename())),
                None => (
                    dl.download_url
                        .clone()
                        .unwrap_or_else(|| self.services.datalink.download_url(&publisher_id)),
                    None,
                ),
            },
            Err(_) => (self.services.datalink.download_url(&publisher_id), None),
        };

        // ── Destination: reuse the recorded path if present, else rebuild it
        //    under the managed directory (the dir may have been pruned). ─────
        let dest_path: std::path::PathBuf = if !obs.local_path.is_empty() {
            std::path::PathBuf::from(&obs.local_path)
        } else {
            let filename = science_name
                .clone()
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| format!("{}.fits", obs.id));
            managed_dir_for(&obs.id).join(filename)
        };
        if let Some(parent) = dest_path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                self.services
                    .toast
                    .toast(crate::tr_fmt!("Cannot create storage directory: {}", e));
                self.show_detail(obs);
                return;
            }
        }

        // ── Stream the file to disk (same idiom as the search_page save flow)
        let label = if !obs.target_name.is_empty() {
            obs.target_name.clone()
        } else {
            publisher_id.clone()
        };
        self.services
            .toast
            .toast(crate::tr_fmt!("Downloading {}…", label));

        let svc = self.services.clone();
        let url_clone = science_url.clone();
        let dest = dest_path.clone();
        let toast_handle = self.services.toast.clone();
        let progress_label = label.clone();
        let dl = self
            .services
            .spawn(async move {
                let token = svc.get_token().await;
                crate::services::transfer::download_to_file(
                    &url_clone,
                    token.as_deref(),
                    &dest,
                    &toast_handle,
                    &progress_label,
                )
                .await
            })
            .await;

        let file_size = match dl {
            Ok(n) => n,
            Err(e) => {
                self.services
                    .toast
                    .toast(crate::tr_fmt!("Download failed: {}", e));
                self.show_detail(obs);
                return;
            }
        };

        // ── Update the record in place and persist ─────────────────────────
        let mut updated = obs.clone();
        updated.local_path = dest_path.to_string_lossy().to_string();
        updated.file_size = file_size;

        let svc = self.services.clone();
        let to_save = updated.clone();
        let _ = self
            .services
            .spawn(async move { svc.observation_store.save_async(to_save).await })
            .await;

        // Reflect the change in the currently displayed list so a later
        // selection sees the downloaded file without a full reload.
        {
            let mut list = self.current_list.borrow_mut();
            if let Some(entry) = list.iter_mut().find(|o| o.id == updated.id) {
                *entry = updated.clone();
            }
        }

        self.services
            .toast
            .toast(crate::tr_fmt!("Downloaded {}", label));
        // Re-render the detail pane — the Open / Open as Cube actions now appear.
        self.show_detail(&updated);
    }

    // -----------------------------------------------------------------------
    // Research export bundle
    // -----------------------------------------------------------------------

    /// Build a Claude-friendly research bundle — a proper wrapper with a
    /// top-level `manifest.json` + `README.md` alongside `research/` and
    /// `search/` module subdirectories — after asking what to include, then pop
    /// a save-file picker, write the `.zip`, and (optionally) upload it to
    /// VOSpace.  The store reads + zip write are offloaded so the UI thread never
    /// blocks.  Mirrors the reference `ResearchPage.xaml.cs` `OnExportClick`.
    async fn export_bundle(self: &Rc<Self>) {
        // Persist any in-flight note edit so it is included in the export.
        self.flush_note();

        // ── Export options dialog (include notes / history / upload) ───────
        let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
        let description = gtk::Label::new(Some(crate::tr_en!(
            "Bundle your saved observations, notes, and searches into a single \
             Claude-friendly .zip."
        )));
        description.add_css_class("caption");
        description.set_wrap(true);
        description.set_xalign(0.0);
        content.append(&description);

        let include_notes = gtk::CheckButton::with_label(crate::tr_en!("Include research notes"));
        include_notes.set_active(true);
        let include_history = gtk::CheckButton::with_label(crate::tr_en!("Include search history"));
        include_history.set_active(true);
        // Off by default, as the reference's checkbox is: a library of cubes
        // turns a kilobyte bundle into a hundred gigabytes, and that should be
        // a decision, never a default.
        let include_files =
            gtk::CheckButton::with_label(crate::tr_en!("Include downloaded data files (large)"));
        include_files.set_active(false);
        let upload_vospace = gtk::CheckButton::with_label(crate::tr_en!("Upload to VOSpace"));
        upload_vospace.set_active(false);
        content.append(&include_notes);
        content.append(&include_history);
        content.append(&include_files);
        content.append(&upload_vospace);

        let opt_dialog = adw::MessageDialog::builder()
            .heading(crate::tr_en!("Export Research Bundle"))
            .build();
        if let Some(win) = self.widget.root().and_downcast_ref::<gtk::Window>() {
            opt_dialog.set_transient_for(Some(win));
        }
        opt_dialog.set_extra_child(Some(&content));
        opt_dialog.add_response("cancel", crate::tr_en!("Cancel"));
        opt_dialog.add_response("export", crate::tr_en!("Choose Folder…"));
        opt_dialog.set_response_appearance("export", adw::ResponseAppearance::Suggested);
        opt_dialog.set_default_response(Some("export"));
        opt_dialog.set_close_response("cancel");

        if opt_dialog.choose_future().await != "export" {
            return;
        }

        let options = crate::helpers::research_exporter::BundleOptions {
            include_notes: include_notes.is_active(),
            include_search_history: include_history.is_active(),
            include_files: include_files.is_active(),
        };
        let do_upload = upload_vospace.is_active();

        // ── Gather the data. Observations come off the blocking pool; the note
        //    + search files are tiny so we read them inline. ────────────────
        let svc = self.services.clone();
        let observations = self
            .services
            .spawn(async move { svc.observation_store.load_async().await })
            .await;
        let notes = self.note_store.all();
        let saved = self.services.search_store.load_saved();
        let recent = self.services.search_store.load_recent();

        if observations.is_empty() && notes.is_empty() && saved.is_empty() && recent.is_empty() {
            self.services.toast.toast(crate::tr_en!(
                "Nothing to export yet — save an observation first"
            ));
            return;
        }

        // Stamp one clock so the default filename and the bundle contents agree.
        let now = chrono::Utc::now();

        // ── Save-file picker (defaults to the timestamped bundle name) ─────
        let root = self.widget.root().and_downcast::<gtk::Window>();

        let filters = gtk::gio::ListStore::new::<gtk::FileFilter>();
        let zip_filter = gtk::FileFilter::new();
        zip_filter.set_name(Some(crate::tr_en!("ZIP archive")));
        zip_filter.add_pattern("*.zip");
        filters.append(&zip_filter);

        let default_name = format!(
            "{}.zip",
            crate::helpers::research_exporter::bundle_name(now)
        );
        let dialog = gtk::FileDialog::builder()
            .title(crate::tr_en!("Export Research Bundle"))
            .modal(true)
            .initial_name(&default_name)
            .filters(&filters)
            .build();

        let file = match dialog.save_future(root.as_ref()).await {
            Ok(f) => f,
            Err(_) => return, // user cancelled
        };
        let mut path = match file.path() {
            Some(p) => p,
            None => return,
        };
        // Default a missing/other extension to .zip.
        if path
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_ascii_lowercase())
            != Some("zip".to_string())
        {
            path.set_extension("zip");
        }

        // ── Write on the blocking pool — pure Rust, no GTK ─────────────────
        let app_version = env!("CARGO_PKG_VERSION").to_string();
        let host = crate::helpers::research_exporter::host_name();
        let write_path = path.clone();
        let result = self
            .services
            .spawn(async move {
                tokio::task::spawn_blocking(move || {
                    crate::helpers::research_exporter::write_research_bundle_zip(
                        &write_path,
                        &crate::helpers::research_exporter::BundleRequest {
                            observations: &observations,
                            notes: &notes,
                            saved: &saved,
                            recent: &recent,
                            options,
                            now,
                            app_version: &app_version,
                            host_name: &host,
                        },
                    )
                })
                .await
                .unwrap_or_else(|e| Err(format!("blocking pool error: {e}")))
            })
            .await;

        let summary = match result {
            Ok(s) => s,
            Err(e) => {
                self.services
                    .toast
                    .toast(crate::tr_fmt!("Export failed: {}", e));
                return;
            }
        };

        // Each count is pluralized on its own, because "quer{y|ies}" is a stem
        // change no single template can express in two languages at once. The
        // parenthetical is then one argument, so a translator moves the whole
        // list where their sentence needs it.
        let contents = [
            crate::tr_plural!(
                summary.observation_count,
                "{} observation",
                "{} observations"
            ),
            crate::tr_plural!(summary.note_count, "{} note", "{} notes"),
            crate::tr_plural!(summary.saved_count, "{} query", "{} queries"),
            crate::tr_fmt!("{} recent", summary.recent_count),
        ]
        .join(", ");
        self.services.toast.toast(crate::tr_fmt!(
            "Exported {} ({}) to {}",
            summary.bundle_name,
            contents,
            path.display()
        ));

        // ── Optional VOSpace upload of the finished bundle ─────────────────
        if do_upload {
            self.upload_bundle_to_vospace(&path).await;
        }
    }

    /// Upload a finished bundle to the user's VOSpace, with progress toasts.
    ///
    /// The destination and the transfer live in `research_exporter::upload_bundle`,
    /// shared with the `export_research_bundle` applier — the two used to carry
    /// separate copies of the folder name, the idempotent create and the content
    /// type, so an agent-made bundle could have landed somewhere the user's own
    /// export would not. Signed out, the bundle is still saved locally and a
    /// toast explains the skip.
    async fn upload_bundle_to_vospace(self: &Rc<Self>, path: &std::path::Path) {
        let Some(remote_path) = crate::helpers::research_exporter::remote_bundle_path(path) else {
            return;
        };
        let local_path = path.to_path_buf();

        self.services
            .toast
            .toast(crate::tr_en!("Uploading bundle to VOSpace…"));

        let svc = self.services.clone();
        let result: Result<String, String> = self
            .services
            .spawn(async move {
                let token = svc.get_token().await.ok_or_else(|| {
                    "Sign in to CANFAR to upload the bundle to VOSpace".to_string()
                })?;
                let username = svc.get_username().await.ok_or_else(|| {
                    "Sign in to CANFAR to upload the bundle to VOSpace".to_string()
                })?;
                crate::helpers::research_exporter::upload_bundle(
                    &svc.vospace,
                    &token,
                    &username,
                    &local_path,
                )
                .await
            })
            .await;

        match result {
            Ok(_) => {
                let user = self.services.get_username().await.unwrap_or_default();
                self.services.toast.toast(crate::tr_fmt!(
                    "Uploaded to vos:{}/{}",
                    user,
                    remote_path
                ));
            }
            Err(e) => self
                .services
                .toast
                .toast(crate::tr_fmt!("VOSpace upload failed: {}", e)),
        }
    }
}

/// Which row to select after a rebuild, given what was open before.
///
/// By IDENTITY, never by row index: a reload re-filters and re-sorts, so the row
/// that was at index 3 is rarely the record the user was reading. `None` means
/// nothing was open, or what was open is gone — deleted, or filtered out by the
/// current search — and the detail pane must stop showing it.
fn selection_index_after_rebuild(
    previously_open: Option<&str>,
    observations: &[DownloadedObservation],
) -> Option<usize> {
    let publisher_id = previously_open?;
    observations
        .iter()
        .position(|o| o.publisher_id == publisher_id)
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

/// Whether the file is worth offering an "Open as Cube" action for: any plain
/// FITS file plus tile-compressed `.fits.fz` files (which `is_fits_path` misses
/// because they end in `.fz`).  The Cube Viewer declines non-cubes gracefully,
/// so a broad match here only costs a toast in the worst case.
fn is_cube_openable(path: &str) -> bool {
    is_fits_path(path) || path.to_lowercase().ends_with(".fz")
}

/// Show an `AdwMessageDialog` confirming that the user wants to remove
/// an observation from the Research library. This deletes both the store
/// record and the managed directory (preview + FITS). Returns `true` iff
/// the user clicked "Delete".
async fn confirm_delete(widget: &impl IsA<gtk::Widget>, target_name: &str) -> bool {
    let root = match widget.root().and_then(|r| r.downcast::<gtk::Window>().ok()) {
        Some(w) => w,
        None => return false,
    };

    let body = if target_name.is_empty() {
        crate::tr_en!("This will permanently remove the observation and its local files.\n\nThis cannot be undone.").to_string()
    } else {
        format!(
            "This will permanently remove {} and its local files.\n\nThis cannot be undone.",
            target_name
        )
    };

    let dialog = adw::MessageDialog::new(
        Some(&root),
        Some(crate::tr_en!("Remove from Research?")),
        Some(&body),
    );
    dialog.add_response("cancel", crate::tr_en!("Cancel"));
    dialog.add_response("delete", crate::tr_en!("Delete"));
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

#[cfg(test)]
mod tests {
    use super::*;

    fn obs_with(publisher_id: &str) -> DownloadedObservation {
        DownloadedObservation {
            publisher_id: publisher_id.to_string(),
            ..blank_obs()
        }
    }

    #[test]
    fn the_open_observation_survives_a_reorder() {
        // The bug this replaces: the selection was dropped on every rebuild, and
        // main_window reloads on each navigation INTO this page — so coming back
        // to Research always returned you to the list. Restoring by row index
        // would be no better, because a reload re-filters and re-sorts.
        let before = [
            obs_with("ivo://a"),
            obs_with("ivo://b"),
            obs_with("ivo://c"),
        ];
        assert_eq!(
            selection_index_after_rebuild(Some("ivo://b"), &before),
            Some(1)
        );

        // Same records, different order — the index moves, the record does not.
        let after = [
            obs_with("ivo://c"),
            obs_with("ivo://b"),
            obs_with("ivo://a"),
        ];
        assert_eq!(
            selection_index_after_rebuild(Some("ivo://b"), &after),
            Some(1)
        );
        let after = [obs_with("ivo://b"), obs_with("ivo://a")];
        assert_eq!(
            selection_index_after_rebuild(Some("ivo://b"), &after),
            Some(0)
        );
    }

    #[test]
    fn an_observation_that_is_gone_stops_being_shown() {
        // Deleted, or filtered out by the current search. Either way the detail
        // pane must not keep displaying a record the list no longer contains.
        let list = [obs_with("ivo://a"), obs_with("ivo://c")];
        assert_eq!(selection_index_after_rebuild(Some("ivo://b"), &list), None);
        assert_eq!(selection_index_after_rebuild(Some("ivo://b"), &[]), None);
    }

    #[test]
    fn nothing_open_stays_nothing_open() {
        let list = [obs_with("ivo://a")];
        assert_eq!(selection_index_after_rebuild(None, &list), None);
    }

    fn blank_obs() -> DownloadedObservation {
        DownloadedObservation {
            id: "obs-1".into(),
            publisher_id: "ivo://cadc/CFHT?1".into(),
            collection: String::new(),
            observation_id: String::new(),
            target_name: String::new(),
            instrument: String::new(),
            filter: String::new(),
            ra: String::new(),
            dec: String::new(),
            start_date: String::new(),
            cal_level: String::new(),
            local_path: String::new(),
            file_size: 0,
            downloaded_at: "2024-01-01T00:00:00Z".into(),
            thumbnail_url: String::new(),
            preview_url: String::new(),
            local_preview_path: String::new(),
            agent_attribution: None,
            proposal_id: String::new(),
            proposal_pi: String::new(),
            proposal_title: String::new(),
            data_release: String::new(),
        }
    }

    #[test]
    fn cube_openable_matches_fits_and_fz() {
        assert!(is_cube_openable("/data/cube.fits"));
        assert!(is_cube_openable("/data/CUBE.FITS.FZ"));
        assert!(is_cube_openable("/data/x.fz"));
        assert!(!is_cube_openable("/data/preview.jpg"));
    }

    #[test]
    fn attribution_absent_yields_none() {
        let obs = blank_obs();
        assert!(agent_attribution_from(&obs).is_none());
        let mut blankish = blank_obs();
        blankish.agent_attribution = Some("   ".into());
        assert!(agent_attribution_from(&blankish).is_none());
    }

    #[test]
    fn attribution_from_bare_label() {
        let mut obs = blank_obs();
        obs.agent_attribution = Some("Claude Desktop".into());
        let attr = agent_attribution_from(&obs).expect("some attribution");
        assert_eq!(attr.client, "Claude Desktop");
        // Timestamp falls back to the record's downloaded_at.
        assert_eq!(attr.timestamp, "2024-01-01T00:00:00Z");
        assert_eq!(attr.fingerprint.len(), 6);
    }

    #[test]
    fn attribution_from_full_json_round_trip() {
        let mut obs = blank_obs();
        let original =
            AgentAttribution::new("Claude Code", "save_observation", "2026-01-02T03:04:05Z");
        obs.agent_attribution = Some(serde_json::to_string(&original).unwrap());
        let attr = agent_attribution_from(&obs).expect("some attribution");
        assert_eq!(attr, original);
    }
}
