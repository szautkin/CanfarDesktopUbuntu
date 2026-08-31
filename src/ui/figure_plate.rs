//! The publication plate: a titled frame, a caption, a colorbar and a footer.
//!
//! Ported from the Windows `CubeExportPlate`, and for a long time the cube
//! viewer's alone. It is not about cubes: the layout is a header band, a framed
//! picture, a caption line, a colour ramp and a grid of key/value facts, and a
//! FITS frame wants exactly that too.
//!
//! Two things differ between viewers, so those are supplied rather than
//! assumed:
//!
//! * an optional [`FramePainter`], drawn over the picture — the cube's WCS
//!   wireframe and axis captions; a FITS frame has none, because its overlay is
//!   already in the capture.
//! * the footer's key/value pairs, which are whatever that viewer knows.
//!
//! Everything is sized as a fraction of the frame width, so a 2x or 4x export
//! stays proportional rather than growing a picture inside fixed furniture.

use crate::helpers::cube_colormaps;
use crate::helpers::image_bytes::rgba_to_surface;
use gtk4::cairo::{Context, Format, ImageSurface};
use std::rc::Rc;

/// Natural (1x) frame size for the captured picture, in px.
const FRAME_W: f64 = 720.0;
const FRAME_H: f64 = 540.0;

/// Plate palette.
const BG: (f64, f64, f64) = (0.05, 0.05, 0.06);
const MAIN: (f64, f64, f64) = (0.94, 0.94, 0.95);
const DIM: (f64, f64, f64) = (0.60, 0.62, 0.66);
const LINE: (f64, f64, f64) = (0.30, 0.30, 0.33);

/// Paints over the framed picture: `(cr, frame_x, frame_y, frame_w, frame_h)`.
///
/// Already positioned but not clipped — a painter clips itself if it needs to,
/// as the cube's does, because only it knows whether its geometry can overrun.
pub type FramePainter = Rc<dyn Fn(&Context, f64, f64, f64, f64)>;

/// Produces the picture at a requested size, as straight RGBA.
pub type Capture = Rc<dyn Fn(i32, i32) -> Option<Vec<u8>>>;

/// Everything a plate needs that is not layout.
pub struct PlateContent {
    pub capture: Capture,
    pub title: String,
    /// Under the title: what kind of picture this is.
    pub subtitle: String,
    /// Under the frame: what it shows.
    pub caption: String,
    pub colormap: String,
    pub lo_label: String,
    pub hi_label: String,
    pub date: String,
    /// Key/value pairs along the bottom, in order.
    pub footer: Vec<(String, String)>,
    pub overlay: Option<FramePainter>,
}

impl PlateContent {
    pub fn compose(&self, scale: i32, transparent: bool) -> Option<ImageSurface> {
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
        let meta_cols = self.footer.clone();
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
            let _ = cr.show_text(&self.subtitle);

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

            // Whatever the viewer wants over the picture — the cube's WCS
            // wireframe and axis captions, nothing for a FITS frame, whose
            // overlay is already in the capture.
            if let Some(paint) = self.overlay.as_ref() {
                paint(&cr, frame_x, frame_y, fwf, fhf);
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
}
