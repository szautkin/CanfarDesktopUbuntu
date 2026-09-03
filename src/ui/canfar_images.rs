//! Full-width dashboard widget listing every CANFAR session container image.
//!
//! Port of `Views/Controls/CanfarImagesControl.xaml(.cs)` +
//! `DashboardPage.OnUseImageRequested`. Images come from
//! [`ImageService::get_images`]; each row shows its cached image-discovery state
//! (never inspected / discovered + package count / failed) read straight from the
//! shared [`JsonManifestStore`], with three per-row / per-widget actions:
//!
//!   * **Inspect** — runs the discovery coordinator probe for that image
//!     (`services.image_discovery.discover_image`, cached successes short-circuit)
//!     and refreshes the row.
//!   * **Use this image** — fires the `on_use_image` callback with the image id;
//!     the dashboard wires it to the `use-launch-image` app action so the launch
//!     form pre-selects the image (mirrors the Windows `UseImageRequested` event).
//!   * **Find images by package…** — opens the faceted
//!     [`show_image_discovery_dialog`]; picking an image there also fires
//!     `on_use_image` and refreshes the list (the dialog may have probed images).
//!
//! Two filter rows (the SelectorBar analog, single-select linked toggles) plus a
//! total-count badge sit in the header: session type, then project. The type
//! answers "what kind of session", the project answers "whose images", and the
//! second row is rebuilt from whatever the first leaves — so it only ever
//! offers projects that still have something in them, and picking CARTA
//! narrows it from 21 buttons to 2. Rows are ordered discovered → failed →
//! never, then by image id (mirrors the reference sort).
//!
//! The list is what the LAUNCH FORM can offer, not everything `/v1/image`
//! returns — see [`launchable_here`]. Of the platform's 365 images, 77 are
//! `desktop-app` and nothing else: an application published inside a desktop
//! session, which no launch tab starts on its own. They made up a fifth of the
//! card, and Inspect on one spent a probe job on something the user could never
//! run. Images the user added from the registry are always kept, whatever their
//! labels say.

use crate::helpers::discovery_formatting::{failure_summary, time_ago};
use crate::helpers::image_parser::ImageParser;
use crate::models::image_manifest::DiscoveryOutcome;
use crate::models::ParsedImage;
use crate::services::image_discovery_coordinator::SyncProgress;
use crate::services::manifest_store::RowSummary;
use crate::state::AppServices;
use crate::ui::image_discovery_dialog::show_image_discovery_dialog;
use crate::ui::registry_browser_dialog::show_registry_browser_dialog;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

/// How many image rows to build.
///
/// The list is 180–360px tall, so about six rows are ever on screen. GTK4's
/// `ListBox` does not virtualise — every row is a live widget — and the Portal
/// pays a measure pass over all of them on every layout, resize and scroll:
///
/// ```text
///  50 rows ->   5.3 ms per measure
/// 171 rows ->  17.6 ms per measure
/// 368 rows ->  38.5 ms per measure
/// ```
///
/// (`cargo run --release --features fits --example row_cost_probe`.) At a full
/// catalogue that is more than two frames of budget spent measuring rows nobody
/// is looking at. Anyone hunting a specific image has the search in
/// "Find images by package…"; this list is for browsing the top of the order.
const MAX_ROWS: usize = 50;

/// Width reserved for the Inspect button, so its contents can change without
/// moving the row's other action.
const INSPECT_BTN_WIDTH: i32 = 96;

/// 3-state discovery status for one image row (mirrors `ImageDiscoveryStatus`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiscoveryStatus {
    Discovered,
    Failed,
    Unknown,
}

pub struct CanfarImagesView {
    container: gtk::Box,
    services: Arc<AppServices>,
    filter_bar: gtk::Box,
    /// The scroller around `filter_bar`; visibility belongs to it, since it is
    /// what occupies space in the card.
    filter_scroll: gtk::ScrolledWindow,
    /// The project filter, and its scroller. Second row, rebuilt whenever the
    /// type selection changes.
    project_bar: gtk::Box,
    project_scroll: gtk::ScrolledWindow,
    count_badge: gtk::Label,
    subtitle: gtk::Label,
    list_box: gtk::ListBox,
    spinner: gtk::Spinner,
    /// Every parsed image last fetched from the images endpoint.
    images: RefCell<Vec<ParsedImage>>,
    /// The distinct session types available, in `available_types` order.
    types: RefCell<Vec<String>>,
    /// The type currently selected in the filter bar (empty ⇒ show all).
    ///
    /// `Rc` because the ListBox's filter function reads it. Capturing the whole
    /// view there would make the widget own the view that owns the widget.
    selected_type: Rc<RefCell<String>>,
    /// The project currently selected (empty ⇒ every project).
    selected_project: Rc<RefCell<String>>,
    #[allow(clippy::type_complexity)]
    on_use_image: Rc<RefCell<Option<Box<dyn Fn(String)>>>>,
}

impl CanfarImagesView {
    pub fn new(services: Arc<AppServices>) -> Rc<Self> {
        let card = crate::ui::card::Card::new(crate::tr_en!("CANFAR Images"));
        let container = card.widget.clone();
        let header = card.header.clone();

        // The count sits with the title rather than after the spacer, so the
        // heading reads "CANFAR Images (42)" as one thing.
        let count_badge = gtk::Label::new(None);
        count_badge.add_css_class("dim-label");
        count_badge.add_css_class("caption");
        header.insert_child_after(&count_badge, header.first_child().as_ref());

        // Pull any manifests this machine has not seen from the user's ARC
        // space. This ran automatically on every sign-in and nobody asked for
        // it: it walks every manifest at 400ms a file and toasts its way
        // through, on the screen where someone is trying to start work. Now it
        // happens when it is wanted.
        let check_btn = gtk::Button::with_label(crate::tr_en!("Check images"));
        check_btn.add_css_class("flat");
        check_btn.set_valign(gtk::Align::Center);
        check_btn.set_tooltip_text(Some(crate::tr_en!(
            "Look in your CANFAR storage for image manifests this machine does not have yet"
        )));
        header.append(&check_btn);

        let find_btn = gtk::Button::with_label(crate::tr_en!("Find images by package…"));
        find_btn.add_css_class("flat");
        find_btn.set_valign(gtk::Align::Center);
        header.append(&find_btn);

        // Next to the package search, because they are the two ways of finding
        // an image and the difference between them is where it looks: the
        // package search looks inside images the app already knows about, this
        // one goes and asks the registry for images it does not.
        let registry_btn = gtk::Button::with_label(crate::tr_en!("Add image from registry"));
        registry_btn.add_css_class("flat");
        registry_btn.set_valign(gtk::Align::Center);
        registry_btn.set_tooltip_text(Some(crate::tr_en!(
            "Search the container registry for an image the platform does not list, and add it to your own"
        )));
        header.append(&registry_btn);

        let spinner = card.spinner.clone();
        let refresh_btn = card.with_refresh();

        // ── Per-type filter bar (linked toggle buttons) ──
        //
        // Inside a horizontal scroller, because the number of buttons is not
        // ours to choose: it is however many session types Skaha reports, seven
        // today. A plain Box makes all of them the card's MINIMUM width, and the
        // card spans two of three homogeneous columns, so seven buttons set the
        // minimum width of the entire Portal grid — past the window, where the
        // page scroller (`hscrollbar_policy(Never)`, deliberately) clips the
        // right-hand column instead of scrolling it.
        //
        // `propagate_natural_width` keeps the bar at its natural size whenever
        // there is room, so this changes nothing until there is not.
        let (filter_bar, filter_scroll) = filter_row();
        card.content.append(&filter_scroll);

        // Second row: the projects. The type answers "what kind of session",
        // the project answers "whose images" — and with 21 projects behind the
        // catalogue, and 62 images in the largest, the type alone still leaves
        // a list nobody browses. Rebuilt from whatever the type selection
        // leaves, so it only ever offers projects that have something in them:
        // picking CARTA narrows this row from 21 buttons to 2.
        let (project_bar, project_scroll) = filter_row();
        card.content.append(&project_scroll);

        // ── Discovered X of Y subtitle + row list, grouped with a tight 6px
        // gap so the caption reads as directly describing the list below it
        // (the outer container's 12px spacing still separates this group
        // from the filter bar above). ──
        let list_section = gtk::Box::new(gtk::Orientation::Vertical, 6);

        let subtitle = gtk::Label::new(None);
        subtitle.add_css_class("dim-label");
        subtitle.set_halign(gtk::Align::Start);
        list_section.append(&subtitle);

        // ── Scrollable row list ──
        let scrolled = gtk::ScrolledWindow::new();
        scrolled.set_vscrollbar_policy(gtk::PolicyType::Automatic);
        scrolled.set_hscrollbar_policy(gtk::PolicyType::Never);
        scrolled.set_min_content_height(180);
        scrolled.set_max_content_height(360);
        scrolled.set_propagate_natural_height(true);

        let list_box = gtk::ListBox::new();
        list_box.add_css_class("boxed-list");
        list_box.set_selection_mode(gtk::SelectionMode::None);
        list_box.set_margin_bottom(12);

        // A placeholder rather than an appended row: with filtering in play
        // "no images" and "none of this type" are the same picture, and the
        // placeholder shows for both without being filtered itself — an
        // appended label would be a row, and the filter would have to special-
        // case it.
        let empty = gtk::Label::new(Some(crate::tr_en!("No images available")));
        empty.add_css_class("dim-label");
        empty.set_margin_top(12);
        empty.set_margin_bottom(12);
        list_box.set_placeholder(Some(&empty));
        scrolled.set_child(Some(&list_box));
        list_section.append(&scrolled);
        card.content.append(&list_section);

        let view = Rc::new(CanfarImagesView {
            container,
            services,
            filter_bar,
            filter_scroll,
            project_bar,
            project_scroll,
            count_badge,
            subtitle,
            list_box,
            spinner,
            images: RefCell::new(Vec::new()),
            types: RefCell::new(Vec::new()),
            selected_type: Rc::new(RefCell::new(String::new())),
            selected_project: Rc::new(RefCell::new(String::new())),
            on_use_image: Rc::new(RefCell::new(None)),
        });

        // Refresh button re-fetches the catalogue.
        {
            let view = view.clone();
            refresh_btn.connect_clicked(move |_| {
                let view = view.clone();
                glib::spawn_future_local(async move {
                    view.refresh().await;
                });
            });
        }

        // Check images: pull manifests from VOSpace, then re-render the list so
        // anything imported is visible without a second click.
        {
            let view = view.clone();
            let btn = check_btn.clone();
            check_btn.connect_clicked(move |_| {
                let view = view.clone();
                let btn = btn.clone();
                // Disabled while it runs: the walk takes as long as the user
                // has manifests, and a second press would start a second walk
                // over the same files.
                btn.set_sensitive(false);
                glib::spawn_future_local(async move {
                    view.sync_manifests_from_vospace().await;
                    view.refresh().await;
                    btn.set_sensitive(true);
                });
            });
        }

        // Find-by-package opens the faceted discovery dialog.
        {
            let view = view.clone();
            find_btn.connect_clicked(move |_| {
                view.clone().open_find_dialog();
            });
        }

        // Add-from-registry opens the browser. Its callback re-reads the
        // catalogue, so an image added in there appears in this list without
        // the user having to know to refresh.
        {
            let view = view.clone();
            registry_btn.connect_clicked(move |_| {
                view.clone().open_registry_browser();
            });
        }

        view
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.container
    }

    /// Register the "Use this image" callback (the dashboard activates the
    /// `use-launch-image` app action with the image id).
    pub fn set_on_use_image(&self, callback: impl Fn(String) + 'static) {
        *self.on_use_image.borrow_mut() = Some(Box::new(callback));
    }

    /// Fetch the image catalogue, then rebuild both filter rows and the list.
    pub async fn refresh(self: &Rc<Self>) {
        self.spinner.set_visible(true);
        self.spinner.start();

        let svc = self.services.clone();
        let result = self
            .services
            .spawn(async move {
                let token = svc.get_token().await.unwrap_or_default();
                // The platform's catalogue AND the images the user added from
                // the registry, from the one place that knows about both.
                svc.image_catalogue(&token).await
            })
            .await;

        self.spinner.stop();
        self.spinner.set_visible(false);

        match result {
            Ok(parsed) => {
                // Only what can actually be launched. See `launchable_here`.
                let mine = self.my_image_ids();
                let parsed: Vec<ParsedImage> = parsed
                    .into_iter()
                    .filter(|img| launchable_here(img, &mine))
                    .collect();

                self.count_badge.set_text(&format!("({})", parsed.len()));
                let types = ImageParser::available_types(&parsed);

                // Keep the current selection if it still exists, else fall back
                // to All (the empty string), never to the first type: an image
                // the user just added may carry no session-type label at all,
                // and landing on a type would leave it filtered out of the very
                // list they added it to see.
                let want = self.selected_type.borrow().clone();
                let selected = if want.is_empty() || types.iter().any(|t| t == &want) {
                    want
                } else {
                    String::new()
                };
                *self.images.borrow_mut() = parsed;
                *self.types.borrow_mut() = types;
                *self.selected_type.borrow_mut() = selected;

                self.rebuild_filter_bar();
                self.rebuild_project_bar();
                self.rebuild_rows();
            }
            Err(e) => {
                self.subtitle
                    .set_text(&crate::tr_fmt!("Failed to load images: {}", e));
            }
        }
    }

    // ── Filter bar ─────────────────────────────────────────────────────────

    /// The session-type row.
    ///
    /// "All" leads it, and it is not a session type — it is the absence of a
    /// filter, which is why it carries the empty string. Without it every view
    /// of this card is filtered, and an image whose registry labels name no
    /// session type has nowhere to appear: the user adds it and the list they
    /// added it to does not show it.
    fn rebuild_filter_bar(self: &Rc<Self>) {
        let types = self.types.borrow().clone();
        let choices = with_all(
            types
                .iter()
                .map(|t| (t.clone(), crate::models::session::type_label(t).to_string())),
        );

        // Read out of the cell BEFORE the row is built, not borrowed across it.
        // `fill_filter_row` mutates widgets and GTK emits `toggled` while it
        // does; a handler that reaches this same cell would meet a live borrow
        // inside a signal trampoline, which cannot unwind — the process aborts
        // rather than panicking. The Portal has hit that once already, and the
        // early-return in the handler is too thin a thing to rest it on.
        let selected = self.selected_type.borrow().clone();
        let view = self.clone();
        fill_filter_row(
            &self.filter_bar,
            &choices,
            &selected,
            move |picked| {
                *view.selected_type.borrow_mut() = picked.to_string();
                // The projects on offer depend on the type, so this row is not
                // just re-filtered but rebuilt — and the project selection may
                // not survive it.
                view.rebuild_project_bar();
                view.rebuild_rows();
            },
        );
        self.filter_scroll.set_visible(!types.is_empty());
    }

    /// The project row, built from whatever the type selection leaves.
    ///
    /// Only projects that still have an image in them: offering a button that
    /// selects nothing is offering the user a dead end. A project that has gone
    /// away with the type change takes the selection with it, back to All —
    /// otherwise the card shows an empty list and the reason is a button in a
    /// row the user is not looking at.
    fn rebuild_project_bar(self: &Rc<Self>) {
        let projects = self.projects_for_selected_type();

        // A project the new type does not have takes the selection with it.
        let surviving = surviving_project(&self.selected_project.borrow(), &projects);
        *self.selected_project.borrow_mut() = surviving;

        let choices = with_all(projects.iter().map(|p| (p.clone(), p.clone())));
        let selected = self.selected_project.borrow().clone();
        let view = self.clone();
        fill_filter_row(
            &self.project_bar,
            &choices,
            &selected,
            move |picked| {
                *view.selected_project.borrow_mut() = picked.to_string();
                view.rebuild_rows();
            },
        );

        // One project is no choice at all — the row would be "All | srcnet",
        // both showing the same list. Hidden until it can actually narrow
        // something.
        self.project_scroll.set_visible(projects.len() > 1);
    }

    /// The ids of the images the user added themselves, for one pass of work.
    ///
    /// Both callers want the same set and neither wants it per row: asking the
    /// store per row took its lock once for every row drawn, to answer a
    /// one-word question fifty times.
    fn my_image_ids(&self) -> std::collections::HashSet<String> {
        self.services
            .user_images
            .list()
            .into_iter()
            .map(|i| i.id)
            .collect()
    }

    /// The distinct projects among the images the current type selection shows.
    ///
    /// Alphabetical, not by size. The biggest two (`srcnet` and `skaha`, 62 and
    /// 61 images) would lead a size-ordered row today and swap places the week
    /// one of them publishes a tag — a filter bar whose buttons move is one the
    /// user has to re-read every time.
    fn projects_for_selected_type(&self) -> Vec<String> {
        let selected = self.selected_type.borrow().clone();
        let images = self.images.borrow();
        let mut seen = std::collections::HashSet::new();
        let mut out: Vec<String> = images
            .iter()
            .filter(|img| in_type_group(&img.types, &selected))
            .filter(|img| !img.project.is_empty())
            .filter(|img| seen.insert(img.project.as_str()))
            .map(|img| img.project.clone())
            .collect();
        out.sort();
        out
    }

    // ── Row list ───────────────────────────────────────────────────────────

    /// Build every row in the catalogue, once, ordered discovered → failed →
    /// never and then by image id.
    ///
    /// Called when the shown set changes: a catalogue refresh, a type toggle,
    /// or a probe starting or finishing.
    fn rebuild_rows(self: &Rc<Self>) {
        while let Some(child) = self.list_box.first_child() {
            self.list_box.remove(&child);
        }

        let selected = self.selected_type.borrow().clone();
        // One locked pass over the cache for the whole rebuild. This used to
        // call `store.get()` — which deep-copies an outcome, package lists and
        // all — once here, again inside `build_row`, and a third time in
        // `update_subtitle`: three full manifest copies per image to render a
        // status glyph and a one-line subtitle.
        let summaries = self.services.image_manifests.row_summaries();

        // Filtered HERE rather than by a `set_filter_func` over every image in
        // the catalogue. Filtering by visibility is cheaper per keystroke, but
        // it keeps a widget alive for every image whether shown or not, and
        // that cost is paid on every layout pass forever — 38ms at 368 images
        // against 5ms at fifty. Rebuilding a short list is the cheaper trade.
        let project = self.selected_project.borrow().clone();
        let images = self.images.borrow();
        let mut rows: Vec<(DiscoveryStatus, &ParsedImage)> = images
            .iter()
            .filter(|img| in_type_group(&img.types, &selected))
            .filter(|img| in_project(&img.project, &project))
            .map(|img| (status_of(summaries.get(&img.id)), img))
            .collect();
        rows.sort_by(|a, b| {
            status_order(a.0)
                .cmp(&status_order(b.0))
                .then_with(|| a.1.id.cmp(&b.1.id))
        });

        // One snapshot for the whole rebuild, like the summaries above.
        let mine = self.my_image_ids();

        let matched = rows.len();
        for (_, img) in rows.iter().take(MAX_ROWS) {
            // Asked of the coordinator, which is the thing that actually runs
            // probes. A second copy here could — and did — disagree with it.
            let is_running = self.services.image_discovery.is_probing(&img.id);
            self.list_box.append(&self.build_row(
                img,
                is_running,
                summaries.get(&img.id),
                mine.contains(&img.id),
            ));
        }

        self.update_subtitle(matched);
    }

    fn build_row(
        self: &Rc<Self>,
        img: &ParsedImage,
        is_running: bool,
        summary: Option<&RowSummary>,
        is_mine: bool,
    ) -> adw::ActionRow {
        let status = status_of(summary);

        let row = adw::ActionRow::new();
        row.set_use_markup(false);
        row.set_title(&img.display_name);
        row.set_subtitle(&if is_running {
            crate::tr_en!("Inspecting…").to_string()
        } else {
            meta_line(summary, &img.id, &chrono::Utc::now().to_rfc3339())
        });
        row.set_subtitle_lines(0);

        // Status glyph prefix.
        let icon = gtk::Image::from_icon_name(status_icon(status, is_running));
        if let Some(css) = status_icon_css(status, is_running) {
            icon.add_css_class(css);
        }
        icon.set_tooltip_text(Some(status_tooltip(status)));
        row.add_prefix(&icon);

        // Inspect button.
        // Label stays "Inspect" in both states on purpose. A button that
        // renames itself changes width, and this one sits in a column beside
        // "Use this image" — the discovery dialog's button did exactly that and
        // knocked every busy row's primary action out of line.
        let inspect_btn = gtk::Button::with_label(crate::tr_en!("Inspect"));
        inspect_btn.add_css_class("flat");
        inspect_btn.set_valign(gtk::Align::Center);
        // Pinned, because the label is replaced by a spinner while a probe
        // runs. Unpinned, the button would shrink to the spinner and drag
        // "Use this image" left for exactly the rows that are busy — the same
        // way the discovery dialog's button used to when it relabelled itself.
        inspect_btn.set_size_request(INSPECT_BTN_WIDTH, -1);
        let already = status == DiscoveryStatus::Discovered;
        inspect_btn.set_tooltip_text(Some(if already {
            crate::tr_en!("Inspect again — the image may have been rebuilt under the same tag")
        } else {
            crate::tr_en!("Run a probe job to list this image's packages")
        }));
        if is_running {
            // A rebuilt row must show the probe that is still going: without
            // this the list redraws a fresh, enabled "Inspect" over running
            // work, which is indistinguishable from a button that did nothing.
            crate::ui::busy::render_busy(&inspect_btn);
        }
        {
            let view = self.clone();
            let id = img.id.clone();
            inspect_btn.connect_clicked(move |_| {
                // FORCED when the image already has a manifest. Without this the
                // coordinator short-circuits on the cached success and returns
                // instantly: the row flashed "Inspecting…" and settled back
                // showing the same thing, which is indistinguishable from a
                // button that does nothing. A successful manifest never expires,
                // so pressing Inspect on a discovered image can only mean
                // "look again".
                view.clone().start_inspect(id.clone(), already);
            });
        }
        row.add_suffix(&inspect_btn);

        // Use-this-image button. Deliberately a regular (non-suggested) button:
        // "suggested-action" is reserved for a single primary call-to-action
        // per view (the Launch button in the launch form), not repeated on
        // every row here.
        let use_btn = gtk::Button::with_label(crate::tr_en!("Use this image"));
        use_btn.set_valign(gtk::Align::Center);
        {
            let on_use = self.on_use_image.clone();
            let id = img.id.clone();
            use_btn.connect_clicked(move |_| {
                if let Some(cb) = on_use.borrow().as_ref() {
                    cb(id.clone());
                }
            });
        }
        row.add_suffix(&use_btn);

        // The row itself opens what is inside the image. A fourth suffix button
        // would crowd a row that already carries three, and GNOME's own lists
        // put "show me this thing" on the row rather than on a control — the
        // buttons keep their own clicks, so the two do not compete.
        row.set_activatable(true);
        row.set_tooltip_text(Some(crate::tr_en!(
            "Show the packages and OS found inside this image"
        )));
        {
            let view = self.clone();
            let id = img.id.clone();
            row.connect_activated(move |row| {
                crate::ui::image_detail_dialog::show_image_detail_dialog(
                    row,
                    view.services.clone(),
                    &id,
                );
            });
        }

        // An image the user added themselves can be taken out again from where
        // they see it. Sending them back through the registry browser to undo
        // something they are looking at is the kind of detour that leaves stale
        // entries in a list forever.
        if is_mine {
            let remove_btn = gtk::Button::from_icon_name("user-trash-symbolic");
            remove_btn.add_css_class("flat");
            remove_btn.set_valign(gtk::Align::Center);
            remove_btn.set_tooltip_text(Some(crate::tr_en!(
                "Remove this image from your list (it stays in the registry)"
            )));
            let view = self.clone();
            let id = img.id.clone();
            remove_btn.connect_clicked(move |_| {
                view.clone().remove_user_image(id.clone());
            });
            row.add_suffix(&remove_btn);
        }

        row
    }

    /// Drop one of the user's own images and redraw.
    ///
    /// A full `refresh`, not a `rebuild_rows`: the row's absence has to come
    /// from the catalogue being rebuilt without it, and the catalogue is what
    /// merges the two sources.
    fn remove_user_image(self: Rc<Self>, image_id: String) {
        match self.services.user_images.remove(&image_id) {
            Ok(()) => {
                self.services
                    .toast
                    .toast(crate::tr_fmt!("Removed {}", image_id));
                glib::spawn_future_local(async move {
                    self.refresh().await;
                });
            }
            Err(e) => {
                self.services
                    .toast
                    .toast(crate::tr_fmt!("Could not save your image list: {}", e));
            }
        }
    }

    /// Kick off (or refresh) a probe for `image_id`, marking the row running,
    /// then refresh the row when the coordinator returns.
    fn start_inspect(self: &Rc<Self>, image_id: String, force: bool) {
        // Claimed HERE, on this thread, before anything redraws.
        //
        // `discover_image` claims inside its own future, which does not begin
        // until the runtime polls it — so the redraw below still saw the image
        // as idle, drew an ordinary enabled button, and Inspect looked dead
        // until the probe finished minutes later.
        let Some(guard) = self.services.image_discovery.claim(&image_id) else {
            return; // a probe for this image is already running
        };
        self.rebuild_rows();

        let view = self.clone();
        let services = self.services.clone();
        let coordinator = self.services.image_discovery.clone();
        glib::spawn_future_local(async move {
            let svc = services.clone();
            let id = image_id.clone();
            let outcome = services
                .spawn(async move { coordinator.discover_claimed(&svc, &id, force, guard).await })
                .await;
            view.rebuild_rows();

            // Say what happened. The outcome used to be dropped on the floor,
            // so a probe that could not even be submitted — not signed in, no
            // registry credentials, another probe already running for this
            // image — looked exactly like one that had not been asked for.
            if let DiscoveryOutcome::Failure {
                category, message, ..
            } = outcome
            {
                services.toast.toast(crate::tr_fmt!(
                    "Could not inspect {}: {}",
                    image_id,
                    crate::helpers::discovery_formatting::failure_summary(&category, &message)
                ));
            }
        });
    }

    /// Open the faceted find-by-package dialog. A committed pick fires
    /// `on_use_image` and rebuilds the list (the dialog may have probed images).
    fn open_find_dialog(self: Rc<Self>) {
        let view = self.clone();
        let on_pick: Rc<dyn Fn(String)> = Rc::new(move |image_id: String| {
            if let Some(cb) = view.on_use_image.borrow().as_ref() {
                cb(image_id);
            }
            view.rebuild_rows();
        });
        show_image_discovery_dialog(self.widget(), self.services.clone(), on_pick);
    }

    /// Open the registry browser.
    ///
    /// A full `refresh` on change, not a `rebuild_rows`: the catalogue this
    /// widget holds is the platform's list merged with the user's, so an added
    /// image is not in it yet and re-rendering what we have would show nothing
    /// new. The platform half comes from a five-minute cache, so this is not a
    /// round trip in the common case.
    fn open_registry_browser(self: Rc<Self>) {
        let view = self.clone();
        let on_changed: Rc<dyn Fn()> = Rc::new(move || {
            let view = view.clone();
            glib::spawn_future_local(async move {
                view.refresh().await;
            });
        });
        show_registry_browser_dialog(self.widget(), self.services.clone(), on_changed);
    }

    /// The caption above the list. `matched` is how many images the current
    /// type filter selects, which may be more than the list shows.
    fn update_subtitle(self: &Rc<Self>, matched: usize) {
        if self.images.borrow().is_empty() {
            self.subtitle.set_text("");
            return;
        }
        // Counted over the rows actually on screen. This counted the WHOLE
        // catalogue while the list showed one session type, so the caption sat
        // 6px above a list of twelve notebooks reading "Discovered 3 of 58" —
        // and that gap exists precisely to make it read as describing them.
        let selected = self.selected_type.borrow().clone();
        let project = self.selected_project.borrow().clone();
        let images = self.images.borrow();
        let shown = || {
            images
                .iter()
                .filter(|i| in_type_group(&i.types, &selected))
                .filter(|i| in_project(&i.project, &project))
        };
        // Same single snapshot the rows use.
        let summaries = self.services.image_manifests.row_summaries();
        let discovered = shown()
            .filter(|i| summaries.get(&i.id).map(|s| s.discovered).unwrap_or(false))
            .count();

        let mut text = crate::tr_fmt!("Discovered {} of {} images", discovered, matched);
        if matched > MAX_ROWS {
            // Say so rather than silently truncating: a list that stops at
            // fifty without mentioning it reads as a catalogue that stops at
            // fifty.
            text.push_str(&crate::tr_fmt!(
                " · showing the first {}, search to narrow",
                MAX_ROWS
            ));
        }
        self.subtitle.set_text(&text);
    }
}

// ---------------------------------------------------------------------------
// Pure presentation helpers (mirror CanfarImageRow / ApplyStatus / StatusOrder)
// ---------------------------------------------------------------------------

/// Whether any of `types` belongs to the selected group; empty selection is all.
///
/// Grouped, so the Desktop filter shows `desktop` and `desktop-app` alike.
/// A linked toggle row inside a horizontal scroller.
///
/// Both filter rows are this shape, and the scroller is not decoration: the
/// number of buttons is not ours to choose — seven session types today, and 21
/// projects — while the card spans two of the Portal's three homogeneous
/// columns. A plain Box makes every button part of the card's MINIMUM width, so
/// a wide row sets the minimum width of the entire grid, past the window, where
/// the page scroller (`hscrollbar_policy(Never)`, deliberately) clips the
/// right-hand column instead of scrolling it.
///
/// `propagate_natural_width` keeps the row at its natural size whenever there is
/// room, so this changes nothing until there is not.
fn filter_row() -> (gtk::Box, gtk::ScrolledWindow) {
    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    bar.add_css_class("linked");
    bar.set_halign(gtk::Align::Start);

    let scroll = gtk::ScrolledWindow::new();
    scroll.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Never);
    scroll.set_propagate_natural_width(true);
    scroll.set_propagate_natural_height(true);
    scroll.set_child(Some(&bar));
    (bar, scroll)
}

/// `choices` with an "All" entry in front, carrying the empty string.
///
/// Shared so both rows mean the same thing by "no filter" — `in_type_group` and
/// `in_project` both read an empty selection as "everything", and two rows each
/// inventing their own sentinel is how one of them ends up filtering on the
/// literal word "All".
fn with_all(choices: impl Iterator<Item = (String, String)>) -> Vec<(String, String)> {
    std::iter::once((String::new(), crate::tr_en!("All").to_string()))
        .chain(choices)
        .collect()
}

/// Fill `bar` with one linked toggle per choice, calling `on_pick` with the
/// value of whichever becomes active.
///
/// One builder for both rows. They were going to be the same twenty lines
/// twice — including the subtlety that a radio group emits `toggled` for the
/// button being switched OFF as well as the one coming on, which read naively
/// fires the handler twice per click with the wrong value on one of them.
fn fill_filter_row(
    bar: &gtk::Box,
    choices: &[(String, String)],
    selected: &str,
    on_pick: impl Fn(&str) + Clone + 'static,
) {
    while let Some(child) = bar.first_child() {
        bar.remove(&child);
    }

    let mut group: Option<gtk::ToggleButton> = None;
    for (value, label) in choices {
        let btn = gtk::ToggleButton::with_label(label);
        match &group {
            Some(first) => btn.set_group(Some(first)),
            None => group = Some(btn.clone()),
        }
        btn.set_active(value == selected);

        let on_pick = on_pick.clone();
        let value = value.clone();
        btn.connect_toggled(move |b| {
            // Only the newly-activated button; the paired deactivation of the
            // previous one is ignored.
            if b.is_active() {
                on_pick(&value);
            }
        });
        bar.append(&btn);
    }
}

/// The project selection that survives a change of session type.
///
/// Selecting CARTA when `uvickbos` is the chosen project leaves a card
/// filtered to a combination with nothing in it — an empty list whose cause is
/// a pressed button in a row that has just been rebuilt without it. Falling
/// back to All shows the CARTA images, which is what pressing CARTA meant.
fn surviving_project(selected: &str, projects: &[String]) -> String {
    if projects.iter().any(|p| p == selected) {
        selected.to_string()
    } else {
        String::new()
    }
}

/// Whether an image belongs to the selected project. Empty ⇒ every project.
fn in_project(project: &str, selected: &str) -> bool {
    selected.is_empty() || project == selected
}

/// Can this image be launched from the launch form, and so is it worth a probe?
///
/// The platform's catalogue is not a list of things you can start. Of the 365
/// images `/v1/image` returns, 77 carry `desktop-app` and nothing else — an
/// application published INSIDE a desktop session, not a session you can
/// launch. Every CASA tag back to 3.4.0 is one. They filled this card, they
/// filled the type filter, and inspecting one spends a probe job on an image no
/// tab in the launch form will ever offer.
///
/// [`LAUNCHABLE_SESSION_TYPES`] is the union of what the tabs accept — the
/// Standard tab's interactive types plus Headless — so it is exactly the
/// question "is this offered anywhere".
///
/// An image the user added themselves is kept whatever its labels say. They
/// went and found it, the Advanced tab launches it by reference, and hiding it
/// from the list they added it to would be the app overruling them.
///
/// [`LAUNCHABLE_SESSION_TYPES`]: crate::models::session::LAUNCHABLE_SESSION_TYPES
fn launchable_here(img: &ParsedImage, mine: &std::collections::HashSet<String>) -> bool {
    mine.contains(&img.id)
        || img.types.iter().any(|t| {
            crate::models::session::LAUNCHABLE_SESSION_TYPES.contains(&t.as_str())
        })
}

fn in_type_group(types: &[String], selected: &str) -> bool {
    selected.is_empty()
        || types
            .iter()
            .any(|t| crate::models::session::type_group(t) == selected)
}

/// Map a cached outcome summary to the 3-state row status.
fn status_of(summary: Option<&RowSummary>) -> DiscoveryStatus {
    match summary {
        Some(s) if s.discovered => DiscoveryStatus::Discovered,
        Some(_) => DiscoveryStatus::Failed,
        None => DiscoveryStatus::Unknown,
    }
}

/// Sort key: discovered first, then failed, then never inspected.
fn status_order(s: DiscoveryStatus) -> u8 {
    match s {
        DiscoveryStatus::Discovered => 0,
        DiscoveryStatus::Failed => 1,
        DiscoveryStatus::Unknown => 2,
    }
}

/// Row subtitle: "os_family os_version · N packages · inspected 3d ago" for a
/// discovered image, a one-line failure summary for a failed probe, else the
/// image id.
fn meta_line(summary: Option<&RowSummary>, image_id: &str, now: &str) -> String {
    match summary {
        Some(s) if s.discovered => {
            let os = match &s.os_family {
                Some(f) if !f.is_empty() && f != "unknown" => {
                    format!("{} {} · ", f, s.os_version.clone().unwrap_or_default())
                }
                _ => String::new(),
            };
            // WHEN, not just what. A successful manifest never expires and is
            // never re-checked, so an image rebuilt under the same tag keeps
            // showing packages it no longer has — silently, and for as long as
            // the cache lives. The age is what turns that from invisible into
            // a judgement the reader can make, next to the Inspect button that
            // acts on it.
            format!(
                "{os}{} packages · {}",
                s.package_count,
                crate::tr_fmt!("inspected {}", time_ago(&s.discovered_at, now))
            )
        }
        Some(s) => match &s.failure {
            Some((category, message)) => failure_summary(category, message),
            None => image_id.to_string(),
        },
        None => image_id.to_string(),
    }
}

fn status_icon(status: DiscoveryStatus, running: bool) -> &'static str {
    if running {
        return "content-loading-symbolic";
    }
    match status {
        DiscoveryStatus::Discovered => "emblem-ok-symbolic",
        DiscoveryStatus::Failed => "dialog-warning-symbolic",
        DiscoveryStatus::Unknown => "content-loading-symbolic",
    }
}

fn status_icon_css(status: DiscoveryStatus, running: bool) -> Option<&'static str> {
    if running {
        return None;
    }
    match status {
        DiscoveryStatus::Discovered => Some("success"),
        DiscoveryStatus::Failed => Some("warning"),
        DiscoveryStatus::Unknown => Some("dim-label"),
    }
}

fn status_tooltip(status: DiscoveryStatus) -> &'static str {
    match status {
        DiscoveryStatus::Discovered => crate::tr_en!("Discovered"),
        DiscoveryStatus::Failed => crate::tr_en!("Inspection failed"),
        DiscoveryStatus::Unknown => crate::tr_en!("Not inspected yet"),
    }
}

#[cfg(test)]
mod tests {
    use super::launchable_here;

    fn img(id: &str, types: &[&str]) -> crate::models::ParsedImage {
        crate::models::ParsedImage::from_raw(&crate::models::RawImage {
            id: id.to_string(),
            types: types.iter().map(|t| t.to_string()).collect(),
        })
    }

    fn nothing_added() -> std::collections::HashSet<String> {
        std::collections::HashSet::new()
    }

    #[test]
    fn narrowing_the_type_drops_a_project_that_went_with_it() {
        // Pick CARTA while filtered to `uvickbos`, which publishes no CARTA
        // image: the combination has nothing in it, and the card would go empty
        // with the cause sitting in a row that just rebuilt without that button.
        let carta_projects = vec!["cadc".to_string(), "skaha".to_string()];
        assert_eq!(super::surviving_project("uvickbos", &carta_projects), "");
    }

    #[test]
    fn a_project_that_is_still_there_is_kept() {
        // Narrowing the type should not throw away a narrowing the user already
        // made, when the two agree.
        let projects = vec!["cadc".to_string(), "skaha".to_string()];
        assert_eq!(super::surviving_project("skaha", &projects), "skaha");
    }

    #[test]
    fn all_survives_anything() {
        // The empty selection is never in `projects`, so a naive membership
        // test would reset it to itself — harmless here, but it must not become
        // a special case that gets it wrong.
        assert_eq!(super::surviving_project("", &[]), "");
        assert_eq!(super::surviving_project("", &["skaha".to_string()]), "");
    }

    #[test]
    fn all_means_every_project() {
        // Both rows read an empty selection as "no filter". A second sentinel
        // here — the literal word "All", say — would filter on a project of
        // that name and show nothing.
        assert!(super::in_project("skaha", ""));
        assert!(super::in_project("", ""));
        assert!(super::in_project("skaha", "skaha"));
        assert!(!super::in_project("skaha", "srcnet"));
    }

    #[test]
    fn the_project_row_leads_with_all() {
        // Without it the card is always filtered to one project, and there is
        // no way back to the whole list.
        let choices = super::with_all(
            ["skaha", "srcnet"]
                .into_iter()
                .map(|p| (p.to_string(), p.to_string())),
        );
        assert_eq!(choices.len(), 3);
        assert_eq!(choices[0].0, "", "the first entry is not the no-filter one");
    }

    #[test]
    fn a_desktop_app_is_not_something_you_can_launch() {
        // 77 of the platform's 365 images carry `desktop-app` and nothing else —
        // an application published inside a desktop session, not a session. They
        // filled this card and the type filter, and every Inspect on one spent a
        // probe job on an image no launch tab offers. Every CASA tag back to
        // 3.4.0 is one of these.
        assert!(!launchable_here(
            &img("images.canfar.net/casa-4/casa:4.2.0", &["desktop-app"]),
            &nothing_added()
        ));
    }

    #[test]
    fn everything_a_launch_tab_offers_stays() {
        // The Standard tab's interactive types plus Headless. Losing any of
        // these would hide an image the user can actually start.
        for ty in crate::models::session::LAUNCHABLE_SESSION_TYPES {
            assert!(
                launchable_here(&img("h/p/n:1", &[ty]), &nothing_added()),
                "`{ty}` is launchable but the card would hide it"
            );
        }
    }

    #[test]
    fn one_launchable_type_is_enough() {
        // The common shape: a CASA image that is both a desktop-app and
        // headless is launchable as a batch job, so it belongs here.
        assert!(launchable_here(
            &img("h/casa-6/casa:6.5", &["desktop-app", "headless"]),
            &nothing_added()
        ));
    }

    #[test]
    fn an_image_the_user_added_is_kept_whatever_its_labels_say() {
        // They went and found it in the registry, and the Advanced tab launches
        // it by reference. Hiding it from the list they added it to would be the
        // app overruling them — and a registry image often carries no
        // session-type label at all.
        let mine: std::collections::HashSet<String> =
            std::iter::once("h/me/mine:1".to_string()).collect();
        assert!(launchable_here(&img("h/me/mine:1", &[]), &mine));
        // ...but an unlabelled image nobody added is still not launchable.
        assert!(!launchable_here(&img("h/other/x:1", &[]), &mine));
    }

    use super::*;
    use crate::models::image_manifest::{ImageManifest, LastOutcome};

    const AT: &str = "2026-07-07T00:00:00Z";

    /// The rows render from a `RowSummary`, but the thing that really exists is
    /// a `LastOutcome` on disk — so the tests still build one of those and
    /// summarise it, rather than hand-rolling the summary the code under test
    /// would have produced.
    fn summary(o: &LastOutcome) -> RowSummary {
        RowSummary::of(o)
    }

    fn discovered_outcome(os: Option<&str>, ver: Option<&str>, python: &[&str]) -> LastOutcome {
        let m = ImageManifest {
            image_id: "img:1".to_string(),
            os_family: os.map(|s| s.to_string()),
            os_version: ver.map(|s| s.to_string()),
            python: python.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        };
        LastOutcome::success(m, AT)
    }

    const SOURCE: &str = include_str!("canfar_images.rs");

    #[test]
    fn inspect_claims_the_probe_before_it_redraws() {
        // The defect this prevents, exactly: `discover_image` claims the
        // in-flight slot inside its OWN future, which the runtime has not
        // polled when the list redraws. So the redraw saw the image as idle,
        // drew an ordinary enabled "Inspect", and the button looked dead until
        // the probe finished minutes later.
        //
        // Claim on this thread, THEN redraw.
        let code = crate::testing::without_comments(crate::testing::code(SOURCE));
        let at = code
            .find("fn start_inspect")
            .expect("start_inspect is gone");
        let body = &code[at..];
        let end = body.find("\n    }\n").unwrap_or(body.len());
        let body = &body[..end];

        let claim = body.find("image_discovery.claim(").expect(
            "Inspect no longer claims the probe slot, so nothing can \
                     show the row as busy",
        );
        let redraw = body
            .find("self.rebuild_rows()")
            .expect("Inspect no longer redraws, so the row never changes");
        assert!(
            claim < redraw,
            "the list redraws before the slot is claimed, so the row is drawn \
             idle and the button looks dead"
        );
        assert!(
            body.contains("discover_claimed("),
            "the probe re-claims a slot the caller already holds, which would \
             report itself Busy"
        );
    }

    #[test]
    fn a_running_row_shows_its_button_as_working() {
        // Disabling alone is not enough — a greyed button next to an identical
        // enabled one reads as broken rather than busy. The user reported it
        // "stays saying Inspect, no disabled state".
        let code = crate::testing::without_comments(crate::testing::code(SOURCE));
        assert!(
            code.contains("render_busy(&inspect_btn)"),
            "a row with a probe running draws an ordinary Inspect button"
        );
        // And the button is pinned, or swapping its label for a spinner would
        // shrink it and drag the row's other action leftwards.
        assert!(
            code.contains("inspect_btn.set_size_request(INSPECT_BTN_WIDTH"),
            "the Inspect button is unpinned, so it changes width while working"
        );
    }

    #[test]
    fn status_of_maps_three_states() {
        assert_eq!(status_of(None), DiscoveryStatus::Unknown);
        assert_eq!(
            status_of(Some(&summary(&discovered_outcome(
                Some("ubuntu"),
                Some("22.04"),
                &["numpy"]
            )))),
            DiscoveryStatus::Discovered
        );
        let failed = LastOutcome::failure("img:1", "JobTimedOut", "timed out", None, AT);
        assert_eq!(status_of(Some(&summary(&failed))), DiscoveryStatus::Failed);
    }

    #[test]
    fn status_order_ranks_discovered_first() {
        assert!(status_order(DiscoveryStatus::Discovered) < status_order(DiscoveryStatus::Failed));
        assert!(status_order(DiscoveryStatus::Failed) < status_order(DiscoveryStatus::Unknown));
    }

    #[test]
    fn rows_sort_discovered_then_failed_then_unknown_then_by_id() {
        let mut rows = vec![
            (DiscoveryStatus::Unknown, "z"),
            (DiscoveryStatus::Discovered, "b"),
            (DiscoveryStatus::Failed, "a"),
            (DiscoveryStatus::Discovered, "a"),
        ];
        rows.sort_by(|x, y| {
            status_order(x.0)
                .cmp(&status_order(y.0))
                .then_with(|| x.1.cmp(y.1))
        });
        assert_eq!(
            rows,
            vec![
                (DiscoveryStatus::Discovered, "a"),
                (DiscoveryStatus::Discovered, "b"),
                (DiscoveryStatus::Failed, "a"),
                (DiscoveryStatus::Unknown, "z"),
            ]
        );
    }

    /// A clock five minutes after the outcomes below were recorded, so the
    /// relative age is stable rather than depending on when the suite runs.
    const NOW: &str = "2026-07-07T00:05:00Z";

    #[test]
    fn meta_line_discovered_includes_os_count_and_age() {
        // The age is the point: a successful manifest never expires and is
        // never re-checked, so an image rebuilt under the same tag keeps
        // showing packages it no longer has. Without a date on the row there
        // is nothing to tell the reader that.
        let o = discovered_outcome(Some("ubuntu"), Some("22.04"), &["numpy", "scipy"]);
        assert_eq!(
            meta_line(Some(&summary(&o)), "img:1", NOW),
            "ubuntu 22.04 · 2 packages · inspected 5m ago"
        );
    }

    #[test]
    fn meta_line_discovered_hides_unknown_os() {
        let o = discovered_outcome(Some("unknown"), None, &["numpy"]);
        assert_eq!(
            meta_line(Some(&summary(&o)), "img:1", NOW),
            "1 packages · inspected 5m ago"
        );
        let none_os = discovered_outcome(None, None, &[]);
        assert_eq!(
            meta_line(Some(&summary(&none_os)), "img:1", NOW),
            "0 packages · inspected 5m ago"
        );
    }

    #[test]
    fn meta_line_failed_summarises_rather_than_dumping_the_logs() {
        // A failure message now carries the tail of the job's logs and events.
        // Dropping the whole thing into a subtitle — which is what this did —
        // makes a row hundreds of lines tall. The full text is one click away
        // in the detail pane.
        let long = "Manifest fetch failed: no JSON in the logs\n\n\
                    --- job logs ---\nline\nline\nline";
        let with_msg = LastOutcome::failure("img:1", "JobTimedOut", long, None, AT);
        assert_eq!(
            meta_line(Some(&summary(&with_msg)), "img:1", NOW),
            "Timed out · Manifest fetch failed: no JSON in the logs"
        );

        let blank_msg = LastOutcome::failure("img:1", "JobTimedOut", "  ", None, AT);
        assert_eq!(
            meta_line(Some(&summary(&blank_msg)), "img:1", NOW),
            "Timed out"
        );
    }

    #[test]
    fn meta_line_never_shows_image_id() {
        assert_eq!(
            meta_line(None, "images.canfar.net/skaha/base:1.0", NOW),
            "images.canfar.net/skaha/base:1.0"
        );
    }

    #[test]
    fn status_icon_and_css_track_state() {
        assert_eq!(
            status_icon(DiscoveryStatus::Discovered, false),
            "emblem-ok-symbolic"
        );
        assert_eq!(
            status_icon(DiscoveryStatus::Failed, false),
            "dialog-warning-symbolic"
        );
        assert_eq!(
            status_icon(DiscoveryStatus::Discovered, true),
            "content-loading-symbolic"
        );
        assert_eq!(
            status_icon_css(DiscoveryStatus::Discovered, false),
            Some("success")
        );
        assert_eq!(
            status_icon_css(DiscoveryStatus::Failed, false),
            Some("warning")
        );
        assert_eq!(status_icon_css(DiscoveryStatus::Discovered, true), None);
    }
}

/// How often the manifest sync says how far it has got.
///
/// Paced at 400ms a file, ten files is about four seconds — often enough to
/// look alive, rare enough that the toasts do not stack.
const SYNC_PROGRESS_EVERY: usize = 10;

impl CanfarImagesView {
    /// Walk the user's ARC space for image manifests this machine has not seen.
    ///
    /// Awaited rather than fire-and-forget, so the caller knows when to
    /// re-render — the button that starts this also refreshes the list, and
    /// doing that before the import finished showed the old contents.
    ///
    /// Ran on every sign-in until 1.3.7. Nobody asked for it, it toasted its
    /// way through, and the per-image recovery already covers anything the user
    /// actually inspects.
    ///
    /// Worth having as a button, though: every manifest a probe has ever
    /// published lives in the user's VOSpace, and without this the local cache
    /// learns about one only when that exact image is inspected. A fresh
    /// install shows "Not inspected yet" against images that were inspected
    /// weeks ago on another machine, and this is how someone fixes that when
    /// they notice it.
    pub(crate) async fn sync_manifests_from_vospace(self: &Rc<Self>) {
        let task = crate::helpers::tasks::begin(
            crate::helpers::tasks::TaskKind::Sync,
            crate::tr_en!("Check images"),
        );
        let services = Arc::clone(&self.services);
        let coordinator = Arc::clone(&services.image_discovery);
        let toast = services.toast.clone();
        let svc = Arc::clone(&services);

        // Onto the Tokio runtime, not the GTK loop: the sync does HTTP and
        // sleeps, both of which need a reactor, and running it on the main loop
        // panicked with "there is no reactor running". `ToastNotifier` is
        // documented as safe to call from any thread, which is what lets the
        // progress callback go over with it.
        let _ = services
            .spawn(async move {
                // Toasts are fire-and-forget with a five-second life, so
                // progress is reported occasionally rather than per file —
                // forty manifests would otherwise be forty toasts.
                let report = move |progress: SyncProgress| match progress {
                    SyncProgress::Started { total } => toast.toast(crate::tr_plural!(
                        total,
                        "Catching up on {} image manifest from CANFAR…",
                        "Catching up on {} image manifests from CANFAR…"
                    )),
                    SyncProgress::Advanced { done, total, .. }
                        if done % SYNC_PROGRESS_EVERY == 0 =>
                    {
                        toast.toast(crate::tr_fmt!("Image manifests: {} of {}", done, total))
                    }
                    SyncProgress::Advanced { .. } => {}
                    SyncProgress::Finished(summary) if summary.imported > 0 => {
                        toast.toast(crate::tr_plural!(
                            summary.imported,
                            "{} image manifest brought over from CANFAR",
                            "{} image manifests brought over from CANFAR"
                        ))
                    }
                    // Nothing new is the normal case on a machine that is
                    // already up to date. It used to stay silent, which was
                    // right for a background sync — but this one was ASKED for,
                    // and a button that appears to do nothing reads as broken.
                    SyncProgress::Finished(_) => {
                        toast.toast(crate::tr_en!("Image manifests are already up to date"))
                    }
                };

                match coordinator.sync_from_vospace(&svc, report).await {
                    Ok(summary) => {
                        // The counts, not just "done": the interesting outcome
                        // is usually "76 checked, 0 new, 27 unusable", which the
                        // toast has never said.
                        task.stage(crate::tr_fmt!(
                            "{} checked · {} new · {} unusable",
                            summary.scanned,
                            summary.imported,
                            summary.unusable
                        ));
                        task.succeed();
                    }
                    Err(e) => {
                        // Silence was right when nobody asked. Now somebody did.
                        toast_error(&svc, &e);
                        task.fail(e);
                    }
                }
            })
            .await;
    }
}

/// Report a sync failure to whoever pressed the button.
fn toast_error(services: &Arc<AppServices>, error: &str) {
    services
        .toast
        .toast(crate::tr_fmt!("Could not check CANFAR images: {}", error));
}

#[cfg(test)]
mod startup_tests {
    /// The manifest sync runs when asked, and only when asked.
    ///
    /// It used to run on every sign-in: a walk of every manifest in the user's
    /// ARC space at 400ms a file, toasting its progress, on the screen where
    /// someone is trying to start work. Nobody asked for it, and the per-image
    /// recovery already covers anything they actually inspect.
    ///
    /// A source scan because there is nothing to assert at runtime — the
    /// regression is a CALL somewhere it should not be, and the only evidence
    /// is that the call exists.
    #[test]
    fn the_manifest_sync_is_only_reachable_from_a_button() {
        let mut callers = Vec::new();
        for (path, text) in crate::testing::rust_sources() {
            let code = crate::testing::without_comments(crate::testing::code(&text));
            for (n, line) in code.lines().enumerate() {
                if !line.contains("sync_manifests_from_vospace(") {
                    continue;
                }
                // The definition itself is not a call.
                if line.contains("async fn sync_manifests_from_vospace") {
                    continue;
                }
                callers.push(format!("{}:{}", path.display(), n + 1));
            }
        }

        assert_eq!(
            callers.len(),
            1,
            "the manifest sync should have exactly one caller — the Check images \
             button. Found: {callers:#?}"
        );
        assert!(
            callers[0].contains("canfar_images.rs"),
            "the sync is called from outside the CANFAR Images card, which is \
             how it ended up running at sign-in: {callers:#?}"
        );
    }

    /// Signing in does not start it.
    ///
    /// Narrower and more direct than the count above: whatever else changes,
    /// the window that handles sign-in must not reach for this.
    #[test]
    fn signing_in_does_not_start_a_manifest_walk() {
        let main_window = include_str!("main_window.rs");
        let code = crate::testing::without_comments(crate::testing::code(main_window));
        for forbidden in ["sync_manifests_from_vospace", "sync_image_manifests"] {
            assert!(
                !code.contains(forbidden),
                "main_window calls {forbidden} — the sync is a button, not a \
                 sign-in step"
            );
        }
    }
}
