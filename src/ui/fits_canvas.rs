use crate::models::fits_image::WcsInfo;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use std::cell::RefCell;
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

type OnCrosshairPlacedCallback = Rc<RefCell<Option<Box<dyn Fn(Option<(f64, f64)>)>>>>;

pub struct FitsCanvas {
    widget: gtk::Box,
    drawing_area: gtk::DrawingArea,
    coord_label: gtk::Label,
    pixel_data: Rc<RefCell<Vec<u8>>>,
    img_width: usize,
    img_height: usize,
    transform: Rc<RefCell<ViewTransform>>,
    shared_cursor: Rc<RefCell<Option<(f64, f64)>>>,
    /// A right-clicked persistent crosshair position (in image-space).
    crosshair_placed: Rc<RefCell<Option<(f64, f64)>>>,
    /// Rotation angle in radians (for North Up).
    rotation: Rc<RefCell<f64>>,
    wcs: Option<WcsInfo>,
    on_crosshair_placed: OnCrosshairPlacedCallback,
}

impl FitsCanvas {
    pub fn new(
        width: usize,
        height: usize,
        rgba: Vec<u8>,
        shared_cursor: Rc<RefCell<Option<(f64, f64)>>>,
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
            shared_cursor,
            crosshair_placed: Rc::new(RefCell::new(None)),
            rotation: Rc::new(RefCell::new(0.0)),
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

    /// Set the North Up rotation angle (radians). 0 disables rotation.
    pub fn set_rotation(&self, angle_rad: f64) {
        *self.rotation.borrow_mut() = angle_rad;
        self.drawing_area.queue_draw();
    }

    /// Place or clear the persistent crosshair (image-space coordinates).
    pub fn set_crosshair(&self, pos: Option<(f64, f64)>) {
        *self.crosshair_placed.borrow_mut() = pos;
        self.drawing_area.queue_draw();
        if let Some(cb) = self.on_crosshair_placed.borrow().as_ref() {
            cb(pos);
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
    pub fn go_to_world_coord(&self, ra: f64, dec: f64) {
        if let Some(ref wcs) = self.wcs {
            let (px, py) = wcs.sky_to_pixel(ra, dec);
            self.set_crosshair(Some((px, py)));
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

    // ── Drawing ──────────────────────────────────────────────────────────────

    fn setup_draw(&self) {
        let pixel_data = self.pixel_data.clone();
        let transform = self.transform.clone();
        let rotation = self.rotation.clone();
        let w = self.img_width;
        let h = self.img_height;
        let shared_cursor = self.shared_cursor.clone();
        let crosshair_placed = self.crosshair_placed.clone();
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

                // Create an image surface from RGBA data
                let stride = cairo::Format::ARgb32
                    .stride_for_width(w as u32)
                    .unwrap_or(w as i32 * 4);
                // Convert RGBA to cairo's BGRA (ARgb32) format
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

                if let Ok(surface) = cairo::ImageSurface::create_for_data(
                    cairo_data,
                    cairo::Format::ARgb32,
                    w as i32,
                    h as i32,
                    stride,
                ) {
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

                // Draw hover crosshair (green dashed, linked across tabs)
                if let Some((cx, cy)) = *shared_cursor.borrow() {
                    let sx = cx * t.scale + t.offset_x;
                    let sy = cy * t.scale + t.offset_y;

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

                // Draw placed crosshair (solid red) with optional RA/Dec label
                if let Some((cx, cy)) = *crosshair_placed.borrow() {
                    let sx = cx * t.scale + t.offset_x;
                    let sy = cy * t.scale + t.offset_y;

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

        scroll_controller.connect_scroll(move |_, _dx, dy| {
            let mut t = transform.borrow_mut();
            let factor = if dy < 0.0 { 1.15 } else { 1.0 / 1.15 };
            t.scale = (t.scale * factor).clamp(0.1, 50.0);
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
        let shared_cursor = self.shared_cursor.clone();
        let drawing_area = self.drawing_area.clone();
        let coord_label = self.coord_label.clone();
        let wcs = self.wcs.clone();
        let w = self.img_width;
        let h = self.img_height;

        motion.connect_motion(move |_, x, y| {
            let t = transform.borrow();
            // Convert widget coords to image coords
            let img_x = (x - t.offset_x) / t.scale;
            let img_y = (y - t.offset_y) / t.scale;

            if img_x >= 0.0 && img_x < w as f64 && img_y >= 0.0 && img_y < h as f64 {
                *shared_cursor.borrow_mut() = Some((img_x, img_y));

                let mut text = format!("Pixel: ({:.0}, {:.0})", img_x, img_y);
                if let Some(ref wcs) = wcs {
                    let (ra, dec) = wcs.pixel_to_sky(img_x, img_y);
                    let (ra_str, dec_str) = WcsInfo::format_coords(ra, dec);
                    text = format!("{} | RA: {} Dec: {}", text, ra_str, dec_str);
                }
                coord_label.set_text(&text);
            } else {
                *shared_cursor.borrow_mut() = None;
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
        let crosshair_placed = self.crosshair_placed.clone();
        let drawing_area = self.drawing_area.clone();
        let on_placed = self.on_crosshair_placed.clone();
        let w = self.img_width;
        let h = self.img_height;

        click.connect_pressed(move |_, _n, x, y| {
            let t = transform.borrow();
            let img_x = (x - t.offset_x) / t.scale;
            let img_y = (y - t.offset_y) / t.scale;
            if img_x >= 0.0 && img_x < w as f64 && img_y >= 0.0 && img_y < h as f64 {
                let pos = Some((img_x, img_y));
                *crosshair_placed.borrow_mut() = pos;
                drawing_area.queue_draw();
                if let Some(cb) = on_placed.borrow().as_ref() {
                    cb(pos);
                }
            }
        });

        self.drawing_area.add_controller(click);
    }
}
