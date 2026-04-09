/// Unit conversion utilities matching the Windows implementation.
const SPEED_OF_LIGHT: f64 = 299_792_458.0; // m/s
const PLANCK_CONSTANT: f64 = 6.626_070_15e-34; // J*s
const EV_TO_JOULES: f64 = 1.602_176_634e-19; // J/eV

/// Convert a wavelength/frequency/energy value to metres.
pub fn to_metres(value: f64, unit: &str) -> Option<f64> {
    if value <= 0.0 {
        return None;
    }
    let u = unit
        .to_lowercase()
        .replace('\u{00b5}', "u")
        .replace('\u{00c5}', "a");
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
