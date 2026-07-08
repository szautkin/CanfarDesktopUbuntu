//! FITS loader that uses `fitsio-sys` raw FFI directly so we can handle the
//! full range of CADC FITS files:
//!
//! * Regular uncompressed FITS images (primary HDU has an image)
//! * Tile-compressed files (`.fits.fz`) where the primary HDU is empty and
//!   the actual image lives in a `BINTABLE` extension with `ZIMAGE=T` and
//!   `ZCMPTYPE=RICE_1` / `GZIP_1` / `PLIO_1` / `HCOMPRESS_1`.
//! * Multi-extension files (HST, CFHT WIRCam / MegaCam) where each CCD is
//!   its own image extension — we pick the first image we find.
//!
//! CFITSIO transparently decompresses RICE (and GZIP / PLIO / HCOMPRESS)
//! tile-compressed images when `fits_read_pix` is called on a
//! compressed-image HDU, so we do not need to implement the
//! decompression algorithm ourselves — we only have to navigate to the
//! right HDU and ask for pixels.
//!
//! Before this rewrite we used the `fitsio` safe wrapper's `primary_hdu()`
//! + `read_image()` path.  That path fails for tile-compressed files
//! because fitsio classifies `ZIMAGE=T` BINTABLEs as tables and refuses
//! `read_image()` on them.

use crate::models::FitsImageData;
#[cfg(feature = "fits")]
use std::collections::HashMap;
#[cfg(feature = "fits")]
use std::ffi::CString;
use std::path::Path;

#[cfg(feature = "fits")]
use fitsio_sys as sys;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Load a FITS image from disk.
///
/// Walks all HDUs and returns the first one that CFITSIO reports as an
/// image — including tile-compressed image extensions such as those
/// produced by `fpack`.
#[cfg(feature = "fits")]
pub fn load_fits_image(path: &Path) -> Result<FitsImageData, String> {
    // Unwrap a surrounding tar / .tar.gz / .tgz container first (CADC
    // "download all" bundles). `resolved` — and the TempDir it owns for an
    // extracted member — stays alive until this function returns, so the
    // extracted file survives the whole load.
    let resolved = crate::helpers::fits_container::resolve_fits_path(path)?;
    unsafe { load_fits_image_raw(&resolved.path, None) }
}

/// Load a specific HDU (1-based) from a FITS file — used by the extension
/// selector and the cube viewer (spectral-axis extension).
#[cfg(feature = "fits")]
#[allow(dead_code)]
pub fn load_fits_image_hdu(path: &Path, hdu: usize) -> Result<FitsImageData, String> {
    unsafe { load_fits_image_raw(path, Some(hdu as i32)) }
}

/// Enumerate all HDUs in a FITS file (index, name, dimensions, image flag).
#[cfg(feature = "fits")]
#[allow(dead_code)]
pub fn list_hdus(path: &Path) -> Result<Vec<crate::models::fits_image::HduInfo>, String> {
    unsafe { list_hdus_raw(path) }
}

/// Fallback when cfitsio is not available: returns an error.
#[cfg(not(feature = "fits"))]
pub fn load_fits_image(path: &Path) -> Result<FitsImageData, String> {
    Err(format!(
        "FITS support not compiled. Install libcfitsio-dev and rebuild with --features fits to load '{}'",
        path.display()
    ))
}

#[cfg(not(feature = "fits"))]
pub fn load_fits_image_hdu(path: &Path, _hdu: usize) -> Result<FitsImageData, String> {
    load_fits_image(path)
}

#[cfg(not(feature = "fits"))]
pub fn list_hdus(_path: &Path) -> Result<Vec<crate::models::fits_image::HduInfo>, String> {
    Ok(Vec::new())
}

/// Get a human-readable summary of FITS header information.
pub fn fits_summary(data: &FitsImageData) -> String {
    let mut lines = Vec::new();
    lines.push(format!("{}x{} pixels", data.width, data.height));

    if let Some(obj) = data.header.get("OBJECT") {
        lines.push(format!("Object: {}", obj));
    }
    if let Some(tel) = data.header.get("TELESCOP") {
        lines.push(format!("Telescope: {}", tel));
    }
    if let Some(inst) = data.header.get("INSTRUME") {
        lines.push(format!("Instrument: {}", inst));
    }
    if let Some(date) = data.header.get("DATE-OBS") {
        lines.push(format!("Date: {}", date));
    }
    if let Some(exp) = data.header.get("EXPTIME") {
        lines.push(format!("Exposure: {}s", exp));
    }
    if data.wcs.is_some() {
        lines.push("WCS: available".to_string());
    }

    lines.join(" | ")
}

// ---------------------------------------------------------------------------
// Raw-FFI implementation
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
        // CFITSIO error messages are short — pull them off the internal stack
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
unsafe fn load_fits_image_raw(
    path: &Path,
    target_hdu: Option<i32>,
) -> Result<FitsImageData, String> {
    // ── 1. Open the file ─────────────────────────────────────────────
    let path_str = path.to_str().ok_or_else(|| {
        format!("FITS path contains invalid UTF-8: {:?}", path)
    })?;
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
    let handle = FitsHandle { fptr };

    // ── 2. Walk HDUs, find the first image (regular or tile-compressed) ─
    let mut num_hdus: i32 = 0;
    status = 0;
    sys::ffthdu(handle.fptr, &mut num_hdus, &mut status);
    check_status(status, "Cannot read number of HDUs")?;

    let mut image_hdu: Option<i32> = None;
    for hdu_idx in 1..=num_hdus {
        let mut hdu_type: i32 = 0;
        status = 0;
        sys::ffmahd(handle.fptr, hdu_idx, &mut hdu_type, &mut status);
        if status != 0 {
            continue;
        }

        // Case A: regular image HDU
        if hdu_type == sys::IMAGE_HDU as i32 {
            let mut naxis: i32 = 0;
            let mut dim_status = 0;
            sys::ffgidm(handle.fptr, &mut naxis, &mut dim_status);
            if dim_status == 0 && naxis >= 2 {
                image_hdu = Some(hdu_idx);
                break;
            }
            continue;
        }

        // Case B: tile-compressed image stored as a BINTABLE
        if hdu_type == sys::BINARY_TBL as i32 {
            let mut compressed_status = 0;
            let is_compressed =
                sys::fits_is_compressed_image(handle.fptr, &mut compressed_status);
            if compressed_status == 0 && is_compressed != 0 {
                // fits_is_compressed_image returns non-zero for compressed
                // image BINTABLEs.  CFITSIO will transparently treat this
                // HDU as an image for subsequent fits_get_img_* and
                // fits_read_pix calls.
                let mut naxis: i32 = 0;
                let mut dim_status = 0;
                sys::ffgidm(handle.fptr, &mut naxis, &mut dim_status);
                if dim_status == 0 && naxis >= 2 {
                    image_hdu = Some(hdu_idx);
                    break;
                }
            }
        }
    }

    // An explicit target HDU (extension selector / cube viewer) overrides the
    // auto-detected first image.
    let chosen_hdu = match target_hdu {
        Some(n) => n,
        None => match image_hdu {
            Some(n) => n,
            None => {
                return Err(
                    "No image HDU found in FITS file (checked primary + all extensions)"
                        .to_string(),
                );
            }
        },
    };

    // Navigate to the chosen HDU (ffmahd above may already have left us
    // there, but do it again explicitly for clarity).
    let mut hdu_type: i32 = 0;
    status = 0;
    sys::ffmahd(handle.fptr, chosen_hdu, &mut hdu_type, &mut status);
    check_status(status, "Cannot move to image HDU")?;

    // ── 3. Get image dimensions ──────────────────────────────────────
    let mut naxis: i32 = 0;
    status = 0;
    sys::ffgidm(handle.fptr, &mut naxis, &mut status);
    check_status(status, "Cannot read image dimension count")?;
    if naxis < 2 {
        return Err(format!(
            "Image HDU has {} axes, need at least 2",
            naxis
        ));
    }

    let mut naxes_long = vec![0i64; naxis as usize];
    status = 0;
    sys::ffgisz(
        handle.fptr,
        naxis,
        naxes_long.as_mut_ptr(),
        &mut status,
    );
    check_status(status, "Cannot read image axis sizes")?;

    // naxes is in FITS order: NAXIS1 (fast), NAXIS2, NAXIS3 ...
    let width = naxes_long[0] as usize;
    let height = naxes_long[1] as usize;
    if width == 0 || height == 0 {
        return Err(format!(
            "Image HDU has zero-size plane: {}x{}",
            width, height
        ));
    }

    // For >2D cubes, read only the first 2D plane to keep memory bounded
    // and to match the rendering assumption (one slice per tab).
    let plane_pixels = width * height;

    // ── 4. Read decompressed pixels ──────────────────────────────────
    // fits_read_pix auto-decompresses tile-compressed images.  Use TDOUBLE
    // so CFITSIO converts int16 / int32 / float32 data to f64 for us,
    // applying BSCALE / BZERO automatically.  NaN marks any BLANK pixels
    // from integer sources.
    let mut fpixel = vec![1i64; naxis as usize]; // 1-based
    let mut nulval = f64::NAN;
    let mut anynul: i32 = 0;
    let mut pixels = vec![0.0f64; plane_pixels];
    status = 0;
    sys::ffgpxv(
        handle.fptr,
        sys::TDOUBLE as i32,
        fpixel.as_mut_ptr(),
        plane_pixels as i64,
        &mut nulval as *mut f64 as *mut std::os::raw::c_void,
        pixels.as_mut_ptr() as *mut std::os::raw::c_void,
        &mut anynul,
        &mut status,
    );
    check_status(status, "Cannot read image pixels")?;

    // ── 5. Read header keywords ──────────────────────────────────────
    let (header, header_ordered) = read_header_all(handle.fptr)?;

    // Handle dropped automatically by Drop impl, closing the file.
    Ok(FitsImageData::new_with_ordered(
        width,
        height,
        pixels,
        header,
        header_ordered,
    ))
}

#[cfg(feature = "fits")]
#[allow(dead_code)]
unsafe fn list_hdus_raw(
    path: &Path,
) -> Result<Vec<crate::models::fits_image::HduInfo>, String> {
    use crate::models::fits_image::HduInfo;

    let path_str = path
        .to_str()
        .ok_or_else(|| format!("FITS path contains invalid UTF-8: {:?}", path))?;
    let c_path = CString::new(path_str)
        .map_err(|e| format!("Cannot encode FITS path as C string: {}", e))?;
    let mut fptr: *mut sys::fitsfile = std::ptr::null_mut();
    let mut status: i32 = 0;
    sys::ffopen(&mut fptr, c_path.as_ptr(), sys::READONLY as i32, &mut status);
    check_status(status, "Cannot open FITS file")?;
    let handle = FitsHandle { fptr };

    let mut num_hdus: i32 = 0;
    status = 0;
    sys::ffthdu(handle.fptr, &mut num_hdus, &mut status);
    check_status(status, "Cannot read number of HDUs")?;

    let mut out = Vec::new();
    for hdu_idx in 1..=num_hdus {
        let mut hdu_type: i32 = 0;
        status = 0;
        sys::ffmahd(handle.fptr, hdu_idx, &mut hdu_type, &mut status);
        if status != 0 {
            continue;
        }

        // Dimensions (ffgidm/ffgisz return the *uncompressed* dims for
        // tile-compressed image HDUs).
        let mut naxis: i32 = 0;
        let mut dim_status = 0;
        sys::ffgidm(handle.fptr, &mut naxis, &mut dim_status);
        let (mut width, mut height, mut depth) = (0usize, 0usize, 0usize);
        if dim_status == 0 && naxis >= 1 {
            let mut naxes = vec![0i64; naxis as usize];
            let mut sz_status = 0;
            sys::ffgisz(handle.fptr, naxis, naxes.as_mut_ptr(), &mut sz_status);
            if sz_status == 0 {
                width = naxes.first().copied().unwrap_or(0) as usize;
                height = naxes.get(1).copied().unwrap_or(0) as usize;
                depth = naxes.get(2).copied().unwrap_or(0) as usize;
            }
        }

        let mut is_image = false;
        if hdu_type == sys::IMAGE_HDU as i32 && naxis >= 2 {
            is_image = true;
        } else if hdu_type == sys::BINARY_TBL as i32 {
            let mut c_status = 0;
            let compressed = sys::fits_is_compressed_image(handle.fptr, &mut c_status);
            if c_status == 0 && compressed != 0 && naxis >= 2 {
                is_image = true;
            }
        }

        let name = read_string_key(handle.fptr, "EXTNAME")
            .or_else(|| read_string_key(handle.fptr, "HDUNAME"))
            .unwrap_or_else(|| {
                if hdu_idx == 1 {
                    "Primary".to_string()
                } else {
                    format!("HDU {}", hdu_idx)
                }
            });

        out.push(HduInfo {
            index: hdu_idx as usize,
            name,
            width,
            height,
            depth,
            is_image,
        });
    }
    Ok(out)
}

/// Read a string-valued keyword from the *current* HDU, or `None` if absent.
#[cfg(feature = "fits")]
#[allow(dead_code)]
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

#[cfg(feature = "fits")]
unsafe fn read_header_all(
    fptr: *mut sys::fitsfile,
) -> Result<(HashMap<String, String>, Vec<(String, String, String)>), String> {
    let mut nkeys: i32 = 0;
    let mut pos: i32 = 0;
    let mut status = 0;
    sys::ffghps(fptr, &mut nkeys, &mut pos, &mut status);
    check_status(status, "Cannot count header keywords")?;

    let mut header: HashMap<String, String> = HashMap::new();
    let mut ordered: Vec<(String, String, String)> = Vec::with_capacity(nkeys as usize);

    let mut key_buf = [0i8; (sys::FLEN_KEYWORD as usize) + 1];
    let mut val_buf = [0i8; (sys::FLEN_VALUE as usize) + 1];
    let mut com_buf = [0i8; (sys::FLEN_COMMENT as usize) + 1];

    for i in 1..=nkeys {
        // Zero out the buffers to guard against leftover bytes
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
        if key.is_empty() {
            continue;
        }
        let raw_val = cstr_to_string(val_buf.as_ptr());
        let comment = cstr_to_string(com_buf.as_ptr());
        let value = clean_fits_value(&raw_val);

        // Skip pure COMMENT / HISTORY cards from the map, keep them in ordered.
        if key != "COMMENT" && key != "HISTORY" && key != "END" {
            header.entry(key.clone()).or_insert_with(|| value.clone());
        }
        ordered.push((key, value, comment));
    }

    Ok((header, ordered))
}

#[cfg(feature = "fits")]
unsafe fn cstr_to_string(p: *const i8) -> String {
    std::ffi::CStr::from_ptr(p)
        .to_string_lossy()
        .trim()
        .to_string()
}

/// Strip the surrounding quotes and padding that CFITSIO leaves on string
/// keyword values (`ffgkyn` returns the raw value including the quotes).
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
        assert_eq!(clean_fits_value("'CFHT    '"), "CFHT");
        assert_eq!(clean_fits_value("  'MegaCam'  "), "MegaCam");
        assert_eq!(clean_fits_value("-32"), "-32");
        assert_eq!(clean_fits_value("T"), "T");
        assert_eq!(clean_fits_value("''"), "");
    }

    /// Integration test: if a WIRCam `.fz` test file is present, verify the
    /// raw-FFI loader decompresses it and returns a sensible image plane.
    ///
    /// Run manually with:
    ///   cargo test --features fits loads_wircam_fz -- --ignored --nocapture
    #[test]
    #[ignore]
    fn loads_wircam_fz() {
        let candidates = [
            "/home/serhii/.local/share/verbinal/observations/obs-8d931ffcf24cbd31/1639607o.fits.fz",
        ];
        let path = candidates
            .iter()
            .map(std::path::Path::new)
            .find(|p| p.exists());
        let path = match path {
            Some(p) => p,
            None => {
                println!("no test fixture found, skipping");
                return;
            }
        };
        let data = load_fits_image(path).expect("should decompress WIRCam fz");
        println!(
            "OK {}x{} pixels  min={:.3} max={:.3}  header keys={}",
            data.width,
            data.height,
            data.min_val,
            data.max_val,
            data.header_ordered.len()
        );
        let finite = data.pixels.iter().filter(|v| v.is_finite()).count();
        println!("  finite pixels: {} / {}", finite, data.pixels.len());
        assert!(data.width >= 16, "width too small: {}", data.width);
        assert!(data.height >= 16, "height too small: {}", data.height);
        assert_eq!(data.pixels.len(), data.width * data.height);
        assert!(finite > 0, "no finite pixels — decompression likely failed");
        assert!(data.max_val > data.min_val, "min == max, image is flat");
        // Sanity check: WIRCam CCD values are typically 0..65535 ADU
        assert!(
            data.min_val > -10000.0 && data.max_val < 200_000.0,
            "pixel range {}..{} looks wrong",
            data.min_val,
            data.max_val
        );
    }
}
