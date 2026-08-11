/// Unit conversion utilities matching the Windows implementation.
const SPEED_OF_LIGHT: f64 = 299_792_458.0; // m/s
const PLANCK_CONSTANT: f64 = 6.626_070_15e-34; // J*s
const EV_TO_JOULES: f64 = 1.602_176_634e-19; // J/eV

/// Spectral units offered in the Search form, in the reference's order
/// (`UnitConverter.SpectralUnits`): wavelength, then frequency, then energy.
///
/// The lists live here rather than in the page because this module is what
/// decides which units mean anything. The Search dropdown previously offered
/// four of these — the frequency and energy units were simply unreachable, so a
/// radio astronomer could not search in GHz and an X-ray one could not search in
/// keV, even though [`to_metres`] has always converted both.
pub const SPECTRAL_UNITS: [&str; 14] = [
    "m", "cm", "mm", "µm", "nm", "Å", "Hz", "kHz", "MHz", "GHz", "eV", "keV", "MeV", "GeV",
];

/// Time units offered in the Search form (`UnitConverter.TimeUnits`).
pub const TIME_UNITS: [&str; 5] = ["s", "m", "h", "d", "y"];

/// Angular units offered for pixel scale (`UnitConverter.PixelScaleUnits`).
pub const PIXEL_SCALE_UNITS: [&str; 3] = ["arcsec", "arcmin", "deg"];

/// Resolve any accepted spelling of a spectral unit to its entry in
/// [`SPECTRAL_UNITS`].
///
/// A unit is PERSISTED as text inside every saved search, and older records hold
/// spellings the current list does not contain verbatim — `Angstrom` for `Å`,
/// `um` for `µm`. A plain equality lookup misses those and falls back to the
/// first entry, which would restore a 500 nm search as **500 metres**. Matching
/// the way [`to_metres`] matches keeps every spelling it accepts selectable.
pub fn canonical_spectral_unit(unit: &str) -> Option<&'static str> {
    let normalized = normalize_spectral(unit);
    SPECTRAL_UNITS
        .iter()
        .copied()
        .find(|candidate| normalize_spectral(candidate) == normalized)
}

/// Fold a spectral unit to the form [`to_metres`] matches on: lower-cased, with
/// `µ`→`u`, `Å`→`a`, and the long name `angstrom` folded onto `a`.
fn normalize_spectral(unit: &str) -> String {
    let folded = unit
        .trim()
        // Substitute BEFORE lower-casing. Doing it after — as this did — is a
        // silent no-op for Å: `to_lowercase` turns U+00C5 into U+00E5 first, so
        // the replacement finds nothing and the unit falls through to `None`.
        // Ångström simply never converted; nothing noticed because the Search
        // dropdown offered the long name `Angstrom` instead.
        //
        // Both the micro sign and the Greek mu appear in the wild, as do the
        // Latin Å, the dedicated Angstrom sign, and their lower-case forms.
        .replace(['\u{00b5}', '\u{03bc}'], "u")
        .replace(['\u{00c5}', '\u{212b}', '\u{00e5}'], "a")
        .to_lowercase();
    match folded.as_str() {
        "angstrom" => "a".to_string(),
        "micron" => "um".to_string(),
        _ => folded,
    }
}

/// Convert a wavelength/frequency/energy value to metres.
pub fn to_metres(value: f64, unit: &str) -> Option<f64> {
    if value <= 0.0 {
        return None;
    }
    // One normalization for the whole module: whatever spelling converts here
    // is also selectable in the Search dropdown, and vice versa.
    let u = normalize_spectral(unit);
    match u.as_str() {
        // Wavelength (direct)
        "m" => Some(value),
        "cm" => Some(value * 1e-2),
        "mm" => Some(value * 1e-3),
        "um" | "micron" => Some(value * 1e-6),
        "nm" => Some(value * 1e-9),
        "a" | "angstrom" => Some(value * 1e-10),
        // Frequency (lambda = c / f)
        "hz" => Some(SPEED_OF_LIGHT / value),
        "khz" => Some(SPEED_OF_LIGHT / (value * 1e3)),
        "mhz" => Some(SPEED_OF_LIGHT / (value * 1e6)),
        "ghz" => Some(SPEED_OF_LIGHT / (value * 1e9)),
        // Energy (lambda = hc / E)
        "ev" => {
            let j = value * EV_TO_JOULES;
            Some(PLANCK_CONSTANT * SPEED_OF_LIGHT / j)
        }
        "kev" => {
            let j = value * 1e3 * EV_TO_JOULES;
            Some(PLANCK_CONSTANT * SPEED_OF_LIGHT / j)
        }
        "mev" => {
            let j = value * 1e6 * EV_TO_JOULES;
            Some(PLANCK_CONSTANT * SPEED_OF_LIGHT / j)
        }
        "gev" => {
            let j = value * 1e9 * EV_TO_JOULES;
            Some(PLANCK_CONSTANT * SPEED_OF_LIGHT / j)
        }
        _ => None,
    }
}

/// Convert a time value to seconds.
pub fn to_seconds(value: f64, unit: &str) -> Option<f64> {
    if value < 0.0 {
        return None;
    }
    match unit.to_lowercase().as_str() {
        "s" => Some(value),
        "m" => Some(value * 60.0),
        "h" => Some(value * 3600.0),
        "d" => Some(value * 86400.0),
        "y" => Some(value * 365.25 * 86400.0),
        _ => None,
    }
}

/// Convert a time value to days.
pub fn to_days(value: f64, unit: &str) -> Option<f64> {
    match unit.to_lowercase().as_str() {
        "s" => Some(value / 86400.0),
        "m" => Some(value / 1440.0),
        "h" => Some(value / 24.0),
        "d" => Some(value),
        "y" => Some(value * 365.25),
        _ => None,
    }
}

/// Convert an angular value to degrees.
pub fn to_degrees(value: f64, unit: &str) -> Option<f64> {
    match unit.to_lowercase().as_str() {
        "arcsec" => Some(value / 3600.0),
        "arcmin" => Some(value / 60.0),
        "deg" => Some(value),
        _ => None,
    }
}

/// Returns true if the unit represents frequency or energy (inverse relationship to wavelength).
pub fn is_inverse_unit(unit: &str) -> bool {
    matches!(
        unit.to_lowercase().as_str(),
        "hz" | "khz" | "mhz" | "ghz" | "ev" | "kev" | "mev" | "gev"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_offered_spectral_unit_actually_converts() {
        // The dropdown is built from this list, so an entry `to_metres` does not
        // understand would be a selectable option that silently drops the whole
        // spectral constraint.
        for unit in SPECTRAL_UNITS {
            assert!(
                to_metres(1.0, unit).is_some(),
                "`{unit}` is offered in the Search form but does not convert"
            );
        }
    }

    #[test]
    fn every_offered_time_unit_actually_converts() {
        for unit in TIME_UNITS {
            assert!(
                to_seconds(1.0, unit).is_some(),
                "`{unit}` is offered but does not convert to seconds"
            );
            assert!(
                to_days(1.0, unit).is_some(),
                "`{unit}` is offered but does not convert to days"
            );
        }
    }

    #[test]
    fn the_frequency_and_energy_units_reach_real_wavelengths() {
        // The point of widening the list: these were unreachable from the UI.
        // 1 GHz is ~29.98 cm; 1 keV is ~1.24 nm.
        let ghz = to_metres(1.0, "GHz").expect("GHz converts");
        assert!((ghz - 0.29979).abs() < 1e-4, "{ghz}");
        let kev = to_metres(1.0, "keV").expect("keV converts");
        assert!((kev - 1.2398e-9).abs() < 1e-12, "{kev}");
    }

    #[test]
    fn a_unit_spelling_from_an_older_saved_search_still_resolves() {
        // Saved searches persist the unit as text. `Angstrom` and `um` were the
        // labels this app shipped before the list matched the reference; if they
        // stopped resolving, restoring a saved search would fall back to the
        // first entry — `m` — turning 500 nm into 500 metres.
        assert_eq!(canonical_spectral_unit("Angstrom"), Some("Å"));
        assert_eq!(canonical_spectral_unit("um"), Some("µm"));
        assert_eq!(canonical_spectral_unit("micron"), Some("µm"));
        assert_eq!(canonical_spectral_unit("nm"), Some("nm"));
        // Case and stray whitespace are not a reason to lose the unit either.
        assert_eq!(canonical_spectral_unit(" GHZ "), Some("GHz"));
        assert_eq!(canonical_spectral_unit("kev"), Some("keV"));
    }

    #[test]
    fn the_alternate_unicode_forms_resolve_to_the_same_unit() {
        // µ (micro sign) vs μ (Greek mu); Å (Latin) vs Å (Angstrom sign).
        assert_eq!(canonical_spectral_unit("\u{00b5}m"), Some("µm"));
        assert_eq!(canonical_spectral_unit("\u{03bc}m"), Some("µm"));
        assert_eq!(canonical_spectral_unit("\u{00c5}"), Some("Å"));
        assert_eq!(canonical_spectral_unit("\u{212b}"), Some("Å"));
        // And each of them converts, not just resolves.
        assert!(to_metres(1.0, "\u{03bc}m").is_some());
        assert!(to_metres(1.0, "\u{212b}").is_some());
    }

    #[test]
    fn an_unknown_unit_resolves_to_nothing_rather_than_the_first_entry() {
        assert_eq!(canonical_spectral_unit("parsec"), None);
        assert_eq!(canonical_spectral_unit(""), None);
    }

    #[test]
    fn nm_to_metres() {
        let m = to_metres(500.0, "nm").unwrap();
        assert!((m - 5e-7).abs() < 1e-15);
    }

    #[test]
    fn angstrom_to_metres() {
        let m = to_metres(5000.0, "Angstrom").unwrap();
        assert!((m - 5e-7).abs() < 1e-15);
    }

    #[test]
    fn ghz_to_metres() {
        // 1 GHz ~= 0.3 m
        let m = to_metres(1.0, "GHz").unwrap();
        assert!((m - 0.299_792_458).abs() < 0.001);
    }

    #[test]
    fn ev_to_metres() {
        // 1 eV ~= 1.24e-6 m
        let m = to_metres(1.0, "eV").unwrap();
        assert!((m - 1.2398e-6).abs() < 1e-9);
    }

    #[test]
    fn seconds_conversion() {
        assert_eq!(to_seconds(1.0, "h"), Some(3600.0));
        assert_eq!(to_seconds(1.0, "d"), Some(86400.0));
    }

    #[test]
    fn degrees_conversion() {
        let d = to_degrees(1.0, "arcsec").unwrap();
        assert!((d - 1.0 / 3600.0).abs() < 1e-12);
    }

    #[test]
    fn inverse_units() {
        assert!(is_inverse_unit("GHz"));
        assert!(is_inverse_unit("eV"));
        assert!(!is_inverse_unit("nm"));
    }
}
