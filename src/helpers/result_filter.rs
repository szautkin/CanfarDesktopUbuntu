//! Per-column filtering of search results, in the syntax CADC's own Advanced
//! Search accepts.
//!
//! The grammar is ported from the code CADC actually runs on its results grid —
//! `cadc.votv.js`, functions `searchFilter()` and `valueFilters()`:
//!
//! | Input | Meaning |
//! |---|---|
//! | `!…` | negate — a prefix on any of the rows below |
//! | `a..b` | between a and b, inclusive |
//! | `>v` `>=v` `<v` `<=v` | comparison |
//! | `=v` | exact match, case-insensitive |
//! | anything else | contains, case-insensitive |
//!
//! Across columns the combination is AND: every column's filter must pass.
//!
//! Three CADC behaviours are deliberately reproduced, because each is the
//! difference between a filter that is useful and one that is a trap:
//!
//! * **Numeric when both sides parse as numbers, lexical otherwise.** `>2020`
//!   works on a date column and `>m` works on a target name.
//! * **A numeric filter drops empty and `NaN` cells.** Asking for `>5` should
//!   not keep rows that have no value at all.
//! * **An operator with no value filters nothing**, so a half-typed `>` does
//!   not blank the table.
//!
//! Two places we knowingly differ from CADC, both narrow:
//!
//! * CADC's `=` is broken upstream — `cadc.vot.filters` has no `=` key, so the
//!   operator is never stripped and the cell is compared against the literal
//!   `"= HST"`. The intent is unambiguous (VOTV computes an `exactMatch` flag
//!   for it), so we implement what was meant.
//! * CADC's range branch compares strings case-*sensitively* while every other
//!   branch upper-cases both sides. That is an inconsistency inside one filter
//!   box, not a feature; we are case-insensitive throughout.

use crate::models::search_result::SearchResultRow;
use std::collections::HashMap;

/// What a filter box asks of one cell, once parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterOp {
    /// `a..b` — inclusive on both ends.
    Range {
        low: String,
        high: String,
    },
    Gt(String),
    Ge(String),
    Lt(String),
    Le(String),
    /// `=v` — whole-cell match, case-insensitive.
    Exact(String),
    /// Bare text — substring, case-insensitive.
    Contains(String),
}

/// A parsed filter box: an operation, possibly negated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilterExpr {
    pub negated: bool,
    pub op: FilterOp,
}

/// `parseFloat` semantics, not Rust's: the longest numeric *prefix* counts, so
/// a cell of `"0.5 arcsec"` compares as 0.5 — which is what CADC does and what
/// someone typing `<1` on a units-bearing column means. Non-finite results are
/// not numbers, matching JS `isFinite`.
fn parse_number(s: &str) -> Option<f64> {
    let t = s.trim_start();
    let b = t.as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    let mut saw_digit = false;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
        saw_digit = true;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
            saw_digit = true;
        }
    }
    if !saw_digit {
        return None;
    }
    // An exponent counts only if it actually has digits: "1e" is 1, not an error.
    let mantissa_end = i;
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        let digits_at = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        i = if j > digits_at { j } else { mantissa_end };
    }
    t[..i].parse::<f64>().ok().filter(|v| v.is_finite())
}

/// Builds a [`FilterOp`] from the text following its operator symbol.
type OpBuilder = fn(String) -> FilterOp;

/// The operator prefixes, longest first — `>=` must win over `>`.
const OPERATORS: &[(&str, OpBuilder)] = &[
    (">=", FilterOp::Ge),
    ("<=", FilterOp::Le),
    ("=", FilterOp::Exact),
    (">", FilterOp::Gt),
    ("<", FilterOp::Lt),
];

impl FilterExpr {
    /// Parse a filter box. `None` means "this text constrains nothing" — an
    /// empty box, a bare operator, or a range missing its upper bound.
    ///
    /// `None` is inert even when negated. CADC blanks the whole table for `!>`
    /// on the way to typing `!>5`; treating a half-typed filter as inert in
    /// both directions is the same rule it already applies un-negated.
    pub fn parse(text: &str) -> Option<Self> {
        let trimmed = text.trim();
        let (negated, rest) = match trimmed.strip_prefix('!') {
            Some(rest) => (true, rest.trim()),
            None => (false, trimmed),
        };
        if rest.is_empty() {
            return None;
        }

        // A range, but only with something on the left: a leading ".." is not
        // one (CADC tests `indexOf('..') > 0` for the same reason).
        if let Some(at) = rest.find("..") {
            if at > 0 {
                let high = rest[at + 2..].trim();
                if high.is_empty() {
                    return None;
                }
                return Some(Self {
                    negated,
                    op: FilterOp::Range {
                        low: rest[..at].trim().to_string(),
                        high: high.to_string(),
                    },
                });
            }
        }

        for (symbol, build) in OPERATORS {
            if let Some(value) = rest.strip_prefix(symbol) {
                let value = value.trim();
                if value.is_empty() {
                    return None;
                }
                return Some(Self {
                    negated,
                    op: build(value.to_string()),
                });
            }
        }

        Some(Self {
            negated,
            op: FilterOp::Contains(rest.to_string()),
        })
    }

    /// Whether `cell` survives this filter.
    pub fn matches(&self, cell: &str) -> bool {
        self.op.keeps(cell) != self.negated
    }
}

/// Case-insensitive lexical comparison, the fallback whenever the values are
/// not both numeric.
fn lexical(a: &str, b: &str) -> std::cmp::Ordering {
    a.to_uppercase().cmp(&b.to_uppercase())
}

impl FilterOp {
    /// The operand(s) this operator compares against, for the numeric test.
    fn operands(&self) -> Vec<&str> {
        match self {
            Self::Range { low, high } => vec![low.as_str(), high.as_str()],
            Self::Gt(v) | Self::Ge(v) | Self::Lt(v) | Self::Le(v) | Self::Exact(v) => {
                vec![v.as_str()]
            }
            Self::Contains(v) => vec![v.as_str()],
        }
    }

    /// Whether every operand is a number — the switch between numeric and
    /// lexical comparison.
    fn is_numeric(&self) -> bool {
        self.operands().iter().all(|v| parse_number(v).is_some())
    }

    fn keeps(&self, cell: &str) -> bool {
        let cell = cell.trim();

        // A numeric filter drops rows that have no value. Without this, `>5`
        // keeps every row whose cell is blank, which reads as the filter having
        // silently failed.
        if self.is_numeric() && (cell.is_empty() || cell.eq_ignore_ascii_case("nan")) {
            return false;
        }

        match self {
            Self::Range { low, high } => {
                match (parse_number(cell), parse_number(low), parse_number(high)) {
                    (Some(c), Some(lo), Some(hi)) => c >= lo && c <= hi,
                    _ => {
                        lexical(cell, low) != std::cmp::Ordering::Less
                            && lexical(cell, high) != std::cmp::Ordering::Greater
                    }
                }
            }
            Self::Gt(v) => self.compare(cell, v, |o| o == std::cmp::Ordering::Greater),
            Self::Ge(v) => self.compare(cell, v, |o| o != std::cmp::Ordering::Less),
            Self::Lt(v) => self.compare(cell, v, |o| o == std::cmp::Ordering::Less),
            Self::Le(v) => self.compare(cell, v, |o| o != std::cmp::Ordering::Greater),
            Self::Exact(v) => lexical(cell, v) == std::cmp::Ordering::Equal,
            Self::Contains(v) => cell.to_lowercase().contains(&v.to_lowercase()),
        }
    }

    fn compare(&self, cell: &str, value: &str, accept: fn(std::cmp::Ordering) -> bool) -> bool {
        let ordering = match (parse_number(cell), parse_number(value)) {
            (Some(c), Some(v)) => c.partial_cmp(&v).unwrap_or(std::cmp::Ordering::Equal),
            _ => lexical(cell, value),
        };
        accept(ordering)
    }
}

/// The tooltip CADC puts on a filter input, chosen by column type. Its own
/// wording (`cadc.votv.js:932`), so a user who knows the web form reads the
/// same sentence here — and so the help cannot drift from the parser without
/// someone noticing.
pub fn filter_tooltip(numeric: bool) -> &'static str {
    if numeric {
        crate::tr_en!("Number: 10 or >=10 or 10..20 for a range , ! to negate")
    } else {
        crate::tr_en!("String: Substring match , ! to negate matches")
    }
}

/// Filter search result rows by their per-column filters. AND across columns.
pub fn filter_rows(
    rows: &[SearchResultRow],
    column_filters: &HashMap<String, String>,
) -> Vec<SearchResultRow> {
    let parsed: Vec<(&String, FilterExpr)> = column_filters
        .iter()
        .filter_map(|(col, text)| FilterExpr::parse(text).map(|e| (col, e)))
        .collect();

    if parsed.is_empty() {
        return rows.to_vec();
    }

    rows.iter()
        .filter(|row| parsed.iter().all(|(col, expr)| expr.matches(row.get(col))))
        .cloned()
        .collect()
}

/// Sort search result rows by a column. Smart: numeric if both parse, else string.
/// Empty values sort last regardless of direction.
pub fn sort_rows(rows: &mut [SearchResultRow], column: &str, ascending: bool) {
    rows.sort_by(|a, b| {
        let va = a.get(column);
        let vb = b.get(column);

        // Empty values sort last
        match (va.is_empty(), vb.is_empty()) {
            (true, true) => return std::cmp::Ordering::Equal,
            (true, false) => return std::cmp::Ordering::Greater,
            (false, true) => return std::cmp::Ordering::Less,
            _ => {}
        }

        // Try numeric comparison
        let cmp = match (va.parse::<f64>(), vb.parse::<f64>()) {
            (Ok(na), Ok(nb)) => na.partial_cmp(&nb).unwrap_or(std::cmp::Ordering::Equal),
            _ => va.to_lowercase().cmp(&vb.to_lowercase()),
        };

        if ascending {
            cmp
        } else {
            cmp.reverse()
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_row(vals: &[(&str, &str)]) -> SearchResultRow {
        let mut row = SearchResultRow::default();
        for (k, v) in vals {
            row.values.insert(k.to_string(), v.to_string());
        }
        row
    }

    fn keeps(filter: &str, cell: &str) -> bool {
        FilterExpr::parse(filter).is_none_or(|e| e.matches(cell))
    }

    // ── The grammar ─────────────────────────────────────────────────────────

    #[test]
    fn bare_text_is_a_case_insensitive_substring() {
        assert!(keeps("androm", "Andromeda"));
        assert!(keeps("ANDROM", "Andromeda"));
        assert!(!keeps("triang", "Andromeda"));
    }

    #[test]
    fn exact_match_ignores_case_and_requires_the_whole_cell() {
        assert!(keeps("=HST", "hst"));
        assert!(keeps("= HST", "HST"));
        assert!(!keeps("=HST", "HSTHLA"));
        // The substring form still matches the longer value — the two operators
        // have to differ, or `=` is decoration.
        assert!(keeps("HST", "HSTHLA"));
    }

    #[test]
    fn comparisons_are_numeric_when_both_sides_are() {
        assert!(keeps(">5", "10"));
        assert!(!keeps(">5", "5"));
        assert!(keeps(">=5", "5"));
        assert!(keeps("<5", "4.9"));
        assert!(keeps("<=5", "5"));
        assert!(!keeps("<=5", "5.1"));
        // Not lexical: "10" < "5" as text, and getting that wrong is the bug
        // this test exists to catch.
        assert!(keeps(">5", "10"));
    }

    #[test]
    fn comparisons_fall_back_to_case_insensitive_text() {
        assert!(keeps(">m", "NGC 253"));
        assert!(!keeps(">m", "abell 2218"));
        // Case must not decide the answer: lowercase 'n' > uppercase 'M' by
        // byte order, but 'ABELL' < 'M' either way.
        assert!(keeps(">M", "ngc 253"));
        assert!(keeps("<m", "Abell 2218"));
    }

    #[test]
    fn a_range_is_inclusive_on_both_ends() {
        assert!(keeps("5..10", "5"));
        assert!(keeps("5..10", "10"));
        assert!(keeps("5..10", "7.5"));
        assert!(!keeps("5..10", "4.9"));
        assert!(!keeps("5..10", "10.1"));
    }

    #[test]
    fn a_text_range_compares_case_insensitively() {
        // CADC's range branch is the one place it compares case-sensitively,
        // which makes one filter box behave two ways. We do not.
        assert!(keeps("a..m", "HST"));
        assert!(keeps("a..m", "hst"));
        assert!(!keeps("a..m", "ngc"));
        assert!(!keeps("a..m", "NGC"));
    }

    // ── Negation: the operand the first research pass missed ────────────────

    #[test]
    fn bang_negates_a_substring() {
        assert!(!keeps("!HST", "HSTHLA"));
        assert!(keeps("!HST", "CFHT"));
    }

    #[test]
    fn bang_composes_with_every_operator() {
        assert!(!keeps("!>5", "10"));
        assert!(keeps("!>5", "1"));
        assert!(!keeps("!5..10", "7"));
        assert!(keeps("!5..10", "20"));
        assert!(!keeps("!=HST", "hst"));
        assert!(keeps("!=HST", "CFHT"));
        // With whitespace between the bang and the rest.
        assert!(keeps("! > 5", "1"));
    }

    // ── The three behaviours worth copying exactly ──────────────────────────

    #[test]
    fn a_numeric_filter_drops_cells_that_have_no_value() {
        assert!(!keeps(">5", ""));
        assert!(!keeps(">5", "NaN"));
        assert!(!keeps("5..10", ""));
        // A text filter has no such rule — an empty cell simply fails to
        // contain the needle.
        assert!(!keeps("HST", ""));
    }

    #[test]
    fn a_negated_numeric_filter_still_drops_empty_cells() {
        // Otherwise `!>5` quietly becomes "everything with no value", which is
        // never what someone excluding large values means.
        assert!(keeps("!>5", ""));
    }

    #[test]
    fn an_operator_with_no_value_filters_nothing() {
        assert_eq!(FilterExpr::parse(">"), None);
        assert_eq!(FilterExpr::parse(">= "), None);
        assert_eq!(FilterExpr::parse("="), None);
        assert_eq!(FilterExpr::parse(""), None);
        assert_eq!(FilterExpr::parse("   "), None);
        // A range still being typed.
        assert_eq!(FilterExpr::parse("5.."), None);
        // And the same half-typed filters with a negation in front, which in
        // CADC blank the entire table on the way to `!>5`.
        assert_eq!(FilterExpr::parse("!"), None);
        assert_eq!(FilterExpr::parse("!>"), None);
        assert_eq!(FilterExpr::parse("!5.."), None);
    }

    #[test]
    fn a_leading_double_dot_is_not_a_range() {
        // CADC tests `indexOf('..') > 0`, so `..5` falls through to the text
        // branch rather than becoming a range with an empty lower bound.
        let expr = FilterExpr::parse("..5").expect("parsed");
        assert_eq!(expr.op, FilterOp::Contains("..5".into()));
    }

    // ── Number parsing follows parseFloat, not Rust ─────────────────────────

    #[test]
    fn a_value_with_a_unit_still_compares_as_a_number() {
        assert!(keeps("<1", "0.5 arcsec"));
        assert!(!keeps("<1", "2.5 arcsec"));
    }

    #[test]
    fn scientific_notation_and_signs_parse() {
        assert_eq!(parse_number("8E-7"), Some(8e-7));
        assert_eq!(parse_number("-1.5"), Some(-1.5));
        assert_eq!(parse_number("+.5"), Some(0.5));
        assert_eq!(parse_number("1e"), Some(1.0));
        assert_eq!(parse_number(""), None);
        assert_eq!(parse_number("NaN"), None);
        assert_eq!(parse_number("abc"), None);
        assert_eq!(parse_number("."), None);
    }

    // ── Parsing shape ───────────────────────────────────────────────────────

    #[test]
    fn the_longer_operator_wins() {
        assert_eq!(
            FilterExpr::parse(">=5").map(|e| e.op),
            Some(FilterOp::Ge("5".into()))
        );
        assert_eq!(
            FilterExpr::parse("<=5").map(|e| e.op),
            Some(FilterOp::Le("5".into()))
        );
        assert_eq!(
            FilterExpr::parse(">5").map(|e| e.op),
            Some(FilterOp::Gt("5".into()))
        );
    }

    #[test]
    fn the_tooltip_is_cadcs_own_wording() {
        // Copied from `cadc.votv.js:932`. If the parser changes, these strings
        // have to change with it — which is the point of pinning them.
        assert_eq!(
            filter_tooltip(true),
            "Number: 10 or >=10 or 10..20 for a range , ! to negate"
        );
        assert_eq!(
            filter_tooltip(false),
            "String: Substring match , ! to negate matches"
        );
    }

    // ── filter_rows ─────────────────────────────────────────────────────────

    #[test]
    fn filter_empty_returns_all() {
        let rows = vec![make_row(&[("name", "M31")])];
        let filters = HashMap::new();
        assert_eq!(filter_rows(&rows, &filters).len(), 1);
    }

    #[test]
    fn filter_by_column() {
        let rows = vec![
            make_row(&[("name", "M31"), ("collection", "HST")]),
            make_row(&[("name", "M42"), ("collection", "JWST")]),
            make_row(&[("name", "M51"), ("collection", "HST")]),
        ];
        let mut filters = HashMap::new();
        filters.insert("collection".to_string(), "HST".to_string());
        let result = filter_rows(&rows, &filters);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn filter_case_insensitive() {
        let rows = vec![make_row(&[("name", "Andromeda")])];
        let mut filters = HashMap::new();
        filters.insert("name".to_string(), "androm".to_string());
        assert_eq!(filter_rows(&rows, &filters).len(), 1);
    }

    #[test]
    fn columns_combine_with_and() {
        let rows = vec![
            make_row(&[("collection", "HST"), ("callev", "2")]),
            make_row(&[("collection", "HST"), ("callev", "0")]),
            make_row(&[("collection", "CFHT"), ("callev", "2")]),
        ];
        let mut filters = HashMap::new();
        filters.insert("collection".to_string(), "HST".to_string());
        filters.insert("callev".to_string(), ">=2".to_string());
        assert_eq!(filter_rows(&rows, &filters).len(), 1);
    }

    #[test]
    fn a_half_typed_filter_does_not_blank_the_table() {
        // The keystroke between "" and ">5" must not empty the grid.
        let rows = vec![make_row(&[("callev", "2")]), make_row(&[("callev", "0")])];
        let mut filters = HashMap::new();
        filters.insert("callev".to_string(), ">".to_string());
        assert_eq!(filter_rows(&rows, &filters).len(), 2);
    }

    // ── Sorting ─────────────────────────────────────────────────────────────

    #[test]
    fn sort_numeric() {
        let mut rows = vec![
            make_row(&[("ra", "100.5")]),
            make_row(&[("ra", "50.2")]),
            make_row(&[("ra", "200.1")]),
        ];
        sort_rows(&mut rows, "ra", true);
        assert_eq!(rows[0].get("ra"), "50.2");
        assert_eq!(rows[2].get("ra"), "200.1");
    }

    #[test]
    fn sort_string() {
        let mut rows = vec![
            make_row(&[("name", "Zebra")]),
            make_row(&[("name", "Apple")]),
            make_row(&[("name", "Mango")]),
        ];
        sort_rows(&mut rows, "name", true);
        assert_eq!(rows[0].get("name"), "Apple");
        assert_eq!(rows[2].get("name"), "Zebra");
    }

    #[test]
    fn sort_empty_last() {
        let mut rows = vec![
            make_row(&[("val", "")]),
            make_row(&[("val", "100")]),
            make_row(&[("val", "50")]),
        ];
        sort_rows(&mut rows, "val", true);
        assert_eq!(rows[0].get("val"), "50");
        assert_eq!(rows[1].get("val"), "100");
        assert_eq!(rows[2].get("val"), ""); // empty last
    }

    #[test]
    fn sort_descending() {
        let mut rows = vec![
            make_row(&[("val", "1")]),
            make_row(&[("val", "3")]),
            make_row(&[("val", "2")]),
        ];
        sort_rows(&mut rows, "val", false);
        assert_eq!(rows[0].get("val"), "3");
        assert_eq!(rows[2].get("val"), "1");
    }
}
