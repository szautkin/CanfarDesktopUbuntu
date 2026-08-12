//! Friendly formatting helpers for CAOM2 values shown in the observation detail
//! viewer. Port of `Helpers/Caom2Format.cs`.
//!
//! Numbers are trimmed of trailing zeros (matching the C# "0.#"/"0.###"/… format
//! strings) and missing values render as an em dash.

use chrono::{Duration, TimeZone, Utc};

/// Em dash shown for "no value".
pub const DASH: &str = "\u{2014}";

/// Format a float with up to `decimals` places, trimming trailing zeros and any
/// dangling decimal point (mirrors C# "0.#"-style formats).
fn trim_num(v: f64, decimals: usize) -> String {
    let s = format!("{:.*}", decimals, v);
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    }
}

/// A string, or the dash when empty/whitespace.
pub fn text(s: Option<&str>) -> String {
    match s {
        Some(v) if !v.trim().is_empty() => v.to_string(),
        _ => DASH.to_string(),
    }
}

/// Human-readable byte size (B/KB/MB/GB/TB).
pub fn bytes(bytes: Option<u64>) -> String {
    let Some(b) = bytes else {
        return DASH.to_string();
    };
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut size = b as f64;
    let mut u = 0;
    while size >= 1024.0 && u < UNITS.len() - 1 {
        size /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{} {}", b, UNITS[u])
    } else {
        format!("{} {}", trim_num(size, 1), UNITS[u])
    }
}

/// Wavelength in metres, rendered as friendly nm/µm/mm/m.
pub fn wavelength(metres: Option<f64>) -> String {
    match metres {
        Some(m) if m > 0.0 && m.is_finite() => {
            if m < 1e-6 {
                format!("{} nm", trim_num(m * 1e9, 3))
            } else if m < 1e-3 {
                format!("{} \u{00b5}m", trim_num(m * 1e6, 3))
            } else if m < 1.0 {
                format!("{} mm", trim_num(m * 1e3, 3))
            } else {
                format!("{} m", trim_num(m, 3))
            }
        }
        _ => DASH.to_string(),
    }
}

/// A wavelength range `lower – upper`, or the dash when both bounds are absent.
pub fn wavelength_range(lower: Option<f64>, upper: Option<f64>) -> String {
    if lower.is_none() && upper.is_none() {
        DASH.to_string()
    } else {
        format!("{} \u{2013} {}", wavelength(lower), wavelength(upper))
    }
}

/// MJD (epoch 1858-11-17 UTC) → calendar UTC string `YYYY-MM-DD HH:MM UTC`.
pub fn mjd_to_date(mjd: Option<f64>) -> String {
    match mjd {
        Some(v) if v.is_finite() => {
            let epoch = Utc.with_ymd_and_hms(1858, 11, 17, 0, 0, 0).unwrap();
            let ms = (v * 86_400_000.0).round() as i64;
            let dt = epoch + Duration::milliseconds(ms);
            dt.format("%Y-%m-%d %H:%M UTC").to_string()
        }
        _ => DASH.to_string(),
    }
}

/// Degrees, up to 6 decimals, with a `°` suffix.
pub fn degrees(d: Option<f64>) -> String {
    match d {
        Some(v) if v.is_finite() => format!("{}\u{00b0}", trim_num(v, 6)),
        _ => DASH.to_string(),
    }
}

/// Seconds, up to 2 decimals, with an `s` suffix.
pub fn seconds(s: Option<f64>) -> String {
    match s {
        Some(v) if v.is_finite() => format!("{} s", trim_num(v, 2)),
        _ => DASH.to_string(),
    }
}

/// A plain number, up to 4 decimals (`Caom2Format.Number`).
pub fn number(d: Option<f64>) -> String {
    match d {
        Some(v) if v.is_finite() => trim_num(v, 4),
        _ => DASH.to_string(),
    }
}

/// Yes / No / dash (`Caom2Format.Bool`).
pub fn yes_no(b: Option<bool>) -> String {
    match b {
        Some(true) => crate::tr_en!("Yes").to_string(),
        Some(false) => crate::tr_en!("No").to_string(),
        None => DASH.to_string(),
    }
}

/// An ISO-8601 timestamp reduced to its calendar date (`Caom2Format.Date`).
///
/// Tolerant by design: CAOM2 dates arrive as free text from a dozen archives,
/// and text we cannot parse is shown as it came rather than replaced by a dash —
/// an unparsed date is still information; a dash is not.
pub fn date(s: Option<&str>) -> String {
    match s {
        Some(v) if !v.trim().is_empty() => {
            let t = v.trim();
            let head = t.get(..10).unwrap_or(t);
            match chrono::NaiveDate::parse_from_str(head, "%Y-%m-%d") {
                Ok(d) => d.format("%Y-%m-%d").to_string(),
                Err(_) => t.to_string(),
            }
        }
        _ => DASH.to_string(),
    }
}

/// A keyword list joined for display, or the dash when empty.
pub fn keywords(keywords: &[String]) -> String {
    if keywords.is_empty() {
        DASH.to_string()
    } else {
        keywords.join(", ")
    }
}

/// Last path segment of a `cadc:`/`vos:` artifact URI — the file name
/// (`Caom2Format.ArtifactFileName`).
pub fn artifact_file_name(uri: &str) -> String {
    match uri.rfind('/') {
        Some(i) if i + 1 < uri.len() => uri[i + 1..].to_string(),
        _ => uri.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_uses_dash_for_empty() {
        assert_eq!(text(Some("hello")), "hello");
        assert_eq!(text(Some("   ")), DASH);
        assert_eq!(text(None), DASH);
    }

    #[test]
    fn bytes_scale_and_trim() {
        assert_eq!(bytes(None), DASH);
        assert_eq!(bytes(Some(512)), "512 B");
        assert_eq!(bytes(Some(1024)), "1 KB");
        assert_eq!(bytes(Some(1536)), "1.5 KB");
        assert_eq!(bytes(Some(1_048_576)), "1 MB");
    }

    #[test]
    fn wavelength_picks_unit() {
        assert_eq!(wavelength(None), DASH);
        assert_eq!(wavelength(Some(0.0)), DASH);
        assert_eq!(wavelength(Some(5e-7)), "500 nm");
        assert_eq!(wavelength(Some(2e-6)), "2 \u{00b5}m");
        // 5e-4 m < 1e-3 → still µm (matches C# thresholds).
        assert_eq!(wavelength(Some(5e-4)), "500 \u{00b5}m");
        // 5e-3 m is in [1e-3, 1.0) → mm.
        assert_eq!(wavelength(Some(5e-3)), "5 mm");
        assert_eq!(wavelength(Some(2.0)), "2 m");
    }

    #[test]
    fn wavelength_range_formats() {
        assert_eq!(wavelength_range(None, None), DASH);
        assert_eq!(
            wavelength_range(Some(3.5e-7), Some(6e-7)),
            "350 nm \u{2013} 600 nm"
        );
    }

    #[test]
    fn mjd_epoch_and_value() {
        assert_eq!(mjd_to_date(None), DASH);
        assert_eq!(mjd_to_date(Some(0.0)), "1858-11-17 00:00 UTC");
        // MJD 56000.5 = 2012-03-14 12:00 UTC.
        assert_eq!(mjd_to_date(Some(56000.5)), "2012-03-14 12:00 UTC");
    }

    #[test]
    fn degrees_and_seconds() {
        assert_eq!(degrees(None), DASH);
        assert_eq!(degrees(Some(10.5)), "10.5\u{00b0}");
        assert_eq!(seconds(Some(120.0)), "120 s");
        assert_eq!(seconds(Some(0.25)), "0.25 s");
    }
}

#[cfg(test)]
mod added_tests {
    use super::*;

    #[test]
    fn a_number_trims_to_four_places() {
        assert_eq!(number(Some(1.5)), "1.5");
        assert_eq!(number(Some(1.0)), "1");
        assert_eq!(number(Some(0.123456)), "0.1235");
        assert_eq!(number(None), DASH);
        assert_eq!(number(Some(f64::NAN)), DASH);
    }

    #[test]
    fn a_missing_flag_is_not_a_no() {
        // "No" is a claim about the observation; the dash says we were not told.
        assert_eq!(yes_no(Some(true)), "Yes");
        assert_eq!(yes_no(Some(false)), "No");
        assert_eq!(yes_no(None), DASH);
    }

    #[test]
    fn a_date_keeps_text_it_cannot_parse() {
        assert_eq!(date(Some("2026-03-08T12:00:00Z")), "2026-03-08");
        assert_eq!(date(Some("2026-03-08")), "2026-03-08");
        // Unparseable, but still information — better than a dash.
        assert_eq!(date(Some("early 2026")), "early 2026");
        assert_eq!(date(Some("  ")), DASH);
        assert_eq!(date(None), DASH);
    }

    #[test]
    fn keywords_join_or_dash() {
        assert_eq!(keywords(&["a".to_string(), "b".to_string()]), "a, b");
        assert_eq!(keywords(&[]), DASH);
    }

    #[test]
    fn an_artifact_uri_reduces_to_its_file_name() {
        assert_eq!(
            artifact_file_name("cadc:CFHT/1234567p.fits.fz"),
            "1234567p.fits.fz"
        );
        assert_eq!(artifact_file_name("vos:user/dir/x.fits"), "x.fits");
        // No slash, or a trailing one: show what we were given rather than "".
        assert_eq!(artifact_file_name("1234567p.fits"), "1234567p.fits");
        assert_eq!(artifact_file_name("cadc:CFHT/"), "cadc:CFHT/");
        assert_eq!(artifact_file_name(""), "");
    }
}
