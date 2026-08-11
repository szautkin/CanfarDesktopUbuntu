//! A process-wide "a workflow changed" signal.
//!
//! Port of the `WorkflowStore.Changed` event. The Rust store is deliberately
//! stateless — every caller constructs its own `WorkflowStore::new()`, including
//! the MCP appliers running on the tokio pool — so there is no single instance to
//! hang an event on. A global counter is the honest substitute: mutations bump
//! it from whichever thread they run on, and the GTK page compares it against
//! what it last rendered.
//!
//! It exists so the page follows an AGENT's edits. When an assistant checks off a
//! step, the user should watch the roundel flip on screen; without this the page
//! refreshes only on its own actions and silently shows stale progress until the
//! user clicks something.

use once_cell::sync::Lazy;
use std::sync::Mutex;

/// The last change: a monotonic sequence plus the workflow id it touched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowChange {
    /// Increments on every mutation. A reader stores the last value it handled
    /// and acts only when it moves — so a reader that misses several changes
    /// still catches up in one refresh rather than replaying each.
    pub seq: u64,
    /// Store id of the workflow that changed (`local:…`, `vospace:…`).
    pub id: String,
}

static LATEST: Lazy<Mutex<Option<WorkflowChange>>> = Lazy::new(|| Mutex::new(None));

/// Announce that `id` was created, edited, checked off, published or deleted.
///
/// Fire-and-forget: a poisoned lock is ignored rather than propagated, because
/// failing a workflow save over a missed UI refresh would be the wrong trade.
pub fn record_change(id: &str) {
    if let Ok(mut latest) = LATEST.lock() {
        let seq = latest.as_ref().map(|c| c.seq).unwrap_or(0) + 1;
        *latest = Some(WorkflowChange {
            seq,
            id: id.to_string(),
        });
    }
}

/// The most recent change, or `None` when nothing has changed this session.
pub fn latest() -> Option<WorkflowChange> {
    LATEST.lock().ok().and_then(|l| l.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tests share one global, so they run under a mutex and read the
    /// sequence relatively rather than assuming it starts at zero.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn a_change_is_visible_to_a_reader() {
        let _guard = TEST_LOCK.lock();
        record_change("local:demo");
        let change = latest().expect("a change was recorded");
        assert_eq!(change.id, "local:demo");
    }

    #[test]
    fn the_sequence_advances_so_a_reader_can_tell_new_from_seen() {
        let _guard = TEST_LOCK.lock();
        record_change("local:a");
        let first = latest().unwrap();
        record_change("local:b");
        let second = latest().unwrap();

        assert!(
            second.seq > first.seq,
            "the sequence must advance, or a reader cannot tell a new change from one it handled"
        );
        assert_eq!(second.id, "local:b", "the latest change wins");
    }

    #[test]
    fn several_changes_collapse_into_one_catch_up() {
        // A reader that was away for three edits should refresh ONCE, not three
        // times — the page rebuilds from the store, so replaying each change
        // would be redundant work with identical results.
        let _guard = TEST_LOCK.lock();
        let before = latest().map(|c| c.seq).unwrap_or(0);
        for id in ["local:x", "local:y", "local:z"] {
            record_change(id);
        }
        let after = latest().unwrap();
        assert_eq!(after.seq, before + 3);
        assert_eq!(after.id, "local:z", "only the newest id is retained");
    }
}
