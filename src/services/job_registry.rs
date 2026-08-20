//! Background jobs an agent can start and then ask about.
//!
//! Some tool calls take longer than a JSON-RPC request should be held open. A
//! 332 MB observation download was one: the router awaited the apply inside the
//! call, the MCP client gave up at its own timeout, and the download vanished
//! with no id, no progress and no error — the caller could not even tell
//! whether it was still running.
//!
//! So the router starts those applies as jobs and answers immediately with an
//! id. This is where their state lives until someone asks.
//!
//! Keyed by the PROPOSAL id rather than a fresh one: the proposal already
//! identifies the piece of work, the agent already has it in the reply, and one
//! identifier is easier to follow than two.

use serde::Serialize;
use std::collections::VecDeque;
use std::sync::Mutex;

/// How many finished jobs to keep. Running ones are never evicted.
const MAX_REMEMBERED: usize = 50;

/// Where a job has got to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum JobStatus {
    Running,
    Succeeded,
    Failed,
}

/// One background job.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    /// The proposal this is applying.
    pub id: String,
    /// The proposal kind, e.g. `download_observation`.
    pub kind: String,
    /// The proposal's one-line summary, so a caller listing jobs can tell them
    /// apart without holding onto what it asked for.
    pub summary: String,
    pub status: JobStatus,
    /// Bytes transferred so far, when the work reports them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub done_bytes: Option<u64>,
    /// Total bytes expected, when known. A server that sends no
    /// `Content-Length` leaves this `None`, and a caller should show "so far"
    /// rather than a percentage.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_bytes: Option<u64>,
    /// The applier's own words: its result on success, its error on failure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    pub started_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
}

impl Job {
    /// Whether this job has stopped, either way.
    pub fn is_finished(&self) -> bool {
        !matches!(self.status, JobStatus::Running)
    }
}

/// The live and recently-finished jobs. Share via the `AppServices` field.
#[derive(Default)]
pub struct JobRegistry {
    jobs: Mutex<VecDeque<Job>>,
}

impl JobRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that `id` has started. Replaces any earlier job with the same id.
    pub fn start(&self, id: &str, kind: &str, summary: &str) {
        let mut jobs = self.lock();
        jobs.retain(|j| j.id != id);
        jobs.push_front(Job {
            id: id.to_string(),
            kind: kind.to_string(),
            summary: summary.to_string(),
            status: JobStatus::Running,
            done_bytes: None,
            total_bytes: None,
            message: None,
            started_at: chrono::Utc::now().to_rfc3339(),
            finished_at: None,
        });
        Self::evict_finished(&mut jobs);
    }

    /// Report how far a running job has got. Ignored for a job that has already
    /// finished — a progress callback can outlive the transfer it belongs to.
    pub fn progress(&self, id: &str, done_bytes: u64, total_bytes: Option<u64>) {
        let mut jobs = self.lock();
        if let Some(job) = jobs.iter_mut().find(|j| j.id == id && !j.is_finished()) {
            job.done_bytes = Some(done_bytes);
            if total_bytes.is_some() {
                job.total_bytes = total_bytes;
            }
        }
    }

    /// Record how a job ended, with the applier's own message either way.
    pub fn finish(&self, id: &str, outcome: Result<String, String>) {
        let mut jobs = self.lock();
        if let Some(job) = jobs.iter_mut().find(|j| j.id == id) {
            let (status, message) = match outcome {
                Ok(msg) => (JobStatus::Succeeded, msg),
                Err(e) => (JobStatus::Failed, e),
            };
            job.status = status;
            job.message = Some(message);
            job.finished_at = Some(chrono::Utc::now().to_rfc3339());
        }
        Self::evict_finished(&mut jobs);
    }

    /// A progress sink that reports into this job.
    ///
    /// The transfer layer already speaks `(done, total)` over a channel, so the
    /// registry meets it there rather than making the download depend on the
    /// registry. The drainer stops when the transfer drops its end.
    pub fn sink(self: &std::sync::Arc<Self>, id: &str) -> crate::services::transfer::ProgressSink {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(u64, Option<u64>)>();
        let registry = std::sync::Arc::clone(self);
        let id = id.to_string();
        tokio::spawn(async move {
            while let Some((done, total)) = rx.recv().await {
                registry.progress(&id, done, total);
            }
        });
        tx
    }

    pub fn get(&self, id: &str) -> Option<Job> {
        self.lock().iter().find(|j| j.id == id).cloned()
    }

    /// Every job we still hold, newest first.
    pub fn recent(&self) -> Vec<Job> {
        self.lock().iter().cloned().collect()
    }

    /// Drop the oldest FINISHED jobs past the cap.
    ///
    /// Running jobs are never evicted: forgetting one loses the only handle on
    /// work that is still happening, which is the failure this whole registry
    /// exists to prevent.
    fn evict_finished(jobs: &mut VecDeque<Job>) {
        while jobs.len() > MAX_REMEMBERED {
            let Some(oldest) = jobs.iter().rposition(|j| j.is_finished()) else {
                break;
            };
            jobs.remove(oldest);
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, VecDeque<Job>> {
        self.jobs.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_started_job_is_running_and_findable() {
        let jobs = JobRegistry::new();
        jobs.start("prop-1", "download_observation", "Download M51");
        let job = jobs.get("prop-1").expect("started");
        assert_eq!(job.status, JobStatus::Running);
        assert_eq!(job.summary, "Download M51");
        assert!(!job.is_finished());
        assert_eq!(job.finished_at, None);
    }

    #[test]
    fn progress_is_reported_while_it_runs() {
        let jobs = JobRegistry::new();
        jobs.start("prop-1", "download_observation", "");
        jobs.progress("prop-1", 1024, Some(4096));
        let job = jobs.get("prop-1").unwrap();
        assert_eq!(job.done_bytes, Some(1024));
        assert_eq!(job.total_bytes, Some(4096));
    }

    #[test]
    fn a_server_that_sends_no_length_leaves_the_total_unknown() {
        // Showing 0 or guessing would make a progress bar lie; the caller needs
        // to know it can only report bytes so far.
        let jobs = JobRegistry::new();
        jobs.start("prop-1", "download_observation", "");
        jobs.progress("prop-1", 512, None);
        let job = jobs.get("prop-1").unwrap();
        assert_eq!(job.done_bytes, Some(512));
        assert_eq!(job.total_bytes, None);
    }

    #[test]
    fn finishing_records_the_appliers_own_words() {
        let jobs = JobRegistry::new();
        jobs.start("ok", "download_observation", "");
        jobs.finish("ok", Ok("Downloaded 1040701p.fits.fz".into()));
        let job = jobs.get("ok").unwrap();
        assert_eq!(job.status, JobStatus::Succeeded);
        assert_eq!(job.message.as_deref(), Some("Downloaded 1040701p.fits.fz"));
        assert!(job.finished_at.is_some());

        jobs.start("bad", "download_observation", "");
        jobs.finish("bad", Err("403 Forbidden".into()));
        let job = jobs.get("bad").unwrap();
        assert_eq!(job.status, JobStatus::Failed);
        assert_eq!(job.message.as_deref(), Some("403 Forbidden"));
    }

    #[test]
    fn progress_after_the_end_is_ignored() {
        // A streaming callback can fire once more after the transfer resolves;
        // it must not resurrect a finished job.
        let jobs = JobRegistry::new();
        jobs.start("prop-1", "download_observation", "");
        jobs.finish("prop-1", Ok("done".into()));
        jobs.progress("prop-1", 999, Some(999));
        let job = jobs.get("prop-1").unwrap();
        assert_eq!(job.status, JobStatus::Succeeded);
        assert_eq!(job.done_bytes, None);
    }

    #[test]
    fn a_running_job_is_never_evicted() {
        // Forgetting one loses the only handle on work still happening — the
        // exact failure this registry exists to prevent.
        let jobs = JobRegistry::new();
        jobs.start("live", "download_observation", "still going");
        for i in 0..MAX_REMEMBERED * 2 {
            let id = format!("done-{i}");
            jobs.start(&id, "download_observation", "");
            jobs.finish(&id, Ok("ok".into()));
        }
        assert!(jobs.get("live").is_some(), "a running job was evicted");
        assert!(jobs.recent().len() <= MAX_REMEMBERED + 1);
    }

    #[test]
    fn restarting_the_same_id_replaces_the_old_attempt() {
        let jobs = JobRegistry::new();
        jobs.start("prop-1", "download_observation", "first");
        jobs.finish("prop-1", Err("timed out".into()));
        jobs.start("prop-1", "download_observation", "retry");
        let all: Vec<Job> = jobs
            .recent()
            .into_iter()
            .filter(|j| j.id == "prop-1")
            .collect();
        assert_eq!(all.len(), 1, "two jobs share an id");
        assert_eq!(all[0].status, JobStatus::Running);
        assert_eq!(all[0].summary, "retry");
    }

    #[test]
    fn the_newest_job_comes_first() {
        let jobs = JobRegistry::new();
        jobs.start("a", "k", "");
        jobs.start("b", "k", "");
        let ids: Vec<String> = jobs.recent().into_iter().map(|j| j.id).collect();
        assert_eq!(ids, ["b", "a"]);
    }
}
