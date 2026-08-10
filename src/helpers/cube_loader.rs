//! Spectral-cube (NAXIS≥3) FITS loader — the Rust port of
//! `Services/CubeViewer/FitsCubeReader.cs` (+ `NativeSliceSource.cs`).
//!
//! Where the C# reference does its own big-endian FITS parsing, we instead
//! drive **raw `fitsio-sys` FFI** exactly like [`crate::helpers::fits_loader`]
//! so we get CFITSIO's transparent tile-decompression (RICE / GZIP / PLIO /
//! HCOMPRESS) and BSCALE/BZERO application for free.  The pipeline mirrors the
//! reference one-to-one:
//!
//! 1. Navigate to the cube HDU (explicit 1-based `hdu`, else the first HDU
//!    whose image has NAXIS ≥ 3 — regular *or* tile-compressed BINTABLE).
//! 2. Read **every** spectral plane as `f64` via `ffgpxv` (auto-decompressed),
//!    accumulating the exact full-cube min/max and NaN count.
//! 3. Stride each axis down so the longest one ≤ 256 (keeps the RAM / GPU
//!    Texture3D budget sane; guarantees total voxels ≤ 256³).
//! 4. Take a robust p0.5 / p99.5 cut from the (strided) finite sample and
//!    normalize the kept voxels into `[0,1]` as `f32`, keeping NaN for masked
//!    voxels so the ray-march / slice view can skip them.
//!
//! Voxels are stored **X-fastest, then Y, then Z** — the ordering the shared
//! [`VolumeData`](crate::models::volume_data::VolumeData) contract and the GL
//! sampler expect (`index = (z*ny + y)*nx + x`).

use std::path::Path;

#[cfg(feature = "fits")]
use std::collections::HashMap;
#[cfg(feature = "fits")]
use std::ffi::CString;

#[cfg(feature = "fits")]
use fitsio_sys as sys;

/// Down-sample so `max(nx,ny,nz) ≤ MAX_DIM` (mirrors `FitsCubeReader.MaxDim`).
/// A per-axis stride keeps the longest side at this cap, which bounds the total
/// voxel count at `MAX_DIM³` = 256³.
#[cfg(feature = "fits")]
const MAX_DIM: usize = 256;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Load a 3D FITS spectral cube into a normalized [`VolumeData`].
///
/// `hdu` is a **1-based** absolute HDU number (matching
/// [`crate::helpers::fits_loader::load_fits_image_hdu`]); `None` auto-detects
/// the first NAXIS≥3 image HDU.
#[cfg(feature = "fits")]
pub fn load_cube(
    path: &Path,
    hdu: Option<usize>,
) -> Result<crate::models::volume_data::VolumeData, String> {
    unsafe { load_cube_raw(path, hdu.map(|h| h as i32)) }
}

/// Read the raw header keywords of the cube HDU as a `KEY → value` map, for the
/// WCS layer ([`crate::helpers::cube_wcs::CubeWcs::from_header`]). Values are
/// unquoted/trimmed; `COMMENT`/`HISTORY`/`END` cards are omitted.
#[cfg(feature = "fits")]
pub fn cube_header(path: &Path) -> Result<HashMap<String, String>, String> {
    unsafe { cube_header_raw(path) }
}

/// Stub when cfitsio is not compiled in.
#[cfg(not(feature = "fits"))]
pub fn load_cube(
    path: &Path,
    _hdu: Option<usize>,
) -> Result<crate::models::volume_data::VolumeData, String> {
    Err(format!(
        "FITS support not compiled. Install libcfitsio-dev and rebuild with --features fits to load cube '{}'",
        path.display()
    ))
}

/// Stub when cfitsio is not compiled in.
#[cfg(not(feature = "fits"))]
pub fn cube_header(path: &Path) -> Result<std::collections::HashMap<String, String>, String> {
    Err(format!(
        "FITS support not compiled. Install libcfitsio-dev and rebuild with --features fits to read header of '{}'",
        path.display()
    ))
}

// ---------------------------------------------------------------------------
// Raw-FFI implementation (mirrors src/helpers/fits_loader.rs patterns)
// ---------------------------------------------------------------------------

#[cfg(feature = "fits")]
struct FitsHandle {
    fptr: *mut sys::fitsfile,
}

#[cfg(feature = "fits")]
impl Drop for FitsHandle {
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
unsafe fn check_status(status: i32, context: &str) -> Result<(), String> {
    if status == 0 {
        Ok(())
    } else {
        let mut buf = [0i8; 31];
        sys::ffgmsg(buf.as_mut_ptr());
        let msg = std::ffi::CStr::from_ptr(buf.as_ptr())
            .to_string_lossy()
            .into_owned();
        if msg.is_empty() {
            Err(format!("{}: CFITSIO status {}", context, status))
        } else {
            Err(format!("{}: {} (status {})", context, msg, status))
        }
    }
}

#[cfg(feature = "fits")]
unsafe fn open_readonly(path: &Path) -> Result<FitsHandle, String> {
    let path_str = path
        .to_str()
        .ok_or_else(|| format!("FITS path contains invalid UTF-8: {:?}", path))?;
    let c_path = CString::new(path_str)
        .map_err(|e| format!("Cannot encode FITS path as C string: {}", e))?;

    let mut fptr: *mut sys::fitsfile = std::ptr::null_mut();
    let mut status: i32 = 0;
    sys::ffopen(
        &mut fptr,
        c_path.as_ptr(),
        sys::READONLY as i32,
        &mut status,
    );
    check_status(status, "Cannot open FITS file")?;
    Ok(FitsHandle { fptr })
}

/// Return the axis count of the *current* HDU, or `None` on error.
#[cfg(feature = "fits")]
unsafe fn current_naxis(fptr: *mut sys::fitsfile) -> Option<i32> {
    let mut naxis: i32 = 0;
    let mut status = 0;
    sys::ffgidm(fptr, &mut naxis, &mut status);
    if status == 0 {
        Some(naxis)
    } else {
        None
    }
}

/// Walk all HDUs and return the 1-based index of the first one whose image has
/// NAXIS ≥ 3 — a regular image HDU, or a tile-compressed image stored as a
/// BINTABLE (CFITSIO reports the *uncompressed* dims for the latter).
#[cfg(feature = "fits")]
unsafe fn find_cube_hdu(handle: &FitsHandle) -> Result<Option<i32>, String> {
    let mut num_hdus: i32 = 0;
    let mut status = 0;
    sys::ffthdu(handle.fptr, &mut num_hdus, &mut status);
    check_status(status, "Cannot read number of HDUs")?;

    for hdu_idx in 1..=num_hdus {
        let mut hdu_type: i32 = 0;
        status = 0;
        sys::ffmahd(handle.fptr, hdu_idx, &mut hdu_type, &mut status);
        if status != 0 {
            continue;
        }

        if hdu_type == sys::IMAGE_HDU as i32 {
            if let Some(naxis) = current_naxis(handle.fptr) {
                if naxis >= 3 {
                    return Ok(Some(hdu_idx));
                }
            }
            continue;
        }

        if hdu_type == sys::BINARY_TBL as i32 {
            let mut c_status = 0;
            let is_compressed = sys::fits_is_compressed_image(handle.fptr, &mut c_status);
            if c_status == 0 && is_compressed != 0 {
                if let Some(naxis) = current_naxis(handle.fptr) {
                    if naxis >= 3 {
                        return Ok(Some(hdu_idx));
                    }
                }
            }
        }
    }
    Ok(None)
}

#[cfg(feature = "fits")]
#[inline]
fn ceil_div(a: usize, b: usize) -> usize {
    (a + b - 1) / b
}

#[cfg(feature = "fits")]
unsafe fn load_cube_raw(
    path: &Path,
    target_hdu: Option<i32>,
) -> Result<crate::models::volume_data::VolumeData, String> {
    use crate::models::volume_data::{CubeMetadata, VolumeData};

    // ── 1. Open + navigate to the cube HDU ───────────────────────────
    let handle = open_readonly(path)?;

    let chosen_hdu = match target_hdu {
        Some(n) => n,
        None => find_cube_hdu(&handle)?.ok_or_else(|| {
            "No 3D cube HDU found in FITS file (need an image with NAXIS ≥ 3)".to_string()
        })?,
    };

    let mut hdu_type: i32 = 0;
    let mut status: i32 = 0;
    sys::ffmahd(handle.fptr, chosen_hdu, &mut hdu_type, &mut status);
    check_status(status, "Cannot move to cube HDU")?;

    // ── 2. Dimensions ────────────────────────────────────────────────
    let mut naxis: i32 = 0;
    status = 0;
    sys::ffgidm(handle.fptr, &mut naxis, &mut status);
    check_status(status, "Cannot read image dimension count")?;
    if naxis < 3 {
        return Err(format!(
            "HDU {} has {} axes, a cube needs NAXIS ≥ 3",
            chosen_hdu, naxis
        ));
    }

    let mut naxes = vec![0i64; naxis as usize];
    status = 0;
    sys::ffgisz(handle.fptr, naxis, naxes.as_mut_ptr(), &mut status);
    check_status(status, "Cannot read image axis sizes")?;

    // FITS order: NAXIS1 (fast), NAXIS2, NAXIS3 …  Higher axes (e.g. a size-1
    // Stokes NAXIS4) are ignored — we read exactly nx·ny·nz voxels.
    let nx = naxes[0] as usize;
    let ny = naxes[1] as usize;
    let nz = naxes[2] as usize;
    if nx < 1 || ny < 1 || nz < 2 {
        return Err(format!(
            "Not a 3D FITS cube (dims {}×{}×{}); need nz ≥ 2",
            nx, ny, nz
        ));
    }

    // ── 3. Stride so the longest axis fits the cap ───────────────────
    let mut step = 1usize;
    while nx.max(ny).max(nz) / step > MAX_DIM {
        step += 1;
    }
    let onx = ceil_div(nx, step);
    let ony = ceil_div(ny, step);
    let onz = ceil_div(nz, step);

    let mut out: Vec<f32> = vec![0.0f32; onx * ony * onz];

    // ── 4. Read every plane as f64 (auto-decompressed), stride kept ──
    let plane_vox = nx * ny;
    let mut plane = vec![0.0f64; plane_vox];
    let mut fpixel = vec![1i64; naxis as usize]; // 1-based coord of first pixel

    // Exact full-cube statistics (every plane is read anyway).
    let mut gmin = f64::INFINITY;
    let mut gmax = f64::NEG_INFINITY;
    let mut nan_count: u64 = 0;

    for z in 0..nz {
        // Point fpixel at the start of plane z: [1, 1, z+1, 1, …].
        for f in fpixel.iter_mut() {
            *f = 1;
        }
        fpixel[2] = (z as i64) + 1;

        let mut nulval = f64::NAN;
        let mut anynul: i32 = 0;
        status = 0;
        sys::ffgpxv(
            handle.fptr,
            sys::TDOUBLE as i32,
            fpixel.as_mut_ptr(),
            plane_vox as i64,
            &mut nulval as *mut f64 as *mut std::os::raw::c_void,
            plane.as_mut_ptr() as *mut std::os::raw::c_void,
            &mut anynul,
            &mut status,
        );
        check_status(status, "Cannot read cube plane pixels")?;

        // Full-plane stats over every voxel.
        for &v in plane.iter() {
            if v.is_finite() {
                if v < gmin {
                    gmin = v;
                }
                if v > gmax {
                    gmax = v;
                }
            } else {
                nan_count += 1;
            }
        }

        // Keep only strided planes.
        if z % step != 0 {
            continue;
        }
        let oz = z / step;
        let mut oy = 0usize;
        let mut y = 0usize;
        while y < ny {
            let row_off = y * nx;
            let dst_row = (oz * ony + oy) * onx;
            let mut ox = 0usize;
            let mut x = 0usize;
            while x < nx {
                out[dst_row + ox] = plane[row_off + x] as f32;
                x += step;
                ox += 1;
            }
            y += step;
            oy += 1;
        }
    }

    // ── 5. Robust p0.5 / p99.5 cut from the strided finite sample ────
    if gmin > gmax {
        // all-NaN edge case
        gmin = 0.0;
        gmax = 1.0;
    }
    let mut finite: Vec<f32> = out.iter().copied().filter(|v| v.is_finite()).collect();
    let mut norm_lo = gmin as f32;
    let mut norm_hi = gmax as f32;
    let mut median = ((gmin + gmax) * 0.5) as f32;
    if !finite.is_empty() {
        finite.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let n = finite.len();
        norm_lo = finite[((n as f32) * 0.005f32) as usize];
        norm_hi = finite[usize::min(n - 1, ((n as f32) * 0.995f32) as usize)];
        median = finite[n / 2];
    }

    // ── 6. Normalize kept voxels into [0,1] (NaN preserved) ──────────
    let range = if norm_hi > norm_lo {
        norm_hi - norm_lo
    } else {
        1.0f32
    };
    for v in out.iter_mut() {
        *v = if v.is_finite() {
            ((*v - norm_lo) / range).clamp(0.0, 1.0)
        } else {
            f32::NAN
        };
    }

    // ── 7. Metadata ──────────────────────────────────────────────────
    let total_vox = (nx as u64) * (ny as u64) * (nz as u64);
    let nan_fraction = if total_vox > 0 {
        nan_count as f64 / total_vox as f64
    } else {
        0.0
    };
    let meta = CubeMetadata {
        object: read_string_key(handle.fptr, "OBJECT"),
        telescope: read_string_key(handle.fptr, "TELESCOP"),
        instrument: read_string_key(handle.fptr, "INSTRUME"),
        bunit: read_string_key(handle.fptr, "BUNIT"),
        data_min: gmin,
        data_max: gmax,
        median: median as f64,
        norm_lo: norm_lo as f64,
        norm_hi: norm_hi as f64,
        nan_fraction,
        nx,
        ny,
        nz,
        render_nx: onx,
        render_ny: ony,
        render_nz: onz,
    };

    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "cube".to_string());

    Ok(VolumeData {
        nx: onx,
        ny: ony,
        nz: onz,
        data: out,
        name,
        meta: Some(meta),
    })
}

#[cfg(feature = "fits")]
unsafe fn cube_header_raw(path: &Path) -> Result<HashMap<String, String>, String> {
    let handle = open_readonly(path)?;

    // Prefer the cube HDU (where the spectral WCS lives); fall back to the
    // primary HDU if the file has no NAXIS≥3 image.
    let target = find_cube_hdu(&handle)?.unwrap_or(1);
    let mut hdu_type: i32 = 0;
    let mut status: i32 = 0;
    sys::ffmahd(handle.fptr, target, &mut hdu_type, &mut status);
    check_status(status, "Cannot move to cube HDU")?;

    read_header_map(handle.fptr)
}

/// Read a string-valued keyword from the *current* HDU (unquoted + trimmed),
/// or `None` if absent/empty.
#[cfg(feature = "fits")]
unsafe fn read_string_key(fptr: *mut sys::fitsfile, key: &str) -> Option<String> {
    let c_key = CString::new(key).ok()?;
    let mut val_buf = [0i8; (sys::FLEN_VALUE as usize) + 1];
    let mut status = 0;
    sys::ffgkys(
        fptr,
        c_key.as_ptr(),
        val_buf.as_mut_ptr(),
        std::ptr::null_mut(),
        &mut status,
    );
    if status != 0 {
        return None;
    }
    let s = cstr_to_string(val_buf.as_ptr());
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Read all header cards of the *current* HDU into a `KEY → value` map.
#[cfg(feature = "fits")]
unsafe fn read_header_map(fptr: *mut sys::fitsfile) -> Result<HashMap<String, String>, String> {
    let mut nkeys: i32 = 0;
    let mut pos: i32 = 0;
    let mut status = 0;
    sys::ffghps(fptr, &mut nkeys, &mut pos, &mut status);
    check_status(status, "Cannot count header keywords")?;

    let mut header: HashMap<String, String> = HashMap::new();
    let mut key_buf = [0i8; (sys::FLEN_KEYWORD as usize) + 1];
    let mut val_buf = [0i8; (sys::FLEN_VALUE as usize) + 1];
    let mut com_buf = [0i8; (sys::FLEN_COMMENT as usize) + 1];

    for i in 1..=nkeys {
        for b in key_buf.iter_mut() {
            *b = 0;
        }
        for b in val_buf.iter_mut() {
            *b = 0;
        }
        for b in com_buf.iter_mut() {
            *b = 0;
        }

        let mut stat = 0;
        sys::ffgkyn(
            fptr,
            i,
            key_buf.as_mut_ptr(),
            val_buf.as_mut_ptr(),
            com_buf.as_mut_ptr(),
            &mut stat,
        );
        if stat != 0 {
            continue;
        }

        let key = cstr_to_string(key_buf.as_ptr());
        if key.is_empty() || key == "COMMENT" || key == "HISTORY" || key == "END" {
            continue;
        }
        let value = clean_fits_value(&cstr_to_string(val_buf.as_ptr()));
        header.entry(key).or_insert(value);
    }

    Ok(header)
}

#[cfg(feature = "fits")]
unsafe fn cstr_to_string(p: *const i8) -> String {
    std::ffi::CStr::from_ptr(p)
        .to_string_lossy()
        .trim()
        .to_string()
}

/// Strip the surrounding quotes CFITSIO leaves on string keyword values from
/// `ffgkyn` (which returns the raw card value including the quotes).
#[cfg(feature = "fits")]
fn clean_fits_value(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.starts_with('\'') && trimmed.ends_with('\'') && trimmed.len() >= 2 {
        trimmed[1..trimmed.len() - 1].trim().to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(all(test, feature = "fits"))]
mod tests {
    use super::*;

    #[test]
    fn clean_value_strips_quotes() {
        assert_eq!(clean_fits_value("'HI      '"), "HI");
        assert_eq!(clean_fits_value("  'ALMA'  "), "ALMA");
        assert_eq!(clean_fits_value("-32"), "-32");
        assert_eq!(clean_fits_value("''"), "");
    }

    #[test]
    fn ceil_div_rounds_up() {
        assert_eq!(ceil_div(512, 2), 256);
        assert_eq!(ceil_div(513, 2), 257);
        assert_eq!(ceil_div(255, 1), 255);
        assert_eq!(ceil_div(257, 2), 129);
    }

    /// The stride cap keeps the longest axis ≤ MAX_DIM, bounding voxels ≤ 256³.
    #[test]
    fn stride_caps_longest_axis() {
        for &(nx, ny, nz) in &[(1000usize, 1000, 1000), (2048, 512, 300), (256, 256, 256)] {
            let mut step = 1usize;
            while nx.max(ny).max(nz) / step > MAX_DIM {
                step += 1;
            }
            let onx = ceil_div(nx, step);
            let ony = ceil_div(ny, step);
            let onz = ceil_div(nz, step);
            assert!(onx.max(ony).max(onz) <= MAX_DIM, "{}×{}×{}", onx, ony, onz);
        }
    }
}
