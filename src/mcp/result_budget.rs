//! Keeping a tool result small enough for the agent that asked for it.
//!
//! `search_observations` caps its result at 1000 rows, which sounds like a
//! budget and is not one: a `SELECT *` over `caom2.Observation` has some sixty
//! columns, so a thousand rows measured **622 KB** in QA. The client truncated
//! it into a file, and the agent's next move was to grep that file — having
//! asked a question and received a document.
//!
//! A row count is the wrong dimension. Ten rows of one column and ten rows of
//! sixty are the same number and nothing like the same cost, so the budget here
//! is in BYTES, and rows are kept whole until it runs out.
//!
//! The other half is telling the caller what happened. A silently shortened
//! list is worse than a long one: the agent reasons over a partial set believing
//! it is complete. Every trim says how many rows exist, how many came back, and
//! what to do to see the rest.

use serde_json::Value;

/// What one tool result may cost the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResultBudget {
    /// Largest serialized payload, in bytes.
    pub max_bytes: usize,
}

/// How much larger the wire reply is than the data it carries.
///
/// Every `Data` result is sent TWICE: once as `structuredContent`, and once as
/// `content[0].text`, where it is the same JSON serialized INTO a string, so
/// every quote is escaped and grows a backslash. The MCP spec recommends that
/// duplication for clients that read only `content`, so it stays — but a budget
/// that ignores it is not a budget.
///
/// Measured, not guessed: 61 KB of rows left the server as a 152 KB reply.
const ENVELOPE_FACTOR: usize = 3;

impl ResultBudget {
    /// The budget from the user's settings.
    ///
    /// The setting names what the user cares about — how big the reply is — so
    /// the data allowance is that divided by the envelope's own overhead.
    /// Without this the setting reads "64 KB" and delivers 152.
    pub fn from_settings() -> Self {
        let s = crate::services::notebook_settings_service::NotebookSettingsService::new().load();
        Self::for_reply_of(s.agent_result_max_kb as usize * 1024)
    }

    /// The data allowance for a reply that may be `reply_bytes` on the wire.
    ///
    /// Separate from `from_settings` so the arithmetic can be tested without
    /// reading a settings file — the rule this encodes is the whole reason the
    /// budget is not simply the number the user typed.
    pub fn for_reply_of(reply_bytes: usize) -> Self {
        Self {
            max_bytes: (reply_bytes / ENVELOPE_FACTOR).max(1024),
        }
    }

    pub fn of_bytes(max_bytes: usize) -> Self {
        Self { max_bytes }
    }
}

/// What a trim did, so the caller can say so.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trimmed<T> {
    /// The rows that fit.
    pub kept: Vec<T>,
    /// How many there were before the budget was applied.
    pub total: usize,
    /// True when rows were dropped to fit.
    pub over_budget: bool,
}

impl<T> Trimmed<T> {
    pub fn dropped(&self) -> usize {
        self.total.saturating_sub(self.kept.len())
    }
}

/// Keep whole rows until `budget` runs out.
///
/// `size_of` measures one row. Rows are kept in order, so a caller that sorted
/// by relevance keeps its best ones — and at least one row always comes back
/// when any exist, because a result of nothing with no explanation is the least
/// useful answer of all.
pub fn fit_rows<T>(
    rows: Vec<T>,
    budget: ResultBudget,
    size_of: impl Fn(&T) -> usize,
) -> Trimmed<T> {
    let total = rows.len();
    let mut used = 0usize;
    let mut kept = Vec::with_capacity(rows.len().min(64));
    for row in rows {
        let size = size_of(&row);
        if !kept.is_empty() && used + size > budget.max_bytes {
            break;
        }
        used += size;
        kept.push(row);
    }
    let over_budget = kept.len() < total;
    Trimmed {
        kept,
        total,
        over_budget,
    }
}

/// The serialized size of a value, as the client will receive it.
pub fn json_size(value: &Value) -> usize {
    serde_json::to_string(value).map(|s| s.len()).unwrap_or(0)
}

/// What to tell an agent whose result was shortened.
///
/// Names the remedy rather than the limit: an agent told only "truncated"
/// retries the same call.
pub fn trim_note(returned: usize, total: usize, what: &str, remedy: &str) -> String {
    format!("Showing {returned} of {total} {what} — the rest did not fit in one reply. {remedy}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rows(n: usize, width: usize) -> Vec<Value> {
        (0..n)
            .map(|i| json!({"id": i, "pad": "x".repeat(width)}))
            .collect()
    }

    #[test]
    fn a_result_that_fits_is_untouched() {
        let t = fit_rows(rows(5, 10), ResultBudget::of_bytes(100_000), json_size);
        assert_eq!(t.kept.len(), 5);
        assert_eq!(t.total, 5);
        assert!(!t.over_budget);
        assert_eq!(t.dropped(), 0);
    }

    /// The budget is bytes, not rows — which is the whole point.
    ///
    /// A thousand narrow rows are cheap; a hundred wide ones are not. The old
    /// cap counted rows and let a `SELECT *` through at 622 KB.
    #[test]
    fn width_counts_not_just_row_count() {
        let budget = ResultBudget::of_bytes(1_000);
        let narrow = fit_rows(rows(100, 1), budget, json_size);
        let wide = fit_rows(rows(100, 200), budget, json_size);
        assert!(
            narrow.kept.len() > wide.kept.len(),
            "wide rows ({}) were not costed above narrow ones ({})",
            wide.kept.len(),
            narrow.kept.len()
        );
    }

    #[test]
    fn a_trimmed_result_reports_what_was_dropped() {
        let t = fit_rows(rows(100, 200), ResultBudget::of_bytes(1_000), json_size);
        assert!(t.over_budget);
        assert_eq!(t.total, 100);
        assert_eq!(t.dropped(), 100 - t.kept.len());
        assert!(t.kept.len() < 100);
    }

    /// One row always comes back, even if it alone busts the budget.
    #[test]
    fn a_single_enormous_row_is_still_returned() {
        let t = fit_rows(rows(3, 50_000), ResultBudget::of_bytes(100), json_size);
        assert_eq!(
            t.kept.len(),
            1,
            "an unreadably wide row left nothing at all"
        );
        assert!(t.over_budget);
    }

    #[test]
    fn an_empty_result_stays_empty_and_is_not_over_budget() {
        let t = fit_rows(Vec::<Value>::new(), ResultBudget::of_bytes(10), json_size);
        assert!(t.kept.is_empty());
        assert_eq!(t.total, 0);
        assert!(!t.over_budget);
    }

    #[test]
    fn rows_are_kept_in_order_so_the_best_survive() {
        let t = fit_rows(rows(50, 100), ResultBudget::of_bytes(1_500), json_size);
        assert_eq!(t.kept[0]["id"], 0);
        for (i, row) in t.kept.iter().enumerate() {
            assert_eq!(row["id"], i, "order was not preserved");
        }
    }

    /// The reply budget accounts for the envelope, not just the data.
    ///
    /// A result is serialized twice on the wire, so a naive budget delivers
    /// roughly three times what the setting promised.
    #[test]
    fn the_data_allowance_is_smaller_than_the_reply_budget() {
        // 64 KB of rows became a 152 KB reply before this existed.
        let reply = 64 * 1024;
        let allowance = ResultBudget::for_reply_of(reply).max_bytes;
        assert!(
            allowance < reply / 2,
            "the allowance ({allowance}) leaves no room for the copy of itself \
             that every reply carries"
        );
    }

    /// A tiny setting still leaves room for a row.
    #[test]
    fn the_allowance_never_collapses_to_nothing() {
        assert!(ResultBudget::for_reply_of(0).max_bytes >= 1024);
        assert!(ResultBudget::for_reply_of(10).max_bytes >= 1024);
    }

    /// A tiny setting still returns something.
    #[test]
    fn a_very_small_budget_does_not_collapse_to_zero() {
        let t = fit_rows(rows(10, 50), ResultBudget::of_bytes(1), json_size);
        assert_eq!(t.kept.len(), 1);
    }

    /// The note says what to DO, not just that something was cut.
    #[test]
    fn the_note_names_a_remedy() {
        let note = trim_note(
            12,
            900,
            "observations",
            "Narrow the columns or lower `max`.",
        );
        assert!(note.contains("12 of 900"), "{note}");
        assert!(note.contains("Narrow the columns"), "{note}");
    }
}
