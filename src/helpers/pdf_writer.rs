//! Export helpers for the cube viewer's "publication figure" plate: write an
//! RGBA raster to PNG or to a single-page 1:1 PDF. The Windows reference hand-rolls
//! a minimal FlateDecode PDF (`Services/CubeViewer/PdfImageWriter.cs`); on Linux we
//! lean on cairo's own PDF backend instead (equivalent output, page sized to the
//! image in points at 72 dpi).
//!
//! Input is straight (non-premultiplied) 8-bit RGBA, row-major, top-down. cairo's
//! `ARgb32` surface format is premultiplied BGRA in native-endian order, so we
//! premultiply and swap channels when packing the surface. As in the Windows
//! writer, any transparency in the PDF path is flattened onto a white background.

use cairo::{Context, Format, ImageSurface, PdfSurface};
use std::path::Path;

/// Premultiply one 8-bit colour channel by an 8-bit alpha (rounded).
#[inline]
fn premultiply(c: u8, a: u8) -> u8 {
    ((c as u16 * a as u16 + 127) / 255) as u8
}

/// Build a cairo `ARgb32` image surface (premultiplied BGRA) from straight RGBA.
fn build_surface(width: i32, height: i32, rgba: &[u8]) -> Result<ImageSurface, String> {
    if width <= 0 || height <= 0 {
        return Err(format!("invalid image dimensions {width}x{height}"));
    }
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(4))
        .ok_or_else(|| "image dimensions overflow".to_string())?;
    if rgba.len() < expected {
        return Err(format!(
            "rgba buffer too small: got {} bytes, need {}",
            rgba.len(),
            expected
        ));
    }

    let stride = Format::ARgb32
        .stride_for_width(width as u32)
        .map_err(|e| format!("cairo stride error: {e}"))?;

    let w = width as usize;
    let h = height as usize;
    let s = stride as usize;
    let mut data = vec![0u8; s * h];
    for y in 0..h {
        let row_src = y * w * 4;
        let row_dst = y * s;
        for x in 0..w {
            let src = row_src + x * 4;
            let dst = row_dst + x * 4;
            let r = rgba[src];
            let g = rgba[src + 1];
            let b = rgba[src + 2];
            let a = rgba[src + 3];
            let (pr, pg, pb) = if a == 255 {
                (r, g, b)
            } else {
                (premultiply(r, a), premultiply(g, a), premultiply(b, a))
            };
            // Little-endian ARgb32 (0xAARRGGBB) => bytes B, G, R, A.
            data[dst] = pb;
            data[dst + 1] = pg;
            data[dst + 2] = pr;
            data[dst + 3] = a;
        }
    }

    ImageSurface::create_for_data(data, Format::ARgb32, width, height, stride)
        .map_err(|e| format!("cairo surface error: {e}"))
}

/// Write straight-alpha RGBA (`width*height*4` bytes, top-down) to a PNG file.
pub fn write_png(path: &Path, width: i32, height: i32, rgba: &[u8]) -> Result<(), String> {
    let surface = build_surface(width, height, rgba)?;
    let mut file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    surface
        .write_to_png(&mut file)
        .map_err(|e| format!("PNG write failed: {e}"))
}

/// Write straight-alpha RGBA to a single-page PDF sized 1:1 to the image (points
/// == pixels at 72 dpi). Transparency is flattened onto white.
pub fn write_pdf(path: &Path, width: i32, height: i32, rgba: &[u8]) -> Result<(), String> {
    let image = build_surface(width, height, rgba)?;
    let pdf = PdfSurface::new(width as f64, height as f64, path)
        .map_err(|e| format!("cairo PDF surface error: {e}"))?;
    {
        let ctx = Context::new(&pdf).map_err(|e| format!("cairo context error: {e}"))?;
        // Flatten any transparency onto white, matching the Windows
        // PdfImageWriter.BgraToRgbOverWhite behaviour.
        ctx.set_source_rgb(1.0, 1.0, 1.0);
        ctx.paint().map_err(|e| e.to_string())?;
        ctx.set_source_surface(&image, 0.0, 0.0)
            .map_err(|e| e.to_string())?;
        ctx.paint().map_err(|e| e.to_string())?;
    }
    pdf.finish();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn temp_path(ext: &str) -> std::path::PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!(
            "verbinal_pdf_writer_{}_{}.{}",
            std::process::id(),
            n,
            ext
        ))
    }

    /// Build a small opaque RGBA gradient (top-down, straight alpha).
    fn sample_rgba(w: i32, h: i32) -> Vec<u8> {
        let mut v = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                v.push((x * 255 / w.max(1)) as u8); // R
                v.push((y * 255 / h.max(1)) as u8); // G
                v.push(128); // B
                v.push(255); // A
            }
        }
        v
    }

    #[test]
    fn premultiply_bounds() {
        assert_eq!(premultiply(255, 255), 255);
        assert_eq!(premultiply(255, 0), 0);
        assert_eq!(premultiply(0, 255), 0);
        // 128 * 128 / 255 ≈ 64 (rounded)
        assert_eq!(premultiply(128, 128), 64);
    }

    #[test]
    fn build_surface_rejects_bad_dims() {
        assert!(build_surface(0, 10, &[]).is_err());
        assert!(build_surface(10, 0, &[]).is_err());
    }

    #[test]
    fn build_surface_rejects_short_buffer() {
        // 4x4 needs 64 bytes; give it 10.
        assert!(build_surface(4, 4, &vec![0u8; 10]).is_err());
    }

    #[test]
    fn build_surface_channel_order_and_premultiply() {
        // Single pixel, half-transparent red.
        let rgba = vec![200u8, 100, 50, 128];
        let mut surface = build_surface(1, 1, &rgba).unwrap();
        let stride = surface.stride() as usize;
        let data = surface.data().unwrap();
        // Bytes are premultiplied BGRA.
        assert_eq!(data[0], premultiply(50, 128)); // B
        assert_eq!(data[1], premultiply(100, 128)); // G
        assert_eq!(data[2], premultiply(200, 128)); // R
        assert_eq!(data[3], 128); // A
        assert!(stride >= 4);
    }

    #[test]
    fn write_png_produces_a_file() {
        let path = temp_path("png");
        let rgba = sample_rgba(16, 12);
        write_png(&path, 16, 12, &rgba).unwrap();
        let meta = std::fs::metadata(&path).unwrap();
        assert!(meta.len() > 0);
        // PNG magic bytes.
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(
            &bytes[..8],
            &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn write_pdf_produces_a_file() {
        let path = temp_path("pdf");
        let rgba = sample_rgba(20, 20);
        write_pdf(&path, 20, 20, &rgba).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes.len() > 0);
        // PDF header.
        assert_eq!(&bytes[..5], b"%PDF-");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn write_png_rejects_short_buffer() {
        let path = temp_path("png");
        assert!(write_png(&path, 8, 8, &vec![0u8; 4]).is_err());
    }
}
