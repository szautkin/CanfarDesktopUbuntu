use crate::models::fits_image::WcsInfo;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

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
    pub fn set_zoom(&self, scale: f64) {
        self.transform.borrow_mut().scale = scale.clamp(0.01, 100.0);
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

    /// Return the optional WCS (needed by the coordinate panel for validation).
    pub fn wcs(&self) -> Option<&WcsInfo> {
        self.wcs.as_ref()
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

    fn setup_draw(&self) {
        let pixel_data = self.pixel_data.clone();
        let transform = self.transform.clone();
        let rotation = self.rotation.clone();
        let w = self.img_width;
        let h = self.img_height;
        let shared = self.shared.clone();
        let local_hover = self.local_hover.clone();
        let crosshair_placed = self.crosshair_placed.clone();
        let blink_overlay = self.blink_overlay.clone();
        let blink_opacity = self.blink_opacity.clone();
        let wcs = self.wcs.clone();

        self.drawing_area
            .set_draw_func(move |_area, cr, widget_w, widget_h| {
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
                    let (sx, sy) =
                        image_to_screen(cx, cy, t.scale, t.offset_x, t.offset_y, rot, w, h);

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
                    let (sx, sy) =
                        image_to_screen(cx, cy, t.scale, t.offset_x, t.offset_y, rot, w, h);

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

                        cr.select_font_face(
                            "monospace",
                            cairo::FontSlant::Normal,
                            cairo::FontWeight::Normal,
                        );
                        cr.set_font_size(11.0);

                        if let Ok(extents) = cr.text_extents(&text) {
                            let padding = 4.0;
                            let box_w = extents.width() + padding * 2.0;
                            let box_h = extents.height() + padding * 2.0;
                            let box_x = (sx + 8.0).min(widget_w as f64 - box_w - 4.0);
                            let box_y = (sy + 8.0).min(widget_h as f64 - box_h - 4.0);

                            // Background
                            cr.set_source_rgba(0.0, 0.0, 0.0, 0.7);
                            cr.rectangle(box_x, box_y, box_w, box_h);
                            let _ = cr.fill();

                            // Text
                            cr.set_source_rgba(1.0, 1.0, 1.0, 1.0);
                            cr.move_to(box_x + padding, box_y + padding + extents.height());
                            let _ = cr.show_text(&text);
                        }
                    }
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
                let s1 = (s0 * factor).clamp(0.1, 50.0);
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

    fn setup_drag_pan(&self) {
        let drag = gtk::GestureDrag::new();
        drag.set_button(1); // Left mouse button
        let transform = self.transform.clone();
        let drawing_area = self.drawing_area.clone();
        let start_offset = Rc::new(RefCell::new((0.0, 0.0)));

        let so = start_offset.clone();
        let t = transform.clone();
        drag.connect_drag_begin(move |_, _x, _y| {
            let t = t.borrow();
            *so.borrow_mut() = (t.offset_x, t.offset_y);
        });

        let so = start_offset;
        drag.connect_drag_update(move |_, dx, dy| {
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
        let (scale, ox, oy, rot, w, h) = (1.5, 12.0, -7.0, 0.5236_f64, 64, 48);
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
