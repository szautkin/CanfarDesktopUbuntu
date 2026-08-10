//! `AgentEventLog` — a bounded, monotonic-seq log of proposal-lifecycle events an
//! agent polls with a cursor. Port of the `AgentEventLog` in
//! `Mcp/Tools/Proposals/ProposalStore.cs` (and the macOS `AgentEventLog`).
//!
//! Write tools enqueue proposals into the [`InMemoryProposalStore`]; each arrival
//! or resolution also `emit`s an [`AgentEvent`] here. The `list_events` tool lets
//! an agent poll with the highest seq it has already seen and receive only newer
//! events plus the next cursor. The ring is capped ([`CAP`]): once an event is
//! evicted a stale cursor silently re-baselines to the retained window rather
//! than erroring.
//!
//! [`InMemoryProposalStore`]: crate::mcp::tools::proposals::InMemoryProposalStore

use std::collections::VecDeque;
use std::sync::Mutex;

/// Maximum number of retained events; older events are evicted FIFO.
const CAP: usize = 256;

/// What happened to a proposal. Serializes to the camelCase wire kind
/// (`proposalArrived`, `proposalApplied`, …) matching the C#/macOS event log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
// The shared `Proposal` prefix is the wire contract, not redundancy: each variant
// serializes to the exact `proposalArrived` / `proposalApplied` / … kind the C#
// and macOS event logs emit. Dropping the prefix would rename the wire values.
#[allow(clippy::enum_variant_names)]
pub enum AgentEventKind {
    ProposalArrived,
    ProposalApplied,
    ProposalRejected,
    ProposalWithdrawn,
}

/// One retained proposal-lifecycle occurrence.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentEvent {
    /// Monotonic cursor token; the first emitted event is `1`.
    pub seq: u64,
    pub kind: AgentEventKind,
    pub proposal_id: String,
    pub proposal_kind: String,
    pub summary: String,
    /// The originating client label (`None` = internal/UI). Used to scope
    /// `list_events` so one agent never sees another agent's activity.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
}

struct LogInner {
    ring: VecDeque<AgentEvent>,
    next_seq: u64,
}

/// Thread-safe, monotonic-seq ring buffer (cap [`CAP`]) of proposal-lifecycle
/// events an agent polls with a cursor.
pub struct AgentEventLog {
    inner: Mutex<LogInner>,
}

impl Default for AgentEventLog {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentEventLog {
    pub fn new() -> Self {
        AgentEventLog {
            inner: Mutex::new(LogInner {
                ring: VecDeque::with_capacity(CAP),
                next_seq: 1,
            }),
        }
    }

    /// Append an event, assigning it the next monotonic seq. Evicts the oldest
    /// retained event once the ring exceeds [`CAP`].
    pub fn emit(
        &self,
        kind: AgentEventKind,
        proposal_id: &str,
        proposal_kind: &str,
        summary: &str,
        origin: Option<&str>,
    ) {
        let mut g = self.inner.lock().unwrap();
        let seq = g.next_seq;
        g.next_seq += 1;
        g.ring.push_back(AgentEvent {
            seq,
            kind,
            proposal_id: proposal_id.to_string(),
            proposal_kind: proposal_kind.to_string(),
            summary: summary.to_string(),
            origin: origin.map(|s| s.to_string()),
        });
        while g.ring.len() > CAP {
            g.ring.pop_front();
        }
    }

    /// The oldest retained event's seq (for gap/loss detection). `None` if empty.
    pub fn oldest_seq(&self) -> Option<u64> {
        self.inner.lock().unwrap().ring.front().map(|e| e.seq)
    }

    /// Events with `seq > cursor` (oldest→newest) plus the new cursor.
    ///
    /// The new cursor is the seq of the last returned event, or `cursor`
    /// unchanged when nothing is newer. When `cursor` predates the retained
    /// window (its successor events were already evicted) the caller silently
    /// re-baselines: every retained event is returned, since they all satisfy
    /// `seq > cursor`, and the cursor jumps to the newest retained seq.
    pub fn since(&self, cursor: u64) -> (Vec<AgentEvent>, u64) {
        let g = self.inner.lock().unwrap();
        let events: Vec<AgentEvent> = g.ring.iter().filter(|e| e.seq > cursor).cloned().collect();
        let new_cursor = events.last().map(|e| e.seq).unwrap_or(cursor);
        (events, new_cursor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn emit_n(log: &AgentEventLog, n: u64) {
        for i in 1..=n {
            log.emit(
                AgentEventKind::ProposalArrived,
                &format!("prop-{i}"),
                "save_query",
                "summary",
                None,
            );
        }
    }

    #[test]
    fn emit_and_since_advance_cursor() {
        let log = AgentEventLog::new();
        emit_n(&log, 3);

        let (events, cursor) = log.since(0);
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].seq, 1);
        assert_eq!(events[2].seq, 3);
        assert_eq!(cursor, 3);
        assert_eq!(events[0].kind, AgentEventKind::ProposalArrived);
        assert_eq!(events[0].proposal_id, "prop-1");

        // Nothing newer: cursor stays put, no events.
        let (events, cursor2) = log.since(cursor);
        assert!(events.is_empty());
        assert_eq!(cursor2, 3);

        // A fresh emit is delivered incrementally.
        log.emit(
            AgentEventKind::ProposalApplied,
            "prop-1",
            "save_query",
            "done",
            None,
        );
        let (events, cursor3) = log.since(cursor2);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].seq, 4);
        assert_eq!(events[0].kind, AgentEventKind::ProposalApplied);
        assert_eq!(cursor3, 4);
    }

    #[test]
    fn ring_evicts_past_cap() {
        let log = AgentEventLog::new();
        let total = CAP as u64 + 44; // 300
        emit_n(&log, total);

        let (events, cursor) = log.since(0);
        // Only the newest CAP events survive.
        assert_eq!(events.len(), CAP);
        assert_eq!(events.first().unwrap().seq, total - CAP as u64 + 1); // 45
        assert_eq!(events.last().unwrap().seq, total); // 300
        assert_eq!(cursor, total);
    }

    #[test]
    fn stale_cursor_rebaselines() {
        let log = AgentEventLog::new();
        let total = CAP as u64 + 44; // 300, oldest retained seq = 45
        emit_n(&log, total);
        let oldest = total - CAP as u64 + 1; // 45

        // A cursor pointing at an already-evicted seq re-baselines to the
        // retained window instead of losing events silently or erroring.
        let (events, cursor) = log.since(10);
        assert_eq!(events.len(), CAP);
        assert_eq!(events.first().unwrap().seq, oldest);
        assert_eq!(cursor, total);

        // Re-polling with the fresh cursor yields nothing.
        let (events, cursor2) = log.since(cursor);
        assert!(events.is_empty());
        assert_eq!(cursor2, total);
    }

    #[test]
    fn default_is_empty() {
        let log = AgentEventLog::default();
        let (events, cursor) = log.since(0);
        assert!(events.is_empty());
        assert_eq!(cursor, 0);
    }

    #[test]
    fn serializes_kind_as_camelcase() {
        let ev = AgentEvent {
            seq: 1,
            kind: AgentEventKind::ProposalWithdrawn,
            proposal_id: "prop-1".into(),
            proposal_kind: "save_query".into(),
            summary: "s".into(),
            origin: None,
        };
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["kind"], "proposalWithdrawn");
        assert_eq!(json["proposalId"], "prop-1");
        assert_eq!(json["proposalKind"], "save_query");
    }
}
