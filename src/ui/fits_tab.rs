//! A single FITS image tab — combines the image canvas with a collapsible
//! FITS header panel. All toolbar controls live in the parent `FitsViewer`.
//!
//! State (stretch / colormap / vmin / vmax / rotation) is owned per-tab so
//! that switching tabs can sync the shared toolbar to the newly-active tab.

use crate::helpers::fits_renderer::{self, ColorMap, Stretch};
use crate::models::FitsImageData;
use crate::ui::fits_canvas::FitsCanvas;
use crate::ui::fits_header_panel::FitsHeaderPanel;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use std::cell::RefCell;
use std::rc::Rc;

pub struct FitsTab {
    widget: gtk::Box,
    canvas: Rc<FitsCanvas>,
    header_panel: Rc<FitsHeaderPanel>,
    data: Rc<FitsImageData>,
    /// Current render parameters, per-tab.
    stretch: RefCell<Stretch>,
    colormap: RefCell<ColorMap>,
    vmin: RefCell<f64>,
    vmax: RefCell<f64>,
    /// Percentile-cut defaults (for Reset Stretch).
    auto_vmin: f64,
    auto_vmax: f64,
    /// Source filename (for bookmark metadata).
    source_file: String,
    /// Precomputed North Up rotation angle from WCS.
    north_up_angle: f64,
    north_up_enabled: RefCell<bool>,
}

impl FitsTab {
    pub fn new(
        data: FitsImageData,
        shared_cursor: Rc<RefCell<Option<(f64, f64)>>>,
        source_file: String,
    ) -> Rc<Self> {
        let widget = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        widget.set_vexpand(true);
        widget.set_hexpand(true);

        let data = Rc::new(data);

        // Compute percentile cuts for a good initial display
        let (vmin, vmax) = fits_renderer::auto_cut(&data.pixels, 0.5, 99.5);

        // Initial render with auto-cut values
        let rgba = fits_renderer::render_to_rgba(
            &data,
            Stretch::Linear,
            ColorMap::Grayscale,
            vmin,
            vmax,
        );

        let canvas = FitsCanvas::new(
            data.width,
            data.height,
            rgba,
            shared_cursor.clone(),
            data.wcs.clone(),
        );

        let header_panel = FitsHeaderPanel::new(data.header_ordered.clone());

        // Layout: header panel (left) | canvas (right)
        widget.append(header_panel.widget());
        widget.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        widget.append(canvas.widget());

        // Precompute North Up rotation angle from WCS
        let north_up_angle = data
            .wcs
            .as_ref()
            .map(|w| -(w.cd2_1.atan2(w.cd2_2)))
            .unwrap_or(0.0);

        Rc::new(FitsTab {
            widget,
            canvas,
            header_panel,
            data,
            stretch: RefCell::new(Stretch::Linear),
            colormap: RefCell::new(ColorMap::Grayscale),
            vmin: RefCell::new(vmin),
            vmax: RefCell::new(vmax),
            auto_vmin: vmin,
            auto_vmax: vmax,
            source_file,
            north_up_angle,
            north_up_enabled: RefCell::new(false),
        })
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.widget
    }

    pub fn canvas(&self) -> &Rc<FitsCanvas> {
        &self.canvas
    }

    pub fn data(&self) -> &Rc<FitsImageData> {
        &self.data
    }

    pub fn source_file(&self) -> &str {
        &self.source_file
    }

    // ── Render state accessors ───────────────────────────────────────────────

    pub fn stretch(&self) -> Stretch {
        *self.stretch.borrow()
    }

    pub fn colormap(&self) -> ColorMap {
        *self.colormap.borrow()
    }

    pub fn vmin(&self) -> f64 {
        *self.vmin.borrow()
    }

    pub fn vmax(&self) -> f64 {
        *self.vmax.borrow()
    }

    pub fn auto_vmin(&self) -> f64 {
        self.auto_vmin
    }

    pub fn auto_vmax(&self) -> f64 {
        self.auto_vmax
    }

    pub fn data_min(&self) -> f64 {
        self.data.min_val
    }

    pub fn data_max(&self) -> f64 {
        self.data.max_val
    }

    // ── Mutators that trigger re-render ──────────────────────────────────────

    pub fn set_stretch(&self, stretch: Stretch) {
        *self.stretch.borrow_mut() = stretch;
        self.re_render();
    }

    pub fn set_colormap(&self, colormap: ColorMap) {
        *self.colormap.borrow_mut() = colormap;
        self.re_render();
    }

    pub fn set_vmin(&self, vmin: f64) {
        *self.vmin.borrow_mut() = vmin;
        self.re_render();
    }

    pub fn set_vmax(&self, vmax: f64) {
        *self.vmax.borrow_mut() = vmax;
        self.re_render();
    }

    pub fn reset_stretch(&self) {
        *self.stretch.borrow_mut() = Stretch::Linear;
        *self.colormap.borrow_mut() = ColorMap::Grayscale;
        *self.vmin.borrow_mut() = self.auto_vmin;
        *self.vmax.borrow_mut() = self.auto_vmax;
        self.re_render();
    }

    pub fn toggle_header(&self) {
        self.header_panel.toggle();
    }

    pub fn set_north_up(&self, enabled: bool) {
        *self.north_up_enabled.borrow_mut() = enabled;
        self.canvas
            .set_rotation(if enabled { self.north_up_angle } else { 0.0 });
    }

    pub fn is_north_up(&self) -> bool {
        *self.north_up_enabled.borrow()
    }

    pub fn set_zoom(&self, scale: f64) {
        self.canvas.set_zoom(scale);
    }

    pub fn zoom_scale(&self) -> f64 {
        self.canvas.zoom_scale()
    }

    pub fn reset_view(&self) {
        self.canvas.reset_view();
    }

    pub fn go_to_coord(&self, ra: f64, dec: f64) {
        self.canvas.go_to_world_coord(ra, dec);
    }

    /// Return the currently-placed crosshair as (ra, dec) if available.
    pub fn crosshair_world_pos(&self) -> Option<(f64, f64)> {
        self.canvas.crosshair_world_pos()
    }

    fn re_render(&self) {
        let stretch = *self.stretch.borrow();
        let colormap = *self.colormap.borrow();
        let vmin = *self.vmin.borrow();
        let vmax = *self.vmax.borrow();
        let rgba = fits_renderer::render_to_rgba(&self.data, stretch, colormap, vmin, vmax);
        self.canvas.update_image(rgba);
    }
}

