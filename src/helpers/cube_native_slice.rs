//! Persistent native-resolution spectral-plane reader for the Cube Viewer's 2D
//! slice view.
//!
//! Rust port of `Services/CubeViewer/NativeSliceSource.cs`. Where
//! [`cube_loader`](crate::helpers::cube_loader) reads *every* plane once and
//! strides the cube down to the GPU voxel cap, this keeps a CFITSIO handle open
//! for the tab's lifetime and seeks straight to any single channel `z`, reading
//! that plane at **native FITS resolution** via `ffgpxv` (`fpixel = [1,1,z+1]`).
//! It drives the on-screen slice so the displayed plane is full detail (not the
//! down-sampled volume) and lets hover sky coordinates be computed against native
//! pixel coordinates (no stride-factor offset).
//!
//! Like the reference, it is restricted to cubes whose single native plane is
//! modest (≤ 64 MB as `f32`): a larger held buffer would defeat the very reason
//! the volume is down-sampled. Such cubes (and any file CFITSIO cannot open as a
//! seekable NAXIS≥3 image) return `None` from [`NativeSliceSource::try_open`], and
//! the slice view falls back to the down-sampled volume plane.

use std::path::Path;

#[cfg(feature = "fits")]
use std::ffi::CString;

#[cfg(feature = "fits")]
use fitsio_sys as sys;

/// Skip cubes whose single native plane exceeds this as `f32` — the reused
/// per-plane buffer would be too large to hold (mirrors the reference
/// `MaxPlaneBytes`, ~4096² float).
#[cfg(feature = "fits")]
const MAX_PLANE_BYTES: u64 = 64 * 1024 * 1024;

// ---------------------------------------------------------------------------
// Real implementation (feature = "fits")
// ---------------------------------------------------------------------------

/// A persistent, seek-based reader for one cube's spectral planes at native
/// resolution. Opened once per loaded cube and held for its lifetime; serves any
/// channel by reading straight to that plane through CFITSIO (transparent
/// tile-decompression + BSCALE/BZERO applied for free).
#[cfg(feature = "fits")]
pub struct NativeSliceSource {
    fptr: *mut sys::fitsfile,
    naxis: i32,
    nx: usize,
    ny: usize,
    nz: usize,
}

#[cfg(feature = "fits")]
impl Drop for NativeSliceSource {
    fn drop(&mut self) {
        if !self.fptr.is_null() {
            let mut status = 0;
            unsafe {
                sys::ffclos(self.fptr, &mut status);
            }
        }
    }
}

#[cfg(feature = "fits")]
impl NativeSliceSource {
    /// Open a persistent native-plane source for `path`, or `None` when the file
    /// is not a plain, CFITSIO-openable NAXIS≥3 image cube with a modest plane
    /// size (caller then uses the down-sampled volume plane).
    pub fn try_open(path: &Path) -> Option<Self> {
        unsafe { Self::try_open_raw(path) }
    }

    /// Original full-resolution cube dimensions `(nx, ny, nz)`.
    pub fn dims(&self) -> (usize, usize, usize) {
        (self.nx, self.ny, self.nz)
    }

    /// Read native channel `z` and normalize to `[0, 1]` against the display cut
    /// `[norm_lo, norm_hi]` (NaN/Inf kept as NaN), returning a fresh `nx*ny`
    /// buffer in x-fastest row order. `None` on a bad channel or an I/O error.
    pub fn read_channel(&self, z: usize, norm_lo: f64, norm_hi: f64) -> Option<Vec<f32>> {
        unsafe { self.read_channel_raw(z, norm_lo, norm_hi) }
    }

    // ── Raw FFI (mirrors src/helpers/cube_loader.rs patterns) ───────────────

    unsafe fn try_open_raw(path: &Path) -> Option<Self> {
        let path_str = path.to_str()?;
        let c_path = CString::new(path_str).ok()?;

        let mut fptr: *mut sys::fitsfile = std::ptr::null_mut();
        let mut status: i32 = 0;
        sys::ffopen(
            &mut fptr,
            c_path.as_ptr(),
            sys::READONLY as i32,
            &mut status,
        );
        if status != 0 || fptr.is_null() {
            return None;
        }
        // Wrap immediately so any early return closes the handle.
        let mut this = NativeSliceSource {
            fptr,
            naxis: 0,
            nx: 0,
            ny: 0,
            nz: 0,
        };

        // Navigate to the first NAXIS≥3 image HDU (regular or tile-compressed).
        let hdu = this.find_cube_hdu()?;
        let mut hdu_type: i32 = 0;
        status = 0;
        sys::ffmahd(this.fptr, hdu, &mut hdu_type, &mut status);
        if status != 0 {
            return None;
        }

        // Dimensions.
        let mut naxis: i32 = 0;
        status = 0;
        sys::ffgidm(this.fptr, &mut naxis, &mut status);
        if status != 0 || naxis < 3 {
            return None;
        }
        let mut naxes = vec![0i64; naxis as usize];
        status = 0;
        sys::ffgisz(this.fptr, naxis, naxes.as_mut_ptr(), &mut status);
        if status != 0 {
            return None;
        }
        let nx = naxes[0].max(0) as usize;
        let ny = naxes[1].max(0) as usize;
        let nz = naxes[2].max(0) as usize;
        if nx < 1 || ny < 1 || nz < 1 {
            return None;
        }
        // Held per-plane buffer must stay modest.
        if (nx as u64) * (ny as u64) * 4 > MAX_PLANE_BYTES {
            return None;
        }

        this.naxis = naxis;
        this.nx = nx;
        this.ny = ny;
        this.nz = nz;
        Some(this)
    }

    /// Walk all HDUs; return the 1-based index of the first whose image has
    /// NAXIS≥3 (regular image HDU or a tile-compressed BINTABLE). Mirrors
    /// `cube_loader::find_cube_hdu`.
    unsafe fn find_cube_hdu(&self) -> Option<i32> {
        let mut num_hdus: i32 = 0;
        let mut status = 0;
        sys::ffthdu(self.fptr, &mut num_hdus, &mut status);
        if status != 0 {
            return None;
        }

        for hdu_idx in 1..=num_hdus {
            let mut hdu_type: i32 = 0;
            status = 0;
            sys::ffmahd(self.fptr, hdu_idx, &mut hdu_type, &mut status);
            if status != 0 {
                continue;
            }

            if hdu_type == sys::IMAGE_HDU as i32 {
                if let Some(naxis) = self.current_naxis() {
                    if naxis >= 3 {
                        return Some(hdu_idx);
                    }
                }
                continue;
            }

            if hdu_type == sys::BINARY_TBL as i32 {
                let mut c_status = 0;
                let is_compressed = sys::fits_is_compressed_image(self.fptr, &mut c_status);
                if c_status == 0 && is_compressed != 0 {
                    if let Some(naxis) = self.current_naxis() {
                        if naxis >= 3 {
                            return Some(hdu_idx);
                        }
                    }
                }
            }
        }
        None
    }

    /// Axis count of the *current* HDU, or `None` on error.
    unsafe fn current_naxis(&self) -> Option<i32> {
        let mut naxis: i32 = 0;
        let mut status = 0;
        sys::ffgidm(self.fptr, &mut naxis, &mut status);
        if status == 0 {
            Some(naxis)
        } else {
            None
        }
    }

    unsafe fn read_channel_raw(&self, z: usize, norm_lo: f64, norm_hi: f64) -> Option<Vec<f32>> {
        if self.fptr.is_null() || z >= self.nz || self.naxis < 3 {
            return None;
        }
        let plane_vox = self.nx.checked_mul(self.ny)?;
        if plane_vox == 0 {
            return None;
        }

        // Point fpixel at the first pixel of plane z: [1, 1, z+1, 1, …].
        let mut fpixel = vec![1i64; self.naxis as usize];
        fpixel[2] = (z as i64) + 1;

        let mut plane = vec![0.0f64; plane_vox];
        let mut nulval = f64::NAN;
        let mut anynul: i32 = 0;
        let mut status: i32 = 0;
        sys::ffgpxv(
            self.fptr,
            sys::TDOUBLE as i32,
            fpixel.as_mut_ptr(),
            plane_vox as i64,
            &mut nulval as *mut f64 as *mut std::os::raw::c_void,
            plane.as_mut_ptr() as *mut std::os::raw::c_void,
            &mut anynul,
            &mut status,
        );
        if status != 0 {
            return None;
        }

        // Normalize against the display cut, keeping NaN/Inf as NaN.
        let lo = norm_lo;
        let hi = norm_hi;
        let range = if hi > lo { hi - lo } else { 1.0 };
        let out: Vec<f32> = plane
            .iter()
            .map(|&v| {
                if v.is_finite() {
                    (((v - lo) / range).clamp(0.0, 1.0)) as f32
                } else {
                    f32::NAN
                }
            })
            .collect();
        Some(out)
    }
}

// ---------------------------------------------------------------------------
// Stub when cfitsio is not compiled in
// ---------------------------------------------------------------------------

/// Stub native-plane source: cfitsio was not compiled in, so there is never a
/// native plane — the slice view always uses the down-sampled volume plane.
#[cfg(not(feature = "fits"))]
pub struct NativeSliceSource;

#[cfg(not(feature = "fits"))]
impl NativeSliceSource {
    /// Always `None` without the `fits` feature.
    pub fn try_open(_path: &Path) -> Option<Self> {
        None
    }

    /// Unreachable without the `fits` feature (no instance can exist).
    pub fn dims(&self) -> (usize, usize, usize) {
        (0, 0, 0)
    }

    /// Unreachable without the `fits` feature (no instance can exist).
    pub fn read_channel(&self, _z: usize, _norm_lo: f64, _norm_hi: f64) -> Option<Vec<f32>> {
        None
    }
}
