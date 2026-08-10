//! Colormap and opacity-transfer-function lookup tables for the cube volume
//! renderer.
//!
//! One-to-one port of `CanfarDesktop/Services/CubeViewer/CubeColormaps.cs`.
//! Mirrors the macOS pipeline: a 256-entry RGBA8 colormap texture (a single
//! row of a 256x1 texture) plus a 256-entry alpha ramp built from
//! transfer-function control points.
//!
//! The perceptual maps (Viridis/Inferno/Magma/Plasma) use the matplotlib /
//! macOS anchor stops exactly as in the reference; Grayscale/Inverted/Heat/Cool
//! are procedural (matching the 2D FITS viewer).

/// The cube viewer's selectable colormaps, in picker order (mirrors the
/// `CubeColormap` enum: Grayscale, Inverted, Heat, Cool, Viridis, Inferno,
/// Magma, Plasma).
pub const NAMES: &[&str] = &[
    "Grayscale",
    "Inverted",
    "Heat",
    "Cool",
    "Viridis",
    "Inferno",
    "Magma",
    "Plasma",
];

/// Default colormap for the cube viewer.
pub const DEFAULT: &str = "Inferno";

// ---------------------------------------------------------------------------
// Perceptual colormap anchor stops (RGB in 0..1), linearly interpolated to 256
// entries. Inferno/Viridis use accurate matplotlib anchors; Magma/Plasma use
// the exact macOS `cubeColormapStops` 9-anchor tables. Preserved verbatim from
// CubeColormaps.cs.
// ---------------------------------------------------------------------------

/// matplotlib inferno, 17 anchors (t = i/16).
const INFERNO_STOPS: &[(f32, f32, f32)] = &[
    (0.001462, 0.000466, 0.013866),
    (0.046915, 0.030324, 0.150164),
    (0.142378, 0.046242, 0.308553),
    (0.258234, 0.038571, 0.406485),
    (0.366529, 0.071579, 0.431994),
    (0.472328, 0.110547, 0.428334),
    (0.578304, 0.148039, 0.404411),
    (0.682656, 0.189501, 0.360757),
    (0.780517, 0.243327, 0.299523),
    (0.865006, 0.316822, 0.226055),
    (0.929644, 0.411479, 0.145367),
    (0.970919, 0.522853, 0.058367),
    (0.987622, 0.645320, 0.039886),
    (0.978806, 0.774545, 0.176037),
    (0.950018, 0.903409, 0.380271),
    (0.954529, 0.972590, 0.612366),
    (0.988362, 0.998364, 0.644924),
];

/// matplotlib viridis, 11 anchors (t = i/10).
const VIRIDIS_STOPS: &[(f32, f32, f32)] = &[
    (0.267004, 0.004874, 0.329415),
    (0.282623, 0.140926, 0.457517),
    (0.253935, 0.265254, 0.529983),
    (0.206756, 0.371758, 0.553117),
    (0.163625, 0.471133, 0.558148),
    (0.127568, 0.566949, 0.550556),
    (0.134692, 0.658636, 0.517649),
    (0.266941, 0.748751, 0.440573),
    (0.477504, 0.821444, 0.318195),
    (0.741388, 0.873449, 0.149561),
    (0.993248, 0.906157, 0.143936),
];

/// macOS magma, 9 anchors (verbatim from `cubeColormapStops`).
const MAGMA_STOPS: &[(f32, f32, f32)] = &[
    (0.001, 0.000, 0.014),
    (0.078, 0.043, 0.206),
    (0.232, 0.059, 0.438),
    (0.390, 0.100, 0.502),
    (0.550, 0.161, 0.506),
    (0.716, 0.215, 0.475),
    (0.868, 0.288, 0.409),
    (0.967, 0.440, 0.360),
    (0.987, 0.991, 0.749),
];

/// macOS plasma, 9 anchors (verbatim from `cubeColormapStops`).
const PLASMA_STOPS: &[(f32, f32, f32)] = &[
    (0.050, 0.030, 0.528),
    (0.254, 0.013, 0.615),
    (0.417, 0.000, 0.658),
    (0.562, 0.052, 0.641),
    (0.692, 0.165, 0.564),
    (0.798, 0.280, 0.470),
    (0.881, 0.392, 0.383),
    (0.949, 0.518, 0.295),
    (0.940, 0.975, 0.131),
];

/// Default opacity transfer-function control points (value in [0,1] -> alpha in
/// [0,1]), copied from `CubeViewerModel.transferFunction`. Low values fade out
/// (suppress the noise floor), high values become opaque.
pub const DEFAULT_TRANSFER: &[(f32, f32)] = &[(0.0, 0.0), (0.15, 0.09), (0.5, 0.42), (1.0, 1.0)];

/// Build the 256x1 RGBA8 LUT for a colormap (tightly packed, RGBA order),
/// returning a `Vec<u8>` of length `256 * 4`. Unknown names fall back to
/// Inferno (matching the C# `_ => Interpolate(InfernoStops)` default).
pub fn lut_rgba(name: &str) -> Vec<u8> {
    match name {
        "Grayscale" => procedural(|t| (t, t, t)),
        "Inverted" => procedural(|t| (1.0 - t, 1.0 - t, 1.0 - t)),
        "Heat" => procedural(|t| {
            (
                (t * 3.0).clamp(0.0, 1.0),
                ((t - 0.33) * 3.0).clamp(0.0, 1.0),
                ((t - 0.67) * 3.0).clamp(0.0, 1.0),
            )
        }),
        "Cool" => procedural(|t| (t, 1.0 - t, 1.0)),
        "Viridis" => interpolate(VIRIDIS_STOPS),
        "Inferno" => interpolate(INFERNO_STOPS),
        "Magma" => interpolate(MAGMA_STOPS),
        "Plasma" => interpolate(PLASMA_STOPS),
        _ => interpolate(INFERNO_STOPS),
    }
}

/// Build a 256-entry alpha ramp from transfer-function control points. Direct
/// port of the Swift/C# `setTransferFunction` piecewise-linear interpolation.
/// `points` are `(x in [0,1], alpha in [0,1])`; they are sorted by `x` here.
pub fn transfer_ramp(points: &[(f32, f32)]) -> [u8; 256] {
    let mut ramp = [0u8; 256];
    if points.is_empty() {
        return ramp;
    }

    let mut sorted: Vec<(f32, f32)> = points.to_vec();
    sorted.sort_by(|a, b| a.0.total_cmp(&b.0));

    let first = sorted[0];
    let last = sorted[sorted.len() - 1];

    for (i, slot) in ramp.iter_mut().enumerate() {
        let x = i as f32 / 255.0;
        let mut a = first.1;
        if x >= last.0 {
            a = last.1;
        } else {
            for j in 0..sorted.len() - 1 {
                if x >= sorted[j].0 && x < sorted[j + 1].0 {
                    let span = (sorted[j + 1].0 - sorted[j].0).max(1e-6);
                    let f = (x - sorted[j].0) / span;
                    a = sorted[j].1 * (1.0 - f) + sorted[j + 1].1 * f;
                    break;
                }
            }
        }
        *slot = to_byte(a);
    }
    ramp
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn procedural(f: impl Fn(f32) -> (f32, f32, f32)) -> Vec<u8> {
    let mut rgba = vec![0u8; 256 * 4];
    for i in 0..256 {
        let (r, g, b) = f(i as f32 / 255.0);
        let o = i * 4;
        rgba[o] = to_byte(r);
        rgba[o + 1] = to_byte(g);
        rgba[o + 2] = to_byte(b);
        rgba[o + 3] = 255;
    }
    rgba
}

fn interpolate(stops: &[(f32, f32, f32)]) -> Vec<u8> {
    let mut rgba = vec![0u8; 256 * 4];
    let seg_count = stops.len() - 1;
    for i in 0..256 {
        let t = i as f32 / 255.0 * seg_count as f32;
        let k = (t as usize).min(seg_count - 1);
        let f = t - k as f32;
        let a = stops[k];
        let b = stops[k + 1];
        let o = i * 4;
        rgba[o] = to_byte(a.0 + (b.0 - a.0) * f);
        rgba[o + 1] = to_byte(a.1 + (b.1 - a.1) * f);
        rgba[o + 2] = to_byte(a.2 + (b.2 - a.2) * f);
        rgba[o + 3] = 255;
    }
    rgba
}

/// Port of the C# `ToByte`: `(byte)(Clamp(v,0,1) * 255 + 0.5)` — truncating cast
/// of a non-negative value gives round-half-up.
#[inline]
fn to_byte(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_count_and_default() {
        assert_eq!(NAMES.len(), 8);
        assert!(NAMES.contains(&DEFAULT));
        assert_eq!(DEFAULT, "Inferno");
    }

    #[test]
    fn lut_rgba_length_is_1024() {
        for name in NAMES {
            assert_eq!(lut_rgba(name).len(), 256 * 4, "{name} LUT length");
        }
        // Unknown name falls back to Inferno, still 1024 bytes.
        assert_eq!(lut_rgba("does-not-exist").len(), 1024);
    }

    #[test]
    fn inferno_first_entry_is_near_black() {
        let lut = lut_rgba("Inferno");
        // Inferno anchor 0 is (0.001462, 0.000466, 0.013866) -> essentially black.
        assert!(lut[0] <= 2, "R={}", lut[0]);
        assert!(lut[1] <= 2, "G={}", lut[1]);
        assert!(lut[2] <= 4, "B={}", lut[2]);
        assert_eq!(lut[3], 255, "alpha opaque");
    }

    #[test]
    fn inferno_last_entry_is_bright() {
        let lut = lut_rgba("Inferno");
        let o = 255 * 4;
        // Final anchor (0.988362, 0.998364, 0.644924).
        assert!(lut[o] > 240);
        assert!(lut[o + 1] > 240);
        assert_eq!(lut[o + 3], 255);
    }

    #[test]
    fn all_luts_opaque() {
        for name in NAMES {
            let lut = lut_rgba(name);
            for a in lut.iter().skip(3).step_by(4) {
                assert_eq!(*a, 255, "{name} alpha");
            }
        }
    }

    #[test]
    fn grayscale_extremes() {
        let lut = lut_rgba("Grayscale");
        assert_eq!((lut[0], lut[1], lut[2]), (0, 0, 0));
        let o = 255 * 4;
        assert_eq!((lut[o], lut[o + 1], lut[o + 2]), (255, 255, 255));
    }

    #[test]
    fn inverted_extremes() {
        let lut = lut_rgba("Inverted");
        assert_eq!((lut[0], lut[1], lut[2]), (255, 255, 255));
        let o = 255 * 4;
        assert_eq!((lut[o], lut[o + 1], lut[o + 2]), (0, 0, 0));
    }

    #[test]
    fn cool_endpoints() {
        let lut = lut_rgba("Cool");
        // t=0 -> (0,1,1) cyan; t=1 -> (1,0,1) magenta. Blue always full.
        assert_eq!((lut[0], lut[1], lut[2]), (0, 255, 255));
        let o = 255 * 4;
        assert_eq!((lut[o], lut[o + 1], lut[o + 2]), (255, 0, 255));
    }

    #[test]
    fn interpolate_hits_exact_anchors() {
        // i=0 must equal the first anchor exactly (t maps to k=0, f=0).
        let lut = lut_rgba("Viridis");
        assert_eq!(lut[0], to_byte(VIRIDIS_STOPS[0].0));
        assert_eq!(lut[1], to_byte(VIRIDIS_STOPS[0].1));
        assert_eq!(lut[2], to_byte(VIRIDIS_STOPS[0].2));
    }

    #[test]
    fn transfer_ramp_default_monotone_endpoints() {
        let ramp = transfer_ramp(DEFAULT_TRANSFER);
        assert_eq!(ramp[0], 0); // (0,0)
        assert_eq!(ramp[255], 255); // (1,1)
                                    // Non-decreasing since DEFAULT_TRANSFER control points ascend.
        for w in ramp.windows(2) {
            assert!(w[1] >= w[0], "ramp should be non-decreasing");
        }
    }

    #[test]
    fn transfer_ramp_midpoint_matches_control() {
        // At x=0.5 the control alpha is 0.42 -> 107.
        let ramp = transfer_ramp(DEFAULT_TRANSFER);
        let mid = ramp[(0.5 * 255.0) as usize];
        assert_eq!(mid, to_byte(0.42));
    }

    #[test]
    fn transfer_ramp_unsorted_input() {
        // Same points, shuffled — must produce identical ramp (sorted internally).
        let shuffled: &[(f32, f32)] = &[(1.0, 1.0), (0.5, 0.42), (0.0, 0.0), (0.15, 0.09)];
        assert_eq!(transfer_ramp(shuffled), transfer_ramp(DEFAULT_TRANSFER));
    }

    #[test]
    fn transfer_ramp_empty_is_zero() {
        assert_eq!(transfer_ramp(&[]), [0u8; 256]);
    }

    #[test]
    fn transfer_ramp_flat_beyond_last() {
        // A single interior point: everything at/after its x holds its alpha,
        // everything before holds the first (same) alpha.
        let ramp = transfer_ramp(&[(0.3, 0.6)]);
        assert_eq!(ramp[0], to_byte(0.6));
        assert_eq!(ramp[255], to_byte(0.6));
    }
}
