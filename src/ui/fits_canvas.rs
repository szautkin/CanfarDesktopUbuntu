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
#[derive(Clone, Copy)]
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

/// What a press on the selected mark took hold of.
#[derive(Clone, Copy, PartialEq)]
enum Grab {
    /// A corner grip: the drag changes the size.
    Handle,
    /// Inside the shape: the drag moves it.
    Body,
}

/// What a draw includes beyond the picture itself.
///
/// A bare `chrome: bool` was fine while there was one choice; a second one —
/// whether to lay down the ground — makes two positional booleans at every call
/// site, which is the point at which nobody can read `draw(cr, w, h, false,
/// true)` and say what it does.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DrawOpts {
    /// Editing chrome: the grips, the shape being dragged out, and the ink that
    /// says which mark is selected. On screen only — none of it means anything
    /// to someone reading an exported figure.
    pub chrome: bool,
    /// Lay down the dark ground. Off for a transparent export, where the
    /// letterboxing around the image should keep its alpha.
    pub background: bool,
}

impl DrawOpts {
    /// What the user is looking at.
    pub const SCREEN: Self = Self {
        chrome: true,
        background: true,
    };
    /// What an agent is shown, and what an opaque export writes.
    pub const CAPTURE: Self = Self {
        chrome: false,
        background: true,
    };

    /// An export, with or without a ground.
    pub fn export(transparent: bool) -> Self {
        Self {
            chrome: false,
            background: !transparent,
        }
    }
}

/// A rectangle of the canvas, in its own screen coordinates.
///
/// Screen rather than image pixels because that is what a drag produces, and
/// because on a north-up rotated frame a screen-aligned drag is not a rectangle
/// in image pixels at all — converting it would export a different region from
/// the one that was drawn.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewRegion {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl ViewRegion {
    /// The whole of a view.
    pub fn whole(view_w: i32, view_h: i32) -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            width: f64::from(view_w),
            height: f64::from(view_h),
        }
    }

    /// The rectangle between two corners, whichever way round they were
    /// dragged. Dragging up-and-left is as natural as down-and-right, and a
    /// negative width is not a rectangle.
    pub fn between(a: (f64, f64), b: (f64, f64)) -> Self {
        Self {
            x: a.0.min(b.0),
            y: a.1.min(b.1),
            width: (b.0 - a.0).abs(),
            height: (b.1 - a.1).abs(),
        }
    }

    pub fn is_usable(&self) -> bool {
        self.width > 0.0 && self.height > 0.0
    }
}

/// Told the region a select-area drag produced.
type RegionCallback = RefCell<Option<Rc<dyn Fn(ViewRegion)>>>;

/// Which mode owns a press on the canvas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PressOwner {
    /// Pan, or pick up a mark — the canvas's own behaviour.
    Canvas,
    /// Draw mode is armed and the press is on empty image.
    Drawing,
    /// Select-area is armed.
    Selecting,
}

/// The scale that shows the WHOLE image in a viewport of `viewport`.
///
/// Never above 1.0. A 64x64 thumbnail blown up to fill a 1600px viewport is a
/// wall of fat pixels and tells you nothing an honest 64 pixels would not;
/// small images open at 100% with space around them, which is what they look
/// like. The limit is the tighter axis, so the whole frame fits both ways.
///
/// Returns 1.0 for a degenerate image or viewport rather than dividing by zero
/// — the caller cannot fit to a viewport that does not exist yet, and 100% is
/// the honest thing to show until it does.
pub fn fit_scale(image: (f64, f64), viewport: (f64, f64)) -> f64 {
    let (iw, ih) = image;
    let (vw, vh) = viewport;
    if !(iw > 0.0 && ih > 0.0 && vw > 0.0 && vh > 0.0) {
        return 1.0;
    }
    (vw / iw).min(vh / ih).min(1.0)
}

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
    /// True while the export's select-area mode is armed.
    selecting: Cell<bool>,
    /// The rectangle being dragged out right now, in screen coordinates.
    pending_region: RefCell<Option<ViewRegion>>,
    /// Told the region when the drag ends.
    on_region_selected: RegionCallback,
    /// Fit the whole image into the viewport on the first REAL allocation.
    ///
    /// Not at load: a `DrawingArea` reports 0x0 until it is allocated and
    /// `viewport_size` falls back to the REQUESTED size, which has nothing to
    /// do with the window the user has — fitting then fits to a guess.
    ///
    /// Cleared the first time it fires, so resizing the window later never
    /// re-fits. A viewer that re-fits on resize throws away the zoom you chose
    /// every time you drag the window edge. A canvas is built per image, so a
    /// new file or a different HDU gets a fresh fit without asking.
    needs_fit: Cell<bool>,
    transform: Rc<RefCell<ViewTransform>>,
    /// Cross-tab shared crosshair/hover state (linked by sky).
    shared: SharedSkyRef,
    /// This canvas's own last hover pixel (image-space) — zoom anchor + the
    /// fallback marker when sky-linking is off or there is no WCS.
    local_hover: Rc<RefCell<Option<(f64, f64)>>>,
    /// A right-clicked persistent crosshair position (in image-space).
    crosshair_placed: Rc<RefCell<Option<(f64, f64)>>>,
    /// The pixel data as cairo wants it, built once per image rather than once
    /// per frame.
    ///
    /// `rgba_to_surface` premultiplies and channel-swaps every pixel. That ran
    /// on EVERY draw: for the 11471x4593 NIRCam frame that is 52 million pixels
    /// converted per redraw, so anything that caused a repaint — a popover
    /// taking a keystroke, a pointer moving — paid for the whole image again.
    /// The conversion depends only on the data, so it is done when the data
    /// changes.
    surface_cache: RefCell<Option<cairo::ImageSurface>>,
    /// Holds the drawing area, and any transient editor placed over it.
    image_overlay: gtk::Overlay,
    /// Installed by the viewer while draw mode is on. Shared with the pan
    /// gesture, which stands down while it is set.
    ///
    /// Called on RELEASE with the centre in image pixels and the half-extent
    /// the user dragged out, also in image pixels.
    #[allow(clippy::type_complexity)]
    on_left_click: Rc<RefCell<Option<Box<dyn Fn(f64, f64, f64)>>>>,
    /// The shape being dragged out: `(centre_x, centre_y, half_extent)` in
    /// image pixels. Drawn as a preview so the size is chosen by eye.
    pending_shape: Rc<RefCell<Option<(f64, f64, f64)>>>,
    /// What the preview should look like, ASKED at draw time.
    ///
    /// A kind handed over when draw mode was armed would be the kind you got
    /// for ever after — the exact bug the shape picker already had. The canvas
    /// asks the picker instead, so the preview cannot fall out of step with
    /// what the release will produce.
    #[allow(clippy::type_complexity)]
    preview_kind: Rc<RefCell<Option<Box<dyn Fn() -> crate::models::annotation::AnnotationKind>>>>,
    /// What the current drag is doing to the selected mark, if anything.
    grab: Rc<RefCell<Option<Grab>>>,
    /// The mark this press selected, if it selected one. A press that turns
    /// into a drag is a move; one that does not is a request to edit.
    tapped: Rc<RefCell<Option<String>>>,
    /// Told whenever the set of marks changes, by any route.
    ///
    /// The canvas owns the marks, so it is the one place that knows they
    /// changed. Pushing a refresh from every caller instead meant each new
    /// route had to remember — and the MCP tools and the load-on-open path both
    /// forgot, so an agent's marks and a reopened file's marks were on the
    /// image and absent from the list.
    #[allow(clippy::type_complexity)]
    on_annotations_changed: Rc<RefCell<Option<Box<dyn Fn()>>>>,
    /// Told whenever marks MOVE ON SCREEN — because one is being dragged, or
    /// because the view panned, zoomed or rotated under them.
    ///
    /// Anything anchored to a mark's on-screen position follows this. It used
    /// to fire only for a mark being dragged, so panning the image slid the
    /// marks along and left the open label editor behind.
    #[allow(clippy::type_complexity)]
    on_marks_moved: Rc<RefCell<Option<Box<dyn Fn()>>>>,
    /// Told with an id when a mark's LABEL is clicked, so the viewer can open
    /// it for editing.
    #[allow(clippy::type_complexity)]
    on_label_clicked: Rc<RefCell<Option<Box<dyn Fn(&str)>>>>,
    /// Told when a click on the image changes which mark is selected, so the
    /// list can follow the canvas.
    #[allow(clippy::type_complexity)]
    on_selection_changed: Rc<RefCell<Option<Box<dyn Fn()>>>>,
    /// Marks drawn on this image, by the user or an agent.
    annotations: Rc<RefCell<Vec<crate::models::annotation::Annotation>>>,
    /// The selected mark's id — picked out on the canvas and in the panel.
    selected_annotation: Rc<RefCell<Option<String>>>,
    /// The mark being EDITED: grips out, label field open. Selection alone is
    /// quieter — it says "this one", not "you are changing this one".
    editing_annotation: Rc<RefCell<Option<String>>>,
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

        // The drawing area sits under an Overlay so transient editing widgets
        // — the label field — can be placed ON the image. A GtkPopover cannot:
        // without autohide it is its own surface, and one was seen floating
        // above an unrelated application's window. An overlay child is clipped
        // to this window and moves with it.
        let image_overlay = gtk::Overlay::new();
        image_overlay.set_child(Some(&drawing_area));
        widget.append(&image_overlay);
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
            selecting: Cell::new(false),
            pending_region: RefCell::new(None),
            on_region_selected: RefCell::new(None),
            needs_fit: Cell::new(true),
            image_overlay,
            surface_cache: RefCell::new(None),
            on_left_click: Rc::new(RefCell::new(None)),
            pending_shape: Rc::new(RefCell::new(None)),
            preview_kind: Rc::new(RefCell::new(None)),
            grab: Rc::new(RefCell::new(None)),
            tapped: Rc::new(RefCell::new(None)),
            on_annotations_changed: Rc::new(RefCell::new(None)),
            on_marks_moved: Rc::new(RefCell::new(None)),
            on_label_clicked: Rc::new(RefCell::new(None)),
            on_selection_changed: Rc::new(RefCell::new(None)),
            annotations: Rc::new(RefCell::new(Vec::new())),
            selected_annotation: Rc::new(RefCell::new(None)),
            editing_annotation: Rc::new(RefCell::new(None)),
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
        canvas.setup_select_region();
        canvas.setup_right_click_crosshair();

        canvas
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.widget
    }

    pub fn update_image(&self, rgba: Vec<u8>) {
        *self.pixel_data.borrow_mut() = rgba;
        // The one writer of the pixels is the one place the cache is dropped,
        // so they cannot disagree about which image is on screen.
        *self.surface_cache.borrow_mut() = None;
        self.drawing_area.queue_draw();
    }

    /// The image as a cairo surface, built on demand and kept.
    fn image_surface(&self, w: usize, h: usize) -> Option<cairo::ImageSurface> {
        if let Some(surface) = self.surface_cache.borrow().as_ref() {
            return Some(surface.clone());
        }
        let built = rgba_to_surface(&self.pixel_data.borrow(), w, h)?;
        *self.surface_cache.borrow_mut() = Some(built.clone());
        Some(built)
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
    /// Fit the whole image in the viewport, once, on the first real allocation.
    ///
    /// Returns whether it fitted, so the caller can tell a fit from a no-op.
    fn apply_fit(&self) -> bool {
        if !self.needs_fit.get() {
            return false;
        }
        // The ALLOCATION, not `viewport_size` — that falls back to the
        // requested size, and fitting to that is fitting to a guess.
        let (vw, vh) = (
            self.drawing_area.width() as f64,
            self.drawing_area.height() as f64,
        );
        if vw <= 0.0 || vh <= 0.0 {
            return false;
        }
        let (iw, ih) = (self.img_width as f64, self.img_height as f64);
        let scale = fit_scale((iw, ih), (vw, vh));
        {
            let mut t = self.transform.borrow_mut();
            t.scale = scale;
            // Centred, so an image smaller than the viewport sits in the
            // middle rather than in the top-left corner with the rest empty.
            t.offset_x = (vw - iw * scale) / 2.0;
            t.offset_y = (vh - ih * scale) / 2.0;
        }
        self.needs_fit.set(false);
        self.view_changed();
        true
    }

    /// Whether a fit is still pending. For the probe, and for asking whether a
    /// deliberate zoom has already cancelled it.
    pub fn fit_pending(&self) -> bool {
        self.needs_fit.get()
    }

    /// Fit now, against a stated viewport, for a probe that has no allocation.
    ///
    /// The real path reads the widget's size, which GTK never gives a headless
    /// process; this takes the number so the arithmetic and the one-shot rule
    /// can still be exercised.
    pub fn fit_to_viewport_for_probe(&self, vw: f64, vh: f64) -> bool {
        if !self.needs_fit.get() || vw <= 0.0 || vh <= 0.0 {
            return false;
        }
        let (iw, ih) = (self.img_width as f64, self.img_height as f64);
        let scale = fit_scale((iw, ih), (vw, vh));
        {
            let mut t = self.transform.borrow_mut();
            t.scale = scale;
            t.offset_x = (vw - iw * scale) / 2.0;
            t.offset_y = (vh - ih * scale) / 2.0;
        }
        self.needs_fit.set(false);
        self.view_changed();
        true
    }

    /// Stop the pending fit. Anything that sets a zoom deliberately calls this.
    pub fn cancel_fit(&self) {
        self.needs_fit.set(false);
    }

    pub fn set_zoom(&self, scale: f64) {
        // Someone has chosen a zoom — the box, the wheel, sync-zoom across
        // tabs, or an agent. Whatever it is, it is more specific than "fit",
        // so the pending fit must not arrive afterwards and overwrite it.
        self.needs_fit.set(false);
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
        self.view_changed();
    }

    /// Current zoom scale.
    pub fn zoom_scale(&self) -> f64 {
        self.transform.borrow().scale
    }

    /// Reset zoom and pan to default.
    pub fn reset_view(&self) {
        *self.transform.borrow_mut() = ViewTransform::default();
        self.view_changed();
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
        self.view_changed();
    }

    /// Set the North Up rotation angle (radians). 0 disables rotation.
    pub fn set_rotation(&self, angle_rad: f64) {
        *self.rotation.borrow_mut() = angle_rad;
        self.view_changed();
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
        self.shared.borrow_mut().placed = pos.map(|(px, py)| w.display_to_sky(px, py));
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
            (Some((ra, dec)), Some(w)) => match w.sky_to_display(ra, dec) {
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
        self.wcs.as_ref().map(|w| w.display_to_sky(px, py))
    }

    /// Center the view on a given sky coordinate and place a crosshair there.
    /// If the coordinate maps outside the image, the crosshair is cleared rather
    /// than floated off the frame.
    pub fn go_to_world_coord(&self, ra: f64, dec: f64) {
        if let Some(ref wcs) = self.wcs {
            match wcs.sky_to_display(ra, dec) {
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
        self.draw_area_inner(cr, widget_w, widget_h, DrawOpts::SCREEN)
    }

    /// The working area, with or without editing chrome.
    ///
    /// The marks are identical either way — that is the whole point of one
    /// drawing serving the screen and the capture. Handles are not marks: they
    /// are the grips you drag to resize one, and an agent looking at
    /// `get_fits_image` should see what was drawn, not the tools for drawing
    /// it. So the destination decides the chrome and nothing else.
    fn draw_area_inner(&self, cr: &cairo::Context, widget_w: i32, widget_h: i32, opts: DrawOpts) {
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

        if opts.background {
            cr.set_source_rgb(0.1, 0.1, 0.1);
            let _ = cr.paint();
        }

        let data = pixel_data.borrow();
        if data.is_empty() || w == 0 || h == 0 {
            return;
        }

        let t = transform.borrow();
        let rot = *rotation.borrow();

        if let Some(surface) = self.image_surface(w, h) {
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
            (true, Some((ra, dec)), Some(w_ref)) => w_ref.sky_to_display(ra, dec),
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
                let (ra, dec) = w_ref.display_to_sky(cx, cy);
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

        // The shape being dragged out, in the same ink as a finished one so
        // what you release is what you saw. Chrome: it is a shape that does not
        // exist yet, and a capture or an export taken mid-drag should not
        // contain a half-made mark. The cube guards its preview the same way.
        if opts.chrome {
            if let Some((ix, iy, half)) = *self.pending_shape.borrow() {
                use crate::models::annotation::AnnotationKind;
                let (sx, sy) = self.image_to_screen_point(ix, iy);
                let r = (half * self.transform.borrow().scale).max(1.0);
                // Asked at draw time, not remembered: the picker can change while
                // drawing is armed, and the preview must be the shape you get.
                let kind = self
                    .preview_kind
                    .borrow()
                    .as_ref()
                    .map(|f| f())
                    .unwrap_or(AnnotationKind::Circle);
                crate::helpers::annotation_render::draw_preview(kind, sx, sy, r, cr);
            }
        }

        // Marks last, over everything, and drawn HERE — inside the function the
        // capture replays — so an agent's picture and the user's screen show
        // the same annotations without either path knowing about the other.
        //
        // Which mark is SELECTED or being EDITED is chrome, though the marks
        // themselves are not. Those two states colour a ring white or amber to
        // say "this is the one you clicked"; in an exported figure they say
        // nothing to a reader except that one mark is inexplicably a different
        // colour. Same rule as the grips, which was only half applied.
        let (selected, editing) = if opts.chrome {
            (
                self.selected_annotation.borrow().clone(),
                self.editing_annotation.borrow().clone(),
            )
        } else {
            (None, None)
        };
        crate::helpers::annotation_render::draw(
            &self.annotations.borrow(),
            self,
            selected.as_deref(),
            editing.as_deref(),
            cr,
            widget_w as f64,
            widget_h as f64,
        );

        if opts.chrome {
            self.draw_handles(cr);
            self.draw_pending_region(cr);
        }
    }

    /// The rectangle being dragged out for an export.
    ///
    /// Chrome, and firmly so: it is the tool for choosing a frame, not part of
    /// the picture. A dashed outline rather than the mark ink, because it is
    /// not a mark and should not read as one.
    fn draw_pending_region(&self, cr: &cairo::Context) {
        let Some(r) = *self.pending_region.borrow() else {
            return;
        };
        if !r.is_usable() {
            return;
        }
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.9);
        cr.set_line_width(1.0);
        cr.set_dash(&[4.0, 4.0], 0.0);
        cr.rectangle(r.x, r.y, r.width, r.height);
        cr.stroke().ok();
        cr.set_dash(&[], 0.0);
    }

    /// The four grips on the mark being edited.
    ///
    /// Only that one. A selected mark is pointed out, not opened up: grips on
    /// it would say "drag me" when nothing is expecting a drag.
    fn draw_handles(&self, cr: &cairo::Context) {
        let Some(mark) = self.editing_mark() else {
            return;
        };
        crate::helpers::annotation_render::draw_handles(&mark, self, cr);
    }

    /// The mark whose LABEL covers `(sx, sy)`, topmost first.
    ///
    /// Clicking the words is how you edit them — it is where you are already
    /// looking, and the shape itself means "select" rather than "rename".
    pub fn label_at(&self, sx: f64, sy: f64) -> Option<String> {
        for a in self.annotations.borrow().iter().rev() {
            if a.text.trim().is_empty() {
                continue;
            }
            if let Some(rect) = self.label_bounds(a) {
                if rect.0 <= sx && sx <= rect.2 && rect.1 <= sy && sy <= rect.3 {
                    return Some(a.id.clone());
                }
            }
        }
        None
    }

    /// A label's box on screen, as `(x0, y0, x1, y1)`.
    ///
    /// The width is estimated from the character count rather than measured:
    /// measuring needs a cairo context, this runs on a click, and a hit box a
    /// few pixels out is not something anyone can feel.
    fn label_bounds(
        &self,
        mark: &crate::models::annotation::Annotation,
    ) -> Option<(f64, f64, f64, f64)> {
        use crate::helpers::annotation_render::{leader_geometry, style};
        let (cx, cy) = self.project_anchor(&mark.anchor)?;
        let scale = self.annotation_scale(&mark.anchor);
        let (hw, hh) = mark
            .extent
            .map(|e| (e.half_width * scale, e.half_height * scale))
            .unwrap_or((3.0, 3.0));
        // Monospace at this size advances about 0.6 of the font size.
        let text_w = mark.text.chars().count() as f64 * style::FONT_SIZE * 0.62;
        let width = self.drawing_area.width().max(1) as f64;
        let (.., ey, _rule_end, text_x, _right) = leader_geometry(
            cx,
            cy,
            hw,
            hh,
            mark.kind != crate::models::annotation::AnnotationKind::Rect,
            mark.label_offset,
            text_w,
            width,
        );
        let pad = 4.0;
        Some((
            text_x - pad,
            ey - style::FONT_SIZE - pad,
            text_x + text_w + pad,
            ey + pad,
        ))
    }

    /// Whether `(sx, sy)` is on one of the edited mark's grips.
    fn handle_at(&self, sx: f64, sy: f64) -> bool {
        let Some(mark) = self.editing_mark() else {
            return false;
        };
        crate::helpers::annotation_render::handle_at(&mark, self, sx, sy)
    }

    /// Resize the selected mark so its corner sits under `(sx, sy)`.
    fn resize_selected_to(&self, sx: f64, sy: f64) {
        let Some(mark) = self.editing_mark() else {
            return;
        };
        let Some((cx, cy)) = self.project_anchor(&mark.anchor) else {
            return;
        };
        let scale = self.annotation_scale(&mark.anchor).max(f64::EPSILON);
        // Symmetric about the centre: the anchor is the subject, and a resize
        // that moved it would slide the mark off the thing it describes.
        let half_w = ((sx - cx).abs() / scale).max(f64::EPSILON);
        let half_h = ((sy - cy).abs() / scale).max(f64::EPSILON);
        let mut all = self.annotations.borrow_mut();
        if let Some(m) = all.iter_mut().find(|a| a.id == mark.id) {
            m.extent = Some(crate::models::annotation::Extent {
                half_width: half_w,
                half_height: half_h,
            });
        }
        drop(all);
        self.drawing_area.queue_draw();
    }

    /// Move the selected mark so its centre sits under `(sx, sy)`.
    fn move_selected_to(&self, sx: f64, sy: f64) {
        let Some(mark) = self.editing_mark() else {
            return;
        };
        let (ix, iy) = self.screen_to_image_point(sx, sy);
        if !on_image(ix, iy, self.img_width, self.img_height) {
            return;
        }
        use crate::models::annotation::Anchor;
        // Stays in the space it was created in: a sky mark stays on the sky, so
        // it still lands correctly on another image of the same field.
        let moved = match mark.anchor {
            Anchor::Sky { .. } => match self.wcs.as_ref() {
                Some(w) => {
                    let (ra, dec) = w.display_to_sky(ix, iy);
                    let a = Anchor::Sky {
                        ra_deg: ra,
                        dec_deg: dec,
                    };
                    if a.is_valid() {
                        a
                    } else {
                        return;
                    }
                }
                None => Anchor::ImagePixel { x: ix, y: iy },
            },
            _ => Anchor::ImagePixel { x: ix, y: iy },
        };
        let mut all = self.annotations.borrow_mut();
        if let Some(m) = all.iter_mut().find(|a| a.id == mark.id) {
            m.anchor = moved;
        }
        drop(all);
        self.drawing_area.queue_draw();
    }

    /// Whether a drag is currently reshaping a mark.
    pub fn is_editing_shape(&self) -> bool {
        self.grab.borrow().is_some()
    }

    /// The selected mark, if there is one.
    pub fn selected_mark(&self) -> Option<crate::models::annotation::Annotation> {
        self.mark_by_id(self.selected_annotation.borrow().clone())
    }

    /// The mark being edited, if there is one.
    pub fn editing_mark(&self) -> Option<crate::models::annotation::Annotation> {
        self.mark_by_id(self.editing_annotation.borrow().clone())
    }

    fn mark_by_id(&self, id: Option<String>) -> Option<crate::models::annotation::Annotation> {
        let id = id?;
        self.annotations
            .borrow()
            .iter()
            .find(|a| a.id == id)
            .cloned()
    }

    // ── Annotations ─────────────────────────────────────────────────────────

    /// Replace the marks on this canvas.
    pub fn set_annotations(&self, annotations: Vec<crate::models::annotation::Annotation>) {
        *self.annotations.borrow_mut() = annotations;
        self.drawing_area.queue_draw();
        // Announced from the one place the set can change, so no caller has to
        // remember to tell the list.
        let notify = self.on_annotations_changed.borrow();
        if let Some(f) = notify.as_ref() {
            f();
        }
    }

    pub fn annotations(&self) -> Vec<crate::models::annotation::Annotation> {
        self.annotations.borrow().clone()
    }

    pub fn set_on_annotations_changed(&self, f: impl Fn() + 'static) {
        *self.on_annotations_changed.borrow_mut() = Some(Box::new(f));
    }

    pub fn set_on_marks_moved(&self, f: impl Fn() + 'static) {
        *self.on_marks_moved.borrow_mut() = Some(Box::new(f));
    }

    /// Redraw, and tell anything anchored to a mark that it has moved.
    ///
    /// Every path that changes the view goes through this instead of calling
    /// `queue_draw` directly, so a new way to pan or zoom cannot forget.
    fn view_changed(&self) {
        self.drawing_area.queue_draw();
        let notify = self.on_marks_moved.borrow();
        if let Some(f) = notify.as_ref() {
            f();
        }
    }

    pub fn set_on_label_clicked(&self, f: impl Fn(&str) + 'static) {
        *self.on_label_clicked.borrow_mut() = Some(Box::new(f));
    }

    pub fn set_on_selection_changed(&self, f: impl Fn() + 'static) {
        *self.on_selection_changed.borrow_mut() = Some(Box::new(f));
    }

    pub fn set_selected_annotation(&self, id: Option<String>) {
        *self.selected_annotation.borrow_mut() = id;
        self.drawing_area.queue_draw();
    }

    pub fn selected_annotation(&self) -> Option<String> {
        self.selected_annotation.borrow().clone()
    }

    /// Mark `id` as the one being edited — grips appear on it alone.
    pub fn set_editing_annotation(&self, id: Option<String>) {
        *self.editing_annotation.borrow_mut() = id;
        self.drawing_area.queue_draw();
    }

    pub fn editing_annotation(&self) -> Option<String> {
        self.editing_annotation.borrow().clone()
    }

    /// The mark whose shape contains `(sx, sy)`, topmost first.
    ///
    /// Hit-testing is done in SCREEN space against the projected shape, so what
    /// the user can click is exactly what they can see — the alternative,
    /// testing in image space, quietly disagrees with the drawing wherever
    /// rotation is in play.
    pub fn annotation_at(&self, sx: f64, sy: f64) -> Option<String> {
        let anns = self.annotations.borrow().clone();
        crate::helpers::annotation_render::annotation_at(&anns, self, sx, sy)
    }

    /// The drawing area, for anchoring to a place on the image.
    pub fn drawing_area(&self) -> &gtk::DrawingArea {
        &self.drawing_area
    }

    /// Put `child` over the image, its top-left near `(x, y)` in device pixels.
    ///
    /// Kept inside the canvas so an editor for a mark near an edge stays
    /// reachable instead of hanging off it.
    pub fn place_over_image(&self, child: &impl IsA<gtk::Widget>, x: f64, y: f64) {
        let child = child.as_ref();
        if child.parent().is_none() {
            self.image_overlay.add_overlay(child);
        }
        child.set_halign(gtk::Align::Start);
        child.set_valign(gtk::Align::Start);
        self.position_over_image(child, x, y);
    }

    /// Move an already-placed child.
    pub fn position_over_image(&self, child: &impl IsA<gtk::Widget>, x: f64, y: f64) {
        let child = child.as_ref();
        let (aw, ah) = (self.drawing_area.width(), self.drawing_area.height());
        let (cw, ch) = (child.width().max(1), child.height().max(1));
        let max_x = (aw - cw).max(0) as f64;
        let max_y = (ah - ch).max(0) as f64;
        child.set_margin_start(x.clamp(0.0, max_x) as i32);
        child.set_margin_top(y.clamp(0.0, max_y) as i32);
    }

    /// Take a transient editor back off the image.
    pub fn remove_from_image(&self, child: &impl IsA<gtk::Widget>) {
        let child = child.as_ref();
        if child.parent().is_some() {
            self.image_overlay.remove_overlay(child);
        }
    }

    /// Where a mark's label sits on screen, as a rectangle to point at.
    ///
    /// Computed with the same `leader_geometry` the renderer uses, so the caret
    /// appears exactly where the text will be drawn rather than near it.
    pub fn leader_label_rect(
        &self,
        mark: &crate::models::annotation::Annotation,
    ) -> Option<gtk::gdk::Rectangle> {
        use crate::helpers::annotation_render::leader_geometry;
        let (cx, cy) = self.project_anchor(&mark.anchor)?;
        let scale = self.annotation_scale(&mark.anchor);
        let (hw, hh) = mark
            .extent
            .map(|e| (e.half_width * scale, e.half_height * scale))
            .unwrap_or((6.0, 6.0));
        let width = self.drawing_area.width().max(1) as f64;
        // A nominal text width: the label does not exist yet, and the caret
        // only needs to land on the rule.
        let (.., ey, _rule_end, text_x, _right) =
            leader_geometry(cx, cy, hw, hh, true, mark.label_offset, 90.0, width);
        Some(gtk::gdk::Rectangle::new(
            text_x as i32,
            (ey - 14.0).max(0.0) as i32,
            1,
            14,
        ))
    }

    /// How many of the anchor's units one image pixel is.
    ///
    /// The inverse of the scale the renderer asks for, without the zoom: a
    /// distance dragged out on screen is in image pixels, and the extent it
    /// becomes has to be in whatever the anchor counts in.
    pub fn units_per_image_pixel(&self, anchor: &crate::models::annotation::Anchor) -> f64 {
        let view = self.transform.borrow().scale.max(f64::EPSILON);
        let image_px_per_unit = self.annotation_scale(anchor) / view;
        if image_px_per_unit.is_finite() && image_px_per_unit > 0.0 {
            1.0 / image_px_per_unit
        } else {
            1.0
        }
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
            // Already a display coordinate: a dragged mark is stored from
            // `screen_to_image`. An agent placing one by array index on an
            // image with NO WCS is half a pixel off here, which is below the
            // size of the mark and not worth a conversion it would then read
            // back in its replies.
            Anchor::ImagePixel { x, y } => (x, y),
            // A sky anchor is placed through this image's OWN WCS, so a mark
            // made on one image lands correctly on another of the same field.
            Anchor::Sky { ra_deg, dec_deg } => {
                self.wcs.as_ref()?.sky_to_display(ra_deg, dec_deg)?
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
    /// Draw `region` of the view into a raster of `out_w` x `out_h`.
    ///
    /// By SUBSTITUTING the view transform, not by cropping a raster. The image,
    /// the crosshair and every mark are all projected through `self.transform`,
    /// so replacing it moves them together and correctly at any output size —
    /// which is the payoff of having one drawing function, and what a second
    /// renderer would lose. Cropping instead would tie an export's resolution
    /// to the window: a small region at 25% zoom would come out as a handful of
    /// blurry pixels.
    ///
    /// The transform maps image to screen as `screen = image * scale + offset`.
    /// Mapping `region` onto the raster is `out = (screen - region.origin) * k`,
    /// so composing gives `scale * k` and `(offset - origin) * k`.
    ///
    /// `k` is uniform — `ViewTransform` has one scale, and a non-uniform one
    /// would stretch the image — so a raster whose aspect differs from the
    /// region's gets the region fitted inside it and centred rather than
    /// distorted.
    fn draw_region_into(
        &self,
        cr: &cairo::Context,
        region: ViewRegion,
        out_w: i32,
        out_h: i32,
        opts: DrawOpts,
    ) {
        if !region.is_usable() {
            self.draw_area_inner(cr, out_w, out_h, opts);
            return;
        }
        let saved = *self.transform.borrow();
        let k = (f64::from(out_w) / region.width).min(f64::from(out_h) / region.height);
        {
            let mut t = self.transform.borrow_mut();
            t.scale = saved.scale * k;
            // Centre whatever the fit leaves over.
            t.offset_x =
                (saved.offset_x - region.x) * k + (f64::from(out_w) - region.width * k) / 2.0;
            t.offset_y =
                (saved.offset_y - region.y) * k + (f64::from(out_h) - region.height * k) / 2.0;
        }
        self.draw_area_inner(cr, out_w, out_h, opts);
        *self.transform.borrow_mut() = saved;
    }

    /// A region of the working area, as a PNG.
    ///
    /// `region` is in the coordinates of a view of `view_w` x `view_h` — the
    /// widget's own, normally. Taking the view size as an argument is what
    /// makes this reachable from a probe: `view_size()` is a widget allocation
    /// and GTK gives a headless process none, which is exactly how a capture
    /// that cropped instead of scaling shipped unnoticed.
    pub fn capture_region_surface(
        &self,
        view_w: i32,
        view_h: i32,
        region: ViewRegion,
        out_w: i32,
        out_h: i32,
        opts: DrawOpts,
    ) -> Result<cairo::ImageSurface, String> {
        validate_capture_size(out_w, out_h)?;
        let region = if region.is_usable() {
            region
        } else {
            ViewRegion::whole(view_w, view_h)
        };
        let surface = cairo::ImageSurface::create(cairo::Format::ARgb32, out_w, out_h)
            .map_err(|e| format!("cairo surface error: {e}"))?;
        {
            let cr =
                cairo::Context::new(&surface).map_err(|e| format!("cairo context error: {e}"))?;
            self.draw_region_into(&cr, region, out_w, out_h, opts);
        }
        Ok(surface)
    }

    /// The same region, encoded as a PNG.
    pub fn capture_region_png(
        &self,
        view_w: i32,
        view_h: i32,
        region: ViewRegion,
        out_w: i32,
        out_h: i32,
    ) -> Result<Vec<u8>, String> {
        let surface =
            self.capture_region_surface(view_w, view_h, region, out_w, out_h, DrawOpts::CAPTURE)?;
        let mut png: Vec<u8> = Vec::new();
        surface
            .write_to_png(&mut png)
            .map_err(|e| format!("PNG encode failed: {e}"))?;
        Ok(png)
    }

    pub fn capture_png(&self, width: i32, height: i32) -> Result<Vec<u8>, String> {
        validate_capture_size(width, height)?;
        let surface = cairo::ImageSurface::create(cairo::Format::ARgb32, width, height)
            .map_err(|e| format!("cairo surface error: {e}"))?;
        {
            let cr =
                cairo::Context::new(&surface).map_err(|e| format!("cairo context error: {e}"))?;
            // Draw the VIEW, scaled into the requested raster — not the view
            // clipped to it.
            //
            // The view transform is in absolute screen pixels, so handing
            // `draw_area_inner` a smaller size did not shrink anything: it drew
            // at the same scale and the raster simply ran out. A capture asked
            // for at 1024 from a 1400px-wide canvas returned the top-left
            // 1024px and reported `scale: 0.73`, so an agent got a crop
            // labelled as a faithful downscale — and the default limit is 1024,
            // which any maximised window exceeds.
            let (view_w, view_h) = self.view_size();
            self.draw_region_into(
                &cr,
                ViewRegion::whole(view_w, view_h),
                width,
                height,
                DrawOpts::CAPTURE,
            );
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
                    // The first frame with a real size is the first moment the
                    // viewport is known, so it is where a fit belongs. Doing it
                    // here rather than in `connect_resize` also covers the case
                    // where the area is allocated once and never resized again,
                    // which is what happens when a tab opens into a window
                    // nobody then drags.
                    canvas.apply_fit();
                    canvas.draw_working_area(cr, widget_w, widget_h);
                }
            });
    }

    fn setup_scroll_zoom(self: &Rc<Self>) {
        let wheel_notify = Rc::downgrade(self);
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
            drop(t);
            if let Some(canvas) = wheel_notify.upgrade() {
                canvas.view_changed();
            } else {
                drawing_area.queue_draw();
            }
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
        let pick = Rc::downgrade(self);
        let pan_pick = Rc::downgrade(self);
        let pan_notify = Rc::downgrade(self);
        let end_pick = Rc::downgrade(self);

        let so = start_offset.clone();
        let t = transform.clone();
        drag.connect_drag_begin(move |gesture, x, y| {
            let shifted = gesture
                .current_event_state()
                .contains(gtk::gdk::ModifierType::SHIFT_MASK);
            // Whoever owns the press gets it; the canvas pans only when
            // nothing else has a claim.
            let owner = pick
                .upgrade()
                .map(|c| c.press_owner(x, y, shifted))
                .unwrap_or(PressOwner::Canvas);
            if owner != PressOwner::Canvas {
                gesture.set_state(gtk::EventSequenceState::Denied);
                return;
            }
            if let Some(canvas) = pick.upgrade() {
                // The words first: clicking a label edits it.
                if let Some(id) = canvas.label_at(x, y) {
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                    canvas.set_selected_annotation(Some(id.clone()));
                    let handler = canvas.on_label_clicked.borrow_mut().take();
                    if let Some(f) = handler {
                        f(&id);
                        *canvas.on_label_clicked.borrow_mut() = Some(f);
                    }
                    return;
                }
                // A grip on the SELECTED mark resizes it; inside its shape
                // moves it. Either way the image does not pan: you are
                // adjusting a mark, not travelling.
                if canvas.handle_at(x, y) {
                    *canvas.grab.borrow_mut() = Some(Grab::Handle);
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                    return;
                }
                let hit = canvas.annotation_at(x, y);
                if hit.is_some() {
                    if hit != canvas.selected_annotation() {
                        // Selecting IS entering edit: the grips appear and the
                        // label opens for typing, on one click rather than two.
                        // Held until the release, because a press that becomes
                        // a drag is a move and should not raise a popover
                        // under the pointer.
                        *canvas.tapped.borrow_mut() = hit.clone();
                        canvas.set_selected_annotation(hit);
                        gesture.set_state(gtk::EventSequenceState::Claimed);
                        let notify = canvas.on_selection_changed.borrow();
                        if let Some(f) = notify.as_ref() {
                            f();
                        }
                        return;
                    } else {
                        // Already selected: this press is a move.
                        *canvas.grab.borrow_mut() = Some(Grab::Body);
                        gesture.set_state(gtk::EventSequenceState::Claimed);
                        return;
                    }
                }
            }
            let t = t.borrow();
            *so.borrow_mut() = (t.offset_x, t.offset_y);
        });

        let so = start_offset;
        drag.connect_drag_update(move |gesture, dx, dy| {
            // A grab in progress reshapes the mark instead of panning.
            if let Some(canvas) = pan_pick.upgrade() {
                let grab = *canvas.grab.borrow();
                if let Some(grab) = grab {
                    if let Some((sx, sy)) = gesture.start_point() {
                        match grab {
                            Grab::Handle => canvas.resize_selected_to(sx + dx, sy + dy),
                            Grab::Body => canvas.move_selected_to(sx + dx, sy + dy),
                        }
                        canvas.view_changed();
                    }
                    return;
                }
            }
            // Belt and braces: drag_begin already denies a press it does not
            // own, but a mode armed mid-drag would otherwise pan underneath it.
            let shifted = gesture
                .current_event_state()
                .contains(gtk::gdk::ModifierType::SHIFT_MASK);
            if let (Some(canvas), Some((sx, sy))) = (pan_pick.upgrade(), gesture.start_point()) {
                if canvas.press_owner(sx, sy, shifted) != PressOwner::Canvas {
                    return;
                }
            }
            {
                let start = so.borrow();
                let mut t = transform.borrow_mut();
                t.offset_x = start.0 + dx;
                t.offset_y = start.1 + dy;
            }
            if let Some(canvas) = pan_notify.upgrade() {
                canvas.view_changed();
            } else {
                drawing_area.queue_draw();
            }
        });

        {
            let notify = self.on_selection_changed.clone();
            drag.connect_drag_end(move |_, dx, dy| {
                let Some(canvas) = end_pick.upgrade() else {
                    return;
                };
                // A tap that selected a mark, and did not become a drag, opens
                // its label. A few pixels of travel is a hand, not an intent.
                let tapped = canvas.tapped.borrow_mut().take();
                if let Some(id) = tapped {
                    if dx.abs() < 4.0 && dy.abs() < 4.0 {
                        let handler = canvas.on_label_clicked.borrow_mut().take();
                        if let Some(f) = handler {
                            f(&id);
                            *canvas.on_label_clicked.borrow_mut() = Some(f);
                        }
                    }
                }
                if canvas.grab.borrow_mut().take().is_some() {
                    // Same channel the list already listens on: the mark
                    // changed, so whoever is showing it should save and
                    // redraw.
                    if let Some(f) = notify.borrow().as_ref() {
                        f();
                    }
                }
            });
        }

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
                    let (ra, dec) = wcs.display_to_sky(img_x, img_y);
                    shared.borrow_mut().hover = Some((ra, dec));
                }

                let mut text = format!("Pixel: ({:.0}, {:.0})", img_x, img_y);
                if let Some(ref wcs) = wcs {
                    let (ra, dec) = wcs.display_to_sky(img_x, img_y);
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
    /// Tell the canvas where to ask what shape is being drawn.
    pub fn set_preview_kind_source(
        &self,
        f: impl Fn() -> crate::models::annotation::AnnotationKind + 'static,
    ) {
        *self.preview_kind.borrow_mut() = Some(Box::new(f));
    }

    pub fn set_on_left_click(&self, f: impl Fn(f64, f64, f64) + 'static) {
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

    /// Which mode owns a press at `(x, y)`.
    ///
    /// Three gestures want the left button — pan, draw, and select-area — and
    /// the first time two of them wanted it, the pan drag claimed the sequence
    /// and marks could not be placed at all. So the question is answered once,
    /// here, and each gesture asks rather than deciding for itself.
    fn press_owner(&self, x: f64, y: f64, shifted: bool) -> PressOwner {
        // Shift always means "move the image, not the contents".
        if shifted {
            return PressOwner::Canvas;
        }
        // Select-area owns EVERY press while it is armed, marks included.
        // Draw mode stands aside on a mark so you can still pick one up; a
        // selection cannot, because the region you want almost always starts
        // on top of something interesting, and marks are what you put on the
        // interesting things.
        if self.selecting.get() {
            return PressOwner::Selecting;
        }
        if self.on_left_click.borrow().is_some()
            && self.annotation_at(x, y).is_none()
            && self.label_at(x, y).is_none()
        {
            return PressOwner::Drawing;
        }
        PressOwner::Canvas
    }

    /// Who owns a press, by name, for `fits_gesture_probe`.
    ///
    /// The enum stays private — it is an implementation detail of three
    /// gestures — but the DECISION is the thing that broke once and is worth a
    /// test, and it needs a real canvas to ask about marks.
    pub fn press_owner_name(&self, x: f64, y: f64, shifted: bool) -> &'static str {
        match self.press_owner(x, y, shifted) {
            PressOwner::Canvas => "canvas",
            PressOwner::Drawing => "drawing",
            PressOwner::Selecting => "selecting",
        }
    }

    /// Arm or disarm select-area mode.
    pub fn set_selecting(&self, on: bool) {
        self.selecting.set(on);
        self.pending_region.borrow_mut().take();
        self.drawing_area
            .set_cursor_from_name(Some(if on { "crosshair" } else { "default" }));
        self.drawing_area.queue_draw();
    }

    pub fn is_selecting(&self) -> bool {
        self.selecting.get()
    }

    /// Called with the region when a select drag finishes.
    pub fn set_on_region_selected(&self, f: impl Fn(ViewRegion) + 'static) {
        *self.on_region_selected.borrow_mut() = Some(Rc::new(f));
    }

    /// The drag that picks a region to export.
    ///
    /// Capture phase and its own gesture, like the drawing one: the pan drag
    /// underneath would otherwise claim the sequence first.
    fn setup_select_region(self: &Rc<Self>) {
        let drag = gtk::GestureDrag::new();
        drag.set_button(1);
        drag.set_propagation_phase(gtk::PropagationPhase::Capture);
        let anchor = Rc::new(RefCell::new(None::<(f64, f64)>));

        {
            let canvas = Rc::downgrade(self);
            let anchor = anchor.clone();
            drag.connect_drag_begin(move |gesture, x, y| {
                let Some(canvas) = canvas.upgrade() else {
                    return;
                };
                let shifted = gesture
                    .current_event_state()
                    .contains(gtk::gdk::ModifierType::SHIFT_MASK);
                if canvas.press_owner(x, y, shifted) != PressOwner::Selecting {
                    return;
                }
                gesture.set_state(gtk::EventSequenceState::Claimed);
                *anchor.borrow_mut() = Some((x, y));
            });
        }
        {
            let canvas = Rc::downgrade(self);
            let anchor = anchor.clone();
            drag.connect_drag_update(move |_, dx, dy| {
                let (Some(canvas), Some(start)) = (canvas.upgrade(), *anchor.borrow()) else {
                    return;
                };
                *canvas.pending_region.borrow_mut() =
                    Some(ViewRegion::between(start, (start.0 + dx, start.1 + dy)));
                canvas.drawing_area.queue_draw();
            });
        }
        {
            let canvas = Rc::downgrade(self);
            drag.connect_drag_end(move |_, dx, dy| {
                let (Some(canvas), Some(start)) = (canvas.upgrade(), anchor.borrow_mut().take())
                else {
                    return;
                };
                let region = ViewRegion::between(start, (start.0 + dx, start.1 + dy));
                canvas.pending_region.borrow_mut().take();
                canvas.drawing_area.queue_draw();
                // A tap is not a region. Without a floor, a click that wobbled
                // would open the dialog on a two-pixel box.
                if !region.is_usable() || region.width < 8.0 || region.height < 8.0 {
                    return;
                }
                let cb = canvas.on_region_selected.borrow().clone();
                if let Some(cb) = cb {
                    cb(region);
                }
            });
        }
        self.drawing_area.add_controller(drag);
    }

    fn setup_left_click(self: &Rc<Self>) {
        let drag = gtk::GestureDrag::new();
        drag.set_button(1);
        // Ahead of the pan gesture, which also wants button 1.
        drag.set_propagation_phase(gtk::PropagationPhase::Capture);

        let started = Rc::new(RefCell::new(None::<(f64, f64)>));

        {
            let canvas = Rc::downgrade(self);
            let started = started.clone();
            drag.connect_drag_begin(move |gesture, x, y| {
                let Some(canvas) = canvas.upgrade() else {
                    return;
                };
                // Shift means "move the image, not the marks"; a press on an
                // existing mark means "that one", not "another on top of it";
                // and select-area outranks both. All three live in
                // `press_owner`, so the modes cannot disagree about who has
                // the button — which is how marks once could not be placed at
                // all.
                let shifted = gesture
                    .current_event_state()
                    .contains(gtk::gdk::ModifierType::SHIFT_MASK);
                if canvas.press_owner(x, y, shifted) != PressOwner::Drawing {
                    return;
                }
                let (ix, iy) = canvas.screen_to_image_point(x, y);
                if !on_image(ix, iy, canvas.img_width, canvas.img_height) {
                    return;
                }
                gesture.set_state(gtk::EventSequenceState::Claimed);
                *started.borrow_mut() = Some((x, y));
                *canvas.pending_shape.borrow_mut() = Some((ix, iy, 0.0));
                canvas.drawing_area.queue_draw();
            });
        }
        {
            let canvas = Rc::downgrade(self);
            let started = started.clone();
            drag.connect_drag_update(move |_, dx, dy| {
                let Some(canvas) = canvas.upgrade() else {
                    return;
                };
                let Some(start) = *started.borrow() else {
                    return;
                };
                // The radius is the drag distance, in IMAGE pixels, so the
                // preview and the finished mark are the same size.
                let (ix, iy) = canvas.screen_to_image_point(start.0, start.1);
                let (ex, ey) = canvas.screen_to_image_point(start.0 + dx, start.1 + dy);
                let half = ((ex - ix).powi(2) + (ey - iy).powi(2)).sqrt();
                *canvas.pending_shape.borrow_mut() = Some((ix, iy, half));
                canvas.drawing_area.queue_draw();
            });
        }
        {
            let canvas = Rc::downgrade(self);
            drag.connect_drag_end(move |_, _dx, _dy| {
                let Some(canvas) = canvas.upgrade() else {
                    return;
                };
                let pending = canvas.pending_shape.borrow_mut().take();
                let Some((ix, iy, half)) = pending else {
                    return;
                };
                *started.borrow_mut() = None;
                canvas.drawing_area.queue_draw();
                // A tap with no drag still makes a mark, at a default size the
                // viewer chooses — insisting on a drag would mean a click that
                // silently does nothing, which is where this started.
                let handler = canvas.on_left_click.borrow_mut().take();
                if let Some(f) = handler {
                    f(ix, iy, half);
                    *canvas.on_left_click.borrow_mut() = Some(f);
                }
            });
        }
        self.drawing_area.add_controller(drag);
    }

    /// Screen point to image pixel, for a probe that checks the view maths.
    pub fn screen_to_image_point_public(&self, x: f64, y: f64) -> (f64, f64) {
        self.screen_to_image_point(x, y)
    }

    /// Screen point to image pixel, through the current view.
    fn screen_to_image_point(&self, x: f64, y: f64) -> (f64, f64) {
        let t = self.transform.borrow();
        let rot = *self.rotation.borrow();
        screen_to_image(
            x,
            y,
            t.scale,
            t.offset_x,
            t.offset_y,
            rot,
            self.img_width,
            self.img_height,
        )
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
                    let (ra, dec) = wcs.display_to_sky(img_x, img_y);
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

#[cfg(test)]
mod fit_tests {
    use super::fit_scale;

    /// A frame far wider than the viewport is limited by its width.
    ///
    /// The case this exists for: an 11471x4593 NIRCam mosaic opened at 100%
    /// and showed about 5% of its width, so the first thing anyone saw was a
    /// patch of sky with no way to tell what they were looking at.
    #[test]
    fn a_wide_frame_is_limited_by_the_tighter_axis() {
        let s = fit_scale((11471.0, 4593.0), (900.0, 700.0));
        assert!((s - 900.0 / 11471.0).abs() < 1e-12, "scale {s}");
        // And the whole frame really does fit, both ways.
        assert!(11471.0 * s <= 900.0 + 1e-9);
        assert!(4593.0 * s <= 700.0 + 1e-9);
    }

    /// A tall frame is limited by its height, not always by width.
    #[test]
    fn a_tall_frame_is_limited_by_its_height() {
        let s = fit_scale((400.0, 4000.0), (900.0, 700.0));
        assert!((s - 700.0 / 4000.0).abs() < 1e-12, "scale {s}");
    }

    /// A small image opens at 100%, not blown up to fill the window.
    ///
    /// A 64x64 thumbnail stretched across a 1600px viewport is a wall of fat
    /// pixels that tells you nothing an honest 64 pixels would not.
    #[test]
    fn a_small_image_is_never_enlarged() {
        assert_eq!(fit_scale((64.0, 64.0), (1600.0, 1200.0)), 1.0);
        assert_eq!(fit_scale((899.0, 699.0), (900.0, 700.0)), 1.0);
    }

    /// An image exactly the viewport's size sits at 100%.
    #[test]
    fn an_exact_fit_is_one_to_one() {
        assert_eq!(fit_scale((900.0, 700.0), (900.0, 700.0)), 1.0);
    }

    /// No viewport yet, or no image: 100%, not a division by zero.
    ///
    /// A `DrawingArea` reports 0x0 until it is allocated, and this is called
    /// from the draw function, so the degenerate case is the FIRST one that
    /// happens rather than a hypothetical.
    #[test]
    fn a_missing_viewport_or_image_does_not_divide_by_zero() {
        for (image, viewport) in [
            ((1000.0, 1000.0), (0.0, 0.0)),
            ((1000.0, 1000.0), (900.0, 0.0)),
            ((0.0, 0.0), (900.0, 700.0)),
            ((-5.0, 10.0), (900.0, 700.0)),
        ] {
            let s = fit_scale(image, viewport);
            assert_eq!(s, 1.0, "image {image:?} viewport {viewport:?} gave {s}");
        }
    }
}
