//! Per-column display-unit catalog + the display-side unit renderers for the
//! search-results grid.
//!
//! Port of `CanfarDesktop/Helpers/ColumnUnitCatalog.cs` (the switchable unit menu
//! for each column) together with the macOS-faithful display converters from
//! `UnitConverter.cs` (`FormatSpectral`/`FormatDuration`/`FormatAngle`/`FormatArea`
//! and `ConvertSpectral`). Kept self-contained (the search-form-side SI converters
//! live in `unit_converter.rs`); these use the legacy CGS constants so a TAP row
//! renders to the same numbers as the reference client for any chosen unit.
//!
//! Column keys are the cleaned header ids produced by
//! [`crate::models::search_result::clean_key`].

use crate::models::search_result::clean_key;

/// One selectable unit in a column's unit menu (stable id + user-facing label).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnitChoice {
    pub id: &'static str,
    pub label: &'static str,
}

const fn uc(id: &'static str, label: &'static str) -> UnitChoice {
    UnitChoice { id, label }
}

// ── Spectral unit table (id, label, dimension, factor-from-base) ─────────────

#[derive(Clone, Copy)]
enum SpectralDim {
    Wavelength,
    Frequency,
    Energy,
}

struct SpectralUnit {
    id: &'static str,
    label: &'static str,
    dim: SpectralDim,
    factor: f64,
}

const SPECTRAL: &[SpectralUnit] = &[
    SpectralUnit {
        id: "m",
        label: "m",
        dim: SpectralDim::Wavelength,
        factor: 1.0,
    },
    SpectralUnit {
        id: "cm",
        label: "cm",
        dim: SpectralDim::Wavelength,
        factor: 1e-2,
    },
    SpectralUnit {
        id: "mm",
        label: "mm",
        dim: SpectralDim::Wavelength,
        factor: 1e-3,
    },
    SpectralUnit {
        id: "um",
        label: "µm",
        dim: SpectralDim::Wavelength,
        factor: 1e-6,
    },
    SpectralUnit {
        id: "nm",
        label: "nm",
        dim: SpectralDim::Wavelength,
        factor: 1e-9,
    },
    SpectralUnit {
        id: "a",
        label: "Å",
        dim: SpectralDim::Wavelength,
        factor: 1e-10,
    },
    SpectralUnit {
        id: "hz",
        label: "Hz",
        dim: SpectralDim::Frequency,
        factor: 1.0,
    },
    SpectralUnit {
        id: "khz",
        label: "kHz",
        dim: SpectralDim::Frequency,
        factor: 1e3,
    },
    SpectralUnit {
        id: "mhz",
        label: "MHz",
        dim: SpectralDim::Frequency,
        factor: 1e6,
    },
    SpectralUnit {
        id: "ghz",
        label: "GHz",
        dim: SpectralDim::Frequency,
        factor: 1e9,
    },
    SpectralUnit {
        id: "ev",
        label: "eV",
        dim: SpectralDim::Energy,
        factor: 1.0,
    },
    SpectralUnit {
        id: "kev",
        label: "keV",
        dim: SpectralDim::Energy,
        factor: 1e3,
    },
    SpectralUnit {
        id: "mev",
        label: "MeV",
        dim: SpectralDim::Energy,
        factor: 1e6,
    },
    SpectralUnit {
        id: "gev",
        label: "GeV",
        dim: SpectralDim::Energy,
        factor: 1e9,
    },
];

/// The ordered spectral unit choices for a spectral column's unit menu.
pub fn spectral_unit_choices() -> Vec<UnitChoice> {
    SPECTRAL.iter().map(|u| uc(u.id, u.label)).collect()
}

// ── Column → unit-menu catalog ──────────────────────────────────────────────

/// Whether a column (by header or cleaned key) has a switchable unit menu.
pub fn has_menu(column_key: &str) -> bool {
    menu_for(column_key).is_some()
}

/// Ordered unit choices for a column's menu (empty if the column has none).
pub fn available_units(column_key: &str) -> Vec<UnitChoice> {
    menu_for(column_key).map(|(c, _)| c).unwrap_or_default()
}

/// Whether `unit_id` is a display unit this column actually offers.
///
/// An empty id means "reset to the column's default" and is always accepted, so
/// a caller can clear a choice without knowing what the default is. Lives here
/// rather than at the call site so validation can never drift from the menu it
/// validates against.
pub fn is_valid_unit(column_key: &str, unit_id: &str) -> bool {
    unit_id.is_empty() || available_units(column_key).iter().any(|c| c.id == unit_id)
}

/// The default unit id for a column's menu, if it has one.
pub fn default_unit_id(column_key: &str) -> Option<&'static str> {
    menu_for(column_key).map(|(_, d)| d)
}

/// Resolve a column's (choices, default) pair. Keys mirror the macOS
/// `CellFormatterRegistry.sets`; RA/Dec default to sexagesimal.
fn menu_for(column_key: &str) -> Option<(Vec<UnitChoice>, &'static str)> {
    let coord = |sex_id, sex_label| vec![uc(sex_id, sex_label), uc("degrees", "Degrees")];
    let choices = match clean_key(column_key).as_str() {
        "ra(j20000)" => (coord("hms", "H:M:S"), "hms"),
        "dec(j20000)" => (coord("dms", "D:M:S"), "dms"),
        "minwavelength" | "maxwavelength" | "restframeenergy" => (spectral_unit_choices(), "m"),
        "inttime" => (
            vec![
                uc("seconds", "Seconds"),
                uc("minutes", "Minutes"),
                uc("hours", "Hours"),
                uc("days", "Days"),
            ],
            "seconds",
        ),
        "pixelscale" => (
            vec![
                uc("milliarcseconds", "Milliarcseconds"),
                uc("arcseconds", "Arcseconds"),
                uc("arcminutes", "Arcminutes"),
                uc("degrees", "Degrees"),
            ],
            "arcseconds",
        ),
        // Image quality: like pixel scale but without a degrees option (macOS imageQualitySet).
        "positionresolution" => (
            vec![
                uc("milliarcseconds", "Milliarcseconds"),
                uc("arcseconds", "Arcseconds"),
                uc("arcminutes", "Arcminutes"),
            ],
            "arcseconds",
        ),
        "fieldofview" => (
            vec![
                uc("sq_arcsec", "Sq. arcsec"),
                uc("sq_arcmin", "Sq. arcmin"),
                uc("sq_deg", "Sq. deg"),
            ],
            "sq_deg",
        ),
        "startdate" | "enddate" => (
            vec![uc("calendar", "Calendar"), uc("mjd", "MJD")],
            "calendar",
        ),
        _ => return None,
    };
    Some(choices)
}

// ── Display-side converters (legacy CGS constants, macOS-faithful) ───────────

const CGS_SPEED_OF_LIGHT: f64 = 2.997925e8; // m/s
const CGS_PLANCK: f64 = 6.6262e-27; // erg·s
const CGS_ERG_PER_EV: f64 = 1.602192e-12; // erg/eV

/// Convert a wavelength in metres to `unit_id` (cross-dimension via c and hc).
/// `None` on non-positive / non-finite input or an unknown unit.
pub fn convert_spectral(metres: f64, unit_id: &str) -> Option<f64> {
    if !metres.is_finite() || metres <= 0.0 {
        return None;
    }
    let id = unit_id.to_lowercase();
    let u = SPECTRAL.iter().find(|u| u.id == id)?;
    Some(match u.dim {
        SpectralDim::Wavelength => metres / u.factor,
        SpectralDim::Frequency => CGS_SPEED_OF_LIGHT / metres / u.factor,
        SpectralDim::Energy => {
            CGS_PLANCK * CGS_SPEED_OF_LIGHT / (CGS_ERG_PER_EV * metres) / u.factor
        }
    })
}

/// Render a metres-stored wavelength as `"value label"` in the chosen spectral
/// unit; trimmed raw on failure.
pub fn format_spectral(raw: &str, unit_id: &str) -> String {
    let Some(metres) = finite(raw) else {
        return raw.trim().to_string();
    };
    let Some(value) = convert_spectral(metres, unit_id) else {
        return raw.trim().to_string();
    };
    let label = SPECTRAL
        .iter()
        .find(|u| u.id == unit_id.to_lowercase())
        .map(|u| u.label)
        .unwrap_or(unit_id);
    format!("{} {}", spectral_value_string(value), label)
}

/// Render seconds as `"value label"` in the chosen duration unit.
pub fn format_duration(raw: &str, unit_id: &str) -> String {
    let Some(seconds) = finite(raw) else {
        return raw.trim().to_string();
    };
    let (factor, label) = match unit_id.to_lowercase().as_str() {
        "minutes" => (60.0, "m"),
        "hours" => (3600.0, "h"),
        "days" => (86400.0, "d"),
        _ => (1.0, "s"),
    };
    format!("{:.3} {}", seconds / factor, label)
}

/// Render degrees as `"value label"` in the chosen angle unit.
pub fn format_angle(raw: &str, unit_id: &str) -> String {
    let Some(degrees) = finite(raw) else {
        return raw.trim().to_string();
    };
    let (factor, label) = match unit_id.to_lowercase().as_str() {
        "milliarcseconds" => (3_600_000.0, "mas"),
        "arcminutes" => (60.0, "′"),
        "degrees" => (1.0, "°"),
        _ => (3600.0, "″"), // arcseconds
    };
    format!("{} {}", adaptive_precision(degrees * factor), label)
}

/// Render square-degrees as `"value label"` in the chosen area unit.
pub fn format_area(raw: &str, unit_id: &str) -> String {
    let Some(sq_deg) = finite(raw) else {
        return raw.trim().to_string();
    };
    let (factor, label) = match unit_id.to_lowercase().as_str() {
        "sq_arcsec" => (12_960_000.0, "sq arcsec"),
        "sq_arcmin" => (3600.0, "sq arcmin"),
        _ => (1.0, "sq deg"),
    };
    format!("{} {}", adaptive_precision(sq_deg * factor), label)
}

// macOS SpectralFormatter precision ladder: >=100 → 1dp, >=1 → 2dp, >=0.001 → 3dp, else 4 sig figs.
fn spectral_value_string(v: f64) -> String {
    let mag = v.abs();
    if mag == 0.0 {
        "0".to_string()
    } else if mag >= 100.0 {
        format!("{:.1}", v)
    } else if mag >= 1.0 {
        format!("{:.2}", v)
    } else if mag >= 0.001 {
        format!("{:.3}", v)
    } else {
        sig_figs(v, 4)
    }
}

/// macOS adaptivePrecisionString: 6dp below 0.001 (non-zero), else 3dp.
fn adaptive_precision(v: f64) -> String {
    let mag = v.abs();
    if mag != 0.0 && mag < 0.001 {
        format!("{:.6}", v)
    } else {
        format!("{:.3}", v)
    }
}

/// Significant-figure formatting with trailing-zero trim (approximates .NET "G{n}"
/// for the small-magnitude spectral branch).
fn sig_figs(v: f64, figs: i32) -> String {
    if v == 0.0 || !v.is_finite() {
        return "0".to_string();
    }
    let exp = v.abs().log10().floor() as i32;
    let decimals = (figs - 1 - exp).max(0) as usize;
    let mut s = format!("{:.*}", decimals, v);
    if s.contains('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
    }
    s
}

fn finite(raw: &str) -> Option<f64> {
    raw.trim().parse::<f64>().ok().filter(|v| v.is_finite())
}

#[cfg(test)]
mod tests {

    #[test]
    fn is_valid_unit_accepts_a_columns_own_menu_and_rejects_others() {
        // `set_search_results_view` validates against this before touching state,
        // so a wrong unit id must be rejected rather than silently stored.
        let units = available_units("Pixel Scale");
        assert!(!units.is_empty(), "pixel scale should offer a unit menu");
        for choice in &units {
            assert!(
                is_valid_unit("Pixel Scale", choice.id),
                "{} is one of this column's own units",
                choice.id
            );
        }
        // A unit that belongs to a DIFFERENT column is still invalid here.
        assert!(!is_valid_unit("Pixel Scale", "hours"));
        assert!(!is_valid_unit("Pixel Scale", "parsec"));
    }

    #[test]
    fn an_empty_unit_id_means_reset_to_default_and_is_always_valid() {
        assert!(is_valid_unit("Pixel Scale", ""));
        // Even for a column with no menu at all.
        assert!(is_valid_unit("collection", ""));
    }

    #[test]
    fn a_column_without_a_menu_accepts_no_unit() {
        assert!(!has_menu("collection"));
        assert!(!is_valid_unit("collection", "deg"));
    }

    use super::*;

    #[test]
    fn ra_dec_have_sexagesimal_default() {
        assert!(has_menu("RA (J2000.0)"));
        assert_eq!(default_unit_id("RA (J2000.0)"), Some("hms"));
        assert_eq!(default_unit_id("Dec. (J2000.0)"), Some("dms"));
        // Both offer a degrees fallback.
        let ra = available_units("RA (J2000.0)");
        assert_eq!(ra.len(), 2);
        assert_eq!(ra[0].id, "hms");
        assert_eq!(ra[1].id, "degrees");
    }

    #[test]
    fn menu_lookup_works_on_cleaned_and_raw_keys() {
        assert!(has_menu("Min. Wavelength"));
        assert!(has_menu("minwavelength"));
        assert_eq!(default_unit_id("Int. Time"), Some("seconds"));
        assert_eq!(default_unit_id("Start Date"), Some("calendar"));
        assert_eq!(default_unit_id("Pixel Scale"), Some("arcseconds"));
        assert!(!has_menu("Instrument"));
        assert!(available_units("Instrument").is_empty());
    }

    #[test]
    fn field_of_view_menu_has_no_degrees_but_area_units() {
        let fov = available_units("Field of View");
        let ids: Vec<&str> = fov.iter().map(|u| u.id).collect();
        assert_eq!(ids, vec!["sq_arcsec", "sq_arcmin", "sq_deg"]);
    }

    #[test]
    fn spectral_metres_to_nm() {
        // 5e-7 m == 500 nm
        assert_eq!(format_spectral("5e-7", "nm"), "500.0 nm");
        // Round wavelength stays readable in µm.
        assert_eq!(format_spectral("1e-6", "um"), "1.00 µm");
    }

    #[test]
    fn duration_and_angle_and_area() {
        assert_eq!(format_duration("3600", "hours"), "1.000 h");
        assert_eq!(format_duration("90", "minutes"), "1.500 m");
        // 1 degree pixel scale → arcseconds
        assert_eq!(format_angle("1.0", "arcseconds"), "3600.000 ″");
        assert_eq!(format_area("1.0", "sq_deg"), "1.000 sq deg");
    }

    #[test]
    fn invalid_input_passes_through() {
        assert_eq!(format_spectral("n/a", "nm"), "n/a");
        assert_eq!(format_duration("  ", "hours"), "");
        assert!(convert_spectral(0.0, "nm").is_none());
        assert!(convert_spectral(-1.0, "nm").is_none());
    }
}
