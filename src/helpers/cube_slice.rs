//! CPU renderer for the cube viewer's 2D slice view.
//!
//! One-to-one port of `Services/CubeViewer/CubeSliceRenderer.cs` (plus the stretch
//! math from `Services/Fits/ImageStretcher.cs`). Maps one spectral channel of the
//! (already-normalized) [`VolumeData`] through the window + stretch + active colormap
//! into pixels for the slice panel. It operates on the same normalized voxel buffer
//! the GL volume ray-marcher samples, so the slice and the 3D volume share the same
//! window / stretch / colormap and stay visually consistent.
//!
//! The [`StretchMode`] discriminants (`Linear`, `Log`, `Sqrt`, `Squared`, `Asinh` =
//! 0..4) match the `stretch` uniform in `CubeVolumeShaders`' `applyStretch`, so the
//! CPU slice reproduces the GPU stretch exactly.

use crate::helpers::cube_colormaps;
use crate::models::volume_data::VolumeData;

/// Image stretch functions applied to the normalized voxel value before the colormap.
///
/// Discriminant order (`Linear = 0` … `Asinh = 4`) intentionally matches the GL
/// shader's `stretch` uniform (`CubeVolumeShaders.applyStretch`) so the 2D slice and
/// the 3D volume apply an identical transfer curve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StretchMode {
    Linear = 0,
    Log = 1,
    Sqrt = 2,
    Squared = 3,
    Asinh = 4,
}

impl StretchMode {
    /// All modes in shader-index order. Handy for a stretch picker UI.
    pub const ALL: [StretchMode; 5] = [
        StretchMode::Linear,
        StretchMode::Log,
        StretchMode::Sqrt,
        StretchMode::Squared,
        StretchMode::Asinh,
    ];

    /// Map a shader index (0..4) back to a mode; out-of-range falls back to `Linear`.
    pub fn from_index(i: usize) -> StretchMode {
        Self::ALL.get(i).copied().unwrap_or(StretchMode::Linear)
    }
}

/// Normalize `value` to `[0, 1]` using `min`/`max` cuts and a stretch curve.
///
/// Direct port of `ImageStretcher.Stretch` — identical guards (degenerate window ->
/// `0.5`, non-finite input -> `0`) and identical curve constants, so it matches both
/// the 2D FITS viewer and the cube's GL shader byte-for-byte.
#[inline]
pub fn stretch(value: f32, min: f32, max: f32, mode: StretchMode) -> f32 {
    if max <= min {
        return 0.5;
    }
    if !value.is_finite() {
        return 0.0;
    }

    // Clamp to the cut range, then apply the curve.
    let n = ((value - min) / (max - min)).clamp(0.0, 1.0);

    match mode {
        StretchMode::Linear => n,
        // MathF.Log10(1 + 9*n) / MathF.Log10(10f); the divisor is 1 but is kept for fidelity.
        StretchMode::Log => (1.0 + 9.0 * n).log10() / (10.0f32).log10(),
        StretchMode::Sqrt => n.sqrt(),
        StretchMode::Squared => n * n,
        // MathF.Asinh(10*n) / MathF.Asinh(10f)
        StretchMode::Asinh => (10.0 * n).asinh() / (10.0f32).asinh(),
    }
}

/// Render channel `z` of `vol` into a BGRA8 buffer (length `nx * ny * 4`) suitable
/// for a Cairo `Format::ARgb32` image surface.
///
/// `window` is the normalized `(lo, hi)` cut applied before `stretch`; `cmap` is a
/// colormap name from [`cube_colormaps::NAMES`]. Each pixel is mapped through the
/// window + stretch, quantized to a 256-entry LUT, and written as B, G, R, A. Output
/// is fully opaque (`A = 255`); because alpha is 255, the premultiplied storage Cairo
/// expects for `ARgb32` is identical to straight color, so no premultiply is needed.
///
/// Blank/`NaN` voxels stretch to `0` (via [`stretch`]) and therefore render as the
/// lowest colormap color, matching `CubeSliceRenderer.RenderPlane`.
pub fn render_plane_bgra(
    vol: &VolumeData,
    z: usize,
    window: (f32, f32),
    stretch_mode: StretchMode,
    cmap: &str,
) -> Vec<u8> {
    let nx = vol.nx;
    let ny = vol.ny;
    let nz = vol.nz;

    let mut dest = vec![0u8; nx * ny * 4];
    if nx == 0 || ny == 0 || nz == 0 {
        return dest;
    }

    // Clamp the requested channel into range (Math.Clamp(channel, 0, nz-1)).
    let z = z.min(nz - 1);
    let (lo, hi) = window;

    let lut = cube_colormaps::lut_rgba(cmap); // 256 * 4 RGBA8

    let plane_base = z * ny * nx;
    for y in 0..ny {
        let row = y * nx;
        for x in 0..nx {
            let v = vol.data[plane_base + row + x]; // normalized [0,1] (NaN = blank)
            let s = stretch(v, lo, hi, stretch_mode);
            // idx = clamp((int)(s*255 + 0.5), 0, 255)
            let idx = ((s * 255.0 + 0.5) as i32).clamp(0, 255) as usize;
            let o = idx * 4; // RGBA in the LUT
            let d = (row + x) * 4; // BGRA out
            dest[d] = lut[o + 2]; // B
            dest[d + 1] = lut[o + 1]; // G
            dest[d + 2] = lut[o]; // R
            dest[d + 3] = 255; // A (opaque)
        }
    }
    dest
}

/// Extract the spectrum (one normalized value per channel) at spatial pixel `(x, y)`.
///
/// Returns `vol.nz` values in channel order `0..nz`, each the normalized voxel value
/// at `(x, y, z)`. Out-of-range coordinates (or an empty spectral axis) yield an empty
/// vector. Port of `CubeSliceRenderer.Spectrum`, but staying in normalized `[0, 1]`
/// space per the shared contract (callers convert back to physical units via the
/// cube's normalization cut).
pub fn extract_spectrum(vol: &VolumeData, x: usize, y: usize) -> Vec<f32> {
    let nx = vol.nx;
    let ny = vol.ny;
    let nz = vol.nz;
    if x >= nx || y >= ny || nz == 0 {
        return Vec::new();
    }

    let plane_vox = ny * nx;
    let col = y * nx + x;
    let mut spectrum = Vec::with_capacity(nz);
    for z in 0..nz {
        spectrum.push(vol.data[z * plane_vox + col]); // normalized [0,1]
    }
    spectrum
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::volume_data::VolumeData;

    /// Build a tiny gradient cube: value increases with the flat voxel index so we get
    /// deterministic, distinct per-voxel normalized values in [0, 1].
    fn make_vol(nx: usize, ny: usize, nz: usize) -> VolumeData {
        let n = nx * ny * nz;
        let denom = if n > 1 { (n - 1) as f32 } else { 1.0 };
        let data: Vec<f32> = (0..n).map(|i| i as f32 / denom).collect();
        VolumeData {
            nx,
            ny,
            nz,
            data,
            name: "test".to_string(),
            meta: None,
        }
    }

    #[test]
    fn stretch_endpoints() {
        for mode in StretchMode::ALL {
            let lo = stretch(0.0, 0.0, 1.0, mode);
            let hi = stretch(1.0, 0.0, 1.0, mode);
            assert!(lo.abs() < 1e-6, "{:?}: stretch(0) = {}", mode, lo);
            assert!((hi - 1.0).abs() < 1e-6, "{:?}: stretch(1) = {}", mode, hi);
        }
    }

    #[test]
    fn stretch_monotonic_nondecreasing() {
        // Every stretch curve must be monotonically non-decreasing across the window,
        // so brighter input never maps to a darker output.
        for mode in StretchMode::ALL {
            let mut prev = f32::NEG_INFINITY;
            for i in 0..=100 {
                let v = i as f32 / 100.0;
                let s = stretch(v, 0.0, 1.0, mode);
                assert!(
                    s >= prev - 1e-6,
                    "{:?} not monotonic at v={}: {} < {}",
                    mode,
                    v,
                    s,
                    prev
                );
                assert!((0.0..=1.0).contains(&s), "{:?}: {} out of [0,1]", mode, s);
                prev = s;
            }
        }
    }

    #[test]
    fn stretch_curve_constants_match_shader() {
        // Log: log10(1 + 9*0.5) / log10(10) = log10(5.5)
        let log_half = stretch(0.5, 0.0, 1.0, StretchMode::Log);
        assert!((log_half - 5.5f32.log10()).abs() < 1e-6);
        // Squared: 0.5^2 = 0.25
        assert!((stretch(0.5, 0.0, 1.0, StretchMode::Squared) - 0.25).abs() < 1e-6);
        // Sqrt: sqrt(0.25) = 0.5
        assert!((stretch(0.25, 0.0, 1.0, StretchMode::Sqrt) - 0.5).abs() < 1e-6);
        // Asinh: asinh(5)/asinh(10)
        let a = stretch(0.5, 0.0, 1.0, StretchMode::Asinh);
        assert!((a - 5.0f32.asinh() / 10.0f32.asinh()).abs() < 1e-6);
    }

    #[test]
    fn stretch_degenerate_window_and_nonfinite() {
        // Degenerate window (max <= min) -> 0.5.
        assert_eq!(stretch(0.7, 0.5, 0.5, StretchMode::Linear), 0.5);
        assert_eq!(stretch(0.7, 0.8, 0.2, StretchMode::Linear), 0.5);
        // Non-finite input -> 0 (blank voxels fall to the lowest colormap color).
        assert_eq!(stretch(f32::NAN, 0.0, 1.0, StretchMode::Linear), 0.0);
        assert_eq!(stretch(f32::INFINITY, 0.0, 1.0, StretchMode::Sqrt), 0.0);
    }

    #[test]
    fn stretch_window_clamps_outside_cut() {
        // Below the low cut clamps to 0, above the high cut clamps to 1.
        assert_eq!(stretch(0.1, 0.3, 0.7, StretchMode::Linear), 0.0);
        assert_eq!(stretch(0.9, 0.3, 0.7, StretchMode::Linear), 1.0);
    }

    #[test]
    fn plane_length_and_opaque_alpha() {
        let vol = make_vol(4, 3, 2);
        let px = render_plane_bgra(&vol, 0, (0.0, 1.0), StretchMode::Linear, "Grayscale");
        assert_eq!(px.len(), 4 * 3 * 4);
        // Every 4th byte is the alpha channel and must be fully opaque.
        assert!(px.iter().skip(3).step_by(4).all(|&a| a == 255));
    }

    #[test]
    fn plane_brightness_monotonic_with_value_grayscale() {
        // With a grayscale LUT (r=g=b=t) and a linear full window, the last voxel of a
        // gradient plane must be at least as bright as the first.
        let vol = make_vol(4, 4, 1);
        let px = render_plane_bgra(&vol, 0, (0.0, 1.0), StretchMode::Linear, "Grayscale");
        let last = (4 * 4 - 1) * 4;
        // B, G, R channels all track brightness for grayscale.
        assert!(px[last + 2] >= px[2], "last R {} < first R {}", px[last + 2], px[2]);
        assert!(px[last] >= px[0], "last B {} < first B {}", px[last], px[0]);
    }

    #[test]
    fn plane_channel_clamped_to_range() {
        // Requesting a channel beyond nz-1 renders the last channel rather than panicking.
        let vol = make_vol(2, 2, 3);
        let a = render_plane_bgra(&vol, 99, (0.0, 1.0), StretchMode::Linear, "Grayscale");
        let b = render_plane_bgra(&vol, 2, (0.0, 1.0), StretchMode::Linear, "Grayscale");
        assert_eq!(a, b);
    }

    #[test]
    fn empty_dims_yield_empty_plane() {
        let vol = VolumeData {
            nx: 0,
            ny: 0,
            nz: 0,
            data: Vec::new(),
            name: "empty".into(),
            meta: None,
        };
        assert!(render_plane_bgra(&vol, 0, (0.0, 1.0), StretchMode::Linear, "Inferno").is_empty());
    }

    #[test]
    fn spectrum_length_and_values() {
        let nx = 3;
        let ny = 2;
        let nz = 5;
        let vol = make_vol(nx, ny, nz);
        let (x, y) = (1, 1);
        let spec = extract_spectrum(&vol, x, y);
        assert_eq!(spec.len(), nz);
        // Each entry must equal the raw normalized voxel at (x, y, z).
        let plane = ny * nx;
        for (z, &val) in spec.iter().enumerate() {
            let expected = vol.data[z * plane + y * nx + x];
            assert!((val - expected).abs() < 1e-9, "z={}: {} != {}", z, val, expected);
        }
    }

    #[test]
    fn spectrum_out_of_range_is_empty() {
        let vol = make_vol(3, 3, 4);
        assert!(extract_spectrum(&vol, 3, 0).is_empty());
        assert!(extract_spectrum(&vol, 0, 3).is_empty());
    }

    #[test]
    fn from_index_roundtrip() {
        for (i, m) in StretchMode::ALL.iter().enumerate() {
            assert_eq!(StretchMode::from_index(i), *m);
        }
        assert_eq!(StretchMode::from_index(99), StretchMode::Linear);
    }
}
