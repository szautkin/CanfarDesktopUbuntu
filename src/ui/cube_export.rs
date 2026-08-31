//! Export the cube viewer's rendered figure to a raster/vector file.
//!
//! Rust port of `Views/CubeViewer/CubeExportDialog.xaml.cs` (+ its plate). The
//! caller hands us a straight-alpha (non-premultiplied) 8-bit RGBA raster of the
//! composed plate; we pop a [`gtk::FileDialog`] save picker, then dispatch on the
//! chosen extension to [`crate::helpers::pdf_writer`] — `.pdf` writes a 1:1 PDF,
//! anything else writes a PNG. Success/failure is surfaced as an
//! [`adw::Toast`] on the nearest [`adw::ToastOverlay`] ancestor, falling back to
//! stderr when the widget tree has no overlay.

use crate::helpers::cube_math::Mat4;
use crate::helpers::cube_wcs::CubeWcs;
use crate::helpers::image_bytes::rgba_to_surface;
use crate::helpers::{cube_axes, cube_colormaps};
use crate::models::volume_data::CubeMetadata;
use gtk4::cairo::{Context, Format, ImageSurface};
use gtk4::prelude::*;
use gtk4::{self as gtk};
use std::rc::Rc;

// ===========================================================================
// Publication figure "plate" export
//
// Rust port of `Views/CubeViewer/CubeExportPlate.xaml.cs` +
// `CubeExportDialog.xaml.cs`. Instead of a live XAML control we compose the
// whole plate onto a Cairo `ARgb32` surface: header band (title / brand /
// date), the captured volume render drawn framed + centered, a WCS caption
// line, and a footer with a colour-mapped colorbar (lo/hi labels + colormap
// name). The same composer feeds both the WYSIWYG preview and the final raster
// that gets written out through [`pdf_writer`].
// ===========================================================================

/// Natural (1×) frame size for the captured render, in px. The plate scales
/// everything (text, padding, colorbar) as a fraction of the frame width, so a
/// 2×/4× export stays crisp and proportional — the Windows plate does the same.
const FRAME_W: f64 = 720.0;
const FRAME_H: f64 = 540.0;

/// Journal-dark palette (mirrors `CubeExportPlate.PlateStyle.Default`, Dark=true).
const BG: (f64, f64, f64) = (0.05, 0.05, 0.06);
const MAIN: (f64, f64, f64) = (0.94, 0.94, 0.95);
const DIM: (f64, f64, f64) = (0.60, 0.62, 0.66);
const LINE: (f64, f64, f64) = (0.30, 0.30, 0.33);

/// Live-overlay + metadata inputs the plate needs to draw the WCS wireframe box,
/// the axis captions, and the expanded metadata footer. The Rust analogue of the
/// Windows `CubeExportPlate.PlateData` overlay/metadata block, populated by the
/// cube viewer's export path (mirroring `CubeViewerPage.BuildPlateData`).
pub struct PlateOverlay {
    /// Draw the box + captions (true only over the 3D volume, not the flat slice).
    pub captions_on: bool,
    /// Cube WCS for the axis captions + footer sky/spectral ranges.
    pub wcs: Rc<CubeWcs>,
    /// `perspective * look_at` for a frame of the given px size (no box scale) —
    /// the SAME camera the capture uses, so the overlay aligns with the render.
    pub view_proj: Rc<dyn Fn(i32, i32) -> Mat4>,
    /// Rendered (uploaded) volume dims — drive the box aspect + caption channels
    /// exactly like the live overlay in `cube_viewer`.
    pub nx: usize,
    pub ny: usize,
    pub nz: usize,
    /// Spectral (Z) box stretch, matching the live view.
    pub spectral_scale: f32,
    /// Full-cube display metadata for the footer (native dims / NaN% / mode); `None`
    /// on the synthetic volume.
    pub meta: Option<CubeMetadata>,
}

/// The plate content that is invariant across scale/transparent toggles.
struct PlateSpec {
    capture: Rc<dyn Fn(i32, i32) -> Option<Vec<u8>>>,
    title: String,
    caption: String,
    colormap: String,
    lo_label: String,
    hi_label: String,
    date: String,
    /// Camera + WCS + metadata for the box/caption overlay and the footer grid.
    overlay: PlateOverlay,
}

impl PlateSpec {
    /// Compose the full plate at `scale` (1/2/4). `transparent` skips the
    /// background fill so the exported PNG keeps an alpha channel.
    fn compose(&self, scale: i32, transparent: bool) -> Option<ImageSurface> {
        let s = scale.max(1) as f64;
        let fw = (FRAME_W * s).round() as i32;
        let fh = (FRAME_H * s).round() as i32;
        let (fwf, fhf) = (fw as f64, fh as f64);

        let pad = (fwf * 0.03).max(18.0 * s);
        let title_f = fwf * 0.026;
        let small_f = fwf * 0.013;
        let line = small_f * 1.6;
        let cb_h = (fwf * 0.012).max(9.0 * s);

        // Vertical layout (baselines / band tops top-to-bottom).
        let title_base = pad + title_f;
        let sub_base = title_base + line;
        let div1 = sub_base + small_f * 0.7;
        let frame_y = div1 + line * 0.6;
        let cap_base = frame_y + fhf + line;
        let div2 = cap_base + small_f * 0.4;
        let cb_y = div2 + line * 0.6;
        let cb_row_h = cb_h.max(small_f * 1.5);

        // ── Footer metadata columns (Dims / RA-DEC-SPECTRAL / NaN% / Mode) ──
        // Measured + wrapped up-front, because the plate height depends on how many
        // rows the columns fold into. Mirrors Windows `BuildMetaGrid`.
        let meta_cols = self.meta_columns();
        let meta_key_f = small_f * 0.86;
        let meta_val_f = small_f;
        let meta_col_gap = small_f * 1.8;
        let meta_row_gap = small_f * 0.9;
        let meta_line_gap = small_f * 0.25;
        let meta_row_h = meta_key_f + meta_line_gap + meta_val_f;
        let mut meta_placed: Vec<(usize, f64, String, String)> = Vec::new();
        let mut meta_rows = 0usize;
        if !meta_cols.is_empty() {
            let scratch = ImageSurface::create(Format::ARgb32, 1, 1).ok()?;
            let mcr = Context::new(&scratch).ok()?;
            let measure = |mono: bool, size: f64, t: &str| -> f64 {
                mcr.select_font_face(
                    if mono { "monospace" } else { "sans" },
                    gtk4::cairo::FontSlant::Normal,
                    gtk4::cairo::FontWeight::Normal,
                );
                mcr.set_font_size(size);
                mcr.text_extents(t).map(|e| e.width()).unwrap_or(0.0)
            };
            let mut row = 0usize;
            let mut mx = 0.0f64;
            for (k, v) in &meta_cols {
                let colw = measure(false, meta_key_f, k).max(measure(true, meta_val_f, v));
                // Wrap to a new row when the column would spill past the frame width.
                if mx > 0.0 && mx + colw > fwf {
                    row += 1;
                    mx = 0.0;
                }
                meta_placed.push((row, mx, k.clone(), v.clone()));
                mx += colw + meta_col_gap;
            }
            meta_rows = row + 1;
        }
        let meta_block_h = if meta_rows == 0 {
            0.0
        } else {
            meta_rows as f64 * meta_row_h + (meta_rows - 1) as f64 * meta_row_gap
        };

        let meta_top = cb_y + cb_row_h + small_f * 1.2;
        let foot_bottom = if meta_rows == 0 {
            cb_y + cb_row_h + small_f * 0.4
        } else {
            meta_top + meta_block_h
        };

        let total_h = (foot_bottom + pad).round() as i32;
        let total_w = (fwf + 2.0 * pad).round() as i32;
        let frame_x = pad;
        let right_edge = frame_x + fwf;

        let surface = ImageSurface::create(Format::ARgb32, total_w, total_h).ok()?;
        {
            let cr = Context::new(&surface).ok()?;

            if !transparent {
                cr.set_source_rgb(BG.0, BG.1, BG.2);
                let _ = cr.paint();
            }

            let font = |mono: bool, bold: bool| {
                cr.select_font_face(
                    if mono { "monospace" } else { "sans" },
                    gtk4::cairo::FontSlant::Normal,
                    if bold {
                        gtk4::cairo::FontWeight::Bold
                    } else {
                        gtk4::cairo::FontWeight::Normal
                    },
                );
            };
            let width_of = |t: &str| cr.text_extents(t).map(|e| e.width()).unwrap_or(0.0);
            let rgb = |c: (f64, f64, f64)| cr.set_source_rgb(c.0, c.1, c.2);

            // ── Header ──────────────────────────────────────────────────────
            font(false, true);
            cr.set_font_size(title_f);
            rgb(MAIN);
            cr.move_to(frame_x, title_base);
            let _ = cr.show_text(&self.title);

            font(false, false);
            cr.set_font_size(small_f);
            rgb(DIM);
            cr.move_to(frame_x, sub_base);
            let _ = cr.show_text(crate::tr_en!("3D volume render"));

            // Brand + date, right-aligned.
            let brand = "\u{25C8} VERBINAL";
            let bw = width_of(brand);
            cr.move_to((right_edge - bw).max(frame_x), pad + small_f * 1.1);
            let _ = cr.show_text(brand);
            font(true, false);
            let dw = width_of(&self.date);
            cr.move_to((right_edge - dw).max(frame_x), pad + small_f * 1.1 + line);
            let _ = cr.show_text(&self.date);

            // Divider under the header.
            rgb(LINE);
            cr.set_line_width((small_f * 0.06).max(1.0));
            cr.move_to(frame_x, div1);
            cr.line_to(right_edge, div1);
            let _ = cr.stroke();

            // ── Framed render ───────────────────────────────────────────────
            let frame = (self.capture)(fw, fh).and_then(|rgba| rgba_to_surface(fw, fh, &rgba));
            match &frame {
                Some(fs) => {
                    let _ = cr.save();
                    cr.rectangle(frame_x, frame_y, fwf, fhf);
                    cr.clip();
                    let _ = cr.set_source_surface(fs, frame_x, frame_y);
                    let _ = cr.paint();
                    let _ = cr.restore();
                }
                None => {
                    // Placeholder box when no GPU snapshot is available.
                    cr.set_source_rgb(0.10, 0.10, 0.12);
                    cr.rectangle(frame_x, frame_y, fwf, fhf);
                    let _ = cr.fill();
                    font(false, false);
                    cr.set_font_size(small_f * 1.4);
                    rgb(DIM);
                    let msg = crate::tr_en!("Render unavailable");
                    let mw = width_of(msg);
                    cr.move_to(frame_x + (fwf - mw) / 2.0, frame_y + fhf / 2.0);
                    let _ = cr.show_text(msg);
                }
            }
            // Frame border.
            rgb(LINE);
            cr.set_line_width((small_f * 0.09).max(1.0));
            cr.rectangle(frame_x, frame_y, fwf, fhf);
            let _ = cr.stroke();

            // ── WCS wireframe box + axis captions (matches the live overlay) ─
            // Reuses cube_axes geometry with the SAME camera the capture used, so
            // edges/captions register onto the rendered volume. Port of Windows
            // CubeExportPlate.BuildCaptionOverlay; re-themed for the dark plate.
            if self.overlay.captions_on {
                let vp = (self.overlay.view_proj)(fw, fh);
                let ov = cube_axes::build(&cube_axes::AxesRequest {
                    dims: (self.overlay.nx, self.overlay.ny, self.overlay.nz),
                    wcs: &self.overlay.wcs,
                    view_proj: &vp,
                    panel: (fwf as f32, fhf as f32),
                    slice_z: 0, // slice-plane marker unused in the export overlay
                    spectral_scale: self.overlay.spectral_scale,
                });

                let _ = cr.save();
                cr.rectangle(frame_x, frame_y, fwf, fhf);
                cr.clip();
                cr.translate(frame_x, frame_y);

                // Box wireframe: faint cool-blue lines (same tone as the live view).
                cr.set_source_rgba(0.62, 0.77, 0.91, 0.40);
                cr.set_line_width((fwf / 1600.0).max(1.0));
                for (a, b) in &ov.edges {
                    cr.move_to(a.0 as f64, a.1 as f64);
                    cr.line_to(b.0 as f64, b.1 as f64);
                }
                let _ = cr.stroke();

                // Axis captions: centered monospace, 1px shadow for legibility.
                font(true, false);
                let cap_f = (fwf * 0.013).max(9.0);
                cr.set_font_size(cap_f);
                for (x, y, text) in &ov.captions {
                    let (mut cx, cy) = (*x as f64, *y as f64);
                    if let Ok(ext) = cr.text_extents(text) {
                        cx -= ext.width() / 2.0;
                    }
                    let cx = cx.clamp(2.0, (fwf - 2.0).max(2.0));
                    let cy = cy.clamp(cap_f, (fhf - 2.0).max(cap_f));
                    cr.set_source_rgba(0.0, 0.0, 0.0, 0.70);
                    cr.move_to(cx + 1.0, cy + 1.0);
                    let _ = cr.show_text(text);
                    cr.set_source_rgba(0.90, 0.95, 1.0, 0.96);
                    cr.move_to(cx, cy);
                    let _ = cr.show_text(text);
                }
                let _ = cr.restore();
            }

            // ── WCS caption line (centered) ─────────────────────────────────
            if !self.caption.is_empty() {
                font(true, false);
                cr.set_font_size(small_f);
                rgb(DIM);
                let cw = width_of(&self.caption);
                cr.move_to((frame_x + (fwf - cw) / 2.0).max(frame_x), cap_base);
                let _ = cr.show_text(&self.caption);
            }

            // Divider above the footer.
            rgb(LINE);
            cr.set_line_width((small_f * 0.06).max(1.0));
            cr.move_to(frame_x, div2);
            cr.line_to(right_edge, div2);
            let _ = cr.stroke();

            // ── Footer: labeled colorbar + colormap name ────────────────────
            let lbl_base = cb_y + cb_row_h / 2.0 + small_f * 0.35;
            let gap = small_f;
            let mut x = frame_x;

            font(true, false);
            cr.set_font_size(small_f);
            rgb(DIM);
            cr.move_to(x, lbl_base);
            let _ = cr.show_text(&self.lo_label);
            x += width_of(&self.lo_label) + gap;

            // Colorbar: sampled strips of the colormap LUT.
            let cb_w = (fwf * 0.22).max(120.0);
            let lut = cube_colormaps::lut_rgba(&self.colormap);
            let steps = 128usize;
            let bar_x = x;
            for i in 0..steps {
                let t = i as f64 / (steps - 1) as f64;
                let o = ((t * 255.0).round() as usize).min(255) * 4;
                cr.set_source_rgb(
                    lut[o] as f64 / 255.0,
                    lut[o + 1] as f64 / 255.0,
                    lut[o + 2] as f64 / 255.0,
                );
                let sx = bar_x + (i as f64 / steps as f64) * cb_w;
                cr.rectangle(sx, cb_y, cb_w / steps as f64 + 1.0, cb_h);
                let _ = cr.fill();
            }
            rgb(LINE);
            cr.set_line_width(1.0);
            cr.rectangle(bar_x, cb_y, cb_w, cb_h);
            let _ = cr.stroke();
            x = bar_x + cb_w + gap;

            font(true, false);
            rgb(DIM);
            cr.move_to(x, lbl_base);
            let _ = cr.show_text(&self.hi_label);
            x += width_of(&self.hi_label) + gap * 1.6;

            font(false, false);
            rgb(DIM);
            let cmap = format!("{} \u{00B7} {}", crate::tr_en!("Colormap"), self.colormap);
            cr.move_to(x, lbl_base);
            let _ = cr.show_text(&cmap);

            // ── Footer: metadata columns (Dims / RA-DEC-SPECTRAL / NaN% / Mode) ──
            // Each column stacks a dim key over a monospaced value. Port of Windows
            // CubeExportPlate.BuildMetaGrid / AddMetaColumn.
            for (r, mx, key, val) in &meta_placed {
                let base_y = meta_top + *r as f64 * (meta_row_h + meta_row_gap);
                let col_x = frame_x + *mx;
                font(false, false);
                cr.set_font_size(meta_key_f);
                rgb(DIM);
                cr.move_to(col_x, base_y + meta_key_f);
                let _ = cr.show_text(key);
                font(true, false);
                cr.set_font_size(meta_val_f);
                rgb(MAIN);
                cr.move_to(col_x, base_y + meta_key_f + meta_line_gap + meta_val_f);
                let _ = cr.show_text(val);
            }
        }
        Some(surface)
    }

    /// The footer metadata columns as `(key, value)` pairs, mirroring Windows
    /// `CubeViewerPage.BuildPlateData` + `BuildMetaGrid`: Dimensions, per-axis WCS
    /// ranges (RA/DEC/SPECTRAL), NaN%, and Mode. Reads from [`CubeMetadata`]; axis
    /// ranges are emitted only when the corresponding WCS solution is present.
    fn meta_columns(&self) -> Vec<(String, String)> {
        let o = &self.overlay;
        let mut cols: Vec<(String, String)> = Vec::new();
        match o.meta.as_ref() {
            Some(m) => {
                // Native (full-resolution) dimensions.
                cols.push((
                    crate::tr_en!("Dimensions").to_string(),
                    format!("{}\u{00D7}{}\u{00D7}{}", m.nx, m.ny, m.nz),
                ));
                // RA/DEC ranges across the native spatial extent, when spatial WCS is valid.
                if o.wcs.spatial.as_ref().is_some_and(|s| s.is_valid()) {
                    let nx = m.nx.max(1);
                    let ny = m.ny.max(1);
                    cols.push((
                        o.wcs.lon_name().to_string(),
                        format!(
                            "{} \u{2026} {}",
                            lon_text(o.wcs.as_ref(), 0, ny),
                            lon_text(o.wcs.as_ref(), nx - 1, ny),
                        ),
                    ));
                    cols.push((
                        o.wcs.lat_name().to_string(),
                        format!(
                            "{} \u{2026} {}",
                            lat_text(o.wcs.as_ref(), 0, nx),
                            lat_text(o.wcs.as_ref(), ny - 1, nx),
                        ),
                    ));
                }
                // Spectral range (display units) when a spectral axis spans >1 channel.
                if o.wcs.has_spectral() && m.nz > 1 {
                    cols.push((
                        o.wcs.spec_axis_name(),
                        format!(
                            "{} \u{2026} {}",
                            o.wcs.channel_label(0),
                            o.wcs.channel_label(m.nz - 1),
                        ),
                    ));
                }
                // NaN fraction (technical term, not localized) + resident/downsampled mode.
                cols.push(("NaN".to_string(), format!("{:.1}%", m.nan_fraction * 100.0)));
                cols.push((crate::tr_en!("Mode").to_string(), m.mode_text()));
            }
            None => {
                // Synthetic volume: rendered dims + a synthetic mode marker.
                cols.push((
                    crate::tr_en!("Dimensions").to_string(),
                    format!("{}\u{00D7}{}\u{00D7}{}", o.nx, o.ny, o.nz),
                ));
                cols.push((
                    crate::tr_en!("Mode").to_string(),
                    crate::tr_en!("Synthetic").to_string(),
                ));
            }
        }
        cols
    }
}

// ── Footer WCS caption formatters (ported from `helpers::cube_axes`, whose copies
// are module-private; the export owns these so the footer ranges match the live
// axis captions verbatim). ───────────────────────────────────────────────────

/// Formatted longitude at a 0-based X pixel, evaluated at the cube's mid Y.
fn lon_text(wcs: &CubeWcs, pix_x0: usize, ny: usize) -> String {
    match wcs.spatial.as_ref().filter(|s| s.is_valid()) {
        Some(s) => {
            let (lon, _lat) = s.pixel_to_sky(pix_x0 as f64 + 1.0, ny as f64 / 2.0);
            if wcs.galactic {
                format_deg(wrap360(lon))
            } else {
                format_ra_short(lon)
            }
        }
        None => format!("px {}", pix_x0),
    }
}

/// Formatted latitude at a 0-based Y pixel, evaluated at the cube's mid X.
fn lat_text(wcs: &CubeWcs, pix_y0: usize, nx: usize) -> String {
    match wcs.spatial.as_ref().filter(|s| s.is_valid()) {
        Some(s) => {
            let (_lon, lat) = s.pixel_to_sky(nx as f64 / 2.0, pix_y0 as f64 + 1.0);
            if wcs.galactic {
                format_deg(lat)
            } else {
                format_dec_short(lat)
            }
        }
        None => format!("px {}", pix_y0),
    }
}

/// `raDeg` → `"HH:MM:SS"` (RA folded into [0,24h)).
fn format_ra_short(ra_deg: f64) -> String {
    let mut ra = ra_deg / 15.0;
    ra %= 24.0;
    if ra < 0.0 {
        ra += 24.0;
    }
    let mut h = ra as i32;
    let mut m = ((ra - h as f64) * 60.0) as i32;
    let mut s = ((ra - h as f64 - m as f64 / 60.0) * 3600.0).round() as i32;
    if s == 60 {
        s = 0;
        m += 1;
    }
    if m == 60 {
        m = 0;
        h = (h + 1) % 24;
    }
    format!("{:02}:{:02}:{:02}", h, m, s)
}

/// `decDeg` → `"±DD:MM:SS"` (U+2212 MINUS SIGN for negatives).
fn format_dec_short(dec_deg: f64) -> String {
    let sign = if dec_deg >= 0.0 { "+" } else { "\u{2212}" };
    let d = dec_deg.abs();
    let mut dd = d as i32;
    let mut m = ((d - dd as f64) * 60.0) as i32;
    let mut s = ((d - dd as f64 - m as f64 / 60.0) * 3600.0).round() as i32;
    if s == 60 {
        s = 0;
        m += 1;
    }
    if m == 60 {
        m = 0;
        dd += 1;
    }
    format!("{}{:02}:{:02}:{:02}", sign, dd, m, s)
}

/// Decimal degrees to 3 places with a trailing degree sign.
fn format_deg(deg: f64) -> String {
    format!("{:.3}\u{00B0}", deg)
}

/// Fold an angle into [0, 360).
fn wrap360(v: f64) -> f64 {
    ((v % 360.0) + 360.0) % 360.0
}

/// Show the publication-figure export modal: a WYSIWYG preview of the composed
/// plate plus scale (1×/2×/4×), transparent-background and format (PNG/PDF)
/// options. On Save the render is re-captured at the chosen scale, the plate is
/// re-composed and written through [`pdf_writer`] via a [`gtk::FileDialog`].
pub fn show_cube_export(
    parent: &impl IsA<gtk::Widget>,
    capture: Rc<dyn Fn(i32, i32) -> Option<Vec<u8>>>,
    plate_title: String,
    caption: String,
    colormap: String,
    lo_label: String,
    hi_label: String,
    overlay: PlateOverlay,
) {
    let spec = Rc::new(PlateSpec {
        capture,
        title: if plate_title.trim().is_empty() {
            crate::tr_en!("Cube").to_string()
        } else {
            plate_title
        },
        caption,
        colormap,
        lo_label,
        hi_label,
        date: chrono::Utc::now().format("%Y-%m-%d").to_string(),
        overlay,
    });

    // The dialog itself is `ui::export_dialog`, shared with the FITS viewer.
    // What is cube-specific is composing the plate, which is what it asks for.
    let compose: crate::ui::export_dialog::Compose = {
        let spec = spec.clone();
        Rc::new(move |scale, transparent| spec.compose(scale, transparent))
    };
    crate::ui::export_dialog::show(parent, &spec.title, compose);
}
