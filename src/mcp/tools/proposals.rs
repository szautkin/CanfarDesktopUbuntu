//! Write-proposal pipeline. Ported from `Mcp/Tools/Proposals/ProposalStore.cs`.
//!
//! Write tools NEVER mutate app state directly — they enqueue a [`PendingProposal`]
//! that the user reviews and approves (or the auto-apply policy accepts for
//! non-destructive kinds). The store is a FIFO with tombstones so a resolved id
//! can't be double-applied. Resolved tombstones expire after [`TOMBSTONE_TTL`] and
//! the store is capped at [`MAX_RETAINED`] so ids don't linger forever, and every
//! enqueue/resolve emits a lifecycle event to the shared [`AgentEventLog`].

use crate::mcp::agent_events::{AgentEventKind, AgentEventLog};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long a resolved (tombstoned) proposal is retained before pruning.
const TOMBSTONE_TTL: Duration = Duration::from_secs(300);
/// Hard cap on total retained proposals (oldest resolved pruned first; pending
/// are never dropped).
const MAX_RETAINED: usize = 256;

/// A queued, not-yet-applied write.
#[derive(Debug, Clone)]
pub struct PendingProposal {
    pub id: String,
    /// Machine kind used by the applier registry (e.g. `"save_query"`).
    pub kind: String,
    /// Human-readable one-line summary for the review UI.
    pub summary: String,
    /// Whether applying this is destructive (destructive kinds NEVER auto-apply).
    pub destructive: bool,
    /// Opaque payload consumed by the applier for `kind`.
    pub payload: Value,
    pub state: ProposalState,
    /// The originating client label (`None` = internal/UI), stamped by the router
    /// so lifecycle reads/withdraws can be scoped to the agent that created it.
    pub origin: Option<String>,
    /// The MCP tool that created this proposal.
    ///
    /// Usually equal to `kind`, but not by rule — one tool may enqueue a kind
    /// named for the applier that handles it. Stamped by the router alongside
    /// `origin`, which is the only place that knows the name the caller used
    /// (including which alias they called it by). Defaults to `kind` so a
    /// proposal built directly in a test is never blank.
    pub tool_name: String,
    /// When it was queued, ISO-8601. Surfaced as `createdAtISO` so a polling
    /// agent can tell a stale proposal from one it just made.
    pub created_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProposalState {
    Pending,
    /// Atomically claimed by an applier and being applied right now. A proposal in
    /// this state can no longer be reject/withdrawn or re-claimed — only `settle`d.
    Applying,
    Applied,
    Rejected,
    Withdrawn,
}

/// A FIFO proposal store with tombstones (resolved ids remain, marked) that expire
/// after a TTL, plus a shared agent event log fed on every lifecycle transition.
pub struct InMemoryProposalStore {
    inner: Mutex<StoreInner>,
    events: Arc<AgentEventLog>,
}

struct StoreInner {
    order: Vec<String>,
    by_id: HashMap<String, PendingProposal>,
    /// When each resolved proposal was tombstoned (for TTL pruning).
    resolved_at: HashMap<String, Instant>,
    seq: u64,
}

impl Default for InMemoryProposalStore {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryProposalStore {
    pub fn new() -> Self {
        InMemoryProposalStore {
            inner: Mutex::new(StoreInner {
                order: Vec::new(),
                by_id: HashMap::new(),
                resolved_at: HashMap::new(),
                seq: 0,
            }),
            events: Arc::new(AgentEventLog::new()),
        }
    }

    /// The shared agent event log (proposal lifecycle feed) for `list_events`.
    pub fn events(&self) -> Arc<AgentEventLog> {
        Arc::clone(&self.events)
    }

    /// Enqueue a new proposal and return a clone of it.
    pub fn enqueue(
        &self,
        kind: &str,
        summary: &str,
        destructive: bool,
        payload: Value,
    ) -> PendingProposal {
        let mut g = self.inner.lock().unwrap();
        prune(&mut g);
        g.seq += 1;
        // Deterministic-but-unique id (avoids a uuid dependency here; the host may
        // stamp a v4 id when surfacing to the agent).
        let id = format!("prop-{}", g.seq);
        let proposal = PendingProposal {
            id: id.clone(),
            kind: kind.to_string(),
            summary: summary.to_string(),
            destructive,
            payload,
            state: ProposalState::Pending,
            origin: None,
            tool_name: kind.to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
        };
        g.order.push(id.clone());
        g.by_id.insert(id, proposal.clone());
        // NOTE: the `ProposalArrived` event is emitted by the router (not here) once
        // it has stamped the origin, so the event carries the originating client.
        proposal
    }

    /// Stamp the originating client label on a just-enqueued proposal (called by
    /// the router before it emits `ProposalArrived` / runs the applier).
    pub fn set_origin(&self, id: &str, origin: Option<String>) {
        if let Some(p) = self.inner.lock().unwrap().by_id.get_mut(id) {
            p.origin = origin;
        }
    }

    /// Stamp both router-known fields at once: the tool the caller invoked and
    /// the client label it came from.
    ///
    /// One method, one lock: stamping them separately leaves a window where a
    /// concurrent read sees the origin set but the tool name still defaulted.
    pub fn stamp_source(&self, id: &str, tool_name: &str, origin: Option<String>) {
        if let Some(p) = self.inner.lock().unwrap().by_id.get_mut(id) {
            p.tool_name = tool_name.to_string();
            p.origin = origin;
        }
    }

    /// All proposals in FIFO order.
    pub fn list(&self) -> Vec<PendingProposal> {
        let g = self.inner.lock().unwrap();
        g.order
            .iter()
            .filter_map(|id| g.by_id.get(id).cloned())
            .collect()
    }

    /// The pending (unresolved) proposals.
    pub fn pending(&self) -> Vec<PendingProposal> {
        self.list()
            .into_iter()
            .filter(|p| p.state == ProposalState::Pending)
            .collect()
    }

    pub fn get(&self, id: &str) -> Option<PendingProposal> {
        self.inner.lock().unwrap().by_id.get(id).cloned()
    }

    /// Atomically claim a PENDING proposal for application: transition it to
    /// `Applying` under the store lock and return it, or `None` if it isn't
    /// pending (already claimed / resolved / unknown). ONLY the winning caller may
    /// then run the applier — this closes the check-then-act window that would
    /// otherwise let the same proposal be applied twice, or applied-then-withdrawn.
    pub fn claim(&self, id: &str) -> Option<PendingProposal> {
        let mut g = self.inner.lock().unwrap();
        let p = g.by_id.get_mut(id)?;
        if p.state != ProposalState::Pending {
            return None;
        }
        p.state = ProposalState::Applying;
        Some(p.clone())
    }

    /// Settle a claimed (`Applying`) proposal to its final state after the applier
    /// ran. Returns the updated proposal, or `None` if it wasn't `Applying`.
    pub fn settle(&self, id: &str, state: ProposalState) -> Option<PendingProposal> {
        debug_assert!(matches!(
            state,
            ProposalState::Applied | ProposalState::Rejected
        ));
        let resolved = {
            let mut g = self.inner.lock().unwrap();
            let p = g.by_id.get_mut(id)?;
            if p.state != ProposalState::Applying {
                return None;
            }
            p.state = state;
            let resolved = p.clone();
            g.resolved_at.insert(id.to_string(), Instant::now());
            prune(&mut g);
            resolved
        };
        self.emit_for(state, &resolved);
        Some(resolved)
    }

    /// Transition a PENDING proposal directly to a resolved state (for reject /
    /// withdraw / budget backstop — no side effect runs). Returns the updated
    /// proposal, or `None` if the id is unknown, being applied, or already
    /// resolved. Applying-in-flight proposals are protected (returns `None`).
    pub fn resolve(&self, id: &str, state: ProposalState) -> Option<PendingProposal> {
        if state == ProposalState::Pending || state == ProposalState::Applying {
            return None;
        }
        let resolved = {
            let mut g = self.inner.lock().unwrap();
            let p = g.by_id.get_mut(id)?;
            if p.state != ProposalState::Pending {
                return None; // tombstone: never double-resolve
            }
            p.state = state;
            let resolved = p.clone();
            g.resolved_at.insert(id.to_string(), Instant::now());
            prune(&mut g);
            resolved
        };
        self.emit_for(state, &resolved);
        Some(resolved)
    }

    /// Emit the lifecycle event matching a resolved `state`. The store lock must
    /// NOT be held here (avoids nesting the store + event-log locks).
    fn emit_for(&self, state: ProposalState, resolved: &PendingProposal) {
        let kind = match state {
            ProposalState::Applied => AgentEventKind::ProposalApplied,
            ProposalState::Rejected => AgentEventKind::ProposalRejected,
            ProposalState::Withdrawn => AgentEventKind::ProposalWithdrawn,
            ProposalState::Pending | ProposalState::Applying => return,
        };
        self.events.emit(
            kind,
            &resolved.id,
            &resolved.kind,
            &resolved.summary,
            resolved.origin.as_deref(),
        );
    }

    pub fn pending_count(&self) -> usize {
        self.pending().len()
    }
}

/// Drop expired tombstones (older than [`TOMBSTONE_TTL`]) and, if still over
/// [`MAX_RETAINED`], the oldest resolved entries. Pending proposals are never
/// pruned. Called under the store lock.
fn prune(inner: &mut StoreInner) {
    let now = Instant::now();

    // 1. TTL: forget resolved proposals whose tombstone has aged out.
    let expired: Vec<String> = inner
        .resolved_at
        .iter()
        .filter(|(_, t)| now.duration_since(**t) > TOMBSTONE_TTL)
        .map(|(id, _)| id.clone())
        .collect();
    for id in expired {
        inner.by_id.remove(&id);
        inner.resolved_at.remove(&id);
        inner.order.retain(|o| o != &id);
    }

    // 2. CAP: while over the retention cap, drop the oldest RESOLVED entry.
    while inner.order.len() > MAX_RETAINED {
        let oldest_resolved = inner.order.iter().position(|id| {
            inner
                .by_id
                .get(id)
                .map(|p| p.state != ProposalState::Pending)
                .unwrap_or(true)
        });
        match oldest_resolved {
            Some(pos) => {
                let id = inner.order.remove(pos);
                inner.by_id.remove(&id);
                inner.resolved_at.remove(&id);
            }
            None => break, // everything left is pending — cannot prune further
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn enqueue_list_resolve() {
        let store = InMemoryProposalStore::new();
        let p = store.enqueue("save_query", "Save query 'M31'", false, json!({"q":"M31"}));
        assert_eq!(store.pending_count(), 1);
        assert_eq!(p.state, ProposalState::Pending);
        let applied = store.resolve(&p.id, ProposalState::Applied).unwrap();
        assert_eq!(applied.state, ProposalState::Applied);
        assert_eq!(store.pending_count(), 0);
        // Double-resolve is refused (tombstone).
        assert!(store.resolve(&p.id, ProposalState::Rejected).is_none());
    }

    #[test]
    fn resolve_emits_scoped_event() {
        // The store emits on resolve/settle (the router emits Arrived after stamping
        // origin); the event carries the proposal's stamped origin.
        let store = InMemoryProposalStore::new();
        let events = store.events();
        let p = store.enqueue("save_query", "Save M31", false, json!({}));
        store.set_origin(&p.id, Some("agent-A".into()));
        store.resolve(&p.id, ProposalState::Withdrawn);
        let (evs, cursor) = events.since(0);
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].kind, AgentEventKind::ProposalWithdrawn);
        assert_eq!(evs[0].origin.as_deref(), Some("agent-A"));
        let (evs2, _) = events.since(cursor);
        assert!(evs2.is_empty());
    }

    #[test]
    fn claim_is_exclusive_and_protects_against_apply_reject_race() {
        let store = InMemoryProposalStore::new();
        let p = store.enqueue("delete_node", "Delete /foo", true, json!({}));
        // First claim wins (Pending → Applying); it's no longer "pending".
        let claimed = store.claim(&p.id).unwrap();
        assert_eq!(claimed.state, ProposalState::Applying);
        assert_eq!(store.pending_count(), 0);
        // A concurrent second claim loses.
        assert!(store.claim(&p.id).is_none());
        // A concurrent reject/withdraw of an in-flight apply is refused (no lost update).
        assert!(store.resolve(&p.id, ProposalState::Withdrawn).is_none());
        // Only settle can finish it, exactly once.
        assert!(store.settle(&p.id, ProposalState::Applied).is_some());
        assert!(store.settle(&p.id, ProposalState::Applied).is_none());
        // And it can't be settled from a non-Applying state.
        assert_eq!(store.get(&p.id).unwrap().state, ProposalState::Applied);
    }

    #[test]
    fn reject_of_pending_still_works_and_blocks_later_claim() {
        let store = InMemoryProposalStore::new();
        let p = store.enqueue("delete_node", "Delete /bar", true, json!({}));
        assert!(store.resolve(&p.id, ProposalState::Withdrawn).is_some());
        // A withdrawn proposal can never be claimed for apply.
        assert!(store.claim(&p.id).is_none());
    }

    #[test]
    fn cap_prunes_oldest_resolved_but_keeps_pending() {
        let store = InMemoryProposalStore::new();
        for i in 0..(MAX_RETAINED + 50) {
            let p = store.enqueue("k", &format!("s{i}"), false, json!({}));
            store.resolve(&p.id, ProposalState::Applied);
        }
        // A surviving pending proposal is never pruned.
        let keep = store.enqueue("keep", "keep-me", true, json!({}));
        assert!(store.list().len() <= MAX_RETAINED + 1);
        assert!(store.get(&keep.id).is_some());
        assert_eq!(store.pending_count(), 1);
    }
}
