//! The Search form's relative date windows — one table for the wire value, the
//! label a person reads, and the window each one actually means.
//!
//! Three facts used to live in three files: the dropdown's items in the page,
//! the MCP schema's enum in the tool, and the window each preset selects in the
//! ADQL builder. Keeping them in step was a matter of remembering.
//!
//! The wire value and the label are deliberately different strings. Verbinal
//! shipped the label AS the value ("Last 24 hours"), which is neither the
//! reference's spelling (`Last24h`) nor the macOS app's (`PAST_24_HOURS`), so an
//! agent that had read either app's schema sent a value we did not recognise —
//! and, before the choice validation landed, got no date constraint at all
//! rather than an error. The value now matches the reference; every spelling any
//! of the three has ever used is still accepted on input, because saved searches
//! hold the old one.

use chrono::{DateTime, Duration, Months, Utc};

/// One relative date window.
pub struct DatePreset {
    /// What crosses the MCP wire and is persisted inside a saved search.
    pub value: &'static str,
    /// What the dropdown shows. Empty for "no preset", which renders as a blank
    /// first entry.
    pub label: &'static str,
    /// Older spellings that must still resolve — ours, and the macOS app's.
    aliases: &'static [&'static str],
}

pub const DATE_PRESETS: [DatePreset; 4] = [
    DatePreset {
        value: "",
        label: "",
        aliases: &[],
    },
    DatePreset {
        value: "Last24h",
        label: "Last 24 hours",
        aliases: &["Last 24 hours", "PAST_24_HOURS"],
    },
    DatePreset {
        value: "LastWeek",
        label: "Last week",
        aliases: &["Last week", "PAST_WEEK"],
    },
    DatePreset {
        value: "LastMonth",
        label: "Last month",
        aliases: &["Last month", "PAST_MONTH"],
    },
];

/// The wire values, in dropdown order — the MCP schema's enum.
pub const VALUES: [&str; 4] = [
    DATE_PRESETS[0].value,
    DATE_PRESETS[1].value,
    DATE_PRESETS[2].value,
    DATE_PRESETS[3].value,
];

/// The labels, in dropdown order — what the widget is built from.
pub const LABELS: [&str; 4] = [
    DATE_PRESETS[0].label,
    DATE_PRESETS[1].label,
    DATE_PRESETS[2].label,
    DATE_PRESETS[3].label,
];

/// The dropdown position for any accepted spelling of `value`.
///
/// The widget shows [`LABELS`] and the wire carries [`VALUES`]; going through
/// here is what keeps a stored value landing on the row that means it, instead
/// of falling back to position 0 — which reads as "no date constraint".
pub fn position(value: &str) -> Option<u32> {
    let canonical = canonical(value)?;
    VALUES
        .iter()
        .position(|v| *v == canonical)
        .map(|i| i as u32)
}

/// Resolve any spelling we have ever used or advertised to its wire value.
///
/// Case-insensitive, matching the reference's `OrdinalIgnoreCase` choice check.
/// `None` means the caller named a preset that does not exist — which is an
/// error to report, never a reason to fall back to "no preset".
pub fn canonical(value: &str) -> Option<&'static str> {
    let needle = value.trim();
    DATE_PRESETS
        .iter()
        .find(|preset| {
            preset.value.eq_ignore_ascii_case(needle)
                || preset
                    .aliases
                    .iter()
                    .any(|alias| alias.eq_ignore_ascii_case(needle))
        })
        .map(|preset| preset.value)
}

/// Where the window for `value` starts, relative to `now`.
///
/// "Last month" steps back a CALENDAR month, as both the Windows
/// (`now.AddMonths(-1)`) and macOS (`byAdding: .month, value: -1`) apps do. A
/// fixed 30 days — what this used to be — searches a window up to three days
/// different from the one the other two apps search for the same preset.
///
/// `None` when `value` names no preset, which is how the ADQL builder knows to
/// fall through to the observation-date field.
pub fn window_start(value: &str, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    match canonical(value)? {
        "Last24h" => Some(now - Duration::days(1)),
        "LastWeek" => Some(now - Duration::days(7)),
        "LastMonth" => now.checked_sub_months(Months::new(1)),
        // "" — no preset selected.
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(y: i32, m: u32, d: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, 12, 0, 0).unwrap()
    }

    #[test]
    fn the_wire_values_are_the_references() {
        // Parity is the point: an agent written against CanfarDesktop sends
        // these exact strings.
        assert_eq!(VALUES, ["", "Last24h", "LastWeek", "LastMonth"]);
    }

    #[test]
    fn the_labels_are_what_a_person_reads() {
        assert_eq!(LABELS, ["", "Last 24 hours", "Last week", "Last month"]);
    }

    #[test]
    fn our_old_values_still_resolve() {
        // Every saved search on disk today holds the label as its value.
        assert_eq!(canonical("Last 24 hours"), Some("Last24h"));
        assert_eq!(canonical("Last week"), Some("LastWeek"));
        assert_eq!(canonical("Last month"), Some("LastMonth"));
    }

    #[test]
    fn the_macos_spellings_resolve_too() {
        assert_eq!(canonical("PAST_24_HOURS"), Some("Last24h"));
        assert_eq!(canonical("PAST_WEEK"), Some("LastWeek"));
        assert_eq!(canonical("PAST_MONTH"), Some("LastMonth"));
    }

    #[test]
    fn a_preset_that_does_not_exist_is_not_silently_none() {
        // The caller must be able to tell "no preset" from "I misspelled it";
        // conflating them is how `LastYear` used to run an unconstrained search.
        assert_eq!(canonical("LastYear"), None);
        assert_eq!(canonical(""), Some(""));
    }

    #[test]
    fn matching_ignores_case_like_the_reference() {
        assert_eq!(canonical("last24h"), Some("Last24h"));
        assert_eq!(canonical("LASTWEEK"), Some("LastWeek"));
    }

    #[test]
    fn a_month_back_is_a_calendar_month() {
        // 31 days from 31 March, 30 from 15 May — this is exactly where a fixed
        // 30-day window diverged from both references.
        assert_eq!(
            window_start("LastMonth", at(2026, 3, 31)),
            Some(at(2026, 2, 28))
        );
        assert_eq!(
            window_start("LastMonth", at(2026, 5, 15)),
            Some(at(2026, 4, 15))
        );
    }

    #[test]
    fn the_shorter_windows_are_exact() {
        assert_eq!(
            window_start("Last24h", at(2026, 3, 2)),
            Some(at(2026, 3, 1))
        );
        assert_eq!(
            window_start("LastWeek", at(2026, 3, 8)),
            Some(at(2026, 3, 1))
        );
    }

    #[test]
    fn no_preset_selects_no_window() {
        assert_eq!(window_start("", at(2026, 3, 8)), None);
        assert_eq!(window_start("LastYear", at(2026, 3, 8)), None);
    }

    #[test]
    fn every_offered_preset_resolves_to_a_window() {
        // The dropdown, the schema and the query builder all read this table. An
        // entry with no window would be offered, selected, and then ignored.
        for preset in DATE_PRESETS.iter().filter(|p| !p.value.is_empty()) {
            assert!(
                window_start(preset.value, at(2026, 3, 8)).is_some(),
                "`{}` is offered but selects no window",
                preset.value
            );
        }
    }

    #[test]
    fn every_label_is_reachable_from_its_value() {
        // The dropdown shows labels and the wire carries values; the two are
        // decoded by POSITION, so the tables must stay the same length and order.
        assert_eq!(VALUES.len(), LABELS.len());
        for (i, preset) in DATE_PRESETS.iter().enumerate() {
            assert_eq!(VALUES[i], preset.value);
            assert_eq!(LABELS[i], preset.label);
            assert_eq!(position(preset.value), Some(i as u32));
        }
        // Every alias lands on its own row, not on row 0.
        assert_eq!(position("Last 24 hours"), position("Last24h"));
        assert_eq!(position("PAST_MONTH"), position("LastMonth"));
        assert_eq!(position("LastYear"), None);
    }
}
