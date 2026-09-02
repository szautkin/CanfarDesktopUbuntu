//! Sexagesimal celestial-coordinate parsing/formatting (RA in hours, Dec in degrees).
//!
//! Port of `CanfarDesktop/Helpers/Sexagesimal.cs`. Accepts `':'` or whitespace as
//! separators (e.g. `"10:00:00"`, `"10 00 00"`, `"-30:15:00"`) and round-trips
//! decimal degrees back to sexagesimal 1-to-1 with the reference implementation.

/// Parse sexagesimal RA (`HH:MM:SS` / `HH MM SS`) to decimal degrees.
///
/// Validates `h in [0,24)`, `m in [0,60)`, `s in [0,60)`. Requires at least two
/// components; returns `None` for a bare number (that is decimal degrees, parsed
/// elsewhere) or malformed input.
pub fn parse_ra(input: &str) -> Option<f64> {
    let parts = split_components(input);
    if parts.len() < 2 {
        return None;
    }
    let h = parts[0].parse::<f64>().ok()?;
    let m = lenient(parts.get(1));
    let s = lenient(parts.get(2));
    if !(0.0..24.0).contains(&h) || !(0.0..60.0).contains(&m) || !(0.0..60.0).contains(&s) {
        return None;
    }
    Some((h + m / 60.0 + s / 3600.0) * 15.0) // hours → degrees
}

/// Parse sexagesimal Dec (`±DD:MM:SS` / `±DD MM SS`) to decimal degrees.
///
/// Validates `d in [0,90]`, `m in [0,60)`, `s in [0,60)`. Requires at least two
/// components; returns `None` otherwise.
pub fn parse_dec(input: &str) -> Option<f64> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    let sign = if trimmed.starts_with('-') { -1.0 } else { 1.0 };
    let cleaned: String = trimmed.chars().filter(|&c| c != '+' && c != '-').collect();
    let parts = split_components(&cleaned);
    if parts.len() < 2 {
        return None;
    }
    let d = parts[0].parse::<f64>().ok()?;
    let m = lenient(parts.get(1));
    let s = lenient(parts.get(2));
    if !(0.0..=90.0).contains(&d) || !(0.0..60.0).contains(&m) || !(0.0..60.0).contains(&s) {
        return None;
    }
    Some(sign * (d + m / 60.0 + s / 3600.0))
}

// ── Formatting (decimal degrees → sexagesimal) ──────────────────────────────

/// Format decimal degrees as sexagesimal RA `HH:MM:SS.cc` (hours, 2-decimal
/// seconds, no sign). Integer-centisecond arithmetic wrapped to `[0,24)`h so
/// `359.999999°` → `23:59:59.99`, never `24:00:00.00`.
pub fn format_ra(deg: f64) -> String {
    let mut hours = (deg / 15.0) % 24.0;
    if hours < 0.0 {
        hours += 24.0;
    }
    const DAY_CS: i64 = 24 * 3600 * 100;
    let mut total_cs = (hours * 3600.0 * 100.0).round() as i64; // round: half away from zero
    total_cs = ((total_cs % DAY_CS) + DAY_CS) % DAY_CS;
    let h = total_cs / 360_000;
    let m = (total_cs / 6_000) % 60;
    let s = (total_cs / 100) % 60;
    let cs = total_cs % 100;
    format!("{:02}:{:02}:{:02}.{:02}", h, m, s, cs)
}

/// Format decimal degrees as sexagesimal Dec `±DD:MM:SS.d` (always-signed,
/// 1-decimal seconds). Integer deci-arcsecond arithmetic.
pub fn format_dec(deg: f64) -> String {
    let sign = if deg < 0.0 { "-" } else { "+" };
    let total_ds = (deg.abs() * 3600.0 * 10.0).round() as i64;
    let d = total_ds / 36_000;
    let m = (total_ds / 600) % 60;
    let s = (total_ds / 10) % 60;
    let ds = total_ds % 10;
    format!("{}{:02}:{:02}:{:02}.{}", sign, d, m, s, ds)
}

// ── Compact forms (ported from CubeWcs.cs) ─────────────────────────────────
//
// Whole seconds rather than the centi- and deci-second precision above: these
// label a plot axis and a figure footer, where the long form does not fit. They
// lived in `cube_axes` and again, verbatim, in `cube_export` — whose copy was
// justified in a comment as being there "so the footer ranges match the live
// axis captions verbatim", which is the one thing copying them cannot ensure.

/// `raDeg` → `"HH:MM:SS"` (RA folded into [0,24h)).
pub fn format_ra_short(ra_deg: f64) -> String {
    let mut ra = ra_deg / 15.0;
    ra %= 24.0;
    if ra < 0.0 {
        ra += 24.0;
    }
    let mut h = ra as i32;
    let mut m = ((ra - h as f64) * 60.0) as i32;
    let mut s = ((ra - h as f64 - m as f64 / 60.0) * 3600.0).round() as i32;
    if s == 60 {
        s = 0;
        m += 1;
    }
    if m == 60 {
        m = 0;
        h = (h + 1) % 24;
    }
    format!("{:02}:{:02}:{:02}", h, m, s)
}

/// `decDeg` → `"±DD:MM:SS"` (uses U+2212 MINUS SIGN for negatives, as the C#).
pub fn format_dec_short(dec_deg: f64) -> String {
    let sign = if dec_deg >= 0.0 { "+" } else { "\u{2212}" };
    let d = dec_deg.abs();
    let mut dd = d as i32;
    let mut m = ((d - dd as f64) * 60.0) as i32;
    let mut s = ((d - dd as f64 - m as f64 / 60.0) * 3600.0).round() as i32;
    if s == 60 {
        s = 0;
        m += 1;
    }
    if m == 60 {
        m = 0;
        dd += 1;
    }
    format!("{}{:02}:{:02}:{:02}", sign, dd, m, s)
}

/// Decimal degrees to 3 places with a trailing degree sign.
pub fn format_deg(deg: f64) -> String {
    format!("{:.3}\u{00B0}", deg)
}

/// Fold an angle into [0, 360).
pub fn wrap360(v: f64) -> f64 {
    ((v % 360.0) + 360.0) % 360.0
}

/// RA formatter over a raw (decimal-degrees) cell string; returns the trimmed
/// raw unchanged when it is not a finite number.
pub fn format_ra_str(raw: &str) -> String {
    let trimmed = raw.trim();
    match trimmed.parse::<f64>() {
        Ok(v) if v.is_finite() => format_ra(v),
        _ => trimmed.to_string(),
    }
}

/// Dec formatter over a raw (decimal-degrees) cell string; passthrough when the
/// value is not finite OR falls outside `[-90, 90]`.
pub fn format_dec_str(raw: &str) -> String {
    let trimmed = raw.trim();
    match trimmed.parse::<f64>() {
        Ok(v) if v.is_finite() && (-90.0..=90.0).contains(&v) => format_dec(v),
        _ => trimmed.to_string(),
    }
}

fn split_components(input: &str) -> Vec<&str> {
    input
        .trim()
        .split([':', ' '])
        .filter(|p| !p.is_empty())
        .collect()
}

/// Lenient component parse (0.0 on absent/malformed), matching the reference
/// `Parse` used for the minute/second fields.
fn lenient(part: Option<&&str>) -> f64 {
    part.and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ra_colon_and_space() {
        let a = parse_ra("10:41:00").unwrap();
        let b = parse_ra("10 41 00").unwrap();
        assert!((a - b).abs() < 1e-9);
        assert!((a - 160.25).abs() < 1e-9);
    }

    #[test]
    fn parse_dec_signed() {
        let p = parse_dec("+41:16:00").unwrap();
        assert!((p - (41.0 + 16.0 / 60.0)).abs() < 1e-9);
        let n = parse_dec("-30:15:00").unwrap();
        assert!((n - -(30.0 + 15.0 / 60.0)).abs() < 1e-9);
    }

    #[test]
    fn parse_rejects_bare_number_and_out_of_range() {
        assert!(parse_ra("10").is_none()); // decimal degrees handled elsewhere
        assert!(parse_ra("25:00:00").is_none()); // hours >= 24
        assert!(parse_dec("91:00:00").is_none()); // dec > 90
        assert!(parse_ra("10:70:00").is_none()); // minutes >= 60
    }

    #[test]
    fn ra_deg_to_hms_and_back() {
        // 150° == 10h exactly
        assert_eq!(format_ra(150.0), "10:00:00.00");
        let back = parse_ra("10:00:00").unwrap();
        assert!((back - 150.0).abs() < 1e-9);
    }

    #[test]
    fn ra_wraps_below_24h() {
        // A value just under a full day renders sub-second, never "24:...".
        assert_eq!(format_ra(359.99996), "23:59:59.99");
        // A value that rounds up to a whole day wraps to zero (not 24:00:00.00).
        assert_eq!(format_ra(359.999999), "00:00:00.00");
        assert_eq!(format_ra(360.0), "00:00:00.00");
        assert!(!format_ra(359.999999).starts_with("24"));
    }

    #[test]
    fn dec_format_sign_always_shown() {
        assert_eq!(format_dec(41.0 + 16.0 / 60.0), "+41:16:00.0");
        assert_eq!(format_dec(-(30.0 + 15.0 / 60.0)), "-30:15:00.0");
        assert_eq!(format_dec(0.0), "+00:00:00.0");
    }

    #[test]
    fn sexagesimal_round_trip() {
        for &deg in &[0.0_f64, 10.5, 83.63321, 160.25, 299.9, 359.5] {
            let s = format_ra(deg);
            let parsed = parse_ra(&s).unwrap();
            assert!(
                (parsed - deg).abs() < 0.01,
                "RA round-trip {} -> {}",
                deg,
                s
            );
        }
        for &deg in &[-89.5_f64, -41.267, 0.0, 22.014, 41.267, 89.9] {
            let s = format_dec(deg);
            let parsed = parse_dec(&s).unwrap();
            assert!(
                (parsed - deg).abs() < 0.01,
                "Dec round-trip {} -> {}",
                deg,
                s
            );
        }
    }

    #[test]
    fn format_str_passthrough() {
        assert_eq!(format_ra_str("83.633"), format_ra(83.633));
        assert_eq!(format_ra_str("not-a-number"), "not-a-number");
        assert_eq!(format_dec_str("22.014"), format_dec(22.014));
        assert_eq!(format_dec_str("120.0"), "120.0"); // out of [-90,90] → passthrough
    }

    /// The compact forms round to whole seconds and carry no half-minutes.
    #[test]
    fn a_compact_form_is_the_long_one_without_the_fraction() {
        assert_eq!(format_ra_short(180.0), "12:00:00");
        assert_eq!(format_ra_short(0.0), "00:00:00");
        assert_eq!(format_ra_short(-15.0), "23:00:00"); // wraps into [0,24h)
        assert_eq!(format_dec_short(45.0), "+45:00:00");
        assert_eq!(format_dec_short(-30.5), "\u{2212}30:30:00");
        assert_eq!(format_deg(12.0), "12.000\u{00B0}");
        assert!((wrap360(-10.0) - 350.0).abs() < 1e-9);
        assert!((wrap360(370.0) - 10.0).abs() < 1e-9);

        // The compact form agrees with the long one about where the sky is; it
        // just says less. Not a prefix of it — the compact form ROUNDS to the
        // second where the long one carries the remainder, so 05:34:31.97 reads
        // back as 05:34:32 — but within the half-second that rounding can move
        // it. A drift wider than that would put a figure's footer and its axis
        // on different coordinates.
        const HALF_SECOND_OF_RA: f64 = 0.5 * 15.0 / 3600.0;
        for deg in [0.0_f64, 10.5, 83.63321, 180.0, 299.9] {
            let (short, long) = (format_ra_short(deg), format_ra(deg));
            let (a, b) = (parse_ra(&short).unwrap(), parse_ra(&long).unwrap());
            assert!(
                (a - b).abs() <= HALF_SECOND_OF_RA + 1e-9,
                "{short} and {long} are more than half a second apart"
            );
        }
    }

    /// One implementation, and no file grows a private copy of it again.
    ///
    /// Three did: `cube_axes` and `cube_export` each held all four of these
    /// verbatim, the second justified in a comment as being there "so the
    /// footer ranges match the live axis captions verbatim" — which is the one
    /// guarantee a copy cannot give. The copies compiled, passed, and drifted
    /// only when someone edited one of them.
    #[test]
    fn nothing_else_defines_these() {
        let mine = std::path::Path::new("helpers/sexagesimal.rs");
        let mut elsewhere: Vec<String> = Vec::new();
        for (path, text) in crate::testing::rust_sources() {
            if path.ends_with(mine) {
                continue;
            }
            let code = crate::testing::without_comments(crate::testing::code(&text));
            for name in [
                "format_ra_short",
                "format_dec_short",
                "format_deg",
                "wrap360",
            ] {
                if code.contains(&format!("fn {name}(")) {
                    elsewhere.push(format!("{} defines {name}", path.display()));
                }
            }
        }
        assert!(
            elsewhere.is_empty(),
            "a second copy of a coordinate formatter: {elsewhere:#?}"
        );
    }
}
