use crate::models::FitsImageData;
#[cfg(feature = "fits")]
use std::collections::HashMap;
use std::path::Path;

/// Load a FITS image from disk. Returns the image data from the primary HDU.
#[cfg(feature = "fits")]
pub fn load_fits_image(path: &Path) -> Result<FitsImageData, String> {
    let mut fptr =
        fitsio::FitsFile::open(path).map_err(|e| format!("Cannot open FITS file: {}", e))?;

    let hdu = fptr
        .primary_hdu()
        .map_err(|e| format!("Cannot read primary HDU: {}", e))?;

    let info = hdu
        .info(&mut fptr)
        .map_err(|e| format!("Cannot read HDU info: {}", e))?;

    let (width, height) = match info {
        fitsio::hdu::HduInfo::ImageInfo { shape, .. } => {
            if shape.len() < 2 {
                return Err("FITS image must be at least 2D".to_string());
            }
            (shape[1], shape[0])
        }
        _ => return Err("Primary HDU is not an image".to_string()),
    };

    let pixels: Vec<f64> = hdu
        .read_image(&mut fptr)
        .map_err(|e| format!("Cannot read image data: {}", e))?;

    let header = read_header_keywords(&mut fptr, &hdu);

    Ok(FitsImageData::new(width, height, pixels, header))
}

#[cfg(feature = "fits")]
fn read_header_keywords(
    fptr: &mut fitsio::FitsFile,
    hdu: &fitsio::hdu::FitsHdu,
) -> HashMap<String, String> {
    let mut header = HashMap::new();

    let keys = [
        "CRPIX1", "CRPIX2", "CRVAL1", "CRVAL2", "CD1_1", "CD1_2", "CD2_1", "CD2_2", "CDELT1",
        "CDELT2", "CTYPE1", "CTYPE2", "BITPIX", "OBJECT", "TELESCOP", "INSTRUME", "DATE-OBS",
        "EXPTIME", "FILTER", "BUNIT",
    ];

    for key in &keys {
        if let Ok(val) = hdu.read_key::<String>(fptr, key) {
            header.insert(key.to_string(), val);
        }
    }

    header
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
