//! A single FITS image tab — combines the image canvas with a collapsible
//! FITS header panel. All toolbar controls live in the parent `FitsViewer`.
//!
//! State (stretch / colormap / vmin / vmax / rotation) is owned per-tab so
//! that switching tabs can sync the shared toolbar to the newly-active tab.

use crate::helpers::fits_renderer::{self, ColorMap, Stretch};
use crate::models::fits_image::HduInfo;
use crate::models::FitsImageData;
use crate::ui::fits_canvas::{FitsCanvas, SharedSkyRef};
use crate::ui::fits_header_panel::FitsHeaderPanel;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use std::cell::{Cell, RefCell};
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
    /// Source filename/path (for bookmark metadata and HDU reloads).
    source_file: String,
    /// Precomputed North Up rotation angle from WCS.
    north_up_angle: f64,
    north_up_enabled: RefCell<bool>,
    /// Cross-tab shared crosshair/hover state, linked by sky (kept so the tab
    /// can clear the markers and apply the linked crosshair on tab switch).
    shared: SharedSkyRef,
    /// All HDUs in the source file, for the extension selector (cached).
    hdus: RefCell<Vec<HduInfo>>,
    /// The 1-based HDU index this tab is currently displaying.
    hdu_index: Cell<usize>,
}

impl FitsTab {
    pub fn new(data: FitsImageData, shared: SharedSkyRef, source_file: String) -> Rc<Self> {
        let widget = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        widget.set_vexpand(true);
        widget.set_hexpand(true);

        let data = Rc::new(data);

        // Compute percentile cuts for a good initial display
        let (vmin, vmax) = fits_renderer::auto_cut(&data.pixels, 0.5, 99.5);

        // Initial render with auto-cut values
        let rgba =
            fits_renderer::render_to_rgba(&data, Stretch::Linear, ColorMap::Grayscale, vmin, vmax);

        let canvas = FitsCanvas::new(
            data.width,
            data.height,
            rgba,
            shared.clone(),
            data.wcs.clone(),
        );

        let header_panel =
            FitsHeaderPanel::new_with_info(data.header_ordered.clone(), data.image_info_rows());

        // Layout: header panel (left) | canvas (right)
        widget.append(header_panel.widget());
        widget.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        widget.append(canvas.widget());

        // Precompute the North-Up rotation (radians). Windows' NorthAngle uses the
        // atan2(-Cd1_2, Cd2_2) convention; to show North up, rotate by -NorthAngle.
        let north_up_angle = data
            .wcs
            .as_ref()
            .map(|w| -w.north_angle().to_radians())
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
            shared,
            hdus: RefCell::new(Vec::new()),
            hdu_index: Cell::new(1),
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

    /// Image-space coordinate at the centre of the viewport.
    pub fn viewport_center(&self) -> (f64, f64) {
        self.canvas.viewport_center()
    }

    /// Pan so image-space `(cx, cy)` is centred in the viewport.
    pub fn set_viewport_center(&self, cx: f64, cy: f64) {
        self.canvas.set_viewport_center(cx, cy);
    }

    /// The currently-placed crosshair in image-pixel coordinates, if any.
    pub fn crosshair_pixel_pos(&self) -> Option<(f64, f64)> {
        self.canvas.crosshair_pos()
    }

    pub fn go_to_coord(&self, ra: f64, dec: f64) {
        self.canvas.go_to_world_coord(ra, dec);
    }

    /// Return the currently-placed crosshair as (ra, dec) if available.
    pub fn crosshair_world_pos(&self) -> Option<(f64, f64)> {
        self.canvas.crosshair_world_pos()
    }

    /// Angular scale in arcsec per screen pixel (`pixel_scale / zoom`), used to
    /// match the field-of-view of tabs with different plate scales. `None` when
    /// this tab has no WCS.
    pub fn angular_scale_arcsec(&self) -> Option<f64> {
        let z = self.zoom_scale();
        if z <= 0.0 {
            return None;
        }
        self.data.wcs.as_ref().map(|w| w.pixel_scale_arcsec() / z)
    }

    /// Set zoom so this tab shows `target_arcsec` per screen pixel (same angular
    /// field as another tab). Returns `false` if this tab has no usable WCS scale.
    pub fn set_angular_scale_arcsec(&self, target_arcsec: f64) -> bool {
        let Some(scale) = self.data.wcs.as_ref().map(|w| w.pixel_scale_arcsec()) else {
            return false;
        };
        if target_arcsec <= 0.0 || scale <= 0.0 {
            return false;
        }
        self.set_zoom(scale / target_arcsec);
        true
    }

    /// Pan so sky coordinate `(ra, dec)` sits at the viewport centre (maps through
    /// this tab's own WCS). Returns `false` if it has no WCS / maps out of frame.
    pub fn center_on_world(&self, ra: f64, dec: f64) -> bool {
        match self
            .data
            .wcs
            .as_ref()
            .and_then(|w| w.world_to_pixel(ra, dec))
        {
            Some((px, py)) => {
                self.set_viewport_center(px, py);
                true
            }
            None => false,
        }
    }

    /// The sky coordinate at the image centre (for framing a blink when no
    /// crosshair is set). `None` without a WCS.
    pub fn image_center_world(&self) -> Option<(f64, f64)> {
        let (cx, cy) = (self.data.width as f64 / 2.0, self.data.height as f64 / 2.0);
        self.data.wcs.as_ref().map(|w| w.pixel_to_sky(cx, cy))
    }

    /// Clear both the placed (red) crosshair and the hover (green) marker, and
    /// clear the shared cross-tab sky state so linked tabs drop their markers too.
    pub fn clear_crosshair(&self) {
        {
            let mut s = self.shared.borrow_mut();
            s.hover = None;
            s.placed = None;
        }
        // Clears the placed crosshair, queues a redraw (dropping the now-`None`
        // hover marker too) and notifies the crosshair-placed callback.
        self.canvas.set_crosshair(None);
    }

    /// Seed the shared placed-crosshair sky point from this tab's own crosshair
    /// (used when the link toggle is turned on so the current mark propagates).
    pub fn publish_current_crosshair(&self) {
        if let Some((ra, dec)) = self.canvas.crosshair_world_pos() {
            self.shared.borrow_mut().placed = Some((ra, dec));
        }
    }

    /// Reposition this tab's placed crosshair from the shared linked sky point
    /// (called on tab switch when linked-crosshair is on).
    pub fn apply_linked_crosshair(&self) {
        self.canvas.apply_linked_crosshair();
    }

    /// North-Up rotation this tab would apply (radians), if it has a WCS.
    pub fn north_up_angle(&self) -> Option<f64> {
        self.data.wcs.as_ref().map(|_| self.north_up_angle)
    }

    // ── HDU / extension context ──────────────────────────────────────────────

    /// Record the file's HDU list and which 1-based HDU this tab displays.
    pub fn set_hdu_context(&self, hdus: Vec<HduInfo>, index: usize) {
        *self.hdus.borrow_mut() = hdus;
        self.hdu_index.set(index);
    }

    /// The cached HDU list for the source file (empty if unknown).
    pub fn hdus(&self) -> Vec<HduInfo> {
        self.hdus.borrow().clone()
    }

    /// The 1-based HDU index currently displayed.
    pub fn hdu_index(&self) -> usize {
        self.hdu_index.get()
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
