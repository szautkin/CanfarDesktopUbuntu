//! Turning pixels into a PNG.
//!
//! Lived as a private function inside `ui::cube_tab_host`, where the cube
//! viewer's figure export needed it. The FITS working-area capture needs
//! exactly the same thing, and copying it would have been the first of several
//! copies — the `imageBase64` promotion next door is already written four times
//! and has drifted three ways.
//!
//! No GTK and no viewer state: an RGBA buffer in, PNG bytes out, so both callers
//! and the tests can reach it.

/// PNG-encode a straight-alpha RGBA8 buffer (`width*height*4`, top-down) to bytes
/// via cairo — the in-memory sibling of [`crate::helpers::pdf_writer::write_png`].
/// cairo's `ARgb32` surface is premultiplied BGRA in native-endian order, so we
/// premultiply + channel-swap while packing.
pub fn encode_rgba(width: i32, height: i32, rgba: &[u8]) -> Result<Vec<u8>, String> {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The eight bytes every PNG starts with.
    const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

    /// Width and height, read out of the IHDR chunk.
    ///
    /// Read by hand rather than through a decoder, so the test asserts the
    /// FILE says 2x2 rather than trusting the same library that wrote it.
    fn ihdr_size(png: &[u8]) -> (u32, u32) {
        assert_eq!(&png[..8], &SIGNATURE, "not a PNG");
        assert_eq!(&png[12..16], b"IHDR", "IHDR is not the first chunk");
        let w = u32::from_be_bytes([png[16], png[17], png[18], png[19]]);
        let h = u32::from_be_bytes([png[20], png[21], png[22], png[23]]);
        (w, h)
    }

    #[test]
    fn it_writes_a_png_of_the_asked_for_size() {
        let rgba = vec![255u8; 3 * 2 * 4];
        let png = encode_rgba(3, 2, &rgba).expect("encode");
        assert_eq!(ihdr_size(&png), (3, 2));
    }

    /// The colours come back as they went in.
    ///
    /// cairo's `ARgb32` is premultiplied BGRA in native-endian order and the
    /// input is straight-alpha RGBA, so the packing does a channel swap and a
    /// multiply. Getting either backwards produces an image that is wrong in a
    /// way only an eye would catch — blue where red should be — which is
    /// exactly what an agent would then describe with confidence.
    #[test]
    fn colours_survive_the_round_trip() {
        // One opaque red pixel, one opaque blue.
        let rgba: Vec<u8> = vec![255, 0, 0, 255, 0, 0, 255, 255];
        let png = encode_rgba(2, 1, &rgba).expect("encode");

        let mut cursor = std::io::Cursor::new(png);
        let mut surface = cairo::ImageSurface::create_from_png(&mut cursor).expect("decode");
        let width = surface.width();
        let stride = surface.stride() as usize;
        let data = surface.data().expect("pixels");
        assert_eq!(width, 2);

        // Native-endian ARgb32 on little-endian: bytes are B, G, R, A.
        assert_eq!(
            (data[2], data[1], data[0], data[3]),
            (255, 0, 0, 255),
            "the first pixel is not red — channels are swapped"
        );
        let second = 4;
        assert_eq!(
            (
                data[second + 2],
                data[second + 1],
                data[second],
                data[second + 3]
            ),
            (0, 0, 255, 255),
            "the second pixel is not blue"
        );
        let _ = stride;
    }

    /// A half-transparent white premultiplies to half grey, not to white.
    #[test]
    fn alpha_is_premultiplied() {
        let rgba: Vec<u8> = vec![255, 255, 255, 128];
        let png = encode_rgba(1, 1, &rgba).expect("encode");
        let mut cursor = std::io::Cursor::new(png);
        let mut surface = cairo::ImageSurface::create_from_png(&mut cursor).expect("decode");
        let data = surface.data().expect("pixels");
        assert_eq!(data[3], 128, "alpha was lost");
        assert!(
            (120..=136).contains(&data[0]),
            "colour was not premultiplied: {} (expected about 128)",
            data[0]
        );
    }

    /// Nonsense dimensions are refused rather than allocated.
    #[test]
    fn impossible_sizes_are_refused() {
        assert!(encode_rgba(0, 4, &[]).is_err());
        assert!(encode_rgba(4, 0, &[]).is_err());
        assert!(encode_rgba(-1, 4, &[]).is_err());
        // The multiplication that sizes the buffer must not wrap.
        assert!(encode_rgba(i32::MAX, i32::MAX, &[]).is_err());
    }

    /// A buffer smaller than the dimensions claim is refused, not read past.
    #[test]
    fn a_short_buffer_is_refused_rather_than_over_read() {
        let too_small = vec![0u8; 4 * 4 * 4 - 1];
        let err = encode_rgba(4, 4, &too_small).expect_err("should refuse");
        assert!(err.contains("too small"), "{err}");
    }
}
