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

pub struct FitsCanvas {
    widget: gtk::Box,
    drawing_area: gtk::DrawingArea,
    coord_label: gtk::Label,
    pixel_data: Rc<RefCell<Vec<u8>>>,
    img_width: usize,
    img_height: usize,
    transform: Rc<RefCell<ViewTransform>>,
    shared_cursor: Rc<RefCell<Option<(f64, f64)>>>,
    wcs: Option<WcsInfo>,
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
            wcs,
        });

        canvas.setup_draw();
        canvas.setup_scroll_zoom();
        canvas.setup_drag_pan();
        canvas.setup_motion_tracking();

        canvas
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.widget
    }

    pub fn update_image(&self, rgba: Vec<u8>) {
        *self.pixel_data.borrow_mut() = rgba;
        self.drawing_area.queue_draw();
    }

    fn setup_draw(&self) {
        let pixel_data = self.pixel_data.clone();
        let transform = self.transform.clone();
        let w = self.img_width;
        let h = self.img_height;
        let shared_cursor = self.shared_cursor.clone();

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
                    cr.translate(t.offset_x, t.offset_y);
                    cr.scale(t.scale, t.scale);
                    cr.set_source_surface(&surface, 0.0, 0.0).ok();
                    // Use nearest-neighbor for pixel-sharp rendering
                    let pattern = cr.source();
                    pattern.set_filter(cairo::Filter::Nearest);
                    cr.paint().ok();
                    cr.restore().ok();
                }

                // Draw crosshair if shared cursor has a position
                if let Some((cx, cy)) = *shared_cursor.borrow() {
                    let sx = cx * t.scale + t.offset_x;
                    let sy = cy * t.scale + t.offset_y;

                    cr.set_source_rgba(0.0, 1.0, 0.0, 0.7);
                    cr.set_line_width(1.0);

                    // Vertical line
                    cr.move_to(sx, 0.0);
                    cr.line_to(sx, widget_h as f64);
                    cr.stroke().ok();

                    // Horizontal line
                    cr.move_to(0.0, sy);
                    cr.line_to(widget_w as f64, sy);
                    cr.stroke().ok();
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
}
