//! Find-by-package image discovery dialog.
//!
//! Port of `Views/Dialogs/ImageDiscoveryDialog.xaml(.cs)` +
//! `ViewModels/ImageDiscovery/ImageDiscoveryViewModel.cs` (with the per-row
//! `ImageRowViewModel` and inline `ManifestDetailViewModel` behaviour folded in).
//!
//! A large modal [`adw::Window`] split by a [`gtk::Paned`]:
//!   * LEFT — a package-search entry, the active-filter chips + Clear-all, and the
//!     faceted filter list (a checkbox per facet value, greyed when selecting it
//!     would collapse the results to zero). Facets come from [`facet_engine`].
//!   * RIGHT — an image-search entry and the project-grouped image list. Each row
//!     shows its cached discovery state (never / discovered + package count /
//!     failed), a Discover/Rediscover button that runs the coordinator probe and
//!     refreshes the row + facets, an inline expander with the manifest broken
//!     into ecosystem sections (or the failure detail), and a primary "Use this
//!     image" button that fires `on_pick` and closes.
//!
//! The whole UI is driven off the shared [`JsonManifestStore`] cache: a rebuild
//! re-reads `store.get(id)` for every row and re-facets against the live
//! [`PackageQuery`]. Rebuilds are coalesced onto a GLib idle so a toggle handler
//! never tears down the widget that emitted it.

use crate::helpers::discovery_formatting::{
    category_label, failure_summary, package_count, time_ago,
};
use crate::helpers::facet_engine;
use crate::helpers::image_parser::ImageParser;
use crate::models::image_manifest::{ImageManifest, LastOutcome, PackageQuery};
use crate::models::ParsedImage;
use crate::state::AppServices;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

/// One chip in the active-filter bar: its label, and the mutation that clears
/// just that constraint when the chip's ✕ is clicked.
type ActiveFilterChip = (String, Box<dyn Fn(&mut PackageQuery)>);

/// Public entry point. Opens the modal discovery dialog over `parent`; when the
/// user commits, `on_pick` is called with the chosen image id and the window is
/// closed.
pub fn show_image_discovery_dialog(
    parent: &impl IsA<gtk::Widget>,
    services: Arc<AppServices>,
    on_pick: Rc<dyn Fn(String)>,
) {
    let window = adw::Window::builder()
        .title(crate::tr_en!("Find image by package"))
        .default_width(1040)
        .default_height(700)
        .modal(true)
        .build();
    if let Some(root) = parent.root().and_downcast::<gtk::Window>() {
        window.set_transient_for(Some(&root));
    }

    // ── Chrome ────────────────────────────────────────────────────────────
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());

    let paned = gtk::Paned::new(gtk::Orientation::Horizontal);
    paned.set_wide_handle(true);
    paned.set_position(380);
    // `shrink-*-child` defaults to TRUE, which lets GTK allocate a pane LESS
    // than its own minimum and clip whatever does not fit — content pushed off
    // the modal's left edge, with the divider still sitting where it was asked
    // to. Refusing to shrink makes GTK move the divider instead, which is
    // visible and recoverable. `cargo run --example facet_pane_probe` prints
    // the default.
    paned.set_shrink_start_child(false);
    paned.set_shrink_end_child(false);
    paned.set_vexpand(true);
    paned.set_hexpand(true);

    // ── Left pane widgets ─────────────────────────────────────────────────
    let left = gtk::Box::new(gtk::Orientation::Vertical, 8);
    // A floor, not a ceiling: `set_size_request` is a minimum. With the facet
    // labels now ellipsized the pane's own minimum is tiny, and without this
    // the divider could be dragged until the filters were unreadable.
    left.set_size_request(FACET_PANE_MIN, -1);
    left.set_margin_start(12);
    left.set_margin_end(12);
    left.set_margin_top(12);
    left.set_margin_bottom(12);

    let pkg_search = gtk::SearchEntry::new();
    pkg_search.set_placeholder_text(Some(crate::tr_en!("Filter packages…")));
    left.append(&pkg_search);

    let chips_header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let chips_title = gtk::Label::new(Some(crate::tr_en!("Active filters")));
    chips_title.add_css_class("heading");
    chips_title.set_halign(gtk::Align::Start);
    chips_title.set_hexpand(true);
    let clear_btn = gtk::Button::with_label(crate::tr_en!("Clear all"));
    clear_btn.add_css_class("flat");
    chips_header.append(&chips_title);
    chips_header.append(&clear_btn);
    left.append(&chips_header);

    let chips_box = gtk::FlowBox::new();
    chips_box.set_selection_mode(gtk::SelectionMode::None);
    chips_box.set_row_spacing(4);
    chips_box.set_column_spacing(4);
    chips_box.set_max_children_per_line(20);
    chips_box.set_valign(gtk::Align::Start);

    // Two rows of chips, then it scrolls.
    //
    // Unbounded, the FlowBox grew downward as filters were added and drew over
    // the facet list beneath it — the pane is a plain Box, so nothing stopped
    // it. A max height turns "more filters than fit" into a scroll instead of
    // an overlap, and `propagate_natural_height` keeps the strip at its natural
    // size until then, so one chip does not reserve room for two.
    let chips_scroll = gtk::ScrolledWindow::new();
    chips_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    chips_scroll.set_propagate_natural_height(true);
    chips_scroll.set_max_content_height(CHIP_ROWS_BEFORE_SCROLL * CHIP_ROW_HEIGHT);
    chips_scroll.set_child(Some(&chips_box));
    left.append(&chips_scroll);

    let facet_scroll = gtk::ScrolledWindow::new();
    facet_scroll.set_vexpand(true);
    facet_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    let facet_container = gtk::Box::new(gtk::Orientation::Vertical, 4);
    facet_scroll.set_child(Some(&facet_container));
    left.append(&facet_scroll);

    // ── Right pane widgets ────────────────────────────────────────────────
    let right = gtk::Box::new(gtk::Orientation::Vertical, 8);
    right.set_margin_start(12);
    right.set_margin_end(12);
    right.set_margin_top(12);
    right.set_margin_bottom(12);

    let img_search = gtk::SearchEntry::new();
    img_search.set_placeholder_text(Some(crate::tr_en!("Search images…")));
    right.append(&img_search);

    let subtitle = gtk::Label::new(Some(crate::tr_en!("Loading images…")));
    subtitle.add_css_class("dim-label");
    subtitle.set_halign(gtk::Align::Start);
    right.append(&subtitle);

    let img_scroll = gtk::ScrolledWindow::new();
    img_scroll.set_vexpand(true);
    img_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    let img_container = gtk::Box::new(gtk::Orientation::Vertical, 12);
    img_scroll.set_child(Some(&img_container));
    right.append(&img_scroll);

    paned.set_start_child(Some(&left));
    paned.set_end_child(Some(&right));
    toolbar.set_content(Some(&paned));
    window.set_content(Some(&toolbar));

    let ui = Rc::new(DiscoveryUi {
        services,
        window: window.clone(),
        on_pick,
        all_images: RefCell::new(Vec::new()),
        query: RefCell::new(PackageQuery::default()),
        pkg_filter: RefCell::new(String::new()),
        img_filter: RefCell::new(String::new()),
        running: RefCell::new(HashSet::new()),
        loaded: Cell::new(false),
        rebuild_pending: Cell::new(false),
        facet_container,
        chips_box,
        chips_scroll,
        img_container,
        subtitle,
    });

    // ── Wire interactions ─────────────────────────────────────────────────
    {
        let ui = ui.clone();
        pkg_search.connect_search_changed(move |e| {
            *ui.pkg_filter.borrow_mut() = e.text().to_string();
            ui.schedule_rebuild();
        });
    }
    {
        let ui = ui.clone();
        img_search.connect_search_changed(move |e| {
            *ui.img_filter.borrow_mut() = e.text().to_string();
            ui.schedule_rebuild();
        });
    }
    {
        let ui = ui.clone();
        clear_btn.connect_clicked(move |_| {
            *ui.query.borrow_mut() = PackageQuery::default();
            ui.schedule_rebuild();
        });
    }

    // ── Load the catalogue, then populate ─────────────────────────────────
    {
        let ui = ui.clone();
        let services = ui.services.clone();
        glib::spawn_future_local(async move {
            let svc = services.clone();
            let result = services
                .spawn(async move {
                    let token = svc.get_token().await.unwrap_or_default();
                    svc.images.get_images(&token).await
                })
                .await;

            match result {
                Ok(raw) => {
                    *ui.all_images.borrow_mut() = ImageParser::parse_all(&raw);
                }
                Err(e) => {
                    ui.subtitle
                        .set_text(&crate::tr_fmt!("Failed to load images: {}", e));
                }
            }
            ui.loaded.set(true);
            ui.rebuild();
        });
    }

    window.present();
}

/// Shared, reference-counted dialog state. Lives on the GLib main thread; every
/// field is single-threaded (`RefCell`/`Cell`).
struct DiscoveryUi {
    services: Arc<AppServices>,
    window: adw::Window,
    on_pick: Rc<dyn Fn(String)>,

    all_images: RefCell<Vec<ParsedImage>>,
    query: RefCell<PackageQuery>,
    pkg_filter: RefCell<String>,
    img_filter: RefCell<String>,
    /// Image ids with a probe currently in flight (shown as "Discovering…").
    running: RefCell<HashSet<String>>,
    loaded: Cell<bool>,
    rebuild_pending: Cell<bool>,

    facet_container: gtk::Box,
    chips_box: gtk::FlowBox,
    /// The scroller around `chips_box`. Held rather than reached for through
    /// `parent()`: a `ScrolledWindow` wraps its child in a `Viewport`, so
    /// walking up two levels works only as long as that stays true.
    chips_scroll: gtk::ScrolledWindow,
    img_container: gtk::Box,
    subtitle: gtk::Label,
}

impl DiscoveryUi {
    fn store(&self) -> &crate::services::manifest_store::JsonManifestStore {
        &self.services.image_manifests
    }

    /// Coalesce rebuilds onto a GLib idle tick so a signal handler can safely
    /// request a rebuild that will tear down the widget that emitted it.
    fn schedule_rebuild(self: &Rc<Self>) {
        if self.rebuild_pending.replace(true) {
            return;
        }
        let me = self.clone();
        glib::idle_add_local_once(move || {
            me.rebuild_pending.set(false);
            me.rebuild();
        });
    }

    /// Rebuild the facet pane, chips, subtitle and grouped image list from the
    /// current query + cache.
    fn rebuild(self: &Rc<Self>) {
        self.rebuild_facets();
        self.rebuild_chips();
        self.rebuild_images();
        self.update_subtitle();
    }

    // ── Left pane: facets ─────────────────────────────────────────────────

    fn rebuild_facets(self: &Rc<Self>) {
        clear_box(&self.facet_container);
        let facets = facet_engine::facets_for_query(self.store(), &self.query.borrow());
        let needle = self.pkg_filter.borrow().trim().to_lowercase();

        for facet in &facets {
            let visible: Vec<&facet_engine::FacetValue> = facet
                .values
                .iter()
                .filter(|v| needle.is_empty() || v.value.to_lowercase().contains(&needle))
                .collect();
            if visible.is_empty() {
                continue;
            }

            let header = facet_label(&format!("{}  ({})", facet.category, visible.len()));
            header.add_css_class("heading");
            header.set_margin_top(6);
            self.facet_container.append(&header);

            // Only the most-used values, plus anything already ticked.
            //
            // A real CANFAR catalogue facets to ~4,650 distinct values —
            // 2,488 dpkg names alone — and this built a CheckButton for every
            // one, then tore them all down and built them again on the next
            // keystroke. The facet computation itself takes under 4ms; the
            // widgets were the cost. Nobody scrolls 2,488 checkboxes either, so
            // capping is the better interface as well as the faster one: the
            // Filter packages box above is how you reach the rest.
            let (shown, hidden) = cap_values(&visible, &self.query.borrow(), &facet.category);

            for value in shown {
                let check = gtk::CheckButton::new();
                check.set_child(Some(&facet_label(&format!(
                    "{}  ·  {}",
                    value.value, value.count
                ))));
                let selected = is_selected(&self.query.borrow(), &facet.category, &value.value);
                check.set_active(selected);
                // A greyed value is unreachable; keep already-ticked values usable.
                check.set_sensitive(value.enabled || selected);

                let ui = self.clone();
                let category = facet.category.clone();
                let val = value.value.clone();
                // set_active above happens BEFORE connect, so this never fires spuriously.
                check.connect_toggled(move |c| {
                    apply_facet_toggle(&mut ui.query.borrow_mut(), &category, &val, c.is_active());
                    ui.schedule_rebuild();
                });
                self.facet_container.append(&check);
            }

            if hidden > 0 {
                let more = dim_label(&crate::tr_plural!(
                    hidden,
                    "+{} more — type above to narrow",
                    "+{} more — type above to narrow"
                ));
                more.set_halign(gtk::Align::Start);
                more.add_css_class("caption");
                self.facet_container.append(&more);
            }
        }

        if self.facet_container.first_child().is_none() {
            let empty = dim_label(if self.loaded.get() {
                "No discovered packages yet — inspect an image on the right."
            } else {
                "Loading…"
            });
            self.facet_container.append(&empty);
        }
    }

    // ── Left pane: active-filter chips ────────────────────────────────────

    fn rebuild_chips(self: &Rc<Self>) {
        while let Some(child) = self.chips_box.first_child() {
            self.chips_box.remove(&child);
        }
        let q = self.query.borrow().clone();

        let mut chips: Vec<ActiveFilterChip> = Vec::new();
        if let Some(fam) = &q.os_family {
            let f = fam.clone();
            chips.push((format!("OS family: {f}"), Box::new(|q| q.os_family = None)));
        }
        if let Some(ver) = &q.os_version {
            let v = ver.clone();
            chips.push((
                format!("OS version: {v}"),
                Box::new(|q| q.os_version = None),
            ));
        }
        for pkg in &q.packages {
            let p = pkg.clone();
            chips.push((
                format!("pkg: {p}"),
                Box::new(move |q| q.packages.retain(|x| x != &p)),
            ));
        }
        for cap in &q.capabilities {
            let c = cap.clone();
            chips.push((
                format!("cap: {c}"),
                Box::new(move |q| q.capabilities.retain(|x| x != &c)),
            ));
        }
        if let Some(want) = q.python {
            chips.push((
                format!("{} Python", if want { "with" } else { "no" }),
                Box::new(|q| q.python = None),
            ));
        }
        if let Some(want) = q.r {
            chips.push((
                format!("{} R", if want { "with" } else { "no" }),
                Box::new(|q| q.r = None),
            ));
        }

        for (label, remove) in chips {
            // The widest thing in the pane, by measurement — an OS-version chip
            // reads "OS version: 22.04.5 LTS (Jammy Jellyfish)". Left to size
            // itself it sets the pane's minimum and pushes the divider, or is
            // clipped. The text is on the chip AND in its tooltip.
            let btn = gtk::Button::new();
            btn.set_child(Some(&facet_label(&format!("{label}  ✕"))));
            btn.set_tooltip_text(Some(&label));
            btn.add_css_class("pill");
            let ui = self.clone();
            let remove = Rc::new(remove);
            btn.connect_clicked(move |_| {
                remove(&mut ui.query.borrow_mut());
                ui.schedule_rebuild();
            });
            self.chips_box.append(&btn);
        }

        // Hide the strip, not just its contents: an empty FlowBox inside a
        // visible ScrolledWindow still reserves height.
        self.chips_scroll
            .set_visible(self.chips_box.first_child().is_some());
    }

    // ── Right pane: grouped image list ────────────────────────────────────

    fn rebuild_images(self: &Rc<Self>) {
        clear_box(&self.img_container);
        let q = self.query.borrow().clone();
        let img_needle = self.img_filter.borrow().trim().to_lowercase();
        let running = self.running.borrow().clone();

        // Filter → group by project (project ascending, images by version desc).
        let mut groups: BTreeMap<String, Vec<ParsedImage>> = BTreeMap::new();
        for img in self.all_images.borrow().iter() {
            let outcome = self.store().get(&img.id);
            let is_running = running.contains(&img.id);
            let manifest = outcome.as_ref().and_then(|o| o.manifest());

            // Discovered rows honour the query; undiscovered/failed/running rows
            // only survive an empty query (there is nothing to match against).
            let include = match manifest {
                Some(m) if !is_running => q.is_empty() || q.matches(m),
                _ => q.is_empty(),
            };
            if !include {
                continue;
            }
            if !img_needle.is_empty()
                && !img.id.to_lowercase().contains(&img_needle)
                && !img.display_name.to_lowercase().contains(&img_needle)
            {
                continue;
            }
            groups
                .entry(img.project.clone())
                .or_default()
                .push(img.clone());
        }

        if groups.is_empty() {
            let msg = if !self.loaded.get() {
                "Loading images…"
            } else if self.all_images.borrow().is_empty() {
                "No images available."
            } else {
                "No images match the current filters."
            };
            self.img_container.append(&dim_label(msg));
            return;
        }

        for (project, mut images) in groups {
            images.sort_by(|a, b| b.version.cmp(&a.version));
            let group = adw::PreferencesGroup::new();
            group.set_title(if project.is_empty() {
                "(no project)"
            } else {
                &project
            });
            for img in &images {
                group.add(&self.build_image_row(img, running.contains(&img.id)));
            }
            self.img_container.append(&group);
        }
    }

    /// One expandable image row: state prefix + status subtitle, Discover / Use
    /// suffix buttons, and (when discovered/failed) an inline detail section.
    fn build_image_row(self: &Rc<Self>, img: &ParsedImage, is_running: bool) -> adw::ExpanderRow {
        let outcome = self.store().get(&img.id);
        let now = chrono::Utc::now().to_rfc3339();

        let row = adw::ExpanderRow::new();
        row.set_title(&img.display_name);
        row.set_subtitle(&status_subtitle(outcome.as_ref(), is_running, &now));

        // State icon.
        let icon = gtk::Image::from_icon_name(state_icon(outcome.as_ref(), is_running));
        if let Some(css) = state_icon_css(outcome.as_ref(), is_running) {
            icon.add_css_class(css);
        }
        row.add_prefix(&icon);

        // Suffix: Discover/Rediscover + Use this image.
        let discovered = outcome.as_ref().map(|o| o.is_success()).unwrap_or(false);
        let discover_btn =
            gtk::Button::with_label(if discovered { "Rediscover" } else { "Discover" });
        discover_btn.add_css_class("flat");
        discover_btn.set_valign(gtk::Align::Center);
        discover_btn.set_sensitive(!is_running);
        if is_running {
            discover_btn.set_label(crate::tr_en!("Discovering…"));
        }
        {
            let ui = self.clone();
            let id = img.id.clone();
            let force = discovered;
            discover_btn.connect_clicked(move |_| {
                ui.start_discovery(id.clone(), force);
            });
        }

        let use_btn = gtk::Button::with_label(crate::tr_en!("Use this image"));
        use_btn.add_css_class("suggested-action");
        use_btn.set_valign(gtk::Align::Center);
        {
            let ui = self.clone();
            let id = img.id.clone();
            use_btn.connect_clicked(move |_| {
                (ui.on_pick)(id.clone());
                ui.window.close();
            });
        }

        row.add_suffix(&discover_btn);
        row.add_suffix(&use_btn);

        // Inline detail: manifest ecosystem sections or failure detail.
        match outcome.as_ref() {
            Some(o) if o.is_success() => {
                if let Some(m) = o.manifest() {
                    row.set_enable_expansion(true);
                    // Built when the row is opened, not before. Every rebuild
                    // was constructing the full package breakdown for every
                    // discovered image — a dozen rows each, none of them
                    // visible — and throwing it away on the next keystroke.
                    let manifest = m.clone();
                    let built = std::cell::Cell::new(false);
                    row.connect_expanded_notify(move |row| {
                        if !row.is_expanded() || built.replace(true) {
                            return;
                        }
                        for detail_row in manifest_detail_rows(&manifest) {
                            row.add_row(&detail_row);
                        }
                    });
                }
            }
            Some(o) => {
                row.set_enable_expansion(true);
                for detail_row in failure_detail_rows(o) {
                    row.add_row(&detail_row);
                }
            }
            None => {
                // Never discovered — nothing to expand.
                row.set_enable_expansion(false);
            }
        }

        row
    }

    /// Kick off (or force-refresh) a probe for `image_id`, marking the row
    /// running, then refresh the row + facets when the coordinator returns.
    fn start_discovery(self: &Rc<Self>, image_id: String, force: bool) {
        if !self.running.borrow_mut().insert(image_id.clone()) {
            return; // already running
        }
        self.schedule_rebuild();

        let me = self.clone();
        let services = self.services.clone();
        let coordinator = self.services.image_discovery.clone();
        glib::spawn_future_local(async move {
            let svc = services.clone();
            let id = image_id.clone();
            let _outcome = services
                .spawn(async move { coordinator.discover_image(&svc, &id, force).await })
                .await;
            me.running.borrow_mut().remove(&image_id);
            me.rebuild();
        });
    }

    fn update_subtitle(self: &Rc<Self>) {
        if !self.loaded.get() {
            return;
        }
        let total = self.all_images.borrow().len();
        let discovered = self
            .all_images
            .borrow()
            .iter()
            .filter(|i| {
                self.store()
                    .get(&i.id)
                    .map(|o| o.is_success())
                    .unwrap_or(false)
            })
            .count();
        self.subtitle.set_text(&crate::tr_fmt!(
            "Discovered {} of {} images",
            discovered,
            total
        ));
    }
}

// ---------------------------------------------------------------------------
// query mutation helpers (facet category ↔ PackageQuery field)
// ---------------------------------------------------------------------------

/// True when `value` is the active selection in the query for `category`.
fn is_selected(q: &PackageQuery, category: &str, value: &str) -> bool {
    match category {
        "OS family" => q.os_family.as_deref() == Some(value),
        "OS version" => q.os_version.as_deref() == Some(value),
        "Capabilities" => q.capabilities.iter().any(|c| c == value),
        _ => q.packages.iter().any(|p| p == value), // Python / R / dpkg / rpm / apk
    }
}

/// Apply a checkbox toggle to the query. OS family/version are single-select
/// (ticking replaces); packages and capabilities are multi-select sets.
fn apply_facet_toggle(q: &mut PackageQuery, category: &str, value: &str, active: bool) {
    match category {
        "OS family" => q.os_family = active.then(|| value.to_string()),
        "OS version" => q.os_version = active.then(|| value.to_string()),
        "Capabilities" => toggle_set(&mut q.capabilities, value, active),
        _ => toggle_set(&mut q.packages, value, active), // Python / R / dpkg / rpm / apk
    }
}

fn toggle_set(set: &mut Vec<String>, value: &str, active: bool) {
    set.retain(|x| x != value);
    if active {
        set.push(value.to_string());
    }
}

// ---------------------------------------------------------------------------
// per-row presentation (mirrors ImageRowViewModel / ManifestDetailViewModel)
// ---------------------------------------------------------------------------

fn status_subtitle(outcome: Option<&LastOutcome>, running: bool, now: &str) -> String {
    if running {
        return "Discovering…".to_string();
    }
    match outcome {
        Some(o) if o.is_success() => {
            let m = o.manifest();
            let count = m.map(package_count).unwrap_or(0);
            format!("{count} packages · {}", time_ago(&o.discovered_at, now))
        }
        Some(o) => match &o.outcome {
            crate::models::image_manifest::DiscoveryOutcome::Failure {
                category, message, ..
            } => format!(
                "{} · {}",
                failure_summary(category, message),
                time_ago(&o.discovered_at, now)
            ),
            _ => "Failed".to_string(),
        },
        None => "Not inspected yet".to_string(),
    }
}

fn state_icon(outcome: Option<&LastOutcome>, running: bool) -> &'static str {
    if running {
        return "content-loading-symbolic";
    }
    match outcome {
        Some(o) if o.is_success() => "emblem-ok-symbolic",
        Some(_) => "dialog-warning-symbolic",
        None => "content-loading-symbolic",
    }
}

fn state_icon_css(outcome: Option<&LastOutcome>, running: bool) -> Option<&'static str> {
    if running {
        return None;
    }
    match outcome {
        Some(o) if o.is_success() => Some("success"),
        Some(_) => Some("warning"),
        None => None,
    }
}

/// The manifest broken into OS/capabilities/ecosystem rows for the inline panel
/// (mirrors `ManifestDetailBuilder.Build` section order).
fn manifest_detail_rows(m: &ImageManifest) -> Vec<adw::ActionRow> {
    let mut rows = Vec::new();

    let os = format!(
        "{} {}",
        m.os_family.clone().unwrap_or_default(),
        m.os_version.clone().unwrap_or_default()
    );
    let os = os.trim();
    if !os.is_empty() {
        rows.push(info_row("OS", os));
    }
    if let Some(k) = &m.kernel {
        if !k.is_empty() && k != "unknown" {
            rows.push(info_row("Kernel", k));
        }
    }
    if !m.capabilities.is_empty() {
        rows.push(info_row("Capabilities", &m.capabilities.join(", ")));
    }

    // Python — flat snapshot first, then each conda-scoped env.
    push_pkg_section(&mut rows, "Python", &m.python);
    for (env, pkgs) in &m.python_by_env {
        push_pkg_section(&mut rows, &format!("Python · {env}"), pkgs);
    }
    if !m.conda_envs.is_empty() {
        rows.push(info_row("Conda envs", &m.conda_envs.join(", ")));
    }
    push_pkg_section(&mut rows, "R", &m.r_packages);
    push_pkg_section(&mut rows, "System (apt / dpkg)", &m.dpkg);
    push_pkg_section(&mut rows, "System (rpm)", &m.rpm);
    push_pkg_section(&mut rows, "System (apk)", &m.apk);

    if rows.is_empty() {
        rows.push(info_row("Manifest", "No packages recorded."));
    }
    rows
}

/// Failure detail rows: the diagnosis, expandable to the job's own logs and
/// events, plus the probe job id.
///
/// The message used to go straight into a subtitle. It now carries the tail of
/// the job's output — which is the point, and which an unbounded subtitle would
/// render as a wall of text pushing the rest of the pane off screen. Same
/// treatment as the Batch Jobs history, from the same helper.
fn failure_detail_rows(outcome: &LastOutcome) -> Vec<gtk::Widget> {
    let mut rows: Vec<gtk::Widget> = Vec::new();
    if let crate::models::image_manifest::DiscoveryOutcome::Failure {
        category,
        message,
        job_id,
    } = &outcome.outcome
    {
        rows.push(crate::ui::failure_detail::reason_row(
            category_label(category),
            "",
            message,
            None,
        ));
        if let Some(job) = job_id {
            if !job.is_empty() {
                // The job itself is already deleted — the coordinator reaps its
                // own probes — so say where the record actually lives rather
                // than implying `skaha logs` will work.
                rows.push(
                    info_row(
                        crate::tr_en!("Probe job"),
                        &crate::tr_fmt!(
                            "{} — deleted after the probe finished; the full \
                             output is kept under Batch Jobs → History",
                            job
                        ),
                    )
                    .upcast(),
                );
            }
        }
    }
    rows
}

/// Append an ecosystem section row (title + up-to-N names) when non-empty.
fn push_pkg_section(rows: &mut Vec<adw::ActionRow>, title: &str, pkgs: &[String]) {
    if pkgs.is_empty() {
        return;
    }
    const MAX: usize = 40;
    let mut names: Vec<String> = pkgs.to_vec();
    names.sort();
    let shown = names.len().min(MAX);
    let mut body = names[..shown].join(", ");
    if names.len() > MAX {
        body.push_str(&format!(", … (+{} more)", names.len() - MAX));
    }
    rows.push(info_row(&format!("{title}  ({})", pkgs.len()), &body));
}

fn info_row(title: &str, subtitle: &str) -> adw::ActionRow {
    let row = adw::ActionRow::new();
    row.set_title(title);
    row.set_subtitle(subtitle);
    row.set_subtitle_lines(0);
    row.set_title_lines(0);
    row
}

// ---------------------------------------------------------------------------
// small widget helpers
// ---------------------------------------------------------------------------

fn clear_box(container: &gtk::Box) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}

/// How many rows of active-filter chips are shown before the strip scrolls.
///
/// Two, so a second filter does not immediately hide the first. A const
/// assertion, since one row that scrolls is worse than no bound at all.
const CHIP_ROWS_BEFORE_SCROLL: i32 = 2;
const _: () = assert!(CHIP_ROWS_BEFORE_SCROLL >= 2);

/// The height one row of chips occupies, including the row spacing.
const CHIP_ROW_HEIGHT: i32 = 40;

/// How many values one facet category renders.
///
/// A real catalogue produces thousands per package category. Rendering them all
/// cost more than everything else in the dialog put together, and no one was
/// ever going to scroll them.
const MAX_FACET_VALUES: usize = 25;

/// The values to render for one category, and how many were left out.
///
/// Ticked values always survive the cap — a filter you cannot see is a filter
/// you cannot remove. The rest are the highest-count ones, since those are the
/// ones that narrow a search usefully, and they are returned in the
/// alphabetical order the pane has always shown so the list stays scannable.
fn cap_values<'a>(
    values: &[&'a facet_engine::FacetValue],
    query: &PackageQuery,
    category: &str,
) -> (Vec<&'a facet_engine::FacetValue>, usize) {
    if values.len() <= MAX_FACET_VALUES {
        return (values.to_vec(), 0);
    }

    let mut ranked: Vec<&facet_engine::FacetValue> = values.to_vec();
    ranked.sort_by(|a, b| {
        let a_selected = is_selected(query, category, &a.value);
        let b_selected = is_selected(query, category, &b.value);
        b_selected
            .cmp(&a_selected)
            .then_with(|| b.count.cmp(&a.count))
            .then_with(|| a.value.cmp(&b.value))
    });
    let hidden = ranked.len() - MAX_FACET_VALUES;
    ranked.truncate(MAX_FACET_VALUES);
    ranked.sort_by(|a, b| a.value.cmp(&b.value));
    (ranked, hidden)
}

/// How wide a facet label may get before it truncates.
///
/// The pane is 380px; this keeps the longest values — "22.04.5 LTS (Jammy
/// Jellyfish)  ·  0" — from setting a minimum wider than that.
const FACET_LABEL_CHARS: i32 = 26;

/// The narrowest the filter pane may be dragged.
const FACET_PANE_MIN: i32 = 240;

/// A facet label that truncates instead of forcing the pane wider.
///
/// `CheckButton::with_label` builds a label with no ellipsize, so its MINIMUM
/// width is the whole string — and a `ScrolledWindow` with a horizontal policy
/// of `Never` passes its child's minimum straight through. Between them the
/// left pane demanded more than its 380px and overflowed off the left edge of
/// the modal, taking "Active filters" and half of every facet name with it.
///
/// The full text goes in a tooltip, since it is now allowed to truncate.
fn facet_label(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label.set_max_width_chars(FACET_LABEL_CHARS);
    label.set_halign(gtk::Align::Start);
    label.set_xalign(0.0);
    label.set_tooltip_text(Some(text));
    label
}

fn dim_label(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.add_css_class("dim-label");
    label.set_halign(gtk::Align::Start);
    label.set_wrap(true);
    label.set_margin_top(8);
    label
}

#[cfg(test)]
mod pane_layout_tests {
    use super::*;

    const SOURCE: &str = include_str!("image_discovery_dialog.rs");

    fn value(v: &str, count: usize) -> facet_engine::FacetValue {
        facet_engine::FacetValue {
            value: v.to_string(),
            count,
            enabled: true,
        }
    }

    #[test]
    fn a_short_category_is_shown_whole() {
        let values: Vec<facet_engine::FacetValue> =
            (0..5).map(|i| value(&format!("v{i}"), i)).collect();
        let refs: Vec<&facet_engine::FacetValue> = values.iter().collect();
        let (shown, hidden) = cap_values(&refs, &PackageQuery::default(), "OS family");
        assert_eq!(shown.len(), 5);
        assert_eq!(hidden, 0);
    }

    #[test]
    fn a_huge_category_is_capped_and_says_how_many_it_left_out() {
        // A real CANFAR catalogue facets dpkg to 2,488 distinct names. Building
        // a checkbox for each, on every keystroke, was the whole of the
        // dialog's slowness — the facet computation behind it takes under 4ms.
        let values: Vec<facet_engine::FacetValue> =
            (0..2488).map(|i| value(&format!("lib{i:04}"), i)).collect();
        let refs: Vec<&facet_engine::FacetValue> = values.iter().collect();
        let (shown, hidden) = cap_values(&refs, &PackageQuery::default(), "System (apt / dpkg)");
        assert_eq!(shown.len(), MAX_FACET_VALUES);
        assert_eq!(hidden, 2488 - MAX_FACET_VALUES);
    }

    #[test]
    fn the_values_kept_are_the_ones_that_narrow_a_search() {
        // Highest count first: a package one image has filters nothing useful,
        // and with a cap the choice of WHICH to drop is the whole design.
        let values: Vec<facet_engine::FacetValue> =
            (0..100).map(|i| value(&format!("lib{i:03}"), i)).collect();
        let refs: Vec<&facet_engine::FacetValue> = values.iter().collect();
        let (shown, _) = cap_values(&refs, &PackageQuery::default(), "System (apt / dpkg)");
        assert!(
            shown.iter().all(|v| v.count >= 100 - MAX_FACET_VALUES),
            "a low-count value displaced a high-count one"
        );
        // Displayed alphabetically, whatever order they were picked in.
        let names: Vec<&String> = shown.iter().map(|v| &v.value).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted, "the list is no longer scannable");
    }

    #[test]
    fn a_ticked_value_survives_the_cap() {
        // A filter you cannot see is a filter you cannot remove — and the
        // lowest-count value is exactly the one a user is most likely to have
        // ticked, since it is the one that narrowed hardest.
        let values: Vec<facet_engine::FacetValue> =
            (0..500).map(|i| value(&format!("lib{i:03}"), i)).collect();
        let refs: Vec<&facet_engine::FacetValue> = values.iter().collect();
        let query = PackageQuery {
            packages: vec!["lib000".to_string()],
            ..Default::default()
        };
        let (shown, _) = cap_values(&refs, &query, "System (apt / dpkg)");
        assert!(
            shown.iter().any(|v| v.value == "lib000"),
            "the only ticked value — and the rarest — was dropped"
        );
    }

    #[test]
    fn the_active_filter_strip_scrolls_instead_of_growing_over_the_facets() {
        // The FlowBox is in a plain Box, so nothing stopped it growing downward
        // and drawing over the facet list as filters were added.
        let code = crate::testing::without_comments(crate::testing::code(SOURCE));
        assert!(
            code.contains("chips_scroll.set_max_content_height"),
            "the chip strip has no height bound again"
        );
        assert!(
            code.contains("chips_scroll.set_propagate_natural_height(true)"),
            "one chip now reserves room for two"
        );
        // Held, not reached for: a ScrolledWindow wraps its child in a
        // Viewport, so walking up from the FlowBox is a guess about GTK's
        // internals that silently stops working if they change.
        assert!(
            !code.contains("chips_box.parent()"),
            "the chip strip is being found by walking the widget tree again"
        );
    }

    #[test]
    fn a_rows_package_breakdown_is_built_only_when_it_is_opened() {
        // Every rebuild was constructing the full breakdown for every
        // discovered image — around a dozen rows each, none of them visible.
        let code = crate::testing::without_comments(crate::testing::code(SOURCE));
        let built = code
            .find("manifest_detail_rows(&manifest)")
            .expect("the package breakdown is gone");
        let lazily = code
            .find("connect_expanded_notify")
            .expect("the breakdown is built eagerly again");
        assert!(
            lazily < built,
            "the breakdown is constructed before anything asks to see it"
        );
    }

    #[test]
    fn a_pane_may_not_be_squeezed_below_its_own_minimum() {
        // GtkPaned defaults shrink-*-child to TRUE, which lets GTK allocate a
        // pane LESS than its minimum and clip whatever does not fit — content
        // pushed off the modal's left edge, with the divider still sitting
        // where it was asked to. Refusing to shrink makes GTK move the divider
        // instead: visible, and recoverable by dragging it back.
        //
        // `cargo run --example facet_pane_probe` prints the default and the
        // pane's measured minimum.
        let code = crate::testing::code(SOURCE);
        assert!(code.contains("paned.set_shrink_start_child(false)"));
        assert!(code.contains("paned.set_shrink_end_child(false)"));
    }

    #[test]
    fn nothing_in_the_filter_pane_sizes_itself_by_its_text() {
        // A label with no ellipsize demands its whole string as a MINIMUM, and
        // a ScrolledWindow whose horizontal policy is Never passes that minimum
        // straight through. One long facet value — "22.04.5 LTS (Jammy
        // Jellyfish)" — is enough to widen the pane past the divider.
        // Code only: the comment on `facet_label` explains the bug by
        // naming it, and prose about a defect is not the defect.
        let code = crate::testing::without_comments(crate::testing::code(SOURCE));
        assert!(
            !code.contains("CheckButton::with_label"),
            "a facet row builds its own unellipsized label again"
        );
        assert!(
            !code.contains("Button::with_label(&format!(\"{label}"),
            "a chip builds its own unellipsized label again"
        );
        let at = code.find("fn facet_label").expect("facet_label is gone");
        let end = code[at..]
            .find("\n}\n")
            .map(|e| at + e)
            .unwrap_or(code.len());
        let body = &code[at..end];
        assert!(body.contains("EllipsizeMode::End"));
        assert!(body.contains("set_max_width_chars"));
        assert!(
            body.contains("set_tooltip_text"),
            "a label allowed to truncate must still be readable in full somewhere"
        );
    }
}
