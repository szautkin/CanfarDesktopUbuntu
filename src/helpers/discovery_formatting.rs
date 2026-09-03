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

/// Total installed packages across every ecosystem (dpkg + rpm + apk + python + R).
///
/// Mirrors `DiscoveryFormatting.PackageCount`. Uses the flat `python` list (not
/// the per-env map) to match the C# `PythonPackages.Count`.
/// Does `name` lead with `needle`, case-insensitively?
///
/// The lead half of package-name relevance, shared so the vocabulary search and
/// the per-image filter rank the same way. Searching "spec" over this cache
/// otherwise surfaces `archspec` and `jsonschema-specifications` ahead of
/// `specutils`: names that merely CONTAIN the term are usually Python plumbing
/// every image carries, which is exactly what does not answer the question.
pub fn leads_with(name: &str, needle: &str) -> bool {
    let needle = needle.trim();
    !needle.is_empty() && name.to_lowercase().starts_with(&needle.to_lowercase())
}

pub fn package_count(m: &ImageManifest) -> usize {
    m.dpkg.len() + m.rpm.len() + m.apk.len() + m.python.len() + m.r_packages.len()
}

/// A failure, in one line: what kind, then the diagnosis without its evidence.
///
/// The one place a discovery failure becomes a subtitle. Both the image list
/// and the discovery dialog render this, and both were building their own —
/// the image list by dropping the WHOLE message in, which since failures
/// started carrying the tail of a job's logs means a subtitle several hundred
/// lines long. The full text is still there, behind
/// [`crate::ui::failure_detail`], where it can scroll.
pub fn failure_summary(category: &str, message: &str) -> String {
    let summary = crate::helpers::job_diagnostics::summary_line(message);
    if summary.is_empty() {
        return category_label(category).to_string();
    }
    format!("{} · {summary}", category_label(category))
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_failure_summary_names_the_kind_and_the_first_line_only() {
        // Failure messages carry the tail of a job's logs and events. A
        // subtitle that took the whole message made the row hundreds of lines
        // tall; one that took only the category said nothing useful.
        let reason = "Manifest fetch failed: no JSON in the logs\n\n\
                      --- job logs ---\nbash: syft: not found";
        assert_eq!(
            failure_summary("ManifestFetchFailed", reason),
            "No manifest · Manifest fetch failed: no JSON in the logs"
        );
    }

    #[test]
    fn a_failure_with_no_message_still_says_what_kind_it_was() {
        assert_eq!(failure_summary("JobTimedOut", "   "), "Timed out");
        assert_eq!(failure_summary("JobTimedOut", ""), "Timed out");
    }
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
        assert_eq!(
            time_ago("2026-06-23T12:00:00Z", "garbage"),
            "2026-06-23T12:00:00Z"
        );
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
        assert_eq!(package_count(&m), (2 + 1) + 3 + 1);
        assert_eq!(package_count(&ImageManifest::default()), 0);
    }
}
