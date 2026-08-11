//! Process-wide "this store changed" signals, so a page can follow edits it did
//! not make.
//!
//! Port of the `*.Changed` events the reference hangs on its store objects. Our
//! stores are deliberately stateless — every caller constructs its own, MCP
//! appliers on the tokio pool included — so there is no single instance to
//! attach an event to. A global counter per store is the honest substitute:
//! mutations bump it from whichever thread they run on, and a GTK page compares
//! it against what it last rendered.
//!
//! It exists because a page otherwise refreshes only on its OWN actions. An
//! agent that checks off a workflow step, saves a query or downloads an
//! observation changes what is on screen, and without this the user keeps
//! looking at the previous state until they happen to click something.

use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::Mutex;

/// Which store a change belongs to. One counter each, so a busy store never
/// forces an unrelated page to re-render.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Store {
    /// Research protocols (`builtin:…`, `local:…`, `vospace:…`).
    Workflows,
    /// Saved ADQL queries, keyed by name.
    SavedQueries,
    /// Recent-search history, keyed by its `searchedAt` stamp.
    RecentSearches,
}

/// The last change to one store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// Increments on every mutation. A reader stores the last value it handled
    /// and acts only when it moves — so a reader that missed several changes
    /// still catches up in one refresh rather than replaying each.
    pub seq: u64,
    /// Identifier of the item that changed, in whatever form the store uses.
    /// Empty when the change was wholesale (a clear, say).
    pub id: String,
}

static LATEST: Lazy<Mutex<HashMap<Store, Change>>> = Lazy::new(|| Mutex::new(HashMap::new()));

/// Announce that `id` in `store` was created, edited or removed.
///
/// Fire-and-forget: a poisoned lock is ignored rather than propagated, because
/// failing a save over a missed UI refresh would be the wrong trade.
pub fn record_change(store: Store, id: &str) {
    if let Ok(mut latest) = LATEST.lock() {
        let seq = latest.get(&store).map(|c| c.seq).unwrap_or(0) + 1;
        latest.insert(
            store,
            Change {
                seq,
                id: id.to_string(),
            },
        );
    }
}

/// The most recent change to `store`, or `None` when it has not changed this
/// session.
pub fn latest(store: Store) -> Option<Change> {
    LATEST.lock().ok().and_then(|l| l.get(&store).cloned())
}

/// The current sequence for `store` — what a reader records as "already seen"
/// when it first renders, so pre-existing changes are not replayed as new.
pub fn current_seq(store: Store) -> u64 {
    latest(store).map(|c| c.seq).unwrap_or(0)
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
        record_change(Store::Workflows, "local:demo");
        let change = latest(Store::Workflows).expect("a change was recorded");
        assert_eq!(change.id, "local:demo");
    }

    #[test]
    fn the_sequence_advances_so_a_reader_can_tell_new_from_seen() {
        let _guard = TEST_LOCK.lock();
        record_change(Store::Workflows, "local:a");
        let first = current_seq(Store::Workflows);
        record_change(Store::Workflows, "local:b");
        let second = latest(Store::Workflows).unwrap();

        assert!(
            second.seq > first,
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
        let before = current_seq(Store::Workflows);
        for id in ["local:x", "local:y", "local:z"] {
            record_change(Store::Workflows, id);
        }
        let after = latest(Store::Workflows).unwrap();
        assert_eq!(after.seq, before + 3);
        assert_eq!(after.id, "local:z", "only the newest id is retained");
    }

    #[test]
    fn each_store_counts_separately() {
        // A page watching saved queries must not redraw because a workflow
        // changed — and, more importantly, must not MISS its own change because
        // another store advanced the shared counter past it.
        let _guard = TEST_LOCK.lock();
        let queries_before = current_seq(Store::SavedQueries);
        record_change(Store::Workflows, "local:unrelated");
        assert_eq!(
            current_seq(Store::SavedQueries),
            queries_before,
            "a workflow edit is not a saved-query edit"
        );

        record_change(Store::SavedQueries, "My query");
        assert_eq!(current_seq(Store::SavedQueries), queries_before + 1);
    }

    #[test]
    fn a_wholesale_change_reports_an_empty_id() {
        // "Clear all" has no single id; the reader still needs to know.
        let _guard = TEST_LOCK.lock();
        record_change(Store::RecentSearches, "");
        let change = latest(Store::RecentSearches).unwrap();
        assert!(change.id.is_empty());
        assert!(change.seq > 0, "it still counts as a change");
    }
}
