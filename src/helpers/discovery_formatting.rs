//! Pure presentation helpers for image-discovery rows.
//!
//! Port of `Helpers/ImageDiscovery/DiscoveryFormatting.cs`: failure-category
//! pill labels, compact relative time, the "still recovering" grace heuristic,
//! and total package counts. Every function is pure and side-effect free.

use crate::models::image_manifest::ImageManifest;
use chrono::DateTime;

/// Short pill label for a failure category.
///
/// Accepts the canonical category strings emitted by [`DiscoveryOutcome::Failure`]
/// (`JobSubmitFailed`, `JobTimedOut`, `ManifestFetchFailed`, `ManifestParseFailed`,
/// `Cancelled`; anything else -> `"Failed"`). Matching is case- and
/// separator-insensitive, so `job_timed_out`, `job-timed-out` and `JOBTIMEDOUT`
/// all resolve identically. Mirrors the macOS `categoryLabel` /
/// `DiscoveryFormatting.CategoryLabel` switch.
///
/// [`DiscoveryOutcome::Failure`]: crate::models::image_manifest
pub fn category_label(cat: &str) -> &'static str {
    match normalize_category(cat).as_str() {
        "jobsubmitfailed" => "Submit failed",
        "jobtimedout" => "Timed out",
        "manifestfetchfailed" => "No manifest",
        "manifestparsefailed" => "Bad manifest",
        "cancelled" | "canceled" => "Cancelled",
        _ => "Failed",
    }
}

/// Reduce a category token to lowercase alphanumerics so that PascalCase,
/// snake_case and kebab-case spellings all collapse to one form.
fn normalize_category(cat: &str) -> String {
    cat.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Compact relative time (`"just now"`, `"5m ago"`, `"3d ago"`); beyond 14 days
/// it falls back to a short absolute date (`"May 14"`) so the row stays narrow.
///
/// Both arguments are RFC-3339 timestamps. If either fails to parse the raw
/// `rfc3339` input is echoed back unchanged (records may carry pre-existing
/// free-form dates). Mirrors the macOS `timeAgo` / `DiscoveryFormatting.TimeAgo`.
pub fn time_ago(rfc3339: &str, now_rfc3339: &str) -> String {
    let (date, now) = match (
        DateTime::parse_from_rfc3339(rfc3339),
        DateTime::parse_from_rfc3339(now_rfc3339),
    ) {
        (Ok(d), Ok(n)) => (d, n),
        _ => return rfc3339.to_string(),
    };

    let elapsed = now.signed_duration_since(date).num_seconds();
    if elapsed < 30 {
        // Also covers future timestamps / clock skew (negative elapsed).
        "just now".to_string()
    } else if elapsed < 60 {
        format!("{elapsed}s ago")
    } else if elapsed < 3_600 {
        format!("{}m ago", elapsed / 60)
    } else if elapsed < 86_400 {
        format!("{}h ago", elapsed / 3_600)
    } else if elapsed < 14 * 86_400 {
        format!("{}d ago", elapsed / 86_400)
    } else {
        // Keep the original offset, matching the C# `DateTimeOffset.ToString`.
        date.format("%b %-d").to_string()
    }
}

/// True when a timed-out attempt might still be rescued by the coordinator's
/// background grace poll (the manifest may yet land at VOSpace). Conservative
/// 10-minute window from the attempt start, matching the macOS grace budget.
///
/// Both arguments are RFC-3339 timestamps; the category gate (only `JobTimedOut`
/// qualifies) is applied by the caller. Future / clock-skewed `started` values
/// count as still recovering. Unparseable input returns `false`. Mirrors the
/// macOS `isLikelyStillRecovering` / `DiscoveryFormatting.IsLikelyStillRecovering`.
pub fn is_likely_still_recovering(started_rfc3339: &str, now_rfc3339: &str) -> bool {
    match (
        DateTime::parse_from_rfc3339(started_rfc3339),
        DateTime::parse_from_rfc3339(now_rfc3339),
    ) {
        (Ok(started), Ok(now)) => now.signed_duration_since(started).num_seconds() < 10 * 60,
        _ => false,
    }
}

/// Total installed packages across every ecosystem (dpkg + rpm + apk + python + R).
///
/// Mirrors `DiscoveryFormatting.PackageCount`. Uses the flat `python` list (not
/// the per-env map) to match the C# `PythonPackages.Count`.
pub fn package_count(m: &ImageManifest) -> usize {
    m.dpkg.len() + m.rpm.len() + m.apk.len() + m.python.len() + m.r_packages.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::image_manifest::ImageManifest;

    #[test]
    fn category_label_matches_macos() {
        assert_eq!(category_label("JobSubmitFailed"), "Submit failed");
        assert_eq!(category_label("JobTimedOut"), "Timed out");
        assert_eq!(category_label("ManifestFetchFailed"), "No manifest");
        assert_eq!(category_label("ManifestParseFailed"), "Bad manifest");
        assert_eq!(category_label("Cancelled"), "Cancelled");
        assert_eq!(category_label("Unknown"), "Failed");
        assert_eq!(category_label("something-else"), "Failed");
        assert_eq!(category_label(""), "Failed");
    }

    #[test]
    fn category_label_is_case_and_separator_insensitive() {
        assert_eq!(category_label("job_timed_out"), "Timed out");
        assert_eq!(category_label("job-timed-out"), "Timed out");
        assert_eq!(category_label("JOBTIMEDOUT"), "Timed out");
        assert_eq!(category_label("manifest.fetch.failed"), "No manifest");
    }

    #[test]
    fn time_ago_buckets_by_elapsed() {
        let now = "2026-06-23T12:00:00Z";
        assert_eq!(time_ago("2026-06-23T11:59:55Z", now), "just now"); // -5s
        assert_eq!(time_ago("2026-06-23T12:00:10Z", now), "just now"); // future / skew
        assert_eq!(time_ago("2026-06-23T11:59:15Z", now), "45s ago");
        assert_eq!(time_ago("2026-06-23T11:55:00Z", now), "5m ago");
        assert_eq!(time_ago("2026-06-23T09:00:00Z", now), "3h ago");
        assert_eq!(time_ago("2026-06-21T12:00:00Z", now), "2d ago");
    }

    #[test]
    fn time_ago_beyond_two_weeks_falls_back_to_absolute_date() {
        let now = "2026-06-23T12:00:00Z";
        assert_eq!(time_ago("2026-05-14T00:00:00Z", now), "May 14");
    }

    #[test]
    fn time_ago_unparseable_echoes_raw() {
        assert_eq!(time_ago("not-a-date", "2026-06-23T12:00:00Z"), "not-a-date");
        assert_eq!(time_ago("2026-06-23T12:00:00Z", "garbage"), "2026-06-23T12:00:00Z");
    }

    #[test]
    fn is_likely_still_recovering_only_within_ten_minutes() {
        let now = "2026-06-23T12:00:00Z";
        assert!(is_likely_still_recovering("2026-06-23T11:55:00Z", now)); // 5m
        assert!(!is_likely_still_recovering("2026-06-23T11:45:00Z", now)); // 15m
        assert!(!is_likely_still_recovering("2026-06-23T11:50:00Z", now)); // exactly 10m -> false
        assert!(is_likely_still_recovering("2026-06-23T12:05:00Z", now)); // future / skew
        assert!(!is_likely_still_recovering("bad", now)); // unparseable -> false
    }

    #[test]
    fn package_count_sums_every_ecosystem() {
        let m = ImageManifest {
            dpkg: vec!["libc|2.35".into(), "bash|5.1".into()],
            rpm: vec!["glibc|2.34".into()],
            apk: vec![],
            python: vec!["numpy|1.0".into(), "scipy|1.11".into(), "astropy|6".into()],
            r_packages: vec!["ggplot2|3.4".into()],
            ..Default::default()
        };
        assert_eq!(package_count(&m), 2 + 1 + 0 + 3 + 1);
        assert_eq!(package_count(&ImageManifest::default()), 0);
    }
}
