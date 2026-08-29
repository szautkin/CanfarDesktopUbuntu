//! Top-level FITS viewer widget.
//!
//! Owns an `adw::TabView` of FitsTabs and the control column beside it — the
//! same shape the cube viewer has, built from `ui::viewer_shell`. Switching tabs
//! synchronises every control in the column to the newly-active tab's state.

use crate::helpers::fits_loader;
use crate::helpers::fits_renderer::{ColorMap, Stretch};
use crate::models::fits_image::{HduInfo, WcsInfo};
use crate::state::AppServices;
use crate::ui::fits_canvas::{BlinkOverlay, FitsCanvas, SharedSky, SharedSkyRef};
use crate::ui::fits_coords_panel::FitsCoordsPanel;
use crate::ui::fits_header_panel::FitsHeaderPanel;
use crate::ui::fits_tab::FitsTab;
use crate::ui::viewer_shell;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use libadwaita as adw;
use serde_json::json;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

const ZOOM_PRESETS: &[(i32, &str)] = &[
    (25, "25%"),
    (50, "50%"),
    (75, "75%"),
    (100, "100%"),
    (150, "150%"),
    (200, "200%"),
    (300, "300%"),
    (400, "400%"),
    (500, "500%"),
    (800, "800%"),
    (1200, "1200%"),
];

/// An in-memory FITS sky-coordinate bookmark, keyed by `name`. Mirrors the
/// Windows `FitsBookmark` record but is process-local (not persisted).
#[derive(Clone, serde::Serialize)]
pub struct FitsBookmark {
    pub name: String,
    pub ra: f64,
    pub dec: f64,
    pub source_file: String,
}

/// Image A's viewport snapshotted before a blink reframes it:
/// `(tab, center_x, center_y, zoom)`. Restored verbatim when the blink stops,
/// so comparing two images never leaves the user somewhere they didn't choose.
type BlinkRestore = (Rc<FitsTab>, f64, f64, f64);

pub struct FitsViewer {
    widget: gtk::Box,
    tab_view: adw::TabView,
    /// Swaps between the empty state and the tab strip, so "no file open" is a
    /// state of the page rather than a tab that has to be removed.
    content_stack: gtk::Stack,
    tabs: Rc<RefCell<Vec<Rc<FitsTab>>>>,
    /// "Search here" in the CROSSHAIR section — the same action the coordinates
    /// panel offers, where a reader looks for it.
    search_here_btn: gtk::Button,
    /// Live MCP bookmarks (in-memory Vec on the viewer).
    bookmarks: RefCell<Vec<FitsBookmark>>,
    /// Cross-tab shared crosshair/hover state, linked by sky (RA/Dec).
    shared: SharedSkyRef,
    status_label: gtk::Label,
    blink_active: Rc<RefCell<bool>>,
    /// The canvas a cross-fade blink overlay currently lives on (image A).
    blink_canvas: Rc<RefCell<Option<Rc<FitsCanvas>>>>,
    /// Blink fade state.
    blink_paused: Rc<Cell<bool>>,
    blink_opacity: Rc<Cell<f64>>,
    blink_fading_in: Rc<Cell<bool>>,
    blink_interval_ms: Rc<Cell<u64>>,
    /// 0-based index of the tab to blink the active tab against.
    blink_target: Rc<Cell<usize>>,
    coords_panel: Rc<FitsCoordsPanel>,
    annotations_panel: Rc<crate::ui::annotations_panel::AnnotationsPanel>,
    /// The label editor currently open and the mark it belongs to, so a second
    /// one replaces it rather than stacking, and so it can follow that mark
    /// while it is dragged.
    open_label_editor: RefCell<Option<(String, gtk::Box)>>,
    /// On while the next click places a mark.
    draw_mode: gtk::ToggleButton,
    /// Which shape that click makes.
    draw_kind: gtk::DropDown,
    // Toolbar widgets (for tab-switch sync)
    stretch_combo: gtk::DropDown,
    colormap_combo: gtk::DropDown,
    min_scale: gtk::Scale,
    max_scale: gtk::Scale,
    zoom_combo: gtk::DropDown,
    /// Free-form zoom % entry (parses "NNN" / "NNN%" on activate).
    zoom_entry: gtk::Entry,
    north_up_btn: gtk::ToggleButton,
    /// The header/info section. The expander IS the state — there is no second
    /// toggle that could disagree with whether the section is open.
    header_panel: Rc<FitsHeaderPanel>,
    header_expander: gtk::Expander,
    coords_expander: gtk::Expander,
    /// Sky-linked crosshair toggle (default ON; also auto-enables North-Up).
    link_btn: gtk::ToggleButton,
    /// Cross-fade blink toggle.
    blink_btn: gtk::ToggleButton,
    /// Picks which other tab the active tab blinks against.
    blink_target_btn: gtk::MenuButton,
    /// Approximate-WCS warning banner (revealed per active tab).
    wcs_banner: adw::Banner,
    /// Extension (HDU) selector bar — hidden for single-image files.
    hdu_bar: gtk::Box,
    hdu_dropdown: gtk::DropDown,
    /// HDUs backing the dropdown (parallel to its string model).
    hdu_infos: Rc<RefCell<Vec<HduInfo>>>,
    /// Dropdown position of the currently-displayed image HDU (for revert).
    hdu_current_pos: Rc<RefCell<u32>>,
    /// Prevents feedback loops when syncing toolbar widgets on tab switch.
    suppress_sync: Rc<RefCell<bool>>,
    /// Guards the selected-page handler during an in-place HDU swap.
    suppress_page_switch: Rc<RefCell<bool>>,
    /// True while a page is being swapped for a rebuilt one.
    ///
    /// `close_page` is how a page is replaced as well as how it is closed, and
    /// the close handler cannot tell the two apart — it retains the tab out of
    /// the registry either way. During a swap that leaves `tabs` one shorter
    /// than the page list, and the re-registration below it silently does
    /// nothing, so the viewer keeps a page nobody owns and every tool answers
    /// "no FITS open".
    rebuilding_page: Rc<RefCell<bool>>,
    /// Persistent sync-zoom toggle (mirrors Windows `IsSyncZoomEnabled`): when on,
    /// every tab is re-zoomed to a shared angular field as it becomes active.
    sync_zoom_enabled: Rc<Cell<bool>>,
    /// The sync-zoom toolbar toggle. Held so MCP can flip it through the SAME
    /// handler a click fires, rather than setting the flag behind the button's
    /// back and leaving the UI showing the opposite state.
    sync_fov_btn: gtk::ToggleButton,
    /// Shared angular zoom in arcsec per screen pixel (mirrors Windows
    /// `SharedAngularZoom`), captured from the active tab and re-applied to each
    /// tab on activation. `0.0` = unset.
    shared_angular_zoom: Rc<Cell<f64>>,
    /// Image A's pre-blink viewport `(tab, center_x, center_y, zoom)`, snapshotted
    /// before a blink reframes it and restored on stop (mirrors `_blinkRestore`).
    blink_restore: RefCell<Option<BlinkRestore>>,
}

impl FitsViewer {
    pub fn new(_services: Arc<AppServices>) -> Rc<Self> {
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        widget.set_vexpand(true);
        widget.set_hexpand(true);

        // ── Toolbar: the file action and the status line, nothing else ──────
        //
        // Every display and view control lives in the control column on the
        // right, the shape the cube viewer already had. A horizontal bar cannot
        // label its controls, cannot group them, and runs out of width on a
        // laptop — which is exactly how eleven of these ended up hidden in a
        // popover. A column labels, groups and grows by one row.
        let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        toolbar.add_css_class("toolbar");

        let open_btn = gtk::Button::new();
        let open_content = adw::ButtonContent::new();
        open_content.set_icon_name("document-open-symbolic");
        open_content.set_label(crate::tr_en!("Open FITS"));
        open_btn.set_child(Some(&open_content));
        open_btn.add_css_class("suggested-action");
        open_btn.set_tooltip_text(Some(crate::tr_en!("Open FITS file")));
        toolbar.append(&open_btn);

        // ── Control column ──────────────────────────────────────────────────
        let (column, control_scroll) = viewer_shell::control_column();

        // One shape for every icon control in the column.
        let icon_toggle = |icon: &str, tooltip: &str| {
            let b = gtk::ToggleButton::new();
            b.set_icon_name(icon);
            b.add_css_class("flat");
            b.set_valign(gtk::Align::Center);
            b.set_tooltip_text(Some(tooltip));
            b
        };
        let icon_button = |icon: &str, tooltip: &str| {
            let b = gtk::Button::from_icon_name(icon);
            b.add_css_class("flat");
            b.set_valign(gtk::Align::Center);
            b.set_tooltip_text(Some(tooltip));
            b
        };

        // ── DISPLAY ─────────────────────────────────────────────────────────
        column.append(&viewer_shell::section_header(crate::tr_en!("DISPLAY")));

        let cmap_items = gtk::StringList::new(&[
            crate::tr_en!("Grayscale"),
            crate::tr_en!("Inverted"),
            crate::tr_en!("Heat"),
            crate::tr_en!("Viridis"),
            crate::tr_en!("Plasma"),
            crate::tr_en!("Inferno"),
            crate::tr_en!("Magma"),
            crate::tr_en!("CoolWarm"),
        ]);
        let colormap_combo = gtk::DropDown::new(Some(cmap_items), gtk::Expression::NONE);
        colormap_combo.set_selected(0);
        column.append(&viewer_shell::labeled(
            crate::tr_en!("Colormap"),
            &colormap_combo,
        ));

        let stretch_items = gtk::StringList::new(&[
            crate::tr_en!("Linear"),
            crate::tr_en!("Log"),
            crate::tr_en!("Sqrt"),
            crate::tr_en!("Squared"),
            crate::tr_en!("Asinh"),
            crate::tr_en!("Histogram Eq"),
        ]);
        let stretch_combo = gtk::DropDown::new(Some(stretch_items), gtk::Expression::NONE);
        stretch_combo.set_selected(0);
        column.append(&viewer_shell::labeled(
            crate::tr_en!("Stretch"),
            &stretch_combo,
        ));

        let min_scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 0.01);
        min_scale.set_draw_value(false);
        min_scale.set_hexpand(true);
        min_scale.set_tooltip_text(Some(crate::tr_en!(
            "Black point — pixels at or below render black"
        )));
        column.append(&viewer_shell::labeled(crate::tr_en!("Min cut"), &min_scale));

        let max_scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 0.01);
        max_scale.set_draw_value(false);
        max_scale.set_hexpand(true);
        max_scale.set_tooltip_text(Some(crate::tr_en!(
            "White point — pixels at or above render white"
        )));
        column.append(&viewer_shell::labeled(crate::tr_en!("Max cut"), &max_scale));

        let reset_btn = gtk::Button::with_label(crate::tr_en!("Reset stretch"));
        reset_btn.add_css_class("flat");
        reset_btn.set_halign(gtk::Align::End);
        reset_btn.set_tooltip_text(Some(crate::tr_en!(
            "Back to the automatic cut levels and Linear stretch"
        )));
        column.append(&reset_btn);

        // ── VIEW ────────────────────────────────────────────────────────────
        column.append(&viewer_shell::section_header(crate::tr_en!("VIEW")));

        let zoom_items =
            gtk::StringList::new(&ZOOM_PRESETS.iter().map(|(_, l)| *l).collect::<Vec<&str>>());
        let zoom_combo = gtk::DropDown::new(Some(zoom_items), gtk::Expression::NONE);
        zoom_combo.set_selected(3); // 100%
        let zoom_entry = gtk::Entry::new();
        zoom_entry.set_width_chars(5);
        zoom_entry.set_max_width_chars(6);
        zoom_entry.set_text("100");
        zoom_entry.set_tooltip_text(Some(crate::tr_en!("Type a zoom % and press Enter")));
        let zoom_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        zoom_box.add_css_class("linked");
        zoom_box.append(&zoom_combo);
        zoom_box.append(&zoom_entry);
        column.append(&viewer_shell::labeled(crate::tr_en!("Zoom"), &zoom_box));

        let north_up_btn = icon_toggle("go-up-symbolic", crate::tr_en!("Rotate so north is up"));
        column.append(&viewer_shell::labeled_row(
            crate::tr_en!("North up"),
            &north_up_btn,
        ));

        // The drawing controls are built here — the viewer owns them and reads
        // the picker at click time — but they live inside the Marks section,
        // beside the list of what they produce.
        let draw_mode = gtk::ToggleButton::new();
        draw_mode.set_icon_name("document-edit-symbolic");
        draw_mode.set_tooltip_text(Some(crate::tr_en!(
            "Draw a mark on the image. Click where you mean, Shift-drag to move the image, \
             Escape to stop."
        )));
        // Two shapes. A "callout" was a small circle with a leader, and every
        // shape has a leader now; a "text" was a label with nothing to point
        // at. Both kinds still exist in the model and over MCP — stored marks
        // and an agent's calls keep working — they are simply not choices a
        // person has to make here.
        let kind_items = gtk::StringList::new(&[crate::tr_en!("Circle"), crate::tr_en!("Box")]);
        let draw_kind = gtk::DropDown::new(Some(kind_items), gtk::Expression::NONE);
        draw_kind.set_selected(0);
        let draw_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        draw_box.append(&draw_mode);
        draw_box.append(&draw_kind);

        // ── CROSSHAIR ───────────────────────────────────────────────────────
        column.append(&viewer_shell::section_header(crate::tr_en!("CROSSHAIR")));

        let crosshair_hint =
            gtk::Label::new(Some(crate::tr_en!("Right-click the image to place it.")));
        crosshair_hint.add_css_class("caption");
        crosshair_hint.add_css_class("dim-label");
        crosshair_hint.set_wrap(true);
        crosshair_hint.set_xalign(0.0);
        column.append(&crosshair_hint);

        let copy_radec_btn = icon_button(
            "edit-copy-symbolic",
            crate::tr_en!("Copy crosshair RA/Dec to clipboard"),
        );
        let clear_crosshair_btn =
            icon_button("edit-clear-symbolic", crate::tr_en!("Clear crosshair"));
        let crosshair_actions = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        crosshair_actions.add_css_class("linked");
        crosshair_actions.set_halign(gtk::Align::Start);
        crosshair_actions.append(&copy_radec_btn);
        crosshair_actions.append(&clear_crosshair_btn);
        column.append(&crosshair_actions);

        // Search the archive at the crosshair. It has always existed, inside the
        // coordinates panel — which is closed by default, so unless you opened
        // that panel the feature did not appear to be there. The reference
        // offers it from the crosshair menu, which is where a reader looks.
        let search_here_btn = gtk::Button::new();
        let search_here_content = adw::ButtonContent::new();
        search_here_content.set_icon_name("system-search-symbolic");
        search_here_content.set_label(crate::tr_en!("Search here"));
        search_here_btn.set_child(Some(&search_here_content));
        search_here_btn.set_tooltip_text(Some(crate::tr_en!(
            "Search the CADC archive at the crosshair's RA/Dec"
        )));
        search_here_btn.set_sensitive(false);
        column.append(&search_here_btn);

        // ── HEADER & IMAGE INFO ─────────────────────────────────────────────
        // A section of the column, not a panel beside the image: one instance
        // for the viewer, refilled when you switch tabs.
        let header_panel = FitsHeaderPanel::new();
        let header_expander = gtk::Expander::new(Some(crate::tr_en!("Header & image info")));
        header_expander.set_child(Some(header_panel.widget()));
        column.append(&header_expander);

        // ── SAVED COORDINATES ───────────────────────────────────────────────
        let coords_panel = FitsCoordsPanel::new();
        let coords_expander = gtk::Expander::new(Some(crate::tr_en!("Saved coordinates")));
        coords_expander.set_child(Some(coords_panel.widget()));
        column.append(&coords_expander);

        // ── ANNOTATIONS ─────────────────────────────────────────────────────
        let annotations_panel = crate::ui::annotations_panel::AnnotationsPanel::new();
        annotations_panel.set_draw_controls(&draw_box);
        let annotations_expander = gtk::Expander::new(Some(crate::tr_en!("Marks")));
        annotations_expander.set_child(Some(annotations_panel.widget()));
        column.append(&annotations_expander);

        // ── COMPARE ─────────────────────────────────────────────────────────
        // Everything that acts across tabs, together.
        column.append(&viewer_shell::section_header(crate::tr_en!("COMPARE")));

        let blink_btn = icon_toggle(
            "media-playlist-repeat-symbolic",
            crate::tr_en!(
                "Cross-fade blink against another tab (Space pause · Left/Right show A/B · Esc stop)"
            ),
        );
        column.append(&viewer_shell::labeled_row(
            crate::tr_en!("Blink"),
            &blink_btn,
        ));

        let blink_target_btn = gtk::MenuButton::new();
        blink_target_btn.set_label(crate::tr_en!("vs…"));
        blink_target_btn.set_valign(gtk::Align::Center);
        blink_target_btn.set_tooltip_text(Some(crate::tr_en!("Choose the tab to blink against")));
        column.append(&viewer_shell::labeled_row(
            crate::tr_en!("Against"),
            &blink_target_btn,
        ));

        let blink_interval_scale =
            gtk::Scale::with_range(gtk::Orientation::Horizontal, 500.0, 5000.0, 100.0);
        blink_interval_scale.set_value(1500.0);
        blink_interval_scale.set_draw_value(true);
        blink_interval_scale.set_value_pos(gtk::PositionType::Right);
        blink_interval_scale.set_hexpand(true);
        blink_interval_scale.set_tooltip_text(Some(crate::tr_en!("Blink fade interval (ms)")));
        column.append(&viewer_shell::labeled(
            crate::tr_en!("Fade speed"),
            &blink_interval_scale,
        ));

        let link_btn = icon_toggle(
            "insert-link-symbolic",
            crate::tr_en!("Link crosshair across tabs by sky position (auto-enables North Up)"),
        );
        link_btn.set_active(true);
        column.append(&viewer_shell::labeled_row(
            crate::tr_en!("Link crosshair"),
            &link_btn,
        ));

        let sync_fov_btn = icon_toggle(
            "zoom-fit-best-symbolic",
            crate::tr_en!(
                "Sync zoom across tabs — match the current image's angular field (re-applied as you switch tabs)"
            ),
        );
        column.append(&viewer_shell::labeled_row(
            crate::tr_en!("Sync zoom"),
            &sync_fov_btn,
        ));

        // Spacer pushes the status caption to the trailing edge.
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        toolbar.append(&spacer);

        let status_label = gtk::Label::new(Some(crate::tr_en!("No file loaded")));
        status_label.add_css_class("dim-label");
        status_label.add_css_class("caption");
        toolbar.append(&status_label);

        widget.append(&toolbar);
        widget.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

        // ── Approximate-WCS warning banner ───────────────────────────────────
        let wcs_banner = adw::Banner::new(crate::tr_en!(
            "Approximate WCS — coordinates and alignment may be imprecise."
        ));
        wcs_banner.set_revealed(false);
        widget.append(&wcs_banner);

        // ── Extension (HDU) selector bar ─────────────────────────────────────
        let hdu_infos: Rc<RefCell<Vec<HduInfo>>> = Rc::new(RefCell::new(Vec::new()));
        let hdu_current_pos = Rc::new(RefCell::new(0u32));

        let hdu_bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        hdu_bar.set_margin_start(12);
        hdu_bar.set_margin_end(12);
        hdu_bar.set_margin_top(6);
        hdu_bar.set_margin_bottom(6);
        let hdu_bar_label = gtk::Label::new(Some(crate::tr_en!("Extension:")));
        hdu_bar.append(&hdu_bar_label);

        let hdu_dropdown =
            gtk::DropDown::new(Some(gtk::StringList::new(&[])), gtk::Expression::NONE);
        // Custom factory so non-image HDUs render dimmed + insensitive.
        let hdu_factory = gtk::SignalListItemFactory::new();
        hdu_factory.connect_setup(|_, item| {
            if let Some(li) = item.downcast_ref::<gtk::ListItem>() {
                let label = gtk::Label::new(None);
                label.set_xalign(0.0);
                li.set_child(Some(&label));
            }
        });
        {
            let infos = hdu_infos.clone();
            hdu_factory.connect_bind(move |_, item| {
                let Some(li) = item.downcast_ref::<gtk::ListItem>() else {
                    return;
                };
                let Some(label) = li.child().and_downcast::<gtk::Label>() else {
                    return;
                };
                let pos = li.position() as usize;
                let infos = infos.borrow();
                if let Some(h) = infos.get(pos) {
                    label.set_text(&h.label());
                    label.set_sensitive(h.is_image);
                    if h.is_image {
                        label.remove_css_class("dim-label");
                    } else {
                        label.add_css_class("dim-label");
                    }
                }
            });
        }
        hdu_dropdown.set_factory(Some(&hdu_factory));
        hdu_bar.append(&hdu_dropdown);
        hdu_bar.set_visible(false);
        widget.append(&hdu_bar);

        // ── Main area: the tab strip; the controls dock beside it ───────────
        let body = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        body.set_vexpand(true);
        body.set_hexpand(true);

        // The same tab machinery the cube host uses: an `adw::TabBar` over an
        // `adw::TabView`. It brings the close button, reordering and the tab
        // overview with it — the Notebook this replaces had a hand-rolled close
        // button that searched every page for its own label to find out which
        // tab it belonged to.
        let tab_view = adw::TabView::new();
        tab_view.set_vexpand(true);
        tab_view.set_hexpand(true);

        let tab_bar = adw::TabBar::new();
        tab_bar.set_view(Some(&tab_view));
        tab_bar.set_autohide(false);

        let tabs_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        tabs_box.set_vexpand(true);
        tabs_box.set_hexpand(true);
        tabs_box.append(&tab_bar);
        tabs_box.append(&tab_view);

        // Empty state (HIG: StatusPage with a primary call to action). A STACK
        // child, not a tab: as a tab it had to be removed when the first file
        // opened, which made page 0 mean two different things depending on how
        // much was open.
        let empty_open_btn = gtk::Button::with_label(crate::tr_en!("Open FITS…"));
        empty_open_btn.add_css_class("suggested-action");
        empty_open_btn.add_css_class("pill");
        empty_open_btn.set_halign(gtk::Align::Center);
        let empty_status = adw::StatusPage::new();
        empty_status.set_icon_name(Some("image-x-generic-symbolic"));
        empty_status.set_title(crate::tr_en!("No FITS File Open"));
        empty_status.set_description(Some(crate::tr_en!("Open a FITS file to get started")));
        empty_status.set_child(Some(&empty_open_btn));

        let content_stack = gtk::Stack::new();
        content_stack.set_vexpand(true);
        content_stack.set_hexpand(true);
        content_stack.add_named(&empty_status, Some("empty"));
        content_stack.add_named(&tabs_box, Some("tabs"));
        content_stack.set_visible_child_name("empty");

        body.append(&content_stack);

        // Image on the left, controls docked on the right — the cube viewer's
        // shape, from the same module.
        let shell = viewer_shell::shell(&body, &control_scroll);
        toolbar.append(&shell.sidebar_toggle);
        widget.append(&shell.widget);

        let viewer = Rc::new(FitsViewer {
            widget,
            tab_view,
            content_stack,
            tabs: Rc::new(RefCell::new(Vec::new())),
            bookmarks: RefCell::new(Vec::new()),
            shared: Rc::new(RefCell::new(SharedSky {
                linked: true,
                ..Default::default()
            })),
            status_label,
            blink_active: Rc::new(RefCell::new(false)),
            blink_canvas: Rc::new(RefCell::new(None)),
            blink_paused: Rc::new(Cell::new(false)),
            blink_opacity: Rc::new(Cell::new(0.0)),
            blink_fading_in: Rc::new(Cell::new(true)),
            blink_interval_ms: Rc::new(Cell::new(1500)),
            blink_target: Rc::new(Cell::new(1)),
            coords_panel,
            annotations_panel,
            open_label_editor: RefCell::new(None),
            draw_mode,
            draw_kind,
            search_here_btn,
            stretch_combo,
            colormap_combo,
            min_scale,
            max_scale,
            zoom_combo,
            zoom_entry,
            north_up_btn,
            header_panel,
            header_expander,
            coords_expander,
            link_btn,
            blink_btn,
            blink_target_btn,
            wcs_banner,
            hdu_bar,
            hdu_dropdown,
            hdu_infos,
            hdu_current_pos,
            suppress_sync: Rc::new(RefCell::new(false)),
            suppress_page_switch: Rc::new(RefCell::new(false)),
            rebuilding_page: Rc::new(RefCell::new(false)),
            sync_zoom_enabled: Rc::new(Cell::new(false)),
            sync_fov_btn: sync_fov_btn.clone(),
            shared_angular_zoom: Rc::new(Cell::new(0.0)),
            blink_restore: RefCell::new(None),
        });

        // ── Wire toolbar signals ─────────────────────────────────────────────
        {
            let v = viewer.clone();
            open_btn.connect_clicked(move |btn| {
                let v = v.clone();
                let btn = btn.clone();
                glib::spawn_future_local(async move {
                    v.open_file_dialog(&btn).await;
                });
            });
        }
        {
            let v = viewer.clone();
            empty_open_btn.connect_clicked(move |btn| {
                let v = v.clone();
                let btn = btn.clone();
                glib::spawn_future_local(async move {
                    v.open_file_dialog(&btn).await;
                });
            });
        }
        {
            let v = viewer.clone();
            viewer.stretch_combo.connect_selected_notify(move |combo| {
                if *v.suppress_sync.borrow() {
                    return;
                }
                if let Some(tab) = v.current_tab() {
                    tab.set_stretch(stretch_from_index(combo.selected()));
                }
            });
        }
        {
            let v = viewer.clone();
            viewer.colormap_combo.connect_selected_notify(move |combo| {
                if *v.suppress_sync.borrow() {
                    return;
                }
                if let Some(tab) = v.current_tab() {
                    tab.set_colormap(colormap_from_index(combo.selected()));
                }
            });
        }
        {
            let v = viewer.clone();
            viewer.min_scale.connect_value_changed(move |scale| {
                if *v.suppress_sync.borrow() {
                    return;
                }
                if let Some(tab) = v.current_tab() {
                    tab.set_vmin(scale.value());
                }
            });
        }
        {
            let v = viewer.clone();
            viewer.max_scale.connect_value_changed(move |scale| {
                if *v.suppress_sync.borrow() {
                    return;
                }
                if let Some(tab) = v.current_tab() {
                    tab.set_vmax(scale.value());
                }
            });
        }
        {
            let v = viewer.clone();
            reset_btn.connect_clicked(move |_| {
                if let Some(tab) = v.current_tab() {
                    tab.reset_stretch();
                    v.sync_controls_to_tab(&tab);
                }
            });
        }
        {
            let v = viewer.clone();
            viewer.north_up_btn.connect_toggled(move |btn| {
                if *v.suppress_sync.borrow() {
                    return;
                }
                if let Some(tab) = v.current_tab() {
                    tab.set_north_up(btn.is_active());
                }
            });
        }
        {
            let v = viewer.clone();
            viewer.blink_btn.connect_toggled(move |btn| {
                *v.blink_active.borrow_mut() = btn.is_active();
                if btn.is_active() {
                    v.start_blink();
                } else {
                    v.stop_blink();
                }
            });
        }
        // Sky-linked crosshair toggle (also auto-enables North Up when turned on).
        {
            let v = viewer.clone();
            viewer.link_btn.connect_toggled(move |btn| {
                v.on_link_toggled(btn.is_active());
            });
        }
        // Seed the shared linked state on startup (default ON).
        viewer.shared.borrow_mut().linked = viewer.link_btn.is_active();
        // Blink fade-interval slider.
        {
            let v = viewer.clone();
            blink_interval_scale.connect_value_changed(move |s| {
                v.blink_interval_ms.set(s.value().round().max(100.0) as u64);
            });
        }
        // Blink target-tab picker: rebuild the popover each time it opens.
        {
            let v = viewer.clone();
            viewer.blink_target_btn.set_create_popup_func(move |mb| {
                v.build_blink_target_popover(mb);
            });
        }
        // Persistent sync-zoom toggle (mirrors OnToggleSyncZoom): enabling it
        // captures the active tab's angular scale as the shared value; other tabs
        // adopt it lazily when they become active.
        {
            let v = viewer.clone();
            sync_fov_btn.connect_toggled(move |btn| {
                v.sync_zoom_enabled.set(btn.is_active());
                if btn.is_active() {
                    v.update_shared_angular_zoom();
                }
                v.update_wcs_banner();
            });
        }
        // Blink keys: Esc stop · Space pause · Left show-A · Right show-B.
        {
            let v = viewer.clone();
            let key = gtk::EventControllerKey::new();
            key.set_propagation_phase(gtk::PropagationPhase::Capture);
            key.connect_key_pressed(move |_, keyval, _, _| {
                if !*v.blink_active.borrow() {
                    return glib::Propagation::Proceed;
                }
                match keyval {
                    gtk::gdk::Key::Escape => {
                        v.blink_btn.set_active(false);
                        glib::Propagation::Stop
                    }
                    gtk::gdk::Key::space => {
                        v.blink_paused.set(!v.blink_paused.get());
                        glib::Propagation::Stop
                    }
                    gtk::gdk::Key::Left => {
                        v.blink_show(0.0);
                        glib::Propagation::Stop
                    }
                    gtk::gdk::Key::Right => {
                        v.blink_show(1.0);
                        glib::Propagation::Stop
                    }
                    _ => glib::Propagation::Proceed,
                }
            });
            viewer.widget.add_controller(key);
        }
        {
            let v = viewer.clone();
            viewer.zoom_combo.connect_selected_notify(move |combo| {
                if *v.suppress_sync.borrow() {
                    return;
                }
                let idx = combo.selected() as usize;
                if let Some((percent, _)) = ZOOM_PRESETS.get(idx) {
                    if let Some(tab) = v.current_tab() {
                        tab.set_zoom(*percent as f64 / 100.0);
                        v.zoom_entry.set_text(&percent.to_string());
                        if v.sync_zoom_enabled.get() {
                            v.update_shared_angular_zoom();
                        }
                    }
                }
            });
        }
        // Editable zoom entry: parse "NNN" / "NNN%" on Enter.
        {
            let v = viewer.clone();
            viewer.zoom_entry.connect_activate(move |entry| {
                if *v.suppress_sync.borrow() {
                    return;
                }
                let raw = entry.text();
                let cleaned = raw.trim().trim_end_matches('%').trim();
                if let Ok(pct) = cleaned.parse::<f64>() {
                    if pct > 0.0 {
                        if let Some(tab) = v.current_tab() {
                            tab.set_zoom(pct / 100.0);
                            // Reflect the (clamped) applied zoom back to both widgets.
                            v.sync_controls_to_tab(&tab);
                            if v.sync_zoom_enabled.get() {
                                v.update_shared_angular_zoom();
                            }
                        }
                    }
                }
            });
        }
        // Copy the crosshair RA/Dec to the clipboard.
        {
            let v = viewer.clone();
            viewer.search_here_btn.connect_clicked(move |_| {
                v.coords_panel.search_here();
            });
        }
        {
            let v = viewer.clone();
            copy_radec_btn.connect_clicked(move |btn| {
                let Some(tab) = v.current_tab() else {
                    return;
                };
                match tab.crosshair_world_pos() {
                    Some((ra, dec)) => {
                        let (ra_s, dec_s) = WcsInfo::format_coords(ra, dec);
                        let text = format!("{}  {}  ({:.6}, {:.6})", ra_s, dec_s, ra, dec);
                        btn.clipboard().set_text(&text);
                        v.status_label
                            .set_text(&crate::tr_fmt!("Copied  {}  {}", ra_s, dec_s));
                    }
                    None => v
                        .status_label
                        .set_text(crate::tr_en!("No crosshair with WCS to copy")),
                }
            });
        }
        // Clear the placed crosshair + hover marker on the active tab.
        {
            let v = viewer.clone();
            clear_crosshair_btn.connect_clicked(move |_| {
                if let Some(tab) = v.current_tab() {
                    tab.clear_crosshair();
                }
            });
        }
        // ── Drawing marks ───────────────────────────────────────────────────
        {
            let v = viewer.clone();
            viewer.draw_mode.connect_toggled(move |btn| {
                v.set_draw_mode(btn.is_active());
            });
        }
        {
            // Escape leaves draw mode — the way out of every mode in this app.
            let v = viewer.clone();
            let keys = gtk::EventControllerKey::new();
            keys.connect_key_pressed(move |_, key, _, _| {
                if key == gtk::gdk::Key::Escape {
                    if v.draw_mode.is_active() {
                        v.draw_mode.set_active(false);
                        return gtk::glib::Propagation::Stop;
                    }
                    // Escape is the way out of whichever one you are in.
                    if v.open_label_editor.borrow().is_some() {
                        v.leave_edit_mode();
                        return gtk::glib::Propagation::Stop;
                    }
                }
                gtk::glib::Propagation::Proceed
            });
            viewer.widget.add_controller(keys);
        }
        {
            let v = viewer.clone();
            viewer.annotations_panel.set_on_select(move |id| {
                if let Some(tab) = v.current_tab() {
                    // An empty id is the list saying "never mind" — the same
                    // row clicked twice.
                    let picked = (!id.is_empty()).then(|| id.to_string());
                    if picked.is_none() {
                        v.leave_edit_mode();
                        return;
                    }
                    // Picking a row points a mark OUT. It does not open it:
                    // the pencil does that, and grips on a mark nobody is
                    // editing invite a drag that means nothing.
                    tab.canvas().set_selected_annotation(picked);
                    v.close_label_editor();
                    tab.canvas().set_editing_annotation(None);
                    v.refresh_annotations_panel();
                }
            });
        }
        {
            let v = viewer.clone();
            viewer.annotations_panel.set_on_delete(move |id| {
                // An editor open on the mark being deleted would be left
                // pointing at a mark that no longer exists.
                v.leave_edit_mode();
                if let Some(tab) = v.current_tab() {
                    let canvas = tab.canvas();
                    let mut all = canvas.annotations();
                    all.retain(|a| a.id != id);
                    canvas.set_annotations(all);
                    v.persist_annotations(&tab);
                    v.refresh_annotations_panel();
                }
            });
        }
        {
            let v = viewer.clone();
            viewer.annotations_panel.set_on_edit(move |id| {
                // Select it first, so the mark being renamed is the one lit up
                // on the image.
                if let Some(tab) = v.current_tab() {
                    tab.canvas().set_selected_annotation(Some(id.to_string()));
                }
                v.refresh_annotations_panel();
                v.ask_for_text_at_leader(id);
            });
        }
        {
            let v = viewer.clone();
            viewer.annotations_panel.set_on_clear(move |_| {
                v.leave_edit_mode();
                if let Some(tab) = v.current_tab() {
                    tab.canvas().set_annotations(Vec::new());
                    v.persist_annotations(&tab);
                    v.refresh_annotations_panel();
                }
            });
        }

        // Extension selector: switch the displayed image HDU.
        {
            let v = viewer.clone();
            viewer.hdu_dropdown.connect_selected_notify(move |dd| {
                if *v.suppress_sync.borrow() {
                    return;
                }
                let pos = dd.selected();
                let selected = v.hdu_infos.borrow().get(pos as usize).cloned();
                let Some(info) = selected else {
                    return;
                };
                if !info.is_image {
                    // Non-image HDU can't be displayed — revert to the shown one.
                    let cur = *v.hdu_current_pos.borrow();
                    *v.suppress_sync.borrow_mut() = true;
                    v.hdu_dropdown.set_selected(cur);
                    *v.suppress_sync.borrow_mut() = false;
                    return;
                }
                // Off the signal, not inside it. `switch_hdu` tears down the
                // page and can rebuild this very dropdown's model; doing that
                // while GTK is still emitting `selected_notify` on it freed
                // objects the emission was still using. Deferring to the next
                // main-loop turn lets the emission unwind first.
                //
                // The set_hdu_selector fix means the model is usually left
                // alone now — this is the belt to that pair of braces, and it
                // also keeps the handler short, which is what a signal handler
                // that rebuilds half a page should be.
                let index = info.index;
                let v2 = v.clone();
                glib::idle_add_local_once(move || {
                    let _ = v2.switch_hdu(index);
                });
            });
        }

        // Tab switch → sync toolbar to the newly-active tab
        {
            let v = viewer.clone();
            viewer.tab_view.connect_selected_page_notify(move |_| {
                if *v.suppress_page_switch.borrow() {
                    return;
                }
                // The tab list is kept in page order, so the selected page's
                // position indexes it directly.
                let tab = v
                    .selected_index()
                    .and_then(|i| v.tabs.borrow().get(i).cloned());
                if let Some(tab) = tab {
                    // Apply the shared view FIRST (mirrors ApplySharedViewToActivePage):
                    // reposition the linked crosshair onto this tab's sky, then match the
                    // shared angular zoom, THEN sync the toolbar so the zoom % reflects it.
                    tab.apply_linked_crosshair();
                    v.apply_shared_view_to_active(&tab);
                    v.sync_controls_to_tab(&tab);
                    v.update_hdu_and_banner(&tab);
                    v.show_status_for(&tab);
                    v.refresh_annotations_panel();
                }
                // The active index changed, so the MCP snapshot is stale.
                v.publish_open_tabs();
            });
        }

        // Per-tab ✕ → drop the backing tab and finish the close. One handler
        // for every tab, instead of one closure per tab that had to find its own
        // page by comparing label widgets.
        {
            let tabs = viewer.tabs.clone();
            let rebuilding = viewer.rebuilding_page.clone();
            viewer.tab_view.connect_close_page(move |view, page| {
                // A rebuild closes the old page and inserts its replacement at
                // the same position. Dropping the tab here would leave the
                // registry one short, and the swap's re-registration indexes
                // into a vector that no longer has that slot — so it does
                // nothing, and the file becomes invisible to every tool while
                // its page is still on screen. The swap owns the registry.
                if !*rebuilding.borrow() {
                    let child = page.child();
                    tabs.borrow_mut()
                        .retain(|t| t.widget().clone().upcast::<gtk::Widget>() != child);
                }
                view.close_page_finish(page, true);
                // Closing a NON-active tab changes no selection, so the MCP
                // snapshot has to be republished here or it keeps the closed file.
                if !*rebuilding.borrow() {
                    publish_fits_tabs(view, &tabs);
                }
                glib::Propagation::Stop
            });
        }
        // Empty state is a page count, not a tab that gets removed.
        {
            let v = viewer.clone();
            viewer.tab_view.connect_n_pages_notify(move |tv| {
                let empty = tv.n_pages() == 0;
                v.content_stack
                    .set_visible_child_name(if empty { "empty" } else { "tabs" });
                if empty {
                    v.show_no_file_open();
                }
            });
        }

        // Wire coords panel → active tab
        {
            let v2 = viewer.clone();
            viewer.coords_panel.set_on_clear_crosshair(move || {
                // Unchoosing a bookmark takes away the crosshair it placed.
                if let Some(tab) = v2.current_tab() {
                    tab.canvas().set_crosshair(None);
                }
            });
        }
        {
            let v = viewer.clone();
            viewer.coords_panel.set_on_go_to(move |ra, dec| {
                if let Some(tab) = v.current_tab() {
                    tab.go_to_coord(ra, dec);
                    if let Some((px, py)) = tab.canvas().crosshair_pos() {
                        v.coords_panel
                            .set_current_crosshair(Some((px, py)), tab.data().wcs.as_ref());
                    }
                }
            });
        }
        {
            let v = viewer.clone();
            viewer
                .coords_panel
                .set_on_save_bookmark(move || -> Option<(f64, f64, String)> {
                    let tab = v.current_tab()?;
                    let (ra, dec) = tab.crosshair_world_pos()?;
                    Some((ra, dec, tab.source_file().to_string()))
                });
        }

        viewer
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.widget
    }

    /// Handle a live MCP viewer command (`op` + JSON `args`) against the FITS
    /// view. Runs on the GTK main thread; reads/mutates the active tab's live
    /// widgets and returns a JSON payload. Unknown ops return an error.
    pub async fn handle_viewer_command(
        self: &Rc<Self>,
        op: &str,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        match op {
            "switch_fits_tab" => {
                let index = crate::mcp::tools::arg(args, "index")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| "index is required".to_string())?
                    as usize;
                let count = self.tabs.borrow().len();
                if index >= count {
                    return Err(format!(
                        "no FITS tab at index {index} ({count} open) — list_open_tabs shows them"
                    ));
                }
                self.select_index(index);
                let tab = self
                    .current_tab()
                    .ok_or_else(|| "no FITS open".to_string())?;
                let state = self.fits_view_state(&tab);
                let active_name = state
                    .get("fileName")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                Ok(crate::mcp::tools::with_tab_switch_outcome(
                    state,
                    index,
                    count,
                    &active_name,
                ))
            }
            "blink_fits_tabs" => self.blink_command(args),
            "get_fits_view" => {
                let tab = self
                    .current_tab()
                    .ok_or_else(|| "no FITS open".to_string())?;
                Ok(self.fits_view_state(&tab))
            }

            // ── Annotations ─────────────────────────────────────────────
            "annotate_fits" => {
                use crate::models::annotation::{
                    Anchor, Annotation, AnnotationKind, Author, Extent,
                };
                let tab = self
                    .current_tab()
                    .ok_or_else(|| "no FITS open".to_string())?;
                let canvas = tab.canvas();

                let num = |k: &str| crate::mcp::tools::arg(args, k).and_then(|v| v.as_f64());
                let kind = crate::mcp::tools::arg(args, "kind")
                    .and_then(|v| v.as_str())
                    .map(|k| {
                        AnnotationKind::parse(k).ok_or_else(|| {
                            format!("'{k}' is not a kind — use rect, circle, callout or text")
                        })
                    })
                    .transpose()?
                    .unwrap_or(AnnotationKind::Circle);

                // Sky when given and usable; image pixels otherwise. Saying
                // which was used matters: an agent that meant sky and got
                // pixels would be pointing somewhere else entirely.
                let anchor = match (num("ra"), num("dec"), num("x"), num("y")) {
                    (Some(ra), Some(dec), _, _) => Anchor::Sky {
                        ra_deg: ra,
                        dec_deg: dec,
                    },
                    (_, _, Some(x), Some(y)) => Anchor::ImagePixel { x, y },
                    _ => {
                        return Err("give ra and dec (degrees), or x and y (image pixels), \
                                    for where to draw"
                            .to_string())
                    }
                };
                let text = crate::mcp::tools::arg(args, "text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                let mut mark = Annotation::new(kind, anchor, text, Author::Agent);
                mark = match num("radius") {
                    Some(r) => mark.with_extent(Extent::square(r)),
                    // No radius given: a size that is visible on THIS image,
                    // whatever its pixel scale.
                    None => mark.with_extent(canvas.default_extent_for(&anchor)),
                };
                mark.validate()?;

                let file = Self::annotation_target(&tab);
                let mut current = canvas.annotations();
                current.push(mark.clone());
                canvas.set_annotations(current.clone());
                // Persist, but a viewer that cannot write must still show the
                // mark it just drew.
                let saved = crate::helpers::annotation_store::save_for(&file, &current).is_ok();
                // An agent's mark is a mark: it belongs in the list the person
                // is looking at, not only on the image.
                self.refresh_annotations_panel();

                Ok(json!({
                    "id": mark.id,
                    "kind": mark.kind.as_str(),
                    "anchoredIn": mark.anchor.space(),
                    "text": mark.text,
                    "total": current.len(),
                    "persisted": saved,
                }))
            }

            "remove_annotation" => {
                let id = crate::mcp::tools::arg(args, "id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let tab = self
                    .current_tab()
                    .ok_or_else(|| "no FITS open".to_string())?;
                let canvas = tab.canvas();
                let mut current = canvas.annotations();
                let before = current.len();
                current.retain(|a| a.id != id);
                let removed = current.len() < before;
                if removed {
                    canvas.set_annotations(current.clone());
                    let file = Self::annotation_target(&tab);
                    let _ = crate::helpers::annotation_store::save_for(&file, &current);
                }
                self.refresh_annotations_panel();
                Ok(json!({ "removed": removed, "viewer": "fits", "remaining": current.len() }))
            }

            "clear_annotations" => {
                let tab = self
                    .current_tab()
                    .ok_or_else(|| "no FITS open".to_string())?;
                let canvas = tab.canvas();
                let removed = canvas.annotations().len();
                canvas.set_annotations(Vec::new());
                let file = Self::annotation_target(&tab);
                let _ = crate::helpers::annotation_store::save_for(&file, &[]);
                self.refresh_annotations_panel();
                Ok(json!({ "cleared": removed, "viewer": "fits" }))
            }

            "list_fits_annotations" => {
                let tab = self
                    .current_tab()
                    .ok_or_else(|| "no FITS open".to_string())?;
                let items: Vec<serde_json::Value> = tab
                    .canvas()
                    .annotations()
                    .iter()
                    .map(|a| {
                        json!({
                            "id": a.id,
                            "kind": a.kind.as_str(),
                            "text": a.text,
                            "anchoredIn": a.anchor.space(),
                            "anchor": a.anchor,
                            "author": a.author.as_str(),
                            "createdAt": a.created_at,
                        })
                    })
                    .collect();
                Ok(json!({ "count": items.len(), "annotations": items }))
            }

            // The working area as an image — what the user is looking at, not
            // a re-render of the file. The view state travels with it under
            // `view`, because an agent that will be asked to point at something
            // needs the frame the app shares, and pixels alone cannot carry it.
            "get_fits_image" => {
                let tab = self
                    .current_tab()
                    .ok_or_else(|| "no FITS open".to_string())?;
                let canvas = tab.canvas();
                let (view_w, view_h) = canvas.view_size();
                // Scaled to the agent-image budget, never up: a model reads a
                // capture at a few hundred pixels and pays for every one. A
                // viewer on a hidden tab has no allocation and gets a stated
                // default, rather than an agent being told to go and ask the
                // user to click something.
                let limits = crate::mcp::agent_image::ImageLimits::from_settings();
                let (w, h, on_screen) =
                    crate::mcp::agent_image::capture_size(view_w, view_h, limits);
                let png = canvas.capture_png(w, h)?;
                let image_base64 = {
                    use base64::Engine as _;
                    base64::engine::general_purpose::STANDARD.encode(&png)
                };
                Ok(json!({
                    "imageBase64": image_base64,
                    "imageMime": "image/png",
                    "width": w,
                    "height": h,
                    // The transform an annotation would be expressed through:
                    // this raster is `scale` times the on-screen view, and the
                    // view itself is described by `view`.
                    "viewWidth": view_w,
                    "viewHeight": view_h,
                    // False when the tab was not on screen, so the aspect ratio
                    // came from a default rather than from the viewport.
                    "viewportOnScreen": on_screen,
                    "scale": if view_w > 0 { f64::from(w) / f64::from(view_w) } else { 1.0 },
                    "view": self.fits_view_state(&tab),
                    "caption": format!(
                        "FITS working area — {}",
                        self.fits_view_state(&tab)
                            .get("fileName")
                            .and_then(|v| v.as_str())
                            .unwrap_or("active tab")
                    ),
                }))
            }
            // Closing a FITS tab. `close_active_tab` is app-level and was never
            // wired for the viewer, so it answered `closed: false` with no
            // reason for every attempt — and `switch_fits_tab` focuses the
            // viewer's tab without changing app-level focus, so the documented
            // "switch then close" sequence could not work either.
            "close_fits_tab" => {
                let count = self.tabs.borrow().len();
                if count == 0 {
                    return Err("no FITS open".to_string());
                }
                let index = match crate::mcp::tools::arg(args, "tabIndex")
                    .or_else(|| crate::mcp::tools::arg(args, "index"))
                    .and_then(|v| v.as_u64())
                {
                    Some(i) => {
                        let i = i as usize;
                        if i >= count {
                            return Err(format!(
                                "tab {i} is out of range — {count} FITS tab(s) are open"
                            ));
                        }
                        i
                    }
                    // No index: the one the other FITS tools act on.
                    None => self
                        .selected_index()
                        .ok_or_else(|| "no FITS tab is active".to_string())?,
                };

                let closed_file = self.tabs.borrow()[index].source_file().to_string();
                let page = {
                    let tab = self.tabs.borrow()[index].clone();
                    self.tab_view.page(tab.widget())
                };
                // The close handler owns the registry; it removes the tab and
                // republishes the snapshot.
                self.tab_view.close_page(&page);

                let remaining = self.tabs.borrow().len();
                Ok(json!({
                    "closed": true,
                    "closedIndex": index,
                    "closedFile": closed_file,
                    "tabCount": remaining,
                }))
            }
            "set_fits_view" => {
                let mut tab = self
                    .current_tab()
                    .ok_or_else(|| "no FITS open".to_string())?;

                if crate::mcp::tools::arg(args, "reset")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                {
                    tab.reset_stretch();
                    tab.reset_view();
                }
                if let Some(s) = crate::mcp::tools::arg(args, "stretch").and_then(|v| v.as_str()) {
                    tab.set_stretch(
                        stretch_from_str(s).ok_or_else(|| format!("unknown stretch '{s}'"))?,
                    );
                }
                if let Some(c) = crate::mcp::tools::arg(args, "colormap").and_then(|v| v.as_str()) {
                    tab.set_colormap(
                        colormap_from_str(c).ok_or_else(|| format!("unknown colormap '{c}'"))?,
                    );
                }
                if let Some(v) = crate::mcp::tools::arg(args, "minCut").and_then(|v| v.as_f64()) {
                    tab.set_vmin(v);
                }
                if let Some(v) = crate::mcp::tools::arg(args, "maxCut").and_then(|v| v.as_f64()) {
                    tab.set_vmax(v);
                }
                // `zoomPercent` is what get_fits_view REPORTS and what the
                // reference declares; this accepted only `zoom`, so an agent
                // reading the view and writing a field straight back was
                // silently ignored. Both spellings work.
                if let Some(z) = crate::mcp::tools::arg(args, "zoomPercent")
                    .or_else(|| crate::mcp::tools::arg(args, "zoom"))
                    .and_then(|v| v.as_f64())
                {
                    tab.set_zoom(z / 100.0);
                }
                if let Some(n) = crate::mcp::tools::arg(args, "northUp").and_then(|v| v.as_bool()) {
                    tab.set_north_up(n);
                }
                // HDU switch (image HDUs only — get_fits_view lists them).
                if let Some(h) = crate::mcp::tools::arg(args, "hdu").and_then(|v| v.as_u64()) {
                    let hdus = tab.hdus();
                    let h = h as usize;
                    // 1-based, as FITS numbers HDUs and as `hdus[].index`
                    // reports them. Zero used to pass this check and reach
                    // cfitsio, which answered "status 301" from inside a status
                    // label while the tool reported success.
                    if h < 1 || h > hdus.len() {
                        return Err(format!(
                            "hdu {h} is out of range — this file has HDUs 1..{}",
                            hdus.len()
                        ));
                    }
                    if !hdus[h - 1].is_image {
                        return Err(format!(
                            "HDU {h} carries no image data; get_fits_view lists which are images"
                        ));
                    }
                    // Propagated. The switch used to write its failure into a
                    // status label and return, so the tool answered
                    // `isError: false` with the PREVIOUS HDU's view state and
                    // the caller had no way to know the switch had not happened.
                    self.switch_hdu(h)?;
                    // The swap replaced the tab: the binding above now points
                    // at a FitsTab detached from the view. Everything below —
                    // crosshair, viewport centre, and the state this returns —
                    // has to act on the one that is actually on screen, or the
                    // reply describes the HDU the caller just left.
                    tab = self
                        .current_tab()
                        .ok_or_else(|| "the HDU switch left no active tab".to_string())?;
                }
                // Crosshair by DISPLAY PIXEL — works with no WCS at all, unlike
                // fits_goto_coordinate. Both halves are required together: one
                // alone would silently place the marker on an axis the caller
                // never specified.
                let chx = crate::mcp::tools::arg(args, "crosshairX").and_then(|v| v.as_f64());
                let chy = crate::mcp::tools::arg(args, "crosshairY").and_then(|v| v.as_f64());
                match (chx, chy) {
                    (Some(x), Some(y)) => {
                        let d = tab.data();
                        if x < 0.0 || y < 0.0 || x >= d.width as f64 || y >= d.height as f64 {
                            return Err(format!(
                                "crosshair ({x}, {y}) is outside the {}x{} image",
                                d.width, d.height
                            ));
                        }
                        tab.canvas().set_crosshair(Some((x, y)));
                        tab.publish_current_crosshair();
                    }
                    (None, None) => {}
                    _ => {
                        return Err("crosshairX and crosshairY must be passed together".to_string())
                    }
                }
                if crate::mcp::tools::arg(args, "clearCrosshair")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                {
                    tab.clear_crosshair();
                }
                // Cross-tab toolbar toggles. Driven through the BUTTONS so their
                // `toggled` handlers run — setting the backing flag directly would
                // leave the toolbar showing the opposite of what is in effect.
                if let Some(v) = crate::mcp::tools::arg(args, "syncZoom").and_then(|v| v.as_bool())
                {
                    self.sync_fov_btn.set_active(v);
                }
                if let Some(v) =
                    crate::mcp::tools::arg(args, "linkedCrosshair").and_then(|v| v.as_bool())
                {
                    self.link_btn.set_active(v);
                }
                if let Some(v) =
                    crate::mcp::tools::arg(args, "showHeaderPanel").and_then(|v| v.as_bool())
                {
                    self.header_expander.set_expanded(v);
                }
                if let Some(v) =
                    crate::mcp::tools::arg(args, "showBookmarksPanel").and_then(|v| v.as_bool())
                {
                    self.coords_expander.set_expanded(v);
                }
                // Centre is applied after zoom so the pan maths uses the new scale.
                let (cur_cx, cur_cy) = tab.viewport_center();
                let cx = crate::mcp::tools::arg(args, "centerX").and_then(|v| v.as_f64());
                let cy = crate::mcp::tools::arg(args, "centerY").and_then(|v| v.as_f64());
                if cx.is_some() || cy.is_some() {
                    tab.set_viewport_center(cx.unwrap_or(cur_cx), cy.unwrap_or(cur_cy));
                }
                self.sync_controls_to_tab(&tab);
                Ok(self.fits_view_state(&tab))
            }
            "probe_fits_pixel" => {
                let tab = self
                    .current_tab()
                    .ok_or_else(|| "no FITS open".to_string())?;
                let x = crate::mcp::tools::arg(args, "x")
                    .and_then(|v| v.as_i64())
                    .ok_or_else(|| "x is required".to_string())?;
                let y = crate::mcp::tools::arg(args, "y")
                    .and_then(|v| v.as_i64())
                    .ok_or_else(|| "y is required".to_string())?;
                if x < 0 || y < 0 {
                    return Err("x and y must be >= 0".into());
                }
                let data = tab.data();
                let value = data.pixel_at(x as usize, y as usize).ok_or_else(|| {
                    format!(
                        "pixel ({x}, {y}) is out of range ({}×{})",
                        data.width, data.height
                    )
                })?;
                let mut out = json!({ "x": x, "y": y, "hasWcs": data.wcs.is_some() });
                // A blanked pixel (NaN/Inf in the data) OMITS `value` rather than
                // emitting a null or a NaN: NaN is not representable in JSON, and
                // serializing one used to fail the whole call.
                if value.is_finite() {
                    out["value"] = json!(value);
                } else {
                    out["blanked"] = json!(true);
                }
                if let Some(w) = data.wcs.as_ref() {
                    let (ra, dec) = w.pixel_to_sky(x as f64, y as f64);
                    out["ra"] = json!(ra);
                    out["dec"] = json!(dec);
                }
                if let Some(u) = header_str(&data.header, "BUNIT") {
                    out["unit"] = json!(u);
                }
                Ok(out)
            }
            "fits_goto_coordinate" => {
                let tab = self
                    .current_tab()
                    .ok_or_else(|| "no FITS open".to_string())?;
                let ra = crate::mcp::tools::arg(args, "ra")
                    .and_then(|v| v.as_f64())
                    .ok_or_else(|| "ra is required".to_string())?;
                let dec = crate::mcp::tools::arg(args, "dec")
                    .and_then(|v| v.as_f64())
                    .ok_or_else(|| "dec is required".to_string())?;
                let data = tab.data();
                let wcs = data
                    .wcs
                    .as_ref()
                    .ok_or_else(|| "the loaded FITS has no WCS".to_string())?;
                match wcs.world_to_pixel(ra, dec) {
                    Some((px, py)) => {
                        let in_bounds = px >= 0.0
                            && px < data.width as f64
                            && py >= 0.0
                            && py < data.height as f64;
                        tab.set_viewport_center(px, py);
                        if in_bounds {
                            tab.canvas().set_crosshair(Some((px, py)));
                        }
                        self.sync_controls_to_tab(&tab);
                        Ok(json!({
                            "moved": true,
                            "ra": ra,
                            "dec": dec,
                            "pixelX": px,
                            "pixelY": py,
                            "inBounds": in_bounds,
                        }))
                    }
                    None => Ok(json!({
                        "moved": false,
                        "ra": ra,
                        "dec": dec,
                        "message": "coordinate is outside the projection domain",
                    })),
                }
            }
            "list_fits_bookmarks" => {
                let items = self.bookmarks.borrow();
                Ok(json!({ "count": items.len(), "bookmarks": *items }))
            }
            "save_fits_bookmark" => {
                let name = crate::mcp::tools::arg(args, "name")
                    .and_then(|v| v.as_str())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| "name is required".to_string())?;

                // Prefer explicit ra/dec; otherwise capture the active tab's crosshair.
                let ra = crate::mcp::tools::arg(args, "ra").and_then(|v| v.as_f64());
                let dec = crate::mcp::tools::arg(args, "dec").and_then(|v| v.as_f64());
                let (ra, dec, source_file) = match (ra, dec) {
                    (Some(ra), Some(dec)) => {
                        let src = self
                            .current_tab()
                            .map(|t| t.source_file().to_string())
                            .unwrap_or_default();
                        (ra, dec, src)
                    }
                    _ => {
                        let tab = self
                            .current_tab()
                            .ok_or_else(|| "no ra/dec given and no FITS open".to_string())?;
                        let (ra, dec) = tab
                            .crosshair_world_pos()
                            .ok_or_else(|| "no ra/dec given and no crosshair placed".to_string())?;
                        (ra, dec, tab.source_file().to_string())
                    }
                };

                let bm = FitsBookmark {
                    name: name.clone(),
                    ra,
                    dec,
                    source_file,
                };
                let mut items = self.bookmarks.borrow_mut();
                // Upsert by name so re-saving a name updates in place.
                if let Some(existing) = items.iter_mut().find(|b| b.name == name) {
                    *existing = bm.clone();
                } else {
                    items.push(bm.clone());
                }
                Ok(serde_json::to_value(&bm).unwrap_or_else(|_| json!({})))
            }
            "delete_fits_bookmark" => {
                let name = crate::mcp::tools::arg(args, "name")
                    .and_then(|v| v.as_str())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| "name is required".to_string())?;
                let mut items = self.bookmarks.borrow_mut();
                let before = items.len();
                items.retain(|b| b.name != name);
                Ok(json!({ "deleted": items.len() != before, "name": name }))
            }
            _ => Err(format!("fits viewer op '{op}' is not supported")),
        }
    }

    /// Snapshot the active tab's file + display state as JSON (shared by
    /// `get_fits_view` and the reply of `set_fits_view`).
    fn fits_view_state(&self, tab: &Rc<FitsTab>) -> serde_json::Value {
        let data = tab.data();
        let (cx, cy) = tab.viewport_center();
        let file_name = std::path::Path::new(tab.source_file())
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| tab.source_file().to_string());
        let has_wcs = data.wcs.is_some();
        let crosshair = tab.crosshair_world_pos();
        json!({
            "loaded": true,
            "fileName": file_name,
            "sourcePath": tab.source_file(),
            "hduName": header_str(&data.header, "EXTNAME"),
            "width": data.width,
            "height": data.height,
            "zoomPercent": tab.zoom_scale() * 100.0,
            "centerX": cx,
            "centerY": cy,
            "stretch": stretch_name(tab.stretch()),
            "colormap": colormap_name(tab.colormap()),
            "minCut": tab.vmin(),
            "maxCut": tab.vmax(),
            "northUp": tab.is_north_up(),
            "hasWcs": has_wcs,
            "crosshairPlaced": tab.crosshair_pixel_pos().is_some(),
            "crosshairRa": crosshair.map(|(ra, _)| ra),
            "crosshairDec": crosshair.map(|(_, dec)| dec),
            // The crosshair's DISPLAY PIXEL too: an image with no WCS has no sky
            // position, but the marker still has a place on the detector.
            "crosshairX": tab.crosshair_pixel_pos().map(|(x, _)| x),
            "crosshairY": tab.crosshair_pixel_pos().map(|(_, y)| y),
            // HDU list + which one is displayed, so a caller can pick a valid
            // `hdu` for set_fits_view without guessing.
            // 1-BASED, matching `hdu` below and what `set_fits_view` takes.
            // These were published from a 0-based `enumerate()` while the list
            // itself is CFITSIO's `1..=n` — so an agent reading `hdus[1].index`
            // and passing it back selected the PRIMARY, one HDU off from the
            // one it had just read about.
            "hdus": tab
                .hdus()
                .iter()
                .map(|h| json!({
                    "index": h.index,
                    "label": h.label(),
                    "isImage": h.is_image,
                }))
                .collect::<Vec<_>>(),
            "hdu": tab.hdu_index(),
            // Pixel units + the true data range, so a caller can choose sane
            // minCut/maxCut values rather than guessing at the scale.
            "pixelUnit": header_str(&data.header, "BUNIT"),
            "dataMin": tab.data_min(),
            "dataMax": tab.data_max(),
            // WCS quality: an approximate solution means sync/blink alignment is
            // only indicative.
            "pixelScaleArcsec": tab.angular_scale_arcsec(),
            "northAngleDeg": tab.north_up_angle(),
            "wcsApproximate": data.wcs.as_ref().map(|w| w.is_approximate),
            // Cross-tab toggles + panels, mirroring the toolbar.
            "syncZoom": self.sync_zoom_enabled.get(),
            "linkedCrosshair": self.link_btn.is_active(),
            "showHeaderPanel": self.header_expander.is_expanded(),
            "showBookmarksPanel": self.coords_expander.is_expanded(),
            "blink": self.blink_state(),
            // Which tab this is, so a reader can correlate with list_open_tabs.
            "tabIndex": self.selected_index().unwrap_or(0),
            "tabCount": self.tabs.borrow().len(),
            "status": self.status_label.text().to_string(),
        })
    }

    /// Register a callback for the coords-panel "Search Here" button — invoked
    /// with the crosshair's `(ra, dec)` in degrees.
    pub fn set_on_search_here(&self, cb: impl Fn(f64, f64) + 'static) {
        self.coords_panel.set_on_search_here(cb);
    }

    /// Position of the selected tab, or `None` when nothing is open.
    ///
    /// `adw::TabView` addresses pages by object where the rest of this viewer
    /// works in indices (the tab list, the blink target, the MCP payload). The
    /// conversion lives here, once, rather than at each of the dozen call sites.
    fn selected_index(&self) -> Option<usize> {
        let page = self.tab_view.selected_page()?;
        Some(self.tab_view.page_position(&page) as usize)
    }

    /// Select the tab at `index`, if there is one.
    /// Point the shared status line at `tab`.
    ///
    /// The line is viewer-wide, not per-tab, and nothing refreshed it when the
    /// selection changed — so switching to a 720x360 image left "64x64 pixels"
    /// on screen, and `get_fits_view` reported that text as the new tab's
    /// status. It describes whichever tab is active now.
    fn show_status_for(&self, tab: &Rc<FitsTab>) {
        self.status_label
            .set_text(&fits_loader::fits_summary(tab.data()));
    }

    fn select_index(&self, index: usize) {
        if index < self.tab_view.n_pages() as usize {
            let page = self.tab_view.nth_page(index as i32);
            self.tab_view.set_selected_page(&page);
        }
    }

    fn current_tab(&self) -> Option<Rc<FitsTab>> {
        let idx = self.selected_index()?;
        self.tabs.borrow().get(idx).cloned()
    }

    /// Sync every control in the column to the given tab's current state.
    fn sync_controls_to_tab(&self, tab: &Rc<FitsTab>) {
        // Called after a tab is registered — on open, on HDU switch and on tab
        // change — so the marks list settles onto the same tab the rest of the
        // toolbar does.
        self.refresh_annotations_panel_for(tab);
        *self.suppress_sync.borrow_mut() = true;

        // The header section shows the image you are LOOKING at. One panel for
        // the viewer means it has to be repointed here; the alternative, one
        // panel per tab, is what used to put 320 px of layout beside every open
        // image whether or not anyone had opened it.
        let data = tab.data();
        self.header_panel
            .set_content(data.header_ordered.clone(), data.image_info_rows());

        self.stretch_combo
            .set_selected(stretch_to_index(tab.stretch()));
        self.colormap_combo
            .set_selected(colormap_to_index(tab.colormap()));

        // Update the min/max scale range to the image's own extrema
        let data_min = tab.data_min();
        let data_max = tab.data_max();
        let step = ((data_max - data_min) / 200.0).max(1e-6);
        self.min_scale.set_range(data_min, data_max);
        self.min_scale.set_increments(step, step * 10.0);
        self.min_scale.set_value(tab.vmin());
        self.max_scale.set_range(data_min, data_max);
        self.max_scale.set_increments(step, step * 10.0);
        self.max_scale.set_value(tab.vmax());

        self.north_up_btn.set_active(tab.is_north_up());

        // Zoom: find the closest preset + reflect the exact % in the entry.
        let current_pct = (tab.zoom_scale() * 100.0).round() as i32;
        let closest = ZOOM_PRESETS
            .iter()
            .position(|(p, _)| *p == current_pct)
            .unwrap_or(3); // default to 100%
        self.zoom_combo.set_selected(closest as u32);
        self.zoom_entry.set_text(&current_pct.to_string());

        // Sync crosshair readout
        self.coords_panel
            .set_current_crosshair(tab.canvas().crosshair_pos(), tab.data().wcs.as_ref());

        *self.suppress_sync.borrow_mut() = false;
    }

    async fn open_file_dialog(self: &Rc<Self>, parent: &impl IsA<gtk::Widget>) {
        let root = parent.root().and_downcast::<gtk::Window>();
        let filter = gtk::FileFilter::new();
        filter.add_pattern("*.fits");
        filter.add_pattern("*.FITS");
        filter.add_pattern("*.fit");
        filter.add_pattern("*.fts");
        filter.set_name(Some(crate::tr_en!("FITS Images")));

        let filters = gtk4::gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);

        let dialog = gtk::FileDialog::builder()
            .title(crate::tr_en!("Open FITS File"))
            .filters(&filters)
            .build();

        if let Ok(file) = dialog.open_future(root.as_ref()).await {
            if let Some(path) = file.path() {
                // The picker's own error goes to the status label the user is
                // already looking at; only the agent needs it as a value.
                let _ = self.load_file(&path);
            }
        }
    }

    fn load_file(self: &Rc<Self>, path: &std::path::Path) -> Result<(), String> {
        match fits_loader::load_fits_image(path) {
            Ok(data) => {
                let filename = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                let source = path.to_string_lossy().to_string();

                let summary = fits_loader::fits_summary(&data);
                self.status_label.set_text(&summary);

                let tab = FitsTab::new(data, self.shared.clone(), source);

                // Wire crosshair callback (coords readout + go-to/right-click recenter)
                self.wire_tab_callbacks(&tab);

                // Record the file's HDU list + which extension we auto-picked.
                let hdus = fits_loader::list_hdus(path).unwrap_or_default();
                let initial_hdu = hdus
                    .iter()
                    .find(|h| h.is_image)
                    .map(|h| h.index)
                    .unwrap_or(1);
                tab.set_hdu_context(hdus, initial_hdu);

                // The tab strip owns its own close button, reordering and
                // overview. The Notebook this replaced needed a hand-rolled
                // close button that walked every page comparing label widgets to
                // discover which tab it belonged to — and a separate republish,
                // because closing a non-active tab fired no switch signal.
                let page = self.tab_view.append(tab.widget());
                page.set_title(&filename);
                page.set_tooltip(path.to_string_lossy().as_ref());
                self.tab_view.set_selected_page(&page);

                self.tabs.borrow_mut().push(tab.clone());
                self.publish_open_tabs();

                // Sync toolbar + extension selector + WCS banner to the new tab
                self.sync_controls_to_tab(&tab);
                self.update_hdu_and_banner(&tab);
            }
            Err(e) => {
                self.status_label.set_text(&crate::tr_fmt!("Error: {}", e));
                return Err(e.to_string());
            }
        }
        Ok(())
    }

    /// Wire a freshly-built tab's crosshair callback: update the coords readout
    /// and, when zoomed in (>1.05×), recenter the viewport on the placed pixel
    /// (mirrors the Windows `PlaceCrosshair` / `CenterOnImagePixel` behaviour so
    /// go-to and right-click targets stay on-screen).
    fn wire_tab_callbacks(self: &Rc<Self>, tab: &Rc<FitsTab>) {
        // Marks come back with the image they were drawn on.
        let file = Self::annotation_target(tab);
        if !file.is_empty() {
            let saved = crate::helpers::annotation_store::load_for(&file);
            if !saved.is_empty() {
                tab.canvas().set_annotations(saved);
                // The list is built from the canvas, so loading marks into the
                // canvas without saying so left the panel showing none of them.
                self.refresh_annotations_panel();
            }
        }

        // The list follows the canvas: clicking a mark on the image highlights
        // its row, so the two are one selection rather than two.
        {
            let viewer = Rc::downgrade(self);
            {
                let viewer = Rc::downgrade(self);
                tab.canvas().set_on_label_clicked(move |id| {
                    if let Some(v) = viewer.upgrade() {
                        v.refresh_annotations_panel();
                        v.ask_for_text_at_leader(id);
                    }
                });
            }

            {
                // One subscription. Every route that changes the marks — the
                // toolbar, the panel, an agent's tool call, loading a file —
                // goes through the canvas, so the list follows all of them
                // without any of them knowing it exists.
                let viewer = Rc::downgrade(self);
                // From THIS tab, not from `current_tab()`. `wire_tab_callbacks`
                // runs BEFORE the tab is registered — on open and on every HDU
                // switch — so a refresh that asked which tab was current got
                // the old one or none, and the panel was handed an empty list
                // while the marks sat on the new canvas. It filled in later
                // only because clicking a mark refreshes again, by which time
                // the tab exists.
                let owner = Rc::downgrade(tab);
                tab.canvas().set_on_annotations_changed(move || {
                    if let (Some(v), Some(tab)) = (viewer.upgrade(), owner.upgrade()) {
                        v.refresh_annotations_panel_for(&tab);
                    }
                });
            }
            {
                let viewer = Rc::downgrade(self);
                tab.canvas().set_on_marks_moved(move || {
                    if let Some(v) = viewer.upgrade() {
                        v.follow_label_editor();
                    }
                });
            }

            let tab_for_save = tab.clone();
            tab.canvas().set_on_selection_changed(move || {
                if let Some(v) = viewer.upgrade() {
                    // Fires for a new selection AND for the end of a
                    // move/resize drag, so this is also where a reshaped mark
                    // reaches the disk.
                    v.persist_annotations(&tab_for_save);
                    v.refresh_annotations_panel();
                }
            });
        }

        let coords_panel = self.coords_panel.clone();
        let search_here_btn = self.search_here_btn.clone();
        let wcs = tab.data().wcs.clone();
        // Weak ref avoids a canvas→callback→canvas reference cycle.
        let canvas_weak = Rc::downgrade(tab.canvas());
        tab.canvas().set_on_crosshair_placed(move |pos| {
            coords_panel.set_current_crosshair(pos, wcs.as_ref());
            // Searchable only when the crosshair has a sky position — an image
            // with no WCS has a marker but no coordinates to search at, and a
            // button that looks available and then does nothing is worse than
            // one that is plainly not yet applicable.
            search_here_btn.set_sensitive(coords_panel.has_sky_position());
            if let Some((px, py)) = pos {
                if let Some(canvas) = canvas_weak.upgrade() {
                    if canvas.zoom_scale() > 1.05 {
                        canvas.set_viewport_center(px, py);
                    }
                }
            }
        });
    }

    /// Refresh the approximate-WCS banner and the extension selector for `tab`.
    fn update_hdu_and_banner(&self, tab: &Rc<FitsTab>) {
        self.update_wcs_banner();
        self.set_hdu_selector(&tab.hdus(), tab.hdu_index());
    }

    /// Reveal the approximate-WCS banner iff a sync/link mode is active AND any
    /// open tab has a missing / invalid / approximate WCS (mirrors Windows
    /// `UpdateWcsSyncWarning`). Sync maps positions through each image's WCS, so
    /// an imprecise one makes the linked crosshair and matched zoom unreliable.
    fn update_wcs_banner(&self) {
        let syncing = self.sync_zoom_enabled.get() || self.link_btn.is_active();
        let any_imprecise = syncing
            && self
                .tabs
                .borrow()
                .iter()
                .any(|t| match t.data().wcs.as_ref() {
                    Some(w) => !w.is_valid() || w.is_approximate,
                    None => true,
                });
        self.wcs_banner.set_revealed(syncing && any_imprecise);
    }

    /// Capture the active tab's current zoom as a shared angular scale
    /// (arcsec per screen pixel) so other tabs can match it when they become
    /// active (mirrors Windows `UpdateSharedAngularZoom`).
    fn update_shared_angular_zoom(&self) {
        let Some(tab) = self.current_tab() else {
            return;
        };
        if let Some(arcsec_per_px) = tab.angular_scale_arcsec() {
            self.shared_angular_zoom.set(arcsec_per_px);
        }
    }

    /// Re-apply the shared angular zoom to `tab` (and re-center it on the shared
    /// linked crosshair sky point) when the sync-zoom toggle is on. Mirrors the
    /// zoom step of Windows `ApplySharedViewToActivePage`.
    fn apply_shared_view_to_active(&self, tab: &Rc<FitsTab>) {
        if !self.sync_zoom_enabled.get() {
            return;
        }
        let shared = self.shared_angular_zoom.get();
        if shared <= 0.0 {
            return;
        }
        // Zoom so this tab shows `shared` arcsec per screen pixel.
        tab.set_angular_scale_arcsec(shared);
        // Re-center on the shared linked crosshair sky point, if one is set.
        let placed = self.shared.borrow().placed;
        if let Some((ra, dec)) = placed {
            tab.center_on_world(ra, dec);
        }
    }

    /// Populate the extension dropdown from `hdus`, selecting `selected_index`
    /// (1-based HDU). Hidden unless the file has more than one image HDU.
    fn set_hdu_selector(&self, hdus: &[HduInfo], selected_index: usize) {
        let image_count = hdus.iter().filter(|h| h.is_image).count();
        if image_count <= 1 {
            self.hdu_bar.set_visible(false);
            *self.hdu_infos.borrow_mut() = hdus.to_vec();
            return;
        }

        *self.suppress_sync.borrow_mut() = true;
        *self.hdu_infos.borrow_mut() = hdus.to_vec();

        // Only rebuild the model when the LIST changed.
        //
        // This is reached from the dropdown's own `selected_notify`, by way of
        // `switch_hdu`: choosing an extension replaced the model of the widget
        // whose signal was still being emitted, freeing the items GTK was
        // holding — a segfault, and only ever from the dropdown, because the
        // MCP path does not run inside that signal.
        //
        // Switching extension within a file cannot change the list — it is the
        // same file — so in the case that crashes there is nothing to rebuild.
        // `suppress_sync` never protected against this: it stops OUR handler
        // re-entering and says nothing about GTK's own references.
        let labels: Vec<String> = hdus.iter().map(|h| h.label()).collect();
        let current: Option<Vec<String>> = self
            .hdu_dropdown
            .model()
            .and_then(|m| m.downcast::<gtk::StringList>().ok())
            .map(|list| {
                (0..list.n_items())
                    .map(|i| list.string(i).map(|s| s.to_string()).unwrap_or_default())
                    .collect()
            });

        if model_needs_rebuild(current.as_deref(), &labels) {
            let model = gtk::StringList::new(&[]);
            for label in &labels {
                model.append(label);
            }
            self.hdu_dropdown.set_model(Some(&model));
        }

        let pos = hdus
            .iter()
            .position(|h| h.index == selected_index)
            .unwrap_or(0) as u32;
        self.hdu_dropdown.set_selected(pos);
        *self.hdu_current_pos.borrow_mut() = pos;
        *self.suppress_sync.borrow_mut() = false;

        self.hdu_bar.set_visible(true);
    }

    // ── Annotations ─────────────────────────────────────────────────────────

    /// What a tab's marks are filed under.
    ///
    /// `tab.source_file()`, and not the view-state JSON: five call sites read
    /// `get("path")` from that payload, the field is called `sourcePath`, and
    /// every one of them silently got an empty string — so every file's marks
    /// went into one bucket keyed `""`, and none came back when a file was
    /// reopened. One function now, and it asks the tab directly rather than
    /// going through a serialized copy of what the tab already knows.
    fn annotation_target(tab: &Rc<FitsTab>) -> String {
        tab.source_file().to_string()
    }

    /// Arm or disarm click-to-place on the active tab.
    fn set_draw_mode(self: &Rc<Self>, on: bool) {
        let Some(tab) = self.current_tab() else {
            return;
        };
        let canvas = tab.canvas();
        if !on {
            canvas.clear_on_left_click();
            return;
        }
        {
            // One source for both the preview and the mark it becomes: the
            // picker itself, asked each time.
            let viewer = Rc::downgrade(self);
            canvas.set_preview_kind_source(move || {
                viewer
                    .upgrade()
                    .map(|v| v.selected_draw_kind())
                    .unwrap_or(crate::models::annotation::AnnotationKind::Circle)
            });
        }
        let viewer = Rc::downgrade(self);
        canvas.set_on_left_click(move |img_x, img_y, half| {
            let Some(v) = viewer.upgrade() else { return };
            // Read at CLICK time, not when Draw was switched on. Capturing it
            // here meant the shape picked when the mode was armed was the shape
            // you got for ever after: choosing Box and drawing gave a circle.
            v.place_mark(v.selected_draw_kind(), img_x, img_y, half);
        });
    }

    fn selected_draw_kind(&self) -> crate::models::annotation::AnnotationKind {
        use crate::models::annotation::AnnotationKind::*;
        match self.draw_kind.selected() {
            1 => Rect,
            _ => Circle,
        }
    }

    /// Add a mark where the user clicked.
    ///
    /// A callout or a text needs words, so those open a small entry first; a
    /// bare shape lands immediately. Anchored to the sky when the image has
    /// WCS, so the mark survives reopening the file.
    fn place_mark(
        self: &Rc<Self>,
        kind: crate::models::annotation::AnnotationKind,
        img_x: f64,
        img_y: f64,
        dragged_half_px: f64,
    ) {
        use crate::models::annotation::{Anchor, Annotation, Author};
        let Some(tab) = self.current_tab() else {
            return;
        };
        let anchor = tab
            .data()
            .wcs
            .as_ref()
            .filter(|w| w.is_valid())
            .map(|w| {
                let (ra, dec) = w.pixel_to_sky(img_x, img_y);
                Anchor::Sky {
                    ra_deg: ra,
                    dec_deg: dec,
                }
            })
            .filter(|a| a.is_valid())
            .unwrap_or(Anchor::ImagePixel { x: img_x, y: img_y });

        // The size the user dragged out, converted into the anchor's own
        // units. A tap with no drag falls back to a default, so a click still
        // makes a mark rather than nothing.
        let canvas = tab.canvas();
        let extent = if dragged_half_px > 3.0 {
            crate::models::annotation::Extent::square(
                dragged_half_px * canvas.units_per_image_pixel(&anchor),
            )
        } else {
            canvas.default_extent_for(&anchor)
        };

        // Every kind gets its label the same way: the shape lands, then a
        // cursor appears at the end of its leader and you type. A callout with
        // no words is fine for the moment — you are about to give it some.
        let mark = Annotation::new(kind, anchor, "", Author::User).with_extent(extent);
        let id = mark.id.clone();
        self.add_mark(mark);
        self.ask_for_text_at_leader(&id);
    }

    /// Store a validated mark and show it.
    fn add_mark(&self, mark: crate::models::annotation::Annotation) {
        if mark.validate().is_err() {
            return;
        }
        let Some(tab) = self.current_tab() else {
            return;
        };
        let canvas = tab.canvas();
        let mut all = canvas.annotations();
        all.push(mark);
        canvas.set_annotations(all);
        self.persist_annotations(&tab);
        self.refresh_annotations_panel();
    }

    /// A cursor at the end of the mark's leader, to type its label into.
    ///
    /// Where the text will BE, rather than in a dialog off to one side: you
    /// drag out a shape, the leader appears, and the caret is waiting on its
    /// rule. Escape leaves the shape unlabelled, which is a perfectly good
    /// mark.
    ///
    /// The popover collects the words; cairo draws them. A label laid out as a
    /// widget would show on screen and be missing from every capture an agent
    /// takes.
    fn ask_for_text_at_leader(self: &Rc<Self>, id: &str) {
        let Some(tab) = self.current_tab() else {
            return;
        };
        let canvas = tab.canvas();
        // Editing implies selection, and only the edited mark gets grips.
        canvas.set_selected_annotation(Some(id.to_string()));
        canvas.set_editing_annotation(Some(id.to_string()));
        let Some(mark) = canvas.annotations().into_iter().find(|a| a.id == id) else {
            return;
        };
        let Some(rect) = canvas.leader_label_rect(&mark) else {
            return;
        };

        // An overlay child, not a popover. A popover is its own surface: with
        // autohide on it dismissed itself the moment the pointer went back to
        // the image, and with autohide off one was seen floating above an
        // unrelated application's window. This is clipped to the canvas, moves
        // with the window, and can follow the mark while it is dragged.
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        row.add_css_class("osd");
        row.add_css_class("toolbar");
        row.set_margin_end(6);

        let entry = gtk::Entry::new();
        entry.set_placeholder_text(Some(crate::tr_en!("What is this?")));
        entry.set_width_chars(16);
        row.append(&entry);
        if !mark.text.is_empty() {
            entry.set_text(&mark.text);
            entry.select_region(0, -1);
        }

        let done = gtk::Button::from_icon_name("object-select-symbolic");
        done.add_css_class("suggested-action");
        done.set_tooltip_text(Some(crate::tr_en!("Done")));
        row.append(&done);

        let bin = gtk::Button::from_icon_name("user-trash-symbolic");
        bin.add_css_class("destructive-action");
        bin.set_tooltip_text(Some(crate::tr_en!("Delete this mark")));
        row.append(&bin);

        // One editor at a time.
        self.close_label_editor();
        canvas.place_over_image(&row, rect.x() as f64, (rect.y() - 26).max(0) as f64);
        *self.open_label_editor.borrow_mut() = Some((id.to_string(), row.clone()));

        let id = id.to_string();
        let commit = {
            let viewer = Rc::downgrade(self);
            let entry = entry.clone();
            let id = id.clone();
            move || {
                let text = entry.text().to_string();
                if let Some(v) = viewer.upgrade() {
                    v.set_mark_text(&id, &text);
                    v.leave_edit_mode();
                }
            }
        };
        {
            let commit = commit.clone();
            entry.connect_activate(move |_| commit());
        }
        {
            let commit = commit.clone();
            done.connect_clicked(move |_| commit());
        }
        {
            let viewer = Rc::downgrade(self);
            let id = id.clone();
            bin.connect_clicked(move |_| {
                if let Some(v) = viewer.upgrade() {
                    v.delete_mark(&id);
                    v.leave_edit_mode();
                }
            });
        }
        {
            let viewer = Rc::downgrade(self);
            let keys = gtk::EventControllerKey::new();
            keys.connect_key_pressed(move |_, key, _, _| {
                if key == gtk::gdk::Key::Escape {
                    if let Some(v) = viewer.upgrade() {
                        v.leave_edit_mode();
                    }
                    return gtk::glib::Propagation::Stop;
                }
                gtk::glib::Propagation::Proceed
            });
            entry.add_controller(keys);
        }
        entry.grab_focus();
    }

    /// Take the label editor off the image, if one is up.
    fn close_label_editor(&self) {
        let open = self.open_label_editor.borrow_mut().take();
        if let Some((_, row)) = open {
            if let Some(tab) = self.current_tab() {
                tab.canvas().remove_from_image(&row);
            }
        }
    }

    /// Leave edit mode: commit nothing, close the field, drop the selection.
    ///
    /// Edit mode IS a mark being selected — the grips and the label field are
    /// two faces of the same state, so they start and end together. Anything
    /// that ends one has to end the other, or the window shows a mark that is
    /// half in and half out of being edited.
    fn leave_edit_mode(&self) {
        self.close_label_editor();
        if let Some(tab) = self.current_tab() {
            let canvas = tab.canvas();
            canvas.set_editing_annotation(None);
            canvas.set_selected_annotation(None);
        }
        self.refresh_annotations_panel();
    }

    /// Re-aim the open label editor at its mark's current position.
    ///
    /// A popover is pointed at a rectangle once, so dragging the mark left the
    /// editor behind, hanging over wherever the shape used to be. It is
    /// re-pointed as the shape moves.
    fn follow_label_editor(&self) {
        let open = self.open_label_editor.borrow();
        let Some((id, row)) = open.as_ref() else {
            return;
        };
        let Some(tab) = self.current_tab() else {
            return;
        };
        let canvas = tab.canvas();
        let Some(mark) = canvas.annotations().into_iter().find(|a| &a.id == id) else {
            return;
        };
        if let Some(rect) = canvas.leader_label_rect(&mark) {
            canvas.position_over_image(row, rect.x() as f64, (rect.y() - 26).max(0) as f64);
        }
    }

    /// Remove one mark.
    fn delete_mark(&self, id: &str) {
        let Some(tab) = self.current_tab() else {
            return;
        };
        let canvas = tab.canvas();
        let mut all = canvas.annotations();
        all.retain(|a| a.id != id);
        canvas.set_annotations(all);
        canvas.set_selected_annotation(None);
        self.persist_annotations(&tab);
        self.refresh_annotations_panel();
    }

    /// Give an existing mark its label.
    fn set_mark_text(&self, id: &str, text: &str) {
        let Some(tab) = self.current_tab() else {
            return;
        };
        let canvas = tab.canvas();
        let mut all = canvas.annotations();
        let Some(mark) = all.iter_mut().find(|a| a.id == id) else {
            return;
        };
        mark.text = text.trim().to_string();
        canvas.set_annotations(all);
        self.persist_annotations(&tab);
        self.refresh_annotations_panel();
    }

    /// Save the active tab's marks under its file.
    fn persist_annotations(&self, tab: &Rc<FitsTab>) {
        let file = Self::annotation_target(tab);
        let _ = crate::helpers::annotation_store::save_for(&file, &tab.canvas().annotations());
    }

    /// Redraw the list from the active tab.
    fn refresh_annotations_panel(&self) {
        match self.current_tab() {
            Some(tab) => self.refresh_annotations_panel_for(&tab),
            None => self.annotations_panel.set_annotations(&[], None),
        }
    }

    /// Put the sidebar back to "no file open".
    ///
    /// Closing the last file left the extension dropdown sitting there naming
    /// HDUs of a file that was gone, and the marks list showing marks from it.
    /// Everything that describes a FILE is cleared in one place, so the next
    /// thing that describes one cannot be forgotten here.
    fn show_no_file_open(&self) {
        self.hdu_bar.set_visible(false);
        self.hdu_infos.borrow_mut().clear();
        self.close_label_editor();
        self.annotations_panel.set_annotations(&[], None);
        self.draw_mode.set_active(false);
    }

    /// Show `tab`'s marks, whether or not it is the registered current tab.
    fn refresh_annotations_panel_for(&self, tab: &Rc<FitsTab>) {
        let canvas = tab.canvas();
        self.annotations_panel.set_annotations(
            &canvas.annotations(),
            canvas.selected_annotation().as_deref(),
        );
    }

    /// Reload a different image HDU of the active tab's file, replacing the
    /// current tab's content in place (mirrors Windows `SelectHdu`).
    fn switch_hdu(self: &Rc<Self>, hdu_index: usize) -> Result<(), String> {
        let page_idx = self
            .selected_index()
            .ok_or_else(|| "no FITS tab is selected".to_string())?;
        let old_tab = self
            .tabs
            .borrow()
            .get(page_idx)
            .cloned()
            .ok_or_else(|| "the selected tab has no viewer".to_string())?;
        if old_tab.hdu_index() == hdu_index {
            return Ok(());
        }

        let path_str = old_tab.source_file().to_string();
        let path = std::path::Path::new(&path_str);
        let data = match fits_loader::load_fits_image_hdu(path, hdu_index) {
            Ok(d) => d,
            Err(e) => {
                self.status_label.set_text(&crate::tr_fmt!("Error: {}", e));
                // Revert the dropdown to the still-displayed HDU.
                self.set_hdu_selector(&old_tab.hdus(), old_tab.hdu_index());
                // And tell the caller. This used to return quietly, leaving the
                // tool to answer with the old HDU's state and no error.
                return Err(format!("could not switch to HDU {hdu_index}: {e}"));
            }
        };

        self.status_label
            .set_text(&fits_loader::fits_summary(&data));

        let new_tab = FitsTab::new(data, self.shared.clone(), path_str.clone());
        new_tab.set_hdu_context(old_tab.hdus(), hdu_index);
        self.wire_tab_callbacks(&new_tab);

        // Swap the page's content, keeping its position and title. `insert`
        // takes the position directly, so the replacement lands where the old
        // one was rather than at the end.
        let old_page = self.tab_view.page(old_tab.widget());
        let title = old_page.title();
        let tooltip = old_page.tooltip().unwrap_or_default();
        *self.suppress_page_switch.borrow_mut() = true;
        *self.rebuilding_page.borrow_mut() = true;
        // The handler runs `close_page_finish` itself; calling it again here
        // finished an already-finished page.
        self.tab_view.close_page(&old_page);
        let new_page = self.tab_view.insert(new_tab.widget(), page_idx as i32);
        new_page.set_title(&title);
        new_page.set_tooltip(&tooltip);
        // The slot is still there because the close handler left it alone.
        // Registering unconditionally rather than only when the index happens
        // to exist: a viewer with a page and no tab answers "no FITS open" for
        // a file that is plainly on screen.
        {
            let mut tabs = self.tabs.borrow_mut();
            let at = page_idx.min(tabs.len());
            match tabs.get_mut(page_idx) {
                Some(slot) => *slot = new_tab.clone(),
                None => tabs.insert(at, new_tab.clone()),
            }
        }
        self.tab_view.set_selected_page(&new_page);
        *self.rebuilding_page.borrow_mut() = false;
        *self.suppress_page_switch.borrow_mut() = false;
        // Republished here instead of by the close handler, which was told to
        // stay quiet: the snapshot must describe the NEW tab, not the old one.
        publish_fits_tabs(&self.tab_view, &self.tabs);

        self.sync_controls_to_tab(&new_tab);
        self.update_hdu_and_banner(&new_tab);
        Ok(())
    }

    /// Begin a cross-fade blink: overlay the target tab (B) onto the active tab
    /// (A) and oscillate its opacity so A fades into B and back. Replaces the old
    /// hard page-flip. The two frames are aligned on a shared sky point at a
    /// matched angular scale + orientation before the overlay is built.
    fn start_blink(&self) {
        // Resolve A (active) and B (target) tabs.
        let a_idx = match self.selected_index() {
            Some(i) => i,
            None => {
                self.cancel_blink_toggle(crate::tr_en!("Blink needs two open tabs"));
                return;
            }
        };
        let (a, b, b_idx) = {
            let tabs = self.tabs.borrow();
            let Some(a) = tabs.get(a_idx).cloned() else {
                drop(tabs);
                self.cancel_blink_toggle(crate::tr_en!("Blink needs two open tabs"));
                return;
            };
            // Prefer the picked target; fall back to the first other tab.
            let want = self.blink_target.get();
            let b_idx = if want != a_idx && want < tabs.len() {
                want
            } else {
                match (0..tabs.len()).find(|&i| i != a_idx) {
                    Some(i) => i,
                    None => {
                        drop(tabs);
                        self.cancel_blink_toggle(crate::tr_en!("Blink needs two open tabs"));
                        return;
                    }
                }
            };
            let b = tabs[b_idx].clone();
            (a, b, b_idx)
        };
        self.blink_target.set(b_idx);

        // Snapshot A's pre-blink viewport (center + zoom) so Stop can restore it —
        // the alignment below re-centers A on the reference sky point (mirrors the
        // Windows `_blinkRestore`).
        let (a_cx, a_cy) = a.viewport_center();
        *self.blink_restore.borrow_mut() = Some((a.clone(), a_cx, a_cy, a.zoom_scale()));

        // Align A and B on a shared sky point at a matched angular scale.
        let ref_sky = a.crosshair_world_pos().or_else(|| a.image_center_world());
        if let Some(target_scale) = a.angular_scale_arcsec() {
            b.set_angular_scale_arcsec(target_scale);
        }
        if let Some((ra, dec)) = ref_sky {
            a.center_on_world(ra, dec);
            b.center_on_world(ra, dec);
        }

        // Overlay B onto A and start the fade cycle.
        let overlay = self.build_blink_overlay(&a, &b, ref_sky);
        a.canvas().enter_blink(overlay);
        *self.blink_canvas.borrow_mut() = Some(a.canvas().clone());
        self.blink_opacity.set(0.0);
        self.blink_fading_in.set(true);
        self.blink_paused.set(false);
        self.update_blink_target_label();

        let blink_active = self.blink_active.clone();
        let paused = self.blink_paused.clone();
        let opacity = self.blink_opacity.clone();
        let fading_in = self.blink_fading_in.clone();
        let interval = self.blink_interval_ms.clone();
        let canvas = self.blink_canvas.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(50), move || {
            if !*blink_active.borrow() {
                return glib::ControlFlow::Break;
            }
            if !paused.get() {
                // Fraction of the fade to advance per 50 ms tick.
                let step = 50.0 / (interval.get().max(100) as f64);
                let mut o = opacity.get();
                if fading_in.get() {
                    o += step;
                    if o >= 1.0 {
                        o = 1.0;
                        fading_in.set(false);
                    }
                } else {
                    o -= step;
                    if o <= 0.0 {
                        o = 0.0;
                        fading_in.set(true);
                    }
                }
                opacity.set(o);
                if let Some(c) = canvas.borrow().as_ref() {
                    c.set_blink_opacity(o);
                }
            }
            glib::ControlFlow::Continue
        });

        if let Some(name) = self.tab_name(b_idx) {
            self.status_label.set_text(&crate::tr_fmt!(
                "Blinking vs {}  (Space pause · Left/Right show A/B · Esc stop)",
                name
            ));
        }
    }

    /// Stop an active cross-fade blink and drop the overlay.
    /// Drive the blink comparison over MCP: start against a partner tab, adjust
    /// the fade interval, pause/resume, or stop.
    ///
    /// Everything goes through `blink_btn`, the same toggle a click drives, so the
    /// toolbar can never show "blinking" while the blink is stopped.
    fn blink_command(&self, args: &serde_json::Value) -> Result<serde_json::Value, String> {
        let action = crate::mcp::tools::str_arg(args, "action");
        if !matches!(action.as_str(), "start" | "stop" | "pause" | "resume") {
            return Err("action must be start, stop, pause, or resume".to_string());
        }

        // Interval applies with ANY action, matching the reference.
        if let Some(ms) = crate::mcp::tools::arg(args, "intervalMs").and_then(|v| v.as_u64()) {
            if !(500..=5000).contains(&ms) {
                return Err(format!("intervalMs must be between 500 and 5000, got {ms}"));
            }
            self.blink_interval_ms.set(ms);
        }

        match action.as_str() {
            "start" => {
                let partner = crate::mcp::tools::arg(args, "withTabIndex")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| {
                        "start requires withTabIndex (a fitsTabs index from list_open_tabs)"
                            .to_string()
                    })? as usize;
                let count = self.tabs.borrow().len();
                if partner >= count {
                    return Err(format!("no FITS tab at index {partner} ({count} open)"));
                }
                let active = self.selected_index().unwrap_or(0);
                if partner == active {
                    return Err(
                        "withTabIndex must be a DIFFERENT tab than the active one".to_string()
                    );
                }
                self.blink_target.set(partner);
                self.blink_paused.set(false);
                self.blink_btn.set_active(true);
            }
            "stop" => self.blink_btn.set_active(false),
            "pause" => self.blink_paused.set(true),
            "resume" => self.blink_paused.set(false),
            _ => unreachable!("action was validated above"),
        }
        Ok(self.blink_state())
    }

    /// The blink state as `blink_fits_tabs` reports it.
    fn blink_state(&self) -> serde_json::Value {
        let active = *self.blink_active.borrow();
        let partner = active
            .then(|| self.tab_name(self.blink_target.get()))
            .flatten();
        serde_json::json!({
            "active": active,
            "paused": self.blink_paused.get(),
            "partnerTab": partner,
            "intervalMs": self.blink_interval_ms.get(),
        })
    }

    fn stop_blink(&self) {
        *self.blink_active.borrow_mut() = false;
        if let Some(c) = self.blink_canvas.borrow_mut().take() {
            c.exit_blink();
        }
        // Restore image A's pre-blink view (blink re-centered/re-zoomed it to frame
        // the overlap). Mirrors Windows `StopBlink` `_blinkRestore`.
        if let Some((a, cx, cy, zoom)) = self.blink_restore.borrow_mut().take() {
            a.set_zoom(zoom);
            a.set_viewport_center(cx, cy);
        }
    }

    /// Snap the blink to A (`0.0`) or B (`1.0`) and set the fade direction so it
    /// eases back afterwards (the ←/→ keys).
    fn blink_show(&self, opacity: f64) {
        self.blink_opacity.set(opacity);
        self.blink_fading_in.set(opacity < 0.5);
        if let Some(c) = self.blink_canvas.borrow().as_ref() {
            c.set_blink_opacity(opacity);
        }
    }

    /// Untoggle the blink button and report why (used for early-exit guards).
    fn cancel_blink_toggle(&self, msg: &str) {
        self.status_label.set_text(msg);
        self.blink_btn.set_active(false);
    }

    /// Build the overlay that draws tab `b`'s image onto tab `a`'s canvas: pin
    /// B's reference pixel to A's on-screen reference point, matching B's angular
    /// scale and sky orientation to A's displayed frame.
    fn build_blink_overlay(
        &self,
        a: &Rc<FitsTab>,
        b: &Rc<FitsTab>,
        ref_sky: Option<(f64, f64)>,
    ) -> BlinkOverlay {
        let ac = a.canvas();
        let bc = b.canvas();
        let (aw, ah) = (ac.img_width(), ac.img_height());
        let (bw, bh) = (bc.img_width(), bc.img_height());

        // Reference pixel in each image's own pixel space.
        let (a_ref_px, a_ref_py) = ref_sky
            .and_then(|(ra, dec)| {
                a.data()
                    .wcs
                    .as_ref()
                    .and_then(|w| w.world_to_pixel(ra, dec))
            })
            .unwrap_or((aw as f64 / 2.0, ah as f64 / 2.0));
        let (b_ref_px, b_ref_py) = ref_sky
            .and_then(|(ra, dec)| {
                b.data()
                    .wcs
                    .as_ref()
                    .and_then(|w| w.world_to_pixel(ra, dec))
            })
            .unwrap_or((bw as f64 / 2.0, bh as f64 / 2.0));

        // Where A currently draws that reference pixel on screen.
        let (anchor_x, anchor_y) = ac.image_to_screen_point(a_ref_px, a_ref_py);

        // Overlay scale: B's matched zoom when sky-aligned, else fit B's width to A.
        let scale = if ref_sky.is_some() {
            bc.zoom_scale()
        } else if bw > 0 {
            ac.zoom_scale() * (aw as f64 / bw as f64)
        } else {
            ac.zoom_scale()
        };

        // Orientation: rotate B so its north matches A's displayed orientation.
        // `north_up_angle()` is the (negated) rotation that puts north up.
        let a_rot = ac.rotation_rad();
        let na = a.north_up_angle().unwrap_or(0.0);
        let nb = b.north_up_angle().unwrap_or(0.0);
        let rot = a_rot - na + nb;

        BlinkOverlay {
            rgba: bc.current_rgba(),
            width: bw,
            height: bh,
            ref_px: b_ref_px,
            ref_py: b_ref_py,
            anchor_x,
            anchor_y,
            scale,
            rot,
        }
    }

    /// React to the sky-link toggle: publish the flag, and when turning it on,
    /// auto-enable North-Up on the active tab and propagate its crosshair.
    fn on_link_toggled(&self, on: bool) {
        self.shared.borrow_mut().linked = on;
        if on {
            if let Some(tab) = self.current_tab() {
                if !tab.is_north_up() {
                    // Fires the North-Up handler, which rotates the active image.
                    self.north_up_btn.set_active(true);
                }
                tab.publish_current_crosshair();
                tab.apply_linked_crosshair();
            }
        }
        if let Some(tab) = self.current_tab() {
            tab.canvas().widget().queue_draw();
        }
        // Link state feeds the approximate-WCS banner (mirrors UpdateWcsSyncWarning).
        self.update_wcs_banner();
    }

    /// Basenames of every open tab (for the blink target picker).
    /// Push the open-tab list + active index into the MCP view state.
    fn publish_open_tabs(&self) {
        publish_fits_tabs(&self.tab_view, &self.tabs);
    }

    fn tab_names(&self) -> Vec<String> {
        self.tabs
            .borrow()
            .iter()
            .map(|t| basename(t.source_file()))
            .collect()
    }

    /// Basename of the tab at `idx`, if any.
    fn tab_name(&self, idx: usize) -> Option<String> {
        self.tabs
            .borrow()
            .get(idx)
            .map(|t| basename(t.source_file()))
    }

    /// Update the blink target MenuButton label to the current target tab.
    fn update_blink_target_label(&self) {
        if let Some(name) = self.tab_name(self.blink_target.get()) {
            self.blink_target_btn
                .set_label(&crate::tr_fmt!("vs {}", name));
        }
    }

    /// (Re)build the blink target picker popover from the currently-open tabs,
    /// excluding the active tab.
    fn build_blink_target_popover(&self, mb: &gtk::MenuButton) {
        let popover = gtk::Popover::new();
        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 2);
        vbox.set_margin_top(4);
        vbox.set_margin_bottom(4);
        vbox.set_margin_start(4);
        vbox.set_margin_end(4);

        let cur = self.selected_index();
        let mut any = false;
        for (i, name) in self.tab_names().into_iter().enumerate() {
            if Some(i) == cur {
                continue;
            }
            any = true;
            let btn = gtk::Button::with_label(&name);
            btn.add_css_class("flat");
            let target = self.blink_target.clone();
            let popover_c = popover.clone();
            let mb_c = mb.clone();
            btn.connect_clicked(move |_| {
                target.set(i);
                mb_c.set_label(&crate::tr_fmt!("vs {}", name));
                popover_c.popdown();
            });
            vbox.append(&btn);
        }
        if !any {
            let l = gtk::Label::new(Some(crate::tr_en!("Open another tab to blink")));
            l.add_css_class("dim-label");
            vbox.append(&l);
        }
        popover.set_child(Some(&vbox));
        mb.set_popover(Some(&popover));
    }

    /// Load a FITS file from a path (used by VOSpace integration).
    /// Open `path`, reporting why if it does not load.
    ///
    /// The MCP `open_fits_file` tool answers with this. It used to report
    /// `opened: true` for any path at all, because the request was dispatched
    /// as a fire-and-forget GTK action and nothing waited to see what happened
    /// — so an agent was told a file had opened when no tab existed. The
    /// reference's own comment on this path reads "report opened:true only on a
    /// confirmed load … not optimism".
    pub fn load_from_path(self: &Rc<Self>, path: &std::path::Path) -> Result<(), String> {
        self.load_file(path)
    }
}

// ─── Helpers ────────────────────────────────────────────────────────────────

/// The file-name portion of a path string (falls back to the whole string).
/// Push the open FITS tabs + active index into the MCP view state.
///
/// `list_open_tabs` reads that snapshot, and its setters had NO callers — so the
/// tool answered with empty arrays no matter how many files were open, an
/// advertised tool that always lied. Free-standing rather than a method because
/// the tab-close closure captures only the tab view and the tab list, not the
/// viewer (capturing the viewer there would be a reference cycle through a
/// widget the viewer owns).
fn publish_fits_tabs(tab_view: &adw::TabView, tabs: &Rc<RefCell<Vec<Rc<FitsTab>>>>) {
    let paths: Vec<String> = tabs
        .borrow()
        .iter()
        .map(|t| t.source_file().to_string())
        .collect();
    let active = tab_view
        .selected_page()
        .map(|p| tab_view.page_position(&p) as usize)
        .filter(|i| *i < paths.len());
    crate::mcp::view_state::set_open_fits(paths, active);
}

fn basename(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string())
}

fn stretch_from_index(i: u32) -> Stretch {
    match i {
        1 => Stretch::Log,
        2 => Stretch::Sqrt,
        3 => Stretch::Squared,
        4 => Stretch::Asinh,
        5 => Stretch::HistogramEq,
        _ => Stretch::Linear,
    }
}

fn stretch_to_index(s: Stretch) -> u32 {
    match s {
        Stretch::Linear => 0,
        Stretch::Log => 1,
        Stretch::Sqrt => 2,
        Stretch::Squared => 3,
        Stretch::Asinh => 4,
        Stretch::HistogramEq => 5,
    }
}

fn colormap_from_index(i: u32) -> ColorMap {
    match i {
        1 => ColorMap::Inverted,
        2 => ColorMap::Heat,
        3 => ColorMap::Viridis,
        4 => ColorMap::Plasma,
        5 => ColorMap::Inferno,
        6 => ColorMap::Magma,
        7 => ColorMap::CoolWarm,
        _ => ColorMap::Grayscale,
    }
}

fn colormap_to_index(c: ColorMap) -> u32 {
    match c {
        ColorMap::Grayscale => 0,
        ColorMap::Inverted => 1,
        ColorMap::Heat => 2,
        ColorMap::Viridis => 3,
        ColorMap::Plasma => 4,
        ColorMap::Inferno => 5,
        ColorMap::Magma => 6,
        ColorMap::CoolWarm => 7,
    }
}

// ─── String <-> enum helpers for the MCP FITS tools ──────────────────────────

/// Canonical stretch names, in the order the toolbar offers them.
///
/// `set_fits_view` advertises this list and `stretch_from_str` parses it, so it
/// is defined once. A hand-written copy in the schema is a promise that a
/// rename here would quietly break: the tool would keep offering a value the
/// viewer no longer understands, and the call would fail with "unknown stretch".
pub const STRETCH_NAMES: [&str; 6] = ["linear", "log", "sqrt", "squared", "asinh", "histogram"];

/// Canonical colormap names, same contract as [`STRETCH_NAMES`].
pub const COLORMAP_NAMES: [&str; 8] = [
    "grayscale",
    "inverted",
    "heat",
    "viridis",
    "plasma",
    "inferno",
    "magma",
    "coolwarm",
];

fn stretch_name(s: Stretch) -> &'static str {
    match s {
        Stretch::Linear => "linear",
        Stretch::Log => "log",
        Stretch::Sqrt => "sqrt",
        Stretch::Squared => "squared",
        Stretch::Asinh => "asinh",
        Stretch::HistogramEq => "histogram",
    }
}

fn stretch_from_str(s: &str) -> Option<Stretch> {
    match s.trim().to_ascii_lowercase().as_str() {
        "linear" => Some(Stretch::Linear),
        "log" => Some(Stretch::Log),
        "sqrt" => Some(Stretch::Sqrt),
        "squared" | "square" | "power" => Some(Stretch::Squared),
        "asinh" => Some(Stretch::Asinh),
        "histogram" | "histogram_eq" | "histeq" => Some(Stretch::HistogramEq),
        _ => None,
    }
}

fn colormap_name(c: ColorMap) -> &'static str {
    match c {
        ColorMap::Grayscale => "grayscale",
        ColorMap::Inverted => "inverted",
        ColorMap::Heat => "heat",
        ColorMap::Viridis => "viridis",
        ColorMap::Plasma => "plasma",
        ColorMap::Inferno => "inferno",
        ColorMap::Magma => "magma",
        ColorMap::CoolWarm => "coolwarm",
    }
}

fn colormap_from_str(c: &str) -> Option<ColorMap> {
    match c.trim().to_ascii_lowercase().as_str() {
        "grayscale" | "greyscale" | "gray" | "grey" => Some(ColorMap::Grayscale),
        "inverted" | "invert" => Some(ColorMap::Inverted),
        "heat" => Some(ColorMap::Heat),
        "viridis" => Some(ColorMap::Viridis),
        "plasma" => Some(ColorMap::Plasma),
        "inferno" => Some(ColorMap::Inferno),
        "magma" => Some(ColorMap::Magma),
        "coolwarm" | "cool" => Some(ColorMap::CoolWarm),
        _ => None,
    }
}

/// Read a FITS header keyword, trimming FITS string quoting/whitespace; `None`
/// when absent or blank.
fn header_str(header: &std::collections::HashMap<String, String>, key: &str) -> Option<String> {
    let v = header.get(key)?.trim().trim_matches('\'').trim();
    if v.is_empty() {
        None
    } else {
        Some(v.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        colormap_from_str, colormap_name, stretch_from_str, stretch_name, COLORMAP_NAMES,
        STRETCH_NAMES,
    };

    #[test]
    fn every_advertised_stretch_parses_and_comes_back_unchanged() {
        // `set_fits_view` advertises these names and the viewer parses them. A
        // name the parser rejects would be an option the tool offers and then
        // refuses; a name that parses to something with a DIFFERENT canonical
        // spelling would round-trip wrong, so get_fits_view would report a
        // stretch the caller never set.
        for name in STRETCH_NAMES {
            let parsed = stretch_from_str(name)
                .unwrap_or_else(|| panic!("`{name}` is advertised but does not parse"));
            assert_eq!(stretch_name(parsed), name, "`{name}` does not round-trip");
        }
    }

    #[test]
    fn every_advertised_colormap_parses_and_comes_back_unchanged() {
        for name in COLORMAP_NAMES {
            let parsed = colormap_from_str(name)
                .unwrap_or_else(|| panic!("`{name}` is advertised but does not parse"));
            assert_eq!(colormap_name(parsed), name, "`{name}` does not round-trip");
        }
    }

    #[test]
    fn each_advertised_name_selects_a_different_mode() {
        // Two names mapping to one mode would mean the list claims a choice the
        // viewer cannot make — and one mode would be unreachable.
        // Compared by canonical NAME, not by enum value: the modes are
        // production types that do not derive Hash/Eq, and adding those purely
        // for a test would be the wrong way round. Two names collapsing to one
        // mode yield the same canonical name, which is what this catches.
        let modes: std::collections::HashSet<&str> = STRETCH_NAMES
            .iter()
            .filter_map(|n| stretch_from_str(n))
            .map(stretch_name)
            .collect();
        assert_eq!(
            modes.len(),
            STRETCH_NAMES.len(),
            "a stretch name is a duplicate"
        );

        let maps: std::collections::HashSet<&str> = COLORMAP_NAMES
            .iter()
            .filter_map(|n| colormap_from_str(n))
            .map(colormap_name)
            .collect();
        assert_eq!(
            maps.len(),
            COLORMAP_NAMES.len(),
            "a colormap name is a duplicate"
        );
    }

    #[test]
    fn the_parser_is_more_forgiving_than_the_advertised_list() {
        // The tool advertises one canonical spelling each, but a human or an
        // agent may send a common variant; those are accepted and normalised
        // rather than refused.
        assert_eq!(stretch_from_str("SQUARE"), stretch_from_str("squared"));
        assert_eq!(stretch_from_str("histeq"), stretch_from_str("histogram"));
        assert_eq!(
            colormap_from_str("greyscale"),
            colormap_from_str("grayscale")
        );
        // And nonsense is still refused, rather than silently defaulting.
        assert!(stretch_from_str("rainbow").is_none());
        assert!(colormap_from_str("nonsense").is_none());
    }
}

#[cfg(test)]
mod control_visibility_tests {
    //! Every control is VISIBLE, with a word next to it.
    //!
    //! The rule outlived its first form. When the controls lived on a toolbar,
    //! this test checked they were on the toolbar; they live in a docked column
    //! now, like the cube viewer's, so it checks the column. What has not
    //! changed is why: this viewer once kept eleven controls inside a popover
    //! behind an unlabelled icon, every one of them working and none of them
    //! findable.

    const SOURCE: &str = include_str!("fits_viewer.rs");

    /// The part of `FitsViewer::new` that builds the control column.
    fn column_section() -> &'static str {
        let start = SOURCE
            .find("let (column, control_scroll) = viewer_shell::control_column();")
            .expect("the control column is built here");
        let end = SOURCE[start..]
            .find("// ── Approximate-WCS warning banner")
            .expect("the column is built before the banner");
        &SOURCE[start..start + end]
    }

    #[test]
    fn every_reference_affordance_is_in_the_column() {
        // Left of each pair: what the reference's toolbar shows. Right: the
        // binding we place in the column. Only `Open FITS` stays on the bar,
        // because opening a file is not a display setting.
        let expected = [
            ("Colormap", "colormap_combo"),
            ("Stretch", "stretch_combo"),
            ("Min cut", "min_scale"),
            ("Max cut", "max_scale"),
            ("Reset", "reset_btn"),
            ("North up", "north_up_btn"),
            ("Header panel", "header_expander"),
            ("Bookmarks panel", "coords_expander"),
            ("Zoom", "zoom_box"),
            ("Blink", "blink_btn"),
            ("Blink target", "blink_target_btn"),
            ("Fade speed", "blink_interval_scale"),
            ("Linked crosshair", "link_btn"),
            ("Sync zoom", "sync_fov_btn"),
            ("Copy RA/Dec", "copy_radec_btn"),
            ("Clear crosshair", "clear_crosshair_btn"),
            // The reference offers this from its crosshair menu. Ours had it
            // only inside the coordinates panel, which is closed by default —
            // so the feature existed and did not appear to.
            ("Search here", "search_here_btn"),
        ];
        let column = column_section();
        for (affordance, binding) in expected {
            assert!(
                column.contains(&format!("&{binding}")),
                "`{affordance}` is not in the control column — a control users \
                 cannot see is one they do not have"
            );
        }
    }

    #[test]
    fn every_control_carries_a_visible_label() {
        // A column earns its keep by labelling. Each of these words must appear
        // as a caption in it, not merely as a tooltip.
        for label in [
            "Colormap",
            "Stretch",
            "Min cut",
            "Max cut",
            "North up",
            "Header & image info",
            "Saved coordinates",
            "Zoom",
            "Blink",
            "Fade speed",
            "Link crosshair",
            "Sync zoom",
            "Search here",
        ] {
            let caption = format!("crate::tr_en!(\"{label}\")");
            assert!(
                column_section().contains(&caption),
                "`{label}` has no visible caption in the column"
            );
        }
    }

    #[test]
    fn no_display_control_is_left_on_the_toolbar() {
        // The bar carries the Open button and the status caption. Anything else
        // is a control that escaped the column and lost its label on the way —
        // and the escape would be silent, since it would still work.
        // The sidebar toggle earns its place: it is not a display control, it
        // is the control that reveals the display controls, and on a narrow
        // window it is the only way back to them.
        let allowed = [
            "&open_btn",
            "&spacer",
            "&status_label",
            "&shell.sidebar_toggle",
        ];
        // Tests stripped, so the scan cannot match its own mention of the call.
        let code = crate::testing::code(SOURCE);
        for (at, _) in code.match_indices("toolbar.append(") {
            let rest = &code[at + "toolbar.append(".len()..];
            let arg = rest
                .split(')')
                .next()
                .expect("an append has an argument")
                .trim();
            assert!(
                allowed.contains(&arg),
                "`{arg}` is on the toolbar; display and view controls belong in \
                 the column, where they can carry a caption"
            );
        }
    }
}

/// Whether the extension dropdown's model has to be replaced.
///
/// Split out because it is the difference between a crash and no crash, and
/// everything around it needs a realized widget to exercise. Replacing the model
/// is only safe when the list actually differs: this function is reached from
/// the dropdown's own `selected_notify`, and swapping the model there frees
/// items GTK is still emitting on.
fn model_needs_rebuild(current: Option<&[String]>, wanted: &[String]) -> bool {
    match current {
        Some(existing) => existing != wanted,
        // No model yet — it has to be built.
        None => true,
    }
}

#[cfg(test)]
mod hdu_selector_tests {
    use super::model_needs_rebuild;

    /// Switching extension within one file must not rebuild the model.
    ///
    /// This is the crash. `set_hdu_selector` is reached from the dropdown's own
    /// `selected_notify`, and replacing the model there freed the items GTK was
    /// still emitting on — a segfault, and only ever from the dropdown, since
    /// the MCP path does not run inside that signal. The HDU list cannot change
    /// when you pick a different extension of the SAME file, so in exactly the
    /// case that crashed there is nothing to rebuild.
    #[test]
    fn choosing_another_extension_of_the_same_file_rebuilds_nothing() {
        let list: Vec<String> = [
            "1: Primary (non-image)",
            "2: SCI 11471×4593",
            "3: ERR 11471×4593",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert!(
            !model_needs_rebuild(Some(&list), &list),
            "the model would be replaced from inside its own signal handler"
        );
    }

    /// A different file does need one.
    #[test]
    fn a_different_file_rebuilds_the_model() {
        let a: Vec<String> = vec!["1: SCI".into(), "2: ERR".into()];
        let b: Vec<String> = vec!["1: IMAGE".into()];
        assert!(model_needs_rebuild(Some(&a), &b));
        // Same length, different labels.
        let c: Vec<String> = vec!["1: SCI".into(), "2: WHT".into()];
        assert!(model_needs_rebuild(Some(&a), &c));
    }

    /// The first population has nothing to compare against.
    #[test]
    fn an_empty_dropdown_is_built() {
        assert!(model_needs_rebuild(None, &["1: SCI".to_string()]));
        // An existing but empty model still needs the real list.
        assert!(model_needs_rebuild(Some(&[]), &["1: SCI".to_string()]));
    }

    /// Two files whose extensions happen to be named the same are the same
    /// list, and reusing the model is correct — the labels carry dimensions,
    /// so a genuine difference shows up in them.
    #[test]
    fn identical_lists_are_treated_as_identical() {
        let a: Vec<String> = vec!["2: SCI 100×100".into()];
        let b: Vec<String> = vec!["2: SCI 100×100".into()];
        assert!(!model_needs_rebuild(Some(&a), &b));
    }
}
