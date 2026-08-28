use crate::models::fits_image::WcsInfo;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// How far the viewer can zoom, as a scale factor (1.0 = 1 image pixel per
/// screen pixel).
///
/// One range for every path that can change the zoom. There were three — the
/// canvas clamped 0.01–100, the scroll wheel 0.1–50, and `set_fits_view`
/// advertised 0.05–20 — so "how far can I zoom" had a different answer
/// depending on whether you dragged, typed, or asked over MCP, and the
/// advertised limit was the one that matched nothing.
pub const ZOOM_SCALE_RANGE: (f64, f64) = (0.01, 100.0);

/// View transform for zoom and pan
#[derive(Clone)]
struct ViewTransform {
    scale: f64,
    offset_x: f64,
    offset_y: f64,
}

impl Default for ViewTransform {
    fn default() -> Self {
        ViewTransform {
            scale: 1.0,
            offset_x: 0.0,
            offset_y: 0.0,
        }
    }
}

/// Cross-tab shared crosshair/hover state, **linked by SKY** (RA/Dec) rather than
/// by pixel so images with different WCS line up on the same sky point. A single
/// instance is shared (via `Rc`) by every tab's canvas.
#[derive(Default)]

pub struct SharedSky {
    /// Live hover position `(ra, dec)`, written by whichever canvas owns the pointer.
    pub hover: Option<(f64, f64)>,
    /// Placed-crosshair position `(ra, dec)`, written whenever a crosshair is set.
    pub placed: Option<(f64, f64)>,
    /// Master toggle: link markers across tabs by sky (default ON, set by the viewer).
    pub linked: bool,
}

pub type SharedSkyRef = Rc<RefCell<SharedSky>>;

/// A second tab's rendered image overlaid on the active canvas for a cross-fade
/// blink. `(ref_px, ref_py)` is the overlay image pixel that must land on
/// `(anchor_x, anchor_y)` (a screen point on the active canvas), drawn at `scale`
/// and rotated by `rot` radians about that anchor.
#[derive(Clone)]
pub struct BlinkOverlay {
    pub rgba: Vec<u8>,
    pub width: usize,
    pub height: usize,
    pub ref_px: f64,
    pub ref_py: f64,
    pub anchor_x: f64,
    pub anchor_y: f64,
    pub scale: f64,
    pub rot: f64,
}

/// True when image-space `(px, py)` falls inside a `w × h` image.
fn on_image(px: f64, py: f64, w: usize, h: usize) -> bool {
    px >= 0.0 && px < w as f64 && py >= 0.0 && py < h as f64
}

/// Map an image pixel to a screen point, replicating the draw transform
/// (`screen = image·scale + offset`) plus the North-Up rotation about the image
/// centre. Keeps crosshair/hover markers locked to their pixel when rotated.
fn image_to_screen(
    px: f64,
    py: f64,
    scale: f64,
    off_x: f64,
    off_y: f64,
    rot: f64,
    w: usize,
    h: usize,
) -> (f64, f64) {
    let sx0 = px * scale + off_x;
    let sy0 = py * scale + off_y;
    if rot.abs() <= 1e-6 {
        return (sx0, sy0);
    }
    let cx = off_x + (w as f64 / 2.0) * scale;
    let cy = off_y + (h as f64 / 2.0) * scale;
    let dx = sx0 - cx;
    let dy = sy0 - cy;
    let (s, c) = rot.sin_cos();
    (cx + dx * c - dy * s, cy + dx * s + dy * c)
}

/// Inverse of [`image_to_screen`]: map a screen point back to an image pixel,
/// undoing pan/zoom and the North-Up rotation about the image centre.
fn screen_to_image(
    sx: f64,
    sy: f64,
    scale: f64,
    off_x: f64,
    off_y: f64,
    rot: f64,
    w: usize,
    h: usize,
) -> (f64, f64) {
    let (ux, uy) = if rot.abs() <= 1e-6 {
        (sx, sy)
    } else {
        let cx = off_x + (w as f64 / 2.0) * scale;
        let cy = off_y + (h as f64 / 2.0) * scale;
        let dx = sx - cx;
        let dy = sy - cy;
        let (s, c) = (-rot).sin_cos();
        (cx + dx * c - dy * s, cy + dx * s + dy * c)
    };
    ((ux - off_x) / scale, (uy - off_y) / scale)
}

/// Build a Cairo ARGB32 surface from a tightly-packed RGBA buffer (`w × h`),
/// converting to Cairo's premultiplied BGRA byte order. `None` on size mismatch.
fn rgba_to_surface(data: &[u8], w: usize, h: usize) -> Option<cairo::ImageSurface> {
    if w == 0 || h == 0 || data.len() < w * h * 4 {
        return None;
    }
    let stride = cairo::Format::ARgb32
        .stride_for_width(w as u32)
        .unwrap_or(w as i32 * 4);
    let mut cairo_data = vec![0u8; (stride as usize) * h];
    for y in 0..h {
        for x in 0..w {
            let src = (y * w + x) * 4;
            let dst = y * stride as usize + x * 4;
            if src + 3 < data.len() && dst + 3 < cairo_data.len() {
                cairo_data[dst] = data[src + 2]; // B
                cairo_data[dst + 1] = data[src + 1]; // G
                cairo_data[dst + 2] = data[src]; // R
                cairo_data[dst + 3] = data[src + 3]; // A
            }
        }
    }
    cairo::ImageSurface::create_for_data(
        cairo_data,
        cairo::Format::ARgb32,
        w as i32,
        h as i32,
        stride,
    )
    .ok()
}

/// Pick the hover pixel to draw: the sky-linked pixel when linking is on and a
/// WCS is present (may be `None` → hidden off-image/no-map), else the canvas's
/// own local hover pixel.
fn choose_hover_pixel(
    linked: bool,
    has_wcs: bool,
    mapped_from_sky: Option<(f64, f64)>,
    local_hover: Option<(f64, f64)>,
) -> Option<(f64, f64)> {
    if linked && has_wcs {
        mapped_from_sky
    } else {
        local_hover
    }
}

type OnCrosshairPlacedCallback = Rc<RefCell<Option<Box<dyn Fn(Option<(f64, f64)>)>>>>;

/// Whether a capture of `width` x `height` can be drawn at all.
///
/// Split out of [`FitsCanvas::capture_png`] because everything else in this
/// file needs a realized widget to test, and a rule only an example probe can
/// reach is a rule `cargo test` never checks. Cairo would refuse these sizes
/// too, with a message about surfaces rather than about the request.
fn validate_capture_size(width: i32, height: i32) -> Result<(), String> {
    if width <= 0 || height <= 0 {
        return Err(format!("invalid capture size {width}x{height}"));
    }
    // Cairo's ARgb32 stride is 4 bytes per pixel; a request past this would try
    // to allocate more memory than the process can address.
    const MAX_PIXELS: i64 = 64 * 1024 * 1024;
    if i64::from(width) * i64::from(height) > MAX_PIXELS {
        return Err(format!(
            "capture of {width}x{height} is too large to render; ask for a smaller region"
        ));
    }
    Ok(())
}

pub struct FitsCanvas {
    widget: gtk::Box,
    drawing_area: gtk::DrawingArea,
    coord_label: gtk::Label,
    pixel_data: Rc<RefCell<Vec<u8>>>,
    img_width: usize,
    img_height: usize,
    transform: Rc<RefCell<ViewTransform>>,
    /// Cross-tab shared crosshair/hover state (linked by sky).
    shared: SharedSkyRef,
    /// This canvas's own last hover pixel (image-space) — zoom anchor + the
    /// fallback marker when sky-linking is off or there is no WCS.
    local_hover: Rc<RefCell<Option<(f64, f64)>>>,
    /// A right-clicked persistent crosshair position (in image-space).
    crosshair_placed: Rc<RefCell<Option<(f64, f64)>>>,
    /// Installed by the viewer while draw mode is on. Shared with the pan
    /// gesture, which stands down while it is set.
    #[allow(clippy::type_complexity)]
    on_left_click: Rc<RefCell<Option<Box<dyn Fn(f64, f64)>>>>,
    /// Marks drawn on this image, by the user or an agent.
    annotations: Rc<RefCell<Vec<crate::models::annotation::Annotation>>>,
    /// The selected mark's id, highlighted on the canvas and in the panel.
    selected_annotation: Rc<RefCell<Option<String>>>,
    /// Rotation angle in radians (for North Up).
    rotation: Rc<RefCell<f64>>,
    /// A second image cross-faded over this canvas during a blink comparison.
    blink_overlay: Rc<RefCell<Option<BlinkOverlay>>>,
    /// Blink overlay opacity (0 = show this image, 1 = show the overlay).
    blink_opacity: Rc<Cell<f64>>,
    wcs: Option<WcsInfo>,
    on_crosshair_placed: OnCrosshairPlacedCallback,
}

impl FitsCanvas {
    pub fn new(
        width: usize,
        height: usize,
        rgba: Vec<u8>,
        shared: SharedSkyRef,
        wcs: Option<WcsInfo>,
    ) -> Rc<Self> {
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        widget.set_vexpand(true);
        widget.set_hexpand(true);

        let drawing_area = gtk::DrawingArea::new();
        drawing_area.set_vexpand(true);
        drawing_area.set_hexpand(true);
        drawing_area.set_content_width(width.min(800) as i32);
        drawing_area.set_content_height(height.min(600) as i32);

        let coord_label = gtk::Label::new(None);
        coord_label.add_css_class("caption");
        coord_label.add_css_class("dim-label");
        coord_label.set_halign(gtk::Align::Start);
        coord_label.set_margin_start(8);
        coord_label.set_margin_bottom(4);

        widget.append(&drawing_area);
        widget.append(&coord_label);

        let canvas = Rc::new(FitsCanvas {
            widget,
            drawing_area,
            coord_label,
            pixel_data: Rc::new(RefCell::new(rgba)),
            img_width: width,
            img_height: height,
            transform: Rc::new(RefCell::new(ViewTransform::default())),
            shared,
            local_hover: Rc::new(RefCell::new(None)),
            crosshair_placed: Rc::new(RefCell::new(None)),
            on_left_click: Rc::new(RefCell::new(None)),
            annotations: Rc::new(RefCell::new(Vec::new())),
            selected_annotation: Rc::new(RefCell::new(None)),
            rotation: Rc::new(RefCell::new(0.0)),
            blink_overlay: Rc::new(RefCell::new(None)),
            blink_opacity: Rc::new(Cell::new(0.0)),
            wcs,
            on_crosshair_placed: Rc::new(RefCell::new(None)),
        });

        canvas.setup_draw();
        canvas.setup_scroll_zoom();
        canvas.setup_drag_pan();
        canvas.setup_motion_tracking();
        canvas.setup_left_click();
        canvas.setup_right_click_crosshair();

        canvas
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.widget
    }

    pub fn update_image(&self, rgba: Vec<u8>) {
        *self.pixel_data.borrow_mut() = rgba;
        self.drawing_area.queue_draw();
    }

    // ── Public API for the toolbar ───────────────────────────────────────────

    /// Set the zoom scale and redraw. Offset is not changed.
    /// Zoom to `scale`, keeping the point the user is looking at where it is.
    ///
    /// The wheel has always compensated the offset so a chosen point stays put;
    /// this did not, so the toolbar dropdown, the zoom entry and MCP all scaled
    /// about the image ORIGIN — zoom in and the subject flew off toward the top
    /// left. It reads as annotations sliding away from their features, which is
    /// how it was noticed, but the marks were on their pixels the whole time:
    /// the image had left.
    ///
    /// Same anchor rule as the wheel — the crosshair if one is placed, else the
    /// centre of the view — so the two ways of zooming now agree.
    pub fn set_zoom(&self, scale: f64) {
        let target = scale.clamp(ZOOM_SCALE_RANGE.0, ZOOM_SCALE_RANGE.1);
        let anchor = (*self.crosshair_placed.borrow()).unwrap_or_else(|| {
            let (vw, vh) = self.viewport_size();
            let t = self.transform.borrow();
            // The image pixel currently at the middle of the viewport.
            (
                (vw / 2.0 - t.offset_x) / t.scale.max(f64::EPSILON),
                (vh / 2.0 - t.offset_y) / t.scale.max(f64::EPSILON),
            )
        });
        {
            let mut t = self.transform.borrow_mut();
            let s0 = t.scale;
            t.offset_x += anchor.0 * (s0 - target);
            t.offset_y += anchor.1 * (s0 - target);
            t.scale = target;
        }
        self.drawing_area.queue_draw();
    }

    /// Current zoom scale.
    pub fn zoom_scale(&self) -> f64 {
        self.transform.borrow().scale
    }

    /// Reset zoom and pan to default.
    pub fn reset_view(&self) {
        *self.transform.borrow_mut() = ViewTransform::default();
        self.drawing_area.queue_draw();
    }

    /// Allocated drawing-area size, falling back to the requested content size
    /// before the first allocation (so viewport maths works headless / pre-realize).
    fn viewport_size(&self) -> (f64, f64) {
        let w = self.drawing_area.width();
        let h = self.drawing_area.height();
        let w = if w > 0 {
            w
        } else {
            self.drawing_area.content_width()
        };
        let h = if h > 0 {
            h
        } else {
            self.drawing_area.content_height()
        };
        (w as f64, h as f64)
    }

    /// The image-space coordinate currently at the centre of the viewport
    /// (inverse of the screen transform `screen = image·scale + offset`).
    pub fn viewport_center(&self) -> (f64, f64) {
        let t = self.transform.borrow();
        let (w, h) = self.viewport_size();
        (
            (w / 2.0 - t.offset_x) / t.scale,
            (h / 2.0 - t.offset_y) / t.scale,
        )
    }

    /// Pan so image-space `(cx, cy)` sits at the centre of the viewport.
    pub fn set_viewport_center(&self, cx: f64, cy: f64) {
        let (w, h) = self.viewport_size();
        {
            let mut t = self.transform.borrow_mut();
            t.offset_x = w / 2.0 - cx * t.scale;
            t.offset_y = h / 2.0 - cy * t.scale;
        }
        self.drawing_area.queue_draw();
    }

    /// Set the North Up rotation angle (radians). 0 disables rotation.
    pub fn set_rotation(&self, angle_rad: f64) {
        *self.rotation.borrow_mut() = angle_rad;
        self.drawing_area.queue_draw();
    }

    /// Place or clear the persistent crosshair (image-space coordinates). Also
    /// publishes the position (as sky) to the shared cross-tab state so other
    /// tabs can show a linked crosshair on the same sky point.
    pub fn set_crosshair(&self, pos: Option<(f64, f64)>) {
        *self.crosshair_placed.borrow_mut() = pos;
        self.publish_placed_sky(pos);
        self.drawing_area.queue_draw();
        if let Some(cb) = self.on_crosshair_placed.borrow().as_ref() {
            cb(pos);
        }
    }

    /// Write `pos` (image pixel → sky via this canvas's WCS) into the shared
    /// placed-crosshair cell, or clear it. No-op without a WCS.
    fn publish_placed_sky(&self, pos: Option<(f64, f64)>) {
        let Some(w) = self.wcs.as_ref() else {
            return;
        };
        self.shared.borrow_mut().placed = pos.map(|(px, py)| w.pixel_to_sky(px, py));
    }

    /// Reposition this canvas's placed crosshair from the shared linked sky point
    /// (called on tab switch while linked-crosshair is on). Maps the shared sky
    /// through *this* canvas's WCS; hides the crosshair when off-image or absent.
    /// Does not re-publish to the shared cell (avoids feedback).
    pub fn apply_linked_crosshair(&self) {
        let (linked, placed) = {
            let s = self.shared.borrow();
            (s.linked, s.placed)
        };
        if !linked {
            return;
        }
        let new_pos = match (placed, self.wcs.as_ref()) {
            (Some((ra, dec)), Some(w)) => match w.world_to_pixel(ra, dec) {
                Some((px, py)) if on_image(px, py, self.img_width, self.img_height) => {
                    Some((px, py))
                }
                _ => None,
            },
            _ => return,
        };
        *self.crosshair_placed.borrow_mut() = new_pos;
        self.drawing_area.queue_draw();
        if let Some(cb) = self.on_crosshair_placed.borrow().as_ref() {
            cb(new_pos);
        }
    }

    /// Return the current placed crosshair pixel coordinates.
    pub fn crosshair_pos(&self) -> Option<(f64, f64)> {
        *self.crosshair_placed.borrow()
    }

    /// Return the current placed crosshair as world coordinates (RA, Dec).
    pub fn crosshair_world_pos(&self) -> Option<(f64, f64)> {
        let (px, py) = self.crosshair_placed.borrow().as_ref().copied()?;
        self.wcs.as_ref().map(|w| w.pixel_to_sky(px, py))
    }

    /// Center the view on a given sky coordinate and place a crosshair there.
    /// If the coordinate maps outside the image, the crosshair is cleared rather
    /// than floated off the frame.
    pub fn go_to_world_coord(&self, ra: f64, dec: f64) {
        if let Some(ref wcs) = self.wcs {
            match wcs.world_to_pixel(ra, dec) {
                Some((px, py))
                    if px >= 0.0
                        && px < self.img_width as f64
                        && py >= 0.0
                        && py < self.img_height as f64 =>
                {
                    self.set_crosshair(Some((px, py)));
                }
                _ => self.set_crosshair(None),
            }
        }
    }

    /// Register a callback invoked when the placed crosshair position changes.
    pub fn set_on_crosshair_placed(&self, cb: impl Fn(Option<(f64, f64)>) + 'static) {
        *self.on_crosshair_placed.borrow_mut() = Some(Box::new(cb));
    }

    pub fn img_width(&self) -> usize {
        self.img_width
    }

    pub fn img_height(&self) -> usize {
        self.img_height
    }

    /// A clone of the current rendered RGBA buffer (used to build a blink overlay).
    pub fn current_rgba(&self) -> Vec<u8> {
        self.pixel_data.borrow().clone()
    }

    /// Current North-Up rotation in radians (0 when disabled).
    pub fn rotation_rad(&self) -> f64 {
        *self.rotation.borrow()
    }

    /// Screen point of an image pixel under the current view (incl. rotation).
    pub fn image_to_screen_point(&self, px: f64, py: f64) -> (f64, f64) {
        let t = self.transform.borrow();
        let rot = *self.rotation.borrow();
        image_to_screen(
            px,
            py,
            t.scale,
            t.offset_x,
            t.offset_y,
            rot,
            self.img_width,
            self.img_height,
        )
    }

    // ── Cross-fade blink overlay ─────────────────────────────────────────────

    /// Begin a cross-fade blink: overlay `overlay` (a second tab's rendered image,
    /// pre-aligned onto this canvas) at opacity 0.
    pub fn enter_blink(&self, overlay: BlinkOverlay) {
        *self.blink_overlay.borrow_mut() = Some(overlay);
        self.blink_opacity.set(0.0);
        self.drawing_area.queue_draw();
    }

    /// Fade the blink overlay to `opacity` (0 = this image, 1 = the overlay).
    pub fn set_blink_opacity(&self, opacity: f64) {
        if self.blink_overlay.borrow().is_none() {
            return;
        }
        self.blink_opacity.set(opacity.clamp(0.0, 1.0));
        self.drawing_area.queue_draw();
    }

    /// End the blink and drop the overlay.
    pub fn exit_blink(&self) {
        *self.blink_overlay.borrow_mut() = None;
        self.blink_opacity.set(0.0);
        self.drawing_area.queue_draw();
    }

    // ── Drawing ──────────────────────────────────────────────────────────────

    /// Draw the working area — everything the user sees — into `cr`.
    ///
    /// This was the body of the `set_draw_func` closure, and moving it out is
    /// what lets an agent be shown the same picture. The screen path and the
    /// capture path call THIS, so a change to how the viewer looks is a change
    /// to what the agent sees, by construction. A second renderer written for
    /// the agent's benefit would start correct and drift, and the only witness
    /// would be an agent describing a picture nobody else had looked at.
    ///
    /// Knows nothing about its destination: a widget's context or an
    /// `ImageSurface` are the same to it.
    pub fn draw_working_area(&self, cr: &cairo::Context, widget_w: i32, widget_h: i32) {
        let pixel_data = &self.pixel_data;
        let transform = &self.transform;
        let rotation = &self.rotation;
        let w = self.img_width;
        let h = self.img_height;
        let shared = &self.shared;
        let local_hover = &self.local_hover;
        let crosshair_placed = &self.crosshair_placed;
        let blink_overlay = &self.blink_overlay;
        let blink_opacity = &self.blink_opacity;
        let wcs = &self.wcs;

        // Black background
        cr.set_source_rgb(0.1, 0.1, 0.1);
        let _ = cr.paint();

        let data = pixel_data.borrow();
        if data.is_empty() || w == 0 || h == 0 {
            return;
        }

        let t = transform.borrow();
        let rot = *rotation.borrow();

        if let Some(surface) = rgba_to_surface(&data, w, h) {
            cr.save().ok();
            // Apply rotation around the center of the image (north-up)
            if rot.abs() > 1e-6 {
                let cx = t.offset_x + (w as f64 / 2.0) * t.scale;
                let cy = t.offset_y + (h as f64 / 2.0) * t.scale;
                cr.translate(cx, cy);
                cr.rotate(rot);
                cr.translate(-cx, -cy);
            }
            cr.translate(t.offset_x, t.offset_y);
            cr.scale(t.scale, t.scale);
            cr.set_source_surface(&surface, 0.0, 0.0).ok();
            // Use nearest-neighbor for pixel-sharp rendering
            let pattern = cr.source();
            pattern.set_filter(cairo::Filter::Nearest);
            cr.paint().ok();
            cr.restore().ok();
        }

        // ── Cross-fade blink overlay (a second tab's image over this one) ──
        if let Some(ov) = blink_overlay.borrow().as_ref() {
            let alpha = blink_opacity.get().clamp(0.0, 1.0);
            if alpha > 0.0 {
                if let Some(osurf) = rgba_to_surface(&ov.rgba, ov.width, ov.height) {
                    cr.save().ok();
                    // Pin the overlay's reference pixel to its screen anchor,
                    // rotated + scaled about that anchor to match this image.
                    cr.translate(ov.anchor_x, ov.anchor_y);
                    cr.rotate(ov.rot);
                    cr.scale(ov.scale, ov.scale);
                    cr.translate(-ov.ref_px, -ov.ref_py);
                    cr.set_source_surface(&osurf, 0.0, 0.0).ok();
                    let pattern = cr.source();
                    pattern.set_filter(cairo::Filter::Nearest);
                    cr.paint_with_alpha(alpha).ok();
                    cr.restore().ok();
                }
            }
        }

        // Resolve the hover pixel: sky-linked (via this canvas's own WCS)
        // when linking is on, else this canvas's own local hover.
        let (linked, hover_sky) = {
            let s = shared.borrow();
            (s.linked, s.hover)
        };
        let mapped_hover = match (linked, hover_sky, wcs.as_ref()) {
            (true, Some((ra, dec)), Some(w_ref)) => w_ref.world_to_pixel(ra, dec),
            _ => None,
        };
        let hover_pixel =
            choose_hover_pixel(linked, wcs.is_some(), mapped_hover, *local_hover.borrow())
                .filter(|&(cx, cy)| on_image(cx, cy, w, h));

        // Draw hover crosshair (green dashed) — locked to its image pixel
        // through the same rotation as the image.
        if let Some((cx, cy)) = hover_pixel {
            let (sx, sy) = image_to_screen(cx, cy, t.scale, t.offset_x, t.offset_y, rot, w, h);

            cr.set_source_rgba(0.0, 1.0, 0.0, 0.7);
            cr.set_line_width(1.0);
            cr.set_dash(&[4.0, 4.0], 0.0);

            cr.move_to(sx, 0.0);
            cr.line_to(sx, widget_h as f64);
            cr.stroke().ok();

            cr.move_to(0.0, sy);
            cr.line_to(widget_w as f64, sy);
            cr.stroke().ok();

            cr.set_dash(&[], 0.0);
        }

        // Draw placed crosshair (solid red) with optional RA/Dec label.
        // Hidden when it falls outside the image (e.g. an off-image Go To).
        if let Some((cx, cy)) =
            (*crosshair_placed.borrow()).filter(|&(cx, cy)| on_image(cx, cy, w, h))
        {
            let (sx, sy) = image_to_screen(cx, cy, t.scale, t.offset_x, t.offset_y, rot, w, h);

            cr.set_source_rgba(1.0, 0.15, 0.15, 0.9);
            cr.set_line_width(1.5);

            cr.move_to(sx, 0.0);
            cr.line_to(sx, widget_h as f64);
            cr.stroke().ok();

            cr.move_to(0.0, sy);
            cr.line_to(widget_w as f64, sy);
            cr.stroke().ok();

            // Label with sky coordinates if WCS available
            if let Some(ref w_ref) = wcs {
                let (ra, dec) = w_ref.pixel_to_sky(cx, cy);
                let (ra_str, dec_str) = WcsInfo::format_coords(ra, dec);
                let text = format!("RA {}  Dec {}", ra_str, dec_str);

                // The same renderer the cube's slice view uses, so the
                // two viewers' readouts cannot drift apart in look or in
                // edge behaviour.
                crate::ui::coord_chip::draw(
                    cr,
                    sx,
                    sy,
                    std::slice::from_ref(&text),
                    widget_w as f64,
                    widget_h as f64,
                );
            }
        }

        // Marks last, over everything, and drawn HERE — inside the function the
        // capture replays — so an agent's picture and the user's screen show
        // the same annotations without either path knowing about the other.
        crate::helpers::annotation_render::draw(
            &self.annotations.borrow(),
            self,
            self.selected_annotation.borrow().as_deref(),
            cr,
            widget_w as f64,
            widget_h as f64,
        );
    }

    // ── Annotations ─────────────────────────────────────────────────────────

    /// Replace the marks on this canvas.
    pub fn set_annotations(&self, annotations: Vec<crate::models::annotation::Annotation>) {
        *self.annotations.borrow_mut() = annotations;
        self.drawing_area.queue_draw();
    }

    pub fn annotations(&self) -> Vec<crate::models::annotation::Annotation> {
        self.annotations.borrow().clone()
    }

    pub fn set_selected_annotation(&self, id: Option<String>) {
        *self.selected_annotation.borrow_mut() = id;
        self.drawing_area.queue_draw();
    }

    pub fn selected_annotation(&self) -> Option<String> {
        self.selected_annotation.borrow().clone()
    }

    /// The mark whose shape contains `(sx, sy)`, topmost first.
    ///
    /// Hit-testing is done in SCREEN space against the projected shape, so what
    /// the user can click is exactly what they can see — the alternative,
    /// testing in image space, quietly disagrees with the drawing wherever
    /// rotation is in play.
    pub fn annotation_at(&self, sx: f64, sy: f64) -> Option<String> {
        let anns = self.annotations.borrow();
        for a in anns.iter().rev() {
            let (cx, cy) = self.project_anchor(&a.anchor)?;
            let scale = self.annotation_scale(&a.anchor);
            let (hw, hh) = a
                .extent
                .map(|e| (e.half_width * scale, e.half_height * scale))
                .unwrap_or((8.0, 8.0));
            // A generous minimum: a hairline circle a few pixels across is
            // impossible to hit exactly, and a near miss reads as broken.
            let (hw, hh) = (hw.max(6.0), hh.max(6.0));
            if (sx - cx).abs() <= hw && (sy - cy).abs() <= hh {
                return Some(a.id.clone());
            }
        }
        None
    }

    /// A sensible size for a new mark, in the anchor's own units.
    ///
    /// The default has to mean the same thing on every image, and an ANGLE does
    /// not: 0.005° is a comfortable ring on a JWST frame at 0.03″/px and a
    /// fifth of one pixel on IRAS at 90″/px, where it drew nothing. So the
    /// default is stated in image pixels — what "about this big" means to
    /// someone looking at the image — and converted into whatever unit the
    /// anchor uses.
    pub fn default_extent_for(
        &self,
        anchor: &crate::models::annotation::Anchor,
    ) -> crate::models::annotation::Extent {
        const IMAGE_PIXELS: f64 = 14.0;
        let view = self.transform.borrow().scale.max(f64::EPSILON);
        // Image pixels per unit of the anchor's space.
        let per_unit = self.annotation_scale(anchor) / view;
        let half = if per_unit.is_finite() && per_unit > 0.0 {
            IMAGE_PIXELS / per_unit
        } else {
            IMAGE_PIXELS
        };
        crate::models::annotation::Extent::square(half)
    }

    /// An anchor's position on this canvas, or `None` when it is not on it.
    fn project_anchor(&self, anchor: &crate::models::annotation::Anchor) -> Option<(f64, f64)> {
        use crate::models::annotation::Anchor;
        let (px, py) = match *anchor {
            Anchor::ImagePixel { x, y } => (x, y),
            // A sky anchor is placed through this image's OWN WCS, so a mark
            // made on one image lands correctly on another of the same field.
            Anchor::Sky { ra_deg, dec_deg } => {
                self.wcs.as_ref()?.world_to_pixel(ra_deg, dec_deg)?
            }
            // A cube's voxel means nothing here.
            Anchor::Data { .. } => return None,
        };
        let (sx, sy) = self.image_to_screen_point(px, py);
        sx.is_finite().then_some((sx, sy))
    }

    /// Device pixels per unit of `anchor`'s own space.
    ///
    /// Not one number: an image-pixel extent is in pixels and a sky extent is
    /// in DEGREES, and treating both as pixels drew a sky circle 0.005 device
    /// pixels across — invisible, and with no error, so a mark placed through
    /// the UI (which prefers sky anchors when there is WCS) appeared to do
    /// nothing at all.
    fn annotation_scale(&self, anchor: &crate::models::annotation::Anchor) -> f64 {
        use crate::models::annotation::Anchor;
        let view = self.transform.borrow().scale;
        match anchor {
            Anchor::ImagePixel { .. } => view,
            Anchor::Sky { .. } => {
                // Degrees → image pixels → device pixels.
                let arcsec_per_px = self
                    .wcs
                    .as_ref()
                    .map(|w| w.pixel_scale_arcsec())
                    .filter(|s| s.is_finite() && *s > 0.0)
                    .unwrap_or(1.0);
                let px_per_degree = 3600.0 / arcsec_per_px;
                view * px_per_degree
            }
            // A cube voxel is not a length on this canvas.
            Anchor::Data { .. } => view,
        }
    }

    /// The working area as PNG bytes, at `(width, height)`.
    ///
    /// Runs [`draw_working_area`](Self::draw_working_area) — the same code the
    /// screen runs — into an off-screen surface. Nothing about the view is
    /// re-derived here, which is the point: pan, zoom, rotation, colormap,
    /// stretch, the crosshair and a blink overlay all appear because they are
    /// drawn by the function that draws them on screen.
    ///
    /// A size of zero or less is refused rather than allocated.
    pub fn capture_png(&self, width: i32, height: i32) -> Result<Vec<u8>, String> {
        validate_capture_size(width, height)?;
        let surface = cairo::ImageSurface::create(cairo::Format::ARgb32, width, height)
            .map_err(|e| format!("cairo surface error: {e}"))?;
        {
            let cr =
                cairo::Context::new(&surface).map_err(|e| format!("cairo context error: {e}"))?;
            self.draw_working_area(&cr, width, height);
        }
        let mut png: Vec<u8> = Vec::new();
        surface
            .write_to_png(&mut png)
            .map_err(|e| format!("PNG encode failed: {e}"))?;
        Ok(png)
    }

    /// The on-screen size of the drawing area, for a capture that matches it.
    pub fn view_size(&self) -> (i32, i32) {
        (self.drawing_area.width(), self.drawing_area.height())
    }

    fn setup_draw(self: &Rc<Self>) {
        let canvas = Rc::downgrade(self);
        self.drawing_area
            .set_draw_func(move |_area, cr, widget_w, widget_h| {
                // Weak, so the closure the widget owns does not keep the canvas
                // alive after the tab holding it is closed.
                if let Some(canvas) = canvas.upgrade() {
                    canvas.draw_working_area(cr, widget_w, widget_h);
                }
            });
    }

    fn setup_scroll_zoom(&self) {
        let scroll_controller =
            gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        let transform = self.transform.clone();
        let drawing_area = self.drawing_area.clone();
        let local_hover = self.local_hover.clone();
        let crosshair_placed = self.crosshair_placed.clone();
        let w = self.img_width;
        let h = self.img_height;

        scroll_controller.connect_scroll(move |ctrl, _dx, dy| {
            let state = ctrl.current_event_state();
            let ctrl_held = state.contains(gtk::gdk::ModifierType::CONTROL_MASK);
            let shift_held = state.contains(gtk::gdk::ModifierType::SHIFT_MASK);
            let mut t = transform.borrow_mut();
            if ctrl_held {
                // Ctrl+wheel pans vertically (Windows 1.3.1 default).
                t.offset_y -= dy * 40.0;
            } else if shift_held {
                // Shift+wheel pans horizontally.
                t.offset_x -= dy * 40.0;
            } else {
                // Plain wheel zooms toward the crosshair (else the last cursor,
                // else the image centre), keeping that point fixed on screen.
                let s0 = t.scale;
                let factor = if dy < 0.0 { 1.15 } else { 1.0 / 1.15 };
                let s1 = (s0 * factor).clamp(ZOOM_SCALE_RANGE.0, ZOOM_SCALE_RANGE.1);
                let anchor = (*crosshair_placed.borrow())
                    .or(*local_hover.borrow())
                    .unwrap_or((w as f64 / 2.0, h as f64 / 2.0));
                t.offset_x += anchor.0 * (s0 - s1);
                t.offset_y += anchor.1 * (s0 - s1);
                t.scale = s1;
            }
            drawing_area.queue_draw();
            gtk::glib::Propagation::Stop
        });

        self.drawing_area.add_controller(scroll_controller);
    }

    fn setup_drag_pan(self: &Rc<Self>) {
        let drag = gtk::GestureDrag::new();
        drag.set_button(1); // Left mouse button
        let transform = self.transform.clone();
        let drawing_area = self.drawing_area.clone();
        let start_offset = Rc::new(RefCell::new((0.0, 0.0)));
        // Draw mode owns the left button while it is on. Both gestures took
        // button 1, and the drag claimed the sequence first — so a click meant
        // to place a mark panned the image instead, and nothing was ever
        // placed.
        let drawing = self.on_left_click.clone();

        let so = start_offset.clone();
        let t = transform.clone();
        let d = drawing.clone();
        drag.connect_drag_begin(move |gesture, _x, _y| {
            let shifted = gesture
                .current_event_state()
                .contains(gtk::gdk::ModifierType::SHIFT_MASK);
            if d.borrow().is_some() && !shifted {
                gesture.set_state(gtk::EventSequenceState::Denied);
                return;
            }
            let t = t.borrow();
            *so.borrow_mut() = (t.offset_x, t.offset_y);
        });

        let so = start_offset;
        drag.connect_drag_update(move |gesture, dx, dy| {
            let shifted = gesture
                .current_event_state()
                .contains(gtk::gdk::ModifierType::SHIFT_MASK);
            if drawing.borrow().is_some() && !shifted {
                return;
            }
            let start = so.borrow();
            let mut t = transform.borrow_mut();
            t.offset_x = start.0 + dx;
            t.offset_y = start.1 + dy;
            drawing_area.queue_draw();
        });

        self.drawing_area.add_controller(drag);
    }

    fn setup_motion_tracking(&self) {
        let motion = gtk::EventControllerMotion::new();
        let transform = self.transform.clone();
        let rotation = self.rotation.clone();
        let shared = self.shared.clone();
        let local_hover = self.local_hover.clone();
        let drawing_area = self.drawing_area.clone();
        let coord_label = self.coord_label.clone();
        let wcs = self.wcs.clone();
        let w = self.img_width;
        let h = self.img_height;

        motion.connect_motion(move |_, x, y| {
            // Invert the draw transform (incl. North-Up rotation) so the hover
            // pixel is correct even when the image is rotated.
            let (img_x, img_y) = {
                let t = transform.borrow();
                let rot = *rotation.borrow();
                screen_to_image(x, y, t.scale, t.offset_x, t.offset_y, rot, w, h)
            };

            if on_image(img_x, img_y, w, h) {
                *local_hover.borrow_mut() = Some((img_x, img_y));
                // Publish the hover as SKY so other tabs can follow it by RA/Dec.
                if let Some(ref wcs) = wcs {
                    let (ra, dec) = wcs.pixel_to_sky(img_x, img_y);
                    shared.borrow_mut().hover = Some((ra, dec));
                }

                let mut text = format!("Pixel: ({:.0}, {:.0})", img_x, img_y);
                if let Some(ref wcs) = wcs {
                    let (ra, dec) = wcs.pixel_to_sky(img_x, img_y);
                    let (ra_str, dec_str) = WcsInfo::format_coords(ra, dec);
                    text = format!("{} | RA: {} Dec: {}", text, ra_str, dec_str);
                }
                coord_label.set_text(&text);
            } else {
                *local_hover.borrow_mut() = None;
                if wcs.is_some() {
                    shared.borrow_mut().hover = None;
                }
                coord_label.set_text("");
            }

            drawing_area.queue_draw();
        });

        motion.connect_leave(move |_| {
            // Don't clear - leave last position for linked crosshairs
        });

        self.drawing_area.add_controller(motion);
    }

    /// Call `f` with the IMAGE pixel of a left click, when one is installed.
    ///
    /// Image coordinates rather than screen, because that is what an annotation
    /// is anchored in — converting at the edge means the viewer never handles a
    /// screen coordinate it might forget to transform.
    pub fn set_on_left_click(&self, f: impl Fn(f64, f64) + 'static) {
        *self.on_left_click.borrow_mut() = Some(Box::new(f));
        // Say so. A mode that changes what a click does and looks identical is
        // a mode people fight with.
        self.drawing_area.set_cursor_from_name(Some("crosshair"));
    }

    /// Remove the left-click hook, restoring plain panning behaviour.
    pub fn clear_on_left_click(&self) {
        *self.on_left_click.borrow_mut() = None;
        self.drawing_area.set_cursor_from_name(Some("default"));
    }

    fn setup_left_click(self: &Rc<Self>) {
        let click = gtk::GestureClick::new();
        click.set_button(1);
        // Ahead of the pan gesture, which also wants button 1.
        click.set_propagation_phase(gtk::PropagationPhase::Capture);
        let canvas = Rc::downgrade(self);
        click.connect_pressed(move |gesture, _n, x, y| {
            let Some(canvas) = canvas.upgrade() else {
                return;
            };
            // Nothing installed means nothing to do — panning and selection are
            // unaffected, which is why this is a hook and not a mode the canvas
            // knows about.
            let has_handler = canvas.on_left_click.borrow().is_some();
            if !has_handler {
                return;
            }
            // Shift means "move the image, not the marks" — you need to
            // reposition while drawing, and leaving the mode to do it and
            // coming back is the kind of thing that makes a mode annoying.
            if gesture
                .current_event_state()
                .contains(gtk::gdk::ModifierType::SHIFT_MASK)
            {
                return;
            }
            let (img_x, img_y) = {
                let t = canvas.transform.borrow();
                let rot = *canvas.rotation.borrow();
                screen_to_image(
                    x,
                    y,
                    t.scale,
                    t.offset_x,
                    t.offset_y,
                    rot,
                    canvas.img_width,
                    canvas.img_height,
                )
            };
            if !on_image(img_x, img_y, canvas.img_width, canvas.img_height) {
                return;
            }
            // Ours: stops the pan gesture picking the same press up.
            gesture.set_state(gtk::EventSequenceState::Claimed);
            // The borrow is released before the callback runs: it may reach
            // back into the canvas to add a mark, and holding a RefCell across
            // a callback is how that becomes a panic.
            let handler = canvas.on_left_click.borrow_mut().take();
            if let Some(f) = handler {
                f(img_x, img_y);
                *canvas.on_left_click.borrow_mut() = Some(f);
            }
        });
        self.drawing_area.add_controller(click);
    }

    fn setup_right_click_crosshair(&self) {
        let click = gtk::GestureClick::new();
        click.set_button(3); // Right mouse button
        let transform = self.transform.clone();
        let rotation = self.rotation.clone();
        let crosshair_placed = self.crosshair_placed.clone();
        let shared = self.shared.clone();
        let drawing_area = self.drawing_area.clone();
        let on_placed = self.on_crosshair_placed.clone();
        let wcs = self.wcs.clone();
        let w = self.img_width;
        let h = self.img_height;

        click.connect_pressed(move |_, _n, x, y| {
            let (img_x, img_y) = {
                let t = transform.borrow();
                let rot = *rotation.borrow();
                screen_to_image(x, y, t.scale, t.offset_x, t.offset_y, rot, w, h)
            };
            if on_image(img_x, img_y, w, h) {
                let pos = Some((img_x, img_y));
                *crosshair_placed.borrow_mut() = pos;
                // Publish as sky so linked tabs can follow this crosshair.
                if let Some(ref wcs) = wcs {
                    let (ra, dec) = wcs.pixel_to_sky(img_x, img_y);
                    shared.borrow_mut().placed = Some((ra, dec));
                }
                drawing_area.queue_draw();
                if let Some(cb) = on_placed.borrow().as_ref() {
                    cb(pos);
                }
            }
        });

        self.drawing_area.add_controller(click);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn on_image_bounds() {
        assert!(on_image(0.0, 0.0, 10, 10));
        assert!(on_image(9.9, 9.9, 10, 10));
        assert!(!on_image(-0.1, 5.0, 10, 10));
        assert!(!on_image(5.0, 10.0, 10, 10));
    }

    #[test]
    fn image_to_screen_identity_without_rotation() {
        let (sx, sy) = image_to_screen(4.0, 3.0, 2.0, 10.0, 20.0, 0.0, 100, 100);
        assert!((sx - (4.0 * 2.0 + 10.0)).abs() < 1e-9);
        assert!((sy - (3.0 * 2.0 + 20.0)).abs() < 1e-9);
    }

    #[test]
    fn image_screen_round_trip_with_rotation() {
        // A 30° rotation must round-trip back to the original pixel.
        let (scale, ox, oy, rot, w, h) = (1.5, 12.0, -7.0, std::f64::consts::FRAC_PI_6, 64, 48);
        for &(px, py) in &[(0.0, 0.0), (63.0, 47.0), (20.0, 33.0)] {
            let (sx, sy) = image_to_screen(px, py, scale, ox, oy, rot, w, h);
            let (rx, ry) = screen_to_image(sx, sy, scale, ox, oy, rot, w, h);
            assert!((rx - px).abs() < 1e-6, "px {px} -> {rx}");
            assert!((ry - py).abs() < 1e-6, "py {py} -> {ry}");
        }
    }

    #[test]
    fn image_to_screen_center_is_rotation_fixed_point() {
        // The image centre maps to the same screen point regardless of rotation.
        let (scale, ox, oy, w, h) = (2.0, 5.0, 9.0, 20, 10);
        let (cx, cy) = (w as f64 / 2.0, h as f64 / 2.0);
        let a = image_to_screen(cx, cy, scale, ox, oy, 0.0, w, h);
        let b = image_to_screen(cx, cy, scale, ox, oy, 1.234, w, h);
        assert!((a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9);
    }

    #[test]
    fn choose_hover_prefers_sky_when_linked_with_wcs() {
        // Linked + WCS → use the sky-mapped pixel (even if that is None/off-map).
        assert_eq!(
            choose_hover_pixel(true, true, Some((3.0, 4.0)), Some((9.0, 9.0))),
            Some((3.0, 4.0))
        );
        assert_eq!(choose_hover_pixel(true, true, None, Some((9.0, 9.0))), None);
        // Not linked → own local hover.
        assert_eq!(
            choose_hover_pixel(false, true, Some((3.0, 4.0)), Some((9.0, 9.0))),
            Some((9.0, 9.0))
        );
        // Linked but no WCS → fall back to own local hover.
        assert_eq!(
            choose_hover_pixel(true, false, Some((3.0, 4.0)), Some((9.0, 9.0))),
            Some((9.0, 9.0))
        );
    }
}

#[cfg(test)]
mod capture_size_tests {
    //! The capture-size rule, checked without a display.
    use super::validate_capture_size;

    #[test]
    fn a_drawable_size_is_accepted() {
        assert!(validate_capture_size(400, 300).is_ok());
        assert!(validate_capture_size(1, 1).is_ok());
    }

    #[test]
    fn an_impossible_size_is_refused_rather_than_allocated() {
        for (w, h) in [(0, 300), (400, 0), (-1, 300), (400, -1), (0, 0)] {
            let err = validate_capture_size(w, h).expect_err(&format!("{w}x{h} should be refused"));
            assert!(err.contains("invalid capture size"), "{err}");
        }
    }

    #[test]
    fn an_enormous_size_is_refused_before_the_allocation() {
        // A caller asking for a gigapixel capture gets an answer, not a
        // process that tries to allocate 4 GB and is killed.
        let err = validate_capture_size(100_000, 100_000).expect_err("should refuse");
        assert!(err.contains("too large"), "{err}");
    }
}

/// The FITS canvas as a place to draw marks.
///
/// Two methods, and they are the entire difference between annotating a flat
/// image and annotating a rotating volume.
impl crate::helpers::annotation_render::AnnotationSurface for FitsCanvas {
    fn project(&self, anchor: &crate::models::annotation::Anchor) -> Option<(f64, f64)> {
        self.project_anchor(anchor)
    }

    fn units_to_pixels(&self, anchor: &crate::models::annotation::Anchor) -> f64 {
        self.annotation_scale(anchor)
    }
}
