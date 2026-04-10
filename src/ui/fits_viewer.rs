//! Top-level FITS viewer widget.
//!
//! Owns a unified toolbar (all image controls), a `gtk::Notebook` of FitsTabs,
//! and a collapsible `FitsCoordsPanel` on the right side. Switching tabs
//! synchronises the toolbar widgets to the newly-active tab's state.

use crate::helpers::fits_loader;
use crate::helpers::fits_renderer::{ColorMap, Stretch};
use crate::state::AppServices;
use crate::ui::fits_coords_panel::FitsCoordsPanel;
use crate::ui::fits_tab::FitsTab;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{self as gtk};

use std::cell::RefCell;
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

pub struct FitsViewer {
    widget: gtk::Box,
    notebook: gtk::Notebook,
    tabs: Rc<RefCell<Vec<Rc<FitsTab>>>>,
    shared_cursor: Rc<RefCell<Option<(f64, f64)>>>,
    status_label: gtk::Label,
    blink_active: Rc<RefCell<bool>>,
    coords_panel: Rc<FitsCoordsPanel>,
    // Toolbar widgets (for tab-switch sync)
    stretch_combo: gtk::DropDown,
    colormap_combo: gtk::DropDown,
    min_scale: gtk::Scale,
    max_scale: gtk::Scale,
    zoom_combo: gtk::DropDown,
    north_up_btn: gtk::ToggleButton,
    header_btn: gtk::ToggleButton,
    coords_btn: gtk::ToggleButton,
    /// Prevents feedback loops when syncing toolbar widgets on tab switch.
    suppress_sync: Rc<RefCell<bool>>,
}

impl FitsViewer {
    pub fn new(_services: Arc<AppServices>) -> Rc<Self> {
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        widget.set_vexpand(true);
        widget.set_hexpand(true);

        // ── Toolbar ──────────────────────────────────────────────────────────
        let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        toolbar.set_margin_start(8);
        toolbar.set_margin_end(8);
        toolbar.set_margin_top(6);
        toolbar.set_margin_bottom(6);

        // Open
        let open_btn = gtk::Button::with_label("Open FITS");
        open_btn.set_icon_name("document-open-symbolic");
        open_btn.add_css_class("suggested-action");
        toolbar.append(&open_btn);

        toolbar.append(&gtk::Separator::new(gtk::Orientation::Vertical));

        // Stretch dropdown
        toolbar.append(&gtk::Label::new(Some("Stretch:")));
        let stretch_items = gtk::StringList::new(&[
            "Linear",
            "Log",
            "Sqrt",
            "Squared",
            "Asinh",
            "Histogram Eq",
        ]);
        let stretch_combo = gtk::DropDown::new(Some(stretch_items), gtk::Expression::NONE);
        stretch_combo.set_selected(0);
        toolbar.append(&stretch_combo);

        // Colormap dropdown
        toolbar.append(&gtk::Label::new(Some("Color:")));
        let cmap_items = gtk::StringList::new(&[
            "Grayscale",
            "Inverted",
            "Heat",
            "Viridis",
            "Plasma",
            "Inferno",
            "Magma",
            "CoolWarm",
        ]);
        let colormap_combo = gtk::DropDown::new(Some(cmap_items), gtk::Expression::NONE);
        colormap_combo.set_selected(0);
        toolbar.append(&colormap_combo);

        toolbar.append(&gtk::Separator::new(gtk::Orientation::Vertical));

        // Min/Max sliders
        toolbar.append(&gtk::Label::new(Some("Min:")));
        let min_scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 0.01);
        min_scale.set_width_request(100);
        min_scale.set_draw_value(false);
        toolbar.append(&min_scale);

        toolbar.append(&gtk::Label::new(Some("Max:")));
        let max_scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 0.01);
        max_scale.set_width_request(100);
        max_scale.set_draw_value(false);
        toolbar.append(&max_scale);

        // Reset stretch
        let reset_btn = gtk::Button::from_icon_name("view-refresh-symbolic");
        reset_btn.add_css_class("flat");
        reset_btn.set_tooltip_text(Some("Reset stretch"));
        toolbar.append(&reset_btn);

        toolbar.append(&gtk::Separator::new(gtk::Orientation::Vertical));

        // Header panel toggle
        let header_btn = gtk::ToggleButton::new();
        header_btn.set_icon_name("view-list-symbolic");
        header_btn.add_css_class("flat");
        header_btn.set_tooltip_text(Some("Toggle FITS header panel"));
        toolbar.append(&header_btn);

        // North Up toggle
        let north_up_btn = gtk::ToggleButton::new();
        north_up_btn.set_icon_name("go-up-symbolic");
        north_up_btn.add_css_class("flat");
        north_up_btn.set_tooltip_text(Some("Rotate so north is up"));
        toolbar.append(&north_up_btn);

        // Blink toggle
        let blink_btn = gtk::ToggleButton::new();
        blink_btn.set_icon_name("media-playlist-repeat-symbolic");
        blink_btn.add_css_class("flat");
        blink_btn.set_tooltip_text(Some("Blink comparison between first two tabs"));
        toolbar.append(&blink_btn);

        // Saved coordinates panel toggle
        let coords_btn = gtk::ToggleButton::new();
        coords_btn.set_icon_name("starred-symbolic");
        coords_btn.add_css_class("flat");
        coords_btn.set_tooltip_text(Some("Toggle saved coordinates panel"));
        toolbar.append(&coords_btn);

        toolbar.append(&gtk::Separator::new(gtk::Orientation::Vertical));

        // Zoom preset combo
        toolbar.append(&gtk::Label::new(Some("Zoom:")));
        let zoom_items = gtk::StringList::new(
            &ZOOM_PRESETS
                .iter()
                .map(|(_, l)| *l)
                .collect::<Vec<&str>>(),
        );
        let zoom_combo = gtk::DropDown::new(Some(zoom_items), gtk::Expression::NONE);
        zoom_combo.set_selected(3); // 100%
        toolbar.append(&zoom_combo);

        // Spacer + status
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        toolbar.append(&spacer);

        let status_label = gtk::Label::new(Some("No file loaded"));
        status_label.add_css_class("dim-label");
        status_label.add_css_class("caption");
        toolbar.append(&status_label);

        widget.append(&toolbar);
        widget.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

        // ── Main area: notebook (center) + coords panel (right) ─────────────
        let body = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        body.set_vexpand(true);
        body.set_hexpand(true);

        let notebook = gtk::Notebook::new();
        notebook.set_vexpand(true);
        notebook.set_hexpand(true);
        notebook.set_scrollable(true);
        notebook.set_show_border(false);

        // Empty state
        let empty_box = gtk::Box::new(gtk::Orientation::Vertical, 12);
        empty_box.set_valign(gtk::Align::Center);
        empty_box.set_halign(gtk::Align::Center);
        let empty_icon = gtk::Image::from_icon_name("image-x-generic-symbolic");
        empty_icon.set_pixel_size(64);
        empty_icon.add_css_class("dim-label");
        empty_box.append(&empty_icon);
        let empty_label = gtk::Label::new(Some("Open a FITS file to get started"));
        empty_label.add_css_class("dim-label");
        empty_box.append(&empty_label);
        notebook.append_page(&empty_box, Some(&gtk::Label::new(Some("Welcome"))));

        body.append(&notebook);
        body.append(&gtk::Separator::new(gtk::Orientation::Vertical));

        let coords_panel = FitsCoordsPanel::new();
        body.append(coords_panel.widget());

        widget.append(&body);

        let viewer = Rc::new(FitsViewer {
            widget,
            notebook,
            tabs: Rc::new(RefCell::new(Vec::new())),
            shared_cursor: Rc::new(RefCell::new(None)),
            status_label,
            blink_active: Rc::new(RefCell::new(false)),
            coords_panel,
            stretch_combo,
            colormap_combo,
            min_scale,
            max_scale,
            zoom_combo,
            north_up_btn,
            header_btn,
            coords_btn,
            suppress_sync: Rc::new(RefCell::new(false)),
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
            blink_btn.connect_toggled(move |btn| {
                *v.blink_active.borrow_mut() = btn.is_active();
                if btn.is_active() {
                    v.start_blink();
                }
            });
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
                    }
                }
            });
        }

        // Tab switch → sync toolbar to the newly-active tab
        {
            let v = viewer.clone();
            viewer
                .notebook
                .connect_switch_page(move |_, _page, page_idx| {
                    // page_idx is 0-based over real pages; tab indices map directly
                    // once the welcome page has been removed.
                    let tabs = v.tabs.borrow();
                    if let Some(tab) = tabs.get(page_idx as usize) {
                        v.sync_toolbar_to_tab(tab);
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

        // Zoom: find the closest preset
        let current_pct = (tab.zoom_scale() * 100.0).round() as i32;
        let closest = ZOOM_PRESETS
            .iter()
            .position(|(p, _)| *p == current_pct)
            .unwrap_or(3); // default to 100%
        self.zoom_combo.set_selected(closest as u32);

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
        filter.set_name(Some("FITS Images"));

        let filters = gtk4::gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);

        let dialog = gtk::FileDialog::builder()
            .title("Open FITS File")
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

                let tab = FitsTab::new(data, self.shared_cursor.clone(), source);

                // Wire crosshair callback to update coords panel
                {
                    let coords_panel = self.coords_panel.clone();
                    let wcs = tab.data().wcs.clone();
                    tab.canvas()
                        .set_on_crosshair_placed(move |pos| {
                            coords_panel.set_current_crosshair(pos, wcs.as_ref());
                        });
                }

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

                // Sync toolbar to the new tab
                self.sync_toolbar_to_tab(&tab);
            }
            Err(e) => {
                self.status_label.set_text(&format!("Error: {}", e));
            }
        }
    }

    fn start_blink(&self) {
        let notebook = self.notebook.clone();
        let blink_active = self.blink_active.clone();

        glib::timeout_add_local(std::time::Duration::from_millis(500), move || {
            if !*blink_active.borrow() {
                return glib::ControlFlow::Break;
            }
            if notebook.n_pages() < 2 {
                return glib::ControlFlow::Break;
            }
            let current = notebook.current_page().unwrap_or(0);
            let next = if current == 0 { 1 } else { 0 };
            notebook.set_current_page(Some(next));
            glib::ControlFlow::Continue
        });
    }

    /// Load a FITS file from a path (used by VOSpace integration).
    pub fn load_from_path(&self, path: &std::path::Path) {
        self.load_file(path);
    }
}

// ─── Helpers ────────────────────────────────────────────────────────────────

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
