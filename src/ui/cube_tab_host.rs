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
use crate::ui::cube_viewer::CubeViewer;
use base64::Engine as _;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use libadwaita as adw;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

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
}

/// Push the open cube tabs + active index into the MCP view state.
///
/// See `fits_viewer::publish_fits_tabs` — `list_open_tabs` had no publisher at
/// all, so it reported nothing regardless of what was open.
fn publish_cube_tabs(tab_view: &adw::TabView, viewers: &Rc<RefCell<Vec<Rc<CubeViewer>>>>) {
    let paths: Vec<String> = viewers
        .borrow()
        .iter()
        .map(|v| v.name().to_string())
        .collect();
    let active = tab_view
        .selected_page()
        .map(|p| tab_view.page_position(&p) as usize)
        .filter(|i| *i < paths.len());
    crate::mcp::view_state::set_open_cubes(paths, active);
}

impl CubeTabHost {
    pub fn new() -> Rc<Self> {
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
                publish_cube_tabs(view, &viewers);
                glib::Propagation::Stop
            });
        }
        // Selection changes move the ACTIVE index, which `list_open_tabs` reports.
        {
            let viewers = host.viewers.clone();
            host.tab_view
                .connect_selected_page_notify(move |view| publish_cube_tabs(view, &viewers));
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
                // `opened: true` is returned unconditionally below, so a path
                // that could never open has to be refused here or the caller is
                // told it worked.
                crate::helpers::local_path::reject_remote(
                    path,
                    crate::helpers::local_path::FETCH_IT_FIRST,
                )?;
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

            // ── Annotations ─────────────────────────────────────────────
            "annotate_cube" => {
                use crate::models::annotation::{
                    Anchor, Annotation, AnnotationKind, Author, Extent,
                };
                let v = self
                    .active_viewer()
                    .ok_or_else(|| "no cube open".to_string())?;
                let num = |k: &str| crate::mcp::tools::arg(args, k).and_then(|x| x.as_f64());
                let (x, y) = match (num("x"), num("y")) {
                    (Some(x), Some(y)) => (x, y),
                    _ => {
                        return Err("give x and y (voxel coordinates) for where to draw".to_string())
                    }
                };
                // No channel given means the one on screen — the plane the user
                // is looking at, which is what "here" means to them.
                let z = num("z").unwrap_or_else(|| v.current_channel() as f64);
                let kind = crate::mcp::tools::arg(args, "kind")
                    .and_then(|k| k.as_str())
                    .map(|k| {
                        AnnotationKind::parse(k).ok_or_else(|| {
                            format!("'{k}' is not a kind — use rect, circle, callout or text")
                        })
                    })
                    .transpose()?
                    .unwrap_or(AnnotationKind::Circle);
                let text = crate::mcp::tools::arg(args, "text")
                    .and_then(|t| t.as_str())
                    .unwrap_or("")
                    .to_string();

                let mut mark = Annotation::new(kind, Anchor::Data { x, y, z }, text, Author::Agent);
                if let Some(r) = num("radius") {
                    mark = mark.with_extent(Extent::square(r));
                }
                mark.validate()?;

                let target = v.source_file();
                let mut current = v.annotations();
                current.push(mark.clone());
                v.set_annotations(current.clone());
                let saved = crate::helpers::annotation_store::save_for(&target, &current).is_ok();

                Ok(json!({
                    "id": mark.id,
                    "kind": mark.kind.as_str(),
                    "voxel": {"x": x, "y": y, "z": z},
                    "text": mark.text,
                    "total": current.len(),
                    "persisted": saved,
                }))
            }

            "remove_annotation" => {
                let id = crate::mcp::tools::arg(args, "id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let v = self
                    .active_viewer()
                    .ok_or_else(|| "no cube open".to_string())?;
                let mut current = v.annotations();
                let before = current.len();
                current.retain(|a| a.id != id);
                let removed = current.len() < before;
                if removed {
                    v.set_annotations(current.clone());
                    let target = v.source_file();
                    let _ = crate::helpers::annotation_store::save_for(&target, &current);
                }
                Ok(json!({ "removed": removed, "viewer": "cube", "remaining": current.len() }))
            }

            "clear_annotations" => {
                let v = self
                    .active_viewer()
                    .ok_or_else(|| "no cube open".to_string())?;
                let removed = v.annotations().len();
                v.set_annotations(Vec::new());
                let target = v.source_file();
                let _ = crate::helpers::annotation_store::save_for(&target, &[]);
                Ok(json!({
                    "cleared": removed,
                    "viewer": "cube",
                    "file": v.source_file(),
                    "note": "marks on other cube tabs are untouched",
                }))
            }

            // Correct a mark instead of destroying it and drawing another:
            // the id an agent has already quoted to someone stays valid. Cube
            // coordinates are voxels throughout, so a radius here needs no
            // unit conversion — unlike the FITS side, where it does.
            "update_annotation" => {
                let v = self
                    .active_viewer()
                    .ok_or_else(|| "no cube open".to_string())?;
                let id = crate::mcp::tools::arg(args, "id")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string();
                let num = |k: &str| crate::mcp::tools::arg(args, k).and_then(|x| x.as_f64());
                let mut current = v.annotations();
                let Some(mark) = current.iter_mut().find(|a| a.id == id) else {
                    return Err(format!(
                        "no annotation '{id}' on this cube — list_cube_annotations shows \
                         what is there"
                    ));
                };
                if let Some(text) = crate::mcp::tools::arg(args, "text").and_then(|t| t.as_str()) {
                    mark.text = text.trim().to_string();
                }
                // A voxel move keeps whichever coordinates were left out, so
                // "shift it two channels" does not also reset x and y.
                if num("x").is_some() || num("y").is_some() || num("z").is_some() {
                    let (cx, cy, cz) = match mark.anchor {
                        crate::models::annotation::Anchor::Data { x, y, z } => (x, y, z),
                        _ => (0.0, 0.0, 0.0),
                    };
                    mark.anchor = crate::models::annotation::Anchor::Data {
                        x: num("x").unwrap_or(cx),
                        y: num("y").unwrap_or(cy),
                        z: num("z").unwrap_or(cz),
                    };
                }
                if let Some(r) = num("radius") {
                    mark.extent = Some(crate::models::annotation::Extent::square(r));
                }
                mark.validate()?;
                let changed = mark.clone();
                v.set_annotations(current.clone());
                let saved =
                    crate::helpers::annotation_store::save_for(&v.source_file(), &current).is_ok();
                Ok(json!({
                    "id": changed.id,
                    "kind": changed.kind.as_str(),
                    "text": changed.text,
                    "anchor": changed.anchor,
                    "viewer": "cube",
                    "persisted": saved,
                }))
            }

            // Pick one mark out so a person looking at the cube can see WHICH
            // one is meant. No id takes the highlight away.
            "select_annotation" => {
                let v = self
                    .active_viewer()
                    .ok_or_else(|| "no cube open".to_string())?;
                let id = crate::mcp::tools::arg(args, "id")
                    .and_then(|x| x.as_str())
                    .map(str::trim)
                    .filter(|x| !x.is_empty());
                match id {
                    Some(id) => {
                        if !v.annotations().iter().any(|a| a.id == id) {
                            return Err(format!(
                                "no annotation '{id}' on this cube — list_cube_annotations \
                                 shows what is there"
                            ));
                        }
                        v.set_selected_annotation(Some(id.to_string()));
                    }
                    None => v.set_selected_annotation(None),
                }
                Ok(json!({ "selected": v.selected_annotation(), "viewer": "cube" }))
            }

            "list_cube_annotations" => {
                let v = self
                    .active_viewer()
                    .ok_or_else(|| "no cube open".to_string())?;
                let items: Vec<serde_json::Value> = v
                    .annotations()
                    .iter()
                    .map(|a| {
                        json!({
                            "id": a.id,
                            "kind": a.kind.as_str(),
                            "text": a.text,
                            "anchor": a.anchor,
                            "author": a.author.as_str(),
                            "createdAt": a.created_at,
                        })
                    })
                    .collect();
                Ok(json!({
                    "count": items.len(),
                    "selected": v.selected_annotation(),
                    "file": v.source_file(),
                    "annotations": items,
                }))
            }

            // SEE the working area — the volume WITH the axes overlay the user
            // reads it by, or the 2D slice when that is the visible mode.
            // `export_cube_figure` is an export and returns the render alone.
            "get_cube_image" => {
                let v = self
                    .active_viewer()
                    .ok_or_else(|| "no cube open".to_string())?;
                let (view_w, view_h) = v.working_area_size();
                let limits = crate::mcp::agent_image::ImageLimits::from_settings();
                let (w, h, on_screen) =
                    crate::mcp::agent_image::capture_size(view_w, view_h, limits);
                let png = v.capture_working_area_png(w, h)?;
                let image_base64 = {
                    use base64::Engine as _;
                    base64::engine::general_purpose::STANDARD.encode(&png)
                };
                Ok(json!({
                    "imageBase64": image_base64,
                    "imageMime": "image/png",
                    "width": w,
                    "height": h,
                    "viewWidth": view_w,
                    "viewHeight": view_h,
                    "viewportOnScreen": on_screen,
                    "scale": if view_w > 0 { f64::from(w) / f64::from(view_w) } else { 1.0 },
                    "view": view_json(&v),
                    "caption": "Cube working area",
                }))
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
                if let Some(x) = crate::mcp::tools::arg(args, "azimuth")
                    .or_else(|| crate::mcp::tools::arg(args, "az"))
                    .and_then(|x| x.as_f64())
                {
                    az = x as f32;
                }
                if let Some(x) = crate::mcp::tools::arg(args, "elevation")
                    .or_else(|| crate::mcp::tools::arg(args, "el"))
                    .and_then(|x| x.as_f64())
                {
                    el = x as f32;
                }
                if let Some(x) = crate::mcp::tools::arg(args, "distance")
                    .or_else(|| crate::mcp::tools::arg(args, "dist"))
                    .and_then(|x| x.as_f64())
                {
                    dist = x as f32;
                }
                v.gl().set_camera(az, el, dist);

                // Display controls the panel offers and MCP could not reach:
                // colormap, stretch, the window levels and their presets, and
                // the two overlay toggles. Every one of them is a control the
                // user can change, so "100% UI coverage" was not true for the
                // cube until now.
                if let Some(name) =
                    crate::mcp::tools::arg(args, "colormap").and_then(|x| x.as_str())
                {
                    if !v.set_colormap_by_name(name) {
                        return Err(format!("unknown colormap '{name}'"));
                    }
                }
                if let Some(name) = crate::mcp::tools::arg(args, "stretch").and_then(|x| x.as_str())
                {
                    if !v.set_stretch_by_name(name) {
                        return Err(format!("unknown stretch '{name}'"));
                    }
                }
                if let Some(preset) =
                    crate::mcp::tools::arg(args, "windowPreset").and_then(|x| x.as_str())
                {
                    if !v.set_window_preset(preset) {
                        return Err(format!(
                            "unknown windowPreset '{preset}' — use 'minmax' or 'p99'"
                        ));
                    }
                }
                {
                    let lo = crate::mcp::tools::arg(args, "windowLo").and_then(|x| x.as_f64());
                    let hi = crate::mcp::tools::arg(args, "windowHi").and_then(|x| x.as_f64());
                    if lo.is_some() || hi.is_some() {
                        v.set_window(lo, hi);
                    }
                }
                if let Some(on) =
                    crate::mcp::tools::arg(args, "showCaptions").and_then(|x| x.as_bool())
                {
                    v.set_captions_visible(on);
                }
                if let Some(on) =
                    crate::mcp::tools::arg(args, "showSlicePlane").and_then(|x| x.as_bool())
                {
                    v.set_slice_plane_visible(on);
                }
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
                    // Refused, not defaulted: the schema advertises three names,
                    // and quietly applying Dark for a fourth reports success for
                    // a change the caller never asked for.
                    let rgb = crate::ui::cube_volume_gl::background_rgb(bg).ok_or_else(|| {
                        format!(
                            "unknown background '{bg}' — use one of: {}",
                            crate::ui::cube_volume_gl::BACKGROUND_NAMES.join(", ")
                        )
                    })?;
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
            "switch_cube_tab" => {
                let index = crate::mcp::tools::arg(args, "index")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| "index is required".to_string())?
                    as usize;
                let count = self.tab_view.n_pages() as usize;
                if count == 0 {
                    return Err("no cubes are open".to_string());
                }
                if index >= count {
                    return Err(format!(
                        "no cube tab at index {index} ({count} open) — list_open_tabs shows them"
                    ));
                }
                let page = self.tab_view.nth_page(index as i32);
                self.tab_view.set_selected_page(&page);
                let v = self
                    .active_viewer()
                    .ok_or_else(|| "no cube open".to_string())?;
                let name = v.name();
                Ok(crate::mcp::tools::with_tab_switch_outcome(
                    // `name` is kept alongside the reference's `activeName`: it
                    // is the key every other cube payload uses for the same
                    // thing, and dropping it would make this one tool the odd
                    // one out.
                    json!({ "name": name }),
                    index,
                    count,
                    name,
                ))
            }
            "list_recent_cubes" => {
                let entries: Vec<serde_json::Value> = self
                    .recents
                    .list()
                    .iter()
                    .enumerate()
                    .map(|(i, p)| {
                        json!({
                            "index": i,
                            "path": p.to_string_lossy(),
                            "name": p.file_name().map(|n| n.to_string_lossy().to_string()),
                            // A recent entry can outlive its file (unmounted volume,
                            // deleted scratch); say so rather than making the caller
                            // discover it by failing to open.
                            "exists": p.exists(),
                        })
                    })
                    .collect();
                // `recents` is the reference's key; `cubes` is ours, kept so
                // existing callers keep working.
                Ok(json!({ "count": entries.len(), "recents": entries.clone(), "cubes": entries }))
            }
            "set_cube_transfer" => {
                let v = self
                    .active_viewer()
                    .ok_or_else(|| "no cube open".to_string())?;
                let reset = crate::mcp::tools::arg(args, "reset")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let points = if reset {
                    v.reset_transfer()
                } else {
                    let raw = crate::mcp::tools::arg(args, "points")
                        .and_then(|p| p.as_array())
                        .ok_or_else(|| {
                            "pass `points` (at least 2 control points), or reset: true".to_string()
                        })?;
                    if raw.len() < 2 {
                        return Err("a transfer curve needs at least 2 control points".to_string());
                    }
                    let mut parsed = Vec::with_capacity(raw.len());
                    for (i, p) in raw.iter().enumerate() {
                        let x = p.get("x").and_then(|v| v.as_f64());
                        let y = p.get("y").and_then(|v| v.as_f64());
                        let (Some(x), Some(y)) = (x, y) else {
                            return Err(format!("point {i} needs numeric x and y"));
                        };
                        if !(0.0..=1.0).contains(&x) || !(0.0..=1.0).contains(&y) {
                            return Err(format!(
                                "point {i} is ({x}, {y}); both must be within 0..1"
                            ));
                        }
                        parsed.push((x as f32, y as f32));
                    }
                    v.set_transfer_points(parsed)
                };
                Ok(json!({
                    "points": points
                        .iter()
                        .map(|(x, y)| serde_json::json!({ "x": x, "y": y }))
                        .collect::<Vec<_>>(),
                }))
            }
            "show_cube_spectrum" => {
                let v = self
                    .active_viewer()
                    .ok_or_else(|| "no cube open".to_string())?;
                if crate::mcp::tools::arg(args, "close")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                {
                    v.slice().hide_spectrum();
                    // `panelOpen` is the reference's field name; `visible` is
                    // kept because it is what our own callers already read.
                    return Ok(json!({ "panelOpen": false, "visible": false }));
                }
                let x = crate::mcp::tools::arg(args, "x")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| "x is required (or close: true)".to_string())?
                    as usize;
                let y = crate::mcp::tools::arg(args, "y")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| "y is required (or close: true)".to_string())?
                    as usize;
                if !v.slice().show_spectrum_at(x, y) {
                    let (nx, ny, _) = v.native_dims();
                    return Err(format!("spaxel ({x}, {y}) is outside the {nx}x{ny} cube"));
                }
                Ok(json!({ "panelOpen": true, "visible": true, "x": x, "y": y }))
            }
            "get_cube_channel_profile" => {
                let v = self
                    .active_viewer()
                    .ok_or_else(|| "no cube open".to_string())?;
                let profile: Vec<serde_json::Value> = v
                    .channel_profile()
                    .into_iter()
                    .map(|(channel, mean)| {
                        json!({
                            "channel": channel,
                            "mean": mean,
                            "spectral": v.wcs().channel_to_physical(channel as f64)
                                .map(|(value, unit)| json!({ "value": value, "unit": unit })),
                        })
                    })
                    .collect();
                Ok(json!({
                    "channels": profile.len(),
                    "downsampled": v.is_downsampled(),
                    "currentChannel": v.current_channel(),
                    "profile": profile,
                }))
            }
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
                // A blanked voxel reports null flux and is counted, rather than
                // being dropped or fabricated as zero — a caller integrating the
                // spectrum has to know how much of it was masked.
                let blanked = spectrum.iter().filter(|s| s.normalized.is_none()).count();
                let samples: Vec<serde_json::Value> = spectrum
                    .iter()
                    .map(|s| {
                        json!({
                            // The channel in the FILE, not in the strided volume.
                            "channel": s.native_channel,
                            "value": s.normalized,
                            "physical": s.physical,
                            "spectral": v.wcs().channel_to_physical(s.native_channel as f64)
                                .map(|(value, unit)| json!({ "value": value, "unit": unit })),
                        })
                    })
                    .collect();
                // The physics the caller needs to do anything with these numbers,
                // as the reference's `CubeSpectrumResult` carries: the beam is how
                // Jy/beam becomes Jy, and the rest frequency is how frequency
                // becomes velocity. `CubeWcs` has parsed all five from the header
                // since it was written; nothing had ever asked it for them.
                let wcs = v.wcs();
                let arcsec = |deg: Option<f64>| deg.map(|d| d * 3600.0);
                Ok(json!({
                    "x": x,
                    "y": y,
                    "channels": spectrum.len(),
                    "blankedChannels": blanked,
                    "downsampled": v.is_downsampled(),
                    "unit": v.value_unit(),
                    "spectralFrame": (!wcs.spectral_frame.is_empty()).then(|| wcs.spectral_frame.clone()),
                    "restFrequencyGHz": wcs.rest_frequency_hz.map(|hz| hz / 1e9),
                    "beamMajorArcsec": arcsec(wcs.beam_major_deg),
                    "beamMinorArcsec": arcsec(wcs.beam_minor_deg),
                    "beamPaDeg": wcs.beam_pa_deg,
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
                    // A `vos:` path is not absolute either, so the check below
                    // would catch it — and blame the wrong thing. "must be
                    // absolute" sends someone to write `/vos:/...`.
                    crate::helpers::local_path::reject_remote(
                        path_str,
                        crate::helpers::local_path::SAVE_THEN_UPLOAD,
                    )?;
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
                let png = crate::helpers::png::encode_rgba(width, height, &rgba)?;
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
                    publish_cube_tabs(&this.tab_view, &this.viewers);
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
    let (window_lo, window_hi) = v.window();
    serde_json::json!({
        // The reference's names, with our original short forms kept so an
        // existing reader does not break.
        "azimuth": az,
        "elevation": el,
        "distance": dist,
        "az": az,
        "el": el,
        "dist": dist,
        "steps": v.gl().steps(),
        "spectralScale": v.gl().spectral_scale(),
        "channel": v.current_channel(),
        "unit": v.value_unit(),
        "dims": { "nx": nx, "ny": ny, "nz": nz },
        // Read back everything `set_cube_view` can change. A control an agent
        // can set but not read leaves it unable to tell what it changed FROM,
        // so it cannot restore the user's view afterwards.
        "colormap": v.colormap_name(),
        "stretch": v.stretch_name(),
        "windowLo": window_lo,
        "windowHi": window_hi,
        "showCaptions": v.captions_visible(),
        "showSlicePlane": v.slice_plane_visible(),
        // The renderer's own settings, which the comment above has always
        // promised and this payload did not carry: an agent could set density,
        // MIP, the background and auto-orbit, and then had no way to learn what
        // they had been — so it could not put the user's view back.
        "density": v.gl().density(),
        "mip": v.gl().mip(),
        "renderMode": if v.gl().mip() { "max-intensity" } else { "composite" },
        "background": v.gl().background_name(),
        "autoOrbit": v.gl().auto_orbit(),
        // The opacity curve. `set_cube_transfer`'s own description — in the
        // reference and in ours — tells the agent "the current curve is in
        // get_cube_view's transferPoints", and this payload did not carry it:
        // the one control an agent could set and then not read.
        "transferPoints": v.transfer_points()
            .into_iter()
            .map(|(x, y)| serde_json::json!({ "x": x, "y": y }))
            .collect::<Vec<_>>(),
    })
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
