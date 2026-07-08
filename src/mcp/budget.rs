//! Proposal budget. Ported from `Mcp/Tools/Proposals/ProposalBudget.cs`.
//!
//! A cap on how many write-proposals an external agent may have PENDING at
//! once — the backstop against a runaway agent loop that would otherwise flood
//! the review strip. Consulted by the router BEFORE (or right after) a write
//! tool enqueues its proposal; on refusal the router withdraws that proposal so
//! no partial batch lands. The user is never capped by this.
//!
//! Unlike the C# original this type is stateless: the current pending count is
//! owned by the proposal store and passed in, so the budget is a pure,
//! immutable policy value (`cap`). That makes it trivially `Send + Sync` with no
//! lock — cloning or sharing it across threads is free.

/// An immutable cap on the number of simultaneously-pending proposals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProposalBudget {
    cap: usize,
}

impl ProposalBudget {
    /// Create a budget that permits at most `cap` proposals pending at once.
    pub fn new(cap: usize) -> Self {
        Self { cap }
    }

    /// The default cap used when none is configured.
    pub const fn default_cap() -> usize {
        32
    }

    /// The maximum number of proposals that may be pending simultaneously.
    pub fn cap(&self) -> usize {
        self.cap
    }

    /// Whether a new proposal may be accepted given the current pending count.
    ///
    /// True only while strictly below the cap, so accepting brings the total to
    /// at most `cap`.
    pub fn can_accept(&self, current_pending: usize) -> bool {
        current_pending < self.cap
    }

    /// How many more proposals may be accepted before the cap is hit.
    ///
    /// Saturates at zero when already at or over the cap.
    pub fn remaining(&self, current_pending: usize) -> usize {
        self.cap.saturating_sub(current_pending)
    }
}

impl Default for ProposalBudget {
    fn default() -> Self {
        Self::new(Self::default_cap())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_uses_default_cap() {
        assert_eq!(ProposalBudget::default().cap(), ProposalBudget::default_cap());
        assert_eq!(ProposalBudget::default_cap(), 32);
    }

    #[test]
    fn new_reports_its_cap() {
        assert_eq!(ProposalBudget::new(5).cap(), 5);
    }

    #[test]
    fn can_accept_below_cap() {
        let b = ProposalBudget::new(3);
        assert!(b.can_accept(0));
        assert!(b.can_accept(2));
    }

    #[test]
    fn can_accept_rejects_at_and_over_cap() {
        let b = ProposalBudget::new(3);
        assert!(!b.can_accept(3)); // exactly at cap
        assert!(!b.can_accept(4)); // over cap
        assert!(!b.can_accept(usize::MAX));
    }

    #[test]
    fn zero_cap_never_accepts() {
        let b = ProposalBudget::new(0);
        assert!(!b.can_accept(0));
        assert_eq!(b.remaining(0), 0);
    }

    #[test]
    fn remaining_counts_down() {
        let b = ProposalBudget::new(3);
        assert_eq!(b.remaining(0), 3);
        assert_eq!(b.remaining(1), 2);
        assert_eq!(b.remaining(3), 0);
    }

    #[test]
    fn remaining_saturates_over_cap() {
        let b = ProposalBudget::new(3);
        assert_eq!(b.remaining(4), 0);
        assert_eq!(b.remaining(usize::MAX), 0);
    }

    #[test]
    fn is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ProposalBudget>();
    }
}
