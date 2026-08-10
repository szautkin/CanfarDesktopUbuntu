//! Top-level FITS viewer widget.
//!
//! Owns a unified toolbar (all image controls), a `gtk::Notebook` of FitsTabs,
//! and a collapsible `FitsCoordsPanel` on the right side. Switching tabs
//! synchronises the toolbar widgets to the newly-active tab's state.

use crate::helpers::fits_loader;
use crate::helpers::fits_renderer::{ColorMap, Stretch};
use crate::models::fits_image::{HduInfo, WcsInfo};
use crate::state::AppServices;
use crate::ui::fits_canvas::{BlinkOverlay, FitsCanvas, SharedSky, SharedSkyRef};
use crate::ui::fits_coords_panel::FitsCoordsPanel;
use crate::ui::fits_tab::FitsTab;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use libadwaita as adw;
use libadwaita::prelude::*;
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

pub struct FitsViewer {
    widget: gtk::Box,
    notebook: gtk::Notebook,
    tabs: Rc<RefCell<Vec<Rc<FitsTab>>>>,
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
    // Toolbar widgets (for tab-switch sync)
    stretch_combo: gtk::DropDown,
    colormap_combo: gtk::DropDown,
    min_scale: gtk::Scale,
    max_scale: gtk::Scale,
    zoom_combo: gtk::DropDown,
    /// Free-form zoom % entry (parses "NNN" / "NNN%" on activate).
    zoom_entry: gtk::Entry,
    north_up_btn: gtk::ToggleButton,
    header_btn: gtk::ToggleButton,
    coords_btn: gtk::ToggleButton,
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
    /// Guards the notebook `switch-page` handler during an in-place HDU swap.
    suppress_page_switch: Rc<RefCell<bool>>,
    /// Persistent sync-zoom toggle (mirrors Windows `IsSyncZoomEnabled`): when on,
    /// every tab is re-zoomed to a shared angular field as it becomes active.
    sync_zoom_enabled: Rc<Cell<bool>>,
    /// Shared angular zoom in arcsec per screen pixel (mirrors Windows
    /// `SharedAngularZoom`), captured from the active tab and re-applied to each
    /// tab on activation. `0.0` = unset.
    shared_angular_zoom: Rc<Cell<f64>>,
    /// Image A's pre-blink viewport `(tab, center_x, center_y, zoom)`, snapshotted
    /// before a blink reframes it and restored on stop (mirrors `_blinkRestore`).
    blink_restore: RefCell<Option<(Rc<FitsTab>, f64, f64, f64)>>,
}

impl FitsViewer {
    pub fn new(_services: Arc<AppServices>) -> Rc<Self> {
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        widget.set_vexpand(true);
        widget.set_hexpand(true);

        // ── Toolbar ──────────────────────────────────────────────────────────
        // GNOME HIG: frequent controls inline — Open, stretch, colormap, zoom,
        // blink — with everything else grouped in one "Display options" popover
        // of boxed-list rows.
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

        toolbar.append(&gtk::Separator::new(gtk::Orientation::Vertical));

        // Stretch + colormap stay inline — the viewer's signature display controls.
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
        stretch_combo.set_tooltip_text(Some(crate::tr_en!("Stretch")));
        toolbar.append(&stretch_combo);

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
        colormap_combo.set_tooltip_text(Some(crate::tr_en!("Colormap")));
        toolbar.append(&colormap_combo);

        toolbar.append(&gtk::Separator::new(gtk::Orientation::Vertical));

        // Zoom cluster: preset dropdown + free-form % entry, visually linked.
        let zoom_items = gtk::StringList::new(
            &ZOOM_PRESETS
                .iter()
                .map(|(_, l)| *l)
                .collect::<Vec<&str>>(),
        );
        let zoom_combo = gtk::DropDown::new(Some(zoom_items), gtk::Expression::NONE);
        zoom_combo.set_selected(3); // 100%
        zoom_combo.set_tooltip_text(Some(crate::tr_en!("Zoom preset")));
        let zoom_entry = gtk::Entry::new();
        zoom_entry.set_width_chars(5);
        zoom_entry.set_max_width_chars(6);
        zoom_entry.set_text("100");
        zoom_entry.set_tooltip_text(Some(crate::tr_en!("Type a zoom % and press Enter")));
        let zoom_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        zoom_box.add_css_class("linked");
        zoom_box.append(&zoom_combo);
        zoom_box.append(&zoom_entry);
        toolbar.append(&zoom_box);

        toolbar.append(&gtk::Separator::new(gtk::Orientation::Vertical));

        // Blink toggle — the viewer's signature compare mode, kept inline.
        let blink_btn = gtk::ToggleButton::new();
        blink_btn.set_icon_name("media-playlist-repeat-symbolic");
        blink_btn.add_css_class("flat");
        blink_btn.set_tooltip_text(Some(crate::tr_en!(
            "Cross-fade blink against another tab (Space pause · Left/Right show A/B · Esc stop)"
        )));
        toolbar.append(&blink_btn);

        // ── "Display options" popover ────────────────────────────────────────
        let min_scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 0.01);
        min_scale.set_width_request(160);
        min_scale.set_draw_value(false);
        min_scale.set_valign(gtk::Align::Center);

        let max_scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 0.01);
        max_scale.set_width_request(160);
        max_scale.set_draw_value(false);
        max_scale.set_valign(gtk::Align::Center);

        let reset_btn = gtk::Button::with_label(crate::tr_en!("Reset stretch"));
        reset_btn.add_css_class("flat");
        reset_btn.set_halign(gtk::Align::End);

        let header_btn = gtk::ToggleButton::new();
        header_btn.set_icon_name("view-list-symbolic");
        header_btn.add_css_class("flat");
        header_btn.set_valign(gtk::Align::Center);
        header_btn.set_tooltip_text(Some(crate::tr_en!("Toggle FITS header panel")));

        let north_up_btn = gtk::ToggleButton::new();
        north_up_btn.set_icon_name("go-up-symbolic");
        north_up_btn.add_css_class("flat");
        north_up_btn.set_valign(gtk::Align::Center);
        north_up_btn.set_tooltip_text(Some(crate::tr_en!("Rotate so north is up")));

        let link_btn = gtk::ToggleButton::new();
        link_btn.set_icon_name("insert-link-symbolic");
        link_btn.add_css_class("flat");
        link_btn.set_valign(gtk::Align::Center);
        link_btn.set_active(true);
        link_btn.set_tooltip_text(Some(crate::tr_en!(
            "Link crosshair across tabs by sky position (auto-enables North Up)"
        )));

        let coords_btn = gtk::ToggleButton::new();
        coords_btn.set_icon_name("starred-symbolic");
        coords_btn.add_css_class("flat");
        coords_btn.set_valign(gtk::Align::Center);
        coords_btn.set_tooltip_text(Some(crate::tr_en!("Toggle saved coordinates panel")));

        let blink_target_btn = gtk::MenuButton::new();
        blink_target_btn.set_label(crate::tr_en!("vs…"));
        blink_target_btn.set_valign(gtk::Align::Center);
        blink_target_btn.set_tooltip_text(Some(crate::tr_en!("Choose the tab to blink against")));

        let blink_interval_scale =
            gtk::Scale::with_range(gtk::Orientation::Horizontal, 500.0, 5000.0, 100.0);
        blink_interval_scale.set_width_request(140);
        blink_interval_scale.set_value(1500.0);
        blink_interval_scale.set_draw_value(true);
        blink_interval_scale.set_value_pos(gtk::PositionType::Right);
        blink_interval_scale.set_valign(gtk::Align::Center);
        blink_interval_scale.set_tooltip_text(Some(crate::tr_en!("Blink fade interval (ms)")));

        let sync_fov_btn = gtk::ToggleButton::new();
        sync_fov_btn.set_icon_name("zoom-fit-best-symbolic");
        sync_fov_btn.add_css_class("flat");
        sync_fov_btn.set_tooltip_text(Some(crate::tr_en!(
            "Sync zoom across tabs — match the current image's angular field (re-applied as you switch tabs)"
        )));

        let copy_radec_btn = gtk::Button::from_icon_name("edit-copy-symbolic");
        copy_radec_btn.add_css_class("flat");
        copy_radec_btn.set_tooltip_text(Some(crate::tr_en!("Copy crosshair RA/Dec to clipboard")));

        let clear_crosshair_btn = gtk::Button::from_icon_name("edit-clear-symbolic");
        clear_crosshair_btn.add_css_class("flat");
        clear_crosshair_btn.set_tooltip_text(Some(crate::tr_en!("Clear crosshair")));

        let group_label = |text: &str| {
            let l = gtk::Label::new(Some(text));
            l.add_css_class("caption-heading");
            l.set_xalign(0.0);
            l
        };
        let action_row = |title: &str, subtitle: Option<&str>, suffix: &gtk::Widget| {
            let row = adw::ActionRow::new();
            row.set_title(title);
            if let Some(s) = subtitle {
                row.set_subtitle(s);
            }
            row.add_suffix(suffix);
            row
        };
        let boxed_list = || {
            let list = gtk::ListBox::new();
            list.set_selection_mode(gtk::SelectionMode::None);
            list.add_css_class("boxed-list");
            list
        };

        let levels_list = boxed_list();
        levels_list.append(&action_row(crate::tr_en!("Min cut"), None, min_scale.upcast_ref()));
        levels_list.append(&action_row(crate::tr_en!("Max cut"), None, max_scale.upcast_ref()));

        let view_list = boxed_list();
        view_list.append(&action_row(
            crate::tr_en!("Header panel"),
            None,
            header_btn.upcast_ref(),
        ));
        view_list.append(&action_row(
            crate::tr_en!("North up"),
            None,
            north_up_btn.upcast_ref(),
        ));
        view_list.append(&action_row(
            crate::tr_en!("Link crosshair across tabs"),
            Some(crate::tr_en!("Also enables North Up")),
            link_btn.upcast_ref(),
        ));
        view_list.append(&action_row(
            crate::tr_en!("Coordinates panel"),
            None,
            coords_btn.upcast_ref(),
        ));

        let blink_list = boxed_list();
        blink_list.append(&action_row(
            crate::tr_en!("Compare against"),
            None,
            blink_target_btn.upcast_ref(),
        ));
        blink_list.append(&action_row(
            crate::tr_en!("Fade speed"),
            None,
            blink_interval_scale.upcast_ref(),
        ));

        let tools_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        tools_box.add_css_class("linked");
        tools_box.set_halign(gtk::Align::Start);
        tools_box.append(&sync_fov_btn);
        tools_box.append(&copy_radec_btn);
        tools_box.append(&clear_crosshair_btn);

        let display_box = gtk::Box::new(gtk::Orientation::Vertical, 12);
        display_box.set_margin_start(12);
        display_box.set_margin_end(12);
        display_box.set_margin_top(12);
        display_box.set_margin_bottom(12);
        display_box.set_size_request(300, -1);
        display_box.append(&group_label(crate::tr_en!("Levels")));
        display_box.append(&levels_list);
        display_box.append(&reset_btn);
        display_box.append(&group_label(crate::tr_en!("View")));
        display_box.append(&view_list);
        display_box.append(&group_label(crate::tr_en!("Blink")));
        display_box.append(&blink_list);
        display_box.append(&group_label(crate::tr_en!("Crosshair tools")));
        display_box.append(&tools_box);

        let display_pop = gtk::Popover::new();
        display_pop.set_child(Some(&display_box));
        let display_btn = gtk::MenuButton::new();
        display_btn.set_icon_name("preferences-desktop-display-symbolic");
        display_btn.add_css_class("flat");
        display_btn.set_tooltip_text(Some(crate::tr_en!("Display options")));
        display_btn.set_popover(Some(&display_pop));
        toolbar.append(&display_btn);

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

        // ── Main area: notebook (center) + coords panel (right) ─────────────
        let body = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        body.set_vexpand(true);
        body.set_hexpand(true);

        let notebook = gtk::Notebook::new();
        notebook.set_vexpand(true);
        notebook.set_hexpand(true);
        notebook.set_scrollable(true);
        notebook.set_show_border(false);

        // Empty state (HIG: StatusPage with a primary call to action)
        let empty_open_btn = gtk::Button::with_label(crate::tr_en!("Open FITS…"));
        empty_open_btn.add_css_class("suggested-action");
        empty_open_btn.add_css_class("pill");
        empty_open_btn.set_halign(gtk::Align::Center);
        let empty_status = adw::StatusPage::new();
        empty_status.set_icon_name(Some("image-x-generic-symbolic"));
        empty_status.set_title(crate::tr_en!("No FITS File Open"));
        empty_status.set_description(Some(crate::tr_en!("Open a FITS file to get started")));
        empty_status.set_child(Some(&empty_open_btn));
        notebook.append_page(&empty_status, Some(&gtk::Label::new(Some(crate::tr_en!("Welcome")))));

        body.append(&notebook);
        body.append(&gtk::Separator::new(gtk::Orientation::Vertical));

        let coords_panel = FitsCoordsPanel::new();
        body.append(coords_panel.widget());

        widget.append(&body);

        let viewer = Rc::new(FitsViewer {
            widget,
            notebook,
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
            stretch_combo,
            colormap_combo,
            min_scale,
            max_scale,
            zoom_combo,
            zoom_entry,
            north_up_btn,
            header_btn,
            coords_btn,
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
            sync_zoom_enabled: Rc::new(Cell::new(false)),
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
                    v.sync_toolbar_to_tab(&tab);
                }
            });
        }
        {
            let v = viewer.clone();
            viewer.header_btn.connect_toggled(move |_| {
                if *v.suppress_sync.borrow() {
                    return;
                }
                if let Some(tab) = v.current_tab() {
                    tab.toggle_header();
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
            viewer.coords_btn.connect_toggled(move |_| {
                if *v.suppress_sync.borrow() {
                    return;
                }
                v.coords_panel.toggle();
            });
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
                            v.sync_toolbar_to_tab(&tab);
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
                v.switch_hdu(info.index);
            });
        }

        // Tab switch → sync toolbar to the newly-active tab
        {
            let v = viewer.clone();
            viewer
                .notebook
                .connect_switch_page(move |_, _page, page_idx| {
                    if *v.suppress_page_switch.borrow() {
                        return;
                    }
                    // page_idx is 0-based over real pages; tab indices map directly
                    // once the welcome page has been removed.
                    let tab = v.tabs.borrow().get(page_idx as usize).cloned();
                    if let Some(tab) = tab {
                        // Apply the shared view FIRST (mirrors ApplySharedViewToActivePage):
                        // reposition the linked crosshair onto this tab's sky, then match the
                        // shared angular zoom, THEN sync the toolbar so the zoom % reflects it.
                        tab.apply_linked_crosshair();
                        v.apply_shared_view_to_active(&tab);
                        v.sync_toolbar_to_tab(&tab);
                        v.update_hdu_and_banner(&tab);
                    }
                });
        }

        // Wire coords panel → active tab
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
            "get_fits_view" => {
                let tab = self.current_tab().ok_or_else(|| "no FITS open".to_string())?;
                Ok(self.fits_view_state(&tab))
            }
            "set_fits_view" => {
                let tab = self.current_tab().ok_or_else(|| "no FITS open".to_string())?;

                if args.get("reset").and_then(|v| v.as_bool()).unwrap_or(false) {
                    tab.reset_stretch();
                    tab.reset_view();
                }
                if let Some(s) = args.get("stretch").and_then(|v| v.as_str()) {
                    tab.set_stretch(stretch_from_str(s).ok_or_else(|| format!("unknown stretch '{s}'"))?);
                }
                if let Some(c) = args.get("colormap").and_then(|v| v.as_str()) {
                    tab.set_colormap(colormap_from_str(c).ok_or_else(|| format!("unknown colormap '{c}'"))?);
                }
                if let Some(v) = args.get("min_cut").and_then(|v| v.as_f64()) {
                    tab.set_vmin(v);
                }
                if let Some(v) = args.get("max_cut").and_then(|v| v.as_f64()) {
                    tab.set_vmax(v);
                }
                if let Some(z) = args.get("zoom").and_then(|v| v.as_f64()) {
                    tab.set_zoom(z / 100.0);
                }
                if let Some(n) = args.get("north_up").and_then(|v| v.as_bool()) {
                    tab.set_north_up(n);
                }
                // Centre is applied after zoom so the pan maths uses the new scale.
                let (cur_cx, cur_cy) = tab.viewport_center();
                let cx = args.get("center_x").and_then(|v| v.as_f64());
                let cy = args.get("center_y").and_then(|v| v.as_f64());
                if cx.is_some() || cy.is_some() {
                    tab.set_viewport_center(cx.unwrap_or(cur_cx), cy.unwrap_or(cur_cy));
                }
                self.sync_toolbar_to_tab(&tab);
                Ok(self.fits_view_state(&tab))
            }
            "probe_fits_pixel" => {
                let tab = self.current_tab().ok_or_else(|| "no FITS open".to_string())?;
                let x = args.get("x").and_then(|v| v.as_i64()).ok_or_else(|| "x is required".to_string())?;
                let y = args.get("y").and_then(|v| v.as_i64()).ok_or_else(|| "y is required".to_string())?;
                if x < 0 || y < 0 {
                    return Err("x and y must be >= 0".into());
                }
                let data = tab.data();
                let value = data
                    .pixel_at(x as usize, y as usize)
                    .ok_or_else(|| format!("pixel ({x}, {y}) is out of range ({}×{})", data.width, data.height))?;
                let mut out = json!({ "x": x, "y": y, "value": value, "has_wcs": false });
                if let Some(w) = data.wcs.as_ref() {
                    let (ra, dec) = w.pixel_to_sky(x as f64, y as f64);
                    out["has_wcs"] = json!(true);
                    out["ra"] = json!(ra);
                    out["dec"] = json!(dec);
                }
                if let Some(u) = header_str(&data.header, "BUNIT") {
                    out["unit"] = json!(u);
                }
                Ok(out)
            }
            "fits_goto_coordinate" => {
                let tab = self.current_tab().ok_or_else(|| "no FITS open".to_string())?;
                let ra = args.get("ra").and_then(|v| v.as_f64()).ok_or_else(|| "ra is required".to_string())?;
                let dec = args.get("dec").and_then(|v| v.as_f64()).ok_or_else(|| "dec is required".to_string())?;
                let data = tab.data();
                let wcs = data.wcs.as_ref().ok_or_else(|| "the loaded FITS has no WCS".to_string())?;
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
                        self.sync_toolbar_to_tab(&tab);
                        Ok(json!({
                            "moved": true,
                            "ra": ra,
                            "dec": dec,
                            "pixel_x": px,
                            "pixel_y": py,
                            "in_bounds": in_bounds,
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
            "list_fits_bookmark" => {
                let items = self.bookmarks.borrow();
                Ok(json!({ "count": items.len(), "bookmarks": *items }))
            }
            "save_fits_bookmark" => {
                let name = args
                    .get("name")
                    .and_then(|v| v.as_str())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| "name is required".to_string())?;

                // Prefer explicit ra/dec; otherwise capture the active tab's crosshair.
                let ra = args.get("ra").and_then(|v| v.as_f64());
                let dec = args.get("dec").and_then(|v| v.as_f64());
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

                let bm = FitsBookmark { name: name.clone(), ra, dec, source_file };
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
                let name = args
                    .get("name")
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
            "file_name": file_name,
            "source_path": tab.source_file(),
            "hdu_name": header_str(&data.header, "EXTNAME"),
            "width": data.width,
            "height": data.height,
            "zoom_percent": tab.zoom_scale() * 100.0,
            "center_x": cx,
            "center_y": cy,
            "stretch": stretch_name(tab.stretch()),
            "colormap": colormap_name(tab.colormap()),
            "min_cut": tab.vmin(),
            "max_cut": tab.vmax(),
            "north_up": tab.is_north_up(),
            "has_wcs": has_wcs,
            "crosshair_placed": tab.crosshair_pixel_pos().is_some(),
            "crosshair_ra": crosshair.map(|(ra, _)| ra),
            "crosshair_dec": crosshair.map(|(_, dec)| dec),
        })
    }

    /// Register a callback for the coords-panel "Search Here" button — invoked
    /// with the crosshair's `(ra, dec)` in degrees.
    pub fn set_on_search_here(&self, cb: impl Fn(f64, f64) + 'static) {
        self.coords_panel.set_on_search_here(cb);
    }

    fn current_tab(&self) -> Option<Rc<FitsTab>> {
        let idx = self.notebook.current_page()?;
        self.tabs.borrow().get(idx as usize).cloned()
    }

    /// Sync all toolbar widgets to the given tab's current state.
    fn sync_toolbar_to_tab(&self, tab: &Rc<FitsTab>) {
        *self.suppress_sync.borrow_mut() = true;

        self.stretch_combo
            .set_selected(stretch_to_index(tab.stretch()));
        self.colormap_combo
            .set_selected(colormap_to_index(tab.colormap()));

        // Update the min/max scale range to the image's own extrema
        let data_min = tab.data_min();
        let data_max = tab.data_max();
        let step = ((data_max - data_min) / 200.0).max(1e-6);
        self.min_scale
            .set_range(data_min, data_max);
        self.min_scale.set_increments(step, step * 10.0);
        self.min_scale.set_value(tab.vmin());
        self.max_scale
            .set_range(data_min, data_max);
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

    async fn open_file_dialog(&self, parent: &impl IsA<gtk::Widget>) {
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
                self.load_file(&path);
            }
        }
    }

    fn load_file(&self, path: &std::path::Path) {
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

                // Remove welcome page if it's the first real tab
                if self.tabs.borrow().is_empty() && self.notebook.n_pages() > 0 {
                    self.notebook.remove_page(Some(0));
                }

                // Add close button to tab label
                let tab_box = gtk::Box::new(gtk::Orientation::Horizontal, 4);
                let tab_label = gtk::Label::new(Some(&filename));
                tab_box.append(&tab_label);
                let close_btn = gtk::Button::from_icon_name("window-close-symbolic");
                close_btn.add_css_class("flat");
                close_btn.add_css_class("circular");
                tab_box.append(&close_btn);

                let page_num = self.notebook.append_page(tab.widget(), Some(&tab_box));
                self.notebook.set_current_page(Some(page_num));

                // Close button handler
                let notebook = self.notebook.clone();
                let tabs = self.tabs.clone();
                let tab_box_clone = tab_box.clone();
                close_btn.connect_clicked(move |_| {
                    let n = notebook.n_pages();
                    for i in 0..n {
                        if let Some(page) = notebook.nth_page(Some(i)) {
                            if let Some(tab_label_widget) = notebook.tab_label(&page) {
                                if tab_label_widget.eq(&tab_box_clone) {
                                    notebook.remove_page(Some(i));
                                    let mut t = tabs.borrow_mut();
                                    if (i as usize) < t.len() {
                                        t.remove(i as usize);
                                    }
                                    break;
                                }
                            }
                        }
                    }
                });

                self.tabs.borrow_mut().push(tab.clone());

                // Sync toolbar + extension selector + WCS banner to the new tab
                self.sync_toolbar_to_tab(&tab);
                self.update_hdu_and_banner(&tab);
            }
            Err(e) => {
                self.status_label.set_text(&crate::tr_fmt!("Error: {}", e));
            }
        }
    }

    /// Wire a freshly-built tab's crosshair callback: update the coords readout
    /// and, when zoomed in (>1.05×), recenter the viewport on the placed pixel
    /// (mirrors the Windows `PlaceCrosshair` / `CenterOnImagePixel` behaviour so
    /// go-to and right-click targets stay on-screen).
    fn wire_tab_callbacks(&self, tab: &Rc<FitsTab>) {
        let coords_panel = self.coords_panel.clone();
        let wcs = tab.data().wcs.clone();
        // Weak ref avoids a canvas→callback→canvas reference cycle.
        let canvas_weak = Rc::downgrade(tab.canvas());
        tab.canvas().set_on_crosshair_placed(move |pos| {
            coords_panel.set_current_crosshair(pos, wcs.as_ref());
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
        let model = gtk::StringList::new(&[]);
        for h in hdus {
            model.append(&h.label());
        }
        *self.hdu_infos.borrow_mut() = hdus.to_vec();
        self.hdu_dropdown.set_model(Some(&model));
        let pos = hdus
            .iter()
            .position(|h| h.index == selected_index)
            .unwrap_or(0) as u32;
        self.hdu_dropdown.set_selected(pos);
        *self.hdu_current_pos.borrow_mut() = pos;
        *self.suppress_sync.borrow_mut() = false;

        self.hdu_bar.set_visible(true);
    }

    /// Reload a different image HDU of the active tab's file, replacing the
    /// current notebook page's content in place (mirrors Windows `SelectHdu`).
    fn switch_hdu(&self, hdu_index: usize) {
        let page_idx = match self.notebook.current_page() {
            Some(i) => i,
            None => return,
        };
        let old_tab = match self.tabs.borrow().get(page_idx as usize).cloned() {
            Some(t) => t,
            None => return,
        };
        if old_tab.hdu_index() == hdu_index {
            return;
        }

        let path_str = old_tab.source_file().to_string();
        let path = std::path::Path::new(&path_str);
        let data = match fits_loader::load_fits_image_hdu(path, hdu_index) {
            Ok(d) => d,
            Err(e) => {
                self.status_label.set_text(&crate::tr_fmt!("Error: {}", e));
                // Revert the dropdown to the still-displayed HDU.
                self.set_hdu_selector(&old_tab.hdus(), old_tab.hdu_index());
                return;
            }
        };

        self.status_label.set_text(&fits_loader::fits_summary(&data));

        let new_tab = FitsTab::new(data, self.shared.clone(), path_str.clone());
        new_tab.set_hdu_context(old_tab.hdus(), hdu_index);
        self.wire_tab_callbacks(&new_tab);

        // Replace the page child in place, reusing the same tab label (so the
        // existing close-button handler keeps matching this page).
        let tab_label = self.notebook.tab_label(old_tab.widget());
        *self.suppress_page_switch.borrow_mut() = true;
        self.notebook.remove_page(Some(page_idx));
        let new_pos =
            self.notebook
                .insert_page(new_tab.widget(), tab_label.as_ref(), Some(page_idx));
        if let Some(slot) = self.tabs.borrow_mut().get_mut(page_idx as usize) {
            *slot = new_tab.clone();
        }
        self.notebook.set_current_page(Some(new_pos));
        *self.suppress_page_switch.borrow_mut() = false;

        self.sync_toolbar_to_tab(&new_tab);
        self.update_hdu_and_banner(&new_tab);
    }

    /// Begin a cross-fade blink: overlay the target tab (B) onto the active tab
    /// (A) and oscillate its opacity so A fades into B and back. Replaces the old
    /// hard page-flip. The two frames are aligned on a shared sky point at a
    /// matched angular scale + orientation before the overlay is built.
    fn start_blink(&self) {
        // Resolve A (active) and B (target) tabs.
        let a_idx = match self.notebook.current_page() {
            Some(i) => i as usize,
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
            .and_then(|(ra, dec)| a.data().wcs.as_ref().and_then(|w| w.world_to_pixel(ra, dec)))
            .unwrap_or((aw as f64 / 2.0, ah as f64 / 2.0));
        let (b_ref_px, b_ref_py) = ref_sky
            .and_then(|(ra, dec)| b.data().wcs.as_ref().and_then(|w| w.world_to_pixel(ra, dec)))
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
    fn tab_names(&self) -> Vec<String> {
        self.tabs
            .borrow()
            .iter()
            .map(|t| basename(t.source_file()))
            .collect()
    }

    /// Basename of the tab at `idx`, if any.
    fn tab_name(&self, idx: usize) -> Option<String> {
        self.tabs.borrow().get(idx).map(|t| basename(t.source_file()))
    }

    /// Update the blink target MenuButton label to the current target tab.
    fn update_blink_target_label(&self) {
        if let Some(name) = self.tab_name(self.blink_target.get()) {
            self.blink_target_btn.set_label(&crate::tr_fmt!("vs {}", name));
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

        let cur = self.notebook.current_page().map(|i| i as usize);
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
    pub fn load_from_path(&self, path: &std::path::Path) {
        self.load_file(path);
    }
}

// ─── Helpers ────────────────────────────────────────────────────────────────

/// The file-name portion of a path string (falls back to the whole string).
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