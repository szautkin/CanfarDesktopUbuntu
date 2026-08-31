//! Turning Cairo surfaces into the straight-RGBA rasters the writers expect,
//! and back.
//!
//! Cairo wants premultiplied BGRA; `pdf_writer` and every export path want
//! straight RGBA. That conversion lived in `ui::cube_export` because the cube
//! was the only thing exporting. It is not UI, and it is now shared with the
//! FITS viewer's export — and it is exactly the kind of code that is quietly a
//! shade off rather than visibly broken when a second copy gets the rounding
//! different.

use gtk4::cairo::{self, Format, ImageSurface};

/// Un-premultiply one channel (inverse of [`pdf_writer`]'s premultiply).
#[inline]
pub fn unpremultiply(c: u8, a: u8) -> u8 {
    if a == 0 {
        0
    } else {
        (((c as u32) * 255 + (a as u32) / 2) / (a as u32)).min(255) as u8
    }
}

/// Build a Cairo `ARgb32` (premultiplied BGRA) surface from straight RGBA.
pub fn rgba_to_surface(width: i32, height: i32, rgba: &[u8]) -> Option<ImageSurface> {
    if width <= 0 || height <= 0 {
        return None;
    }
    let stride = Format::ARgb32.stride_for_width(width as u32).ok()? as usize;
    let (w, h) = (width as usize, height as usize);
    if rgba.len() < w * h * 4 {
        return None;
    }
    let mut data = vec![0u8; stride * h];
    for y in 0..h {
        let src_row = y * w * 4;
        let dst_row = y * stride;
        for x in 0..w {
            let s = src_row + x * 4;
            let d = dst_row + x * 4;
            let (r, g, b, a) = (rgba[s], rgba[s + 1], rgba[s + 2], rgba[s + 3]);
            let pm = |c: u8| ((c as u16 * a as u16 + 127) / 255) as u8;
            let (pr, pg, pb) = if a == 255 {
                (r, g, b)
            } else {
                (pm(r), pm(g), pm(b))
            };
            // Little-endian ARgb32 => bytes B, G, R, A.
            data[d] = pb;
            data[d + 1] = pg;
            data[d + 2] = pr;
            data[d + 3] = a;
        }
    }
    ImageSurface::create_for_data(data, Format::ARgb32, width, height, stride as i32).ok()
}

/// Read a composed plate surface back as straight (non-premultiplied) RGBA,
/// top-down — exactly what [`pdf_writer::write_png`] / [`write_pdf`] expect.
pub fn surface_to_rgba(surface: &mut ImageSurface) -> (i32, i32, Vec<u8>) {
    let (w, h) = (surface.width(), surface.height());
    let stride = surface.stride() as usize;
    surface.flush();
    let data = match surface.data() {
        Ok(d) => d,
        Err(_) => return (w, h, vec![0u8; (w.max(0) * h.max(0) * 4) as usize]),
    };
    let (wu, hu) = (w.max(0) as usize, h.max(0) as usize);
    let mut out = vec![0u8; wu * hu * 4];
    for y in 0..hu {
        for x in 0..wu {
            let s = y * stride + x * 4;
            // Premultiplied BGRA in native (little-endian) order.
            let (b, g, r, a) = (data[s], data[s + 1], data[s + 2], data[s + 3]);
            let d = (y * wu + x) * 4;
            out[d] = unpremultiply(r, a);
            out[d + 1] = unpremultiply(g, a);
            out[d + 2] = unpremultiply(b, a);
            out[d + 3] = a;
        }
    }
    (w, h, out)
}

/// Draw over a straight-RGBA raster with cairo, and hand it back in the same
/// format.
///
/// Exists so callers can composite onto an exported plate without learning
/// that Cairo wants premultiplied BGRA and the exporters want straight RGBA.
/// That conversion is this module's business, and it is the kind of detail
/// that is silently wrong rather than loudly wrong when a second copy of it
/// gets the rounding a shade different.
///
/// Returns the raster unchanged if it cannot be wrapped in a surface, so a
/// composite that fails costs the overlay rather than the picture.
pub fn draw_over_rgba(
    width: i32,
    height: i32,
    rgba: Vec<u8>,
    paint: impl FnOnce(&cairo::Context),
) -> Vec<u8> {
    let Some(mut surface) = rgba_to_surface(width, height, &rgba) else {
        return rgba;
    };
    {
        let Ok(cr) = cairo::Context::new(&surface) else {
            return rgba;
        };
        paint(&cr);
    }
    let (_, _, out) = surface_to_rgba(&mut surface);
    out
}

#[cfg(test)]
mod tests {
    use super::draw_over_rgba;

    /// Drawing over a plate changes it, and hands back the format it was given.
    ///
    /// The reason exported figures had no marks was never the drawing — it was
    /// that nothing drew. This pins the compositing step itself: opaque pixels
    /// survive the round trip through premultiplied BGRA unchanged, and what
    /// is painted actually lands.
    #[test]
    fn painting_over_a_plate_lands_and_keeps_straight_rgba() {
        let (w, h) = (8i32, 4i32);
        // A known opaque colour that is NOT grey, so a channel swap shows up.
        let mut rgba = Vec::new();
        for _ in 0..(w * h) {
            rgba.extend_from_slice(&[10, 120, 240, 255]);
        }
        let untouched = draw_over_rgba(w, h, rgba.clone(), |_| {});
        assert_eq!(untouched, rgba, "an empty paint must not alter the plate");

        let painted = draw_over_rgba(w, h, rgba.clone(), |cr| {
            cr.set_source_rgb(1.0, 0.0, 0.0);
            cr.rectangle(0.0, 0.0, 1.0, 1.0);
            let _ = cr.fill();
        });
        assert_eq!(&painted[0..4], &[255, 0, 0, 255], "the paint did not land");
        // Everything outside the paint is byte-identical.
        assert_eq!(&painted[4..], &rgba[4..], "the rest of the plate moved");
    }

    /// A transparent plate stays transparent where nothing was drawn.
    ///
    /// `export_cube_figure` offers a transparent background, and
    /// un-premultiplying a fully transparent pixel is the classic place to
    /// divide by zero or to resurrect black.
    #[test]
    fn a_transparent_plate_survives_the_round_trip() {
        let (w, h) = (4i32, 2i32);
        let rgba = vec![0u8; (w * h * 4) as usize];
        let out = draw_over_rgba(w, h, rgba.clone(), |_| {});
        assert_eq!(out, rgba, "transparent pixels were altered");
    }

    /// A plate that cannot be wrapped costs the overlay, not the picture.
    #[test]
    fn a_malformed_plate_comes_back_unchanged() {
        let short = vec![1u8, 2, 3, 4];
        assert_eq!(draw_over_rgba(64, 64, short.clone(), |_| {}), short);
        assert_eq!(draw_over_rgba(0, 0, short.clone(), |_| {}), short);
    }
}
