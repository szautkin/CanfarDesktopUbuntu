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
const PAN_THRESHOLD: f64 = 6.0;
/// Cap the on-screen native slice bitmap's longest axis (mirrors `SliceDisplayCap`).
const SLICE_DISPLAY_CAP: usize = 2048;

/// Per-cube slice display / interaction state.
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
    channel_label: gtk::Label,
    coord_label: gtk::Label,
    /// Floating readout chip (lon/lat/spectral/value) that tracks the pointer.
    cursor_chip: gtk::Label,
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

        // Floating cursor chip layered over the slice via an Overlay.
        ensure_chip_css();
        let cursor_chip = gtk::Label::new(None);
        cursor_chip.add_css_class("cube-cursor-chip");
        cursor_chip.set_halign(gtk::Align::Start);
        cursor_chip.set_valign(gtk::Align::Start);
        cursor_chip.set_xalign(0.0);
        cursor_chip.set_justify(gtk::Justification::Left);
        cursor_chip.set_visible(false);
        cursor_chip.set_can_target(false); // never intercept the pointer
        let slice_overlay = gtk::Overlay::new();
        slice_overlay.set_hexpand(true);
        slice_overlay.set_vexpand(true);
        slice_overlay.set_child(Some(&slice_area));
        slice_overlay.add_overlay(&cursor_chip);
        slice_overlay.set_measure_overlay(&cursor_chip, false);
        slice_overlay.set_clip_overlay(&cursor_chip, true);
        widget.append(&slice_overlay);

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
        widget.append(&bottom_bar);

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
            channel_label,
            coord_label,
            cursor_chip,
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
                probe: None,
                spectrum: Vec::new(),
                playing: false,
            }),
            suppress: Cell::new(false),
            play_gen: Cell::new(0),
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
        self.cursor_chip.set_text(&lines.join("\n"));
        self.cursor_chip.set_visible(true);
        self.position_chip(px, py);

        self.slice_area.queue_draw();
    }

    /// Place the chip near the pointer, clamped inside the slice viewport
    /// (mirrors `PositionCursorChip`: +16 right, above the pointer).
    fn position_chip(&self, px: f64, py: f64) {
        let (_, cw, _, _) = self.cursor_chip.measure(gtk::Orientation::Horizontal, -1);
        let (_, ch, _, _) = self.cursor_chip.measure(gtk::Orientation::Vertical, cw);
        let vw = self.slice_area.width();
        let vh = self.slice_area.height();
        let cx = (px as i32 + 16).clamp(0, (vw - cw).max(0));
        let cy = (py as i32 - ch - 12).clamp(0, (vh - ch).max(0));
        self.cursor_chip.set_margin_start(cx);
        self.cursor_chip.set_margin_top(cy);
    }

    /// Hide the chip + crosshair and reset the coordinate bar to its hint.
    fn clear_readout(&self) {
        self.state.borrow_mut().hover = None;
        self.cursor_chip.set_visible(false);
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
            let (zoom, pan_x, pan_y, hover) = {
                let s = this.state.borrow();
                (s.zoom, s.pan_x, s.pan_y, s.hover)
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
            });
        }
        {
            let this = self.clone();
            let start = start.clone();
            drag.connect_drag_update(move |_, dx, dy| {
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
            drag.connect_drag_end(move |_, _dx, _dy| {
                let st = *start.borrow();
                if !st.4 {
                    this.probe_at(st.0, st.1);
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
                        s.zoom = 1.0;
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
        let new_zoom = (s.zoom * factor).clamp(1.0, MAX_ZOOM);
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

/// Register the floating-cursor-chip style once for the whole app.
fn ensure_chip_css() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let provider = gtk::CssProvider::new();
        provider.load_from_string(
            ".cube-cursor-chip { \
                background-color: rgba(20, 22, 28, 0.85); \
                color: #eaeef5; \
                border: 1px solid rgba(255, 255, 255, 0.14); \
                border-radius: 6px; \
                padding: 3px 7px; \
                font-family: monospace; \
                font-size: 10pt; }",
        );
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
    });
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
