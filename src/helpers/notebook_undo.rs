//! Undo/redo history for notebook structural operations.
//!
//! Port of `Services/Notebook/UndoRedoService.cs`. Stores full
//! [`NotebookDocument`] snapshots rather than per-cell deltas — a notebook
//! document is small and cloning is cheap, so a whole-document snapshot keeps
//! the restore logic trivial and correct (outputs and metadata survive an
//! undo, matching the reference `CellSnapshot` round-trip).
//!
//! Semantics mirror the reference:
//! - [`UndoRedoStack::push`] is called *before* every structural change and
//!   clears the redo history (a new action invalidates any redo path).
//! - [`UndoRedoStack::undo`] / [`UndoRedoStack::redo`] take the *current* state
//!   so it can be moved onto the opposite stack before the previous state is
//!   returned for restoration.
//! - Capacity is bounded to [`MAX_DEPTH`] to keep memory usage in check; the
//!   oldest entries are dropped once the cap is exceeded.

use crate::models::notebook_document::NotebookDocument;

/// Maximum number of undo snapshots retained (matches the reference `MaxDepth`).
pub const MAX_DEPTH: usize = 50;

/// A bounded undo/redo history over [`NotebookDocument`] snapshots.
#[derive(Default)]
pub struct UndoRedoStack {
    undo: Vec<NotebookDocument>,
    redo: Vec<NotebookDocument>,
}

impl UndoRedoStack {
    /// Create an empty stack.
    pub fn new() -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    /// `true` if there is at least one state to undo.
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    /// `true` if there is at least one state to redo.
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Push the current state *before* a structural change. Clears the redo
    /// history and trims the oldest entries once [`MAX_DEPTH`] is exceeded.
    pub fn push(&mut self, state: NotebookDocument) {
        self.undo.push(state);
        self.redo.clear(); // a new action invalidates redo
        if self.undo.len() > MAX_DEPTH {
            let overflow = self.undo.len() - MAX_DEPTH;
            self.undo.drain(0..overflow);
        }
    }

    /// Pop the last undo state, moving `current` onto the redo stack.
    /// Returns `None` when there is nothing to undo.
    pub fn undo(&mut self, current: NotebookDocument) -> Option<NotebookDocument> {
        let state = self.undo.pop()?;
        self.redo.push(current);
        Some(state)
    }

    /// Pop the last redo state, moving `current` onto the undo stack.
    /// Returns `None` when there is nothing to redo.
    pub fn redo(&mut self, current: NotebookDocument) -> Option<NotebookDocument> {
        let state = self.redo.pop()?;
        self.undo.push(current);
        Some(state)
    }

    /// Drop all history (both stacks).
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::notebook_document::{CellSource, NotebookDocument};

    /// Build a one-cell document whose single cell carries `marker` as its
    /// source, so snapshots are easy to tell apart in assertions.
    fn doc_with(marker: &str) -> NotebookDocument {
        let mut d = NotebookDocument::create_empty();
        d.cells[0].source = CellSource::Single(marker.to_string());
        d
    }

    fn marker_of(d: &NotebookDocument) -> String {
        d.cells[0].source.joined()
    }

    #[test]
    fn empty_stack_has_nothing() {
        let mut s = UndoRedoStack::new();
        assert!(!s.can_undo());
        assert!(!s.can_redo());
        assert!(s.undo(doc_with("cur")).is_none());
        assert!(s.redo(doc_with("cur")).is_none());
    }

    #[test]
    fn push_then_undo_returns_pushed_state() {
        let mut s = UndoRedoStack::new();
        s.push(doc_with("A")); // state before an edit that produced "B"
        assert!(s.can_undo());
        let restored = s.undo(doc_with("B")).expect("something to undo");
        assert_eq!(marker_of(&restored), "A");
        assert!(!s.can_undo());
        assert!(s.can_redo());
    }

    #[test]
    fn undo_then_redo_round_trips() {
        let mut s = UndoRedoStack::new();
        s.push(doc_with("A"));
        let undone = s.undo(doc_with("B")).unwrap();
        assert_eq!(marker_of(&undone), "A");
        // Current is now "A"; redo should hand back the "B" we passed to undo.
        let redone = s.redo(doc_with("A")).unwrap();
        assert_eq!(marker_of(&redone), "B");
        assert!(s.can_undo());
        assert!(!s.can_redo());
    }

    #[test]
    fn push_clears_redo() {
        let mut s = UndoRedoStack::new();
        s.push(doc_with("A"));
        let _ = s.undo(doc_with("B")); // now redo has "B"
        assert!(s.can_redo());
        s.push(doc_with("C")); // a brand-new action invalidates redo
        assert!(!s.can_redo());
    }

    #[test]
    fn capacity_is_bounded_to_max_depth() {
        let mut s = UndoRedoStack::new();
        for i in 0..(MAX_DEPTH + 10) {
            s.push(doc_with(&format!("s{i}")));
        }
        // Only MAX_DEPTH states are retained; the oldest were dropped.
        let mut count = 0;
        while let Some(restored) = s.undo(doc_with("cur")) {
            // The very newest pushed state was "s{MAX_DEPTH+9}".
            let _ = restored;
            count += 1;
        }
        assert_eq!(count, MAX_DEPTH);
    }

    #[test]
    fn clear_drops_both_stacks() {
        let mut s = UndoRedoStack::new();
        s.push(doc_with("A"));
        let _ = s.undo(doc_with("B"));
        s.clear();
        assert!(!s.can_undo());
        assert!(!s.can_redo());
    }
}
