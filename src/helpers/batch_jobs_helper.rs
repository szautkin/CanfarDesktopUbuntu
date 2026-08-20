//! Batch jobs helper — groups CANFAR `headless` sessions by status.
//!
//! Ported from the Windows reference `BatchJobsHelper.GroupByState`. Batch jobs
//! are just CANFAR sessions where `session_type == "headless"`.

use crate::models::job_record::{JobOutcome, JobRecord};
use crate::models::session::Session;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchJobState {
    Pending,
    Running,
    Completed,
    Failed,
}

impl BatchJobState {
    pub fn label(&self) -> &'static str {
        match self {
            BatchJobState::Pending => "Pending",
            BatchJobState::Running => "Running",
            BatchJobState::Completed => "Completed",
            BatchJobState::Failed => "Failed",
        }
    }

    pub fn css_class(&self) -> &'static str {
        match self {
            BatchJobState::Pending => "batch-dot-pending",
            BatchJobState::Running => "batch-dot-running",
            BatchJobState::Completed => "batch-dot-completed",
            BatchJobState::Failed => "batch-dot-failed",
        }
    }

    pub fn from_status(status: &str) -> Self {
        let s = status.to_ascii_lowercase();
        if s == "pending" {
            BatchJobState::Pending
        } else if s == "running" {
            BatchJobState::Running
        } else if s == "succeeded" || s == "completed" {
            BatchJobState::Completed
        } else if s == "failed" || s == "error" {
            BatchJobState::Failed
        } else {
            // Unknown statuses default to Pending
            BatchJobState::Pending
        }
    }
}

#[derive(Debug, Clone, Default, Copy, PartialEq, Eq)]
pub struct BatchJobCounts {
    pub pending: usize,
    pub running: usize,
    pub completed: usize,
    pub failed: usize,
}

impl BatchJobCounts {}

/// How far back a finished job still counts toward the dashboard tiles.
///
/// The tiles answer "what happened lately", and the remembered history is
/// capped by count rather than age — without a window, one bad afternoon would
/// leave Failed reading 12 for as long as those records survive. The History
/// tab shows everything regardless.
pub const RECENT_WINDOW_HOURS: i64 = 24;

/// One batch job as the UI shows it, whether Skaha still has it or only we do.
///
/// CANFAR reaps finished headless jobs and the image-discovery coordinator
/// deletes its own probes within seconds of them finishing, so the live listing
/// almost never contains a completed or failed job. Counting only the listing
/// left two of the four dashboard tiles reading zero permanently — the widget
/// was not empty, it was lying.
#[derive(Debug, Clone)]
pub enum JobEntry {
    /// Skaha still lists it: it can be inspected, and deleted.
    Live(Session),
    /// Only the history has it. Its logs and events are gone, but the reason it
    /// failed was captured while they were not.
    Remembered(JobRecord),
}

impl JobEntry {
    pub fn state(&self) -> BatchJobState {
        match self {
            Self::Live(s) => BatchJobState::from_status(&s.status),
            // The recorded outcome, not the status word: a remembered job may
            // have been last seen as "Terminating", which `from_status` reads
            // as Pending — and a finished job is never pending.
            Self::Remembered(r) => match r.outcome {
                JobOutcome::Succeeded => BatchJobState::Completed,
                JobOutcome::Failed => BatchJobState::Failed,
            },
        }
    }
}

/// Every batch job worth showing: the live headless sessions, plus finished
/// jobs we remember from the last [`RECENT_WINDOW_HOURS`].
///
/// A live session wins over a remembered record of the same id — its status is
/// the fresher of the two. `now` is passed in rather than read so the window is
/// testable.
pub fn merge(sessions: &[Session], history: &[JobRecord], now: &str) -> Vec<JobEntry> {
    let mut entries: Vec<JobEntry> = sessions
        .iter()
        .filter(|s| s.is_headless())
        .cloned()
        .map(JobEntry::Live)
        .collect();

    let live: std::collections::HashSet<&str> = sessions.iter().map(|s| s.id.as_str()).collect();
    for record in history {
        if live.contains(record.id.as_str()) {
            continue;
        }
        if within_window(&record.finished_at, now) {
            entries.push(JobEntry::Remembered(record.clone()));
        }
    }
    entries
}

/// Whether `finished_at` is inside the recent window ending at `now`.
///
/// An unparseable timestamp counts as recent: dropping a job because its date
/// could not be read is a worse answer than showing it.
fn within_window(finished_at: &str, now: &str) -> bool {
    use chrono::DateTime;
    match (
        DateTime::parse_from_rfc3339(finished_at),
        DateTime::parse_from_rfc3339(now),
    ) {
        (Ok(then), Ok(now)) => now.signed_duration_since(then).num_hours() < RECENT_WINDOW_HOURS,
        _ => true,
    }
}

/// Count the entries by state.
pub fn count_by_state(entries: &[JobEntry]) -> BatchJobCounts {
    let mut counts = BatchJobCounts::default();
    for entry in entries {
        match entry.state() {
            BatchJobState::Pending => counts.pending += 1,
            BatchJobState::Running => counts.running += 1,
            BatchJobState::Completed => counts.completed += 1,
            BatchJobState::Failed => counts.failed += 1,
        }
    }
    counts
}

/// The entries in one state.
pub fn of_state(entries: &[JobEntry], state: BatchJobState) -> Vec<JobEntry> {
    entries
        .iter()
        .filter(|e| e.state() == state)
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::job_record::JobOrigin;

    fn sess(id: &str, ty: &str, status: &str) -> Session {
        Session {
            id: id.into(),
            userid: String::new(),
            image: String::new(),
            session_type: ty.into(),
            status: status.into(),
            name: id.into(),
            start_time: String::new(),
            expiry_time: String::new(),
            connect_url: String::new(),
            requested_ram: String::new(),
            requested_cpu_cores: String::new(),
            requested_gpu_cores: String::new(),
            ram_in_use: String::new(),
            cpu_cores_in_use: String::new(),
            is_fixed_resources: true,
        }
    }

    const NOW: &str = "2026-08-20T12:00:00Z";

    fn remembered(id: &str, outcome: JobOutcome, finished_at: &str) -> JobRecord {
        JobRecord {
            id: id.into(),
            name: format!("job-{id}"),
            image: "images.canfar.net/skaha/terminal:1.1.2".into(),
            origin: JobOrigin::ImageProbe,
            outcome,
            status: match outcome {
                JobOutcome::Succeeded => "Succeeded".into(),
                JobOutcome::Failed => "Failed".into(),
            },
            started_at: finished_at.into(),
            finished_at: finished_at.into(),
            failure_reason: None,
            target_image: None,
        }
    }

    #[test]
    fn only_headless_sessions_count() {
        let sessions = vec![
            sess("a", "headless", "Running"),
            sess("b", "headless", "Pending"),
            sess("c", "notebook", "Running"),
            sess("d", "headless", "Succeeded"),
            sess("e", "headless", "Failed"),
            sess("f", "headless", "Running"),
        ];
        let counts = count_by_state(&merge(&sessions, &[], NOW));
        assert_eq!(counts.running, 2);
        assert_eq!(counts.pending, 1);
        assert_eq!(counts.completed, 1);
        assert_eq!(counts.failed, 1);
    }

    #[test]
    fn of_state_selects_headless_jobs_in_that_state() {
        let sessions = vec![
            sess("a", "headless", "Running"),
            sess("b", "notebook", "Running"),
            sess("c", "headless", "Running"),
        ];
        let running = of_state(&merge(&sessions, &[], NOW), BatchJobState::Running);
        assert_eq!(running.len(), 2);
    }

    #[test]
    fn a_finished_job_counts_even_though_skaha_has_forgotten_it() {
        // The reason the tiles existed and read zero: CANFAR reaps finished
        // headless jobs, and the image-discovery coordinator deletes its own
        // probes within seconds. Counting only the live listing meant Completed
        // and Failed were permanently empty.
        let history = vec![
            remembered("x", JobOutcome::Failed, "2026-08-20T11:00:00Z"),
            remembered("y", JobOutcome::Succeeded, "2026-08-20T11:30:00Z"),
        ];
        let counts = count_by_state(&merge(&[], &history, NOW));
        assert_eq!(counts.failed, 1);
        assert_eq!(counts.completed, 1);
    }

    #[test]
    fn a_job_that_is_both_live_and_remembered_counts_once() {
        // The poller records a job the moment it sees it finish, while the
        // listing may still carry it. Counting both would show two.
        let sessions = vec![sess("a", "headless", "Failed")];
        let history = vec![remembered("a", JobOutcome::Failed, "2026-08-20T11:59:00Z")];
        let entries = merge(&sessions, &history, NOW);
        assert_eq!(entries.len(), 1);
        assert!(
            matches!(entries[0], JobEntry::Live(_)),
            "the live status is fresher"
        );
    }

    #[test]
    fn a_job_older_than_the_window_is_not_counted() {
        // The tiles answer "what happened lately". The history is capped by
        // count, not age, so without a window one bad afternoon would leave
        // Failed reading twelve indefinitely.
        let old = remembered("x", JobOutcome::Failed, "2026-08-18T12:00:00Z");
        assert!(merge(&[], &[old], NOW).is_empty());
    }

    #[test]
    fn a_remembered_job_uses_its_outcome_not_its_last_status_word() {
        // A job last seen as "Terminating" reads as Pending through
        // `from_status`, and a finished job is never pending.
        let mut record = remembered("x", JobOutcome::Failed, "2026-08-20T11:00:00Z");
        record.status = "Terminating".into();
        assert_eq!(merge(&[], &[record], NOW)[0].state(), BatchJobState::Failed);
    }

    #[test]
    fn a_real_history_file_populates_the_tiles() {
        // `tests/fixtures/job_history_sample.json` is a real file this app
        // wrote: sixteen image probes over one morning, fourteen of them
        // failures — and the Batch Jobs card showed none of them, because
        // Skaha no longer listed a single one.
        //
        // It also carries the timestamp format the app really writes
        // (`+00:00`, with nanoseconds), which a hand-written fixture would not.
        const SAMPLE: &str = include_str!("../../tests/fixtures/job_history_sample.json");
        let history: Vec<JobRecord> = serde_json::from_str(SAMPLE).expect("parsed");
        assert_eq!(history.len(), 16, "the fixture lost records");

        // An hour after the newest record, so every one is inside the window.
        let counts = count_by_state(&merge(&[], &history, "2026-08-20T11:00:00Z"));
        assert_eq!(counts.failed, 14);
        assert_eq!(counts.completed, 2);
        assert_eq!(
            counts.pending + counts.running,
            0,
            "a finished job is never pending"
        );

        // Two days later the window has closed on all of them.
        assert_eq!(
            count_by_state(&merge(&[], &history, "2026-08-22T11:00:00Z")),
            BatchJobCounts::default()
        );
    }

    #[test]
    fn an_unreadable_timestamp_keeps_the_job_rather_than_dropping_it() {
        let mut record = remembered("x", JobOutcome::Failed, "");
        record.finished_at = "not a date".into();
        assert_eq!(merge(&[], &[record], NOW).len(), 1);
    }
}
