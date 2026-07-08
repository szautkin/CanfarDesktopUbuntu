//! `AgentActivityLog` — a global, thread-safe, newest-first ring of the MCP tool
//! calls an AI agent has recently made. Port of the "agent is working" side of
//! `Mcp/Agents/AgentActivity.cs` + `McpHost.NotifyAgentWorking` / the macOS
//! `AgentActivityLog`.
//!
//! The MCP router calls [`record`] (fire-and-forget) for every agent tool
//! dispatch; the UI polls [`is_active_within`] to flash a transient
//! "agent working…" indicator and can read [`recent`] to show the last few
//! tools. This is deliberately dependency-light: no proposal/outcome tracking
//! lives here (that is `crate::mcp::agent_events`), only a lightweight
//! "an agent touched a tool at time T" breadcrumb.

use once_cell::sync::Lazy;
use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Maximum number of retained breadcrumbs; older entries are evicted FIFO.
const CAP: usize = 200;

/// One agent-activity breadcrumb: the tool that ran and a human-readable local
/// timestamp (`HH:MM:SS`) for display in the activity feed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentActivity {
    pub tool: String,
    pub at: String,
}

/// Ring entry: the public breadcrumb plus a monotonic instant used for the
/// "active within N secs" test (wall-clock strings are for display only).
struct Entry {
    activity: AgentActivity,
    at_instant: Instant,
}

/// Newest-first, capped, thread-safe ring of agent-activity breadcrumbs.
pub struct AgentActivityLog {
    ring: Mutex<VecDeque<Entry>>,
}

impl Default for AgentActivityLog {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentActivityLog {
    pub fn new() -> Self {
        AgentActivityLog {
            ring: Mutex::new(VecDeque::with_capacity(CAP)),
        }
    }

    /// Record that `tool` was invoked by an agent just now. Newest goes to the
    /// front; the oldest is evicted once the ring exceeds [`CAP`].
    pub fn record(&self, tool: &str) {
        let at = chrono::Local::now().format("%H:%M:%S").to_string();
        let entry = Entry {
            activity: AgentActivity {
                tool: tool.to_string(),
                at,
            },
            at_instant: Instant::now(),
        };
        // Poisoning is harmless here (breadcrumbs only) — recover the guard.
        let mut ring = self.ring.lock().unwrap_or_else(|e| e.into_inner());
        ring.push_front(entry);
        while ring.len() > CAP {
            ring.pop_back();
        }
    }

    /// The most recent `n` breadcrumbs, newest first.
    pub fn recent(&self, n: usize) -> Vec<AgentActivity> {
        let ring = self.ring.lock().unwrap_or_else(|e| e.into_inner());
        ring.iter().take(n).map(|e| e.activity.clone()).collect()
    }

    /// Whether an agent tool ran within the last `secs` seconds.
    pub fn is_active_within(&self, secs: u64) -> bool {
        let ring = self.ring.lock().unwrap_or_else(|e| e.into_inner());
        match ring.front() {
            Some(e) => e.at_instant.elapsed() < Duration::from_secs(secs),
            None => false,
        }
    }
}

/// The process-wide log the router writes and the UI reads.
static GLOBAL: Lazy<AgentActivityLog> = Lazy::new(AgentActivityLog::new);

/// Record an agent tool call on the global log. Called from the MCP router
/// dispatch (the integrator wires this in `crate::mcp::router`).
pub fn record(tool: &str) {
    GLOBAL.record(tool);
}

/// The most recent `n` breadcrumbs from the global log, newest first.
pub fn recent(n: usize) -> Vec<AgentActivity> {
    GLOBAL.recent(n)
}

/// Whether an agent tool ran within the last `secs` seconds (global log).
pub fn is_active_within(secs: u64) -> bool {
    GLOBAL.is_active_within(secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_log_is_never_active() {
        let log = AgentActivityLog::new();
        assert!(!log.is_active_within(5));
        assert!(log.recent(10).is_empty());
    }

    #[test]
    fn record_makes_it_active_within_window() {
        let log = AgentActivityLog::new();
        log.record("search_observations");
        // Just recorded → active for any positive window…
        assert!(log.is_active_within(5));
        // …but a zero-length window is never satisfied (elapsed is never < 0).
        assert!(!log.is_active_within(0));
    }

    #[test]
    fn recent_is_newest_first() {
        let log = AgentActivityLog::new();
        log.record("first_tool");
        log.record("second_tool");
        let r = log.recent(10);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].tool, "second_tool");
        assert_eq!(r[1].tool, "first_tool");
        // Timestamp string is populated (HH:MM:SS → 8 chars).
        assert_eq!(r[0].at.len(), 8);
    }

    #[test]
    fn recent_respects_limit() {
        let log = AgentActivityLog::new();
        for i in 0..5 {
            log.record(&format!("tool_{i}"));
        }
        assert_eq!(log.recent(2).len(), 2);
        assert_eq!(log.recent(2)[0].tool, "tool_4");
    }

    #[test]
    fn ring_is_capped() {
        let log = AgentActivityLog::new();
        for i in 0..(CAP + 50) {
            log.record(&format!("tool_{i}"));
        }
        let all = log.recent(CAP * 2);
        assert_eq!(all.len(), CAP);
        // Newest retained is the last one recorded; oldest were evicted.
        assert_eq!(all[0].tool, format!("tool_{}", CAP + 50 - 1));
    }
}
