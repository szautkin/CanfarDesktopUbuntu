use crate::models::FitsImageData;
#[cfg(feature = "fits")]
use std::collections::HashMap;
use std::path::Path;

/// Load a FITS image from disk. Returns the image data from the primary HDU.
///
/// Handles N-dimensional images by treating the **last two axes** (FITS
/// `NAXIS1`, `NAXIS2`) as the display plane.  Higher axes (e.g. `NAXIS3`
/// for wavelength / time cubes) are flattened into the pixel buffer but
/// only the first 2D slice is rendered.
///
/// IMPORTANT: The `fitsio` crate reverses the shape Vec to **C order**
/// before returning it — see `fitsio/src/fitsfile.rs:346`.  So for a file
/// with `NAXIS=3, NAXIS1=500, NAXIS2=500, NAXIS3=1`, `shape` is
/// `[1, 500, 500]`, meaning width = `shape[last] = NAXIS1` and
/// height = `shape[last-1] = NAXIS2`.
#[cfg(feature = "fits")]
pub fn load_fits_image(path: &Path) -> Result<FitsImageData, String> {
    let mut fptr =
        fitsio::FitsFile::open(path).map_err(|e| format!("Cannot open FITS file: {}", e))?;

    let hdu = fptr
        .primary_hdu()
        .map_err(|e| format!("Cannot read primary HDU: {}", e))?;

    let (width, height) = match &hdu.info {
        fitsio::hdu::HduInfo::ImageInfo { shape, .. } => {
            let n = shape.len();
            if n < 2 {
                return Err(format!(
                    "FITS image must be at least 2D (got {} axes)",
                    n
                ));
            }
            // Shape is in C order: [..., NAXIS2, NAXIS1].
            // Width = NAXIS1 (last), height = NAXIS2 (second-to-last).
            (shape[n - 1], shape[n - 2])
        }
        _ => return Err("Primary HDU is not an image".to_string()),
    };

    if width == 0 || height == 0 {
        return Err(format!(
            "FITS image has zero-size display plane: width={}, height={}",
            width, height
        ));
    }

    let pixels: Vec<f64> = hdu
        .read_image(&mut fptr)
        .map_err(|e| format!("Cannot read image data: {}", e))?;

    // Sanity check: make sure we have enough pixels for at least one 2D slice.
    // (Extra pixels from higher dimensions are ignored by the renderer.)
    let plane = width * height;
    if pixels.len() < plane {
        return Err(format!(
            "FITS pixel buffer too small: {} pixels, need at least {} for {}x{}",
            pixels.len(),
            plane,
            width,
            height
        ));
    }

    let (header, header_ordered) = read_header_keywords(&mut fptr, &hdu);

    Ok(FitsImageData::new_with_ordered(
        width,
        height,
        pixels,
        header,
        header_ordered,
    ))
}

#[cfg(feature = "fits")]
fn read_header_keywords(
    fptr: &mut fitsio::FitsFile,
    hdu: &fitsio::hdu::FitsHdu,
) -> (HashMap<String, String>, Vec<(String, String, String)>) {
    let mut header = HashMap::new();
    let mut ordered = Vec::new();

    // Extended whitelist covering the most common FITS vocabulary.
    // Full header iteration is deferred (requires fits_sys FFI).
    let keys = [
        // Core
        "SIMPLE", "BITPIX", "NAXIS", "NAXIS1", "NAXIS2", "NAXIS3", "BSCALE", "BZERO", "BUNIT",
        "BLANK", "OBJECT", "DATE", "DATE-OBS", "MJD-OBS", "EQUINOX", "RADESYS", "EPOCH",
        // WCS (CD matrix)
        "CRPIX1", "CRPIX2", "CRVAL1", "CRVAL2", "CD1_1", "CD1_2", "CD2_1", "CD2_2",
        // WCS (PC matrix + CDELT)
        "PC1_1", "PC1_2", "PC2_1", "PC2_2", "CDELT1", "CDELT2", "CROTA1", "CROTA2",
        "CTYPE1", "CTYPE2", "CUNIT1", "CUNIT2",
        // Instrument/observation metadata
        "TELESCOP", "INSTRUME", "OBSERVER", "OBSERVAT", "DETECTOR", "FILTER", "EXPTIME",
        "EXPOSURE", "AIRMASS", "GAIN", "RDNOISE", "SATURATE", "RA", "DEC", "PROPID",
        "RUN", "ORIGIN", "COMMENT",
    ];

    for key in &keys {
        if let Ok(val) = hdu.read_key::<String>(fptr, key) {
            header.insert(key.to_string(), val.clone());
            ordered.push((key.to_string(), val, String::new()));
        }
    }

    (header, ordered)
}

/// Fallback when cfitsio is not available: returns an error.
#[cfg(not(feature = "fits"))]
pub fn load_fits_image(path: &Path) -> Result<FitsImageData, String> {
    Err(format!(
        "FITS support not compiled. Install libcfitsio-dev and rebuild with --features fits to load '{}'",
        path.display()
    ))
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
