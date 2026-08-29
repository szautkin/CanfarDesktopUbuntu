use std::collections::HashMap;

/// Zenithal projection family resolved from CTYPE, with a linear fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Projection {
    Tan,
    Sin,
    Stg,
    Zea,
    Linear,
}

/// World Coordinate System parameters extracted from a FITS header.
///
/// Ported from `Models/Fits/WcsInfo.cs`: supports the CD matrix and CDELT+CROTA2
/// conventions, the four common zenithal projections (TAN/SIN/STG/ZEA) with a
/// linear fallback, SIP distortion (Shupe et al. 2005), and an approximate
/// reconstruction from legacy RA/DEC keywords.
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
    pub ctype1: String,
    pub ctype2: String,
    /// SIP forward/inverse coefficient tables indexed `[p][q]` = coeff of uᵖvᵍ.
    pub sip_a: Option<Vec<Vec<f64>>>,
    pub sip_b: Option<Vec<Vec<f64>>>,
    pub sip_ap: Option<Vec<Vec<f64>>>,
    pub sip_bp: Option<Vec<Vec<f64>>>,
    /// True when reconstructed from legacy RA/DEC keywords — spatial ops are approximate.
    pub is_approximate: bool,
}

impl Default for WcsInfo {
    fn default() -> Self {
        WcsInfo {
            crpix1: 0.0,
            crpix2: 0.0,
            crval1: 0.0,
            crval2: 0.0,
            cd1_1: 0.0,
            cd1_2: 0.0,
            cd2_1: 0.0,
            cd2_2: 0.0,
            ctype1: String::new(),
            ctype2: String::new(),
            sip_a: None,
            sip_b: None,
            sip_ap: None,
            sip_bp: None,
            is_approximate: false,
        }
    }
}

impl WcsInfo {
    /// A usable solution requires a non-degenerate CD matrix.
    pub fn is_valid(&self) -> bool {
        self.cd1_1 != 0.0 || self.cd2_2 != 0.0
    }

    /// The algorithm code from a CTYPE string. Per FITS WCS Paper II, CTYPE is
    /// `<coord>-<ALGO>` so the algorithm is the token AFTER the coordinate name
    /// (`RA---TAN` → `TAN`); a further token is a distortion suffix, not the
    /// projection (`RA---TAN-SIP` → `TAN`). Taking the last token misreads every
    /// SIP image as linear.
    fn projection_code(ctype: &str) -> &str {
        ctype
            .split('-')
            .filter(|s| !s.is_empty())
            .nth(1)
            .unwrap_or("")
    }

    /// Resolved projection; both axes must agree or we fall back to Linear.
    pub fn proj(&self) -> Projection {
        let p1 = Self::projection_code(&self.ctype1);
        let p2 = Self::projection_code(&self.ctype2);
        if !p1.eq_ignore_ascii_case(p2) {
            return Projection::Linear;
        }
        match p1.to_ascii_uppercase().as_str() {
            "TAN" => Projection::Tan,
            "SIN" => Projection::Sin,
            "STG" => Projection::Stg,
            "ZEA" => Projection::Zea,
            _ => Projection::Linear,
        }
    }

    /// Rotation from celestial North (East of North), in degrees. Rotate the image
    /// by `-north_angle()` to display North-up.
    pub fn north_angle(&self) -> f64 {
        (-self.cd1_2).atan2(self.cd2_2).to_degrees()
    }

    /// True if the image has a parity flip (East appears right instead of left).
    pub fn has_parity_flip(&self) -> bool {
        (self.cd1_1 * self.cd2_2 - self.cd1_2 * self.cd2_1) > 0.0
    }

    /// Pixel scale in arcseconds per pixel (geometric mean of axis scales).
    pub fn pixel_scale_arcsec(&self) -> f64 {
        let sx = (self.cd1_1 * self.cd1_1 + self.cd2_1 * self.cd2_1).sqrt();
        let sy = (self.cd1_2 * self.cd1_2 + self.cd2_2 * self.cd2_2).sqrt();
        (sx * sy).sqrt() * 3600.0
    }

    /// Evaluate a SIP polynomial Σ c[p][q]·uᵖ·vᵍ.
    fn sip_poly(c: &[Vec<f64>], u: f64, v: f64) -> f64 {
        let order = c.len().saturating_sub(1);
        let mut sum = 0.0;
        // `p` / `q` are the u / v exponents as well as the coefficient indices.
        for (p, row) in c.iter().enumerate() {
            for q in 0..=(order - p) {
                let coeff = row.get(q).copied().unwrap_or(0.0);
                if coeff != 0.0 {
                    sum += coeff * u.powi(p as i32) * v.powi(q as i32);
                }
            }
        }
        sum
    }

    /// Convert pixel `(x, y)` to world `(RA, Dec)` in degrees using a rigorous
    /// spherical deprojection for TAN/SIN/STG/ZEA (with SIP), linear otherwise.
    pub fn pixel_to_sky(&self, x: f64, y: f64) -> (f64, f64) {
        let mut dx = x - self.crpix1;
        let mut dy = y - self.crpix2;
        if let (Some(a), Some(b)) = (&self.sip_a, &self.sip_b) {
            let fx = dx + Self::sip_poly(a, dx, dy);
            let fy = dy + Self::sip_poly(b, dx, dy);
            dx = fx;
            dy = fy;
        }
        let xi = self.cd1_1 * dx + self.cd1_2 * dy;
        let eta = self.cd2_1 * dx + self.cd2_2 * dy;
        match deproject(xi, eta, self.crval1, self.crval2, self.proj()) {
            Some(world) => world,
            None => (self.crval1 + xi, self.crval2 + eta),
        }
    }

    /// Convert world `(RA, Dec)` in degrees to pixel `(x, y)`. Returns `None` if
    /// the CD matrix is singular or the coordinate is outside the projection domain.
    pub fn world_to_pixel(&self, ra: f64, dec: f64) -> Option<(f64, f64)> {
        let det = self.cd1_1 * self.cd2_2 - self.cd1_2 * self.cd2_1;
        if det.abs() < 1e-30 {
            return None;
        }
        let proj = self.proj();
        let (xi, eta) = match project(ra, dec, self.crval1, self.crval2, proj) {
            Some((xi, eta)) => (xi, eta),
            None if proj == Projection::Linear => (ra - self.crval1, dec - self.crval2),
            None => return None,
        };
        let mut dx = (self.cd2_2 * xi - self.cd1_2 * eta) / det;
        let mut dy = (-self.cd2_1 * xi + self.cd1_1 * eta) / det;
        if let (Some(ap), Some(bp)) = (&self.sip_ap, &self.sip_bp) {
            let u = dx + Self::sip_poly(ap, dx, dy);
            let v = dy + Self::sip_poly(bp, dx, dy);
            dx = u;
            dy = v;
        }
        Some((self.crpix1 + dx, self.crpix2 + dy))
    }

    // ── Pixel conventions ───────────────────────────────────────────────────
    //
    // `pixel_to_sky` / `world_to_pixel` speak the FITS convention, because that
    // is what CRPIX is stated in: pixels are 1-based and measured at their
    // CENTRES, so the first pixel's centre is 1.0. Feeding either of the other
    // two conventions straight in is a silent one-pixel error — right at the
    // reference pixel, wrong everywhere else by a constant, and invisible
    // unless you compare against astropy. It measured 0.045 arcsec on a JWST
    // frame at 0.03 arcsec/px; on a wide-field image the same one pixel is
    // arcminutes.
    //
    // So the two other conventions get their own names. Callers say which kind
    // of number they hold instead of every call site rediscovering the offset,
    // and the sums live here once.

    /// Sky position of a 0-based ARRAY index — astropy's convention, and what
    /// an agent means by "pixel (x, y)" because it is what the readouts show.
    pub fn array_to_sky(&self, x: f64, y: f64) -> (f64, f64) {
        self.pixel_to_sky(x + 1.0, y + 1.0)
    }

    /// The 0-based array index under a sky position. Inverse of
    /// [`Self::array_to_sky`].
    pub fn sky_to_array(&self, ra: f64, dec: f64) -> Option<(f64, f64)> {
        self.world_to_pixel(ra, dec)
            .map(|(x, y)| (x - 1.0, y - 1.0))
    }

    /// Sky position of a corner-origin DISPLAY coordinate: 0.0 is the left edge
    /// of the first pixel, so its centre is 0.5 and the image spans `0..width`.
    ///
    /// This is what the canvas produces — `screen_to_image` divides by the zoom
    /// and `on_image` bounds the result to `0 <= p < width` — so it is what the
    /// crosshair, the hover readout and a dragged mark all hold.
    pub fn display_to_sky(&self, x: f64, y: f64) -> (f64, f64) {
        self.pixel_to_sky(x + 0.5, y + 0.5)
    }

    /// The corner-origin display coordinate of a sky position. Inverse of
    /// [`Self::display_to_sky`].
    ///
    /// Paired with it deliberately: crosshair and mark sync go display -> sky on
    /// one tab and sky -> display on another, so the two offsets cancel and the
    /// convention cannot pull linked tabs apart. Changing one without the other
    /// would.
    pub fn sky_to_display(&self, ra: f64, dec: f64) -> Option<(f64, f64)> {
        self.world_to_pixel(ra, dec)
            .map(|(x, y)| (x - 0.5, y - 0.5))
    }

    /// A short human label for the WCS solution kind (for the Image Info panel).
    pub fn solution_kind(&self) -> &'static str {
        if self.is_approximate {
            "approximate"
        } else if !self.is_valid() {
            "none"
        } else {
            match self.proj() {
                Projection::Linear => "linear",
                _ => {
                    if self.sip_a.is_some() {
                        "TAN+SIP"
                    } else {
                        // Report the actual projection family.
                        match self.proj() {
                            Projection::Tan => "TAN",
                            Projection::Sin => "SIN",
                            Projection::Stg => "STG",
                            Projection::Zea => "ZEA",
                            Projection::Linear => "linear",
                        }
                    }
                }
            }
        }
    }

    /// Format as a CADC resolver-compatible coordinate string
    /// `"HH:MM:SS.ss,+DD:MM:SS.s"` (no spaces).
    pub fn format_for_resolver(ra_deg: f64, dec_deg: f64) -> String {
        let mut ra = ra_deg / 15.0;
        if ra < 0.0 {
            ra += 24.0;
        }
        let rh = ra as i32;
        let rm = ((ra - rh as f64) * 60.0) as i32;
        let rs = (ra - rh as f64 - rm as f64 / 60.0) * 3600.0;
        let sign = if dec_deg >= 0.0 { "+" } else { "-" };
        let dec = dec_deg.abs();
        let dd = dec as i32;
        let dm = ((dec - dd as f64) * 60.0) as i32;
        let ds = (dec - dd as f64 - dm as f64 / 60.0) * 3600.0;
        let rs_int = (rs * 100.0).round() as i32;
        let ds_int = (ds * 10.0).round() as i32;
        format!(
            "{:02}:{:02}:{:04},{}{:02}:{:02}:{:03}",
            rh, rm, rs_int, sign, dd, dm, ds_int
        )
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

/// Forward project `(RA, Dec)` → intermediate world `(ξ, η)` in degrees.
/// Returns `None` for projection-domain violations or a Linear request.
/// (Calabretta & Greisen 2002, A&A 395, 1077.)
pub fn project(
    ra: f64,
    dec: f64,
    crval1: f64,
    crval2: f64,
    projection: Projection,
) -> Option<(f64, f64)> {
    if projection == Projection::Linear {
        return None;
    }
    let ra_rad = ra.to_radians();
    let dec_rad = dec.to_radians();
    let ra0 = crval1.to_radians();
    let dec0 = crval2.to_radians();
    let rad_to_deg = 180.0 / std::f64::consts::PI;

    let cos_psi = dec_rad.sin() * dec0.sin() + dec_rad.cos() * dec0.cos() * (ra_rad - ra0).cos();
    let x_num = dec_rad.cos() * (ra_rad - ra0).sin();
    let y_num = dec_rad.sin() * dec0.cos() - dec_rad.cos() * dec0.sin() * (ra_rad - ra0).cos();

    match projection {
        Projection::Tan => {
            if cos_psi <= 1e-12 {
                return None;
            }
            Some((x_num / cos_psi * rad_to_deg, y_num / cos_psi * rad_to_deg))
        }
        Projection::Sin => Some((x_num * rad_to_deg, y_num * rad_to_deg)),
        Projection::Stg => {
            let denom = 1.0 + cos_psi;
            if denom <= 1e-12 {
                return None;
            }
            Some((
                2.0 * x_num / denom * rad_to_deg,
                2.0 * y_num / denom * rad_to_deg,
            ))
        }
        Projection::Zea => {
            if cos_psi <= -1.0 + 1e-12 {
                return None;
            }
            let factor = (2.0 / (1.0 + cos_psi)).sqrt();
            Some((x_num * factor * rad_to_deg, y_num * factor * rad_to_deg))
        }
        Projection::Linear => None,
    }
}

/// Inverse project intermediate world `(ξ, η)` in degrees → `(RA, Dec)`.
/// Returns `None` when outside the projection domain. RA normalised to `[0, 360)`.
pub fn deproject(
    xi: f64,
    eta: f64,
    crval1: f64,
    crval2: f64,
    projection: Projection,
) -> Option<(f64, f64)> {
    if projection == Projection::Linear {
        return None;
    }
    let xi_rad = xi.to_radians();
    let eta_rad = eta.to_radians();
    let rho = (xi_rad * xi_rad + eta_rad * eta_rad).sqrt();
    let ra0 = crval1.to_radians();
    let dec0 = crval2.to_radians();

    if rho < 1e-12 {
        return Some((crval1, crval2));
    }

    let (cos_psi, sin_psi) = match projection {
        Projection::Tan => {
            let denom = (1.0 + rho * rho).sqrt();
            (1.0 / denom, rho / denom)
        }
        Projection::Sin => {
            if rho > 1.0 {
                return None;
            }
            (((1.0 - rho * rho).max(0.0)).sqrt(), rho)
        }
        Projection::Stg => {
            let half_psi = (rho / 2.0).atan();
            ((2.0 * half_psi).cos(), (2.0 * half_psi).sin())
        }
        Projection::Zea => {
            if rho > 2.0 {
                return None;
            }
            let half_psi = (rho / 2.0).asin();
            ((2.0 * half_psi).cos(), (2.0 * half_psi).sin())
        }
        Projection::Linear => return None,
    };

    let sin_b = xi_rad / rho;
    let cos_b = eta_rad / rho;

    let sin_dec = cos_psi * dec0.sin() + sin_psi * cos_b * dec0.cos();
    let dec_rad = sin_dec.clamp(-1.0, 1.0).asin();

    let y_arg = sin_psi * sin_b;
    let x_arg = cos_psi * dec0.cos() - sin_psi * cos_b * dec0.sin();
    let ra_rad = ra0 + y_arg.atan2(x_arg);

    let mut ra_deg = ra_rad.to_degrees() % 360.0;
    if ra_deg < 0.0 {
        ra_deg += 360.0;
    }
    Some((ra_deg, dec_rad.to_degrees()))
}

/// Parse a sexagesimal RA string in hours (`"HH:MM:SS.s"` / `"HH MM SS"`) → degrees.
fn parse_ra_sexagesimal(s: &str) -> Option<f64> {
    let parts: Vec<f64> = s
        .trim()
        .split([':', ' '])
        .filter(|t| !t.is_empty())
        .filter_map(|t| t.parse().ok())
        .collect();
    if parts.is_empty() {
        return None;
    }
    let hours = parts[0]
        + parts.get(1).copied().unwrap_or(0.0) / 60.0
        + parts.get(2).copied().unwrap_or(0.0) / 3600.0;
    Some(hours * 15.0)
}

/// Parse a sexagesimal Dec string in degrees (`"+DD:MM:SS.s"`) → degrees.
fn parse_dec_sexagesimal(s: &str) -> Option<f64> {
    let trimmed = s.trim();
    let neg = trimmed.starts_with('-');
    let parts: Vec<f64> = trimmed
        .trim_start_matches(['+', '-'])
        .split([':', ' '])
        .filter(|t| !t.is_empty())
        .filter_map(|t| t.parse().ok())
        .collect();
    if parts.is_empty() {
        return None;
    }
    let deg = parts[0]
        + parts.get(1).copied().unwrap_or(0.0) / 60.0
        + parts.get(2).copied().unwrap_or(0.0) / 3600.0;
    Some(if neg { -deg } else { deg })
}

/// Read a SIP coefficient set (`<prefix>_ORDER` + `<prefix>_p_q`) into a `[p][q]`
/// table, or `None` when absent.
fn read_sip_coefficients(header: &HashMap<String, String>, prefix: &str) -> Option<Vec<Vec<f64>>> {
    let get_f64 =
        |key: &str| -> Option<f64> { header.get(key).and_then(|v| v.trim().parse().ok()) };
    let order = get_f64(&format!("{}_ORDER", prefix))? as usize;
    if !(1..=9).contains(&order) {
        return None;
    }
    let mut c = vec![vec![0.0; order + 1]; order + 1];
    let mut any = false;
    // `p` / `q` are the u / v exponents as well as the coefficient indices; the
    // FITS keyword for each term is literally `<prefix>_<p>_<q>`.
    for (p, row) in c.iter_mut().enumerate() {
        for (q, cell) in row.iter_mut().enumerate().take(order - p + 1) {
            if let Some(v) = get_f64(&format!("{}_{}_{}", prefix, p, q)) {
                *cell = v;
                any = true;
            }
        }
    }
    if any {
        Some(c)
    } else {
        None
    }
}

/// Metadata for one HDU (extension) in a FITS file, for the extension selector.
#[derive(Debug, Clone)]
pub struct HduInfo {
    /// 1-based CFITSIO HDU number.
    pub index: usize,
    /// EXTNAME, or "Primary" / "HDU N".
    pub name: String,
    pub width: usize,
    pub height: usize,
    /// NAXIS3 (0 when the HDU is 2D).
    pub depth: usize,
    /// True when this HDU is a loadable ≥2D image (regular or tile-compressed).
    pub is_image: bool,
}

impl HduInfo {
    /// A short one-line label for the selector, e.g. "1: SCI 2048×4096".
    pub fn label(&self) -> String {
        let dims = if self.depth > 1 {
            format!("{}×{}×{}", self.width, self.height, self.depth)
        } else {
            format!("{}×{}", self.width, self.height)
        };
        if self.is_image {
            format!("{}: {} {}", self.index, self.name, dims)
        } else {
            format!("{}: {} (non-image)", self.index, self.name)
        }
    }
}

/// Human description of a FITS BITPIX value.
fn describe_bitpix(bitpix: &str) -> String {
    match bitpix.parse::<i32>() {
        Ok(8) => "8-bit unsigned integer".into(),
        Ok(16) => "16-bit integer".into(),
        Ok(32) => "32-bit integer".into(),
        Ok(64) => "64-bit integer".into(),
        Ok(-32) => "32-bit float".into(),
        Ok(-64) => "64-bit float".into(),
        _ => bitpix.to_string(),
    }
}

/// Format an angular field of view given in arcseconds, auto-selecting °/′/″.
fn format_fov(arcsec: f64) -> String {
    let a = arcsec.abs();
    if a >= 3600.0 {
        format!("{:.2}°", a / 3600.0)
    } else if a >= 60.0 {
        format!("{:.1}′", a / 60.0)
    } else {
        format!("{:.1}″", a)
    }
}

#[derive(Debug, Clone)]
pub struct FitsImageData {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<f64>,
    pub header: HashMap<String, String>,
    /// Ordered list of (keyword, value, comment) tuples for the Header Panel.
    pub header_ordered: Vec<(String, String, String)>,
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
        // Build ordered list from the HashMap (alphabetical for now; loader can populate directly)
        let mut header_ordered: Vec<(String, String, String)> = header
            .iter()
            .map(|(k, v)| (k.clone(), v.clone(), String::new()))
            .collect();
        header_ordered.sort_by(|a, b| a.0.cmp(&b.0));

        Self::new_with_ordered(width, height, pixels, header, header_ordered)
    }

    pub fn new_with_ordered(
        width: usize,
        height: usize,
        pixels: Vec<f64>,
        header: HashMap<String, String>,
        header_ordered: Vec<(String, String, String)>,
    ) -> Self {
        let min_val = pixels
            .iter()
            .cloned()
            .filter(|v| v.is_finite())
            .fold(f64::INFINITY, f64::min);
        let max_val = pixels
            .iter()
            .cloned()
            .filter(|v| v.is_finite())
            .fold(f64::NEG_INFINITY, f64::max);

        // Guard against all-NaN or empty pixel arrays
        let (min_val, max_val) = if min_val.is_finite() && max_val.is_finite() {
            (min_val, max_val)
        } else {
            (0.0, 1.0)
        };

        let wcs = Self::parse_wcs(&header);

        FitsImageData {
            width,
            height,
            pixels,
            header,
            header_ordered,
            wcs,
            min_val,
            max_val,
        }
    }

    fn parse_wcs(header: &HashMap<String, String>) -> Option<WcsInfo> {
        let get_f64 =
            |key: &str| -> Option<f64> { header.get(key).and_then(|v| v.trim().parse().ok()) };
        let get_str = |key: &str| -> String {
            header
                .get(key)
                .map(|v| v.trim().trim_matches('\'').trim().to_string())
                .unwrap_or_default()
        };
        let contains = |key: &str| header.contains_key(key);

        let ctype1 = get_str("CTYPE1");
        let ctype2 = get_str("CTYPE2");

        // Build the CD matrix from CD*_* if present, else CDELT + CROTA2.
        let (cd1_1, cd1_2, cd2_1, cd2_2) = if contains("CD1_1") {
            (
                get_f64("CD1_1").unwrap_or(0.0),
                get_f64("CD1_2").unwrap_or(0.0),
                get_f64("CD2_1").unwrap_or(0.0),
                get_f64("CD2_2").unwrap_or(0.0),
            )
        } else if contains("PC1_1") || contains("PC1_2") || contains("PC2_1") || contains("PC2_2") {
            // PC + CDELT, the modern FITS convention: CDi_j = CDELTi * PCi_j.
            //
            // Missing entirely before this, so a file with PC and no CD fell
            // through to the CDELT+CROTA2 branch, found no CROTA2, and got a
            // rotation of zero — the scale right and the orientation lost. It
            // is not exotic: JWST i2d products use it, and one measured 40 to
            // 90 arcseconds out, growing with distance from the reference
            // pixel, which is the signature of a dropped rotation. Nothing
            // reported a problem because a rotation of zero is a valid WCS.
            //
            // Defaults are the standard's: 1 on the diagonal, 0 off it.
            let cdelt1 = get_f64("CDELT1").unwrap_or(1.0);
            let cdelt2 = get_f64("CDELT2").unwrap_or(1.0);
            let pc1_1 = get_f64("PC1_1").unwrap_or(1.0);
            let pc1_2 = get_f64("PC1_2").unwrap_or(0.0);
            let pc2_1 = get_f64("PC2_1").unwrap_or(0.0);
            let pc2_2 = get_f64("PC2_2").unwrap_or(1.0);
            (
                cdelt1 * pc1_1,
                cdelt1 * pc1_2,
                cdelt2 * pc2_1,
                cdelt2 * pc2_2,
            )
        } else if contains("CDELT1") || contains("CDELT2") {
            let cdelt1 = get_f64("CDELT1").unwrap_or(0.0);
            let cdelt2 = get_f64("CDELT2").unwrap_or(0.0);
            let crota2 = get_f64("CROTA2").unwrap_or(0.0).to_radians();
            (
                cdelt1 * crota2.cos(),
                -cdelt2 * crota2.sin(),
                cdelt1 * crota2.sin(),
                cdelt2 * crota2.cos(),
            )
        } else {
            (0.0, 0.0, 0.0, 0.0)
        };

        let mut wcs = WcsInfo {
            crpix1: get_f64("CRPIX1").unwrap_or(0.0),
            crpix2: get_f64("CRPIX2").unwrap_or(0.0),
            crval1: get_f64("CRVAL1").unwrap_or(0.0),
            crval2: get_f64("CRVAL2").unwrap_or(0.0),
            cd1_1,
            cd1_2,
            cd2_1,
            cd2_2,
            ctype1: ctype1.clone(),
            ctype2,
            ..WcsInfo::default()
        };

        // SIP distortion (CTYPE "…-SIP").
        if ctype1.to_ascii_uppercase().contains("SIP") {
            wcs.sip_a = read_sip_coefficients(header, "A");
            wcs.sip_b = read_sip_coefficients(header, "B");
            wcs.sip_ap = read_sip_coefficients(header, "AP");
            wcs.sip_bp = read_sip_coefficients(header, "BP");
        }

        // Degenerate/half-zero CD: try a legacy reconstruction from RA/DEC keywords.
        if wcs.cd1_1 == 0.0 || wcs.cd2_2 == 0.0 {
            if let Some(legacy) = Self::parse_legacy_wcs(header) {
                return Some(legacy);
            }
        }

        // No usable solution at all → no WCS.
        if !wcs.is_valid() {
            return None;
        }
        Some(wcs)
    }

    /// Approximate WCS from legacy RA/DEC keywords + a plate scale, when standard
    /// WCS keywords are absent. Assumes RA/DEC is the image centre.
    fn parse_legacy_wcs(header: &HashMap<String, String>) -> Option<WcsInfo> {
        let get_f64 =
            |key: &str| -> Option<f64> { header.get(key).and_then(|v| v.trim().parse().ok()) };
        let ra = header
            .get("RA")
            .and_then(|s| parse_ra_sexagesimal(s))
            .or_else(|| get_f64("RA"))?;
        let dec = header
            .get("DEC")
            .and_then(|s| parse_dec_sexagesimal(s))
            .or_else(|| get_f64("DEC"))?;

        let naxis1 = get_f64("NAXIS1").unwrap_or(0.0);
        let naxis2 = get_f64("NAXIS2").unwrap_or(0.0);
        if naxis1 <= 0.0 || naxis2 <= 0.0 {
            return None;
        }

        let mut scale = get_f64("SECPIX").unwrap_or(0.0);
        if scale == 0.0 {
            scale = get_f64("PIXSCALE").unwrap_or(0.0);
        }
        if scale == 0.0 {
            scale = get_f64("SCALE").unwrap_or(0.0);
        }
        if scale == 0.0 {
            scale = 0.5; // conservative default (arcsec/px)
        }
        let cdelt = scale / 3600.0;

        Some(WcsInfo {
            crpix1: naxis1 / 2.0,
            crpix2: naxis2 / 2.0,
            crval1: ra,
            crval2: dec,
            cd1_1: -cdelt,
            cd1_2: 0.0,
            cd2_1: 0.0,
            cd2_2: cdelt,
            ctype1: "RA---TAN".to_string(),
            ctype2: "DEC--TAN".to_string(),
            is_approximate: true,
            ..WcsInfo::default()
        })
    }

    /// An at-a-glance summary of the image for the Image Info panel: label/value
    /// rows covering dimensions, data type, WCS solution, pixel scale, field of
    /// view, sky centre, orientation, and instrument metadata.
    pub fn image_info_rows(&self) -> Vec<(String, String)> {
        let mut rows: Vec<(String, String)> = Vec::new();
        rows.push((
            "Dimensions".into(),
            format!("{} × {} px", self.width, self.height),
        ));
        if let Some(bitpix) = self.header.get("BITPIX") {
            rows.push(("Data type".into(), describe_bitpix(bitpix.trim())));
        }
        match &self.wcs {
            Some(w) if w.is_valid() => {
                rows.push(("WCS".into(), w.solution_kind().to_string()));
                let scale = w.pixel_scale_arcsec();
                rows.push(("Pixel scale".into(), format!("{:.3}″/px", scale)));
                rows.push((
                    "Field of view".into(),
                    format!(
                        "{} × {}",
                        format_fov(scale * self.width as f64),
                        format_fov(scale * self.height as f64)
                    ),
                ));
                let (ra, dec) = w.pixel_to_sky(self.width as f64 / 2.0, self.height as f64 / 2.0);
                let (ra_s, dec_s) = WcsInfo::format_coords(ra, dec);
                rows.push(("Sky centre".into(), format!("{}  {}", ra_s, dec_s)));
                let parity = if w.has_parity_flip() {
                    "flipped"
                } else {
                    "normal"
                };
                rows.push((
                    "Orientation".into(),
                    format!("N {:+.1}° · {}", w.north_angle(), parity),
                ));
            }
            _ => rows.push(("WCS".into(), "none".into())),
        }
        for (key, label) in [
            ("OBJECT", "Object"),
            ("TELESCOP", "Telescope"),
            ("INSTRUME", "Instrument"),
            ("FILTER", "Filter"),
            ("DATE-OBS", "Date"),
            ("EXPTIME", "Exposure"),
        ] {
            if let Some(v) = self.header.get(key) {
                let v = v.trim().trim_matches('\'').trim();
                if !v.is_empty() {
                    rows.push((label.to_string(), v.to_string()));
                }
            }
        }
        rows
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

    fn tan_wcs() -> WcsInfo {
        WcsInfo {
            crpix1: 100.0,
            crpix2: 100.0,
            crval1: 180.0,
            crval2: 45.0,
            cd1_1: -0.001,
            cd1_2: 0.0,
            cd2_1: 0.0,
            cd2_2: 0.001,
            ctype1: "RA---TAN".to_string(),
            ctype2: "DEC--TAN".to_string(),
            ..WcsInfo::default()
        }
    }

    #[test]
    fn wcs_reference_pixel_maps_to_crval() {
        let (ra, dec) = tan_wcs().pixel_to_sky(100.0, 100.0);
        assert!((ra - 180.0).abs() < 1e-9);
        assert!((dec - 45.0).abs() < 1e-9);
    }

    #[test]
    fn wcs_roundtrips_tan() {
        let w = tan_wcs();
        let (ra, dec) = w.pixel_to_sky(150.0, 80.0);
        let (px, py) = w.world_to_pixel(ra, dec).unwrap();
        assert!((px - 150.0).abs() < 1e-6, "px={}", px);
        assert!((py - 80.0).abs() < 1e-6, "py={}", py);
    }

    #[test]
    fn projection_code_ignores_sip_suffix() {
        let mut w = tan_wcs();
        w.ctype1 = "RA---TAN-SIP".to_string();
        w.ctype2 = "DEC--TAN-SIP".to_string();
        assert_eq!(w.proj(), Projection::Tan);
    }

    #[test]
    fn sip_distortion_shifts_result() {
        let mut w = tan_wcs();
        // A_ORDER 2 with a nonzero A_1_1 term perturbs off-reference pixels.
        w.sip_a = Some(vec![
            vec![0.0, 0.0, 0.0],
            vec![0.0, 0.01, 0.0],
            vec![0.0, 0.0, 0.0],
        ]);
        w.sip_b = Some(vec![vec![0.0; 3]; 3]);
        let plain = tan_wcs().pixel_to_sky(150.0, 130.0);
        let distorted = w.pixel_to_sky(150.0, 130.0);
        assert!((plain.0 - distorted.0).abs() > 1e-6 || (plain.1 - distorted.1).abs() > 1e-6);
    }

    #[test]
    fn north_angle_and_parity() {
        let w = tan_wcs();
        assert!(w.north_angle().abs() < 1e-9); // no rotation
        assert!(!w.has_parity_flip()); // det < 0
        assert!((w.pixel_scale_arcsec() - 3.6).abs() < 1e-6); // 0.001 deg = 3.6"
    }

    #[test]
    fn parse_wcs_cdelt_crota() {
        let mut h = HashMap::new();
        h.insert("CRPIX1".into(), "50".into());
        h.insert("CRPIX2".into(), "50".into());
        h.insert("CRVAL1".into(), "10".into());
        h.insert("CRVAL2".into(), "20".into());
        h.insert("CDELT1".into(), "-0.001".into());
        h.insert("CDELT2".into(), "0.001".into());
        h.insert("CROTA2".into(), "0".into());
        h.insert("CTYPE1".into(), "RA---TAN".into());
        h.insert("CTYPE2".into(), "DEC--TAN".into());
        let w = FitsImageData::parse_wcs(&h).unwrap();
        assert!((w.cd1_1 + 0.001).abs() < 1e-12);
        assert!((w.cd2_2 - 0.001).abs() < 1e-12);
        assert_eq!(w.solution_kind(), "TAN");
    }

    #[test]
    fn legacy_wcs_is_approximate() {
        let mut h = HashMap::new();
        h.insert("RA".into(), "12:00:00".into());
        h.insert("DEC".into(), "+45:00:00".into());
        h.insert("NAXIS1".into(), "1000".into());
        h.insert("NAXIS2".into(), "1000".into());
        h.insert("SECPIX".into(), "0.5".into());
        let w = FitsImageData::parse_wcs(&h).unwrap();
        assert!(w.is_approximate);
        assert!((w.crval1 - 180.0).abs() < 1e-9);
        assert_eq!(w.solution_kind(), "approximate");
    }

    #[test]
    fn format_coords_basic() {
        let (ra, dec) = WcsInfo::format_coords(180.0, 45.0);
        assert!(ra.starts_with("12h00m"));
        assert!(dec.starts_with("+45d00m"));
    }

    #[test]
    fn hdu_label_formats() {
        let sci = HduInfo {
            index: 1,
            name: "SCI".into(),
            width: 2048,
            height: 4096,
            depth: 0,
            is_image: true,
        };
        assert_eq!(sci.label(), "1: SCI 2048×4096");
        let tbl = HduInfo {
            index: 2,
            name: "EVENTS".into(),
            width: 0,
            height: 0,
            depth: 0,
            is_image: false,
        };
        assert_eq!(tbl.label(), "2: EVENTS (non-image)");
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

#[cfg(test)]
mod pc_matrix_tests {
    use super::*;

    /// A JWST i2d header: PC + CDELT, no CD, no CROTA2.
    ///
    /// The real keywords from
    /// `jw01783-o003_t009_nircam_clear-f187n_i2d.fits`, whose sky positions
    /// were 40 to 90 arcseconds out before the PC branch existed — the scale
    /// was right and the rotation had silently become zero.
    fn jwst_cards() -> std::collections::HashMap<String, String> {
        [
            ("CTYPE1", "RA---TAN"),
            ("CTYPE2", "DEC--TAN"),
            ("CRPIX1", "5738.423074184073"),
            ("CRPIX2", "2279.5994461063583"),
            ("CRVAL1", "202.46718693459422"),
            ("CRVAL2", "47.20006625102562"),
            ("CDELT1", "8.67324533272714e-06"),
            ("CDELT2", "8.67324533272714e-06"),
            ("PC1_1", "0.6458455917417035"),
            ("PC1_2", "0.763468055407565"),
            ("PC2_1", "0.763468055407565"),
            ("PC2_2", "-0.6458455917417035"),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
    }

    /// The rotation survives, and the position matches astropy.
    ///
    /// Through `array_to_sky`, because astropy's `pixel_to_world` is 0-based
    /// and `pixel_to_sky` is not. The tolerance is 5e-6 deg — the precision of
    /// the reference values, and 70x tighter than the 1e-4 this started at.
    /// That slack was 11 pixels wide and hid a whole-pixel convention error
    /// underneath a passing test.
    #[test]
    fn a_pc_matrix_header_places_a_pixel_where_astropy_does() {
        let wcs = FitsImageData::parse_wcs(&jwst_cards()).expect("a valid WCS");
        // astropy, same header: (202.484210, 47.197671).
        let (ra, dec) = wcs.array_to_sky(6388.0, 3475.0);
        assert!(
            (ra - 202.484210).abs() < 5e-6,
            "RA {ra} is not astropy's 202.484210"
        );
        assert!(
            (dec - 47.197671).abs() < 5e-6,
            "Dec {dec} is not astropy's 47.197671"
        );
    }

    /// A second point, further from the reference pixel.
    ///
    /// A dropped rotation is right AT the reference pixel and wrong everywhere
    /// else, in proportion to the distance — so one point near it proves
    /// nothing.
    #[test]
    fn the_error_does_not_grow_with_distance_from_the_reference_pixel() {
        let wcs = FitsImageData::parse_wcs(&jwst_cards()).expect("a valid WCS");
        // astropy on the real file, 0-based: these four are the reference.
        for (x, y, ra_ref, dec_ref) in [
            (0.0, 0.0, 202.397711448, 47.174817246),
            (2682.0, 2246.0, 202.441688739, 47.180013797),
            // The far corner of an 11471x4593 frame: the point where a
            // dropped rotation is most wrong, and the one worth keeping.
            (11470.0, 4592.0, 202.537027580, 47.225046005),
        ] {
            let (ra, dec) = wcs.array_to_sky(x, y);
            assert!(
                (ra - ra_ref).abs() < 5e-6,
                "RA at ({x},{y}): {ra} not {ra_ref}"
            );
            assert!(
                (dec - dec_ref).abs() < 5e-6,
                "Dec at ({x},{y}): {dec} not {dec_ref}"
            );
        }
    }

    /// CRPIX maps to CRVAL, which is what pins the pixel convention.
    ///
    /// Measured rather than assumed: passing CRPIX straight in lands exactly on
    /// CRVAL, so `pixel_to_sky` counts pixels the way CRPIX does. Subtracting
    /// one — astropy's 0-based convention — misses by 0.044 arcsec, which on
    /// this 0.031 arcsec image is the diagonal of one pixel.
    ///
    /// That one-pixel difference from astropy is a separate question from the
    /// rotation this branch fixes, and a much smaller one: the dropped PC
    /// matrix was out by 40 to 90 arcseconds.
    #[test]
    fn the_reference_pixel_lands_on_crval() {
        let wcs = FitsImageData::parse_wcs(&jwst_cards()).expect("a valid WCS");
        let (ra, dec) = wcs.pixel_to_sky(5738.423074184073, 2279.5994461063583);
        let sep = (((ra - 202.46718693459422) * 47.2f64.to_radians().cos()).powi(2)
            + (dec - 47.20006625102562).powi(2))
        .sqrt()
            * 3600.0;
        assert!(sep < 0.001, "CRPIX is {sep:.4} arcsec from CRVAL");
    }

    /// Each convention round-trips through the sky and comes back unchanged.
    ///
    /// This is the property crosshair and mark sync depend on: a position goes
    /// display -> sky on one tab and sky -> display on another, so the offsets
    /// have to cancel exactly. Changing one direction without the other would
    /// pull linked tabs a pixel apart, which is the failure this pairing
    /// exists to prevent — and it would look like a WCS problem, not a
    /// bookkeeping one.
    #[test]
    fn every_convention_round_trips() {
        let wcs = FitsImageData::parse_wcs(&jwst_cards()).expect("a valid WCS");
        for (x, y) in [(0.0, 0.0), (6388.0, 3475.0), (2682.5, 2246.5)] {
            let (ra, dec) = wcs.display_to_sky(x, y);
            let (bx, by) = wcs.sky_to_display(ra, dec).expect("invertible");
            assert!(
                (bx - x).abs() < 1e-6 && (by - y).abs() < 1e-6,
                "display ({x},{y}) came back ({bx},{by})"
            );

            let (ra, dec) = wcs.array_to_sky(x, y);
            let (bx, by) = wcs.sky_to_array(ra, dec).expect("invertible");
            assert!(
                (bx - x).abs() < 1e-6 && (by - y).abs() < 1e-6,
                "array ({x},{y}) came back ({bx},{by})"
            );
        }
    }

    /// The three conventions sit half a pixel apart, in the stated order.
    ///
    /// Pinned as an identity rather than a number so it reads as the
    /// definition it is: a display coordinate is the pixel's corner, an array
    /// index is its centre counting from zero, and a FITS pixel is its centre
    /// counting from one.
    #[test]
    fn the_conventions_differ_by_the_offsets_they_claim() {
        let wcs = FitsImageData::parse_wcs(&jwst_cards()).expect("a valid WCS");
        let fits = wcs.pixel_to_sky(101.0, 51.0);
        assert_eq!(
            wcs.array_to_sky(100.0, 50.0),
            fits,
            "array index is FITS minus one"
        );
        assert_eq!(
            wcs.display_to_sky(100.5, 50.5),
            fits,
            "display is FITS minus a half"
        );
    }

    /// CD still wins when both are present, as the standard says.
    #[test]
    fn a_cd_matrix_takes_precedence_over_pc() {
        let mut cards = jwst_cards();
        for (k, v) in [
            ("CD1_1", "-1.0e-05"),
            ("CD1_2", "0.0"),
            ("CD2_1", "0.0"),
            ("CD2_2", "1.0e-05"),
        ] {
            cards.insert(k.to_string(), v.to_string());
        }
        let wcs = FitsImageData::parse_wcs(&cards).expect("a valid WCS");
        assert!(
            (wcs.cd1_1 - -1.0e-05).abs() < 1e-12,
            "PC overrode an explicit CD matrix: cd1_1 = {}",
            wcs.cd1_1
        );
    }
}
