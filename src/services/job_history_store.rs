//! The last N finished batch jobs, on disk.
//!
//! CANFAR reaps headless jobs and the image-discovery coordinator deletes its
//! own probes the moment they finish, so the live sessions listing is the wrong
//! place to look for what happened. This store keeps what the listing forgets:
//! the outcome, when, and — for failures — the reason, captured while the job
//! still existed.

use crate::models::job_record::JobRecord;
use directories::ProjectDirs;
use std::path::PathBuf;
use std::sync::Mutex;

/// How many finished jobs to keep.
///
/// The ask was "at least 30"; 50 costs a few tens of kilobytes and means a
/// morning of failed probes does not push the interesting one off the end.
pub const MAX_JOBS: usize = 50;

pub struct JobHistoryStore {
    path: PathBuf,
    /// Serialises read-modify-write. Two pollers finishing at once would
    /// otherwise each read the old list and the second would drop the first's
    /// entry.
    lock: Mutex<()>,
}

impl JobHistoryStore {
    pub fn new() -> Self {
        let dir = ProjectDirs::from("net", "canfar", "Verbinal")
            .map(|dirs| dirs.data_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
        Self::with_dir(dir)
    }

    pub fn with_dir(dir: PathBuf) -> Self {
        JobHistoryStore {
            path: dir.join("job_history.json"),
            lock: Mutex::new(()),
        }
    }

    /// Every remembered job, newest first.
    pub fn load(&self) -> Vec<JobRecord> {
        match std::fs::read_to_string(&self.path) {
            Ok(json) => serde_json::from_str(&json).unwrap_or_default(),
            Err(_) => Vec::new(),
        }
    }

    /// Remember a finished job, replacing any earlier record of the same id.
    ///
    /// Replacing rather than skipping matters: the Batch Jobs poller may notice
    /// a job failed before the reason has been fetched, and the record that
    /// carries the reason has to win.
    pub fn record(&self, entry: JobRecord) -> Result<(), String> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut entries = self.load();
        entries.retain(|e| e.id != entry.id);
        entries.insert(0, entry);
        entries.truncate(MAX_JOBS);
        self.write(&entries)
    }

    /// Forget everything.
    pub fn clear(&self) -> Result<(), String> {
        let _guard = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        self.write(&[])
    }

    fn write(&self, entries: &[JobRecord]) -> Result<(), String> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let json = serde_json::to_string_pretty(entries).map_err(|e| e.to_string())?;
        std::fs::write(&self.path, json).map_err(|e| e.to_string())
    }
}

impl Default for JobHistoryStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::job_record::{JobOrigin, JobOutcome};

    fn store() -> (JobHistoryStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        (JobHistoryStore::with_dir(dir.path().to_path_buf()), dir)
    }

    fn record(id: &str) -> JobRecord {
        JobRecord {
            id: id.to_string(),
            name: format!("job-{id}"),
            image: "images.canfar.net/skaha/terminal:1.1.2".to_string(),
            origin: JobOrigin::ImageProbe,
            outcome: JobOutcome::Failed,
            status: "Failed".to_string(),
            started_at: "2026-08-19T10:00:00Z".to_string(),
            finished_at: "2026-08-19T10:01:00Z".to_string(),
            failure_reason: Some("bash: syft: command not found".to_string()),
            target_image: Some("images.canfar.net/skaha/astroml:1.0".to_string()),
        }
    }

    #[test]
    fn an_empty_store_reads_as_empty_rather_than_failing() {
        let (store, _dir) = store();
        assert!(store.load().is_empty());
    }

    #[test]
    fn the_newest_job_is_first() {
        let (store, _dir) = store();
        store.record(record("a")).unwrap();
        store.record(record("b")).unwrap();
        let ids: Vec<String> = store.load().into_iter().map(|r| r.id).collect();
        assert_eq!(ids, ["b", "a"]);
    }

    #[test]
    fn recording_a_job_twice_keeps_the_later_record() {
        // The poller can see a job fail before the reason has been fetched. The
        // record that carries the reason must win, not be dropped as a
        // duplicate.
        let (store, _dir) = store();
        let mut first = record("a");
        first.failure_reason = None;
        store.record(first).unwrap();

        store.record(record("a")).unwrap();

        let all = store.load();
        assert_eq!(all.len(), 1, "the same job is remembered twice");
        assert_eq!(
            all[0].failure_reason.as_deref(),
            Some("bash: syft: command not found")
        );
    }

    #[test]
    fn the_history_is_capped_and_keeps_the_newest() {
        let (store, _dir) = store();
        for i in 0..MAX_JOBS + 10 {
            store.record(record(&i.to_string())).unwrap();
        }
        let all = store.load();
        assert_eq!(all.len(), MAX_JOBS);
        assert_eq!(all[0].id, (MAX_JOBS + 9).to_string());
    }

    /// The ask was "at least 30 last jobs". A const assertion rather than a
    /// runtime one: lowering the cap should not compile.
    const _: () = assert!(MAX_JOBS >= 30, "the history keeps fewer than 30 jobs");

    #[test]
    fn a_record_survives_a_round_trip_through_the_file() {
        let (store, _dir) = store();
        let original = record("a");
        store.record(original.clone()).unwrap();
        assert_eq!(store.load(), vec![original]);
    }

    #[test]
    fn clearing_empties_the_history() {
        let (store, _dir) = store();
        store.record(record("a")).unwrap();
        store.clear().unwrap();
        assert!(store.load().is_empty());
    }
}
