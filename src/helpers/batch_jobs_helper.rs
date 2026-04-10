//! Batch jobs helper — groups CANFAR `headless` sessions by status.
//!
//! Ported from the Windows reference `BatchJobsHelper.GroupByState`. Batch jobs
//! are just CANFAR sessions where `session_type == "headless"`.

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

#[derive(Debug, Clone, Default, Copy)]
pub struct BatchJobCounts {
    pub pending: usize,
    pub running: usize,
    pub completed: usize,
    pub failed: usize,
}

impl BatchJobCounts {
    pub fn get(&self, state: BatchJobState) -> usize {
        match state {
            BatchJobState::Pending => self.pending,
            BatchJobState::Running => self.running,
            BatchJobState::Completed => self.completed,
            BatchJobState::Failed => self.failed,
        }
    }
}

/// Filter to headless jobs and count by state.
pub fn group_by_state(sessions: &[Session]) -> BatchJobCounts {
    let mut counts = BatchJobCounts::default();
    for s in sessions {
        if !is_batch_job(s) {
            continue;
        }
        match BatchJobState::from_status(&s.status) {
            BatchJobState::Pending => counts.pending += 1,
            BatchJobState::Running => counts.running += 1,
            BatchJobState::Completed => counts.completed += 1,
            BatchJobState::Failed => counts.failed += 1,
        }
    }
    counts
}

/// Return only batch jobs (headless sessions) with the given state.
pub fn filter_by_state(sessions: &[Session], state: BatchJobState) -> Vec<Session> {
    sessions
        .iter()
        .filter(|s| is_batch_job(s) && BatchJobState::from_status(&s.status) == state)
        .cloned()
        .collect()
}

fn is_batch_job(s: &Session) -> bool {
    s.session_type.eq_ignore_ascii_case("headless")
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn groups_headless_only() {
        let sessions = vec![
            sess("a", "headless", "Running"),
            sess("b", "headless", "Pending"),
            sess("c", "notebook", "Running"),
            sess("d", "headless", "Succeeded"),
            sess("e", "headless", "Failed"),
            sess("f", "headless", "Running"),
        ];
        let counts = group_by_state(&sessions);
        assert_eq!(counts.running, 2);
        assert_eq!(counts.pending, 1);
        assert_eq!(counts.completed, 1);
        assert_eq!(counts.failed, 1);
    }

    #[test]
    fn filter_by_state_works() {
        let sessions = vec![
            sess("a", "headless", "Running"),
            sess("b", "notebook", "Running"),
            sess("c", "headless", "Running"),
        ];
        let running = filter_by_state(&sessions, BatchJobState::Running);
        assert_eq!(running.len(), 2);
    }
}
