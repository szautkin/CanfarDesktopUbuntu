//! 3D scalar volume for the Cube Viewer GL ray-marcher.
//!
//! One-to-one port of `Services/CubeViewer/VolumeData.cs` (plus the display-metadata
//! surface of `CubeMetadata` from `Services/CubeViewer/CubeWcs.cs`).
//!
//! Voxels are stored **x-fastest, then y, then z**, matching the
//! `pz*nx*ny + py*nx + px` indexing used by the HLSL sampler on the reference app
//! and by the GLSL sampler here. Values are normalized to roughly `[0, 1]` so the
//! shader's window mapping operates directly. NaN voxels are preserved verbatim as
//! blank/BLANK markers (the ray-marcher skips them).

/// Display metadata for a loaded cube: object/instrument labels and the physical
/// value extremes surfaced in the info panel and export plate.
///
/// Populated by the FITS cube reader; `None` on the synthetic fallback volume.
/// Fields are `Option` so the reader can fill only what the header provides
/// (`CubeMetadata { object: Some(..), ..Default::default() }`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CubeMetadata {
    pub object: Option<String>,
    pub telescope: Option<String>,
    pub instrument: Option<String>,
    pub bunit: Option<String>,
    /// True full-cube minimum (physical, in `bunit`), NaN voxels excluded.
    pub data_min: f64,
    /// True full-cube maximum (physical, in `bunit`), NaN voxels excluded.
    pub data_max: f64,
    /// Median of the finite sample (physical).
    pub median: f64,
    /// Physical values the display normalization maps to 0 and 1 (the p0.5…p99.5 cut).
    pub norm_lo: f64,
    pub norm_hi: f64,
    /// Fraction of voxels that were NaN/Inf, 0..1.
    pub nan_fraction: f64,
    /// Original full-resolution NAXIS1/2/3.
    pub nx: usize,
    pub ny: usize,
    pub nz: usize,
    /// Rendered (down-sampled) dimensions actually uploaded to the GPU.
    pub render_nx: usize,
    pub render_ny: usize,
    pub render_nz: usize,
}

/// Map an index in the FILE's coordinate space onto the resident array, which
/// may have been strided down before upload.
///
/// A large cube is decimated to fit the GPU, but the UI, the WCS and every
/// caller work in native pixels. Bounds-checking or indexing against the
/// resident array directly makes valid native coordinates look out-of-range —
/// the reference shipped exactly that bug. The `min` guards the last row/column
/// when the stride does not divide evenly.
///
/// Returns 0 for a degenerate (zero-length) axis rather than dividing by zero.
pub fn native_to_resident(native_index: usize, native_len: usize, resident_len: usize) -> usize {
    if native_len == 0 || resident_len == 0 {
        return 0;
    }
    (native_index * resident_len / native_len).min(resident_len - 1)
}

/// The inverse: which FILE index a resident slot stands for. Used to label a
/// strided channel with its true world value.
pub fn resident_to_native(resident_index: usize, resident_len: usize, native_len: usize) -> usize {
    if resident_len == 0 || native_len == 0 {
        return 0;
    }
    (resident_index * native_len / resident_len).min(native_len - 1)
}

impl CubeMetadata {
    /// Physical value at a normalized display position `t ∈ [0,1]` — maps through
    /// the display cut (NOT the full extremes), so colorbar/hover labels are correct.
    pub fn value_at_normalized(&self, t: f64) -> f64 {
        self.norm_lo + (self.norm_hi - self.norm_lo) * t
    }

    /// True when the GPU volume was strided below the native dimensions.
    pub fn is_downsampled(&self) -> bool {
        self.render_nx != self.nx || self.render_ny != self.ny || self.render_nz != self.nz
    }

    /// How the in-memory volume relates to the file (info-panel Mode row).
    pub fn mode_text(&self) -> String {
        if self.is_downsampled() {
            format!(
                "Downsampled to GPU cap ({}×{}×{})",
                self.render_nx, self.render_ny, self.render_nz
            )
        } else {
            "Resident (full)".to_string()
        }
    }
}

/// A normalized 3D scalar field ready for upload to a GL `R16F`/`R32F` Texture3D.
///
/// `data.len() == nx * ny * nz`, x-fastest ordering, values in `[0, 1]` with NaN
/// marking blank voxels.
#[derive(Debug, Clone)]
pub struct VolumeData {
    /// X dimension (voxels).
    pub nx: usize,
    /// Y dimension (voxels).
    pub ny: usize,
    /// Z dimension (voxels / spectral channels).
    pub nz: usize,
    /// Voxel data, length `nx*ny*nz`, x-fastest then y then z, normalized `[0,1]`,
    /// NaN = blank/BLANK.
    pub data: Vec<f32>,
    /// A short human-readable label (file name or "Synthetic …").
    ///
    /// Read by tests only: the viewer titles its tab from the path it opened,
    /// not from the volume. Kept because every loader sets it and a volume that
    /// cannot say what it is would be worse than an unread string.
    #[allow(dead_code)]
    pub name: String,
    /// WCS + value statistics for a real FITS cube (`None` for the synthetic volume).
    pub meta: Option<CubeMetadata>,
}

impl VolumeData {
    #[cfg(test)]
    /// Construct from an already-normalized, x-fastest voxel buffer.
    ///
    /// Debug builds assert the buffer length matches `nx*ny*nz`.
    pub fn new(
        nx: usize,
        ny: usize,
        nz: usize,
        data: Vec<f32>,
        name: String,
        meta: Option<CubeMetadata>,
    ) -> Self {
        debug_assert_eq!(
            data.len(),
            nx * ny * nz,
            "VolumeData buffer length {} != nx*ny*nz {}",
            data.len(),
            nx * ny * nz
        );
        VolumeData {
            nx,
            ny,
            nz,
            data,
            name,
            meta,
        }
    }

    #[cfg(test)]
    /// Total voxel count (`nx*ny*nz`).
    #[inline]
    pub fn voxel_count(&self) -> usize {
        self.nx * self.ny * self.nz
    }

    /// Flat buffer index for voxel `(x, y, z)`, x-fastest: `(z*ny + y)*nx + x`
    /// (identical to the reference `pz*nx*ny + py*nx + px`).
    #[inline]
    pub fn index(&self, x: usize, y: usize, z: usize) -> usize {
        (z * self.ny + y) * self.nx + x
    }

    /// Voxel value at `(x, y, z)` (may be NaN for a blank voxel). Panics on
    /// out-of-bounds coordinates, matching the reference sampler's fast path.
    #[inline]
    pub fn sample(&self, x: usize, y: usize, z: usize) -> f32 {
        self.data[self.index(x, y, z)]
    }

    #[cfg(test)]
    /// Normalize physical voxel values into `[0, 1]` against a display cut
    /// `[lo, hi]`, clamping in-range values and **preserving NaN** (blank/BLANK)
    /// voxels. Mirrors the FITS cube reader's `p0.5…p99.5` normalization so the
    /// shader window maps `lo→0`, `hi→1`. A degenerate `lo==hi` cut maps every
    /// finite voxel to 0.
    pub fn normalize_cut(raw: &[f32], lo: f32, hi: f32) -> Vec<f32> {
        let span = hi - lo;
        let inv = if span.abs() > 1e-30 { 1.0 / span } else { 0.0 };
        raw.iter()
            .map(|&v| {
                if v.is_nan() {
                    f32::NAN
                } else {
                    ((v - lo) * inv).clamp(0.0, 1.0)
                }
            })
            .collect()
    }

    #[cfg(test)]
    /// Generate a synthetic procedural "nebula" volume: a soft Gaussian core
    /// modulated by multi-octave value noise plus a couple of off-center clumps,
    /// so the 3D ray-march has genuine internal structure to orbit around.
    ///
    /// `size` is the edge length of the cube (`nx=ny=nz`); 128 is a good default.
    /// `seed` seeds a small deterministic PRNG for reproducible noise.
    ///
    /// Ported from `VolumeData.GenerateSyntheticNebula`. The lattice noise field
    /// is filled from a Rust xorshift PRNG rather than .NET `Random`, so the exact
    /// voxel values differ from the reference while the fractal character matches.
    pub fn generate_synthetic_nebula(size: usize, seed: u64) -> Self {
        let (nx, ny, nz) = (size, size, size);
        let mut data = vec![0.0f32; nx * ny * nz];

        // Pre-build a small 3D hash-noise lattice, sampled trilinearly at several
        // octaves to get a cloudy fractal field.
        let mut rng = Xorshift64::new(seed);
        let mut noise = vec![0.0f32; LATTICE * LATTICE * LATTICE];
        for v in noise.iter_mut() {
            *v = rng.next_unit();
        }

        // Two extra emission clumps to break radial symmetry: (cx, cy, cz, r, w).
        let clumps: [(f32, f32, f32, f32, f32); 2] =
            [(0.33, 0.40, 0.55, 0.16, 0.9), (0.66, 0.62, 0.42, 0.12, 0.7)];

        // Guard against a 1-voxel edge (division by nz-1 etc.).
        let denom = |n: usize| if n > 1 { (n - 1) as f32 } else { 1.0 };

        for z in 0..nz {
            let fz = z as f32 / denom(nz);
            let dz = fz - 0.5;
            for y in 0..ny {
                let fy = y as f32 / denom(ny);
                let dy = fy - 0.5;
                let row_base = (z * ny + y) * nx;
                for x in 0..nx {
                    let fx = x as f32 / denom(nx);
                    let dx = fx - 0.5;

                    // Soft Gaussian core (anisotropic, slightly elongated on Z).
                    let r2 = dx * dx + dy * dy + (dz * dz) * 0.6;
                    let core = (-r2 / (2.0 * 0.045)).exp();

                    // Cloudy structure: fbm modulates the core; a shell ring adds wisps.
                    let cloud = fbm(&noise, fx * 2.3, fy * 2.3, fz * 2.3);
                    let shell = (-(r2.sqrt() - 0.28).powi(2) / 0.010).exp();

                    let mut v = core * (0.45 + 0.85 * cloud) + 0.35 * shell * cloud;

                    for &(cx, cy, cz, r, w) in &clumps {
                        let cdx = fx - cx;
                        let cdy = fy - cy;
                        let cdz = fz - cz;
                        let cr2 = cdx * cdx + cdy * cdy + cdz * cdz;
                        v += w * (-cr2 / (2.0 * r * r)).exp() * (0.5 + 0.7 * cloud);
                    }

                    // Mild noise floor so the transfer function has something to cut.
                    v += 0.02 * cloud;

                    v = v.clamp(0.0, 1.3);
                    data[row_base + x] = v;
                }
            }
        }

        // Normalize to a robust max so the default window [0,1] frames it well.
        let mut max = 0.0f32;
        for &v in &data {
            if v > max {
                max = v;
            }
        }
        if max > 1e-4 {
            let inv = 1.0 / max;
            for v in data.iter_mut() {
                *v *= inv;
            }
        }

        VolumeData {
            nx,
            ny,
            nz,
            data,
            name: format!("Synthetic Nebula {}³", size),
            meta: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Synthetic-noise helpers (module-private).
// ---------------------------------------------------------------------------

#[cfg(test)]
/// Edge length of the hash-noise lattice (matches the reference `const lattice = 24`).
const LATTICE: usize = 24;

#[cfg(test)]
/// A tiny deterministic xorshift64 PRNG used only to fill the noise lattice.
/// (.NET `Random` is not reproducible in Rust, so we substitute this.)
struct Xorshift64(u64);

#[cfg(test)]
impl Xorshift64 {
    fn new(seed: u64) -> Self {
        // Ensure a nonzero state (xorshift is undefined at 0); mix the seed.
        let mixed = seed
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .wrapping_add(0xD1B5_4A32_D192_ED03);
        Xorshift64(if mixed == 0 {
            0x1234_5678_9ABC_DEF0
        } else {
            mixed
        })
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// A float in `[0, 1)` from the top 24 bits.
    fn next_unit(&mut self) -> f32 {
        ((self.next_u64() >> 40) as f32) / ((1u32 << 24) as f32)
    }
}

#[cfg(test)]
#[inline]
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

#[cfg(test)]
/// Trilinearly sample the wrapped noise lattice at fractional coordinates.
fn sample_lattice(noise: &[f32], fx: f32, fy: f32, fz: f32) -> f32 {
    let l = LATTICE as f32;
    let fx = fx * l;
    let fy = fy * l;
    let fz = fz * l;
    let x0 = fx.floor() as i32;
    let y0 = fy.floor() as i32;
    let z0 = fz.floor() as i32;
    let tx = fx - x0 as f32;
    let ty = fy - y0 as f32;
    let tz = fz - z0 as f32;

    let li = LATTICE as i32;
    let wrap = |v: i32| (((v % li) + li) % li) as usize;
    let x0w = wrap(x0);
    let y0w = wrap(y0);
    let z0w = wrap(z0);
    let x1 = (x0w + 1) % LATTICE;
    let y1 = (y0w + 1) % LATTICE;
    let z1 = (z0w + 1) % LATTICE;

    let n = |x: usize, y: usize, z: usize| noise[(z * LATTICE + y) * LATTICE + x];
    let c00 = lerp(n(x0w, y0w, z0w), n(x1, y0w, z0w), tx);
    let c10 = lerp(n(x0w, y1, z0w), n(x1, y1, z0w), tx);
    let c01 = lerp(n(x0w, y0w, z1), n(x1, y0w, z1), tx);
    let c11 = lerp(n(x0w, y1, z1), n(x1, y1, z1), tx);
    let c0 = lerp(c00, c10, ty);
    let c1 = lerp(c01, c11, ty);
    lerp(c0, c1, tz)
}

#[cfg(test)]
/// 4-octave fractional Brownian motion over the noise lattice (~`[0, 1)`).
fn fbm(noise: &[f32], x: f32, y: f32, z: f32) -> f32 {
    let mut sum = 0.0f32;
    let mut amp = 0.5f32;
    let mut freq = 1.0f32;
    for _ in 0..4 {
        sum += amp * sample_lattice(noise, x * freq, y * freq, z * freq);
        freq *= 2.07;
        amp *= 0.5;
    }
    sum
}

#[cfg(test)]
mod tests {
    #[test]
    fn native_to_resident_spans_the_whole_resident_axis() {
        // A 1000-pixel axis strided to 250: the first and last native pixels must
        // land on the first and last resident slots, and nothing may exceed the
        // array bounds.
        assert_eq!(native_to_resident(0, 1000, 250), 0);
        assert_eq!(native_to_resident(999, 1000, 250), 249);
        for n in 0..1000 {
            assert!(native_to_resident(n, 1000, 250) < 250);
        }
    }

    #[test]
    fn native_to_resident_is_identity_when_nothing_was_strided() {
        for n in 0..64 {
            assert_eq!(native_to_resident(n, 64, 64), n);
        }
    }

    #[test]
    fn native_to_resident_clamps_an_uneven_stride() {
        // 10 → 3 does not divide evenly; the last native index must still be in
        // range rather than running one past the end.
        assert_eq!(native_to_resident(9, 10, 3), 2);
        assert_eq!(
            native_to_resident(10, 10, 3),
            2,
            "an over-range input is clamped"
        );
    }

    #[test]
    fn resident_to_native_labels_a_strided_channel_with_a_file_channel() {
        // 250 resident channels standing for 1000 native ones: slot 0 is channel 0
        // and the last slot names a channel near the end, never past it.
        assert_eq!(resident_to_native(0, 250, 1000), 0);
        assert_eq!(resident_to_native(249, 250, 1000), 996);
        for r in 0..250 {
            assert!(resident_to_native(r, 250, 1000) < 1000);
        }
    }

    #[test]
    fn degenerate_axes_do_not_divide_by_zero() {
        assert_eq!(native_to_resident(5, 0, 10), 0);
        assert_eq!(native_to_resident(5, 10, 0), 0);
        assert_eq!(resident_to_native(5, 0, 10), 0);
        assert_eq!(resident_to_native(5, 10, 0), 0);
    }

    use super::*;

    #[test]
    fn index_is_x_fastest() {
        let vol = VolumeData::new(2, 3, 4, vec![0.0; 24], "t".into(), None);
        // X-fastest, then Y, then Z: index = (z*ny + y)*nx + x.
        assert_eq!(vol.index(0, 0, 0), 0);
        assert_eq!(vol.index(1, 0, 0), 1); // step in x
        assert_eq!(vol.index(0, 1, 0), 2); // step in y == nx
        assert_eq!(vol.index(0, 0, 1), 6); // step in z == nx*ny
        assert_eq!(vol.index(1, 2, 3), (3 * 3 + 2) * 2 + 1);
        // Matches the reference pz*nx*ny + py*nx + px formula.
        assert_eq!(vol.index(1, 2, 3), 3 * 2 * 3 + 2 * 2 + 1);
    }

    #[test]
    fn sample_reads_the_indexed_voxel() {
        let mut data = vec![0.0f32; 8];
        // 2x2x2 cube; tag voxel (1,1,1).
        let vol_dims = (2usize, 2usize, 2usize);
        let idx = (vol_dims.1 + 1) * vol_dims.0 + 1; // = 7
        data[idx] = 0.75;
        let vol = VolumeData::new(vol_dims.0, vol_dims.1, vol_dims.2, data, "t".into(), None);
        assert_eq!(vol.voxel_count(), 8);
        assert_eq!(vol.sample(1, 1, 1), 0.75);
        assert_eq!(vol.sample(0, 0, 0), 0.0);
    }

    #[test]
    fn normalize_cut_maps_and_preserves_nan() {
        let raw = vec![0.0f32, 5.0, 10.0, -3.0, 20.0, f32::NAN];
        let out = VolumeData::normalize_cut(&raw, 0.0, 10.0);
        assert!((out[0] - 0.0).abs() < 1e-6);
        assert!((out[1] - 0.5).abs() < 1e-6);
        assert!((out[2] - 1.0).abs() < 1e-6);
        assert!((out[3] - 0.0).abs() < 1e-6); // below-cut clamps to 0
        assert!((out[4] - 1.0).abs() < 1e-6); // above-cut clamps to 1
        assert!(out[5].is_nan()); // blank voxel preserved
    }

    #[test]
    fn normalize_cut_degenerate_window() {
        let raw = vec![1.0f32, 2.0, f32::NAN];
        let out = VolumeData::normalize_cut(&raw, 5.0, 5.0);
        assert_eq!(out[0], 0.0);
        assert_eq!(out[1], 0.0);
        assert!(out[2].is_nan());
    }

    #[test]
    fn synthetic_nebula_is_finite_normalized() {
        let vol = VolumeData::generate_synthetic_nebula(16, 1234);
        assert_eq!(vol.nx, 16);
        assert_eq!(vol.ny, 16);
        assert_eq!(vol.nz, 16);
        assert_eq!(vol.data.len(), 16 * 16 * 16);
        assert!(vol.meta.is_none());
        assert_eq!(vol.name, "Synthetic Nebula 16³");

        let mut max = 0.0f32;
        for &v in &vol.data {
            assert!(v.is_finite(), "synthetic voxel must be finite");
            assert!((0.0..=1.0).contains(&v), "voxel {} out of [0,1]", v);
            if v > max {
                max = v;
            }
        }
        // Robust-max normalization pins the peak at ~1.0.
        assert!((max - 1.0).abs() < 1e-5, "peak={}", max);
    }

    #[test]
    fn synthetic_nebula_is_deterministic() {
        let a = VolumeData::generate_synthetic_nebula(12, 42);
        let b = VolumeData::generate_synthetic_nebula(12, 42);
        assert_eq!(a.data, b.data);
        let c = VolumeData::generate_synthetic_nebula(12, 43);
        assert_ne!(a.data, c.data);
    }

    #[test]
    fn cube_metadata_partial_construction() {
        let m = CubeMetadata {
            object: Some("M51".into()),
            bunit: Some("Jy/beam".into()),
            data_min: -0.5,
            data_max: 3.25,
            ..Default::default()
        };
        assert_eq!(m.object.as_deref(), Some("M51"));
        assert!(m.telescope.is_none());
        assert!(m.instrument.is_none());
        assert_eq!(m.bunit.as_deref(), Some("Jy/beam"));
        assert_eq!(m.data_min, -0.5);
        assert_eq!(m.data_max, 3.25);
    }
}
