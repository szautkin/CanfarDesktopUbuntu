use crate::models::FitsImageData;

/// Maximum number of pixels to sample when computing percentile cuts.
const AUTO_CUT_SAMPLE_SIZE: usize = 100_000;

/// Image stretch function types.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Stretch {
    Linear,
    Log,
    Sqrt,
    Squared,
    Asinh,
    HistogramEq,
}

/// Color map types.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ColorMap {
    Grayscale,
    Inverted,
    Heat,
    Viridis,
    Plasma,
    Inferno,
    Magma,
    CoolWarm,
}

/// Compute low/high cut values from a pixel sample using percentiles.
///
/// Samples up to `AUTO_CUT_SAMPLE_SIZE` pixels evenly from `data`, sorts the
/// sample, and returns the `low_pct`th and `high_pct`th percentile values.
/// Typical defaults: `low_pct = 0.5`, `high_pct = 99.5`.
pub fn auto_cut(data: &[f64], low_pct: f64, high_pct: f64) -> (f64, f64) {
    if data.is_empty() {
        return (0.0, 1.0);
    }

    let n = data.len();
    let step = if n > AUTO_CUT_SAMPLE_SIZE {
        n / AUTO_CUT_SAMPLE_SIZE
    } else {
        1
    };

    let mut sample: Vec<f64> = data
        .iter()
        .step_by(step)
        .cloned()
        .filter(|v| v.is_finite())
        .collect();

    if sample.is_empty() {
        return (0.0, 1.0);
    }

    sample.sort_by(|a, b| a.total_cmp(b));

    let low_idx = ((low_pct / 100.0) * (sample.len() - 1) as f64)
        .round()
        .clamp(0.0, (sample.len() - 1) as f64) as usize;
    let high_idx = ((high_pct / 100.0) * (sample.len() - 1) as f64)
        .round()
        .clamp(0.0, (sample.len() - 1) as f64) as usize;

    let lo = sample[low_idx];
    let hi = sample[high_idx];

    if (hi - lo).abs() < f64::EPSILON {
        (lo - 1.0, hi + 1.0)
    } else {
        (lo, hi)
    }
}

/// Render a FITS image to an RGBA buffer using the specified stretch and color map.
///
/// Returns a `Vec<u8>` of length `width * height * 4` (RGBA, one byte per channel).
pub fn render_to_rgba(
    data: &FitsImageData,
    stretch: Stretch,
    colormap: ColorMap,
    vmin: f64,
    vmax: f64,
) -> Vec<u8> {
    let npixels = data.width * data.height;
    let mut rgba = vec![0u8; npixels * 4];

    let range = vmax - vmin;
    if range <= 0.0 {
        return rgba;
    }

    let cdf = if stretch == Stretch::HistogramEq {
        Some(compute_cdf(&data.pixels, vmin, vmax, 65536))
    } else {
        None
    };

    let lut = build_lut(colormap);

    for i in 0..npixels {
        let val = data.pixels[i];
        let t = normalize(val, vmin, vmax, stretch, cdf.as_deref());
        let idx = (t * 255.0).round().clamp(0.0, 255.0) as usize;
        let (r, g, b) = lut[idx];
        rgba[i * 4] = r;
        rgba[i * 4 + 1] = g;
        rgba[i * 4 + 2] = b;
        rgba[i * 4 + 3] = 255;
    }

    rgba
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn normalize(val: f64, vmin: f64, vmax: f64, stretch: Stretch, cdf: Option<&[f64]>) -> f64 {
    let clamped = val.clamp(vmin, vmax);
    let range = vmax - vmin;
    if range <= 0.0 {
        return 0.0;
    }
    let x = (clamped - vmin) / range;

    match stretch {
        Stretch::Linear => x,
        Stretch::Log => (1.0 + x * 999.0).log10() / 3.0,
        Stretch::Sqrt => x.sqrt(),
        Stretch::Squared => x * x,
        Stretch::Asinh => {
            let norm = (10.0_f64).asinh();
            (10.0 * x).asinh() / norm
        }
        Stretch::HistogramEq => {
            if let Some(cdf) = cdf {
                let idx = (x * (cdf.len() - 1) as f64) as usize;
                cdf[idx.min(cdf.len() - 1)]
            } else {
                x
            }
        }
    }
}

fn compute_cdf(pixels: &[f64], vmin: f64, vmax: f64, nbins: usize) -> Vec<f64> {
    let range = vmax - vmin;
    let mut histogram = vec![0u64; nbins];

    for &val in pixels {
        let clamped = val.clamp(vmin, vmax);
        let idx = (((clamped - vmin) / range) * (nbins - 1) as f64) as usize;
        histogram[idx.min(nbins - 1)] += 1;
    }

    let total = pixels.len() as f64;
    let mut cdf = vec![0.0f64; nbins];
    let mut cumulative = 0u64;
    for i in 0..nbins {
        cumulative += histogram[i];
        cdf[i] = cumulative as f64 / total;
    }
    cdf
}

fn build_lut(colormap: ColorMap) -> [(u8, u8, u8); 256] {
    let mut lut = [(0u8, 0u8, 0u8); 256];
    for (i, entry) in lut.iter_mut().enumerate() {
        let v = i as f64 / 255.0;
        *entry = match colormap {
            ColorMap::Grayscale => {
                let g = i as u8;
                (g, g, g)
            }
            ColorMap::Inverted => {
                let g = (255 - i) as u8;
                (g, g, g)
            }
            ColorMap::Heat => heat_lookup(v),
            ColorMap::Viridis => viridis_lookup(v),
            ColorMap::Plasma => plasma_lookup(v),
            ColorMap::Inferno => inferno_lookup(v),
            ColorMap::Magma => magma_lookup(v),
            ColorMap::CoolWarm => coolwarm_lookup(v),
        };
    }
    lut
}

// ---------------------------------------------------------------------------
// Colormap implementations
// ---------------------------------------------------------------------------

fn heat_lookup(v: f64) -> (u8, u8, u8) {
    let r = (v * 3.0).min(1.0);
    let g = ((v - 0.33) * 3.0).clamp(0.0, 1.0);
    let b = ((v - 0.67) * 3.0).clamp(0.0, 1.0);
    (f2u(r), f2u(g), f2u(b))
}

fn viridis_lookup(t: f64) -> (u8, u8, u8) {
    let pts: &[(f64, f64, f64, f64)] = &[
        (0.000, 0.267, 0.005, 0.329),
        (0.125, 0.283, 0.141, 0.458),
        (0.250, 0.254, 0.265, 0.530),
        (0.375, 0.207, 0.372, 0.553),
        (0.500, 0.164, 0.471, 0.558),
        (0.625, 0.128, 0.567, 0.551),
        (0.750, 0.198, 0.661, 0.478),
        (0.875, 0.427, 0.752, 0.346),
        (1.000, 0.993, 0.906, 0.144),
    ];
    interpolate_colormap(t, pts)
}

fn plasma_lookup(t: f64) -> (u8, u8, u8) {
    let pts: &[(f64, f64, f64, f64)] = &[
        (0.000, 0.050, 0.030, 0.528),
        (0.125, 0.296, 0.007, 0.625),
        (0.250, 0.500, 0.015, 0.659),
        (0.375, 0.670, 0.122, 0.605),
        (0.500, 0.799, 0.240, 0.485),
        (0.625, 0.893, 0.362, 0.348),
        (0.750, 0.951, 0.514, 0.201),
        (0.875, 0.977, 0.694, 0.069),
        (1.000, 0.940, 0.975, 0.131),
    ];
    interpolate_colormap(t, pts)
}

fn inferno_lookup(t: f64) -> (u8, u8, u8) {
    let pts: &[(f64, f64, f64, f64)] = &[
        (0.000, 0.000, 0.000, 0.014),
        (0.125, 0.102, 0.023, 0.216),
        (0.250, 0.302, 0.047, 0.416),
        (0.375, 0.509, 0.082, 0.467),
        (0.500, 0.706, 0.149, 0.329),
        (0.625, 0.867, 0.290, 0.137),
        (0.750, 0.960, 0.494, 0.000),
        (0.875, 0.988, 0.741, 0.000),
        (1.000, 0.988, 1.000, 0.645),
    ];
    interpolate_colormap(t, pts)
}

fn magma_lookup(t: f64) -> (u8, u8, u8) {
    let pts: &[(f64, f64, f64, f64)] = &[
        (0.000, 0.000, 0.000, 0.016),
        (0.125, 0.091, 0.034, 0.205),
        (0.250, 0.263, 0.062, 0.431),
        (0.375, 0.475, 0.094, 0.542),
        (0.500, 0.679, 0.169, 0.576),
        (0.625, 0.861, 0.309, 0.567),
        (0.750, 0.976, 0.525, 0.567),
        (0.875, 0.994, 0.765, 0.698),
        (1.000, 1.000, 1.000, 1.000),
    ];
    interpolate_colormap(t, pts)
}

fn coolwarm_lookup(t: f64) -> (u8, u8, u8) {
    let pts: &[(f64, f64, f64, f64)] = &[
        (0.000, 0.017, 0.357, 0.867),
        (0.250, 0.358, 0.580, 0.930),
        (0.500, 0.900, 0.900, 0.900),
        (0.750, 0.949, 0.447, 0.368),
        (1.000, 0.694, 0.016, 0.016),
    ];
    interpolate_colormap(t, pts)
}

fn interpolate_colormap(t: f64, pts: &[(f64, f64, f64, f64)]) -> (u8, u8, u8) {
    let t = t.clamp(0.0, 1.0);
    let lo = pts.iter().rposition(|pt| pt.0 <= t).unwrap_or(0);
    let hi = (lo + 1).min(pts.len() - 1);

    let (t0, r0, g0, b0) = pts[lo];
    let (t1, r1, g1, b1) = pts[hi];
    let frac = if (t1 - t0).abs() < f64::EPSILON {
        0.0
    } else {
        ((t - t0) / (t1 - t0)).clamp(0.0, 1.0)
    };
    (
        f2u(r0 + frac * (r1 - r0)),
        f2u(g0 + frac * (g1 - g0)),
        f2u(b0 + frac * (b1 - b0)),
    )
}

#[inline]
fn f2u(v: f64) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_image(pixels: Vec<f64>) -> FitsImageData {
        FitsImageData::new(
            pixels.len(),
            1,
            pixels,
            std::collections::HashMap::new(),
        )
    }

    #[test]
    fn linear_stretch_midpoint() {
        assert!((normalize(5.0, 0.0, 10.0, Stretch::Linear, None) - 0.5).abs() < 1e-10);
        assert!((normalize(0.0, 0.0, 10.0, Stretch::Linear, None)).abs() < 1e-10);
        assert!((normalize(10.0, 0.0, 10.0, Stretch::Linear, None) - 1.0).abs() < 1e-10);
    }

    #[test]
    fn squared_stretch() {
        let v = normalize(5.0, 0.0, 10.0, Stretch::Squared, None);
        assert!((v - 0.25).abs() < 1e-10, "squared(0.5) should be 0.25, got {}", v);
    }

    #[test]
    fn asinh_stretch_bounds() {
        let lo = normalize(0.0, 0.0, 10.0, Stretch::Asinh, None);
        let hi = normalize(10.0, 0.0, 10.0, Stretch::Asinh, None);
        assert!(lo.abs() < 1e-10, "asinh(0) should be 0, got {}", lo);
        assert!((hi - 1.0).abs() < 1e-10, "asinh(1) should be 1, got {}", hi);
    }

    #[test]
    fn auto_cut_basic() {
        let data: Vec<f64> = (0..=100).map(|i| i as f64).collect();
        let (lo, hi) = auto_cut(&data, 0.5, 99.5);
        assert!(lo < 2.0, "lo={}", lo);
        assert!(hi > 98.0, "hi={}", hi);
    }

    #[test]
    fn auto_cut_empty() {
        let (lo, hi) = auto_cut(&[], 0.5, 99.5);
        assert_eq!((lo, hi), (0.0, 1.0));
    }

    #[test]
    fn auto_cut_large_sample() {
        let data: Vec<f64> = (0..200_000).map(|i| i as f64).collect();
        let (lo, hi) = auto_cut(&data, 0.5, 99.5);
        assert!(lo >= 0.0);
        assert!(hi <= 200_000.0);
        assert!(hi > lo);
    }

    #[test]
    fn grayscale_extremes() {
        let lut = build_lut(ColorMap::Grayscale);
        assert_eq!(lut[0], (0, 0, 0));
        assert_eq!(lut[255], (255, 255, 255));
    }

    #[test]
    fn inverted_grayscale_extremes() {
        let lut = build_lut(ColorMap::Inverted);
        assert_eq!(lut[0], (255, 255, 255));
        assert_eq!(lut[255], (0, 0, 0));
    }

    #[test]
    fn render_small_image() {
        let data = make_image(vec![0.0, 0.5, 0.5, 1.0]);
        let rgba = render_to_rgba(&data, Stretch::Linear, ColorMap::Grayscale, 0.0, 1.0);
        assert_eq!(rgba.len(), 16);
        assert_eq!(rgba[0], 0);
        assert_eq!(rgba[3], 255);
    }

    #[test]
    fn all_colormaps_render() {
        let data = make_image(vec![0.0, 0.25, 0.5, 0.75, 1.0]);
        let maps = [
            ColorMap::Grayscale,
            ColorMap::Inverted,
            ColorMap::Heat,
            ColorMap::Viridis,
            ColorMap::Plasma,
            ColorMap::Inferno,
            ColorMap::Magma,
            ColorMap::CoolWarm,
        ];
        for cm in maps {
            let rgba = render_to_rgba(&data, Stretch::Linear, cm, 0.0, 1.0);
            assert_eq!(rgba.len(), 5 * 4);
            for chunk in rgba.chunks(4) {
                assert_eq!(chunk[3], 255);
            }
        }
    }
}
