//! Tabbed host for the 3D Cube Viewer.
//!
//! Rust port of `Views/CubeViewer/CubeTabHost.xaml(.cs)`. Each tab is a
//! self-contained [`CubeViewer`] (its own GL ray-marcher + slice fallback +
//! controls). Cubes open into new [`adw::TabView`] pages, close with the per-tab
//! ✕, and an empty-state prompt — with a persisted "recent cubes" list from
//! [`RecentCubesService`] — is shown whenever no tab is open. Mirrors the FITS
//! viewer's tabbed approach ([`crate::ui::fits_viewer`]) but uses libadwaita's
//! `TabView`/`TabBar` for the tab strip.

use crate::helpers::cube_loader;
use crate::helpers::cube_wcs::CubeWcs;
use crate::models::volume_data::VolumeData;
use crate::services::recent_cubes_service::RecentCubesService;
use crate::state::AppServices;
use crate::ui::cube_viewer::CubeViewer;
use base64::Engine as _;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use libadwaita as adw;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

/// Top-level widget owning the toolbar, the tab strip, and every open cube.
pub struct CubeTabHost {
    /// Root widget exposed to `main_window`.
    pub widget: gtk::Box,
    /// libadwaita tab container (one page per open cube).
    tab_view: adw::TabView,
    /// Live [`CubeViewer`]s, parallel to the tab pages (kept alive here so their
    /// self-referential signal closures don't drop while the tab is open).
    viewers: Rc<RefCell<Vec<Rc<CubeViewer>>>>,
    /// Persistent recent-cubes store (surfaced in the empty state).
    recents: RecentCubesService,
    /// Stack switching between the empty state and the tab strip.
    content_stack: gtk::Stack,
    /// Toast overlay for load/other errors.
    toast_overlay: adw::ToastOverlay,
    /// The recents section container (hidden when there are no recents).
    recents_section: gtk::Box,
    /// The list box the recents rows live in.
    recents_list: gtk::ListBox,
    /// Paths backing the recents rows (row index → path).
    recents_paths: RefCell<Vec<PathBuf>>,
    /// App services (unused today; retained for parity/future MCP control).
    #[allow(dead_code)]
    services: Arc<AppServices>,
}

impl CubeTabHost {
    pub fn new(services: Arc<AppServices>) -> Rc<Self> {
        // ── Root ─────────────────────────────────────────────────────────────
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        widget.set_vexpand(true);
        widget.set_hexpand(true);

        // ── Toolbar ──────────────────────────────────────────────────────────
        let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        toolbar.add_css_class("toolbar");

        let open_btn = gtk::Button::new();
        let open_content = adw::ButtonContent::new();
        open_content.set_icon_name("document-open-symbolic");
        open_content.set_label(crate::tr_en!("Open Cube…"));
        open_btn.set_child(Some(&open_content));
        open_btn.add_css_class("suggested-action");
        open_btn.set_tooltip_text(Some(crate::tr_en!("Open a FITS spectral cube (NAXIS≥3)")));
        toolbar.append(&open_btn);

        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        toolbar.append(&spacer);

        let title = gtk::Label::new(Some(crate::tr_en!("Cube Viewer")));
        title.add_css_class("dim-label");
        title.add_css_class("caption");
        toolbar.append(&title);

        widget.append(&toolbar);
        widget.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

        // ── Content stack: empty state ↔ tab strip ───────────────────────────
        let content_stack = gtk::Stack::new();
        content_stack.set_transition_type(gtk::StackTransitionType::Crossfade);
        content_stack.set_vexpand(true);
        content_stack.set_hexpand(true);

        // Empty state
        let (empty_page, empty_open_btn, recents_section, recents_list) = build_empty_state();
        content_stack.add_named(&empty_page, Some("empty"));

        // Tab strip: an adw::TabBar above the adw::TabView.
        let tab_view = adw::TabView::new();
        tab_view.set_vexpand(true);
        tab_view.set_hexpand(true);

        let tab_bar = adw::TabBar::new();
        tab_bar.set_view(Some(&tab_view));
        tab_bar.set_autohide(false);

        let tabs_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        tabs_box.set_vexpand(true);
        tabs_box.set_hexpand(true);
        tabs_box.append(&tab_bar);
        tabs_box.append(&tab_view);
        content_stack.add_named(&tabs_box, Some("tabs"));

        content_stack.set_visible_child_name("empty");

        // Wrap in a toast overlay so load errors are visible in-widget.
        let toast_overlay = adw::ToastOverlay::new();
        toast_overlay.set_child(Some(&content_stack));
        toast_overlay.set_vexpand(true);
        toast_overlay.set_hexpand(true);
        widget.append(&toast_overlay);

        let host = Rc::new(CubeTabHost {
            widget,
            tab_view,
            viewers: Rc::new(RefCell::new(Vec::new())),
            recents: RecentCubesService::new(),
            content_stack,
            toast_overlay,
            recents_section,
            recents_list,
            recents_paths: RefCell::new(Vec::new()),
            services,
        });

        // ── Wire signals ─────────────────────────────────────────────────────
        // Toolbar "Open Cube…"
        {
            let h = host.clone();
            open_btn.connect_clicked(move |btn| {
                let h = h.clone();
                let parent = btn.clone().upcast::<gtk::Widget>();
                glib::spawn_future_local(async move {
                    h.open_dialog(parent).await;
                });
            });
        }
        // Empty-state "Open cube…"
        {
            let h = host.clone();
            empty_open_btn.connect_clicked(move |btn| {
                let h = h.clone();
                let parent = btn.clone().upcast::<gtk::Widget>();
                glib::spawn_future_local(async move {
                    h.open_dialog(parent).await;
                });
            });
        }
        // Recents row → open
        {
            let h = host.clone();
            host.recents_list.connect_row_activated(move |_, row| {
                let idx = row.index() as usize;
                let path = h.recents_paths.borrow().get(idx).cloned();
                if let Some(path) = path {
                    h.open_path(&path);
                }
            });
        }
        // Per-tab ✕ → clean up the backing viewer and finish the close.
        {
            let viewers = host.viewers.clone();
            host.tab_view.connect_close_page(move |view, page| {
                let child = page.child();
                viewers
                    .borrow_mut()
                    .retain(|v| v.widget().clone().upcast::<gtk::Widget>() != child);
                view.close_page_finish(page, true);
                glib::Propagation::Stop
            });
        }
        // Page count → toggle empty state (and refresh recents when emptied).
        {
            let h = host.clone();
            host.tab_view.connect_n_pages_notify(move |tv| {
                if tv.n_pages() == 0 {
                    h.refresh_recents();
                    h.content_stack.set_visible_child_name("empty");
                } else {
                    h.content_stack.set_visible_child_name("tabs");
                }
            });
        }

        host.refresh_recents();
        host
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.widget
    }

    /// Handle a live MCP viewer command (`op` + JSON `args`) against the open cube.
    /// Runs on the GTK main thread; reads/mutates the live viewer and returns JSON.
    /// Ops: `open_cube`, `get_cube_view`, `set_cube_view`, `probe_cube_spectrum`,
    /// `export_cube_figure`. Ops needing an open cube return `Err("no cube open")`.
    pub async fn handle_viewer_command(
        self: &Rc<Self>,
        op: &str,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        use serde_json::json;
        match op {
            // Load a cube path (reuses the tabbed loader; returns immediately while
            // the decode runs on a worker thread).
            "open_cube" => {
                let path = crate::mcp::tools::arg(args, "path")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| "open_cube requires a 'path' string".to_string())?;
                self.open_path(std::path::Path::new(path));
                Ok(json!({ "opened": true, "path": path }))
            }
            // Read the active cube's 3D view parameters + dims.
            "get_cube_view" => {
                let v = self
                    .active_viewer()
                    .ok_or_else(|| "no cube open".to_string())?;
                Ok(view_json(&v))
            }
            // Mutate any subset of the active cube's view parameters. Mirrors the
            // reachable half of Windows `ApplyCubeView`: the GL-only volume controls
            // (camera + reset, quality steps, spectral stretch, MIP/render-mode,
            // density, background preset, idle auto-orbit) plus the slice-plane
            // channel. Colormap / stretch / window / mode / slice-plane + caption
            // toggles are intentionally NOT applied here: in the UI they drive BOTH
            // the GL volume AND the 2D slice view + colorbar, which requires
            // `CubeViewer` accessors this module cannot reach (see the parity note in
            // the returning summary).
            "set_cube_view" => {
                let v = self
                    .active_viewer()
                    .ok_or_else(|| "no cube open".to_string())?;

                // Camera: reset first (as ApplyCubeView does), then apply any
                // az/el/dist overrides on top. set_camera re-applies the interactive
                // clamps (el ±1.4, dist 0.5–8) so an agent can't push an invalid pose.
                if crate::mcp::tools::arg(args, "reset_camera")
                    .and_then(|x| x.as_bool())
                    .unwrap_or(false)
                {
                    v.gl().reset_view();
                }
                let (mut az, mut el, mut dist) = v.gl().camera();
                if let Some(x) = crate::mcp::tools::arg(args, "az").and_then(|x| x.as_f64()) {
                    az = x as f32;
                }
                if let Some(x) = crate::mcp::tools::arg(args, "el").and_then(|x| x.as_f64()) {
                    el = x as f32;
                }
                if let Some(x) = crate::mcp::tools::arg(args, "dist").and_then(|x| x.as_f64()) {
                    dist = x as f32;
                }
                v.gl().set_camera(az, el, dist);

                if let Some(x) = crate::mcp::tools::arg(args, "steps").and_then(|x| x.as_f64()) {
                    v.gl().set_steps(x as f32);
                }
                if let Some(x) =
                    crate::mcp::tools::arg(args, "spectralScale").and_then(|x| x.as_f64())
                {
                    v.gl().set_spectral_scale(x as f32);
                }
                if let Some(x) = crate::mcp::tools::arg(args, "density").and_then(|x| x.as_f64()) {
                    v.gl().set_density(x as f32);
                }
                // MIP: an explicit `mip` bool wins; otherwise derive it from a
                // `render_mode` string ("max-intensity"/"mip" → on, else off),
                // matching ApplyCubeView's RenderModeCombo mapping.
                if let Some(on) = crate::mcp::tools::arg(args, "mip").and_then(|x| x.as_bool()) {
                    v.gl().set_mip(on);
                } else if let Some(mode) =
                    crate::mcp::tools::arg(args, "renderMode").and_then(|x| x.as_str())
                {
                    let on = mode.to_ascii_lowercase().contains("max")
                        || mode.eq_ignore_ascii_case("mip");
                    v.gl().set_mip(on);
                }
                // Background preset (Dark / Black / Light) — the exact RGB the
                // Background dropdown applies; unknown names fall back to Dark.
                if let Some(bg) =
                    crate::mcp::tools::arg(args, "background").and_then(|x| x.as_str())
                {
                    let rgb = match bg.trim().to_ascii_lowercase().as_str() {
                        "black" => [0.0, 0.0, 0.0],
                        "light" => [0.92, 0.92, 0.94],
                        _ => [0.06, 0.06, 0.08], // dark (default)
                    };
                    v.gl().set_background(rgb);
                }
                if let Some(on) =
                    crate::mcp::tools::arg(args, "autoOrbit").and_then(|x| x.as_bool())
                {
                    v.gl().set_auto_orbit(on);
                }
                if let Some(x) = crate::mcp::tools::arg(args, "channel").and_then(|x| x.as_u64()) {
                    v.set_current_channel(x as usize);
                }
                Ok(view_json(&v))
            }
            // Sample the spectrum through voxel column (x, y) across all channels.
            "probe_cube_spectrum" => {
                let v = self
                    .active_viewer()
                    .ok_or_else(|| "no cube open".to_string())?;
                let x = crate::mcp::tools::arg(args, "x")
                    .and_then(|x| x.as_u64())
                    .ok_or_else(|| "probe_cube_spectrum requires an integer 'x'".to_string())?
                    as usize;
                let y = crate::mcp::tools::arg(args, "y")
                    .and_then(|x| x.as_u64())
                    .ok_or_else(|| "probe_cube_spectrum requires an integer 'y'".to_string())?
                    as usize;
                let spectrum = v
                    .spectrum_at(x, y)
                    .ok_or_else(|| format!("pixel ({x}, {y}) is outside the cube"))?;
                let samples: Vec<serde_json::Value> = spectrum
                    .iter()
                    .enumerate()
                    .map(|(z, (normalized, physical))| {
                        json!({ "channel": z, "value": normalized, "physical": physical })
                    })
                    .collect();
                Ok(json!({
                    "x": x,
                    "y": y,
                    "channels": spectrum.len(),
                    "unit": v.value_unit(),
                    "spectrum": samples,
                }))
            }
            // Render the current view (3D volume or 2D slice) to a figure. With a
            // `path`, write it straight to disk as PNG or PDF and return the path
            // (mirrors ExportCubeToPathAsync); without a `path`, return the PNG as
            // base64 (the existing behavior). `scale` multiplies the base
            // width/height so an agent can pull a higher-resolution plate.
            //
            // NOTE: this writes the raw rendered frame. The Windows path composes the
            // *styled plate* (header band, WCS caption, colorbar, metadata footer,
            // dark/light theme). That composer (`cube_export::PlateSpec::compose`) and
            // its inputs live behind private `CubeViewer` state, so a faithful themed
            // plate export needs the accessors listed in the returning summary.
            "export_cube_figure" => {
                let v = self
                    .active_viewer()
                    .ok_or_else(|| "no cube open".to_string())?;
                let scale = crate::mcp::tools::arg(args, "scale")
                    .and_then(|x| x.as_u64())
                    .unwrap_or(1)
                    .clamp(1, 4) as i32;
                let base_w = crate::mcp::tools::arg(args, "width")
                    .and_then(|x| x.as_u64())
                    .unwrap_or(1024)
                    .clamp(16, 4096) as i32;
                let base_h = crate::mcp::tools::arg(args, "height")
                    .and_then(|x| x.as_u64())
                    .unwrap_or(768)
                    .clamp(16, 4096) as i32;
                let width = (base_w * scale).clamp(16, 8192);
                let height = (base_h * scale).clamp(16, 8192);
                let transparent = crate::mcp::tools::arg(args, "transparent")
                    .and_then(|x| x.as_bool())
                    .unwrap_or(false);

                // ── File-path export (PNG / PDF) ──────────────────────────────────
                if let Some(path_str) = crate::mcp::tools::arg(args, "path")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                {
                    let path = std::path::Path::new(path_str);
                    if !path.is_absolute() {
                        return Err("path must be a full (absolute) file path".to_string());
                    }
                    let ext = path
                        .extension()
                        .and_then(|e| e.to_str())
                        .map(|s| s.to_ascii_lowercase());
                    // Format: explicit `format` wins; else infer from the extension.
                    let fmt = crate::mcp::tools::arg(args, "format")
                        .and_then(|v| v.as_str())
                        .map(|s| s.trim().to_ascii_lowercase())
                        .unwrap_or_else(|| ext.clone().unwrap_or_else(|| "png".to_string()));
                    let pdf = fmt == "pdf";
                    // Enforce a matching extension, exactly like ExportCubeToPathAsync.
                    if pdf && ext.as_deref() != Some("pdf") {
                        return Err("path must end in .pdf for a PDF export".to_string());
                    }
                    if !pdf && ext.as_deref() != Some("png") {
                        return Err("path must end in .png for a PNG export".to_string());
                    }
                    let rgba = v.render_figure(width, height, transparent).ok_or_else(|| {
                        "cube figure could not be rendered (GL unavailable)".to_string()
                    })?;
                    let res = if pdf {
                        crate::helpers::pdf_writer::write_pdf(path, width, height, &rgba)
                    } else {
                        crate::helpers::pdf_writer::write_png(path, width, height, &rgba)
                    };
                    res.map_err(|e| format!("export failed: {e}"))?;
                    return Ok(json!({
                        "path": path_str,
                        "format": if pdf { "pdf" } else { "png" },
                        "width": width,
                        "height": height,
                        "scale": scale,
                        "transparent": transparent,
                    }));
                }

                // ── Base64 export (no path) ───────────────────────────────────────
                let rgba = v.render_figure(width, height, transparent).ok_or_else(|| {
                    "cube figure could not be rendered (GL unavailable)".to_string()
                })?;
                let png = encode_png_bytes(width, height, &rgba)?;
                let image_base64 = base64::engine::general_purpose::STANDARD.encode(&png);
                Ok(json!({
                    "width": width,
                    "height": height,
                    "scale": scale,
                    "transparent": transparent,
                    "imageBase64": image_base64,
                }))
            }
            _ => Err(format!("cube viewer op '{op}' is not supported")),
        }
    }

    /// The [`CubeViewer`] backing the currently selected tab, if any.
    fn active_viewer(&self) -> Option<Rc<CubeViewer>> {
        let page = self.tab_view.selected_page()?;
        let child = page.child();
        self.viewers
            .borrow()
            .iter()
            .find(|v| v.widget().clone().upcast::<gtk::Widget>() == child)
            .cloned()
    }

    /// Load a cube from `path` OFF the UI thread (cfitsio decode can take seconds
    /// on a large cube), showing a spinner tab until it's ready, then swap in the
    /// viewer — or a toast on failure.
    pub fn open_path(self: &Rc<Self>, path: &Path) {
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| path.display().to_string());

        // A loading placeholder page (spinner + label).
        let loading = gtk::Box::new(gtk::Orientation::Vertical, 12);
        loading.set_valign(gtk::Align::Center);
        loading.set_halign(gtk::Align::Center);
        loading.set_vexpand(true);
        let spinner = gtk::Spinner::new();
        spinner.set_size_request(48, 48);
        spinner.start();
        loading.append(&spinner);
        let load_label = gtk::Label::new(Some(&crate::tr_fmt!("Loading {}…", name)));
        load_label.add_css_class("dim-label");
        loading.append(&load_label);

        let loading_page = self.tab_view.append(&loading);
        loading_page.set_title(&name);
        loading_page.set_tooltip(&path.display().to_string());
        self.tab_view.set_selected_page(&loading_page);
        self.content_stack.set_visible_child_name("tabs");

        // Decode on a worker thread; bridge the result back to the GTK loop.
        let (tx, rx) = tokio::sync::oneshot::channel::<Result<(VolumeData, CubeWcs), String>>();
        let path_buf = path.to_path_buf();
        std::thread::spawn(move || {
            let result = (|| {
                let vol = cube_loader::load_cube(&path_buf, None)?;
                // WCS is best-effort: a header read failure still yields a usable viewer.
                let wcs = cube_loader::cube_header(&path_buf)
                    .map(|h| CubeWcs::from_header(&h))
                    .unwrap_or_else(|_| CubeWcs::from_header(&std::collections::HashMap::new()));
                Ok((vol, wcs))
            })();
            let _ = tx.send(result);
        });

        let this = self.clone();
        let path_for_viewer = path.to_path_buf();
        let name2 = name.clone();
        let loading_weak = loading_page.downgrade();
        glib::spawn_future_local(async move {
            let outcome = rx.await;
            if let Some(page) = loading_weak.upgrade() {
                this.tab_view.close_page(&page);
            }
            match outcome {
                Ok(Ok((vol, wcs))) => {
                    let viewer = CubeViewer::new(vol, wcs, name2.clone());
                    viewer.set_source_path(&path_for_viewer); // native-res slice source
                    let page = this.tab_view.append(viewer.widget());
                    page.set_title(&name2);
                    page.set_tooltip(&path_for_viewer.display().to_string());
                    this.viewers.borrow_mut().push(viewer);
                    this.tab_view.set_selected_page(&page);
                    this.recents.add(&path_for_viewer);
                    this.refresh_recents();
                }
                Ok(Err(e)) => {
                    this.toast_overlay
                        .add_toast(adw::Toast::new(&crate::tr_fmt!(
                            "Failed to load cube: {}",
                            e
                        )));
                }
                Err(_) => {}
            }
        });
    }

    /// Open a file picker and load the chosen cube.
    async fn open_dialog(self: Rc<Self>, parent: gtk::Widget) {
        let root = parent.root().and_downcast::<gtk::Window>();

        let filter = gtk::FileFilter::new();
        filter.set_name(Some(crate::tr_en!("FITS Cubes")));
        for pat in ["*.fits", "*.FITS", "*.fit", "*.fts", "*.fz"] {
            filter.add_pattern(pat);
        }
        let filters = gtk::gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);

        let dialog = gtk::FileDialog::builder()
            .title(crate::tr_en!("Open Cube"))
            .modal(true)
            .filters(&filters)
            .build();

        if let Ok(file) = dialog.open_future(root.as_ref()).await {
            if let Some(path) = file.path() {
                self.open_path(&path);
            }
        }
    }

    /// Rebuild the empty-state recents list from the persisted store. Entries
    /// whose file no longer exists are already filtered out by [`RecentCubesService::list`].
    fn refresh_recents(&self) {
        while let Some(child) = self.recents_list.first_child() {
            self.recents_list.remove(&child);
        }

        let paths = self.recents.list();
        self.recents_section.set_visible(!paths.is_empty());

        for path in &paths {
            let name = path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| path.display().to_string());

            let row = gtk::ListBoxRow::new();
            let row_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            row_box.set_margin_start(8);
            row_box.set_margin_end(8);
            row_box.set_margin_top(6);
            row_box.set_margin_bottom(6);

            let icon = gtk::Image::from_icon_name("image-x-generic-symbolic");
            row_box.append(&icon);

            let label = gtk::Label::new(Some(&name));
            label.set_halign(gtk::Align::Start);
            label.set_hexpand(true);
            label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
            row_box.append(&label);

            row.set_child(Some(&row_box));
            row.set_tooltip_text(Some(&path.display().to_string()));
            self.recents_list.append(&row);
        }

        *self.recents_paths.borrow_mut() = paths;
    }
}

// ---------------------------------------------------------------------------
// Live MCP helpers
// ---------------------------------------------------------------------------

/// The live 3D view parameters + cube dims of a viewer, as the shared JSON shape
/// returned by both `get_cube_view` and `set_cube_view`.
fn view_json(v: &CubeViewer) -> serde_json::Value {
    let (az, el, dist) = v.gl().camera();
    let (nx, ny, nz) = v.dims();
    serde_json::json!({
        "az": az,
        "el": el,
        "dist": dist,
        "steps": v.gl().steps(),
        "spectralScale": v.gl().spectral_scale(),
        "channel": v.current_channel(),
        "unit": v.value_unit(),
        "dims": { "nx": nx, "ny": ny, "nz": nz },
    })
}

/// PNG-encode a straight-alpha RGBA8 buffer (`width*height*4`, top-down) to bytes
/// via cairo — the in-memory sibling of [`crate::helpers::pdf_writer::write_png`].
/// cairo's `ARgb32` surface is premultiplied BGRA in native-endian order, so we
/// premultiply + channel-swap while packing.
fn encode_png_bytes(width: i32, height: i32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    use cairo::{Format, ImageSurface};
    if width <= 0 || height <= 0 {
        return Err(format!("invalid image dimensions {width}x{height}"));
    }
    let (w, h) = (width as usize, height as usize);
    let need = w
        .checked_mul(h)
        .and_then(|n| n.checked_mul(4))
        .ok_or_else(|| "image dimensions overflow".to_string())?;
    if rgba.len() < need {
        return Err(format!("rgba buffer too small: {} < {}", rgba.len(), need));
    }
    let stride = Format::ARgb32
        .stride_for_width(width as u32)
        .map_err(|e| format!("cairo stride error: {e}"))? as usize;
    let mut data = vec![0u8; stride * h];
    for y in 0..h {
        let row_src = y * w * 4;
        let row_dst = y * stride;
        for x in 0..w {
            let s = row_src + x * 4;
            let d = row_dst + x * 4;
            let (r, g, b, a) = (rgba[s], rgba[s + 1], rgba[s + 2], rgba[s + 3]);
            let pm = |c: u8| ((c as u16 * a as u16 + 127) / 255) as u8;
            let (pr, pg, pb) = if a == 255 {
                (r, g, b)
            } else {
                (pm(r), pm(g), pm(b))
            };
            // Little-endian ARgb32 (0xAARRGGBB) => bytes B, G, R, A.
            data[d] = pb;
            data[d + 1] = pg;
            data[d + 2] = pr;
            data[d + 3] = a;
        }
    }
    let surface = ImageSurface::create_for_data(data, Format::ARgb32, width, height, stride as i32)
        .map_err(|e| format!("cairo surface error: {e}"))?;
    let mut buf: Vec<u8> = Vec::new();
    surface
        .write_to_png(&mut buf)
        .map_err(|e| format!("PNG encode failed: {e}"))?;
    Ok(buf)
}

// ---------------------------------------------------------------------------
// Empty-state widget
// ---------------------------------------------------------------------------

/// Build the "no cube open" placeholder. Returns the page plus the open button,
/// the recents section container, and the recents list box (wired by the host).
fn build_empty_state() -> (gtk::Box, gtk::Button, gtk::Box, gtk::ListBox) {
    let page = gtk::Box::new(gtk::Orientation::Vertical, 14);
    page.set_vexpand(true);
    page.set_valign(gtk::Align::Center);
    page.set_halign(gtk::Align::Center);
    page.set_margin_top(48);
    page.set_margin_bottom(48);

    let icon = gtk::Image::from_icon_name("view-fullscreen-symbolic");
    icon.set_pixel_size(56);
    icon.add_css_class("dim-label");
    page.append(&icon);

    let title = gtk::Label::new(Some(crate::tr_en!("No cube open")));
    title.add_css_class("title-2");
    page.append(&title);

    let body = gtk::Label::new(Some(crate::tr_en!(
        "Open a FITS spectral cube (NAXIS≥3) to explore it in 3D — orbit the \
         volume, scrub channels, probe spectra, and export figures."
    )));
    body.add_css_class("dim-label");
    body.set_justify(gtk::Justification::Center);
    body.set_wrap(true);
    body.set_max_width_chars(48);
    page.append(&body);

    let open_btn = gtk::Button::with_label(crate::tr_en!("Open cube…"));
    open_btn.set_icon_name("document-open-symbolic");
    open_btn.add_css_class("suggested-action");
    open_btn.add_css_class("pill");
    open_btn.set_halign(gtk::Align::Center);
    page.append(&open_btn);

    // Recents section (hidden until populated).
    let recents_section = gtk::Box::new(gtk::Orientation::Vertical, 6);
    recents_section.set_margin_top(12);
    recents_section.set_visible(false);

    let recents_header = gtk::Label::new(Some(crate::tr_en!("Recent cubes")));
    recents_header.add_css_class("heading");
    recents_header.set_halign(gtk::Align::Center);
    recents_section.append(&recents_header);

    let recents_list = gtk::ListBox::new();
    recents_list.add_css_class("boxed-list");
    recents_list.set_selection_mode(gtk::SelectionMode::None);
    recents_list.set_width_request(420);
    recents_section.append(&recents_list);

    page.append(&recents_section);

    (page, open_btn, recents_section, recents_list)
}
