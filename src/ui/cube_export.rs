//! Export the cube viewer's rendered figure to a raster/vector file.
//!
//! Rust port of `Views/CubeViewer/CubeExportDialog.xaml.cs` (+ its plate). The
//! caller hands us a straight-alpha (non-premultiplied) 8-bit RGBA raster of the
//! composed plate; we pop a [`gtk::FileDialog`] save picker, then dispatch on the
//! chosen extension to [`crate::helpers::pdf_writer`] — `.pdf` writes a 1:1 PDF,
//! anything else writes a PNG. Success/failure is surfaced as an
//! [`adw::Toast`] on the nearest [`adw::ToastOverlay`] ancestor, falling back to
//! stderr when the widget tree has no overlay.

use crate::helpers::cube_axes;
use crate::helpers::cube_math::Mat4;
use crate::helpers::cube_wcs::CubeWcs;
use crate::models::volume_data::CubeMetadata;
use gtk4::cairo::ImageSurface;
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

/// Live-overlay + metadata inputs the plate needs to draw the WCS wireframe box,
/// the axis captions, and the expanded metadata footer. The Rust analogue of the
/// Windows `CubeExportPlate.PlateData` overlay/metadata block, populated by the
/// cube viewer's export path (mirroring `CubeViewerPage.BuildPlateData`).
#[derive(Clone)]
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
    /// The plate this spec describes.
    ///
    /// The layout lives in [`crate::ui::figure_plate`]; what a cube adds is the
    /// wireframe over the picture and the facts along the bottom.
    fn content(&self) -> crate::ui::figure_plate::PlateContent {
        let ov_data = self.overlay.clone();
        let painter: crate::ui::figure_plate::FramePainter =
            Rc::new(move |cr, frame_x, frame_y, frame_w, frame_h| {
                // The WCS wireframe box and axis captions, over the rendered volume.
                // Built from `cube_axes` with the SAME camera the capture used, so the
                // edges register on the render rather than floating near it. Port of
                // Windows `CubeExportPlate.BuildCaptionOverlay`, re-themed for the
                // dark plate.
                let vp = (ov_data.view_proj)(frame_w as i32, frame_h as i32);
                let ov = cube_axes::build(&cube_axes::AxesRequest {
                    dims: (ov_data.nx, ov_data.ny, ov_data.nz),
                    wcs: &ov_data.wcs,
                    view_proj: &vp,
                    panel: (frame_w as f32, frame_h as f32),
                    slice_z: 0, // slice-plane marker unused in the export overlay
                    spectral_scale: ov_data.spectral_scale,
                });

                let _ = cr.save();
                cr.rectangle(frame_x, frame_y, frame_w, frame_h);
                cr.clip();
                cr.translate(frame_x, frame_y);

                // Box wireframe: faint cool-blue lines (same tone as the live view).
                cr.set_source_rgba(0.62, 0.77, 0.91, 0.40);
                cr.set_line_width((frame_w / 1600.0).max(1.0));
                for (a, b) in &ov.edges {
                    cr.move_to(a.0 as f64, a.1 as f64);
                    cr.line_to(b.0 as f64, b.1 as f64);
                }
                let _ = cr.stroke();

                // Axis captions: centered monospace, 1px shadow for legibility.
                // The face is set here rather than inherited: a painter is handed a
                // context whose font is whatever the plate last used, and captions
                // in the title's face would be a different bug every time the
                // layout above them changed.
                cr.select_font_face(
                    "monospace",
                    gtk4::cairo::FontSlant::Normal,
                    gtk4::cairo::FontWeight::Normal,
                );
                let cap_f = (frame_w * 0.013).max(9.0);
                cr.set_font_size(cap_f);
                for (x, y, text) in &ov.captions {
                    let (mut cx, cy) = (*x as f64, *y as f64);
                    if let Ok(ext) = cr.text_extents(text) {
                        cx -= ext.width() / 2.0;
                    }
                    let cx = cx.clamp(2.0, (frame_w - 2.0).max(2.0));
                    let cy = cy.clamp(cap_f, (frame_h - 2.0).max(cap_f));
                    cr.set_source_rgba(0.0, 0.0, 0.0, 0.70);
                    cr.move_to(cx + 1.0, cy + 1.0);
                    let _ = cr.show_text(text);
                    cr.set_source_rgba(0.90, 0.95, 1.0, 0.96);
                    cr.move_to(cx, cy);
                    let _ = cr.show_text(text);
                }
                let _ = cr.restore();
            });
        crate::ui::figure_plate::PlateContent {
            capture: self.capture.clone(),
            title: self.title.clone(),
            subtitle: crate::tr_en!("3D volume render").to_string(),
            caption: self.caption.clone(),
            colormap: self.colormap.clone(),
            ramp: ramp_from_flat(&crate::helpers::cube_colormaps::lut_rgba(&self.colormap)),
            lo_label: self.lo_label.clone(),
            hi_label: self.hi_label.clone(),
            date: self.date.clone(),
            footer: self.meta_columns(),
            overlay: self.overlay.captions_on.then_some(painter),
        }
    }

    fn compose(&self, scale: i32, transparent: bool) -> Option<ImageSurface> {
        self.content().compose(scale, transparent)
    }

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
/// The cube's flat RGBA colour run as the triples the plate wants.
///
/// `cube_colormaps` hands back 256 RGBA bytes; the plate takes 256 triples.
/// The conversion lives here, on the side that knows the layout, rather than
/// the plate learning about either viewer's byte order.
fn ramp_from_flat(flat: &[u8]) -> [(u8, u8, u8); 256] {
    let mut ramp = [(0u8, 0u8, 0u8); 256];
    for (i, e) in ramp.iter_mut().enumerate() {
        let o = i * 4;
        if o + 2 < flat.len() {
            *e = (flat[o], flat[o + 1], flat[o + 2]);
        }
    }
    ramp
}

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

#[cfg(test)]
mod tests {
    use super::ramp_from_flat;

    /// The colour the plate draws is the colour the cube draws.
    ///
    /// The bar used to be looked up by NAME inside the plate, which fell back
    /// silently when the cube spelled its colormaps "Grayscale" and the FITS
    /// viewer spelled them "grayscale" — a grayscale image with an inferno bar
    /// under it, labelled "grayscale". The ramp is handed over now, so the only
    /// thing that can go wrong is this conversion.
    #[test]
    fn the_ramp_keeps_every_colour_the_lut_had() {
        for name in crate::helpers::cube_colormaps::NAMES {
            let flat = crate::helpers::cube_colormaps::lut_rgba(name);
            let ramp = ramp_from_flat(&flat);
            for (i, entry) in ramp.iter().enumerate() {
                let o = i * 4;
                assert_eq!(
                    *entry,
                    (flat[o], flat[o + 1], flat[o + 2]),
                    "{name} entry {i} changed on the way to the plate"
                );
            }
        }
    }

    /// A short or empty run leaves black rather than reading past the end.
    #[test]
    fn a_truncated_lut_does_not_read_past_its_end() {
        assert_eq!(ramp_from_flat(&[])[0], (0, 0, 0));
        assert_eq!(ramp_from_flat(&[1, 2, 3, 4])[0], (1, 2, 3));
        assert_eq!(ramp_from_flat(&[1, 2, 3, 4])[1], (0, 0, 0));
    }
}
