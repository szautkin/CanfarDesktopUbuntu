//! 2D slice view for the Cube Viewer.
//!
//! Port of `Views/CubeViewer/CubeViewerPage.Slice.cs` +
//! `CubeViewerPage.SliceInteraction.cs`. Renders one spectral channel of the
//! (already-normalized) [`VolumeData`] at native resolution via
//! [`render_plane_bgra`](crate::helpers::cube_slice::render_plane_bgra), with:
//!
//!  * a channel scrubber ([`gtk::Scale`] 0..nz-1) backed by a per-channel
//!    mean-intensity waveform, and a Play/Pause button that advances channels on
//!    a `glib` timeout;
//!  * wheel-zoom toward the cursor + drag-pan, and a hover coordinate bar
//!    (pixel + physical value + sky + [`CubeWcs::channel_label`]);
//!  * a click-to-probe spectrum panel ([`extract_spectrum`](crate::helpers::cube_slice::extract_spectrum))
//!    drawn against the physical spectral axis from [`CubeWcs::channel_to_physical`].
//!
//! [`set_window`](CubeSliceView::set_window) / [`set_stretch`](CubeSliceView::set_stretch) /
//! [`set_colormap`](CubeSliceView::set_colormap) re-render, so the slice shares the
//! window/stretch/colormap with the 3D volume.

use crate::helpers::annotation_render::AnnotationSurface;
use crate::helpers::cube_colormaps;
use crate::helpers::cube_native_slice::NativeSliceSource;
use crate::helpers::cube_slice::{extract_spectrum, render_plane_bgra, StretchMode};
use crate::helpers::cube_wcs::CubeWcs;
use crate::models::fits_image::WcsInfo;
use crate::models::volume_data::VolumeData;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

const PLAYBACK_FPS: u64 = 12;
const MAX_ZOOM: f64 = 20.0;
/// How far out the slice can be pulled, as a fraction of fit-to-widget.
///
/// The floor used to be 1.0 — fit — so the wheel simply stopped and the plane
/// could not be pulled back at all. That is wrong on its own, and it is also
/// what made switching from the volume jarring: the volume frames the box so
/// it never clips while orbiting, which is further out than fit, and the
/// slice had no way to sit at the same distance.
const MIN_ZOOM: f64 = 0.2;
const PAN_THRESHOLD: f64 = 6.0;
/// Cap the on-screen native slice bitmap's longest axis (mirrors `SliceDisplayCap`).
const SLICE_DISPLAY_CAP: usize = 2048;

/// Per-cube slice display / interaction state.
/// What a press on the slice is going to do.
///
/// Settled at drag-begin from what is under the pointer, and not revisited:
/// deciding per motion event let a slow drag on a grip cross the pan threshold
/// and start dragging the image instead, halfway through a resize.
#[derive(Clone, PartialEq)]
enum DragIntent {
    Pan,
    Place,
    /// Moving a mark: its id, and where the pointer was relative to its centre,
    /// so the mark does not jump to sit under the cursor on the first motion.
    Move {
        id: String,
        grab_dx: f64,
        grab_dy: f64,
    },
    /// Resizing a mark by a grip: its id.
    Resize {
        id: String,
    },
}

struct SliceState {
    z: usize,
    window: (f32, f32),
    stretch: StretchMode,
    cmap: String,
    zoom: f64,
    pan_x: f64,
    pan_y: f64,
    hover: Option<(usize, usize)>,
    last_cursor: (f64, f64),
    /// The readout for the hovered voxel, painted in the same frame as the
    /// image. Held here rather than pushed into a widget: an overlay label
    /// repositioned on every motion event re-runs layout dozens of times a
    /// second and lands a frame behind its own text.
    hover_lines: Vec<String>,
    probe: Option<(usize, usize)>,
    spectrum: Vec<f32>,
    playing: bool,
}

pub struct CubeSliceView {
    pub widget: gtk::Box,
    slice_area: gtk::DrawingArea,
    waveform_area: gtk::DrawingArea,
    spectrum_area: gtk::DrawingArea,
    spectrum_revealer: gtk::Revealer,
    spectrum_title: gtk::Label,
    channel_scale: gtk::Scale,
    /// Play + waveform + scrubber + label, hosted by the page rather than by
    /// this widget so it stays on screen in volume mode too.
    channel_bar: gtk::Box,
    /// Notified on every channel change, so the volume's slice-plane marker
    /// follows the scrubber. The crate's own slot type, which is `Rc` rather
    /// than `Box` so the handler can be cloned out before it runs — a handler
    /// that reaches back into this view would otherwise panic on the borrow.
    on_channel_changed: crate::ui::CallbackSlot<dyn Fn(usize)>,
    channel_label: gtk::Label,
    coord_label: gtk::Label,
    /// Floating readout chip (lon/lat/spectral/value) that tracks the pointer.
    play_btn: gtk::Button,
    vol: Rc<VolumeData>,
    wcs: Rc<CubeWcs>,
    surface: RefCell<Option<gtk4::cairo::ImageSurface>>,
    state: RefCell<SliceState>,
    /// Optional native-resolution plane reader; when present the on-screen slice
    /// is drawn at native FITS resolution and hover sky coords use native pixels.
    native: RefCell<Option<Rc<NativeSliceSource>>>,
    /// True while the native plane is actually driving the display.
    use_native: Cell<bool>,
    /// Displayed-slice pixel dims: native dims when `use_native`, else volume dims.
    disp_nx: Cell<usize>,
    disp_ny: Cell<usize>,
    /// Suppress the scale's `value-changed` while we drive it programmatically.
    suppress: Cell<bool>,
    /// Generation token so a restarted playback timer supersedes the old one.
    play_gen: Cell<u64>,
    /// True while the viewer is in draw mode: a click places a mark instead of
    /// probing, and a drag sizes it instead of panning.
    placing: Cell<bool>,
    /// The zoom a fresh view — and a double-click reset — lands on.
    ///
    /// Not always 1.0 (fit): the viewer measures what the VOLUME shows a voxel
    /// at and matches it, so switching modes does not change the size of
    /// anything. Fit-to-widget is the obvious default for a 2-D image and the
    /// wrong one here, because this view has a sibling showing the same data.
    default_zoom: Cell<f64>,
    /// What the preview should look like, ASKED at draw time rather than
    /// remembered — the picker can change while drawing is armed, and a
    /// preview that showed a ring and released a box teaches people not to
    /// trust it.
    preview_kind: RefCell<Option<Box<dyn Fn() -> crate::models::annotation::AnnotationKind>>>,
    /// The shape a placing drag is about to create: `(voxel x, voxel y, half
    /// in voxels)`. Drawn while the button is down so the size is chosen by
    /// eye rather than guessed — without it you draw blind and find out what
    /// you got on release.
    pending_shape: RefCell<Option<(f64, f64, f64)>>,
    /// The mark being edited: grips out, and a drag moves or resizes it.
    editing_annotation: RefCell<Option<String>>,
    /// What the drag in progress is doing. Decided once at drag-begin, so a
    /// drag cannot change its mind halfway and start panning out from under a
    /// mark being resized.
    drag_intent: RefCell<DragIntent>,
    /// Told when a drag has finished changing the marks, so the viewer — which
    /// owns them — can save. This view holds a mirror for live feedback; it is
    /// not the record.
    on_marks_changed: crate::ui::CallbackSlot<dyn Fn(Vec<crate::models::annotation::Annotation>)>,
    /// Told when a click picks a mark out, or clears the choice.
    on_mark_selected: crate::ui::CallbackSlot<dyn Fn(Option<String>)>,
    /// Called with `(voxel x, voxel y, radius in voxels)` when a placing click
    /// lands. The viewer decides what kind of mark that becomes — this view
    /// knows where, not what.
    on_place: crate::ui::CallbackSlot<dyn Fn(f64, f64, f64)>,
    /// The cube's marks, mirrored here so the slice draws the same set the
    /// volume does. The viewer owns them; this is a copy kept in step by
    /// `CubeViewer::set_annotations`, which is the only writer.
    annotations: RefCell<Vec<crate::models::annotation::Annotation>>,
    selected_annotation: RefCell<Option<String>>,
}

impl CubeSliceView {
    pub fn new(vol: Rc<VolumeData>, wcs: Rc<CubeWcs>) -> Rc<Self> {
        let nz = vol.nz;
        let mid = nz / 2;

        // ── Layout ──────────────────────────────────────────────────────────
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        widget.set_hexpand(true);
        widget.set_vexpand(true);

        // The channel plane, drawn with zoom/pan + a hover crosshair. It must be
        // focusable so the keyboard controller (play/pause + channel stepping)
        // receives key events once the slice has been clicked.
        let slice_area = gtk::DrawingArea::new();
        slice_area.set_hexpand(true);
        slice_area.set_vexpand(true);
        slice_area.set_focusable(true);
        slice_area.set_content_width(vol.nx.clamp(1, 800) as i32);
        slice_area.set_content_height(vol.ny.clamp(1, 600) as i32);

        // The cursor readout is painted by the draw function (see
        // `ui::coord_chip`), not layered as a widget — so the slice needs no
        // overlay at all.
        widget.append(&slice_area);

        // Persistent hover readout bar.
        let coord_label = gtk::Label::new(Some(crate::tr_en!("Hover the slice for coordinates")));
        coord_label.add_css_class("caption");
        coord_label.add_css_class("dim-label");
        coord_label.set_halign(gtk::Align::Start);
        coord_label.set_margin_start(8);
        coord_label.set_margin_top(2);
        coord_label.set_margin_bottom(2);
        widget.append(&coord_label);

        // Spectrum probe panel (collapsible).
        let spectrum_revealer = gtk::Revealer::new();
        spectrum_revealer.set_transition_type(gtk::RevealerTransitionType::SlideUp);
        let spectrum_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
        spectrum_box.set_margin_start(8);
        spectrum_box.set_margin_end(8);
        spectrum_box.set_margin_bottom(4);
        let spectrum_header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let spectrum_title = gtk::Label::new(Some(crate::tr_en!("Spectrum")));
        spectrum_title.add_css_class("caption");
        spectrum_title.set_halign(gtk::Align::Start);
        spectrum_title.set_hexpand(true);
        spectrum_header.append(&spectrum_title);
        let spectrum_close = gtk::Button::from_icon_name("window-close-symbolic");
        spectrum_close.add_css_class("flat");
        spectrum_close.add_css_class("circular");
        spectrum_header.append(&spectrum_close);
        spectrum_box.append(&spectrum_header);
        let spectrum_area = gtk::DrawingArea::new();
        spectrum_area.set_content_height(96);
        spectrum_area.set_hexpand(true);
        spectrum_box.append(&spectrum_area);
        spectrum_revealer.set_child(Some(&spectrum_box));
        widget.append(&spectrum_revealer);

        // Bottom bar: play + waveform/scrubber + channel label.
        let bottom_bar = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        bottom_bar.set_margin_start(8);
        bottom_bar.set_margin_end(8);
        bottom_bar.set_margin_top(4);
        bottom_bar.set_margin_bottom(6);
        bottom_bar.set_valign(gtk::Align::Center);

        let play_btn = gtk::Button::from_icon_name("media-playback-start-symbolic");
        play_btn.set_tooltip_text(Some(crate::tr_en!("Play / Pause channels")));
        play_btn.add_css_class("circular");
        bottom_bar.append(&play_btn);

        let scrub_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        scrub_box.set_hexpand(true);
        scrub_box.set_valign(gtk::Align::Center);
        let waveform_area = gtk::DrawingArea::new();
        waveform_area.set_content_height(20);
        waveform_area.set_hexpand(true);
        scrub_box.append(&waveform_area);
        let channel_scale = gtk::Scale::with_range(
            gtk::Orientation::Horizontal,
            0.0,
            (nz.max(1) - 1) as f64,
            1.0,
        );
        channel_scale.set_hexpand(true);
        channel_scale.set_draw_value(false);
        channel_scale.set_value(mid as f64);
        scrub_box.append(&channel_scale);
        bottom_bar.append(&scrub_box);

        let channel_label = gtk::Label::new(None);
        channel_label.add_css_class("caption");
        channel_label.set_halign(gtk::Align::End);
        channel_label.set_width_chars(20);
        bottom_bar.append(&channel_label);

        // A single-channel cube has nothing to scrub / play.
        bottom_bar.set_visible(nz > 1);
        // NOT appended here: the channel bar belongs to BOTH modes — in slice
        // mode it picks the plane being drawn, in volume mode it drives the
        // slice-plane marker through the cube. The page places it under the
        // mode stack; see `CubeSliceView::channel_bar`.

        // ── Per-channel mean waveform heights (computed once) ───────────────
        let heights = channel_profile(&vol);

        let this = Rc::new(CubeSliceView {
            waveform_area,
            widget,
            slice_area,
            spectrum_area,
            spectrum_revealer,
            spectrum_title,
            channel_scale,
            channel_bar: bottom_bar,
            on_channel_changed: RefCell::new(None),
            channel_label,
            coord_label,
            play_btn,
            disp_nx: Cell::new(vol.nx),
            disp_ny: Cell::new(vol.ny),
            vol,
            wcs,
            surface: RefCell::new(None),
            native: RefCell::new(None),
            use_native: Cell::new(false),
            state: RefCell::new(SliceState {
                z: mid,
                window: (0.0, 1.0),
                stretch: StretchMode::Linear,
                cmap: cube_colormaps::DEFAULT.to_string(),
                zoom: 1.0,
                pan_x: 0.0,
                pan_y: 0.0,
                hover: None,
                last_cursor: (0.0, 0.0),
                hover_lines: Vec::new(),
                probe: None,
                spectrum: Vec::new(),
                playing: false,
            }),
            suppress: Cell::new(false),
            play_gen: Cell::new(0),
            placing: Cell::new(false),
            default_zoom: Cell::new(1.0),
            preview_kind: RefCell::new(None),
            pending_shape: RefCell::new(None),
            editing_annotation: RefCell::new(None),
            drag_intent: RefCell::new(DragIntent::Pan),
            on_marks_changed: RefCell::new(None),
            on_mark_selected: RefCell::new(None),
            on_place: RefCell::new(None),
            annotations: RefCell::new(Vec::new()),
            selected_annotation: RefCell::new(None),
        });

        this.setup_slice_draw();
        this.setup_waveform_draw(heights);
        this.setup_spectrum_draw();
        this.setup_gestures();
        this.setup_keyboard();
        this.wire_controls(&spectrum_close);

        this.render();
        this.update_channel_label();
        this
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.widget
    }

    /// Attach a persistent native-resolution plane reader. When its dimensions
    /// are sane and within the display cap, the on-screen slice switches to the
    /// crisp native plane (and hover sky coords use native pixels); otherwise the
    /// down-sampled volume plane is kept. Re-renders the current channel.
    pub fn set_native_source(&self, src: NativeSliceSource) {
        let (snx, sny, snz) = src.dims();
        if snx == 0 || sny == 0 || snz == 0 || snx.max(sny) > SLICE_DISPLAY_CAP {
            return; // too large / degenerate → keep the down-sampled plane
        }
        self.disp_nx.set(snx);
        self.disp_ny.set(sny);
        self.use_native.set(true);
        *self.native.borrow_mut() = Some(Rc::new(src));
        // The native aspect matches the volume's, so the content-size hint stays
        // valid; refresh it and re-render at native resolution.
        self.slice_area.set_content_width(snx.clamp(1, 800) as i32);
        self.slice_area.set_content_height(sny.clamp(1, 600) as i32);
        self.render();
    }

    /// The display cut (physical `norm_lo`/`norm_hi`) the volume was normalized
    /// against — native planes must use the same cut so their `[0,1]` scale
    /// matches the volume's before window/stretch are applied.
    fn norm_cut(&self) -> (f64, f64) {
        match self.vol.meta.as_ref() {
            Some(m) => (m.norm_lo, m.norm_hi),
            None => (0.0, 1.0),
        }
    }

    /// A native read failed mid-session → release the source and fall back to the
    /// down-sampled volume plane for the rest of the tab's life.
    fn disable_native(&self) {
        self.use_native.set(false);
        *self.native.borrow_mut() = None;
        self.disp_nx.set(self.vol.nx);
        self.disp_ny.set(self.vol.ny);
    }

    // ── Shared display controls (also drive the 3D volume) ──────────────────

    pub fn set_window(&self, lo: f32, hi: f32) {
        self.state.borrow_mut().window = (lo, hi);
        self.render();
    }

    pub fn set_stretch(&self, m: StretchMode) {
        self.state.borrow_mut().stretch = m;
        self.render();
    }

    pub fn set_colormap(&self, name: &str) {
        self.state.borrow_mut().cmap = name.to_string();
        self.render();
    }

    /// Straight (non-premultiplied) RGBA8 of the current channel plane, plus its
    /// pixel dimensions — used by the parent page's "Export…" action.
    pub fn export_rgba(&self) -> (i32, i32, Vec<u8>) {
        let (z, window, stretch, cmap) = {
            let s = self.state.borrow();
            (s.z, s.window, s.stretch, s.cmap.clone())
        };
        let bgra = render_plane_bgra(&self.vol, z, window, stretch, &cmap);
        let (nx, ny) = (self.vol.nx, self.vol.ny);
        let mut rgba = vec![0u8; nx * ny * 4];
        for i in 0..nx * ny {
            rgba[i * 4] = bgra[i * 4 + 2]; // R
            rgba[i * 4 + 1] = bgra[i * 4 + 1]; // G
            rgba[i * 4 + 2] = bgra[i * 4]; // B
            rgba[i * 4 + 3] = 255; // A
        }
        (nx as i32, ny as i32, rgba)
    }

    // ── Rendering ────────────────────────────────────────────────────────────

    /// Re-render the current channel plane into a cached Cairo surface. Prefers
    /// the crisp native plane; falls back to the down-sampled volume plane.
    fn render(&self) {
        let (z, window, stretch, cmap) = {
            let s = self.state.borrow();
            (s.z, s.window, s.stretch, s.cmap.clone())
        };

        // Native-resolution plane when a source is attached and usable.
        if self.use_native.get() {
            let src = self.native.borrow().clone();
            if let Some(src) = src {
                let (nlo, nhi) = self.norm_cut();
                let (snx, sny, snz) = src.dims();
                let native_ch = map_native_channel(z, self.vol.nz, snz);
                if let Some(plane) = src.read_channel(native_ch, nlo, nhi) {
                    if snx > 0 && sny > 0 && plane.len() == snx * sny {
                        // Wrap the single native plane as a 1-channel volume so the
                        // shared BGRA renderer applies window + stretch + colormap.
                        let tmp = VolumeData {
                            nx: snx,
                            ny: sny,
                            nz: 1,
                            data: plane,
                            name: String::new(),
                            meta: None,
                        };
                        let bgra = render_plane_bgra(&tmp, 0, window, stretch, &cmap);
                        self.build_surface(&bgra, snx, sny);
                        self.slice_area.queue_draw();
                        self.spectrum_area.queue_draw();
                        return;
                    }
                }
                // I/O error or a size mismatch → drop to the down-sampled plane.
                self.disable_native();
            }
        }

        // Down-sampled volume plane (default path).
        let (nx, ny) = (self.vol.nx, self.vol.ny);
        if nx == 0 || ny == 0 {
            return;
        }
        // render_plane_bgra emits tightly-packed BGRA (Cairo ARgb32 channel order).
        let plane = render_plane_bgra(&self.vol, z, window, stretch, &cmap);
        self.build_surface(&plane, nx, ny);
        self.slice_area.queue_draw();
        self.spectrum_area.queue_draw();
    }

    /// Copy a tightly-packed `nx*ny` BGRA buffer into a stride-aligned Cairo
    /// `ARgb32` surface and cache it for the draw callback.
    fn build_surface(&self, plane: &[u8], nx: usize, ny: usize) {
        if nx == 0 || ny == 0 {
            return;
        }
        let stride = gtk4::cairo::Format::ARgb32
            .stride_for_width(nx as u32)
            .unwrap_or(nx as i32 * 4);
        let mut buf = vec![0u8; stride as usize * ny];
        let row = nx * 4;
        for y in 0..ny {
            let src = y * row;
            let dst = y * stride as usize;
            buf[dst..dst + row].copy_from_slice(&plane[src..src + row]);
        }
        let surface = gtk4::cairo::ImageSurface::create_for_data(
            buf,
            gtk4::cairo::Format::ARgb32,
            nx as i32,
            ny as i32,
            stride,
        )
        .ok();
        *self.surface.borrow_mut() = surface;
    }

    fn set_channel(&self, z: usize) {
        let clamped = z.min(self.vol.nz.saturating_sub(1));
        self.state.borrow_mut().z = clamped;
        self.render();
        self.update_channel_label();
        // Tell the page, so the volume's slice-plane marker moves with the
        // scrubber. There is ONE channel in this viewer; it used to be two —
        // this view's and the volume's — which is why the marker sat wherever
        // it was seeded no matter where you scrubbed.
        let handler = self.on_channel_changed.borrow().clone();
        if let Some(cb) = handler {
            cb(clamped);
        }
    }

    /// The play + waveform + scrubber + label bar, for the page to place.
    pub fn channel_bar(&self) -> &gtk::Box {
        &self.channel_bar
    }

    /// The channel the slice view is showing.
    pub fn channel(&self) -> usize {
        self.state.borrow().z
    }

    /// Move the scrubber, and with it the slice and the volume marker.
    ///
    /// Drives the WIDGET, so the same handler runs as when a person drags it —
    /// one path into the change rather than two that can disagree.
    pub fn set_channel_from(&self, z: usize) {
        let clamped = z.min(self.vol.nz.saturating_sub(1)) as f64;
        if (self.channel_scale.value() - clamped).abs() > f64::EPSILON {
            self.channel_scale.set_value(clamped);
        }
    }

    /// Called whenever the channel changes, however it changed.
    pub fn set_on_channel_changed(&self, cb: impl Fn(usize) + 'static) {
        *self.on_channel_changed.borrow_mut() = Some(std::rc::Rc::new(cb));
    }

    fn update_channel_label(&self) {
        let z = self.state.borrow().z;
        let nz = self.vol.nz.max(1);
        self.channel_label.set_text(&format!(
            "{} / {} · {}",
            z,
            nz - 1,
            self.wcs.channel_label(z)
        ));
    }

    // ── Coordinate mapping (viewport ⇄ voxel) ───────────────────────────────

    /// Base fit scale + letterbox offset for the current viewport (before zoom).
    fn fit_params(&self) -> Option<(f64, f64, f64, f64, f64)> {
        let aw = self.slice_area.width() as f64;
        let ah = self.slice_area.height() as f64;
        let (nx, ny) = (self.disp_nx.get() as f64, self.disp_ny.get() as f64);
        if aw <= 0.0 || ah <= 0.0 || nx <= 0.0 || ny <= 0.0 {
            return None;
        }
        let fit = (aw / nx).min(ah / ny);
        let ox = (aw - nx * fit) / 2.0;
        let oy = (ah - ny * fit) / 2.0;
        Some((fit, ox, oy, aw, ah))
    }

    /// Invert the zoom/pan + aspect-fit transform: viewport point → voxel (x, y).
    /// Mirror the cube's marks into this view. Called by `CubeViewer`, which
    /// owns them; nothing here writes the set.
    pub fn set_annotations(&self, annotations: Vec<crate::models::annotation::Annotation>) {
        *self.annotations.borrow_mut() = annotations;
        self.slice_area.queue_draw();
    }

    pub fn set_selected_annotation(&self, id: Option<String>) {
        *self.selected_annotation.borrow_mut() = id;
        self.slice_area.queue_draw();
    }

    pub fn set_editing_annotation(&self, id: Option<String>) {
        *self.editing_annotation.borrow_mut() = id;
        self.slice_area.queue_draw();
    }

    /// The mark currently open for editing, if any.
    pub fn editing_annotation(&self) -> Option<String> {
        self.editing_annotation.borrow().clone()
    }

    /// Where a mark sits on this view, in widget coordinates. `None` when it
    /// is not on the plane being shown.
    /// How many screen pixels one VOLUME voxel spans at fit-to-widget, for a
    /// panel of `w` x `h`. The denominator when matching another view's scale.
    pub fn voxel_pixels_at_fit(&self, w: i32, h: i32) -> f64 {
        let (dnx, dny) = (self.disp_nx.get() as f64, self.disp_ny.get() as f64);
        if dnx <= 0.0 || dny <= 0.0 || w <= 0 || h <= 0 {
            return 0.0;
        }
        let fit = (w as f64 / dnx).min(h as f64 / dny);
        // Displayed pixels per volume voxel, times screen pixels per displayed
        // pixel — the slice may be showing a native-resolution plane with more
        // pixels than the cube has voxels.
        (dnx / self.vol.nx.max(1) as f64) * fit
    }

    /// Zoom and pan together, for `get_cube_view`.
    pub fn probe_view(&self) -> (f64, f64, f64) {
        let s = self.state.borrow();
        (s.zoom, s.pan_x, s.pan_y)
    }

    /// Set the zoom and pan directly, as `set_cube_view` does.
    ///
    /// `reset` puts both back to the default — the same thing a double-click
    /// on the view does, so an agent and a person have one way back.
    pub fn set_view(&self, zoom: Option<f64>, pan: Option<(f64, f64)>, reset: bool) {
        {
            let mut s = self.state.borrow_mut();
            if reset {
                s.zoom = self.default_zoom.get();
                s.pan_x = 0.0;
                s.pan_y = 0.0;
            }
            if let Some(z) = zoom {
                s.zoom = z.clamp(MIN_ZOOM, MAX_ZOOM);
            }
            if let Some((px, py)) = pan {
                s.pan_x = px;
                s.pan_y = py;
            }
        }
        self.slice_area.queue_draw();
    }

    /// The current zoom. For `cube_slice_zoom_probe`.
    pub fn probe_zoom(&self) -> f64 {
        self.state.borrow().zoom
    }

    /// Zoom about the centre, as the wheel does — including its clamps, which
    /// is the point: the probe checks how far out the wheel can actually go.
    pub fn probe_scroll(&self, factor: f64) {
        let (aw, ah) = (
            self.slice_area.width().max(1) as f64,
            self.slice_area.height().max(1) as f64,
        );
        self.zoom_toward(aw / 2.0, ah / 2.0, factor);
    }

    /// Set the zoom a fresh view and a reset land on, and apply it if the
    /// user has not already zoomed away from the old default.
    pub fn set_default_zoom(&self, zoom: f64) {
        let zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        let previous = self.default_zoom.replace(zoom);
        let mut s = self.state.borrow_mut();
        if default_is_welcome(s.zoom, previous) {
            s.zoom = zoom;
            drop(s);
            self.slice_area.queue_draw();
        }
    }

    pub fn project_mark(&self, anchor: &crate::models::annotation::Anchor) -> Option<(f64, f64)> {
        self.annotation_surface()?.project(anchor)
    }

    pub fn set_preview_kind_source(
        &self,
        f: impl Fn() -> crate::models::annotation::AnnotationKind + 'static,
    ) {
        *self.preview_kind.borrow_mut() = Some(Box::new(f));
    }

    pub fn set_on_marks_changed(
        &self,
        cb: impl Fn(Vec<crate::models::annotation::Annotation>) + 'static,
    ) {
        *self.on_marks_changed.borrow_mut() = Some(std::rc::Rc::new(cb));
    }

    pub fn set_on_mark_selected(&self, cb: impl Fn(Option<String>) + 'static) {
        *self.on_mark_selected.borrow_mut() = Some(std::rc::Rc::new(cb));
    }

    /// Arm or disarm placing. While armed the pointer is a crosshair, so it is
    /// visible that a click will do something other than probe.
    pub fn set_placing(&self, on: bool) {
        self.placing.set(on);
        self.slice_area
            .set_cursor_from_name(Some(if on { "crosshair" } else { "default" }));
    }

    pub fn set_on_place(&self, cb: impl Fn(f64, f64, f64) + 'static) {
        *self.on_place.borrow_mut() = Some(std::rc::Rc::new(cb));
    }

    /// The current channel, for placing a mark on the plane being looked at.
    pub fn current_z(&self) -> usize {
        self.state.borrow().z
    }

    /// Screen point to a CONTINUOUS volume-voxel position, or `None` off-image.
    ///
    /// `map_to_pixel` floors to a voxel index, which is right for sampling a
    /// value and wrong for placing a mark: a click in the middle of a voxel
    /// would be stored at its corner, half a voxel from where the user aimed.
    /// Voxel coordinates are continuous positions here, which is also how
    /// `project_voxel` reads them in the volume view — the two views have to
    /// agree or a mark moves when you switch mode.
    pub fn screen_to_voxel(&self, px: f64, py: f64) -> Option<(f64, f64)> {
        self.annotation_surface()?.screen_to_voxel(px, py)
    }

    /// The projection marks use on this slice, for the current view.
    fn annotation_surface(&self) -> Option<SliceAnnotationSurface> {
        let (fit, ox, oy, aw, ah) = self.fit_params()?;
        let (zoom, pan_x, pan_y, z) = {
            let s = self.state.borrow();
            (s.zoom, s.pan_x, s.pan_y, s.z)
        };
        Some(SliceAnnotationSurface {
            fit,
            ox,
            oy,
            aw,
            ah,
            zoom,
            pan_x,
            pan_y,
            z,
            disp_nx: self.disp_nx.get() as f64,
            disp_ny: self.disp_ny.get() as f64,
            vol_nx: self.vol.nx as f64,
            vol_ny: self.vol.ny as f64,
        })
    }

    /// What a press at `(px, py)` should do.
    ///
    /// Order matters: a grip sits ON the edge of its own shape, so testing the
    /// shape first would mean a grip could never be grabbed.
    fn intent_at(&self, px: f64, py: f64) -> DragIntent {
        let Some(surface) = self.annotation_surface() else {
            return DragIntent::Pan;
        };
        let marks = self.annotations.borrow().clone();
        let editing = self.editing_annotation.borrow().clone();
        // Drawing armed does NOT short-circuit this. It used to, so a press on
        // an existing mark dropped a new one on top of it and a mark could
        // never be moved without disarming the pencil first.
        match crate::helpers::annotation_render::grab_at(
            &marks,
            &surface,
            editing.as_deref(),
            self.placing.get(),
            px,
            py,
        ) {
            crate::helpers::annotation_render::MarkGrab::Place => DragIntent::Place,
            crate::helpers::annotation_render::MarkGrab::None => DragIntent::Pan,
            crate::helpers::annotation_render::MarkGrab::Move {
                id,
                grab_dx,
                grab_dy,
            } => DragIntent::Move {
                id,
                grab_dx,
                grab_dy,
            },
            crate::helpers::annotation_render::MarkGrab::Resize { id } => DragIntent::Resize { id },
        }
    }

    /// Apply a move or resize to the mirror, for live feedback while dragging.
    fn drag_mark(&self, intent: &DragIntent, px: f64, py: f64) {
        let Some(surface) = self.annotation_surface() else {
            return;
        };
        let mut marks = self.annotations.borrow_mut();
        match intent {
            DragIntent::Move {
                id,
                grab_dx,
                grab_dy,
            } => {
                let Some((vx, vy)) = surface.screen_to_voxel(px - grab_dx, py - grab_dy) else {
                    return;
                };
                if let Some(m) = marks.iter_mut().find(|a| &a.id == id) {
                    // Only the plane position moves. A drag on a 2D slice says
                    // nothing about the channel, and silently changing z would
                    // move the mark off the plane you are looking at.
                    if let crate::models::annotation::Anchor::Data { z, .. } = m.anchor {
                        m.anchor = crate::models::annotation::Anchor::Data { x: vx, y: vy, z };
                    }
                }
            }
            DragIntent::Resize { id } => {
                let Some(m) = marks.iter_mut().find(|a| &a.id == id) else {
                    return;
                };
                let Some(half) =
                    crate::helpers::annotation_render::resize_half(m, &surface, px, py)
                else {
                    return;
                };
                m.extent = Some(crate::models::annotation::Extent::square(half));
            }
            _ => return,
        }
        drop(marks);
        self.slice_area.queue_draw();
    }

    /// Tell the viewer a mark was picked out (or the choice cleared).
    fn announce_selected(&self, id: Option<String>) {
        let cb = self.on_mark_selected.borrow().clone();
        if let Some(cb) = cb {
            cb(id);
        }
    }

    /// Turn a placing click-drag into a voxel position and a radius.
    ///
    /// The radius is measured in VOXELS, not screen pixels, so a mark drawn at
    /// one zoom is the same size on the data at any other — the same rule the
    /// FITS viewer follows, and the reason a mark does not swell when you zoom
    /// in on it.
    fn place_at(&self, px: f64, py: f64, dx: f64, dy: f64) {
        // Size it once more from the same function the preview used, then
        // place exactly what was on screen. Computing the radius a second way
        // here is how a preview and the mark it becomes drift apart.
        self.size_pending(px, py, dx, dy);
        let pending = self.pending_shape.borrow_mut().take();
        self.slice_area.queue_draw();
        let Some((vx, vy, half)) = pending else {
            return;
        };
        let cb = self.on_place.borrow().clone();
        if let Some(cb) = cb {
            cb(vx, vy, half);
        }
    }

    /// Update the shape the placing drag is drawing.
    fn size_pending(&self, px: f64, py: f64, dx: f64, dy: f64) {
        let Some(surface) = self.annotation_surface() else {
            return;
        };
        let Some((vx, vy)) = surface.screen_to_voxel(px, py) else {
            return;
        };
        // A drag of a few pixels is a click that wobbled, not a size; zero
        // tells the viewer to use its default.
        let half = if dx.hypot(dy) > PAN_THRESHOLD {
            let anchor = crate::models::annotation::Anchor::Data {
                x: vx,
                y: vy,
                z: self.state.borrow().z as f64,
            };
            crate::helpers::annotation_render::half_from_drag(&surface, &anchor, dx.hypot(dy))
        } else {
            0.0
        };
        *self.pending_shape.borrow_mut() = Some((vx, vy, half));
        self.slice_area.queue_draw();
    }

    fn map_to_pixel(&self, px: f64, py: f64) -> Option<(usize, usize)> {
        let (fit, ox, oy, aw, ah) = self.fit_params()?;
        let (zoom, pan_x, pan_y) = {
            let s = self.state.borrow();
            (s.zoom, s.pan_x, s.pan_y)
        };
        let (cx, cy) = (aw / 2.0, ah / 2.0);
        let fx = cx + (px - pan_x - cx) / zoom;
        let fy = cy + (py - pan_y - cy) / zoom;
        let x = ((fx - ox) / fit).floor();
        let y = ((fy - oy) / fit).floor();
        if x < 0.0 || y < 0.0 || x >= self.disp_nx.get() as f64 || y >= self.disp_ny.get() as f64 {
            return None;
        }
        Some((x as usize, y as usize))
    }

    fn update_readout(&self, px: f64, py: f64) {
        let Some((dx, dy)) = self.map_to_pixel(px, py) else {
            self.clear_readout();
            return;
        };
        // `hover` holds the displayed-slice pixel; the crosshair draws in disp space.
        self.state.borrow_mut().hover = Some((dx, dy));
        let z = self.state.borrow().z;
        let (dnx, dny) = (self.disp_nx.get(), self.disp_ny.get());

        // Voxel value: sampled from the (down-sampled) in-RAM volume — the same
        // source the spectrum probe uses — mapped back to physical via the cut.
        let vx = map_disp_to(dx, dnx, self.vol.nx);
        let vy = map_disp_to(dy, dny, self.vol.ny);
        let value = self.value_text(self.vol.sample(vx, vy, z));

        // Sky at the NATIVE pixel matching the displayed one (the WCS is
        // native-resolution): identity when the native slice drives the display,
        // else the displayed volume pixel scaled up to native (fixes the old
        // stride-factor offset that fed down-sampled coords to a native WCS).
        let sky = self.wcs.spatial.as_ref().map(|w| {
            let wnx = self.vol.meta.as_ref().map(|m| m.nx).unwrap_or(self.vol.nx);
            let wny = self.vol.meta.as_ref().map(|m| m.ny).unwrap_or(self.vol.ny);
            let nxp = map_disp_to(dx, dnx, wnx);
            let nyp = map_disp_to(dy, dny, wny);
            let (ra, dec) = w.pixel_to_sky(nxp as f64, nyp as f64);
            WcsInfo::format_coords(ra, dec)
        });
        let spec = self.wcs.channel_label(z);

        // Persistent coordinate bar: sky · value · spectral, on one line.
        let mut parts: Vec<String> = Vec::new();
        match &sky {
            Some((ra_s, dec_s)) => parts.push(format!("RA {}  Dec {}", ra_s, dec_s)),
            None => parts.push(format!("px ({}, {})", dx, dy)),
        }
        parts.push(value.clone());
        if !spec.is_empty() {
            parts.push(spec.clone());
        }
        self.coord_label.set_text(&parts.join("  ·  "));

        // Floating chip: lon / lat / spectral / value (empty lines collapse).
        let mut lines: Vec<String> = Vec::new();
        match &sky {
            Some((ra_s, dec_s)) => {
                lines.push(format!("RA  {}", ra_s));
                lines.push(format!("Dec {}", dec_s));
            }
            None => {
                lines.push(format!("X {}", dx));
                lines.push(format!("Y {}", dy));
            }
        }
        if !spec.is_empty() {
            lines.push(spec);
        }
        lines.push(value);
        {
            let mut st = self.state.borrow_mut();
            st.hover_lines = lines;
            st.last_cursor = (px, py);
        }

        self.slice_area.queue_draw();
    }

    /// Hide the chip + crosshair and reset the coordinate bar to its hint.
    fn clear_readout(&self) {
        {
            let mut st = self.state.borrow_mut();
            st.hover = None;
            st.hover_lines.clear();
        }
        self.coord_label
            .set_text(crate::tr_en!("Hover the slice for coordinates"));
        self.slice_area.queue_draw();
    }

    /// Format a normalized voxel value as a physical quantity (+ unit) when the
    /// cube carries metadata; blank voxels show as an em dash.
    fn value_text(&self, norm: f32) -> String {
        if !norm.is_finite() {
            return "—".to_string();
        }
        match self.vol.meta.as_ref() {
            Some(m) => {
                // Map through the DISPLAY CUT (p0.5…p99.5), not the full extremes —
                // otherwise the read-back is wrong whenever the cut ≠ full range.
                let phys = m.value_at_normalized(norm as f64);
                let unit = m.bunit.as_deref().unwrap_or("");
                if unit.is_empty() {
                    format!("{:.4}", phys)
                } else {
                    format!("{:.4} {}", phys, unit)
                }
            }
            None => format!("{:.3}", norm),
        }
    }

    /// Open the on-screen spectrum panel at a **native cube spaxel**.
    ///
    /// The click path goes through display coordinates; this is the programmatic
    /// (MCP) entry point, so it takes cube pixels directly. Returns false when the
    /// spaxel is outside the cube. Distinct from `probe_cube_spectrum`, which only
    /// returns data — this is what the USER sees.
    pub fn show_spectrum_at(&self, x: usize, y: usize) -> bool {
        let (native_x, native_y) = match self.vol.meta.as_ref() {
            Some(m) => (m.nx, m.ny),
            None => (self.vol.nx, self.vol.ny),
        };
        if x >= native_x || y >= native_y {
            return false;
        }
        let vx = crate::models::volume_data::native_to_resident(x, native_x, self.vol.nx);
        let vy = crate::models::volume_data::native_to_resident(y, native_y, self.vol.ny);
        let spectrum = extract_spectrum(&self.vol, vx, vy);
        {
            let mut st = self.state.borrow_mut();
            st.probe = Some((x, y));
            st.spectrum = spectrum;
        }
        self.spectrum_title
            .set_text(&crate::tr_fmt!("Spectrum at ({}, {})", x, y));
        self.spectrum_revealer.set_reveal_child(true);
        self.spectrum_area.queue_draw();
        true
    }

    /// Close the spectrum panel.
    pub fn hide_spectrum(&self) {
        self.spectrum_revealer.set_reveal_child(false);
    }

    fn probe_at(&self, px: f64, py: f64) {
        let Some((dx, dy)) = self.map_to_pixel(px, py) else {
            return;
        };
        // The spectrum is sampled from the (down-sampled) volume, so map the
        // displayed pixel down to its voxel; the title shows the displayed pixel.
        let vx = map_disp_to(dx, self.disp_nx.get(), self.vol.nx);
        let vy = map_disp_to(dy, self.disp_ny.get(), self.vol.ny);
        let spectrum = extract_spectrum(&self.vol, vx, vy);
        {
            let mut s = self.state.borrow_mut();
            s.probe = Some((dx, dy));
            s.spectrum = spectrum;
        }
        self.spectrum_title
            .set_text(&crate::tr_fmt!("Spectrum at ({}, {})", dx, dy));
        self.spectrum_revealer.set_reveal_child(true);
        self.spectrum_area.queue_draw();
    }

    // ── Playback ─────────────────────────────────────────────────────────────

    fn toggle_play(self: &Rc<Self>) {
        if self.state.borrow().playing {
            self.stop_play();
        } else {
            self.start_play();
        }
    }

    fn start_play(self: &Rc<Self>) {
        if self.vol.nz < 2 {
            return;
        }
        self.state.borrow_mut().playing = true;
        self.play_btn.set_icon_name("media-playback-pause-symbolic");
        let gen = self.play_gen.get().wrapping_add(1);
        self.play_gen.set(gen);
        let this = self.clone();
        glib::timeout_add_local(Duration::from_millis(1000 / PLAYBACK_FPS), move || {
            if this.play_gen.get() != gen || !this.state.borrow().playing {
                return glib::ControlFlow::Break;
            }
            let nz = this.vol.nz.max(1);
            let next = (this.state.borrow().z + 1) % nz;
            this.suppress.set(true);
            this.channel_scale.set_value(next as f64);
            this.suppress.set(false);
            this.set_channel(next);
            glib::ControlFlow::Continue
        });
    }

    fn stop_play(&self) {
        self.state.borrow_mut().playing = false;
        self.play_btn.set_icon_name("media-playback-start-symbolic");
    }

    /// Step the current channel by `delta`, clamped to `[0, nz-1]` (mirrors
    /// `StepChannel`): drives the scrubber (suppressed) then re-renders + relabels.
    fn step_channel(&self, delta: i32) {
        let nz = self.vol.nz;
        if nz == 0 {
            return;
        }
        let cur = self.state.borrow().z as i32;
        let c = (cur + delta).clamp(0, nz as i32 - 1) as usize;
        self.suppress.set(true);
        self.channel_scale.set_value(c as f64);
        self.suppress.set(false);
        self.set_channel(c);
    }

    // ── Keyboard shortcuts (Space play/pause, ←/→ ±1, ⇧←/→ ±10) ─────────────

    fn setup_keyboard(self: &Rc<Self>) {
        let key = gtk::EventControllerKey::new();
        let this = self.clone();
        key.connect_key_pressed(move |_, keyval, _code, modifier| {
            let shift = modifier.contains(gtk::gdk::ModifierType::SHIFT_MASK);
            match keyval {
                gtk::gdk::Key::space => {
                    this.toggle_play();
                    glib::Propagation::Stop
                }
                gtk::gdk::Key::Left => {
                    this.step_channel(if shift { -10 } else { -1 });
                    glib::Propagation::Stop
                }
                gtk::gdk::Key::Right => {
                    this.step_channel(if shift { 10 } else { 1 });
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });
        self.slice_area.add_controller(key);
    }

    // ── Drawing wiring ───────────────────────────────────────────────────────

    fn setup_slice_draw(self: &Rc<Self>) {
        let this = self.clone();
        self.slice_area.set_draw_func(move |_area, cr, w, h| {
            // Dark backdrop matching the volume clear color.
            cr.set_source_rgb(0.02, 0.03, 0.06);
            let _ = cr.paint();

            let surf = this.surface.borrow();
            let Some(surface) = surf.as_ref() else {
                return;
            };
            let Some((fit, ox, oy, aw, ah)) = this.fit_params() else {
                return;
            };
            let (zoom, pan_x, pan_y, hover, hover_lines, cursor) = {
                let s = this.state.borrow();
                (
                    s.zoom,
                    s.pan_x,
                    s.pan_y,
                    s.hover,
                    s.hover_lines.clone(),
                    s.last_cursor,
                )
            };

            cr.save().ok();
            // view = pan · (center + zoom·(center⁻¹ · (fit))): see map_to_pixel.
            cr.translate(pan_x, pan_y);
            cr.translate(aw / 2.0, ah / 2.0);
            cr.scale(zoom, zoom);
            cr.translate(-aw / 2.0, -ah / 2.0);
            cr.translate(ox, oy);
            cr.scale(fit, fit);
            cr.set_source_surface(surface, 0.0, 0.0).ok();
            let pattern = cr.source();
            pattern.set_filter(gtk4::cairo::Filter::Nearest);
            cr.paint().ok();
            cr.restore().ok();
            let _ = (w, h);

            // Hover crosshair (green), drawn in screen space at the voxel center.
            if let Some((hx, hy)) = hover {
                let fitx = ox + (hx as f64 + 0.5) * fit;
                let fity = oy + (hy as f64 + 0.5) * fit;
                let sx = pan_x + aw / 2.0 + (fitx - aw / 2.0) * zoom;
                let sy = pan_y + ah / 2.0 + (fity - ah / 2.0) * zoom;
                cr.set_source_rgba(0.0, 1.0, 0.0, 0.7);
                cr.set_line_width(1.0);
                cr.move_to(sx - 8.0, sy);
                cr.line_to(sx + 8.0, sy);
                cr.stroke().ok();
                cr.move_to(sx, sy - 8.0);
                cr.line_to(sx, sy + 8.0);
                cr.stroke().ok();

                // The readout, in this same frame — see `ui::coord_chip`.
                crate::ui::coord_chip::draw(
                    cr,
                    cursor.0,
                    cursor.1,
                    &hover_lines,
                    w as f64,
                    h as f64,
                );
            }

            // The shape being drawn right now, under the finished ones.
            if let (Some((vx, vy, half)), Some(surface)) =
                (*this.pending_shape.borrow(), this.annotation_surface())
            {
                let (sx, sy) = surface.voxel_to_screen(vx, vy);
                let r = half
                    * surface.units_to_pixels(&crate::models::annotation::Anchor::Data {
                        x: vx,
                        y: vy,
                        z: 0.0,
                    });
                let kind = this
                    .preview_kind
                    .borrow()
                    .as_ref()
                    .map(|f| f())
                    .unwrap_or(crate::models::annotation::AnnotationKind::Circle);
                crate::helpers::annotation_render::draw_preview(
                    kind,
                    sx,
                    sy,
                    r,
                    crate::models::annotation::MarkStyle::default(),
                    cr,
                );
            }

            // Marks last, so they sit over the image — and through the same
            // renderer the volume view and the FITS canvas use, so one shape
            // cannot start looking different depending on where you see it.
            if let Some(surface) = this.annotation_surface() {
                let editing = this.editing_annotation.borrow().clone();
                crate::helpers::annotation_render::draw(
                    &this.annotations.borrow(),
                    &surface,
                    this.selected_annotation.borrow().as_deref(),
                    editing.as_deref(),
                    cr,
                    w as f64,
                    h as f64,
                );
                // Grips on the edited mark alone — the same four the FITS
                // canvas draws, from the same function, so a grip cannot end
                // up somewhere the hit test is not looking.
                if let Some(mark) = editing.and_then(|id| {
                    this.annotations
                        .borrow()
                        .iter()
                        .find(|a| a.id == id)
                        .cloned()
                }) {
                    crate::helpers::annotation_render::draw_handles(&mark, &surface, cr);
                }
            }
        });
    }

    fn setup_waveform_draw(&self, heights: Vec<f32>) {
        self.waveform_area.set_draw_func(move |_area, cr, w, h| {
            if heights.len() < 2 {
                return;
            }
            let (wf, hf) = (w as f64, h as f64);
            let n = heights.len();
            cr.set_source_rgba(0.62, 0.77, 0.91, 0.31);
            cr.move_to(0.0, hf);
            for (i, &v) in heights.iter().enumerate() {
                let x = wf * i as f64 / (n - 1) as f64;
                let y = hf * (1.0 - v as f64);
                cr.line_to(x, y);
            }
            cr.line_to(wf, hf);
            cr.close_path();
            cr.fill().ok();
        });
    }

    fn setup_spectrum_draw(self: &Rc<Self>) {
        let this = self.clone();
        self.spectrum_area.set_draw_func(move |_area, cr, w, h| {
            cr.set_source_rgba(0.0, 0.0, 0.0, 0.19);
            let _ = cr.paint();

            let (wf, hf) = (w as f64, h as f64);
            let s = this.state.borrow();
            let sp = &s.spectrum;

            // Value range over finite channels.
            let mut mn = f32::MAX;
            let mut mx = f32::MIN;
            for &v in sp.iter() {
                if v.is_finite() {
                    mn = mn.min(v);
                    mx = mx.max(v);
                }
            }
            if sp.len() < 2 || mn > mx {
                cr.set_source_rgba(1.0, 1.0, 1.0, 0.63);
                cr.select_font_face(
                    "sans",
                    gtk4::cairo::FontSlant::Normal,
                    gtk4::cairo::FontWeight::Normal,
                );
                cr.set_font_size(12.0);
                cr.move_to(8.0, hf / 2.0);
                cr.show_text(crate::tr_en!("No signal at this spaxel")).ok();
                return;
            }

            let range = (mx - mn).max(1e-6);
            let nz = sp.len();
            // Spectrum polyline (cyan).
            cr.set_source_rgba(0.45, 0.85, 1.0, 1.0);
            cr.set_line_width(1.5);
            let mut started = false;
            for (z, &v) in sp.iter().enumerate() {
                let x = z as f64 / (nz - 1) as f64 * wf;
                let y = if v.is_finite() {
                    hf - 3.0 - (v - mn) as f64 / range as f64 * (hf - 6.0)
                } else {
                    hf
                };
                if !started {
                    cr.move_to(x, y);
                    started = true;
                } else {
                    cr.line_to(x, y);
                }
            }
            cr.stroke().ok();

            // Current-channel marker (dashed amber).
            let cx = s.z as f64 / (nz - 1) as f64 * wf;
            cr.set_source_rgba(1.0, 0.65, 0.24, 1.0);
            cr.set_line_width(1.0);
            cr.set_dash(&[3.0, 3.0], 0.0);
            cr.move_to(cx, 0.0);
            cr.line_to(cx, hf);
            cr.stroke().ok();
            cr.set_dash(&[], 0.0);
            drop(s);

            // Physical spectral-axis endpoint labels (from the cube WCS).
            cr.set_source_rgba(0.75, 0.85, 0.95, 0.85);
            cr.select_font_face(
                "monospace",
                gtk4::cairo::FontSlant::Normal,
                gtk4::cairo::FontWeight::Normal,
            );
            cr.set_font_size(10.0);
            let lo_lbl = this.wcs.channel_label(0);
            let hi_lbl = this.wcs.channel_label(nz - 1);
            cr.move_to(2.0, hf - 2.0);
            cr.show_text(&lo_lbl).ok();
            if let Ok(ext) = cr.text_extents(&hi_lbl) {
                cr.move_to((wf - ext.width() - 2.0).max(0.0), hf - 2.0);
                cr.show_text(&hi_lbl).ok();
            }
        });
    }

    // ── Gestures ─────────────────────────────────────────────────────────────

    fn setup_gestures(self: &Rc<Self>) {
        // Drag: pan when moved past a threshold, otherwise a clean click probes.
        let drag = gtk::GestureDrag::new();
        drag.set_button(1);
        let start = Rc::new(RefCell::new((0.0f64, 0.0f64, 0.0f64, 0.0f64, false)));
        {
            let this = self.clone();
            let start = start.clone();
            drag.connect_drag_begin(move |_, x, y| {
                // Take keyboard focus so Space / arrow-key channel stepping works.
                this.slice_area.grab_focus();
                let (pan_x, pan_y) = {
                    let s = this.state.borrow();
                    (s.pan_x, s.pan_y)
                };
                *start.borrow_mut() = (x, y, pan_x, pan_y, false);
                *this.drag_intent.borrow_mut() = this.intent_at(x, y);
            });
        }
        {
            let this = self.clone();
            let start = start.clone();
            drag.connect_drag_update(move |_, dx, dy| {
                // While placing, a drag sizes the mark; panning would drag the
                // image out from under the shape being drawn.
                let intent = this.drag_intent.borrow().clone();
                if intent == DragIntent::Place {
                    let st = *start.borrow();
                    this.size_pending(st.0, st.1, dx, dy);
                    return;
                }
                if intent != DragIntent::Pan {
                    let st = *start.borrow();
                    this.drag_mark(&intent, st.0 + dx, st.1 + dy);
                    return;
                }
                let mut st = start.borrow_mut();
                if !st.4 && (dx.abs() > PAN_THRESHOLD || dy.abs() > PAN_THRESHOLD) {
                    st.4 = true;
                }
                if st.4 {
                    let mut s = this.state.borrow_mut();
                    s.pan_x = st.2 + dx;
                    s.pan_y = st.3 + dy;
                    drop(s);
                    this.slice_area.queue_draw();
                }
            });
        }
        {
            let this = self.clone();
            let start = start.clone();
            drag.connect_drag_end(move |_, dx, dy| {
                let st = *start.borrow();
                let intent = this.drag_intent.borrow().clone();
                *this.drag_intent.borrow_mut() = DragIntent::Pan;
                match intent {
                    DragIntent::Place => this.place_at(st.0, st.1, dx, dy),
                    // A press on a mark that never moved is a click on it:
                    // pick it out and open it, the way clicking a shape does
                    // on the FITS canvas.
                    DragIntent::Move { ref id, .. } | DragIntent::Resize { ref id }
                        if dx.hypot(dy) <= PAN_THRESHOLD =>
                    {
                        this.announce_selected(Some(id.clone()));
                    }
                    DragIntent::Move { .. } | DragIntent::Resize { .. } => {
                        let marks = this.annotations.borrow().clone();
                        let cb = this.on_marks_changed.borrow().clone();
                        if let Some(cb) = cb {
                            cb(marks);
                        }
                    }
                    DragIntent::Pan => {
                        if !st.4 {
                            // A click on empty image with a mark open closes
                            // it — the same "click away to finish" the FITS
                            // viewer has — otherwise it probes a spectrum.
                            if this.editing_annotation.borrow().is_some() {
                                this.announce_selected(None);
                            } else {
                                this.probe_at(st.0, st.1);
                            }
                        }
                    }
                }
            });
        }
        self.slice_area.add_controller(drag);

        // Motion: hover readout + remember cursor for wheel-zoom anchoring.
        let motion = gtk::EventControllerMotion::new();
        {
            let this = self.clone();
            motion.connect_motion(move |_, x, y| {
                this.state.borrow_mut().last_cursor = (x, y);
                this.update_readout(x, y);
            });
        }
        {
            let this = self.clone();
            motion.connect_leave(move |_| {
                this.clear_readout();
            });
        }
        self.slice_area.add_controller(motion);

        // Scroll: zoom toward the cursor (matches the volume orbit-zoom feel).
        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        {
            let this = self.clone();
            scroll.connect_scroll(move |_, _dx, dy| {
                let cursor = this.state.borrow().last_cursor;
                this.zoom_toward(cursor.0, cursor.1, (-dy * 0.12).exp());
                glib::Propagation::Stop
            });
        }
        self.slice_area.add_controller(scroll);

        // Double-click resets zoom/pan.
        let dbl = gtk::GestureClick::new();
        dbl.set_button(1);
        {
            let this = self.clone();
            dbl.connect_pressed(move |_, n, _, _| {
                if n >= 2 {
                    {
                        let mut s = this.state.borrow_mut();
                        s.zoom = this.default_zoom.get();
                        s.pan_x = 0.0;
                        s.pan_y = 0.0;
                    }
                    this.slice_area.queue_draw();
                }
            });
        }
        self.slice_area.add_controller(dbl);

        // Click the spectrum to jump to that channel.
        let spec_click = gtk::GestureClick::new();
        spec_click.set_button(1);
        {
            let this = self.clone();
            spec_click.connect_pressed(move |_, _n, x, _y| {
                let nz = this.vol.nz;
                if nz < 2 {
                    return;
                }
                let w = this.spectrum_area.width().max(1) as f64;
                let c = (x / w * (nz - 1) as f64)
                    .round()
                    .clamp(0.0, (nz - 1) as f64) as usize;
                this.suppress.set(true);
                this.channel_scale.set_value(c as f64);
                this.suppress.set(false);
                this.set_channel(c);
            });
        }
        self.spectrum_area.add_controller(spec_click);
    }

    fn zoom_toward(&self, cursor_x: f64, cursor_y: f64, factor: f64) {
        let (_fit, _ox, _oy, aw, ah) = match self.fit_params() {
            Some(v) => v,
            None => return,
        };
        let (cx, cy) = (aw / 2.0, ah / 2.0);
        let mut s = self.state.borrow_mut();
        let new_zoom = zoom_after(s.zoom, factor);
        if (new_zoom - s.zoom).abs() < f64::EPSILON {
            return;
        }
        // Keep the fit-space point under the cursor fixed.
        let fx = cx + (cursor_x - s.pan_x - cx) / s.zoom;
        let fy = cy + (cursor_y - s.pan_y - cy) / s.zoom;
        s.zoom = new_zoom;
        s.pan_x = cursor_x - cx - (fx - cx) * new_zoom;
        s.pan_y = cursor_y - cy - (fy - cy) * new_zoom;
        drop(s);
        self.slice_area.queue_draw();
    }

    fn wire_controls(self: &Rc<Self>, spectrum_close: &gtk::Button) {
        {
            let this = self.clone();
            self.channel_scale.connect_value_changed(move |scale| {
                if this.suppress.get() {
                    return;
                }
                this.set_channel(scale.value().round().max(0.0) as usize);
            });
        }
        {
            let this = self.clone();
            self.play_btn.connect_clicked(move |_| {
                this.toggle_play();
            });
        }
        {
            let this = self.clone();
            spectrum_close.connect_clicked(move |_| {
                this.state.borrow_mut().probe = None;
                this.spectrum_revealer.set_reveal_child(false);
            });
        }
    }
}

/// Where a cube mark lands on the 2D slice, for one frame's view.
///
/// A snapshot rather than a borrow of the view: the renderer runs inside the
/// draw closure, which already holds `state`, and a second borrow there
/// panics.
struct SliceAnnotationSurface {
    fit: f64,
    ox: f64,
    oy: f64,
    aw: f64,
    ah: f64,
    zoom: f64,
    pan_x: f64,
    pan_y: f64,
    /// The channel on screen. A mark on another one is not drawn.
    z: usize,
    disp_nx: f64,
    disp_ny: f64,
    vol_nx: f64,
    vol_ny: f64,
}

impl SliceAnnotationSurface {
    /// Screen point to a CONTINUOUS volume-voxel position, or `None` when the
    /// point is off the plane.
    ///
    /// The exact inverse of [`Self::voxel_to_screen`], and it lives beside it
    /// so the two are read together: placing uses this and drawing uses that,
    /// so any disagreement between them puts a mark somewhere other than where
    /// the user clicked. A round-trip test pins it.
    fn screen_to_voxel(&self, px: f64, py: f64) -> Option<(f64, f64)> {
        if self.fit <= 0.0 || self.zoom <= 0.0 || self.disp_nx <= 0.0 || self.disp_ny <= 0.0 {
            return None;
        }
        let fx = self.aw / 2.0 + (px - self.pan_x - self.aw / 2.0) / self.zoom;
        let fy = self.ah / 2.0 + (py - self.pan_y - self.ah / 2.0) / self.zoom;
        let dx = (fx - self.ox) / self.fit;
        let dy = (fy - self.oy) / self.fit;
        if dx < 0.0 || dy < 0.0 || dx >= self.disp_nx || dy >= self.disp_ny {
            return None;
        }
        Some((
            dx * self.vol_nx / self.disp_nx,
            dy * self.vol_ny / self.disp_ny,
        ))
    }

    /// Volume voxel to screen, the same chain the image itself is drawn
    /// through: fit, then pan and zoom about the widget centre.
    fn voxel_to_screen(&self, vx: f64, vy: f64) -> (f64, f64) {
        let dx = vx * self.disp_nx / self.vol_nx.max(1.0);
        let dy = vy * self.disp_ny / self.vol_ny.max(1.0);
        let fitx = self.ox + dx * self.fit;
        let fity = self.oy + dy * self.fit;
        (
            self.pan_x + self.aw / 2.0 + (fitx - self.aw / 2.0) * self.zoom,
            self.pan_y + self.ah / 2.0 + (fity - self.ah / 2.0) * self.zoom,
        )
    }
}

impl crate::helpers::annotation_render::AnnotationSurface for SliceAnnotationSurface {
    fn project(&self, anchor: &crate::models::annotation::Anchor) -> Option<(f64, f64)> {
        use crate::models::annotation::Anchor;
        let Anchor::Data { x, y, z } = *anchor else {
            // A sky position or a FITS image pixel means nothing in voxel
            // space; skipped rather than guessed at.
            return None;
        };
        // Only marks on the channel being shown. A mark three channels away is
        // not AT this position on this plane, and drawing it here would say it
        // was — the marks list carries the others, with their channel.
        if (z.round() as i64) != self.z as i64 {
            return None;
        }
        Some(self.voxel_to_screen(x, y))
    }

    fn units_to_pixels(&self, _anchor: &crate::models::annotation::Anchor) -> f64 {
        // One volume voxel, in screen pixels. Uniform across the plane — unlike
        // the volume view, this projection has no perspective.
        let a = self.voxel_to_screen(0.0, 0.0);
        let b = self.voxel_to_screen(1.0, 0.0);
        ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt().max(0.25)
    }
}

/// Whether a newly measured default should move a view sitting at `current`.
///
/// Only if it is still where the last default put it. Someone who has scrolled
/// has said what they want to look at, and a default arriving later — the
/// match is recomputed on every switch back to the slice — must not take it
/// away from them. Without this, zooming in on the slice and glancing at the
/// volume would silently undo the zoom.
fn default_is_welcome(current: f64, previous_default: f64) -> bool {
    (current - previous_default).abs() < 1e-9
}

/// The zoom a scroll step lands on, clamped to what the view allows.
///
/// A named function because the range is the interesting part and it is
/// otherwise buried in a widget that only allocates under a real display: the
/// floor used to be 1.0, so the wheel refused to pull the plane back from
/// fit-to-widget at all.
fn zoom_after(current: f64, factor: f64) -> f64 {
    (current * factor).clamp(MIN_ZOOM, MAX_ZOOM)
}

/// Map a displayed-slice pixel to the matching index in a `target_n`-wide axis
/// (down-sampled volume voxel, or native WCS pixel). Mirrors `MapDispToVolume`.
fn map_disp_to(p: usize, disp_n: usize, target_n: usize) -> usize {
    if target_n == 0 {
        return 0;
    }
    if disp_n > 0 {
        (((p as u64) * (target_n as u64)) / (disp_n as u64)).min((target_n - 1) as u64) as usize
    } else {
        p.min(target_n - 1)
    }
}

/// Map a down-sampled channel index to the matching native channel (endpoints
/// exact). Mirrors `MapNativeChannel`.
fn map_native_channel(ch: usize, down_nz: usize, orig_nz: usize) -> usize {
    if orig_nz == 0 {
        return 0;
    }
    if down_nz > 1 && orig_nz > 1 {
        let t = (ch as f64 / (down_nz - 1) as f64 * (orig_nz - 1) as f64).round() as i64;
        t.clamp(0, (orig_nz - 1) as i64) as usize
    } else {
        ch.min(orig_nz - 1)
    }
}

/// NaN-aware per-channel mean intensity, normalized to `[0, 1]` heights for the
/// scrubber waveform backdrop.
fn channel_profile(vol: &VolumeData) -> Vec<f32> {
    let (nx, ny, nz) = (vol.nx, vol.ny, vol.nz);
    if nz == 0 || nx == 0 || ny == 0 {
        return Vec::new();
    }
    let plane = nx * ny;
    let mut means = Vec::with_capacity(nz);
    for z in 0..nz {
        let base = z * plane;
        let mut sum = 0.0f64;
        let mut cnt = 0u64;
        for &v in &vol.data[base..base + plane] {
            if v.is_finite() {
                sum += v as f64;
                cnt += 1;
            }
        }
        means.push(if cnt > 0 {
            (sum / cnt as f64) as f32
        } else {
            0.0
        });
    }
    let mn = means.iter().copied().fold(f32::INFINITY, f32::min);
    let mx = means.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    if mx > mn {
        means.iter().map(|&v| (v - mn) / (mx - mn)).collect()
    } else {
        vec![0.5; nz]
    }
}

#[cfg(test)]
mod slice_annotation_tests_support {
    use super::SliceAnnotationSurface;

    /// A surface with no pan, no zoom, and the displayed plane at the volume's
    /// own resolution — the plain case, where the arithmetic is checkable by
    /// hand. Shared so the projection and placement tests cannot drift onto
    /// different geometry and both pass while disagreeing.
    pub fn surface(z: usize) -> SliceAnnotationSurface {
        SliceAnnotationSurface {
            fit: 4.0,
            ox: 10.0,
            oy: 20.0,
            aw: 400.0,
            ah: 300.0,
            zoom: 1.0,
            pan_x: 0.0,
            pan_y: 0.0,
            z,
            disp_nx: 64.0,
            disp_ny: 64.0,
            vol_nx: 64.0,
            vol_ny: 64.0,
        }
    }
}

#[cfg(test)]
mod slice_annotation_tests {
    use super::slice_annotation_tests_support::surface;
    use crate::helpers::annotation_render::AnnotationSurface;
    use crate::models::annotation::Anchor;

    /// A mark on another channel is not drawn.
    ///
    /// The important half of this view's projection. A cube mark sits on one
    /// plane; drawing it on every plane would say it was at that position in
    /// all of them, which is exactly the false statement the renderer's
    /// "skip, do not clamp" rule exists to avoid.
    #[test]
    fn a_mark_on_another_channel_is_not_drawn() {
        let s = surface(12);
        let here = Anchor::Data {
            x: 30.0,
            y: 30.0,
            z: 12.0,
        };
        let elsewhere = Anchor::Data {
            x: 30.0,
            y: 30.0,
            z: 13.0,
        };
        assert!(s.project(&here).is_some(), "the current channel draws");
        assert!(
            s.project(&elsewhere).is_none(),
            "a mark one channel away must not be drawn here"
        );
    }

    /// A sky or image-pixel anchor means nothing in voxel space.
    #[test]
    fn only_voxel_anchors_project() {
        let s = surface(0);
        assert!(s
            .project(&Anchor::Sky {
                ra_deg: 202.0,
                dec_deg: 47.0
            })
            .is_none());
        assert!(s.project(&Anchor::ImagePixel { x: 5.0, y: 5.0 }).is_none());
    }

    /// The projection follows the same chain the image is drawn through.
    ///
    /// Voxel 0 sits at the image origin `ox, oy`, and one voxel spans `fit`
    /// screen pixels — so a mark lands on the feature under it rather than
    /// beside it.
    #[test]
    fn a_voxel_lands_where_the_image_draws_it() {
        let s = surface(0);
        assert_eq!(s.voxel_to_screen(0.0, 0.0), (10.0, 20.0));
        assert_eq!(s.voxel_to_screen(1.0, 0.0), (14.0, 20.0));
        assert!(
            (s.units_to_pixels(&Anchor::Data {
                x: 0.0,
                y: 0.0,
                z: 0.0
            }) - 4.0)
                .abs()
                < 1e-9
        );
    }

    /// Zoom and pan move marks with the image, not independently of it.
    #[test]
    fn marks_travel_with_zoom_and_pan() {
        let mut s = surface(0);
        s.zoom = 2.0;
        s.pan_x = 7.0;
        s.pan_y = -3.0;
        // Same chain as the draw function: fit, then zoom about the widget
        // centre, then pan.
        let expect =
            |v: f64, o: f64, half: f64, pan: f64| pan + half + ((o + v * 4.0) - half) * 2.0;
        let (sx, sy) = s.voxel_to_screen(5.0, 6.0);
        assert!((sx - expect(5.0, 10.0, 200.0, 7.0)).abs() < 1e-9, "x {sx}");
        assert!((sy - expect(6.0, 20.0, 150.0, -3.0)).abs() < 1e-9, "y {sy}");
        // A mark keeps its size on the DATA: at 2x zoom one voxel is 8px.
        assert!(
            (s.units_to_pixels(&Anchor::Data {
                x: 0.0,
                y: 0.0,
                z: 0.0
            }) - 8.0)
                .abs()
                < 1e-9
        );
    }

    /// A native-resolution plane does not shift the marks.
    ///
    /// The slice can show a plane with more pixels than the in-RAM cube, while
    /// marks are anchored in VOLUME voxels. Getting this ratio wrong puts every
    /// mark at a fraction of its true position — visible only on the files that
    /// have a native source, which is not the one you test on.
    #[test]
    fn a_native_resolution_plane_keeps_marks_in_place() {
        let mut s = surface(0);
        s.disp_nx = 256.0;
        s.disp_ny = 256.0; // 4x the volume's 64
                           // Voxel 32 is halfway across the volume, so halfway across the
                           // displayed plane too: display pixel 128.
        assert_eq!(s.voxel_to_screen(32.0, 0.0).0, 10.0 + 128.0 * 4.0);
    }
}

#[cfg(test)]
mod slice_placement_tests {
    use super::slice_annotation_tests_support::*;

    /// Where you click is where the mark lands.
    ///
    /// Placing goes screen -> voxel and drawing goes voxel -> screen; if the
    /// two disagree the mark appears beside the feature you pointed at, which
    /// looks like a rendering bug and is an arithmetic one. Checked across
    /// zoom, pan, and a native-resolution plane, because each of those is a
    /// term that can be dropped from one direction and not the other.
    #[test]
    fn a_click_round_trips_to_the_pixel_it_came_from() {
        for (zoom, pan_x, pan_y, disp) in [
            (1.0, 0.0, 0.0, 64.0),
            (2.5, 17.0, -9.0, 64.0),
            (0.4, -30.0, 12.0, 256.0),
        ] {
            let mut s = surface(0);
            s.zoom = zoom;
            s.pan_x = pan_x;
            s.pan_y = pan_y;
            s.disp_nx = disp;
            s.disp_ny = disp;
            for (vx, vy) in [(0.0, 0.0), (31.5, 8.25), (63.0, 63.0)] {
                let (sx, sy) = s.voxel_to_screen(vx, vy);
                let back = s.screen_to_voxel(sx, sy).expect("on the plane");
                assert!(
                    (back.0 - vx).abs() < 1e-9 && (back.1 - vy).abs() < 1e-9,
                    "zoom {zoom} pan ({pan_x},{pan_y}) disp {disp}: \
                     voxel ({vx},{vy}) came back as {back:?}"
                );
            }
        }
    }

    /// A click outside the plane places nothing rather than clamping to an edge.
    #[test]
    fn a_click_off_the_plane_places_nothing() {
        let s = surface(0);
        let (sx, sy) = s.voxel_to_screen(0.0, 0.0);
        assert!(
            s.screen_to_voxel(sx - 1.0, sy).is_none(),
            "left of the plane"
        );
        assert!(s.screen_to_voxel(sx, sy - 1.0).is_none(), "above the plane");
        let (ex, ey) = s.voxel_to_screen(64.0, 64.0);
        assert!(s.screen_to_voxel(ex, ey).is_none(), "past the far corner");
    }
}

#[cfg(test)]
mod slice_zoom_tests {
    use super::{default_is_welcome, zoom_after, MAX_ZOOM, MIN_ZOOM};

    /// The plane can be pulled back from fit.
    ///
    /// The floor was 1.0 — fit-to-widget — so scrolling out simply stopped,
    /// and the slice could not be put at the same distance the volume frames
    /// the box at. Everything, marks included, changed size when you switched
    /// modes and there was no way to correct it.
    #[test]
    fn the_wheel_can_pull_back_past_fit() {
        let out = zoom_after(1.0, 0.8);
        assert!(out < 1.0, "scrolling out from fit went to {out}");
        // And keeps going, rather than stopping one step below.
        assert!(zoom_after(out, 0.8) < out, "the wheel stalled at {out}");
    }

    /// The range is bounded at both ends.
    #[test]
    fn zoom_stops_at_both_ends() {
        assert_eq!(
            zoom_after(MIN_ZOOM, 0.1),
            MIN_ZOOM,
            "zoomed out past the floor"
        );
        assert_eq!(
            zoom_after(MAX_ZOOM, 10.0),
            MAX_ZOOM,
            "zoomed in past the ceiling"
        );
    }

    /// A default only moves a view nobody has touched.
    #[test]
    fn a_zoom_the_user_chose_is_left_alone() {
        // Untouched: sitting exactly where the last default put it.
        assert!(default_is_welcome(1.0, 1.0));
        assert!(default_is_welcome(0.465, 0.465));
        // Scrolled away from it: theirs now.
        assert!(
            !default_is_welcome(2.5, 1.0),
            "a scrolled-in view was reset"
        );
        assert!(
            !default_is_welcome(0.3, 0.465),
            "a scrolled-out view was reset"
        );
    }

    /// The default the volume match produces is inside the range.
    ///
    /// Measured at about 0.47 across cube shapes. A floor above that would
    /// clamp the match away silently and put fit back.
    #[test]
    fn the_measured_match_is_reachable() {
        // The probe measures about 0.47 across cube shapes. A floor above it
        // would clamp the match away and silently put fit back.
        for measured in [0.479, 0.465, 0.45] {
            assert_eq!(
                zoom_after(measured, 1.0),
                measured,
                "a measured match of {measured} is clamped away"
            );
        }
    }
}
