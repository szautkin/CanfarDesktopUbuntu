//! World Coordinate System for a 3D spectral cube.
//!
//! Ported from `Services/CubeViewer/CubeWcs.cs`. The two spatial axes reuse the
//! rigorous [`WcsInfo`](crate::models::fits_image::WcsInfo) projection machinery
//! (parsed from the same FITS header via the shared parser); the spectral third
//! axis (FREQ / VELO / WAVE / WAVN / FDEP …) is parsed here and exposed through
//! unit-aware channel→physical conversions used by the 3D box captions and the
//! channel readout.

use std::collections::HashMap;

use crate::models::fits_image::{FitsImageData, WcsInfo};

/// The spectral (third) axis of a cube, as read from CTYPE3/CUNIT3/CRPIX3/CRVAL3
/// and CDELT3 (falling back to CD3_3). All fields are in the header's native units.
#[derive(Debug, Clone)]
pub struct SpectralAxis {
    /// CTYPE3 (e.g. `"FREQ"`, `"VRAD"`, `"WAVE-F2W"`), trimmed.
    pub ctype3: String,
    /// CUNIT3 (e.g. `"Hz"`, `"m/s"`, `"m"`), trimmed; may be empty.
    pub cunit3: String,
    /// Reference pixel along the spectral axis (1-based FITS convention), default 1.0.
    pub crpix3: f64,
    /// World value at the reference pixel, in `cunit3` units.
    pub crval3: f64,
    /// Increment per channel, in `cunit3` units (CDELT3 or CD3_3).
    pub cdelt3: f64,
}

impl SpectralAxis {
    /// Parse the spectral axis from a FITS header, or `None` when there is no
    /// usable spectral increment (mirrors the C# `HasSpectral` guard `CDELT3 != 0`).
    fn from_header(h: &HashMap<String, String>) -> Option<SpectralAxis> {
        let get_str = |key: &str| -> String {
            h.get(key)
                .map(|v| v.trim().trim_matches('\'').trim().to_string())
                .unwrap_or_default()
        };
        let get_f64 = |key: &str| -> Option<f64> { h.get(key).and_then(|v| v.trim().parse().ok()) };

        // CDELT3 is the common spectral increment; some cubes use CD3_3 instead.
        let mut cdelt3 = get_f64("CDELT3").unwrap_or(0.0);
        if cdelt3 == 0.0 && h.contains_key("CD3_3") {
            cdelt3 = get_f64("CD3_3").unwrap_or(0.0);
        }
        if cdelt3 == 0.0 {
            return None;
        }

        Some(SpectralAxis {
            ctype3: get_str("CTYPE3"),
            cunit3: get_str("CUNIT3"),
            crpix3: get_f64("CRPIX3").unwrap_or(1.0),
            crval3: get_f64("CRVAL3").unwrap_or(0.0),
            cdelt3,
        })
    }

    /// The algorithm-independent base code of CTYPE3 (`"WAVE-F2W"` → `"WAVE"`),
    /// upper-cased. Mirrors C# `SpecBase()`: the token before the first `-`, but
    /// only when that `-` is not the leading character.
    fn base(ctype: &str) -> String {
        match ctype.find('-') {
            Some(dash) if dash > 0 => ctype[..dash].to_ascii_uppercase(),
            _ => ctype.to_ascii_uppercase(),
        }
    }

    /// Raw spectral world value at a fractional 0-based channel, in native units:
    /// `CRVAL3 + ((z + 1) − CRPIX3)·CDELT3` (the `+1` converts 0-based to FITS 1-based).
    fn raw_value(&self, z: f64) -> f64 {
        self.crval3 + ((z + 1.0) - self.crpix3) * self.cdelt3
    }

    /// The native physical unit label: CUNIT3 when present, else the FITS default
    /// for the axis kind (FREQ→Hz, velocity→m/s, wavelength→m, …).
    fn native_unit(&self) -> String {
        if !self.cunit3.is_empty() {
            return self.cunit3.clone();
        }
        match Self::base(&self.ctype3).as_str() {
            "FREQ" => "Hz".into(),
            "VRAD" | "VELO" | "VOPT" => "m/s".into(),
            "WAVE" | "AWAV" => "m".into(),
            "WAVN" => "m^-1".into(),
            "FDEP" => "rad/m^2".into(),
            _ => String::new(),
        }
    }

    /// Convert a raw spectral world value to the convenience display unit, honoring
    /// CUNIT3 so a cube already stored in km/s or GHz is never divided again.
    /// Mirrors C# `ConvertSpectral`.
    fn convert_display(&self, v: f64) -> f64 {
        let u = self.cunit3.trim().to_ascii_lowercase();
        match Self::base(&self.ctype3).as_str() {
            "FREQ" => {
                if u == "ghz" {
                    v
                } else if u == "mhz" {
                    v / 1e3
                } else if u == "khz" {
                    v / 1e6
                } else {
                    v / 1e9 // Hz (default) → GHz
                }
            }
            "VRAD" | "VELO" | "VOPT" => {
                if u.starts_with("km") {
                    v
                } else {
                    v / 1e3 // m/s → km/s
                }
            }
            "WAVE" | "AWAV" => {
                if u == "um" || u == "µm" || u == "micron" || u == "microns" {
                    v
                } else if u == "nm" {
                    v / 1e3 // nm → µm
                } else if u == "angstrom" || u == "a" || u == "ang" {
                    v / 1e4 // Å → µm
                } else {
                    v * 1e6 // m (default) → µm
                }
            }
            _ => v, // WAVN / FDEP / unknown: raw
        }
    }

    /// Display unit label after the convenience conversion. Mirrors C# `SpecUnitDisplay`.
    fn display_unit(&self) -> String {
        match Self::base(&self.ctype3).as_str() {
            "FREQ" => "GHz".into(),
            "VRAD" | "VELO" | "VOPT" => "km/s".into(),
            "WAVE" | "AWAV" => "µm".into(),
            "WAVN" => {
                if self.cunit3.is_empty() {
                    "cm⁻¹".into()
                } else {
                    self.cunit3.clone()
                }
            }
            "FDEP" => {
                if self.cunit3.is_empty() {
                    "rad/m²".into()
                } else {
                    self.cunit3.clone()
                }
            }
            _ => self.cunit3.clone(),
        }
    }
}

/// The full cube WCS: spatial (first two axes) plus optional spectral third axis.
#[derive(Debug, Clone)]
pub struct CubeWcs {
    /// Spatial (RA/Dec or GLON/GLAT) WCS for the first two axes, when valid.
    pub spatial: Option<WcsInfo>,
    /// Spectral third axis, when the cube has a usable increment.
    pub spectral: Option<SpectralAxis>,
    /// True when the spatial frame is galactic (CTYPE1 = GLON-…).
    pub galactic: bool,
    /// Rest frequency in Hz (RESTFRQ/RESTFREQ) for frequency↔velocity conversion.
    pub rest_frequency_hz: Option<f64>,
    /// Spectral reference frame (SPECSYS — LSRK/BARYCENT/TOPOCENT/…).
    pub spectral_frame: String,
    /// Synthesized beam (degrees): major/minor axis + position angle (BMAJ/BMIN/BPA).
    pub beam_major_deg: Option<f64>,
    pub beam_minor_deg: Option<f64>,
    pub beam_pa_deg: Option<f64>,
}

impl CubeWcs {
    /// Longitude axis name: "GLON" (galactic) or "RA" (equatorial).
    pub fn lon_name(&self) -> &'static str {
        if self.galactic {
            "GLON"
        } else {
            "RA"
        }
    }

    /// Latitude axis name: "GLAT" (galactic) or "DEC" (equatorial).
    pub fn lat_name(&self) -> &'static str {
        if self.galactic {
            "GLAT"
        } else {
            "DEC"
        }
    }
}

impl CubeWcs {
    /// Build the cube WCS from a parsed FITS header. The spatial solution reuses
    /// the shared [`WcsInfo`] parser (via [`FitsImageData`]) so the projection
    /// machinery is identical to the 2D viewer; the spectral axis is parsed here.
    pub fn from_header(h: &HashMap<String, String>) -> Self {
        // Reuse the canonical WCS parser: FitsImageData::new runs parse_wcs on the
        // header and stores the resulting Option<WcsInfo>. Dimensions/pixels are
        // irrelevant to the spatial WCS solution, so pass an empty image.
        let spatial = FitsImageData::new(0, 0, Vec::new(), h.clone()).wcs;
        let spectral = SpectralAxis::from_header(h);

        let get_str = |k: &str| -> String {
            h.get(k)
                .map(|v| v.trim().trim_matches('\'').trim().to_string())
                .unwrap_or_default()
        };
        let get_f64 = |k: &str| -> Option<f64> { h.get(k).and_then(|v| v.trim().parse().ok()) };
        let galactic = get_str("CTYPE1").to_ascii_uppercase().starts_with("GLON");

        CubeWcs {
            spatial,
            spectral,
            galactic,
            rest_frequency_hz: get_f64("RESTFRQ").or_else(|| get_f64("RESTFREQ")),
            spectral_frame: get_str("SPECSYS"),
            beam_major_deg: get_f64("BMAJ"),
            beam_minor_deg: get_f64("BMIN"),
            beam_pa_deg: get_f64("BPA"),
        }
    }

    /// True when the cube carries a usable spectral axis.
    pub fn has_spectral(&self) -> bool {
        self.spectral.is_some()
    }

    /// The raw physical spectral value at a fractional 0-based channel `z`, together
    /// with its native unit label (e.g. `(1.4e9, "Hz")` for a FREQ axis in Hz).
    /// `None` when the cube has no spectral axis.
    pub fn channel_to_physical(&self, z: f64) -> Option<(f64, String)> {
        let s = self.spectral.as_ref()?;
        Some((s.raw_value(z), s.native_unit()))
    }

    /// Human axis name for the spectral axis (`"FREQUENCY"`, `"VELOCITY"`, …), or
    /// `"CHANNEL"` when there is no spectral WCS. Mirrors C# `SpecAxisName`.
    pub fn spec_axis_name(&self) -> String {
        match &self.spectral {
            None => "CHANNEL".into(),
            Some(s) => match SpectralAxis::base(&s.ctype3).as_str() {
                "FREQ" => "FREQUENCY".into(),
                "VRAD" | "VELO" | "VOPT" => "VELOCITY".into(),
                "WAVE" | "AWAV" => "WAVELENGTH".into(),
                "WAVN" => "WAVENUMBER".into(),
                "FDEP" => "FARADAY DEPTH".into(),
                _ => {
                    if s.ctype3.is_empty() {
                        "SPECTRAL".into()
                    } else {
                        s.ctype3.to_ascii_uppercase()
                    }
                }
            },
        }
    }

    /// A compact caption for a 0-based channel in the convenience display unit
    /// (e.g. `"1.4 GHz"`, `"−12.5 km/s"`), or `"CH {z}"` when there is no spectral
    /// axis. Combines C# `SpecText` (value) with `SpecUnitDisplay` (label).
    pub fn channel_label(&self, z: usize) -> String {
        match &self.spectral {
            None => format!("CH {}", z),
            Some(s) => {
                let converted = s.convert_display(s.raw_value(z as f64));
                let value = fmt3(converted);
                let unit = s.display_unit();
                if unit.is_empty() {
                    value
                } else {
                    format!("{} {}", value, unit)
                }
            }
        }
    }
}

/// Format a number with up to 3 decimal places, trimming trailing zeros (and a
/// bare decimal point). The Rust analogue of C#'s `"0.###"` numeric format.
fn fmt3(v: f64) -> String {
    let s = format!("{:.3}", v);
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn linear_freq_axis_channel_to_hz() {
        // A linear FREQ axis: 1.4 GHz at channel 0, +1 MHz per channel, in Hz.
        let h = header(&[
            ("CTYPE3", "FREQ"),
            ("CUNIT3", "Hz"),
            ("CRPIX3", "1"),
            ("CRVAL3", "1.4e9"),
            ("CDELT3", "1e6"),
        ]);
        let wcs = CubeWcs::from_header(&h);
        assert!(wcs.has_spectral());

        let (v0, unit0) = wcs.channel_to_physical(0.0).unwrap();
        assert!((v0 - 1.4e9).abs() < 1.0, "v0 = {}", v0);
        assert_eq!(unit0, "Hz");

        // Channel 100 → 1.4 GHz + 100 MHz = 1.5 GHz (still in Hz).
        let (v100, _) = wcs.channel_to_physical(100.0).unwrap();
        assert!((v100 - 1.5e9).abs() < 1.0, "v100 = {}", v100);
    }

    #[test]
    fn freq_channel_label_uses_ghz() {
        let h = header(&[
            ("CTYPE3", "FREQ"),
            ("CUNIT3", "Hz"),
            ("CRPIX3", "1"),
            ("CRVAL3", "1.4e9"),
            ("CDELT3", "1e6"),
        ]);
        let wcs = CubeWcs::from_header(&h);
        assert_eq!(wcs.channel_label(0), "1.4 GHz");
        assert_eq!(wcs.spec_axis_name(), "FREQUENCY");
    }

    #[test]
    fn already_ghz_is_not_reconverted() {
        // CUNIT3 = GHz: display must not divide by 1e9 again.
        let h = header(&[
            ("CTYPE3", "FREQ"),
            ("CUNIT3", "GHz"),
            ("CRPIX3", "1"),
            ("CRVAL3", "230.0"),
            ("CDELT3", "0.001"),
        ]);
        let wcs = CubeWcs::from_header(&h);
        let (v, unit) = wcs.channel_to_physical(0.0).unwrap();
        assert!((v - 230.0).abs() < 1e-9);
        assert_eq!(unit, "GHz");
        assert_eq!(wcs.channel_label(0), "230 GHz");
    }

    #[test]
    fn velocity_axis_ms_to_kms() {
        // VRAD in m/s: physical stays m/s, label converts to km/s.
        let h = header(&[
            ("CTYPE3", "VRAD"),
            ("CUNIT3", "m/s"),
            ("CRPIX3", "1"),
            ("CRVAL3", "1000"),
            ("CDELT3", "500"),
        ]);
        let wcs = CubeWcs::from_header(&h);
        let (v, unit) = wcs.channel_to_physical(0.0).unwrap();
        assert!((v - 1000.0).abs() < 1e-9);
        assert_eq!(unit, "m/s");
        assert_eq!(wcs.spec_axis_name(), "VELOCITY");
        assert_eq!(wcs.channel_label(0), "1 km/s"); // 1000 m/s → 1 km/s
        assert_eq!(wcs.channel_label(2), "2 km/s"); // 2000 m/s → 2 km/s
    }

    #[test]
    fn wavelength_meters_to_microns() {
        // WAVE in metres (default) → µm in the display.
        let h = header(&[
            ("CTYPE3", "WAVE"),
            ("CRPIX3", "1"),
            ("CRVAL3", "5e-7"), // 500 nm
            ("CDELT3", "1e-9"),
        ]);
        let wcs = CubeWcs::from_header(&h);
        let (v, unit) = wcs.channel_to_physical(0.0).unwrap();
        assert!((v - 5e-7).abs() < 1e-18);
        assert_eq!(unit, "m"); // native default for WAVE
        assert_eq!(wcs.channel_label(0), "0.5 µm"); // 5e-7 m → 0.5 µm
        assert_eq!(wcs.spec_axis_name(), "WAVELENGTH");
    }

    #[test]
    fn cd3_3_is_used_when_cdelt3_absent() {
        let h = header(&[
            ("CTYPE3", "FREQ"),
            ("CUNIT3", "Hz"),
            ("CRPIX3", "1"),
            ("CRVAL3", "1.0e9"),
            ("CD3_3", "2e6"),
        ]);
        let wcs = CubeWcs::from_header(&h);
        let s = wcs.spectral.as_ref().unwrap();
        assert!((s.cdelt3 - 2e6).abs() < 1.0);
        let (v1, _) = wcs.channel_to_physical(1.0).unwrap(); // 1.0e9 + 2e6
        assert!((v1 - 1.002e9).abs() < 1.0, "v1 = {}", v1);
    }

    #[test]
    fn spec_base_strips_algorithm_suffix() {
        // "WAVE-F2W" must resolve to the WAVE family.
        let h = header(&[
            ("CTYPE3", "WAVE-F2W"),
            ("CUNIT3", "m"),
            ("CRPIX3", "1"),
            ("CRVAL3", "1e-6"),
            ("CDELT3", "1e-9"),
        ]);
        let wcs = CubeWcs::from_header(&h);
        assert_eq!(wcs.spec_axis_name(), "WAVELENGTH");
    }

    #[test]
    fn no_spectral_axis_falls_back_to_channel() {
        let h = header(&[("CTYPE1", "RA---TAN"), ("CTYPE2", "DEC--TAN")]);
        let wcs = CubeWcs::from_header(&h);
        assert!(!wcs.has_spectral());
        assert!(wcs.channel_to_physical(3.0).is_none());
        assert_eq!(wcs.channel_label(5), "CH 5");
        assert_eq!(wcs.spec_axis_name(), "CHANNEL");
    }

    #[test]
    fn spatial_reuses_wcsinfo_parser() {
        // A valid TAN header must yield a spatial solution identical to the 2D path.
        let h = header(&[
            ("CTYPE1", "RA---TAN"),
            ("CTYPE2", "DEC--TAN"),
            ("CRPIX1", "50"),
            ("CRPIX2", "50"),
            ("CRVAL1", "180"),
            ("CRVAL2", "45"),
            ("CD1_1", "-0.001"),
            ("CD1_2", "0"),
            ("CD2_1", "0"),
            ("CD2_2", "0.001"),
        ]);
        let wcs = CubeWcs::from_header(&h);
        let sp = wcs.spatial.as_ref().expect("spatial WCS");
        let (ra, dec) = sp.pixel_to_sky(50.0, 50.0);
        assert!((ra - 180.0).abs() < 1e-9);
        assert!((dec - 45.0).abs() < 1e-9);
    }

    #[test]
    fn fmt3_trims_trailing_zeros() {
        assert_eq!(fmt3(1.4), "1.4");
        assert_eq!(fmt3(1.0), "1");
        assert_eq!(fmt3(1000.0), "1000");
        assert_eq!(fmt3(1.2345), "1.234"); // rounds to 3 dp ({:.3} banker-free)
        assert_eq!(fmt3(0.5), "0.5");
    }
}
