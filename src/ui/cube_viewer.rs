//! Per-cube page for the Cube Viewer.
//!
//! Port of `Views/CubeViewer/CubeViewerPage.xaml(.cs)` +
//! `CubeViewerPage.Transfer.cs`. LEFT: a [`gtk::Stack`] switching between the GL
//! volume ray-marcher ([`CubeVolumeGl`]) in "3D" mode and a [`CubeSliceView`] in
//! "Slice" mode (forced to Slice, with the 3D toggle disabled, when GL is
//! unavailable). RIGHT: a control column — colormap + stretch pickers, a window
//! low/high pair, density / quality sliders, MIP + auto-orbit toggles, a compact
//! opacity transfer-function editor, an Info expander, and an "Export…" button.
//! Every control drives BOTH the GL volume and the slice view so the two stay
//! visually consistent.

use crate::helpers::cube_axes;
use crate::helpers::cube_colormaps;
use crate::helpers::cube_slice::StretchMode;
use crate::helpers::cube_wcs::CubeWcs;
use crate::helpers::transfer_function::TransferFunctionModel;
use crate::models::volume_data::{native_to_resident, resident_to_native, VolumeData};
use crate::ui::cube_slice_view::CubeSliceView;
use crate::ui::cube_volume_gl::CubeVolumeGl;
use crate::ui::viewer_shell::{self, labeled};
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

/// Stretch curves the cube offers, in shader-index order.
///
/// One list: it populates the dropdown, `set_cube_view` advertises it, and
/// `set_stretch_by_name` parses it. `StretchMode::from_index` maps a position
/// here to the shader mode, so the ORDER is load-bearing — reordering these
/// would silently apply a different curve than the one named.
pub const STRETCH_NAMES: &[&str] = &["Linear", "Log", "Sqrt", "Squared", "Asinh"];

pub struct CubeViewer {
    pub widget: gtk::Box,
    gl: Rc<CubeVolumeGl>,
    slice: Rc<CubeSliceView>,
    /// Cube name + WCS + normalized volume, kept for the axes overlay + export.
    vol: Rc<VolumeData>,
    wcs: Rc<CubeWcs>,
    name: String,
    stack: gtk::Stack,
    mode_3d: gtk::ToggleButton,
    mode_slice: gtk::ToggleButton,
    volume_section: gtk::Box,
    /// Transparent wireframe + WCS-caption overlay stacked on the GL surface.
    overlay_area: gtk::DrawingArea,
    /// Current spectral channel for the slice-plane marker.
    ///
    /// Mirrors the scrubber, which is the single source of truth: this used to
    /// be a SECOND channel, decoupled from the slice view's, so the marker in
    /// the volume stayed where it was seeded however far you scrubbed.
    current_channel: Cell<usize>,
    // Handles the overlay / colorbar methods read back.
    window_lo: gtk::Scale,
    window_hi: gtk::Scale,
    colormap: gtk::DropDown,
    stretch: gtk::DropDown,
    colorbar_area: gtk::DrawingArea,
    colorbar_lo: gtk::Label,
    colorbar_hi: gtk::Label,
    captions_toggle: gtk::CheckButton,
    slice_toggle: gtk::CheckButton,
    transfer_area: gtk::DrawingArea,
    transfer: RefCell<TransferFunctionModel>,
    tf_drag: Cell<i32>,
    tf_start: Cell<(f64, f64)>,
    /// Bounded polling for GL realize/availability.
    probe_tries: Cell<u32>,
}

/// One channel of a probed spectrum.
///
/// `normalized` is the display-space value in `[0,1]`; `physical` maps it back
/// through the display cut into the cube's own units. Both are `None` for a
/// blanked (NaN) voxel — a distinction a plain `f32` cannot carry, and one that
/// must survive to the wire rather than becoming a fabricated zero.
pub struct SpectrumSample {
    pub normalized: Option<f32>,
    pub physical: Option<f64>,
    /// The channel index in the FILE, not in the strided resident volume.
    pub native_channel: usize,
}

impl CubeViewer {
    pub fn new(vol: VolumeData, wcs: CubeWcs, name: String) -> Rc<Self> {
        let vol = Rc::new(vol);
        let wcs = Rc::new(wcs);

        // ── GL volume + 2D slice (share one normalized volume) ──────────────
        let gl = CubeVolumeGl::new();
        gl.set_volume((*vol).clone());
        let slice = CubeSliceView::new(vol.clone(), wcs.clone());

        // ── LEFT: mode toggle + stack ───────────────────────────────────────
        let left = gtk::Box::new(gtk::Orientation::Vertical, 0);
        left.set_hexpand(true);
        left.set_vexpand(true);

        let mode_bar = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        mode_bar.add_css_class("linked");
        mode_bar.set_halign(gtk::Align::Center);
        mode_bar.set_margin_top(8);
        mode_bar.set_margin_bottom(8);
        let mode_3d = gtk::ToggleButton::with_label(crate::tr_en!("3D"));
        mode_3d.set_active(true);
        let mode_slice = gtk::ToggleButton::with_label(crate::tr_en!("Slice"));
        mode_slice.set_group(Some(&mode_3d));
        mode_bar.append(&mode_3d);
        mode_bar.append(&mode_slice);
        left.append(&mode_bar);

        let stack = gtk::Stack::new();
        stack.set_hexpand(true);
        stack.set_vexpand(true);
        stack.set_transition_type(gtk::StackTransitionType::Crossfade);

        // Wrap the GL surface in an Overlay carrying a transparent DrawingArea for
        // the wireframe box + WCS axis captions + slice-plane marker. The overlay
        // is hit-test transparent so orbit/zoom input reaches the GLArea beneath.
        let gl_overlay = gtk::Overlay::new();
        gl_overlay.set_hexpand(true);
        gl_overlay.set_vexpand(true);
        gl_overlay.set_child(Some(gl.widget()));
        let overlay_area = gtk::DrawingArea::new();
        overlay_area.set_can_target(false);
        overlay_area.set_can_focus(false);
        overlay_area.set_hexpand(true);
        overlay_area.set_vexpand(true);
        gl_overlay.add_overlay(&overlay_area);

        stack.add_named(&gl_overlay, Some("volume"));
        stack.add_named(slice.widget(), Some("slice"));
        stack.set_visible_child_name("volume");
        left.append(&stack);

        // The channel scrubber sits UNDER the mode stack, not inside the slice
        // view, so it is on screen in both modes — the reference is explicit
        // that it "lives in BOTH modes: in slice mode it shows the 2D plane, in
        // volume mode it drives the slice-plane marker". Ours was reachable only
        // in slice mode, and the marker it should have driven was a second,
        // unconnected channel that nothing on screen could move.
        left.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        left.append(slice.channel_bar());

        // ── RIGHT: scrollable control column ────────────────────────────────
        let (controls, ctl) = build_controls(&name);

        let shell = viewer_shell::shell(&left, &controls.scroll);
        // The controls toggle sits with the mode buttons: both decide what the
        // left pane is showing you.
        mode_bar.append(&shell.sidebar_toggle);

        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        widget.set_hexpand(true);
        widget.set_vexpand(true);
        widget.append(&shell.widget);

        // ── Seed the Info expander from the cube metadata ───────────────────
        fill_info(&ctl.info_grid, &vol, &wcs, &name);

        let this = Rc::new(CubeViewer {
            widget,
            gl,
            slice,
            vol: vol.clone(),
            wcs: wcs.clone(),
            name: name.clone(),
            stack,
            mode_3d,
            mode_slice,
            volume_section: ctl.volume_section.clone(),
            overlay_area,
            current_channel: Cell::new(vol.nz / 2),
            window_lo: ctl.window_lo.clone(),
            window_hi: ctl.window_hi.clone(),
            colormap: ctl.colormap.clone(),
            stretch: ctl.stretch.clone(),
            colorbar_area: ctl.colorbar_area.clone(),
            colorbar_lo: ctl.colorbar_lo.clone(),
            colorbar_hi: ctl.colorbar_hi.clone(),
            captions_toggle: ctl.captions_toggle.clone(),
            slice_toggle: ctl.slice_toggle.clone(),
            transfer_area: ctl.transfer_area.clone(),
            transfer: RefCell::new(TransferFunctionModel::default_ramp()),
            tf_drag: Cell::new(-1),
            tf_start: Cell::new((0.0, 0.0)),
            probe_tries: Cell::new(0),
        });

        // Seed both renderers with the initial control state.
        this.gl.set_colormap(cube_colormaps::DEFAULT);
        this.slice.set_colormap(cube_colormaps::DEFAULT);
        this.gl.set_spectral_scale(ctl.spectral.value() as f32);
        this.gl.set_background([0.06, 0.06, 0.08]); // "Dark" (default dropdown row)
        this.apply_transfer();

        this.wire_mode_toggle();
        this.wire_channel();
        this.wire_controls(&ctl);
        this.setup_transfer_editor();
        this.setup_overlay();
        this.setup_colorbar();
        this.refresh_colorbar();
        this.start_gl_probe();

        this
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.widget
    }

    /// Attach a native-resolution plane reader for this cube's file, so the 2D
    /// slice renders at full FITS resolution (and hover sky coords are exact).
    /// A no-op when the file isn't a plain, seekable, modestly-sized cube.
    pub fn set_source_path(&self, path: &std::path::Path) {
        if let Some(src) = crate::helpers::cube_native_slice::NativeSliceSource::try_open(path) {
            self.slice.set_native_source(src);
        }
    }

    // ── Live MCP accessors (per-viewer cube tools) ───────────────────────────

    /// The GL volume renderer, for live camera control + figure export.
    pub fn gl(&self) -> &Rc<CubeVolumeGl> {
        &self.gl
    }

    /// Native cube voxel dimensions `(nx, ny, nz)`.
    pub fn dims(&self) -> (usize, usize, usize) {
        (self.vol.nx, self.vol.ny, self.vol.nz)
    }

    /// The current spectral channel (slice-plane marker position).
    pub fn current_channel(&self) -> usize {
        self.current_channel.get()
    }

    /// Move the slice-plane marker to `ch` (clamped to the cube depth) and repaint
    /// the wireframe overlay.
    pub fn set_current_channel(&self, ch: usize) {
        // Through the scrubber, so an agent's change moves the control a person
        // is looking at — and so `get_cube_view` reports what the slider shows.
        self.slice.set_channel_from(ch);
        let c = ch.min(self.vol.nz.saturating_sub(1));
        self.current_channel.set(c);
        self.overlay_area.queue_draw();
    }

    /// The physical value unit (`BUNIT`) of the cube, when the header provided one.
    pub fn value_unit(&self) -> Option<String> {
        self.vol.meta.as_ref().and_then(|m| m.bunit.clone())
    }

    /// Sample the spectrum through voxel column `(x, y)` across every channel.
    /// Each entry is `(normalized, physical)`: `normalized` is the display-cut
    /// value in `[0,1]` (`None` for a blank/NaN voxel) and `physical` is that value
    /// mapped back through the cube metadata (`None` without metadata or on NaN).
    /// Returns `None` when `(x, y)` is outside the cube footprint.
    /// The spectrum through one spaxel, addressed in **native cube pixels**.
    ///
    /// A large cube is strided down before upload, so the in-memory volume is
    /// smaller than the file. Callers (and the UI's own readouts) work in native
    /// pixels, so bounds-checking against the strided array rejected perfectly
    /// valid coordinates — the reference hit exactly this. Native coordinates are
    /// mapped onto the resident array here instead.
    ///
    /// Each entry is `(normalized, physical, native_channel)`; a blanked voxel
    /// (NaN) yields `None` for both values rather than a fabricated zero.
    pub fn spectrum_at(&self, x: usize, y: usize) -> Option<Vec<SpectrumSample>> {
        let (native_x, native_y, native_z) = self.native_dims();
        if x >= native_x || y >= native_y {
            return None;
        }
        let ax = native_to_resident(x, native_x, self.vol.nx);
        let ay = native_to_resident(y, native_y, self.vol.ny);

        let meta = self.vol.meta.as_ref();
        Some(
            (0..self.vol.nz)
                .map(|z| {
                    // Report the NATIVE channel this sample stands for, so the
                    // spectral axis lines up with the file rather than the stride.
                    let native_channel = resident_to_native(z, self.vol.nz, native_z);
                    let v = self.vol.sample(ax, ay, z);
                    if v.is_nan() {
                        SpectrumSample {
                            normalized: None,
                            physical: None,
                            native_channel,
                        }
                    } else {
                        SpectrumSample {
                            normalized: Some(v),
                            physical: meta.map(|m| m.value_at_normalized(v as f64)),
                            native_channel,
                        }
                    }
                })
                .collect(),
        )
    }

    /// The cube's dimensions as they are in the FILE, which may exceed the
    /// resident (strided) volume.
    pub fn native_dims(&self) -> (usize, usize, usize) {
        match self.vol.meta.as_ref() {
            Some(m) => (m.nx, m.ny, m.nz),
            None => (self.vol.nx, self.vol.ny, self.vol.nz),
        }
    }

    /// True when the resident volume was strided below the file's dimensions.
    pub fn is_downsampled(&self) -> bool {
        self.vol
            .meta
            .as_ref()
            .map(|m| m.is_downsampled())
            .unwrap_or(false)
    }

    /// Replace the opacity transfer curve's control points.
    ///
    /// Points are sorted and the endpoints pinned to x=0 and x=1, so a caller
    /// cannot leave the ramp with an undefined span. Returns the applied points.
    pub fn set_transfer_points(&self, mut points: Vec<(f32, f32)>) -> Vec<(f32, f32)> {
        points.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        if let Some(first) = points.first_mut() {
            first.0 = 0.0;
        }
        if let Some(last) = points.last_mut() {
            last.0 = 1.0;
        }
        self.transfer.borrow_mut().points = points;
        self.apply_transfer();
        self.transfer_area.queue_draw();
        self.transfer.borrow().points.clone()
    }

    /// Restore the renderer's default opacity ramp.
    pub fn reset_transfer(&self) -> Vec<(f32, f32)> {
        self.transfer.borrow_mut().reset();
        self.apply_transfer();
        self.transfer_area.queue_draw();
        self.transfer.borrow().points.clone()
    }

    /// The current transfer-curve control points.
    pub fn transfer_points(&self) -> Vec<(f32, f32)> {
        self.transfer.borrow().points.clone()
    }

    /// The cube's display name (its tab title).
    /// The colormap currently applied, by name.
    pub fn colormap_name(&self) -> String {
        cube_colormaps::NAMES
            .get(self.colormap.selected() as usize)
            .copied()
            .unwrap_or_default()
            .to_string()
    }

    /// The stretch curve currently applied, by name.
    pub fn stretch_name(&self) -> String {
        STRETCH_NAMES
            .get(self.stretch.selected() as usize)
            .copied()
            .unwrap_or_default()
            .to_string()
    }

    /// The display window, normalised 0..1.
    pub fn window(&self) -> (f64, f64) {
        (self.window_lo.value(), self.window_hi.value())
    }

    /// Whether the WCS axis captions are shown.
    pub fn captions_visible(&self) -> bool {
        self.captions_toggle.is_active()
    }

    /// Whether the slice-plane marker is shown in the volume.
    pub fn slice_plane_visible(&self) -> bool {
        self.slice_toggle.is_active()
    }

    /// Set the colormap by name, as the dropdown does.
    ///
    /// Drives the WIDGET rather than the renderer directly: its handler is what
    /// applies the change to the volume, the slice and the colorbar together,
    /// and it leaves the control showing what is actually in effect. An agent
    /// changing the view behind a stale dropdown would be worse than no control
    /// at all. `false` when the name is not one this build offers.
    pub fn set_colormap_by_name(&self, name: &str) -> bool {
        let Some(index) = cube_colormaps::NAMES
            .iter()
            .position(|n| n.eq_ignore_ascii_case(name))
        else {
            return false;
        };
        self.colormap.set_selected(index as u32);
        true
    }

    /// Set the stretch curve by name. See [`Self::set_colormap_by_name`].
    pub fn set_stretch_by_name(&self, name: &str) -> bool {
        let Some(index) = STRETCH_NAMES
            .iter()
            .position(|n| n.eq_ignore_ascii_case(name))
        else {
            return false;
        };
        self.stretch.set_selected(index as u32);
        true
    }

    /// Move the display window. Values are normalised 0..1; the sliders clamp.
    ///
    /// A window whose low is above its high renders nothing, so the two are
    /// ordered before they are applied rather than left to produce a blank
    /// volume the caller cannot explain.
    pub fn set_window(&self, lo: Option<f64>, hi: Option<f64>) {
        let mut low = lo.unwrap_or_else(|| self.window_lo.value());
        let mut high = hi.unwrap_or_else(|| self.window_hi.value());
        if low > high {
            std::mem::swap(&mut low, &mut high);
        }
        self.window_lo.set_value(low);
        self.window_hi.set_value(high);
    }

    /// Apply a window preset: `minmax` (full range) or `p99` (1st–99th
    /// percentile), the two buttons the controls offer.
    pub fn set_window_preset(&self, preset: &str) -> bool {
        match preset.trim().to_ascii_lowercase().as_str() {
            "minmax" => {
                self.set_window(Some(0.0), Some(1.0));
                true
            }
            "p99" => {
                self.set_window(Some(0.01), Some(0.99));
                true
            }
            _ => false,
        }
    }

    /// Show or hide the WCS axis captions overlay.
    pub fn set_captions_visible(&self, on: bool) {
        self.captions_toggle.set_active(on);
    }

    /// Show or hide the slice-plane marker in the volume.
    pub fn set_slice_plane_visible(&self, on: bool) {
        self.slice_toggle.set_active(on);
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// The 2D slice view, which owns the on-screen spectrum panel.
    pub fn slice(&self) -> &Rc<CubeSliceView> {
        &self.slice
    }

    /// The cube's spectral WCS, for world-value labels.
    pub fn wcs(&self) -> &Rc<CubeWcs> {
        &self.wcs
    }

    /// Mean of each resident channel — the scrubber's waveform, and what
    /// `get_cube_channel_profile` reports. Blank voxels are excluded from the
    /// mean rather than counted as zero; a wholly blank channel yields `None`.
    pub fn channel_profile(&self) -> Vec<(usize, Option<f64>)> {
        let (_, _, native_z) = self.native_dims();
        (0..self.vol.nz)
            .map(|z| {
                let mut sum = 0.0f64;
                let mut n = 0usize;
                for y in 0..self.vol.ny {
                    for x in 0..self.vol.nx {
                        let v = self.vol.sample(x, y, z);
                        if !v.is_nan() {
                            sum += v as f64;
                            n += 1;
                        }
                    }
                }
                let native_channel = resident_to_native(z, self.vol.nz, native_z);
                (native_channel, (n > 0).then(|| sum / n as f64))
            })
            .collect()
    }

    /// Render the currently displayed view (3D volume or 2D slice) to straight
    /// RGBA8 (`w*h*4`, top-down) for figure export. `transparent` clears the 3D
    /// background to alpha 0. `None` when nothing can be captured.
    pub fn render_figure(&self, w: i32, h: i32, transparent: bool) -> Option<Vec<u8>> {
        let is_3d = self
            .stack
            .visible_child_name()
            .is_none_or(|n| n == "volume");
        if is_3d {
            self.gl.render_to_rgba(w, h, transparent)
        } else {
            let (sw, sh, rgba) = self.slice.export_rgba();
            Some(scale_rgba(&rgba, sw, sh, w, h))
        }
    }

    /// One channel for the whole viewer: the scrubber moves the 2D plane and the
    /// volume's slice-plane marker together.
    fn wire_channel(self: &Rc<Self>) {
        let this = self.clone();
        self.slice.set_on_channel_changed(move |ch| {
            this.current_channel.set(ch);
            this.overlay_area.queue_draw();
        });
        // Seed both from the scrubber's own starting position, rather than
        // seeding each separately and hoping they agree.
        self.current_channel.set(self.slice.channel());
    }

    // ── Mode switching ───────────────────────────────────────────────────────

    fn wire_mode_toggle(self: &Rc<Self>) {
        let this = self.clone();
        self.mode_slice.connect_toggled(move |btn| {
            let slice = btn.is_active();
            this.stack
                .set_visible_child_name(if slice { "slice" } else { "volume" });
            // The VOLUME control group only applies to the 3D ray-march.
            this.volume_section.set_visible(!slice);
        });
    }

    /// Poll until the GL context realizes: keep 3D when it succeeds, otherwise
    /// force Slice-only and disable the 3D toggle (headless / llvmpipe fallback).
    fn start_gl_probe(self: &Rc<Self>) {
        let this = self.clone();
        glib::timeout_add_local(Duration::from_millis(200), move || {
            if this.gl.is_available() {
                return glib::ControlFlow::Break;
            }
            let realized = this.gl.widget().is_realized();
            let n = this.probe_tries.get() + 1;
            this.probe_tries.set(n);
            // Realized-but-not-available means shader/context init failed; also
            // give up after ~5 s in case the volume view is never shown.
            if (realized && !this.gl.is_available()) || n > 25 {
                this.force_slice_only();
                return glib::ControlFlow::Break;
            }
            glib::ControlFlow::Continue
        });
    }

    fn force_slice_only(&self) {
        self.mode_slice.set_active(true);
        self.stack.set_visible_child_name("slice");
        self.volume_section.set_visible(false);
        self.mode_3d.set_sensitive(false);
    }

    // ── Shared display controls → BOTH renderers ────────────────────────────

    fn apply_window(&self, ctl: &Controls) {
        let a = ctl.window_lo.value() as f32;
        let b = ctl.window_hi.value() as f32;
        let (lo, hi) = (a.min(b), a.max(b));
        self.gl.set_window(lo, hi);
        self.slice.set_window(lo, hi);
        // The window drives which physical values the colorbar endpoints map to.
        self.refresh_colorbar();
    }

    fn wire_controls(self: &Rc<Self>, ctl: &Controls) {
        // Colormap.
        {
            let this = self.clone();
            ctl.colormap.connect_selected_notify(move |d| {
                if let Some(name) = cube_colormaps::NAMES.get(d.selected() as usize) {
                    this.gl.set_colormap(name);
                    this.slice.set_colormap(name);
                    this.refresh_colorbar();
                }
            });
        }
        // Stretch.
        {
            let this = self.clone();
            ctl.stretch.connect_selected_notify(move |d| {
                let m = StretchMode::from_index(d.selected() as usize);
                this.gl.set_stretch(m);
                this.slice.set_stretch(m);
            });
        }
        // Window low / high.
        {
            let this = self.clone();
            let ctl2 = ctl.clone();
            ctl.window_lo
                .connect_value_changed(move |_| this.apply_window(&ctl2));
        }
        {
            let this = self.clone();
            let ctl2 = ctl.clone();
            ctl.window_hi
                .connect_value_changed(move |_| this.apply_window(&ctl2));
        }
        // Density (volume only).
        {
            let this = self.clone();
            ctl.density
                .connect_value_changed(move |s| this.gl.set_density(s.value() as f32));
        }
        // Quality (steps, volume only).
        {
            let this = self.clone();
            ctl.steps
                .connect_value_changed(move |s| this.gl.set_steps(s.value() as f32));
        }
        // MIP (max-intensity projection).
        {
            let this = self.clone();
            ctl.mip
                .connect_toggled(move |b| this.gl.set_mip(b.is_active()));
        }
        // Idle auto-orbit.
        {
            let this = self.clone();
            ctl.auto_orbit
                .connect_toggled(move |b| this.gl.set_auto_orbit(b.is_active()));
        }
        // Transfer-function reset.
        {
            let this = self.clone();
            ctl.transfer_reset.connect_clicked(move |_| {
                this.transfer.borrow_mut().reset();
                this.apply_transfer();
                this.transfer_area.queue_draw();
            });
        }
        // Spectral (Z) scale (volume only): stretches the box + moves the overlay.
        {
            let this = self.clone();
            ctl.spectral.connect_value_changed(move |s| {
                this.gl.set_spectral_scale(s.value() as f32);
                this.overlay_area.queue_draw();
            });
        }
        // Background preset (Dark / Black / Light).
        {
            let this = self.clone();
            ctl.background.connect_selected_notify(move |d| {
                let rgb = match d.selected() {
                    1 => [0.0, 0.0, 0.0],    // Black
                    2 => [0.92, 0.92, 0.94], // Light
                    _ => [0.06, 0.06, 0.08], // Dark
                };
                this.gl.set_background(rgb);
            });
        }
        // Reset view.
        {
            let this = self.clone();
            ctl.reset_view
                .connect_clicked(move |_| this.gl.reset_view());
        }
        // Window 99%: snap the display cut to the 1..99% window.
        {
            let ctl2 = ctl.clone();
            ctl.window_99.connect_clicked(move |_| {
                // Setting the sliders fires apply_window (→ gl + slice + colorbar).
                ctl2.window_lo.set_value(0.01);
                ctl2.window_hi.set_value(0.99);
            });
        }
        // Axis-captions + slice-plane overlay toggles.
        {
            let this = self.clone();
            ctl.captions_toggle
                .connect_toggled(move |_| this.overlay_area.queue_draw());
        }
        {
            let this = self.clone();
            ctl.slice_toggle
                .connect_toggled(move |_| this.overlay_area.queue_draw());
        }
        // Export the composed figure (3D snapshot or current slice) → PNG / PDF.
        {
            let this = self.clone();
            ctl.export.connect_clicked(move |_| this.show_export());
        }
    }

    // ── Wireframe box + WCS caption overlay ──────────────────────────────────

    /// Draw the projected box edges, WCS axis captions, and slice-plane marker on
    /// the transparent overlay, and track the camera so it stays aligned.
    fn setup_overlay(self: &Rc<Self>) {
        let this = self.clone();
        self.overlay_area.set_draw_func(move |_area, cr, w, h| {
            if w < 1 || h < 1 {
                return;
            }
            let vp = this.gl.view_proj(w, h);
            let overlay = cube_axes::build(&cube_axes::AxesRequest {
                dims: (this.vol.nx, this.vol.ny, this.vol.nz),
                wcs: &this.wcs,
                view_proj: &vp,
                panel: (w as f32, h as f32),
                slice_z: this.current_channel.get(),
                spectral_scale: this.gl.spectral_scale(),
            });

            // Slice-plane marker (behind the edges): translucent cyan fill + edge.
            if this.slice_toggle.is_active() && overlay.slice_quad.len() == 4 {
                let q = &overlay.slice_quad;
                cr.move_to(q[0].0 as f64, q[0].1 as f64);
                for p in &q[1..] {
                    cr.line_to(p.0 as f64, p.1 as f64);
                }
                cr.close_path();
                cr.set_source_rgba(0.34, 0.78, 1.0, 0.16);
                cr.fill_preserve().ok();
                cr.set_source_rgba(0.34, 0.78, 1.0, 0.70);
                cr.set_line_width(1.5);
                cr.stroke().ok();
            }

            // Box wireframe: thin faint cool-blue lines.
            cr.set_source_rgba(0.62, 0.77, 0.91, 0.40);
            cr.set_line_width(1.0);
            for (a, b) in &overlay.edges {
                cr.move_to(a.0 as f64, a.1 as f64);
                cr.line_to(b.0 as f64, b.1 as f64);
            }
            cr.stroke().ok();

            // WCS axis captions: small monospaced text, centered on their points,
            // drawn with a 1px shadow for legibility over the volume.
            if this.captions_toggle.is_active() {
                cr.select_font_face(
                    "monospace",
                    gtk4::cairo::FontSlant::Normal,
                    gtk4::cairo::FontWeight::Normal,
                );
                cr.set_font_size(11.0);
                for (x, y, text) in &overlay.captions {
                    let (mut cx, cy) = (*x as f64, *y as f64);
                    if let Ok(ext) = cr.text_extents(text) {
                        cx -= ext.width() / 2.0;
                    }
                    let cx = cx.clamp(2.0, (w as f64 - 2.0).max(2.0));
                    let cy = cy.clamp(11.0, (h as f64 - 2.0).max(11.0));
                    cr.set_source_rgba(0.0, 0.0, 0.0, 0.70);
                    cr.move_to(cx + 1.0, cy + 1.0);
                    cr.show_text(text).ok();
                    cr.set_source_rgba(0.90, 0.95, 1.0, 0.96);
                    cr.move_to(cx, cy);
                    cr.show_text(text).ok();
                }
            }
        });

        // Track the camera: any orbit/zoom/auto-orbit move repaints the overlay.
        let area = self.overlay_area.clone();
        self.gl.set_on_camera_changed(move || area.queue_draw());
    }

    // ── Live colorbar ─────────────────────────────────────────────────────────

    /// Paint the active colormap gradient across the colorbar strip.
    fn setup_colorbar(self: &Rc<Self>) {
        let dropdown = self.colormap.clone();
        self.colorbar_area.set_content_height(16);
        self.colorbar_area.set_draw_func(move |_area, cr, w, h| {
            if w < 1 || h < 1 {
                return;
            }
            let name = cube_colormaps::NAMES
                .get(dropdown.selected() as usize)
                .copied()
                .unwrap_or(cube_colormaps::DEFAULT);
            let lut = cube_colormaps::lut_rgba(name); // 256 * 4 RGBA
            let denom = (w - 1).max(1) as f64;
            for i in 0..w {
                let t = i as f64 / denom;
                let o = ((t * 255.0).round() as usize).min(255) * 4;
                cr.set_source_rgb(
                    lut[o] as f64 / 255.0,
                    lut[o + 1] as f64 / 255.0,
                    lut[o + 2] as f64 / 255.0,
                );
                cr.rectangle(i as f64, 0.0, 1.0, h as f64);
                cr.fill().ok();
            }
        });
    }

    /// Physical value labels for the colorbar endpoints (window low / high mapped
    /// through the display cut), suffixed with the unit. Empty without metadata.
    fn colorbar_labels(&self) -> (String, String) {
        match self.vol.meta.as_ref() {
            Some(m) => {
                let unit = m.bunit.as_deref().unwrap_or("");
                let lo = m.value_at_normalized(self.window_lo.value());
                let hi = m.value_at_normalized(self.window_hi.value());
                (label_with_unit(lo, unit), label_with_unit(hi, unit))
            }
            None => (String::new(), String::new()),
        }
    }

    /// Repaint the gradient and refresh the endpoint value labels.
    fn refresh_colorbar(&self) {
        self.colorbar_area.queue_draw();
        let (lo, hi) = self.colorbar_labels();
        self.colorbar_lo.set_text(&lo);
        self.colorbar_hi.set_text(&hi);
    }

    // ── Export ────────────────────────────────────────────────────────────────

    /// Snapshot the currently displayed view (3D volume or 2D slice) as straight
    /// RGBA at the requested size — the capture callback the export plate rasterizes.
    fn capture_plate(&self, w: i32, h: i32) -> Option<Vec<u8>> {
        let is_3d = self
            .stack
            .visible_child_name()
            .is_none_or(|n| n == "volume");
        if is_3d {
            self.gl.render_to_rgba(w, h, true)
        } else {
            // The slice view renders the current channel through the shared
            // window/stretch/colormap; scale it to the requested plate size.
            let (sw, sh, rgba) = self.slice.export_rgba();
            Some(scale_rgba(&rgba, sw, sh, w, h))
        }
    }

    /// A WCS caption for the export plate: cube name + current channel label.
    fn wcs_caption(&self) -> String {
        let label = self.wcs.channel_label(self.current_channel.get());
        if label.is_empty() {
            self.name.clone()
        } else {
            format!("{} · {}", self.name, label)
        }
    }

    fn show_export(self: &Rc<Self>) {
        let this = self.clone();
        let capture: Rc<dyn Fn(i32, i32) -> Option<Vec<u8>>> =
            Rc::new(move |w, h| this.capture_plate(w, h));
        let colormap = cube_colormaps::NAMES
            .get(self.colormap.selected() as usize)
            .copied()
            .unwrap_or(cube_colormaps::DEFAULT)
            .to_string();
        let (lo_label, hi_label) = self.colorbar_labels();

        // Live box + caption overlay + metadata footer inputs (mirrors Windows
        // CubeViewerPage.BuildPlateData). Captions render only over the 3D volume;
        // the export overlay shares the GL camera via `view_proj`, so it aligns
        // with the captured render at any plate scale.
        let is_3d = self
            .stack
            .visible_child_name()
            .is_none_or(|n| n == "volume");
        let gl = self.gl.clone();
        let overlay = crate::ui::cube_export::PlateOverlay {
            captions_on: is_3d && self.captions_toggle.is_active(),
            wcs: self.wcs.clone(),
            view_proj: Rc::new(move |w, h| gl.view_proj(w, h)),
            nx: self.vol.nx,
            ny: self.vol.ny,
            nz: self.vol.nz,
            spectral_scale: self.gl.spectral_scale(),
            meta: self.vol.meta.clone(),
        };

        crate::ui::cube_export::show_cube_export(
            &self.widget,
            capture,
            self.name.clone(),
            self.wcs_caption(),
            colormap,
            lo_label,
            hi_label,
            overlay,
        );
    }

    // ── Opacity transfer-function editor ─────────────────────────────────────

    fn apply_transfer(&self) {
        self.gl.set_transfer_ramp(self.transfer.borrow().ramp());
    }

    /// Canvas position → normalized (value, alpha), alpha increasing upward.
    fn tf_norm(&self, x: f64, y: f64) -> Option<(f32, f32)> {
        let w = self.transfer_area.width() as f64;
        let h = self.transfer_area.height() as f64;
        if w <= 0.0 || h <= 0.0 {
            return None;
        }
        Some((
            (x / w).clamp(0.0, 1.0) as f32,
            (1.0 - y / h).clamp(0.0, 1.0) as f32,
        ))
    }

    /// The 9 px hit radius expressed in normalized units (canvas isn't square).
    fn tf_radius(&self) -> f32 {
        let w = (self.transfer_area.width() as f32).max(1.0);
        let h = (self.transfer_area.height() as f32).max(1.0);
        (9.0 / w).max(9.0 / h)
    }

    fn setup_transfer_editor(self: &Rc<Self>) {
        self.transfer_area.set_content_height(110);
        self.transfer_area.set_hexpand(true);

        // Draw.
        {
            let this = self.clone();
            self.transfer_area.set_draw_func(move |_area, cr, w, h| {
                let (wf, hf) = (w as f64, h as f64);
                cr.set_source_rgba(0.0, 0.0, 0.0, 0.19);
                let _ = cr.paint();
                let m = this.transfer.borrow();
                if m.points.is_empty() {
                    return;
                }
                let mut order: Vec<usize> = (0..m.points.len()).collect();
                order.sort_by(|&a, &b| m.points[a].0.total_cmp(&m.points[b].0));
                let view =
                    |i: usize| (m.points[i].0 as f64 * wf, hf * (1.0 - m.points[i].1 as f64));

                // Filled area under the curve (translucent cyan).
                cr.set_source_rgba(0.34, 0.78, 1.0, 0.19);
                let (x0, _) = view(order[0]);
                cr.move_to(x0, hf);
                for &i in &order {
                    let (x, y) = view(i);
                    cr.line_to(x, y);
                }
                let (xl, _) = view(order[order.len() - 1]);
                cr.line_to(xl, hf);
                cr.close_path();
                cr.fill().ok();

                // Curve line.
                cr.set_source_rgba(0.34, 0.78, 1.0, 1.0);
                cr.set_line_width(1.5);
                for (k, &i) in order.iter().enumerate() {
                    let (x, y) = view(i);
                    if k == 0 {
                        cr.move_to(x, y);
                    } else {
                        cr.line_to(x, y);
                    }
                }
                cr.stroke().ok();

                // Handles (endpoints outlined — pinned in value, can't be removed).
                for i in 0..m.points.len() {
                    let (x, y) = view(i);
                    cr.set_source_rgba(0.34, 0.78, 1.0, 1.0);
                    cr.arc(x, y, 5.0, 0.0, std::f64::consts::TAU);
                    cr.fill().ok();
                    if m.is_endpoint(i) {
                        cr.set_source_rgba(0.94, 0.94, 0.94, 1.0);
                        cr.set_line_width(1.5);
                        cr.arc(x, y, 5.0, 0.0, std::f64::consts::TAU);
                        cr.stroke().ok();
                    }
                }
            });
        }

        // Redraw on resize (normalized coordinates depend on the pixel size).
        {
            let area = self.transfer_area.clone();
            self.transfer_area
                .connect_resize(move |_, _, _| area.queue_draw());
        }

        // Drag: grab a point, or add one under the cursor and drag it live.
        let drag = gtk::GestureDrag::new();
        drag.set_button(1);
        {
            let this = self.clone();
            drag.connect_drag_begin(move |_, x, y| {
                this.tf_start.set((x, y));
                let Some((nx, ny)) = this.tf_norm(x, y) else {
                    this.tf_drag.set(-1);
                    return;
                };
                let r = this.tf_radius();
                let idx = {
                    let mut m = this.transfer.borrow_mut();
                    match m.hit_test(nx, ny, r) {
                        Some(hit) => hit as i32,
                        None => {
                            m.add(nx, ny);
                            (m.points.len() - 1) as i32
                        }
                    }
                };
                this.tf_drag.set(idx);
                this.apply_transfer();
                this.transfer_area.queue_draw();
            });
        }
        {
            let this = self.clone();
            drag.connect_drag_update(move |_, dx, dy| {
                let idx = this.tf_drag.get();
                if idx < 0 {
                    return;
                }
                let (sx, sy) = this.tf_start.get();
                let Some((nx, ny)) = this.tf_norm(sx + dx, sy + dy) else {
                    return;
                };
                this.transfer.borrow_mut().drag(idx as usize, nx, ny);
                this.apply_transfer();
                this.transfer_area.queue_draw();
            });
        }
        {
            let this = self.clone();
            drag.connect_drag_end(move |_, _, _| this.tf_drag.set(-1));
        }
        self.transfer_area.add_controller(drag);

        // Right-click removes an interior point.
        let remove = gtk::GestureClick::new();
        remove.set_button(3);
        {
            let this = self.clone();
            remove.connect_pressed(move |_, _, x, y| {
                let Some((nx, ny)) = this.tf_norm(x, y) else {
                    return;
                };
                let r = this.tf_radius();
                let removed = {
                    let mut m = this.transfer.borrow_mut();
                    match m.hit_test(nx, ny, r) {
                        Some(hit) => m.remove(hit),
                        None => false,
                    }
                };
                if removed {
                    this.apply_transfer();
                    this.transfer_area.queue_draw();
                }
            });
        }
        self.transfer_area.add_controller(remove);
    }
}

// ── Control column construction ─────────────────────────────────────────────

/// Handles to every interactive control in the right-hand column.
#[derive(Clone)]
struct Controls {
    scroll: gtk::ScrolledWindow,
    colormap: gtk::DropDown,
    colorbar_area: gtk::DrawingArea,
    colorbar_lo: gtk::Label,
    colorbar_hi: gtk::Label,
    stretch: gtk::DropDown,
    window_lo: gtk::Scale,
    window_hi: gtk::Scale,
    window_99: gtk::Button,
    background: gtk::DropDown,
    density: gtk::Scale,
    spectral: gtk::Scale,
    steps: gtk::Scale,
    mip: gtk::ToggleButton,
    auto_orbit: gtk::ToggleButton,
    captions_toggle: gtk::CheckButton,
    slice_toggle: gtk::CheckButton,
    reset_view: gtk::Button,
    volume_section: gtk::Box,
    transfer_area: gtk::DrawingArea,
    transfer_reset: gtk::Button,
    info_grid: gtk::Grid,
    export: gtk::Button,
}

fn build_controls(_name: &str) -> (Controls, Controls) {
    let (column, scroll) = viewer_shell::control_column();

    // ── DISPLAY ─────────────────────────────────────────────────────────────
    column.append(&viewer_shell::section_header(crate::tr_en!("DISPLAY")));

    let colormap = gtk::DropDown::from_strings(cube_colormaps::NAMES);
    let cmap_default = cube_colormaps::NAMES
        .iter()
        .position(|&n| n == cube_colormaps::DEFAULT)
        .unwrap_or(0);
    colormap.set_selected(cmap_default as u32);
    column.append(&labeled(crate::tr_en!("Colormap"), &colormap));

    // Live colorbar: the active colormap gradient with physical endpoint labels.
    let colorbar_area = gtk::DrawingArea::new();
    colorbar_area.add_css_class("card");
    colorbar_area.set_content_height(16);
    colorbar_area.set_hexpand(true);
    let colorbar_lo = gtk::Label::new(None);
    colorbar_lo.add_css_class("caption");
    colorbar_lo.add_css_class("dim-label");
    colorbar_lo.set_halign(gtk::Align::Start);
    colorbar_lo.set_hexpand(true);
    let colorbar_hi = gtk::Label::new(None);
    colorbar_hi.add_css_class("caption");
    colorbar_hi.add_css_class("dim-label");
    colorbar_hi.set_halign(gtk::Align::End);
    let colorbar_labels = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    colorbar_labels.append(&colorbar_lo);
    colorbar_labels.append(&colorbar_hi);
    let colorbar_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
    colorbar_box.append(&colorbar_area);
    colorbar_box.append(&colorbar_labels);
    column.append(&colorbar_box);

    let stretch = gtk::DropDown::from_strings(STRETCH_NAMES);
    stretch.set_selected(0);
    column.append(&labeled(crate::tr_en!("Stretch"), &stretch));

    let window_lo = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 0.005);
    window_lo.set_value(0.0);
    window_lo.set_draw_value(false);
    window_lo.set_hexpand(true);
    column.append(&labeled(crate::tr_en!("Window low"), &window_lo));

    let window_hi = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 0.005);
    window_hi.set_value(1.0);
    window_hi.set_draw_value(false);
    window_hi.set_hexpand(true);
    column.append(&labeled(crate::tr_en!("Window high"), &window_hi));

    let window_99 = gtk::Button::with_label(crate::tr_en!("Window 99%"));
    window_99.add_css_class("flat");
    window_99.set_halign(gtk::Align::End);
    window_99.set_tooltip_text(Some(crate::tr_en!(
        "Set the display cut to the 1–99% window"
    )));
    column.append(&window_99);

    let background = gtk::DropDown::from_strings(&[
        crate::tr_en!("Dark"),
        crate::tr_en!("Black"),
        crate::tr_en!("Light"),
    ]);
    background.set_selected(0);
    column.append(&labeled(crate::tr_en!("Background"), &background));

    // ── VOLUME (hidden in Slice mode) ───────────────────────────────────────
    let volume_section = gtk::Box::new(gtk::Orientation::Vertical, 12);
    volume_section.append(&viewer_shell::section_header(crate::tr_en!("VOLUME")));

    let density = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.1, 3.0, 0.05);
    density.set_value(1.0);
    density.set_draw_value(false);
    density.set_hexpand(true);
    volume_section.append(&labeled(crate::tr_en!("Density"), &density));

    let spectral = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.5, 4.0, 0.05);
    spectral.set_value(1.5);
    spectral.set_draw_value(false);
    spectral.set_hexpand(true);
    volume_section.append(&labeled(crate::tr_en!("Spectral scale"), &spectral));

    let steps = gtk::Scale::with_range(gtk::Orientation::Horizontal, 96.0, 768.0, 16.0);
    steps.set_value(512.0);
    steps.set_draw_value(false);
    steps.set_hexpand(true);
    volume_section.append(&labeled(crate::tr_en!("Quality (steps)"), &steps));

    let toggles = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let mip = gtk::ToggleButton::with_label(crate::tr_en!("MIP"));
    mip.set_tooltip_text(Some(crate::tr_en!("Max-intensity projection")));
    mip.set_hexpand(true);
    let auto_orbit = gtk::ToggleButton::with_label(crate::tr_en!("Auto-orbit"));
    auto_orbit.set_tooltip_text(Some(crate::tr_en!("Idle auto-orbit")));
    auto_orbit.set_hexpand(true);
    toggles.append(&mip);
    toggles.append(&auto_orbit);
    volume_section.append(&toggles);

    // Overlay toggles (default on, matching the reference ToggleSwitches).
    let captions_toggle = gtk::CheckButton::with_label(crate::tr_en!("Axis captions"));
    captions_toggle.set_active(true);
    volume_section.append(&captions_toggle);
    let slice_toggle = gtk::CheckButton::with_label(crate::tr_en!("Slice plane"));
    slice_toggle.set_active(true);
    volume_section.append(&slice_toggle);

    // Opacity transfer-function editor.
    let tf_header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let tf_label = gtk::Label::new(Some(crate::tr_en!("Opacity curve")));
    tf_label.add_css_class("caption");
    tf_label.set_halign(gtk::Align::Start);
    tf_label.set_hexpand(true);
    tf_header.append(&tf_label);
    let transfer_reset = gtk::Button::with_label(crate::tr_en!("Reset"));
    transfer_reset.add_css_class("flat");
    tf_header.append(&transfer_reset);
    volume_section.append(&tf_header);

    let transfer_area = gtk::DrawingArea::new();
    transfer_area.add_css_class("card");
    volume_section.append(&transfer_area);

    let tf_hint = gtk::Label::new(Some(crate::tr_en!(
        "Drag points · click adds · right-click removes"
    )));
    tf_hint.add_css_class("caption");
    tf_hint.add_css_class("dim-label");
    tf_hint.set_halign(gtk::Align::Start);
    tf_hint.set_wrap(true);
    volume_section.append(&tf_hint);

    let reset_view = gtk::Button::with_label(crate::tr_en!("Reset view"));
    reset_view.set_hexpand(true);
    volume_section.append(&reset_view);

    column.append(&volume_section);

    // ── Info expander ───────────────────────────────────────────────────────
    let info_grid = gtk::Grid::new();
    info_grid.set_row_spacing(3);
    info_grid.set_column_spacing(10);
    let info_expander = gtk::Expander::new(Some(crate::tr_en!("Info")));
    info_expander.set_child(Some(&info_grid));
    info_expander.set_expanded(true);
    column.append(&info_expander);

    // ── Export ──────────────────────────────────────────────────────────────
    let export = gtk::Button::with_label(crate::tr_en!("Export…"));
    export.add_css_class("suggested-action");
    export.set_hexpand(true);
    column.append(&export);

    let controls = Controls {
        scroll,
        colormap,
        colorbar_area,
        colorbar_lo,
        colorbar_hi,
        stretch,
        window_lo,
        window_hi,
        window_99,
        background,
        density,
        spectral,
        steps,
        mip,
        auto_orbit,
        captions_toggle,
        slice_toggle,
        reset_view,
        volume_section,
        transfer_area,
        transfer_reset,
        info_grid,
        export,
    };
    (controls.clone(), controls)
}

/// Populate the Info grid: dimensions, spectral axis, object / instrument, unit
/// and physical value range.
fn fill_info(grid: &gtk::Grid, vol: &VolumeData, wcs: &CubeWcs, name: &str) {
    let mut row = 0i32;
    let mut add = |grid: &gtk::Grid, k: &str, v: &str| {
        if v.is_empty() {
            return;
        }
        let key = gtk::Label::new(Some(k));
        key.add_css_class("caption");
        key.add_css_class("dim-label");
        key.set_halign(gtk::Align::Start);
        key.set_yalign(0.0);
        let val = gtk::Label::new(Some(v));
        val.add_css_class("caption");
        val.set_halign(gtk::Align::Start);
        val.set_xalign(0.0);
        val.set_wrap(true);
        val.set_hexpand(true);
        grid.attach(&key, 0, row, 1, 1);
        grid.attach(&val, 1, row, 1, 1);
        row += 1;
    };

    add(grid, crate::tr_en!("NAME"), name);
    add(
        grid,
        crate::tr_en!("DIMENSIONS"),
        &format!("{} × {} × {}", vol.nx, vol.ny, vol.nz),
    );

    // Spectral axis: name + physical channel range.
    let axis = wcs.spec_axis_name();
    let spectral = if wcs.has_spectral() && vol.nz > 1 {
        format!(
            "{} ({} → {})",
            axis,
            wcs.channel_label(0),
            wcs.channel_label(vol.nz - 1)
        )
    } else {
        axis
    };
    add(grid, crate::tr_en!("SPECTRAL"), &spectral);

    if let Some(m) = vol.meta.as_ref() {
        add(
            grid,
            crate::tr_en!("OBJECT"),
            m.object.as_deref().unwrap_or(""),
        );
        add(
            grid,
            crate::tr_en!("INSTRUMENT"),
            m.instrument.as_deref().unwrap_or(""),
        );
        add(
            grid,
            crate::tr_en!("TELESCOPE"),
            m.telescope.as_deref().unwrap_or(""),
        );
        let unit = m.bunit.as_deref().unwrap_or("");
        add(grid, crate::tr_en!("UNIT"), unit);
        // RANGE = the display cut (p0.5…p99.5); MIN/MAX = true full-cube extremes.
        let unit_suffix = if unit.is_empty() {
            String::new()
        } else {
            format!(" {}", unit)
        };
        add(
            grid,
            crate::tr_en!("RANGE"),
            &format!(
                "{} → {}{}",
                fmt_num(m.norm_lo),
                fmt_num(m.norm_hi),
                unit_suffix
            ),
        );
        add(
            grid,
            crate::tr_en!("MIN / MAX"),
            &format!("{} / {}", fmt_num(m.data_min), fmt_num(m.data_max)),
        );
        add(
            grid,
            crate::tr_en!("MEDIAN"),
            &label_with_unit(m.median, unit),
        );
        add(
            grid,
            crate::tr_en!("NaN"),
            &format!("{:.2}%", m.nan_fraction * 100.0),
        );
        add(grid, crate::tr_en!("MODE"), &m.mode_text());
    }
}

/// Compact numeric formatter (~4 significant figures, trailing zeros trimmed,
/// scientific notation for very large/small magnitudes). Mirrors the reference's
/// `G4` display for info-panel and colorbar values.
fn fmt_num(v: f64) -> String {
    if !v.is_finite() {
        return "—".to_string();
    }
    let a = v.abs();
    if a != 0.0 && !(1e-3..1e5).contains(&a) {
        return format!("{:.3e}", v);
    }
    let s = format!("{:.4}", v);
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    }
}

/// A physical value formatted with its unit suffix (`"1.42 Jy/beam"`), or bare
/// when there is no unit.
fn label_with_unit(v: f64, unit: &str) -> String {
    if unit.is_empty() {
        fmt_num(v)
    } else {
        format!("{} {}", fmt_num(v), unit)
    }
}

/// Nearest-neighbor resample of a straight-RGBA8 buffer from `sw×sh` to `dw×dh`.
fn scale_rgba(src: &[u8], sw: i32, sh: i32, dw: i32, dh: i32) -> Vec<u8> {
    let (sw, sh) = (sw.max(1) as usize, sh.max(1) as usize);
    let (dw, dh) = (dw.max(1) as usize, dh.max(1) as usize);
    let mut out = vec![0u8; dw * dh * 4];
    if src.len() < sw * sh * 4 {
        return out;
    }
    for y in 0..dh {
        let sy = (y * sh / dh).min(sh - 1);
        for x in 0..dw {
            let sx = (x * sw / dw).min(sw - 1);
            let so = (sy * sw + sx) * 4;
            let do_ = (y * dw + x) * 4;
            out[do_..do_ + 4].copy_from_slice(&src[so..so + 4]);
        }
    }
    out
}

#[cfg(test)]
mod channel_wiring_tests {
    //! The channel is ONE value, and its control belongs to both modes.
    //!
    //! It used to be two: this page's `current_channel`, which positions the
    //! slice-plane marker in the volume, and the slice view's own scrubber
    //! position. Nothing connected them, and the only control was inside the
    //! slice view — so in 3D the marker sat wherever it had been seeded and
    //! there was no way on screen to move it. The reference is explicit that the
    //! scrubber "lives in BOTH modes: in slice mode it shows the 2D plane, in
    //! volume mode it drives the slice-plane marker".
    //!
    //! A source scan, because the wiring is between GTK widgets a unit test
    //! cannot build.

    const SOURCE: &str = include_str!("cube_viewer.rs");

    #[test]
    fn the_channel_bar_lives_outside_the_mode_stack() {
        // Inside the stack it is a slice-mode control; outside it, it is the
        // viewer's control.
        //
        // Tests stripped: the first version of this guard passed against a
        // deliberately broken layout by finding its own assertion text.
        let code = crate::testing::code(SOURCE);
        let bar_at = code
            .find("left.append(slice.channel_bar());")
            .unwrap_or_else(|| {
                panic!(
                    "the channel scrubber must be placed under the mode stack, or \
                 it disappears in 3D"
                )
            });
        let stack_at = code
            .find("stack.add_named(slice.widget()")
            .expect("the slice view is a stack child");
        assert!(
            bar_at > stack_at,
            "the bar should be added after the stack, as a sibling of it"
        );
    }

    #[test]
    fn scrubbing_moves_the_slice_plane_marker() {
        // The page subscribes to the scrubber and updates the channel the
        // wireframe overlay draws at.
        let wiring = SOURCE
            .split("fn wire_channel")
            .nth(1)
            .expect("the channel wiring lives here");
        assert!(wiring.contains("set_on_channel_changed"));
        assert!(
            wiring.contains("current_channel.set(ch)"),
            "a channel change must move the marker's channel"
        );
        assert!(
            wiring.contains("overlay_area.queue_draw()"),
            "and repaint the overlay that draws it"
        );
    }

    #[test]
    fn an_agents_channel_change_moves_the_visible_control() {
        // set_cube_view { channel } drives the scrubber rather than only the
        // internal value, so what an agent sets is what the slider shows and
        // what get_cube_view reports.
        let setter = SOURCE
            .split("pub fn set_current_channel")
            .nth(1)
            .expect("the setter exists");
        assert!(
            setter.contains("slice.set_channel_from(ch)"),
            "an agent's channel change should move the on-screen scrubber"
        );
    }
}
