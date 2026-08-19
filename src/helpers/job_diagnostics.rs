//! What a failed job can still tell you, and how much of it to keep.
//!
//! Assembling the reason lives here rather than in either caller: the
//! image-discovery coordinator and the Batch Jobs poller both record failures,
//! and each had grown its own copy of "logs, then events, each trimmed". Two
//! copies means two shapes in one history file the moment either is edited.

/// How much of a job's output to keep with its record.
///
/// Enough for a stack trace or a missing-command message, bounded so a job that
/// printed a megabyte of progress bars cannot bloat the history file.
pub const MAX_REASON_CHARS: usize = 4_000;

/// Trim a diagnosis to something that fits in a record, keeping the END.
///
/// The end is where the error is: a probe that dies prints its setup chatter
/// first and the reason last, so truncating the head loses nothing and
/// truncating the tail loses everything.
pub fn tail(text: &str, limit: usize) -> String {
    let text = text.trim_end();
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= limit {
        return text.to_string();
    }
    let kept: String = chars[chars.len() - limit..].iter().collect();
    // Start at a line boundary so the excerpt does not open mid-word.
    let kept = match kept.find('\n') {
        Some(at) => &kept[at + 1..],
        None => kept.as_str(),
    };
    format!("…\n{kept}")
}

/// A failed job's own account of itself: its logs, then its events.
///
/// Both, because they answer different questions. Events say why a job never
/// ran — the image could not be pulled, there was no quota, a node evicted it.
/// Logs say why it ran and died. Keeping only one leaves half the failures
/// unexplained.
pub fn evidence(logs: &str, events: &str) -> String {
    let half = MAX_REASON_CHARS / 2;
    let mut parts: Vec<String> = Vec::new();
    if !logs.trim().is_empty() {
        parts.push(format!("--- job logs ---\n{}", tail(logs, half)));
    }
    if !events.trim().is_empty() {
        parts.push(format!("--- job events ---\n{}", tail(events, half)));
    }
    if parts.is_empty() {
        // An empty reason renders as a row with no explanation, which reads as
        // "we forgot to look" rather than "there was nothing to see".
        return crate::tr_en!("The job produced no logs or events.").to_string();
    }
    parts.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logs_and_events_are_both_kept() {
        let reason = evidence("Traceback...\nKeyError: 'x'", "Failed to pull image");
        assert!(reason.contains("KeyError"), "{reason}");
        assert!(reason.contains("Failed to pull image"), "{reason}");
        assert!(reason.contains("job logs"), "{reason}");
        assert!(reason.contains("job events"), "{reason}");
    }

    #[test]
    fn either_one_alone_still_produces_a_reason() {
        assert!(evidence("boom", "").contains("boom"));
        assert!(evidence("", "evicted").contains("evicted"));
    }

    #[test]
    fn a_silent_job_says_so_rather_than_recording_nothing() {
        let reason = evidence("", "   ");
        assert!(!reason.trim().is_empty());
        assert!(reason.contains("no logs or events"), "{reason}");
    }

    #[test]
    fn a_flood_of_output_is_trimmed_to_its_end() {
        // A job that printed a megabyte of progress bars must not write a
        // megabyte into the history file — and the end is where the error is.
        let flood = format!("{}\nfatal: out of memory", "progress\n".repeat(50_000));
        let reason = evidence(&flood, "");
        assert!(
            reason.contains("fatal: out of memory"),
            "the error was trimmed away"
        );
        assert!(
            reason.chars().count() <= MAX_REASON_CHARS,
            "{} chars kept",
            reason.chars().count()
        );
    }

    // ── tail ────────────────────────────────────────────────────────────────

    #[test]
    fn a_short_diagnosis_is_kept_whole() {
        assert_eq!(tail("boom", 100), "boom");
    }

    #[test]
    fn a_long_diagnosis_keeps_its_end() {
        let text = format!("{}\nfatal: no such file", "noise\n".repeat(500));
        let kept = tail(&text, 40);
        assert!(kept.contains("fatal: no such file"), "{kept:?}");
        assert!(kept.starts_with('\u{2026}'), "{kept:?}");
        assert!(kept.chars().count() <= 42, "{} chars", kept.chars().count());
    }

    #[test]
    fn an_excerpt_starts_at_a_line_boundary() {
        let text = "aaaaaaaaaa\nbbbbbbbbbb\ncccccccccc";
        assert_eq!(tail(text, 15), "\u{2026}\ncccccccccc");
    }

    #[test]
    fn only_this_module_assembles_a_failure_reason() {
        // Two callers record failures — the image-discovery coordinator and the
        // Batch Jobs poller — and each had grown its own copy of "logs, then
        // events, each trimmed". Two copies means two shapes in one history
        // file the moment either is edited, and the second copy is invisible
        // until someone reads both.
        let owners: Vec<String> = crate::testing::rust_sources()
            .into_iter()
            .filter(|(_, text)| crate::testing::code(text).contains("--- job logs ---"))
            .map(|(path, _)| path.display().to_string())
            .collect();
        assert_eq!(
            owners.len(),
            1,
            "the failure-reason format is written in {} places: {owners:?}",
            owners.len()
        );
        assert!(owners[0].ends_with("job_diagnostics.rs"), "{owners:?}");
    }

    #[test]
    fn a_multibyte_diagnosis_does_not_split_a_character() {
        // Slicing by bytes here would panic on a UTF-8 boundary, and a probe
        // that prints a non-ASCII path is not exotic.
        let text = "\u{3b1}".repeat(200);
        assert!(tail(&text, 50).chars().count() <= 52);
    }
}
