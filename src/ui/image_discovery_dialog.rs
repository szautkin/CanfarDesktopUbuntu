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

use crate::helpers::discovery_formatting::{category_label, package_count, time_ago};
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
        .title("Find image by package")
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
    paned.set_vexpand(true);
    paned.set_hexpand(true);

    // ── Left pane widgets ─────────────────────────────────────────────────
    let left = gtk::Box::new(gtk::Orientation::Vertical, 8);
    left.set_margin_start(12);
    left.set_margin_end(12);
    left.set_margin_top(12);
    left.set_margin_bottom(12);

    let pkg_search = gtk::SearchEntry::new();
    pkg_search.set_placeholder_text(Some("Filter packages…"));
    left.append(&pkg_search);

    let chips_header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let chips_title = gtk::Label::new(Some("Active filters"));
    chips_title.add_css_class("heading");
    chips_title.set_halign(gtk::Align::Start);
    chips_title.set_hexpand(true);
    let clear_btn = gtk::Button::with_label("Clear all");
    clear_btn.add_css_class("flat");
    chips_header.append(&chips_title);
    chips_header.append(&clear_btn);
    left.append(&chips_header);

    let chips_box = gtk::FlowBox::new();
    chips_box.set_selection_mode(gtk::SelectionMode::None);
    chips_box.set_row_spacing(4);
    chips_box.set_column_spacing(4);
    chips_box.set_max_children_per_line(20);
    left.append(&chips_box);

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
    img_search.set_placeholder_text(Some("Search images…"));
    right.append(&img_search);

    let subtitle = gtk::Label::new(Some("Loading images…"));
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
                    ui.subtitle.set_text(&format!("Failed to load images: {e}"));
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

            let header = gtk::Label::new(Some(&format!("{}  ({})", facet.category, visible.len())));
            header.add_css_class("heading");
            header.set_halign(gtk::Align::Start);
            header.set_margin_top(6);
            self.facet_container.append(&header);

            for value in visible {
                let check =
                    gtk::CheckButton::with_label(&format!("{}  ·  {}", value.value, value.count));
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
            let btn = gtk::Button::with_label(&format!("{label}  ✕"));
            btn.add_css_class("pill");
            let ui = self.clone();
            let remove = Rc::new(remove);
            btn.connect_clicked(move |_| {
                remove(&mut ui.query.borrow_mut());
                ui.schedule_rebuild();
            });
            self.chips_box.append(&btn);
        }

        self.chips_box
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
            discover_btn.set_label("Discovering…");
        }
        {
            let ui = self.clone();
            let id = img.id.clone();
            let force = discovered;
            discover_btn.connect_clicked(move |_| {
                ui.start_discovery(id.clone(), force);
            });
        }

        let use_btn = gtk::Button::with_label("Use this image");
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
                    for detail_row in manifest_detail_rows(m) {
                        row.add_row(&detail_row);
                    }
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
        self.subtitle
            .set_text(&format!("Discovered {discovered} of {total} images"));
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
            crate::models::image_manifest::DiscoveryOutcome::Failure { category, .. } => {
                format!(
                    "{} · {}",
                    category_label(category),
                    time_ago(&o.discovered_at, now)
                )
            }
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

/// Failure detail rows: the category-labelled message, and the probe job id when
/// one is available (so the user can pull logs by hand).
fn failure_detail_rows(outcome: &LastOutcome) -> Vec<adw::ActionRow> {
    let mut rows = Vec::new();
    if let crate::models::image_manifest::DiscoveryOutcome::Failure {
        category,
        message,
        job_id,
    } = &outcome.outcome
    {
        rows.push(info_row(category_label(category), message));
        if let Some(job) = job_id {
            if !job.is_empty() {
                rows.push(info_row("Probe job", job));
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

fn dim_label(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.add_css_class("dim-label");
    label.set_halign(gtk::Align::Start);
    label.set_wrap(true);
    label.set_margin_top(8);
    label
}
