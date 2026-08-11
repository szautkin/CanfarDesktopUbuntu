//! The Workflows (research-protocols) page.
//!
//! A one-to-one port of CanfarDesktop's `WorkflowsPage` (see
//! `Views/WorkflowsPage.xaml.cs`). Master-detail layout: the left pane lists
//! the built-in templates and the user's local working copies; the right pane
//! renders the selected workflow as a stack of check-off step cards, with an
//! inline editor for local copies.
//!
//! The file itself IS the state (see `helpers::workflow_format`): check-off
//! toggles are only ever written to LOCAL workflows. Toggling a step on a
//! read-only built-in template first duplicates it to a local copy.
//!
//! Layout mirrors `ui::research_page` — a `gtk::Paned` with an imperatively
//! rebuilt detail pane.

use crate::helpers::workflow_format::{self, KNOWN_VIEWS};
use crate::models::workflow::{WorkflowInfo, WorkflowSource, WorkflowStep};
use crate::services::workflow_store::{WorkflowStore, VOSPACE_PREFIX};
use crate::state::AppServices;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

type NavigateCb = Rc<RefCell<Option<Box<dyn Fn(&str)>>>>;

// ---------------------------------------------------------------------------
// WorkflowsPage
// ---------------------------------------------------------------------------

/// The Workflows landing surface: built-in templates + local working copies,
/// rendered as check-off step cards. Matches the Windows master-detail layout.
pub struct WorkflowsPage {
    widget: gtk::Box,
    services: Arc<AppServices>,
    /// Stateless config — the local workflows directory + built-in templates.
    store: WorkflowStore,
    /// Left-pane list. Rows carry their workflow id in the widget name.
    list_box: gtk::ListBox,
    /// Detail pane stack (empty placeholder ↔ detail/editor view).
    detail_stack: gtk::Stack,
    /// Container for the currently rendered detail view. Cleared and rebuilt
    /// imperatively on every selection change or mutation.
    detail_container: gtk::Box,
    /// The id (`builtin:<slug>` / `local:<slug>`) currently shown, if any.
    selected_id: RefCell<Option<String>>,
    /// True while the inline editor is showing for a local workflow.
    editing: RefCell<bool>,
    /// Guards against re-entrant selection while programmatically reselecting.
    suppress: RefCell<bool>,
    /// Invoked with a `View:` key when a step's "Go to …" button is clicked.
    on_navigate: NavigateCb,
    /// The `vos:<user>/workflows/` listing, refreshed on demand. Empty until the
    /// user signs in and the first refresh completes.
    vospace_entries: RefCell<Vec<crate::services::workflow_store::VoSpaceWorkflowEntry>>,
    /// Bodies fetched from VOSpace, keyed by store id.
    ///
    /// `render_detail` is synchronous, but a VOSpace workflow needs a network
    /// round trip — so selecting one kicks off a fetch that fills this and
    /// re-renders. Caching also means re-selecting a workflow the user already
    /// looked at is instant.
    vospace_cache: RefCell<std::collections::HashMap<String, WorkflowInfo>>,
}

impl WorkflowsPage {
    pub fn new(services: Arc<AppServices>) -> Rc<Self> {
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        widget.set_vexpand(true);
        widget.set_hexpand(true);

        // ----------------------------------------------------------------
        // Toolbar: New + Import
        // ----------------------------------------------------------------
        let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        toolbar.add_css_class("toolbar");

        let new_btn = make_icon_button(
            "document-new-symbolic",
            crate::tr_en!("New"),
            crate::tr_en!("Create a new workflow from a starter template"),
            Some("suggested-action"),
        );
        toolbar.append(&new_btn);

        let import_btn = make_icon_button(
            "document-open-symbolic",
            crate::tr_en!("Import…"),
            crate::tr_en!("Import a .workflow.md / .md file as a local copy"),
            None,
        );
        toolbar.append(&import_btn);

        let toolbar_spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        toolbar_spacer.set_hexpand(true);
        toolbar.append(&toolbar_spacer);

        widget.append(&toolbar);
        widget.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

        // ----------------------------------------------------------------
        // Master-detail split
        // ----------------------------------------------------------------
        let paned = gtk::Paned::new(gtk::Orientation::Horizontal);
        paned.set_vexpand(true);
        paned.set_hexpand(true);
        paned.set_position(320);
        paned.set_shrink_start_child(false);
        paned.set_shrink_end_child(false);
        paned.set_resize_start_child(false);
        paned.set_resize_end_child(true);

        // ── Left pane: sectioned list ──────────────────────────────────
        let left_pane = gtk::Box::new(gtk::Orientation::Vertical, 0);
        left_pane.set_size_request(280, -1);

        let scrolled = gtk::ScrolledWindow::new();
        scrolled.set_vexpand(true);
        scrolled.set_hexpand(true);
        scrolled.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);

        let list_box = gtk::ListBox::new();
        list_box.set_selection_mode(gtk::SelectionMode::Single);
        list_box.add_css_class("navigation-sidebar");
        scrolled.set_child(Some(&list_box));
        left_pane.append(&scrolled);

        paned.set_start_child(Some(&left_pane));

        // ── Right pane: detail / editor ────────────────────────────────
        let detail_stack = gtk::Stack::new();
        detail_stack.set_vexpand(true);
        detail_stack.set_hexpand(true);
        detail_stack.set_transition_type(gtk::StackTransitionType::Crossfade);
        detail_stack.set_transition_duration(150);

        let detail_empty = adw::StatusPage::new();
        detail_empty.set_icon_name(Some("view-list-symbolic"));
        detail_empty.set_title(crate::tr_en!("Select a workflow"));
        detail_empty.set_description(Some(crate::tr_en!(
            "Pick a built-in template or one of your local copies to walk \
             through its steps. Use “New” to start one from scratch."
        )));
        detail_stack.add_named(&detail_empty, Some("empty"));

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
        let page = Rc::new(WorkflowsPage {
            widget,
            services,
            store: WorkflowStore::new(),
            list_box,
            detail_stack,
            detail_container,
            selected_id: RefCell::new(None),
            editing: RefCell::new(false),
            suppress: RefCell::new(false),
            on_navigate: Rc::new(RefCell::new(None)),
            vospace_entries: RefCell::new(Vec::new()),
            vospace_cache: RefCell::new(std::collections::HashMap::new()),
        });

        // Toolbar wiring
        {
            let page = Rc::clone(&page);
            new_btn.connect_clicked(move |_| {
                page.create_new();
            });
        }
        {
            let page = Rc::clone(&page);
            import_btn.connect_clicked(move |_| {
                let page = Rc::clone(&page);
                glib::spawn_future_local(async move {
                    page.import_dialog().await;
                });
            });
        }

        // Row selection → render the selected workflow.
        {
            let page = Rc::clone(&page);
            let list_box = page.list_box.clone();
            list_box.connect_row_selected(move |_, row_opt| {
                if *page.suppress.borrow() {
                    return;
                }
                if let Some(row) = row_opt {
                    let id = row.widget_name().to_string();
                    if id.is_empty() {
                        return;
                    }
                    *page.selected_id.borrow_mut() = Some(id);
                    *page.editing.borrow_mut() = false;
                    page.render_detail();
                }
            });
        }
        // Redundant activation handler for click-through on activatable rows.
        {
            let page = Rc::clone(&page);
            let list_box = page.list_box.clone();
            list_box.connect_row_activated(move |_, row| {
                if *page.suppress.borrow() {
                    return;
                }
                if !row.is_selectable() {
                    return;
                }
                let id = row.widget_name().to_string();
                if id.is_empty() {
                    return;
                }
                *page.selected_id.borrow_mut() = Some(id);
                *page.editing.borrow_mut() = false;
                page.render_detail();
            });
        }

        // Initial population.
        page.rebuild_lists();

        // Load the shared VOSpace tier in the background: the built-in and local
        // sections must render immediately rather than waiting on the network.
        {
            let p = page.clone();
            glib::spawn_future_local(async move { p.refresh_vospace().await });
        }
        page.render_detail();

        page
    }

    /// Root widget to embed in the view stack.
    pub fn widget(&self) -> &gtk::Box {
        &self.widget
    }

    /// Register the navigation callback used by step `View:` deep-links.
    /// The argument is one of [`KNOWN_VIEWS`].
    pub fn set_on_navigate(&self, cb: impl Fn(&str) + 'static) {
        *self.on_navigate.borrow_mut() = Some(Box::new(cb));
    }

    // -----------------------------------------------------------------------
    // List rebuild
    // -----------------------------------------------------------------------

    /// Clear and repopulate the left-pane list from the store, restoring the
    /// current selection without triggering a redundant detail render.
    fn rebuild_lists(self: &Rc<Self>) {
        *self.suppress.borrow_mut() = true;

        while let Some(child) = self.list_box.first_child() {
            self.list_box.remove(&child);
        }

        // Built-in section
        self.list_box
            .append(&section_header(crate::tr_en!("Built-in")));
        for info in self.store.list_built_in() {
            self.list_box.append(&workflow_row(&info));
        }

        // Local section
        self.list_box
            .append(&section_header(crate::tr_en!("Local")));
        let locals = self.store.list_local();
        if locals.is_empty() {
            self.list_box
                .append(&hint_row(crate::tr_en!("No local workflows yet")));
        } else {
            for info in &locals {
                self.list_box.append(&workflow_row(info));
            }
        }

        // VOSpace section — shared protocols, read-only. Only shown once a
        // refresh has run: an empty section before the user signs in would read
        // as "you have published nothing" rather than "not loaded".
        let entries = self.vospace_entries.borrow();
        if !entries.is_empty() {
            self.list_box
                .append(&section_header(crate::tr_en!("VOSpace")));
            for entry in entries.iter() {
                self.list_box.append(&vospace_row(entry));
            }
        }
        drop(entries);

        // Restore selection (id → row via widget name).
        let sel = self.selected_id.borrow().clone();
        if let Some(id) = sel {
            let mut child = self.list_box.first_child();
            while let Some(w) = child {
                if let Some(row) = w.downcast_ref::<gtk::ListBoxRow>() {
                    if row.is_selectable() && row.widget_name().as_str() == id.as_str() {
                        self.list_box.select_row(Some(row));
                        break;
                    }
                }
                child = w.next_sibling();
            }
        }

        *self.suppress.borrow_mut() = false;
    }

    /// Refresh the sidebar (progress counts may have changed) and re-render
    /// the detail pane for the current selection.
    fn reload_and_render(self: &Rc<Self>) {
        self.rebuild_lists();
        self.render_detail();
    }

    // -----------------------------------------------------------------------
    // Detail render
    // -----------------------------------------------------------------------

    /// Rebuild the right-side pane from `selected_id` + `editing`.
    /// Refresh the VOSpace listing.
    ///
    /// Silent when signed out — the section simply stays hidden, since a shared
    /// folder the user cannot reach is not an error worth interrupting them for.
    async fn refresh_vospace(self: &Rc<Self>) {
        let Some(token) = self.services.get_token().await else {
            return;
        };
        let Some(username) = self.services.get_username().await else {
            return;
        };
        let store = WorkflowStore::new();
        let entries = store
            .list_vospace(&self.services.vospace, &token, &username)
            .await;
        *self.vospace_entries.borrow_mut() = entries;
        self.rebuild_lists();
    }

    /// Fetch one VOSpace workflow's body, then re-render the detail pane.
    ///
    /// The selection is re-checked on completion: the user may have clicked
    /// something else while the download was in flight, and overwriting their
    /// new selection with a stale one is worse than showing nothing.
    fn fetch_vospace_detail(self: &Rc<Self>, path: String) {
        let page = self.clone();
        glib::spawn_future_local(async move {
            let (Some(token), Some(username)) = (
                page.services.get_token().await,
                page.services.get_username().await,
            ) else {
                return;
            };
            let store = WorkflowStore::new();
            let fetched = store
                .fetch_vospace(&page.services.vospace, &token, &username, &path)
                .await;
            match fetched {
                Ok(info) => {
                    let id = info.id.clone();
                    page.vospace_cache.borrow_mut().insert(id.clone(), info);
                    if page.selected_id.borrow().as_deref() == Some(id.as_str()) {
                        page.render_detail();
                    }
                }
                Err(e) => {
                    page.services
                        .toast
                        .toast(crate::tr_fmt!("Could not load workflow: {}", e));
                }
            }
        });
    }

    /// Publish the selected local workflow to `vos:<user>/workflows/`.
    ///
    /// Offers to reset progress first, checked by default: publishing shares a
    /// protocol for others to follow, not a record of one run.
    async fn publish_selected(self: &Rc<Self>, info: WorkflowInfo) {
        let (Some(token), Some(username)) = (
            self.services.get_token().await,
            self.services.get_username().await,
        ) else {
            self.services
                .toast
                .toast(crate::tr_en!("Sign in to CADC to publish a workflow"));
            return;
        };

        let root = self.widget.root().and_downcast::<gtk::Window>();
        let dialog = adw::MessageDialog::new(
            root.as_ref(),
            Some(crate::tr_en!("Publish to VOSpace?")),
            None,
        );
        let reset = gtk::CheckButton::with_label(crate::tr_en!("Reset step progress"));
        reset.set_active(true);
        let body = gtk::Box::new(gtk::Orientation::Vertical, 8);
        let target = gtk::Label::new(Some(&crate::tr_fmt!(
            "Uploads to vos:{}/workflows/",
            username
        )));
        target.set_wrap(true);
        target.set_xalign(0.0);
        body.append(&target);
        body.append(&reset);
        dialog.set_extra_child(Some(&body));
        dialog.add_response("cancel", crate::tr_en!("Cancel"));
        dialog.add_response("publish", crate::tr_en!("Publish"));
        dialog.set_response_appearance("publish", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("publish"));

        if dialog.choose_future().await != "publish" {
            return;
        }

        let store = WorkflowStore::new();
        match store
            .publish_to_vospace(
                &self.services.vospace,
                &token,
                &username,
                &info,
                reset.is_active(),
            )
            .await
        {
            Ok(remote) => {
                self.services
                    .toast
                    .toast(crate::tr_fmt!("Published to vos:{}", remote));
                // The new file must appear in the sidebar without a manual
                // refresh, or the user cannot tell the publish worked.
                self.refresh_vospace().await;
            }
            Err(e) => {
                self.services
                    .toast
                    .toast(crate::tr_fmt!("Publish failed: {}", e));
            }
        }
    }

    fn render_detail(self: &Rc<Self>) {
        while let Some(child) = self.detail_container.first_child() {
            self.detail_container.remove(&child);
        }

        let id_opt = self.selected_id.borrow().clone();
        let id = match id_opt {
            Some(i) => i,
            None => {
                self.detail_stack.set_visible_child_name("empty");
                return;
            }
        };
        // A VOSpace workflow lives behind a network fetch, so it resolves from
        // the cache; a miss schedules the fetch and shows a placeholder rather
        // than falling through to "vanished".
        let resolved = if let Some(path) = id.strip_prefix(VOSPACE_PREFIX) {
            match self.vospace_cache.borrow().get(&id).cloned() {
                Some(info) => Some(info),
                None => {
                    self.fetch_vospace_detail(path.to_string());
                    self.detail_container
                        .append(&hint_label(crate::tr_en!("Loading from VOSpace…")));
                    self.detail_stack.set_visible_child_name("detail");
                    return;
                }
            }
        } else {
            self.store.get(&id)
        };
        let info = match resolved {
            Some(i) => i,
            None => {
                // The workflow vanished (e.g. deleted) — reset to empty.
                *self.selected_id.borrow_mut() = None;
                self.detail_stack.set_visible_child_name("empty");
                return;
            }
        };

        self.detail_stack.set_visible_child_name("detail");

        if *self.editing.borrow() && info.source == WorkflowSource::Local {
            self.build_editor(&info);
        } else {
            self.build_detail_view(&info);
        }
    }

    /// Read-only (per-step-toggle) rendering of a workflow.
    fn build_detail_view(self: &Rc<Self>, info: &WorkflowInfo) {
        let container = &self.detail_container;
        let local = info.source == WorkflowSource::Local;

        // ── Title ──────────────────────────────────────────────────────
        let title = gtk::Label::new(Some(&info.doc.title));
        title.add_css_class("title-2");
        title.set_halign(gtk::Align::Start);
        title.set_xalign(0.0);
        title.set_wrap(true);
        container.append(&title);

        // ── Description ────────────────────────────────────────────────
        if !info.doc.description.is_empty() {
            let desc = gtk::Label::new(Some(&info.doc.description));
            desc.add_css_class("dim-label");
            desc.set_halign(gtk::Align::Start);
            desc.set_xalign(0.0);
            desc.set_wrap(true);
            container.append(&desc);
        }

        // ── Metadata line: source badge · Time · tags ──────────────────
        let meta = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        meta.set_halign(gtk::Align::Start);

        let source_text = match info.source {
            WorkflowSource::BuiltIn => crate::tr_en!("Built-in"),
            WorkflowSource::Local => crate::tr_en!("Local"),
            WorkflowSource::VoSpace => crate::tr_en!("VOSpace"),
        };
        meta.append(&chip(
            source_text,
            if local {
                "badge-fits"
            } else {
                "badge-bookmarked"
            },
        ));

        if let Some(t) = info.doc.metadata_get("Time") {
            if !t.is_empty() {
                let time_lbl = gtk::Label::new(Some(&format!("Time: {}", t)));
                time_lbl.add_css_class("caption");
                time_lbl.add_css_class("dim-label");
                time_lbl.set_valign(gtk::Align::Center);
                meta.append(&time_lbl);
            }
        }
        for tag in info.doc.tags() {
            meta.append(&chip(&tag, "badge-bookmarked"));
        }
        container.append(&meta);

        // ── Progress (local only) ──────────────────────────────────────
        let n = info.doc.steps.len();
        let done = info.doc.done_count();
        if local && n > 0 {
            let prog_lbl = gtk::Label::new(Some(&format!("{}/{} done", done, n)));
            prog_lbl.add_css_class("caption");
            prog_lbl.add_css_class("dim-label");
            prog_lbl.set_halign(gtk::Align::Start);
            container.append(&prog_lbl);

            let bar = gtk::ProgressBar::new();
            bar.set_fraction(done as f64 / n as f64);
            container.append(&bar);
        }

        // ── Action bar ─────────────────────────────────────────────────
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        actions.set_halign(gtk::Align::Start);

        // Duplicate to Local (always available)
        let dup_btn = make_icon_button(
            "edit-copy-symbolic",
            crate::tr_en!("Duplicate to Local"),
            crate::tr_en!("Create an editable local copy of this workflow"),
            None,
        );
        {
            let page = Rc::clone(self);
            let title = info.doc.title.clone();
            let raw = info.raw_text.clone();
            dup_btn.connect_clicked(move |_| {
                match page.store.save_new(&format!("{} (copy)", title), &raw) {
                    Ok(new_info) => {
                        *page.selected_id.borrow_mut() = Some(new_info.id.clone());
                        *page.editing.borrow_mut() = false;
                        page.services
                            .toast
                            .toast(crate::tr_en!("Duplicated to a local copy"));
                        page.reload_and_render();
                    }
                    Err(e) => page
                        .services
                        .toast
                        .toast(format!("Could not duplicate: {}", e)),
                }
            });
        }
        actions.append(&dup_btn);

        if local {
            // Publish to VOSpace — local only: a built-in is already shared, and a
            // VOSpace workflow is where a publish would go.
            let publish_btn = make_icon_button(
                "send-to-symbolic",
                crate::tr_en!("Publish"),
                crate::tr_en!("Share this workflow via your VOSpace"),
                None,
            );
            {
                let page = Rc::clone(self);
                let published = info.clone();
                publish_btn.connect_clicked(move |_| {
                    let page = Rc::clone(&page);
                    let published = published.clone();
                    glib::spawn_future_local(async move {
                        page.publish_selected(published).await;
                    });
                });
            }
            actions.append(&publish_btn);

            // Edit toggle
            let edit_btn = make_icon_button(
                "document-edit-symbolic",
                crate::tr_en!("Edit"),
                crate::tr_en!("Edit the raw workflow markdown"),
                None,
            );
            {
                let page = Rc::clone(self);
                edit_btn.connect_clicked(move |_| {
                    *page.editing.borrow_mut() = true;
                    page.render_detail();
                });
            }
            actions.append(&edit_btn);

            // Spacer → push Delete right
            let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            spacer.set_hexpand(true);
            actions.append(&spacer);

            // Delete (local only)
            let del_btn = make_icon_button(
                "user-trash-symbolic",
                crate::tr_en!("Delete"),
                crate::tr_en!("Delete this local workflow"),
                Some("destructive-action"),
            );
            {
                let page = Rc::clone(self);
                let id = info.id.clone();
                let title = info.doc.title.clone();
                del_btn.connect_clicked(move |_| {
                    let page = Rc::clone(&page);
                    let id = id.clone();
                    let title = title.clone();
                    glib::spawn_future_local(async move {
                        if !confirm_delete(&page.widget, &title).await {
                            return;
                        }
                        if let Err(e) = page.store.delete(&id) {
                            page.services.toast.toast(format!("Delete failed: {}", e));
                            return;
                        }
                        *page.selected_id.borrow_mut() = None;
                        *page.editing.borrow_mut() = false;
                        page.services.toast.toast(crate::tr_en!("Workflow deleted"));
                        page.reload_and_render();
                    });
                });
            }
            actions.append(&del_btn);
        }
        container.append(&actions);
        container.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

        // ── Steps ──────────────────────────────────────────────────────
        if info.doc.steps.is_empty() {
            let empty = gtk::Label::new(Some(crate::tr_en!(
                "This workflow has no steps yet. Use “Edit” (local copy) to add \
                 lines like `- [ ] **Step title** — what to do`."
            )));
            empty.add_css_class("dim-label");
            empty.set_halign(gtk::Align::Start);
            empty.set_xalign(0.0);
            empty.set_wrap(true);
            container.append(&empty);
        } else {
            for step in &info.doc.steps {
                let card = self.build_step_card(&info.id, step);
                container.append(&card);
            }
        }

        // ── Parse warnings (non-fatal) ─────────────────────────────────
        if !info.doc.warnings.is_empty() {
            let warn = gtk::Label::new(Some(&info.doc.warnings.join("\n")));
            warn.add_css_class("caption");
            warn.add_css_class("warning");
            warn.set_halign(gtk::Align::Start);
            warn.set_xalign(0.0);
            warn.set_wrap(true);
            container.append(&warn);
        }
    }

    /// Build a single check-off step card.
    fn build_step_card(self: &Rc<Self>, workflow_id: &str, step: &WorkflowStep) -> gtk::Widget {
        let frame = gtk::Frame::new(None);
        frame.add_css_class("card");

        let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        hbox.set_margin_start(12);
        hbox.set_margin_end(12);
        hbox.set_margin_top(12);
        hbox.set_margin_bottom(12);

        // Checkbox — set state BEFORE connecting so the initial state is silent.
        let check = gtk::CheckButton::new();
        check.set_active(step.done);
        check.set_valign(gtk::Align::Start);
        check.set_tooltip_text(Some(crate::tr_en!("Toggle done")));
        hbox.append(&check);

        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 6);
        vbox.set_hexpand(true);

        // Title (strikethrough when done)
        let title = gtk::Label::new(None);
        let esc = glib::markup_escape_text(&step.title);
        if step.done {
            title.set_markup(&format!("<b><s>{}</s></b>", esc));
        } else {
            title.set_markup(&format!("<b>{}</b>", esc));
        }
        title.set_halign(gtk::Align::Start);
        title.set_xalign(0.0);
        title.set_wrap(true);
        vbox.append(&title);

        // Body
        if !step.body.is_empty() {
            let body = gtk::Label::new(Some(&step.body));
            body.set_halign(gtk::Align::Start);
            body.set_xalign(0.0);
            body.set_wrap(true);
            if step.done {
                body.add_css_class("dim-label");
            }
            vbox.append(&body);
        }

        // Tool chips (small pill labels)
        if !step.tools.is_empty() {
            let tools_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            tools_box.set_halign(gtk::Align::Start);
            for tool in &step.tools {
                tools_box.append(&chip(tool, "badge-fits"));
            }
            vbox.append(&tools_box);
        }

        // "Go to <view>" deep-link (only for known views)
        if let Some(view) = &step.view {
            if !view.is_empty() && KNOWN_VIEWS.contains(&view.as_str()) {
                let go_btn = gtk::Button::with_label(&format!("Go to {}", view));
                go_btn.set_halign(gtk::Align::Start);
                go_btn.add_css_class("flat");
                {
                    let page = Rc::clone(self);
                    let key = view.clone();
                    go_btn.connect_clicked(move |_| {
                        if let Some(cb) = page.on_navigate.borrow().as_ref() {
                            cb(key.as_str());
                        }
                    });
                }
                vbox.append(&go_btn);
            }
        }

        // Note (dim italic)
        if let Some(note) = &step.note {
            if !note.is_empty() {
                let note_lbl = gtk::Label::new(None);
                note_lbl.set_markup(&format!("<i>{}</i>", glib::markup_escape_text(note)));
                note_lbl.add_css_class("caption");
                note_lbl.add_css_class("dim-label");
                note_lbl.set_halign(gtk::Align::Start);
                note_lbl.set_xalign(0.0);
                note_lbl.set_wrap(true);
                vbox.append(&note_lbl);
            }
        }

        hbox.append(&vbox);
        frame.set_child(Some(&hbox));

        // Wire the toggle AFTER building — defer the rebuild off the signal so
        // we never destroy the checkbox mid-emission.
        {
            let page = Rc::clone(self);
            let id = workflow_id.to_string();
            let index = step.index;
            check.connect_toggled(move |btn| {
                let want = btn.is_active();
                let page = Rc::clone(&page);
                let id = id.clone();
                glib::spawn_future_local(async move {
                    page.toggle_step(&id, index, want);
                });
            });
        }

        frame.upcast()
    }

    /// Toggle a step's done state. Local workflows are written in place;
    /// toggling a built-in first duplicates it to a local copy.
    fn toggle_step(self: &Rc<Self>, id: &str, index: usize, done: bool) {
        let info = match self.store.get(id) {
            Some(i) => i,
            None => return,
        };
        match info.source {
            WorkflowSource::Local => {
                if let Err(e) = self.store.set_step_done(id, index, done) {
                    self.services
                        .toast
                        .toast(format!("Could not update step: {}", e));
                }
                self.reload_and_render();
            }
            WorkflowSource::BuiltIn => match self.store.save_new(&info.doc.title, &info.raw_text) {
                Ok(local) => {
                    self.services
                        .toast
                        .toast(crate::tr_en!("Saved a local copy to edit"));
                    let _ = self.store.set_step_done(&local.id, index, done);
                    *self.selected_id.borrow_mut() = Some(local.id.clone());
                    *self.editing.borrow_mut() = false;
                    self.reload_and_render();
                }
                Err(e) => self
                    .services
                    .toast
                    .toast(format!("Could not create local copy: {}", e)),
            },
            WorkflowSource::VoSpace => { /* read-only source — ignore */ }
        }
    }

    // -----------------------------------------------------------------------
    // Editor (local only)
    // -----------------------------------------------------------------------

    /// Inline raw-markdown editor for a local workflow, with a live parsed
    /// preview count and Save / Cancel actions.
    fn build_editor(self: &Rc<Self>, info: &WorkflowInfo) {
        let container = &self.detail_container;
        let id = info.id.clone();

        let header = gtk::Label::new(Some(crate::tr_en!("Edit workflow")));
        header.add_css_class("title-3");
        header.set_halign(gtk::Align::Start);
        container.append(&header);

        // Live preview count.
        let preview = gtk::Label::new(None);
        preview.add_css_class("caption");
        preview.add_css_class("dim-label");
        preview.set_halign(gtk::Align::Start);
        preview.set_xalign(0.0);
        preview.set_wrap(true);
        container.append(&preview);

        // Editor text view.
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_vexpand(true);
        scroll.set_hexpand(true);
        scroll.add_css_class("card");
        scroll.set_min_content_height(280);

        let text_view = gtk::TextView::new();
        text_view.set_monospace(true);
        text_view.set_wrap_mode(gtk::WrapMode::WordChar);
        text_view.set_left_margin(12);
        text_view.set_right_margin(12);
        text_view.set_top_margin(8);
        text_view.set_bottom_margin(8);
        text_view.buffer().set_text(&info.raw_text);
        scroll.set_child(Some(&text_view));
        container.append(&scroll);

        // Seed the preview and keep it in sync with edits.
        set_preview_text(&preview, &info.raw_text);
        {
            let preview = preview.clone();
            text_view.buffer().connect_changed(move |buf| {
                let text = buf.text(&buf.start_iter(), &buf.end_iter(), false);
                set_preview_text(&preview, text.as_str());
            });
        }

        // Save / Cancel row.
        let btns = gtk::Box::new(gtk::Orientation::Horizontal, 6);

        let cancel_btn = gtk::Button::with_label(crate::tr_en!("Cancel"));
        {
            let page = Rc::clone(self);
            cancel_btn.connect_clicked(move |_| {
                *page.editing.borrow_mut() = false;
                page.render_detail();
            });
        }
        btns.append(&cancel_btn);

        let btn_spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        btn_spacer.set_hexpand(true);
        btns.append(&btn_spacer);

        let save_btn = gtk::Button::with_label(crate::tr_en!("Save"));
        save_btn.add_css_class("suggested-action");
        {
            let page = Rc::clone(self);
            let id = id.clone();
            let text_view = text_view.clone();
            save_btn.connect_clicked(move |_| {
                let buf = text_view.buffer();
                let text = buf
                    .text(&buf.start_iter(), &buf.end_iter(), false)
                    .to_string();
                if let Err(e) = page.store.update_text(&id, &text) {
                    page.services.toast.toast(format!("Save failed: {}", e));
                    return;
                }
                *page.editing.borrow_mut() = false;
                page.services.toast.toast(crate::tr_en!("Workflow saved"));
                page.reload_and_render();
            });
        }
        btns.append(&save_btn);

        container.append(&btns);
    }

    // -----------------------------------------------------------------------
    // Toolbar actions
    // -----------------------------------------------------------------------

    /// "New" — instantiate the starter skeleton, persist it, select it, and
    /// drop straight into the editor so the user can rename/flesh it out.
    fn create_new(self: &Rc<Self>) {
        let text = workflow_format::skeleton("New Workflow");
        match self.store.save_new("New Workflow", &text) {
            Ok(info) => {
                *self.selected_id.borrow_mut() = Some(info.id.clone());
                *self.editing.borrow_mut() = true;
                self.reload_and_render();
            }
            Err(e) => self
                .services
                .toast
                .toast(format!("Could not create workflow: {}", e)),
        }
    }

    /// "Import…" — pick a markdown file, save it as a new local workflow.
    async fn import_dialog(self: &Rc<Self>) {
        let root = self.widget.root().and_downcast::<gtk::Window>();

        let filter = gtk::FileFilter::new();
        filter.add_pattern("*.md");
        filter.add_pattern("*.markdown");
        filter.set_name(Some(crate::tr_en!("Workflow / Markdown files")));

        let filters = gtk::gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);

        let dialog = gtk::FileDialog::builder()
            .title(crate::tr_en!("Import Workflow"))
            .filters(&filters)
            .build();

        if let Ok(file) = dialog.open_future(root.as_ref()).await {
            if let Some(path) = file.path() {
                match std::fs::read_to_string(&path) {
                    Ok(text) => {
                        let doc = workflow_format::parse(&text);
                        match self.store.save_new(&doc.title, &text) {
                            Ok(info) => {
                                *self.selected_id.borrow_mut() = Some(info.id.clone());
                                *self.editing.borrow_mut() = false;
                                self.services
                                    .toast
                                    .toast(crate::tr_en!("Imported workflow"));
                                self.reload_and_render();
                            }
                            Err(e) => self.services.toast.toast(format!("Import failed: {}", e)),
                        }
                    }
                    Err(e) => self
                        .services
                        .toast
                        .toast(format!("Could not read file: {}", e)),
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Free helpers
// ---------------------------------------------------------------------------

/// Non-selectable bold section header row for the sidebar.
fn section_header(text: &str) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.set_selectable(false);
    row.set_activatable(false);

    let label = gtk::Label::new(Some(text));
    label.add_css_class("caption-heading");
    label.add_css_class("dim-label");
    label.set_halign(gtk::Align::Start);
    label.set_margin_start(12);
    label.set_margin_end(12);
    label.set_margin_top(12);
    label.set_margin_bottom(6);
    row.set_child(Some(&label));
    row
}

/// Non-selectable dim hint row (e.g. "No local workflows yet").
fn hint_row(text: &str) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.set_selectable(false);
    row.set_activatable(false);

    let label = gtk::Label::new(Some(text));
    label.add_css_class("dim-label");
    label.add_css_class("caption");
    label.set_halign(gtk::Align::Start);
    label.set_margin_start(12);
    label.set_margin_end(12);
    label.set_margin_top(6);
    label.set_margin_bottom(6);
    row.set_child(Some(&label));
    row
}

/// A selectable workflow row: title, "{done}/{n} done" caption, and tag chips.
/// The workflow id is stored as the row's widget name for later lookup.
/// A sidebar row for a VOSpace workflow, which is listed by filename only —
/// its title lives inside the file, and fetching every one to build a list
/// would make opening the page wait on the network.
fn vospace_row(entry: &crate::services::workflow_store::VoSpaceWorkflowEntry) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.set_selectable(true);
    row.set_activatable(true);
    row.set_widget_name(&entry.id);

    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 2);
    vbox.set_margin_start(12);
    vbox.set_margin_end(12);
    vbox.set_margin_top(6);
    vbox.set_margin_bottom(6);

    let title = gtk::Label::new(Some(
        entry
            .name
            .strip_suffix(crate::helpers::workflow_format::FILE_EXTENSION)
            .unwrap_or(&entry.name),
    ));
    title.add_css_class("heading");
    title.set_halign(gtk::Align::Start);
    title.set_xalign(0.0);
    title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    vbox.append(&title);

    vbox.append(&chip(crate::tr_en!("VOSpace"), "accent"));
    row.set_child(Some(&vbox));
    row
}

/// A dim, centred one-line message for a transient detail-pane state.
fn hint_label(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.add_css_class("dim-label");
    label.set_margin_top(24);
    label
}

fn workflow_row(info: &WorkflowInfo) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.set_selectable(true);
    row.set_activatable(true);
    row.set_widget_name(&info.id);

    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 2);
    vbox.set_margin_start(12);
    vbox.set_margin_end(12);
    vbox.set_margin_top(6);
    vbox.set_margin_bottom(6);

    let title = gtk::Label::new(Some(&info.doc.title));
    title.add_css_class("heading");
    title.set_halign(gtk::Align::Start);
    title.set_xalign(0.0);
    title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    vbox.append(&title);

    let meta = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    meta.set_halign(gtk::Align::Start);

    let n = info.doc.steps.len();
    let done = info.doc.done_count();
    let caption = gtk::Label::new(Some(&format!("{}/{} done", done, n)));
    caption.add_css_class("caption");
    caption.add_css_class("dim-label");
    caption.set_valign(gtk::Align::Center);
    meta.append(&caption);

    for tag in info.doc.tags().iter().take(3) {
        meta.append(&chip(tag, "badge-bookmarked"));
    }
    vbox.append(&meta);

    row.set_child(Some(&vbox));
    row
}

/// A small pill label with the given badge CSS class.
fn chip(text: &str, css: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.add_css_class(css);
    label.add_css_class("caption");
    label.set_valign(gtk::Align::Center);
    label
}

/// Set the editor's live preview label from a parse of `text`.
fn set_preview_text(label: &gtk::Label, text: &str) {
    let doc = workflow_format::parse(text);
    let n = doc.steps.len();
    let mut out = crate::tr_fmt!(
        "Preview: “{}” — {} step(s), {} done",
        doc.title,
        n,
        doc.done_count(),
    );

    // Validate as the user types. `validate` existed but nothing called it, so a
    // typo'd `View:` or `Tool:` only surfaced later as a dead deep-link or a
    // tool-name hint that resolves to nothing.
    let problems =
        workflow_format::validate(&doc, workflow_format::KNOWN_VIEWS, &known_tool_names());
    if problems.is_empty() {
        label.remove_css_class("error");
    } else {
        label.add_css_class("error");
        // Cap the list: a malformed paste can produce a problem per line, and a
        // preview label that grows without bound pushes the editor off screen.
        const MAX_SHOWN: usize = 5;
        for p in problems.iter().take(MAX_SHOWN) {
            out.push('\n');
            out.push_str(p);
        }
        if problems.len() > MAX_SHOWN {
            out.push('\n');
            out.push_str(&crate::tr_fmt!(
                "…and {} more problem(s)",
                problems.len() - MAX_SHOWN
            ));
        }
    }
    label.set_text(&out);
}

/// Every tool name an agent could actually call, for validating a step's
/// `Tool:` hints.
///
/// Taken from the live router rather than a hand-maintained list: that set is
/// already pinned against the reference's manifest, so a workflow can never
/// name a tool this build does not have, and the list cannot drift.
fn known_tool_names() -> Vec<String> {
    crate::mcp::tools::router::McpToolRouter::canonical_descriptors()
        .into_iter()
        .map(|d| d.name)
        .collect()
}

/// Build a standard `Icon + Label` button (mirrors `research_page`).
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

/// Confirm deletion of a local workflow via an `AdwMessageDialog`.
/// Returns `true` iff the user chose "Delete".
async fn confirm_delete(widget: &impl IsA<gtk::Widget>, title: &str) -> bool {
    let root = match widget.root().and_then(|r| r.downcast::<gtk::Window>().ok()) {
        Some(w) => w,
        None => return false,
    };

    let body = if title.is_empty() {
        crate::tr_en!("This will permanently delete this local workflow.\n\nThis cannot be undone.")
            .to_string()
    } else {
        format!(
            "This will permanently delete “{}”.\n\nThis cannot be undone.",
            title
        )
    };

    let dialog = adw::MessageDialog::new(
        Some(&root),
        Some(crate::tr_en!("Delete workflow?")),
        Some(&body),
    );
    dialog.add_response("cancel", crate::tr_en!("Cancel"));
    dialog.add_response("delete", crate::tr_en!("Delete"));
    dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");

    let result = Rc::new(RefCell::new(false));
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let tx = Rc::new(RefCell::new(Some(tx)));
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
