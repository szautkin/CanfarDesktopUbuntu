use crate::models::FitsImageData;

/// Image stretch function types
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Stretch {
    Linear,
    Log,
    Sqrt,
    HistogramEq,
}

/// Color map types
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ColorMap {
    Grayscale,
    Heat,
    Viridis,
}

/// Render a FITS image to an RGBA buffer using the specified stretch and color map.
/// Returns a Vec<u8> of length width * height * 4 (RGBA).
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

    // For histogram equalization, precompute the CDF
    let cdf = if stretch == Stretch::HistogramEq {
        Some(compute_cdf(&data.pixels, vmin, vmax, 65536))
    } else {
        None
    };

    for i in 0..npixels {
        let val = data.pixels[i];
        let normalized = normalize(val, vmin, vmax, stretch, cdf.as_deref());
        let (r, g, b) = apply_colormap(normalized, colormap);
        rgba[i * 4] = r;
        rgba[i * 4 + 1] = g;
        rgba[i * 4 + 2] = b;
        rgba[i * 4 + 3] = 255;
    }

    rgba
}

fn normalize(val: f64, vmin: f64, vmax: f64, stretch: Stretch, cdf: Option<&[f64]>) -> f64 {
    let clamped = val.clamp(vmin, vmax);
    let range = vmax - vmin;
    if range <= 0.0 {
        return 0.0;
    }

    match stretch {
        Stretch::Linear => (clamped - vmin) / range,
        Stretch::Log => {
            let scaled = (clamped - vmin) / range;
            (1.0 + scaled * 999.0).log10() / 3.0
        }
        Stretch::Sqrt => ((clamped - vmin) / range).sqrt(),
        Stretch::HistogramEq => {
            if let Some(cdf) = cdf {
                let idx = (((clamped - vmin) / range) * (cdf.len() - 1) as f64) as usize;
                let idx = idx.min(cdf.len() - 1);
                cdf[idx]
            } else {
                (clamped - vmin) / range
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
        let idx = idx.min(nbins - 1);
        histogram[idx] += 1;
    }

    let total = pixels.len() as f64;
    let mut cdf = vec![0.0; nbins];
    let mut cumulative = 0u64;
    for i in 0..nbins {
        cumulative += histogram[i];
        cdf[i] = cumulative as f64 / total;
    }
    cdf
}

fn apply_colormap(val: f64, colormap: ColorMap) -> (u8, u8, u8) {
    let v = val.clamp(0.0, 1.0);
    match colormap {
        ColorMap::Grayscale => {
            let g = (v * 255.0) as u8;
            (g, g, g)
        }
        ColorMap::Heat => {
            let r = (v * 3.0).min(1.0);
            let g = ((v - 0.33) * 3.0).clamp(0.0, 1.0);
            let b = ((v - 0.67) * 3.0).clamp(0.0, 1.0);
            ((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
        }
        ColorMap::Viridis => viridis_lookup(v),
    }
}

fn viridis_lookup(t: f64) -> (u8, u8, u8) {
    // Simplified viridis approximation
    let r = (-0.35 * (1.0 - t) + 0.99 * t).clamp(0.0, 1.0);
    let g = (0.0 + 0.87 * (4.0 * t * (1.0 - t)).sqrt()).clamp(0.0, 1.0);
    let b = (0.53 - 0.2 * t + 0.47 * (1.0 - t)).clamp(0.0, 1.0);
    ((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_stretch() {
        assert!((normalize(5.0, 0.0, 10.0, Stretch::Linear, None) - 0.5).abs() < 1e-10);
        assert!((normalize(0.0, 0.0, 10.0, Stretch::Linear, None) - 0.0).abs() < 1e-10);
        assert!((normalize(10.0, 0.0, 10.0, Stretch::Linear, None) - 1.0).abs() < 1e-10);
    }

    #[test]
    fn grayscale_map() {
        let (r, g, b) = apply_colormap(0.0, ColorMap::Grayscale);
        assert_eq!((r, g, b), (0, 0, 0));
        let (r, g, b) = apply_colormap(1.0, ColorMap::Grayscale);
        assert_eq!((r, g, b), (255, 255, 255));
    }

    #[test]
    fn render_small_image() {
        let data = FitsImageData::new(
            2,
            2,
            vec![0.0, 0.5, 0.5, 1.0],
            std::collections::HashMap::new(),
        );
        let rgba = render_to_rgba(&data, Stretch::Linear, ColorMap::Grayscale, 0.0, 1.0);
        assert_eq!(rgba.len(), 16);
        assert_eq!(rgba[0], 0); // first pixel R = 0
        assert_eq!(rgba[3], 255); // first pixel A = 255
    }
}
