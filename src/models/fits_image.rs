use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct WcsInfo {
    pub crpix1: f64,
    pub crpix2: f64,
    pub crval1: f64,
    pub crval2: f64,
    pub cd1_1: f64,
    pub cd1_2: f64,
    pub cd2_1: f64,
    pub cd2_2: f64,
}

impl WcsInfo {
    /// Convert pixel coordinates to sky coordinates (RA, Dec) in degrees
    pub fn pixel_to_sky(&self, x: f64, y: f64) -> (f64, f64) {
        let dx = x - self.crpix1;
        let dy = y - self.crpix2;
        let ra = self.crval1 + self.cd1_1 * dx + self.cd1_2 * dy;
        let dec = self.crval2 + self.cd2_1 * dx + self.cd2_2 * dy;
        (ra, dec)
    }

    /// Format RA/Dec as sexagesimal strings
    pub fn format_coords(ra_deg: f64, dec_deg: f64) -> (String, String) {
        // RA: degrees -> hours
        let ra_h = ra_deg / 15.0;
        let h = ra_h.floor() as i32;
        let m = ((ra_h - h as f64) * 60.0).floor() as i32;
        let s = ((ra_h - h as f64) * 3600.0 - m as f64 * 60.0).abs();
        let ra_str = format!("{:02}h{:02}m{:05.2}s", h, m, s);

        // Dec: degrees
        let sign = if dec_deg < 0.0 { "-" } else { "+" };
        let dec_abs = dec_deg.abs();
        let d = dec_abs.floor() as i32;
        let dm = ((dec_abs - d as f64) * 60.0).floor() as i32;
        let ds = ((dec_abs - d as f64) * 3600.0 - dm as f64 * 60.0).abs();
        let dec_str = format!("{}{:02}d{:02}m{:05.2}s", sign, d, dm, ds);

        (ra_str, dec_str)
    }
}

#[derive(Debug, Clone)]
pub struct FitsImageData {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<f64>,
    pub header: HashMap<String, String>,
    pub wcs: Option<WcsInfo>,
    pub min_val: f64,
    pub max_val: f64,
}

impl FitsImageData {
    pub fn new(
        width: usize,
        height: usize,
        pixels: Vec<f64>,
        header: HashMap<String, String>,
    ) -> Self {
        let min_val = pixels.iter().cloned().fold(f64::INFINITY, f64::min);
        let max_val = pixels.iter().cloned().fold(f64::NEG_INFINITY, f64::max);

        let wcs = Self::parse_wcs(&header);

        FitsImageData {
            width,
            height,
            pixels,
            header,
            wcs,
            min_val,
            max_val,
        }
    }

    fn parse_wcs(header: &HashMap<String, String>) -> Option<WcsInfo> {
        let get_f64 =
            |key: &str| -> Option<f64> { header.get(key).and_then(|v| v.trim().parse().ok()) };

        Some(WcsInfo {
            crpix1: get_f64("CRPIX1")?,
            crpix2: get_f64("CRPIX2")?,
            crval1: get_f64("CRVAL1")?,
            crval2: get_f64("CRVAL2")?,
            cd1_1: get_f64("CD1_1").or_else(|| get_f64("CDELT1"))?,
            cd1_2: get_f64("CD1_2").unwrap_or(0.0),
            cd2_1: get_f64("CD2_1").unwrap_or(0.0),
            cd2_2: get_f64("CD2_2").or_else(|| get_f64("CDELT2"))?,
        })
    }

    /// Get pixel value at (x, y), returns None if out of bounds
    pub fn pixel_at(&self, x: usize, y: usize) -> Option<f64> {
        if x < self.width && y < self.height {
            Some(self.pixels[y * self.width + x])
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wcs_pixel_to_sky() {
        let wcs = WcsInfo {
            crpix1: 100.0,
            crpix2: 100.0,
            crval1: 180.0,
            crval2: 45.0,
            cd1_1: -0.001,
            cd1_2: 0.0,
            cd2_1: 0.0,
            cd2_2: 0.001,
        };
        let (ra, dec) = wcs.pixel_to_sky(100.0, 100.0);
        assert!((ra - 180.0).abs() < 1e-10);
        assert!((dec - 45.0).abs() < 1e-10);
    }

    #[test]
    fn format_coords_basic() {
        let (ra, dec) = WcsInfo::format_coords(180.0, 45.0);
        assert!(ra.starts_with("12h00m"));
        assert!(dec.starts_with("+45d00m"));
    }

    #[test]
    fn fits_pixel_at() {
        let pixels = vec![1.0, 2.0, 3.0, 4.0];
        let img = FitsImageData::new(2, 2, pixels, HashMap::new());
        assert_eq!(img.pixel_at(0, 0), Some(1.0));
        assert_eq!(img.pixel_at(1, 1), Some(4.0));
        assert_eq!(img.pixel_at(2, 0), None);
    }
}
