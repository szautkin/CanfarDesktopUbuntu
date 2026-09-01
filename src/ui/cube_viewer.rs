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
    /// The file this cube was loaded from — what its marks are filed under.
    ///
    /// Held here rather than looked up from the view payload: the cube's
    /// `view_json` has no `path` key, so `get("path")` was `None` on every
    /// call and every cube's marks were saved under `""`. The FITS side had
    /// exactly this bug; ask the viewer, which knows.
    source_path: RefCell<String>,
    /// Marks drawn on this cube, by the user or an agent.
    annotations: RefCell<Vec<crate::models::annotation::Annotation>>,
    selected_annotation: RefCell<Option<String>>,
    /// Hosts the label editor, over whichever view is showing.
    view_overlay: gtk::Overlay,
    /// The editor currently on the view, and the mark it belongs to.
    open_label_editor: RefCell<Option<(String, gtk::Box)>>,
    /// The shape a placing drag in the VOLUME is about to create: `(voxel x,
    /// voxel y, half in voxels)` on the current channel. The slice keeps its
    /// own; a drag happens in one view at a time, and a shape being born
    /// belongs to the drag making it.
    pending_shape: RefCell<Option<(f64, f64, f64)>>,
    /// The sidebar Marks section — list, pencil and shape picker.
    marks_section: Rc<crate::ui::annotations_panel::MarksSection>,
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

        // An overlay over the WHOLE stack, not over one view: a mark can be
        // named in either mode, and the label editor is the same widget in
        // both. Wrapping each view separately would mean two hosts and two
        // ways for the editor to be positioned.
        let view_overlay = gtk::Overlay::new();
        view_overlay.set_hexpand(true);
        view_overlay.set_vexpand(true);
        view_overlay.set_child(Some(&stack));
        left.append(&view_overlay);

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
            source_path: RefCell::new(String::new()),
            annotations: RefCell::new(Vec::new()),
            selected_annotation: RefCell::new(None),
            view_overlay,
            open_label_editor: RefCell::new(None),
            pending_shape: RefCell::new(None),
            marks_section: ctl.marks_section.clone(),
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
        *self.source_path.borrow_mut() = path.display().to_string();
        if let Some(src) = crate::helpers::cube_native_slice::NativeSliceSource::try_open(path) {
            self.slice.set_native_source(src);
        }
        // Marks saved from a previous session come back with the file. The
        // store was write-only on this side: `save_for` on every change and
        // `load_for` nowhere, so nothing a user or an agent drew on a cube
        // survived closing it.
        let saved = crate::helpers::annotation_store::load_for(&self.source_path.borrow());
        if !saved.is_empty() {
            self.set_annotations(saved);
        }
    }

    /// The file this cube's marks are filed under. Empty before a path is set.
    pub fn source_file(&self) -> String {
        self.source_path.borrow().clone()
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
        let base = if is_3d {
            self.gl.render_to_rgba(w, h, transparent)?
        } else {
            let (sw, sh, rgba) = self.slice.export_rgba();
            scale_rgba(&rgba, sw, sh, w, h)
        };
        Some(self.draw_marks_on_plate(w, h, base, is_3d))
    }

    /// Composite the marks onto an export plate.
    ///
    /// A mark is something a person put there to say "look at this", so a
    /// figure exported without them says nothing — which is the whole reason
    /// the feature exists. The wireframe box and axis captions stay out of the
    /// export on purpose; those are furniture for reading the view on screen,
    /// and a mark is not.
    ///
    /// Grips are excluded too, for the same reason a capture leaves them out:
    /// an exported figure should show what is marked, not the controls for
    /// adjusting it.
    fn draw_marks_on_plate(&self, w: i32, h: i32, base: Vec<u8>, is_3d: bool) -> Vec<u8> {
        let marks = self.annotations.borrow().clone();
        if marks.is_empty() {
            return base;
        }
        let selected = self.selected_annotation.borrow().clone();
        crate::helpers::image_bytes::draw_over_rgba(w, h, base, |cr| {
            // The surface is built for the PLATE size, not the widget's, so
            // marks land correctly at any export resolution rather than only
            // when the plate happens to match the window.
            if is_3d {
                let surface = self.annotation_surface(w, h);
                crate::helpers::annotation_render::draw(
                    &marks,
                    &surface,
                    selected.as_deref(),
                    None,
                    cr,
                    w as f64,
                    h as f64,
                );
            } else {
                let surface = SlicePlateSurface::new(
                    (self.vol.nx, self.vol.ny),
                    (w, h),
                    self.current_channel.get(),
                    self.plate_ink(w),
                );
                crate::helpers::annotation_render::draw(
                    &marks,
                    &surface,
                    selected.as_deref(),
                    None,
                    cr,
                    w as f64,
                    h as f64,
                );
            }
        })
    }

    // ── Annotations ─────────────────────────────────────────────────────────

    pub fn set_annotations(&self, annotations: Vec<crate::models::annotation::Annotation>) {
        *self.annotations.borrow_mut() = annotations;
        self.overlay_area.queue_draw();
        self.slice
            .set_annotations(self.annotations.borrow().clone());
        self.refresh_annotations_panel();
    }

    /// Open the label editor on a mark, at the end of its leader.
    ///
    /// The same widget and the same gestures as the FITS viewer: type, Enter
    /// or the tick to confirm, the bin to delete, Escape to back out. It was a
    /// modal dialog here, which stopped everything to ask one question and put
    /// the answer somewhere other than where you were looking.
    fn open_label_editor(self: &Rc<Self>, id: &str) {
        let Some(mark) = self.annotations().into_iter().find(|a| a.id == id) else {
            return;
        };
        let Some((sx, sy)) = self.project_for_editor(&mark) else {
            return;
        };

        let id_owned = id.to_string();
        let editor = crate::ui::mark_label_editor::MarkLabelEditor::new(
            &mark.text,
            {
                let this = Rc::downgrade(self);
                let id = id_owned.clone();
                move |text| {
                    if let Some(v) = this.upgrade() {
                        v.set_mark_text(&id, &text);
                        v.leave_edit_mode();
                    }
                }
            },
            {
                let this = Rc::downgrade(self);
                let id = id_owned.clone();
                move || {
                    if let Some(v) = this.upgrade() {
                        v.delete_mark(&id);
                        v.leave_edit_mode();
                    }
                }
            },
            {
                let this = Rc::downgrade(self);
                move || {
                    if let Some(v) = this.upgrade() {
                        v.leave_edit_mode();
                    }
                }
            },
        );
        let row = editor.widget().clone();
        self.close_label_editor();
        self.place_over_view(&row, sx, sy);
        *self.open_label_editor.borrow_mut() = Some((id_owned, row));
        editor.focus();
    }

    /// Where the editor should sit for `mark`, in view coordinates.
    ///
    /// Beside the shape rather than on it, so the field does not cover the
    /// thing being named. Whichever view is showing does the projecting, so
    /// the editor lands correctly in both.
    fn project_for_editor(
        &self,
        mark: &crate::models::annotation::Annotation,
    ) -> Option<(f64, f64)> {
        use crate::helpers::annotation_render::AnnotationSurface;
        let (w, h) = self.working_area_size();
        if w <= 0 || h <= 0 {
            return None;
        }
        let (cx, cy) = if self.is_slice_mode() {
            self.slice.project_mark(&mark.anchor)?
        } else {
            self.annotation_surface(w, h).project(&mark.anchor)?
        };
        Some((cx + 12.0, (cy - 34.0).max(0.0)))
    }

    /// Put `child` over the view, its top-left near `(x, y)`.
    fn place_over_view(&self, child: &impl IsA<gtk::Widget>, x: f64, y: f64) {
        let child = child.as_ref();
        if child.parent().is_none() {
            self.view_overlay.add_overlay(child);
        }
        child.set_halign(gtk::Align::Start);
        child.set_valign(gtk::Align::Start);
        child.set_margin_start(x.max(0.0) as i32);
        child.set_margin_top(y.max(0.0) as i32);
    }

    /// Take the editor off the view, if one is up.
    fn close_label_editor(&self) {
        if let Some((_, row)) = self.open_label_editor.borrow_mut().take() {
            self.view_overlay.remove_overlay(&row);
        }
    }

    /// Re-aim the open editor at its mark's current position.
    ///
    /// Without this, dragging a mark leaves the field hanging over where the
    /// shape used to be — which is exactly the complaint the FITS viewer had
    /// before it followed its own.
    fn follow_label_editor(&self) {
        let open = self.open_label_editor.borrow().clone();
        let Some((id, row)) = open else {
            return;
        };
        let Some(mark) = self.annotations().into_iter().find(|a| a.id == id) else {
            return;
        };
        if let Some((x, y)) = self.project_for_editor(&mark) {
            self.place_over_view(&row, x, y);
        }
    }

    /// Leave edit mode: close the field, drop the grips and the selection.
    ///
    /// Edit mode IS a mark being open — the grips and the field are two faces
    /// of one state, so they end together or the view shows a mark half in and
    /// half out of being edited.
    fn leave_edit_mode(&self) {
        self.close_label_editor();
        self.set_editing_annotation(None);
        self.set_selected_annotation(None);
    }

    fn set_mark_text(&self, id: &str, text: &str) {
        let mut all = self.annotations();
        if let Some(m) = all.iter_mut().find(|a| a.id == id) {
            m.text = text.trim().to_string();
        }
        self.set_annotations(all);
        self.persist_annotations();
    }

    /// Give one mark a new look, and keep it.
    fn restyle_mark(&self, id: &str, style: crate::models::annotation::MarkStyle) {
        let mut all = self.annotations();
        let Some(mark) = all.iter_mut().find(|a| a.id == id) else {
            return;
        };
        if mark.effective_style() == style {
            return;
        }
        mark.style = Some(style);
        self.set_annotations(all);
        self.persist_annotations();
    }

    fn delete_mark(&self, id: &str) {
        let mut all = self.annotations();
        all.retain(|a| a.id != id);
        self.set_annotations(all);
        self.persist_annotations();
    }

    /// Refill the sidebar list from what is actually stored.
    ///
    /// Called by the setters rather than by their callers: the owner announces
    /// its own change, so no path — MCP, a click, a file load — can move the
    /// marks and leave the list showing the old set.
    pub fn refresh_annotations_panel(&self) {
        self.marks_section.panel().set_annotations(
            &self.annotations.borrow(),
            self.selected_annotation.borrow().as_deref(),
        );
        let selected = self.selected_annotation.borrow().clone();
        let all = self.annotations.borrow();
        self.marks_section
            .show_style_for(selected.and_then(|id| all.iter().find(|a| a.id == id)));
    }

    /// Write the current marks to the store, under this cube's own path.
    pub fn persist_annotations(&self) -> bool {
        crate::helpers::annotation_store::save_for(
            &self.source_path.borrow(),
            &self.annotations.borrow(),
        )
        .is_ok()
    }

    pub fn annotations(&self) -> Vec<crate::models::annotation::Annotation> {
        self.annotations.borrow().clone()
    }

    /// Open a mark for editing: grips out, and a drag moves or resizes it.
    ///
    /// Separate from selection because the two mean different things — a
    /// selected mark is pointed OUT, an edited one is opened UP, and grips on
    /// a mark nobody is editing invite a drag that means nothing.
    pub fn set_editing_annotation(&self, id: Option<String>) {
        self.slice.set_editing_annotation(id);
    }

    pub fn set_selected_annotation(&self, id: Option<String>) {
        *self.selected_annotation.borrow_mut() = id.clone();
        self.overlay_area.queue_draw();
        self.slice.set_selected_annotation(id);
        self.refresh_annotations_panel();
    }

    pub fn selected_annotation(&self) -> Option<String> {
        self.selected_annotation.borrow().clone()
    }

    /// The voxel a click lands on, using the slice plane for depth.
    ///
    /// A click on a volume is a RAY: every voxel along it projects to the same
    /// pixel, and nothing here can tell which was meant. The slice plane
    /// answers it — it is already drawn in the volume as the cyan quad, and the
    /// scrubber that moves it is right there, so the user places on a surface
    /// they can see. Someone wanting another depth scrubs to it first, which is
    /// a gesture they already use.
    ///
    /// An agent never meets this: `annotate_cube` takes an explicit voxel.
    pub fn voxel_at_screen(&self, sx: f64, sy: f64, w: i32, h: i32) -> Option<(f64, f64, f64)> {
        if w <= 0 || h <= 0 {
            return None;
        }
        let z = self.current_channel.get() as f64;
        let dims = (self.vol.nx, self.vol.ny, self.vol.nz);
        let vp = self.gl.view_proj(w, h);
        let spectral = self.gl.spectral_scale();
        // Search the plane for the voxel that projects nearest the click. The
        // plane is a quad under an arbitrary rotation, so there is no closed
        // form worth writing; a coarse pass then a fine one around the winner
        // is exact enough for placing a mark and costs nothing at click rates.
        let mut best: Option<(f64, f64, f64)> = None;
        let mut best_d2 = f64::MAX;
        let consider = |x: f64, y: f64, best: &mut Option<(f64, f64, f64)>, best_d2: &mut f64| {
            if let Some((px, py)) = crate::helpers::cube_axes::project_voxel(
                &vp,
                dims,
                spectral,
                (x, y, z),
                (w as f32, h as f32),
            ) {
                let d2 = (px as f64 - sx).powi(2) + (py as f64 - sy).powi(2);
                if d2 < *best_d2 {
                    *best_d2 = d2;
                    *best = Some((x, y, z));
                }
            }
        };
        let (nx, ny) = (self.vol.nx.max(1), self.vol.ny.max(1));
        let coarse = 24usize;
        for i in 0..=coarse {
            for j in 0..=coarse {
                let x = (nx - 1) as f64 * i as f64 / coarse as f64;
                let y = (ny - 1) as f64 * j as f64 / coarse as f64;
                consider(x, y, &mut best, &mut best_d2);
            }
        }
        let (cx, cy, _) = best?;
        let step_x = (nx - 1) as f64 / coarse as f64;
        let step_y = (ny - 1) as f64 / coarse as f64;
        for i in -6i32..=6 {
            for j in -6i32..=6 {
                let x = (cx + step_x * i as f64 / 6.0).clamp(0.0, (nx - 1) as f64);
                let y = (cy + step_y * j as f64 / 6.0).clamp(0.0, (ny - 1) as f64);
                consider(x, y, &mut best, &mut best_d2);
            }
        }
        // A click far from the plane is not a placement.
        (best_d2.sqrt() <= 40.0).then_some(best?)
    }

    /// The on-screen size of the working area, for a capture that matches it.
    pub fn working_area_size(&self) -> (i32, i32) {
        (self.overlay_area.width(), self.overlay_area.height())
    }

    /// Whether the 3D volume is the visible mode.
    ///
    /// The axes overlay belongs to the volume; the 2D slice draws its own.
    fn showing_volume(&self) -> bool {
        self.stack
            .visible_child_name()
            .is_none_or(|n| n == "volume")
    }

    /// The WORKING AREA as PNG bytes: what the user is looking at.
    ///
    /// `render_figure` — and so `export_cube_figure` — returns the volume or the
    /// slice ALONE. On screen the volume sits under a transparent overlay
    /// carrying the wireframe box, the WCS axis captions and the slice-plane
    /// marker, so an agent handed the export was given the data without the
    /// frame of reference the user reads it by, and nothing said so.
    ///
    /// Here the two are composited the way the widgets are stacked: the render
    /// underneath, the same overlay drawing on top, both at the same size so
    /// the projection they derive from it agrees.
    pub fn capture_working_area_png(&self, w: i32, h: i32) -> Result<Vec<u8>, String> {
        if w <= 0 || h <= 0 {
            return Err(format!("invalid capture size {w}x{h}"));
        }
        // Opaque: a transparent export is for compositing into a document, but
        // an agent looking at a picture wants the background it sees.
        let rgba = self
            .render_figure(w, h, false)
            .ok_or_else(|| "cube could not be rendered (GL unavailable)".to_string())?;

        let png = crate::helpers::png::encode_rgba(w, h, &rgba)?;
        if !self.showing_volume() {
            // The 2D slice has no separate overlay layer to add.
            return Ok(png);
        }

        // Re-read the render as a surface, then draw the overlay over it.
        let mut cursor = std::io::Cursor::new(png);
        let base = cairo::ImageSurface::create_from_png(&mut cursor)
            .map_err(|e| format!("cairo could not read the render back: {e}"))?;
        let surface = cairo::ImageSurface::create(cairo::Format::ARgb32, w, h)
            .map_err(|e| format!("cairo surface error: {e}"))?;
        {
            let cr =
                cairo::Context::new(&surface).map_err(|e| format!("cairo context error: {e}"))?;
            cr.set_source_surface(&base, 0.0, 0.0)
                .map_err(|e| format!("cairo source error: {e}"))?;
            cr.paint().map_err(|e| format!("cairo paint error: {e}"))?;
            self.draw_axes_overlay(&cr, w, h, false);
        }
        let mut out: Vec<u8> = Vec::new();
        surface
            .write_to_png(&mut out)
            .map_err(|e| format!("PNG encode failed: {e}"))?;
        Ok(out)
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
            if slice {
                // Recomputed on each switch rather than once at construction:
                // the panel has a real size by now, and attaching a
                // native-resolution plane changes what a voxel is worth. It
                // leaves a view the user has already zoomed alone.
                this.match_slice_zoom_to_volume();
            }
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

    /// Show the 2-D slice, or the volume.
    ///
    /// Drives the toggle rather than the stack, so the same handler runs as
    /// when a person clicks it — one path into the change instead of two that
    /// can disagree about the VOLUME control group's visibility.
    pub fn set_slice_mode(&self, slice: bool) {
        if slice {
            self.mode_slice.set_active(true);
        } else {
            self.mode_3d.set_active(true);
        }
    }

    /// Whether the 2-D slice is the visible mode.
    pub fn is_slice_mode(&self) -> bool {
        self.stack
            .visible_child_name()
            .is_some_and(|n| n == "slice")
    }

    fn force_slice_only(&self) {
        self.match_slice_zoom_to_volume();
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

        // ── Marks ───────────────────────────────────────────────────────────
        // Picking a row points a mark OUT, and takes you to the channel it is
        // on: a cube mark lives on one plane, so selecting one you cannot see
        // would light up nothing. Clicking the lit row again clears it.
        {
            let this = self.clone();
            ctl.marks_section.panel().set_on_select(move |id| {
                if id.is_empty() {
                    this.leave_edit_mode();
                    return;
                }
                let channel = this
                    .annotations()
                    .iter()
                    .find(|a| a.id == id)
                    .and_then(|a| match a.anchor {
                        crate::models::annotation::Anchor::Data { z, .. } => Some(z),
                        _ => None,
                    });
                if let Some(z) = channel {
                    this.set_current_channel(z.round().max(0.0) as usize);
                }
                // Pointed out, not opened up: the list picks a mark, the image
                // is where you edit one.
                this.close_label_editor();
                this.set_editing_annotation(None);
                this.set_selected_annotation(Some(id.to_string()));
            });
        }
        {
            let this = self.clone();
            ctl.marks_section.panel().set_on_delete(move |id| {
                // An open field on the mark being deleted would be left
                // pointing at a mark that no longer exists.
                this.leave_edit_mode();
                this.delete_mark(id);
            });
        }
        {
            let this = self.clone();
            ctl.marks_section.panel().set_on_edit(move |id| {
                this.set_selected_annotation(Some(id.to_string()));
                this.set_editing_annotation(Some(id.to_string()));
                this.open_label_editor(id);
            });
        }
        {
            let this = self.clone();
            ctl.marks_section.panel().set_on_clear(move |_| {
                this.leave_edit_mode();
                this.set_annotations(Vec::new());
                this.persist_annotations();
            });
        }

        {
            // Moving a style control restyles the selected mark if there is
            // one, and otherwise sets what the next mark will look like.
            let this = self.clone();
            ctl.marks_section
                .set_on_style_changed(move |style| match this.selected_annotation() {
                    Some(id) => this.restyle_mark(&id, style),
                    None => crate::services::settings_service::remember_mark_style(style),
                });
        }

        // One picker, asked by both previews at draw time.
        {
            let section = ctl.marks_section.clone();
            self.slice
                .set_pending_mark_source(move || section.pending());
        }

        // Draw mode arms the slice: a click there places a mark instead of
        // probing a spectrum. Only the 2D slice, because a click on a volume
        // is a ray through it rather than a point — see `place_mark`.
        {
            let this = self.clone();
            ctl.marks_section.draw_mode().connect_toggled(move |btn| {
                let on = btn.is_active();
                this.slice.set_placing(on);
                if !on {
                    this.set_editing_annotation(None);
                }
                // No mode switch. Placing works in BOTH views now — on the
                // slice directly, and in the volume by landing the click on
                // the plane the slice marker draws — so arming drawing must
                // not decide for the user which one they meant to draw on.
            });
        }
        {
            let this = self.clone();
            self.slice.set_on_place(move |vx, vy, radius| {
                this.place_mark(vx, vy, radius);
            });
        }
        // A drag finished moving or resizing a mark. The slice holds a mirror
        // for live feedback; this is where it becomes the record.
        {
            let this = self.clone();
            self.slice.set_on_marks_changed(move |marks| {
                this.set_annotations(marks);
                this.persist_annotations();
                this.follow_label_editor();
            });
        }
        // Clicking a mark on the image opens it; clicking away closes it.
        {
            let this = self.clone();
            self.slice.set_on_mark_selected(move |id| match id {
                // Opening a mark shows its field straight away — naming it is
                // the reason you opened it, and a second click to get a cursor
                // is a click nobody wants to make.
                Some(id) => {
                    this.set_selected_annotation(Some(id.clone()));
                    this.set_editing_annotation(Some(id.clone()));
                    this.open_label_editor(&id);
                }
                None => this.leave_edit_mode(),
            });
        }

        // ── Marks in the VOLUME view ────────────────────────────────────────
        //
        // A click on a volume is a RAY, not a point, so it needs a plane to
        // land on — and the plane you are looking at is the one the slice
        // marker already draws. Every mark is on some channel anyway, so
        // resolving to one is not a compromise; it is what `annotate_cube`
        // already means when it defaults `z` to the channel on screen.
        //
        // Capture phase, and it claims the sequence only when it has actually
        // taken hold of something. Anything else falls through to the orbit
        // drag underneath, so the camera still works exactly as before —
        // marks are not allowed to make the volume harder to look at.
        {
            let this = self.clone();
            let drag = gtk::GestureDrag::new();
            drag.set_button(1);
            drag.set_propagation_phase(gtk::PropagationPhase::Capture);
            let grabbed: Rc<RefCell<Option<VolumeGrab>>> = Rc::new(RefCell::new(None));
            {
                let this = this.clone();
                let grabbed = grabbed.clone();
                drag.connect_drag_begin(move |g, x, y| {
                    let grab = this.volume_grab_at(x, y);
                    if grab.is_none() {
                        *grabbed.borrow_mut() = None;
                        return;
                    }
                    *grabbed.borrow_mut() = grab;
                    g.set_state(gtk::EventSequenceState::Claimed);
                });
            }
            {
                let this = this.clone();
                let grabbed = grabbed.clone();
                drag.connect_drag_update(move |g, dx, dy| {
                    let Some(grab) = grabbed.borrow().clone() else {
                        return;
                    };
                    let Some((sx, sy)) = g.start_point() else {
                        return;
                    };
                    this.volume_drag(&grab, sx, sy, dx, dy);
                });
            }
            {
                let this = this.clone();
                drag.connect_drag_end(move |g, dx, dy| {
                    let Some(grab) = grabbed.borrow_mut().take() else {
                        return;
                    };
                    let Some((sx, sy)) = g.start_point() else {
                        return;
                    };
                    this.volume_drag_end(&grab, sx, sy, dx, dy);
                });
            }
            self.gl.widget().add_controller(drag);
        }

        // Escape leaves draw mode, as it does in the FITS viewer. Driven
        // through the toggle rather than by disarming the slice directly, so
        // the button cannot end up pressed while nothing is armed.
        {
            let key = gtk::EventControllerKey::new();
            let draw_mode = ctl.marks_section.draw_mode().clone();
            let this = self.clone();
            key.connect_key_pressed(move |_, keyval, _code, _modifier| {
                if keyval != gtk::gdk::Key::Escape {
                    return glib::Propagation::Proceed;
                }
                // Closing an open mark first: Escape means "stop what I am in
                // the middle of", and editing is the more immediate of the two.
                if this.slice.editing_annotation().is_some() {
                    this.leave_edit_mode();
                    return glib::Propagation::Stop;
                }
                if draw_mode.is_active() {
                    draw_mode.set_active(false);
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            });
            self.widget.add_controller(key);
        }
    }

    /// What a press on the volume panel has taken hold of.
    ///
    /// Placing is separate from the shared [`MarkGrab`] because it is not a
    /// grab at all: nothing is under the pointer, and the press is going to
    /// create something rather than take hold of it.
    fn volume_grab_at(self: &Rc<Self>, px: f64, py: f64) -> Option<VolumeGrab> {
        let (w, h) = self.working_area_size();
        if w <= 0 || h <= 0 {
            return None;
        }
        let surface = self.annotation_surface(w, h);
        let marks = self.annotations.borrow().clone();
        let editing = self.slice.editing_annotation();
        match crate::helpers::annotation_render::grab_at(
            &marks,
            &surface,
            editing.as_deref(),
            self.marks_section.draw_mode().is_active(),
            px,
            py,
        ) {
            crate::helpers::annotation_render::MarkGrab::Move { id, .. } => {
                // The grab offset is deliberately dropped here. On a
                // foreshortened plane a screen offset is not a constant voxel
                // offset, so carrying it would drag the mark away from the
                // pointer as the perspective changed across the plane.
                Some(VolumeGrab::Move { id })
            }
            crate::helpers::annotation_render::MarkGrab::Resize { id } => {
                Some(VolumeGrab::Resize { id })
            }
            crate::helpers::annotation_render::MarkGrab::Place => Some(VolumeGrab::Place),
            // Nothing of ours: the press belongs to the camera.
            crate::helpers::annotation_render::MarkGrab::None => None,
        }
    }

    /// Update the shape a placing drag in the volume is drawing.
    ///
    /// Sized from the SCREEN drag divided by the local scale, not from
    /// unprojecting the drag's two ends. The plane is foreshortened, so a drag
    /// along the receding axis covers far more voxels than the same drag
    /// across it — measuring between the unprojected ends made a mark much
    /// bigger than the one you dragged out, and with no preview to show it you
    /// only found out on release.
    fn size_pending(self: &Rc<Self>, sx: f64, sy: f64, dx: f64, dy: f64) {
        const CLICK_SLOP: f64 = 4.0;
        let (w, h) = self.working_area_size();
        if w <= 0 || h <= 0 {
            return;
        }
        let z = self.current_channel.get() as f64;
        let Some((vx, vy)) = self.volume_voxel_at(sx, sy, z) else {
            return;
        };
        let half = if dx.hypot(dy) > CLICK_SLOP {
            crate::helpers::annotation_render::half_from_drag(
                &self.annotation_surface(w, h),
                &crate::models::annotation::Anchor::Data { x: vx, y: vy, z },
                dx.hypot(dy),
            )
        } else {
            0.0
        };
        *self.pending_shape.borrow_mut() = Some((vx, vy, half));
        self.overlay_area.queue_draw();
    }

    /// The voxel under `(px, py)` on plane `z`, or `None` if the plane is not
    /// facing the camera there.
    fn volume_voxel_at(&self, px: f64, py: f64, z: f64) -> Option<(f64, f64)> {
        let (w, h) = self.working_area_size();
        if w <= 0 || h <= 0 {
            return None;
        }
        crate::helpers::cube_axes::unproject_to_plane(
            &self.gl.view_proj(w, h),
            (self.vol.nx, self.vol.ny, self.vol.nz),
            self.gl.spectral_scale(),
            z,
            (w as f32, h as f32),
            (px, py),
        )
    }

    /// The channel a mark sits on.
    fn mark_channel(&self, id: &str) -> Option<f64> {
        self.annotations()
            .iter()
            .find(|a| a.id == id)
            .and_then(|a| match a.anchor {
                crate::models::annotation::Anchor::Data { z, .. } => Some(z),
                _ => None,
            })
    }

    /// Live feedback while dragging in the volume. `(sx, sy)` is where the
    /// press started; `(dx, dy)` is how far it has moved.
    fn volume_drag(self: &Rc<Self>, grab: &VolumeGrab, sx: f64, sy: f64, dx: f64, dy: f64) {
        let (w, h) = self.working_area_size();
        if w <= 0 || h <= 0 {
            return;
        }
        let (px, py) = (sx + dx, sy + dy);
        let surface = self.annotation_surface(w, h);
        let mut marks = self.annotations.borrow().clone();
        match grab {
            VolumeGrab::Place => {
                self.size_pending(sx, sy, dx, dy);
                return;
            }
            VolumeGrab::Move { id } => {
                let Some(z) = self.mark_channel(id) else {
                    return;
                };
                // A mark moves within its OWN plane, not the one on screen.
                // Dragging a mark on channel 40 while looking at channel 12
                // must not quietly haul it to 12.
                let Some((vx, vy)) = self.volume_voxel_at(px, py, z) else {
                    return;
                };
                if let Some(m) = marks.iter_mut().find(|a| &a.id == id) {
                    m.anchor = crate::models::annotation::Anchor::Data { x: vx, y: vy, z };
                }
            }
            VolumeGrab::Resize { id } => {
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
        }
        *self.annotations.borrow_mut() = marks;
        self.overlay_area.queue_draw();
        self.follow_label_editor();
    }

    /// Commit a volume drag.
    fn volume_drag_end(self: &Rc<Self>, grab: &VolumeGrab, sx: f64, sy: f64, dx: f64, dy: f64) {
        const CLICK_SLOP: f64 = 4.0;
        match grab {
            VolumeGrab::Place => {
                // Size it once more through the same function the preview
                // used, then place exactly what was on screen. Computing the
                // radius a second way here is how a preview and the mark it
                // becomes drift apart.
                self.size_pending(sx, sy, dx, dy);
                let pending = self.pending_shape.borrow_mut().take();
                self.overlay_area.queue_draw();
                if let Some((vx, vy, half)) = pending {
                    self.place_mark(vx, vy, half);
                }
            }
            // A press that never moved is a click on the mark: point it out
            // and open it, the same as on the slice.
            VolumeGrab::Move { id } | VolumeGrab::Resize { id } if dx.hypot(dy) <= CLICK_SLOP => {
                self.set_selected_annotation(Some(id.clone()));
                self.set_editing_annotation(Some(id.clone()));
                self.open_label_editor(id);
            }
            VolumeGrab::Move { .. } | VolumeGrab::Resize { .. } => {
                let marks = self.annotations.borrow().clone();
                self.set_annotations(marks);
                self.persist_annotations();
            }
        }
    }

    /// The cube's Info-panel facts, for `get_cube_view`.
    ///
    /// The panel shows object, telescope, instrument, unit and the value
    /// range, and none of it reached an agent — so a tool could describe a
    /// cube's camera angle in detail and not say what object it was pointed
    /// at. `None` for a synthetic volume, which genuinely has no metadata.
    pub fn metadata_json(&self) -> serde_json::Value {
        let Some(m) = self.vol.meta.as_ref() else {
            return serde_json::Value::Null;
        };
        serde_json::json!({
            "object": m.object,
            "telescope": m.telescope,
            "instrument": m.instrument,
            "unit": m.bunit,
            "dataMin": m.data_min,
            "dataMax": m.data_max,
            "median": m.median,
            "nanFraction": m.nan_fraction,
            // The cube as it is on disk, which is not what is in RAM: a large
            // cube is decimated to load, and an agent converting voxels to
            // anything else needs to know which grid it is on.
            "nativeDims": { "nx": m.nx, "ny": m.ny, "nz": m.nz },
        })
    }

    /// The 2-D slice's own view state — the zoom and pan of that view, which
    /// are nowhere in the volume's camera.
    pub fn slice_view_json(&self) -> serde_json::Value {
        let (zoom, pan_x, pan_y) = self.slice.probe_view();
        serde_json::json!({ "zoom": zoom, "panX": pan_x, "panY": pan_y })
    }

    /// Set the 2-D slice's zoom and pan.
    pub fn set_slice_view(&self, zoom: Option<f64>, pan: Option<(f64, f64)>, reset: bool) {
        self.slice.set_view(zoom, pan, reset);
    }

    /// Put the slice at the same apparent scale as the volume.
    ///
    /// Measured, not assumed: ask both surfaces what one voxel is worth in
    /// screen pixels at the same panel size, and take the ratio. The two views
    /// frame the data differently on purpose — the volume keeps the whole box
    /// on screen at every orbit angle, which is further out than fitting a
    /// plane to the widget — so fit-to-widget made everything, marks included,
    /// jump by about a factor of two when you switched modes.
    ///
    /// A ratio rather than a constant because it is not one number: it depends
    /// on the cube's shape and on the camera's defaults, and measuring keeps it
    /// right when either changes.
    fn match_slice_zoom_to_volume(&self) {
        use crate::helpers::annotation_render::AnnotationSurface;
        // The STACK's size, not the volume overlay's: both views live in it, so
        // it has a real allocation whichever one is showing. Reading the
        // volume's own overlay gave zero while the slice was up, and the match
        // silently did nothing.
        //
        // Before the first allocation, a nominal panel. Both terms of the
        // ratio scale with the panel, so it barely depends on the number — and
        // a slightly-off match is far better than the factor-of-two jump that
        // doing nothing leaves.
        let (w, h) = match (self.stack.width(), self.stack.height()) {
            (w, h) if w > 0 && h > 0 => (w, h),
            _ => (800, 800),
        };
        // The middle of the cube, on the middle channel: the volume has
        // perspective, so a voxel is worth different amounts at the front and
        // the back, and the centre is the honest average.
        let anchor = crate::models::annotation::Anchor::Data {
            x: self.vol.nx as f64 / 2.0,
            y: self.vol.ny as f64 / 2.0,
            z: self.vol.nz as f64 / 2.0,
        };
        let in_volume = self.annotation_surface(w, h).units_to_pixels(&anchor);
        let at_fit = self.slice.voxel_pixels_at_fit(w, h);
        if in_volume > 0.0 && at_fit > 0.0 {
            self.slice.set_default_zoom(in_volume / at_fit);
        }
    }

    /// The slice's current zoom, and what one voxel is worth on screen in
    /// each view. For `cube_slice_zoom_probe`, which checks that the two views
    /// agree — something no unit test can see, because both numbers come from
    /// live widgets.
    pub fn probe_scales(&self) -> (f64, f64, f64) {
        use crate::helpers::annotation_render::AnnotationSurface;
        let (w, h) = match (self.stack.width(), self.stack.height()) {
            (w, h) if w > 0 && h > 0 => (w, h),
            _ => (800, 800),
        };
        let anchor = crate::models::annotation::Anchor::Data {
            x: self.vol.nx as f64 / 2.0,
            y: self.vol.ny as f64 / 2.0,
            z: self.vol.nz as f64 / 2.0,
        };
        let in_volume = self.annotation_surface(w, h).units_to_pixels(&anchor);
        let zoom = self.slice.probe_zoom();
        let on_slice = self.slice.voxel_pixels_at_fit(w, h) * zoom;
        (zoom, in_volume, on_slice)
    }

    /// Scroll the slice, as the wheel does. For the zoom probe.
    pub fn probe_scroll_slice(&self, factor: f64) {
        self.slice.probe_scroll(factor);
    }

    /// A mark size that is visible on THIS cube.
    ///
    /// Not a fixed number of voxels: that is a dot on a 2048-wide plane and
    /// covers a 32-wide one. A mark with no extent at all draws at zero size —
    /// the renderer falls back to `(0.0, 0.0)` — so "no radius given" must
    /// become a real number here rather than nothing, or the mark is invisible
    /// with nothing reporting a problem.
    pub fn default_mark_extent(&self) -> f64 {
        (self.vol.nx.min(self.vol.ny) as f64 * 0.03).max(1.5)
    }

    /// Add a mark at a voxel, from a click on the slice.
    ///
    /// `radius` of zero means the click was not dragged.
    fn place_mark(self: &Rc<Self>, vx: f64, vy: f64, radius: f64) {
        use crate::models::annotation::{Anchor, Annotation, Author, Extent};
        // Read at CLICK time, not when drawing was armed.
        let pending = self.marks_section.pending();
        let half = if radius > 0.0 {
            radius
        } else {
            self.default_mark_extent()
        };
        // The style is COPIED into the mark now, and never consulted again:
        // changing the row afterwards must not restyle marks already drawn.
        let mark = Annotation::new(
            pending.kind,
            Anchor::Data {
                x: vx,
                y: vy,
                z: self.current_channel.get() as f64,
            },
            String::new(),
            Author::User,
        )
        .with_extent(Extent::square(half))
        .with_style(pending.style);
        let id = mark.id.clone();
        let mut all = self.annotations();
        all.push(mark);
        self.set_annotations(all);
        self.persist_annotations();
        // Straight into naming it: a mark with no label is a ring around
        // nothing, and the point of drawing one is to say what it is.
        self.set_selected_annotation(Some(id.clone()));
        self.set_editing_annotation(Some(id.clone()));
        self.open_label_editor(&id);
    }

    // ── Wireframe box + WCS caption overlay ──────────────────────────────────

    /// Draw the projected box edges, WCS axis captions, and slice-plane marker on
    /// the transparent overlay, and track the camera so it stays aligned.
    /// Draw the axes overlay — the wireframe box, WCS captions and slice-plane
    /// marker — into `cr` at `w` x `h`.
    ///
    /// Extracted from the `set_draw_func` closure so a capture for an agent can
    /// run the SAME drawing over the same volume render. `export_cube_figure`
    /// returns `render_figure` alone, which is the volume stripped of the axes
    /// the user is reading it by — not wrong so much as not the picture on
    /// screen.
    ///
    /// The projection is derived from `w` and `h`, so the overlay aligns with
    /// the volume only when both are rendered at the same size. That is the
    /// shape of the HiDPI bug these two layers already had once.
    /// `chrome` draws the editing grips as well as the marks. Off for a
    /// capture: an agent's picture should show what is marked, not the
    /// controls a person uses to adjust it — the same split the FITS canvas
    /// makes.
    pub fn draw_axes_overlay(&self, cr: &cairo::Context, w: i32, h: i32, chrome: bool) {
        if w < 1 || h < 1 {
            return;
        }
        let vp = self.gl.view_proj(w, h);
        let overlay = cube_axes::build(&cube_axes::AxesRequest {
            dims: (self.vol.nx, self.vol.ny, self.vol.nz),
            wcs: &self.wcs,
            view_proj: &vp,
            panel: (w as f32, h as f32),
            slice_z: self.current_channel.get(),
            spectral_scale: self.gl.spectral_scale(),
        });

        // Slice-plane marker (behind the edges): translucent cyan fill + edge.
        if self.slice_toggle.is_active() && overlay.slice_quad.len() == 4 {
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
        if self.captions_toggle.is_active() {
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

        // Marks over the axes, inside the same function a capture replays, so
        // the user's screen and an agent's picture cannot show different sets.
        let surface = self.annotation_surface(w, h);
        // The shape being drawn right now, under the finished ones. Chrome
        // only: a capture or an export should show marks, not a half-made one.
        if chrome {
            if let Some((vx, vy, half)) = *self.pending_shape.borrow() {
                let z = self.current_channel.get() as f64;
                let anchor = crate::models::annotation::Anchor::Data { x: vx, y: vy, z };
                if let Some((sx, sy)) = {
                    use crate::helpers::annotation_render::AnnotationSurface;
                    surface.project(&anchor)
                } {
                    use crate::helpers::annotation_render::AnnotationSurface;
                    let r = half * surface.units_to_pixels(&anchor);
                    let pending = self.marks_section.pending();
                    crate::helpers::annotation_render::draw_preview(
                        pending.kind,
                        sx,
                        sy,
                        r,
                        pending.style,
                        cr,
                    );
                }
            }
        }
        let editing = self.slice.editing_annotation();
        crate::helpers::annotation_render::draw(
            &self.annotations.borrow(),
            &surface,
            self.selected_annotation.borrow().as_deref(),
            editing.as_deref(),
            cr,
            w as f64,
            h as f64,
        );
        // Grips on the edited mark, so it can be resized here as well as on
        // the slice. Skipped for a capture: an agent's picture should show the
        // marks, not the controls for editing them.
        if chrome {
            if let Some(mark) = editing.and_then(|id| {
                self.annotations
                    .borrow()
                    .iter()
                    .find(|a| a.id == id)
                    .cloned()
            }) {
                crate::helpers::annotation_render::draw_handles(&mark, &surface, cr);
            }
        }
    }

    /// The projection annotations use, for a given panel size.
    ///
    /// A small struct rather than an `impl` on `CubeViewer` because the surface
    /// needs the panel size, which the viewer only learns when it is asked to
    /// draw. Same trait, so the renderer is unchanged.
    fn annotation_surface(&self, w: i32, h: i32) -> CubeAnnotationSurface {
        CubeAnnotationSurface {
            view_proj: self.gl.view_proj(w, h),
            dims: (self.vol.nx, self.vol.ny, self.vol.nz),
            spectral_scale: self.gl.spectral_scale(),
            panel: (w as f32, h as f32),
            ink: self.plate_ink(w),
        }
    }

    /// How much bigger a plate `w` pixels wide is than the working area.
    ///
    /// The camera and the framing are the same either way, so the plate is the
    /// screen at a different resolution — and the marks belong at that
    /// resolution too. Without it a 4x figure kept 2px rings and 12px labels
    /// while its own title and caption scaled, so the annotations were the one
    /// thing in the picture that shrank.
    ///
    /// 1.0 when the working area has no size: a headless render — a probe, or
    /// an agent asking before the window is mapped — has a zero allocation, and
    /// dividing by it would put every mark at infinity.
    fn plate_ink(&self, w: i32) -> f64 {
        plate_ink(w, self.working_area_size().0)
    }

    fn setup_overlay(self: &Rc<Self>) {
        let this = self.clone();
        self.overlay_area.set_draw_func(move |_area, cr, w, h| {
            this.draw_axes_overlay(cr, w, h, true);
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
    /// The plate the Export dialog composes onto.
    ///
    /// Was a second copy of `render_figure` differing only in the transparency
    /// flag, which is how marks came to be missing from the exported figure in
    /// one place and would have had to be fixed in two.
    fn capture_plate(&self, w: i32, h: i32) -> Option<Vec<u8>> {
        self.render_figure(w, h, true)
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

    /// The plate this cube would export, for the dialog and for the MCP tool.
    ///
    /// One builder, so an agent's figure and a person's are the same figure —
    /// the tool used to return a bare render while the button produced a
    /// captioned plate, and neither said so.
    pub fn plate_content(self: &Rc<Self>) -> crate::ui::figure_plate::PlateContent {
        let this = self.clone();
        let capture: Rc<dyn Fn(i32, i32) -> Option<Vec<u8>>> =
            Rc::new(move |w, h| this.capture_plate(w, h));
        let colormap = cube_colormaps::NAMES
            .get(self.colormap.selected() as usize)
            .copied()
            .unwrap_or(cube_colormaps::DEFAULT)
            .to_string();
        let (lo_label, hi_label) = self.colorbar_labels();

        // Captions render only over the 3D volume; the export overlay shares
        // the GL camera through `view_proj`, so it aligns with the captured
        // render at any plate scale.
        let is_3d = !self.is_slice_mode();
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
        crate::ui::cube_export::plate_content(
            capture,
            self.name.clone(),
            self.wcs_caption(),
            colormap,
            lo_label,
            hi_label,
            overlay,
        )
    }

    fn show_export(self: &Rc<Self>) {
        let this = self.clone();
        let compose: crate::ui::export_dialog::Compose =
            Rc::new(move |scale, transparent| this.plate_content().compose(scale, transparent));
        crate::ui::export_dialog::show(&self.widget, &self.name, compose);
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
    /// The Marks section, shared with the FITS viewer.
    marks_section: Rc<crate::ui::annotations_panel::MarksSection>,
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

    // ── Marks ───────────────────────────────────────────────────────────────
    // The same section the FITS viewer mounts, from the same component: one
    // collapsible, one list, one pencil, one shape picker.
    let marks_section = crate::ui::annotations_panel::MarksSection::new(crate::tr_en!(
        "Draw a mark on the cube, in either view. Click where you mean, drag to size it, \
         Escape to stop. In the 3D view the mark lands on the channel you are on."
    ));
    column.append(marks_section.widget());

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
        marks_section,
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

#[cfg(test)]
mod working_area_tests {
    //! The capture must composite, and must refuse what it cannot draw.
    //!
    //! The composite itself needs GL and a realized widget, so it is verified by
    //! `dev_info/17` — probe, then eye. What CAN be checked here is the thing
    //! that would silently undo it: a capture that returns the render alone.
    //! That is not a hypothetical. `export_cube_figure` does exactly that, and
    //! shipped for months looking like a cube export while omitting the
    //! wireframe box, the WCS axis captions and the slice-plane marker.

    /// The working-area capture draws the overlay; the export does not.
    #[test]
    fn the_capture_composites_the_overlay_rather_than_returning_the_render() {
        let source =
            crate::testing::without_comments(crate::testing::code(include_str!("cube_viewer.rs")));
        let at = source
            .find("pub fn capture_working_area_png")
            .expect("capture_working_area_png");
        // Scope to the function, and scope it on CODE. The first version
        // looked for the next `///` doc comment — which `without_comments` had
        // already removed, so the search failed, the body became the whole
        // file, and `setup_overlay`'s own call to `draw_axes_overlay` satisfied
        // the assertion with the composite deleted. Mutation testing caught it;
        // reading it did not.
        let end = source[at + 1..]
            .find("\n    fn ")
            .into_iter()
            .chain(source[at + 1..].find("\n    pub fn "))
            .min()
            .map(|e| at + 1 + e)
            .unwrap_or(source.len());
        let body = &source[at..end];

        assert!(
            body.contains("draw_axes_overlay"),
            "the capture no longer draws the overlay, so an agent is shown the \
             volume without the axes the user reads it by:\n{body}"
        );
        assert!(
            body.contains("render_figure"),
            "the capture no longer renders the volume:\n{body}"
        );
        // The overlay is skipped ONLY for the 2D slice, which draws its own.
        // A source scan cannot see reachability — an early return of `if true`
        // leaves the call in the text while never running it — so the condition
        // is pinned rather than merely the presence of the call.
        assert!(
            body.contains("if !self.showing_volume()"),
            "the overlay is skipped on some condition other than the 2D slice \
             being visible, which would silently drop it in volume mode:\n{body}"
        );
    }
}

/// Where a mark lands on an exported 2-D slice plate.
///
/// The plate is the whole plane scaled to the requested size — no pan, no
/// zoom, unlike the on-screen slice — so a voxel maps by a straight scale.
struct SlicePlateSurface {
    sx: f64,
    sy: f64,
    /// How much bigger this plate is than the working area — see `plate_ink`.
    ink: f64,
    /// The channel the plate was rendered from. A mark on any other one is not
    /// on this picture.
    z: usize,
}

impl SlicePlateSurface {
    fn new(vol: (usize, usize), plate: (i32, i32), z: usize, ink: f64) -> Self {
        Self {
            sx: plate.0.max(1) as f64 / vol.0.max(1) as f64,
            sy: plate.1.max(1) as f64 / vol.1.max(1) as f64,
            ink,
            z,
        }
    }
}

impl crate::helpers::annotation_render::AnnotationSurface for SlicePlateSurface {
    fn project(&self, anchor: &crate::models::annotation::Anchor) -> Option<(f64, f64)> {
        let crate::models::annotation::Anchor::Data { x, y, z } = *anchor else {
            return None;
        };
        (z.round() as i64 == self.z as i64).then_some((x * self.sx, y * self.sy))
    }

    fn ink_scale(&self) -> f64 {
        self.ink
    }

    fn units_to_pixels(&self, _anchor: &crate::models::annotation::Anchor) -> f64 {
        // The export stretches the plane to the requested plate size without
        // preserving its aspect, so the two axes can scale differently and a
        // single number cannot describe both. The geometric mean splits the
        // difference: exact whenever the plate keeps the plane's aspect, which
        // is the case worth being exact for, and never wrong by more than the
        // distortion the picture itself already has.
        (self.sx * self.sy).sqrt().max(0.01)
    }
}

/// What a press on the volume panel took hold of.
#[derive(Clone, Debug, PartialEq)]
enum VolumeGrab {
    /// Drawing is armed and the press was on empty space.
    Place,
    Move {
        id: String,
    },
    Resize {
        id: String,
    },
}

/// The cube's volume as a place to draw marks.
///
/// Holds the frame's projection rather than borrowing the viewer, because the
/// panel size is only known at draw time. The near-plane cull comes from
/// `project_voxel` and is honoured rather than clamped: a mark behind the
/// camera that is clamped onto the canvas looks placed, and is pointing at
/// nothing.
/// How much bigger a plate `plate_w` pixels wide is than a `screen_w` view.
///
/// A free function because the method around it needs a realised widget, and a
/// widget is the one thing a headless test cannot have — which is precisely the
/// case that has to be right: an unrealised working area is zero pixels wide,
/// and dividing by it would send every mark to infinity.
fn plate_ink(plate_w: i32, screen_w: i32) -> f64 {
    if plate_w <= 0 || screen_w <= 0 {
        return 1.0;
    }
    f64::from(plate_w) / f64::from(screen_w)
}

struct CubeAnnotationSurface {
    view_proj: crate::helpers::cube_math::Mat4,
    dims: (usize, usize, usize),
    spectral_scale: f32,
    panel: (f32, f32),
    /// How much bigger this rendering is than the working area — 1.0 on
    /// screen, the export scale on a plate. See `plate_ink`.
    ink: f64,
}

impl crate::helpers::annotation_render::AnnotationSurface for CubeAnnotationSurface {
    fn project(&self, anchor: &crate::models::annotation::Anchor) -> Option<(f64, f64)> {
        use crate::models::annotation::Anchor;
        let voxel = match *anchor {
            Anchor::Data { x, y, z } => (x, y, z),
            // A FITS image pixel or a sky position means nothing in a cube's
            // voxel space; skipped rather than guessed at.
            _ => return None,
        };
        crate::helpers::cube_axes::project_voxel(
            &self.view_proj,
            self.dims,
            self.spectral_scale,
            voxel,
            self.panel,
        )
        .map(|(x, y)| (x as f64, y as f64))
    }

    fn ink_scale(&self) -> f64 {
        self.ink
    }

    fn units_to_pixels(&self, anchor: &crate::models::annotation::Anchor) -> f64 {
        use crate::models::annotation::Anchor;
        let Anchor::Data { x, y, z } = *anchor else {
            return 1.0;
        };
        // Measured, not derived: project the voxel and one a step along X, and
        // take the distance. Perspective makes the answer depend on where in
        // the cube the mark is, so a single global scale would size a mark at
        // the back the same as one at the front.
        let here = crate::helpers::cube_axes::project_voxel(
            &self.view_proj,
            self.dims,
            self.spectral_scale,
            (x, y, z),
            self.panel,
        );
        let along = crate::helpers::cube_axes::project_voxel(
            &self.view_proj,
            self.dims,
            self.spectral_scale,
            (x + 1.0, y, z),
            self.panel,
        );
        match (here, along) {
            (Some(a), Some(b)) => {
                let d = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt() as f64;
                // A voxel seen edge-on projects to almost nothing; a shape of
                // zero size is invisible, so keep a floor.
                d.max(0.25)
            }
            _ => 1.0,
        }
    }
}

#[cfg(test)]
mod plate_ink_tests {
    use super::plate_ink;

    /// The plate is the screen at a different resolution, and says so.
    ///
    /// The bug: a cube figure exported at 4x scaled its own title, caption and
    /// colorbar and left every mark at its screen size, so the annotations were
    /// the one thing in the picture that shrank.
    #[test]
    fn a_bigger_plate_asks_for_bigger_marks() {
        assert_eq!(plate_ink(800, 800), 1.0, "a plate the size of the view");
        assert_eq!(plate_ink(3200, 800), 4.0, "a plate four times the view");
        assert_eq!(plate_ink(400, 800), 0.5, "a plate half the view");
    }

    /// Every plate surface is told how big the plate is.
    ///
    /// The arithmetic being right is worth nothing if a construction site
    /// leaves the field at a literal — which is exactly the shape the bug had:
    /// the number existed, and the marks were drawn without it. Two sites, and
    /// they are two because the cube exports a volume and a slice.
    #[test]
    fn every_plate_surface_is_told_how_big_the_plate_is() {
        let source = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/src/ui/cube_viewer.rs"
        ))
        .expect("this file is readable");
        let code = crate::testing::code(&source);
        // Only the non-test half: the tests below construct surfaces with
        // literal factors on purpose.
        let code = &code[..code.find("#[cfg(test)]").unwrap_or(code.len())];
        for built in ["CubeAnnotationSurface {", "SlicePlateSurface::new("] {
            let at = code
                .find(built)
                .unwrap_or_else(|| panic!("{built} is gone"));
            let block = &code[at..(at + 400).min(code.len())];
            assert!(
                block.contains("plate_ink"),
                "{built} is built without plate_ink, so its marks keep their \
                 screen size however big the figure is"
            );
        }
    }

    /// An unrealised working area is zero pixels wide.
    ///
    /// A probe, or an agent asking before the window is mapped. Dividing by it
    /// would put every mark at infinity — drawn nowhere, with nothing reporting
    /// a problem — so the screen's own numbers are the answer.
    #[test]
    fn a_view_with_no_size_falls_back_to_the_screens_look() {
        assert_eq!(plate_ink(1024, 0), 1.0);
        assert_eq!(plate_ink(0, 800), 1.0);
        assert_eq!(plate_ink(-1, 800), 1.0);
        assert_eq!(plate_ink(1024, -1), 1.0);
    }
}

#[cfg(test)]
mod slice_plate_tests {
    use super::SlicePlateSurface;
    use crate::helpers::annotation_render::AnnotationSurface;
    use crate::models::annotation::Anchor;

    /// A plate surface reports the factor it was built with.
    ///
    /// It is the only thing between `plate_ink` and the renderer, and a field
    /// stored and never read is exactly the shape of this whole bug.
    #[test]
    fn the_plate_surface_passes_its_ink_scale_on() {
        let s = SlicePlateSurface::new((64, 64), (1024, 1024), 0, 4.0);
        assert_eq!(s.ink_scale(), 4.0);
        let screen = SlicePlateSurface::new((64, 64), (64, 64), 0, 1.0);
        assert_eq!(screen.ink_scale(), 1.0);
    }

    fn data(x: f64, y: f64, z: f64) -> Anchor {
        Anchor::Data { x, y, z }
    }

    /// A voxel lands at the same fraction across the plate as across the plane.
    ///
    /// The export scales the whole plane to the plate, so this is the only
    /// thing that has to hold — and it is what puts a mark on the feature it
    /// was drawn on rather than beside it.
    #[test]
    fn a_voxel_lands_at_the_same_fraction_of_the_plate() {
        let s = SlicePlateSurface::new((64, 32), (1024, 768), 5, 1.0);
        assert_eq!(s.project(&data(0.0, 0.0, 5.0)), Some((0.0, 0.0)));
        assert_eq!(s.project(&data(32.0, 16.0, 5.0)), Some((512.0, 384.0)));
        assert_eq!(s.project(&data(64.0, 32.0, 5.0)), Some((1024.0, 768.0)));
    }

    /// Only the channel the plate was rendered from appears on it.
    ///
    /// The plate is one plane. A mark from another channel is not at that
    /// position in this picture, and exporting it there would put a claim in a
    /// figure that someone then publishes.
    #[test]
    fn a_mark_from_another_channel_is_not_on_the_plate() {
        let s = SlicePlateSurface::new((64, 32), (1024, 768), 5, 1.0);
        assert!(s.project(&data(10.0, 10.0, 5.0)).is_some());
        assert!(s.project(&data(10.0, 10.0, 4.0)).is_none());
        assert!(s.project(&data(10.0, 10.0, 6.0)).is_none());
    }

    /// Anchors from another viewer's space are skipped, not guessed at.
    #[test]
    fn only_voxel_anchors_reach_the_plate() {
        let s = SlicePlateSurface::new((64, 32), (1024, 768), 0, 1.0);
        assert!(s
            .project(&Anchor::Sky {
                ra_deg: 202.0,
                dec_deg: 47.0
            })
            .is_none());
        assert!(s.project(&Anchor::ImagePixel { x: 1.0, y: 1.0 }).is_none());
    }

    /// A mark scales with the plate, so exporting bigger does not shrink it.
    ///
    /// The size is in voxels; at 16x the plate, a mark is 16x the pixels. A
    /// scale that ignored the plate size would draw a ring the same number of
    /// pixels across on a 4096px plate as on a 256px one, which on the big one
    /// is a speck.
    #[test]
    fn a_mark_grows_with_the_plate() {
        let small = SlicePlateSurface::new((64, 64), (64, 64), 0, 1.0);
        let big = SlicePlateSurface::new((64, 64), (1024, 1024), 0, 1.0);
        let a = data(0.0, 0.0, 0.0);
        assert!((small.units_to_pixels(&a) - 1.0).abs() < 1e-9);
        assert!((big.units_to_pixels(&a) - 16.0).abs() < 1e-9);
    }

    /// A stretched plate takes the middle of the two scales, and never zero.
    #[test]
    fn a_stretched_plate_splits_the_difference() {
        // 4x across, 1x down: the geometric mean is 2.
        let s = SlicePlateSurface::new((64, 64), (256, 64), 0, 1.0);
        let a = data(0.0, 0.0, 0.0);
        assert!((s.units_to_pixels(&a) - 2.0).abs() < 1e-9);
        // A degenerate plate must not give a zero-size mark.
        let d = SlicePlateSurface::new((64, 64), (0, 0), 0, 1.0);
        assert!(d.units_to_pixels(&a) > 0.0);
    }
}
