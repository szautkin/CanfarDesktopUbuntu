//! Per-column filtering of search results.
//!
//! The vocabulary of a single condition is CADC's — ported from the code its
//! Advanced Search grid actually runs, `cadc.votv.js`, functions
//! `searchFilter()` and `valueFilters()`:
//!
//! | Input | Meaning |
//! |---|---|
//! | `a..b` | between a and b, inclusive |
//! | `>v` `>=v` `<v` `<=v` | comparison |
//! | `=v` | exact match, case-insensitive |
//! | anything else | contains, case-insensitive |
//!
//! Three of its behaviours are reproduced exactly, because each is the
//! difference between a filter that is useful and one that is a trap:
//!
//! * **Numeric when both sides parse as numbers, lexical otherwise.** `>2020`
//!   works on a date column and `>m` works on a target name.
//! * **A numeric condition drops empty and `NaN` cells.** Asking for `>5`
//!   should not keep rows that have no value at all.
//! * **A condition with no value constrains nothing**, so a half-typed `>`
//!   does not blank the table.
//!
//! On top of that vocabulary we accept **boolean expressions**, which CADC does
//! not: `&`/`&&`/`AND`, `|`/`||`/`OR`, `!`/`NOT`, and parentheses, with the
//! usual precedence (NOT binds tightest, then AND, then OR). CADC gives you one
//! condition per box and a bare `!`; `!tess & !apass` is a thing people
//! immediately try to type, and reading it as one long literal — which is what
//! a single-condition parser does — silently matches nothing and, negated,
//! keeps every row.
//!
//! Two rules keep the operators from eating real data:
//!
//! * The **word** forms must be upper-case and stand alone (` AND `, ` OR `,
//!   ` NOT `), so a proposal title containing "and" is still just text.
//! * **Double quotes** make any run literal, `""` for a quote inside one — the
//!   escape hatch for a value that really does contain `&` or `|`.
//!
//! `!` is an operator only where an operand is expected, so `foo!bar` is text.
//!
//! Two places we knowingly differ from CADC inside a single condition:
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

/// What one condition asks of a cell.
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

/// A parsed filter box.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterExpr {
    Condition(FilterOp),
    Not(Box<FilterExpr>),
    All(Vec<FilterExpr>),
    Any(Vec<FilterExpr>),
}

// ───────────────────────────────────────────────────────────────────────────
// Numbers
// ───────────────────────────────────────────────────────────────────────────

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

// ───────────────────────────────────────────────────────────────────────────
// Tokens
// ───────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Open,
    Close,
    And,
    Or,
    Not,
    /// One condition's text, quotes already resolved.
    Text(String),
}

/// Characters that always end a condition. Everything else — including the
/// comparison operators, spaces, dots and a mid-word `!` — is part of it.
const BREAKS: [char; 5] = ['(', ')', '&', '|', '"'];

/// The upper-case word operators, which must stand alone to count.
const WORDS: [(&str, Token); 3] = [("AND", Token::And), ("OR", Token::Or), ("NOT", Token::Not)];

/// Whether `rest` opens with a stand-alone upper-case word operator, and which.
fn word_operator(rest: &str) -> Option<(Token, usize)> {
    for (word, token) in WORDS {
        if let Some(after) = rest.strip_prefix(word) {
            let ends = after
                .chars()
                .next()
                .is_none_or(|c| c.is_whitespace() || c == '(' || c == '!');
            if ends {
                return Some((token, word.len()));
            }
        }
    }
    None
}

fn tokenize(input: &str) -> Vec<Token> {
    let chars: Vec<char> = input.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;

    while i < chars.len() {
        if chars[i].is_whitespace() {
            i += 1;
            continue;
        }
        let rest: String = chars[i..].iter().collect();

        match chars[i] {
            '(' => {
                tokens.push(Token::Open);
                i += 1;
                continue;
            }
            ')' => {
                tokens.push(Token::Close);
                i += 1;
                continue;
            }
            '&' => {
                tokens.push(Token::And);
                i += if chars.get(i + 1) == Some(&'&') { 2 } else { 1 };
                continue;
            }
            '|' => {
                tokens.push(Token::Or);
                i += if chars.get(i + 1) == Some(&'|') { 2 } else { 1 };
                continue;
            }
            // `!` is an operator only in operand position — which is exactly
            // where the tokenizer is when it reaches the start of a token.
            '!' => {
                tokens.push(Token::Not);
                i += 1;
                continue;
            }
            _ => {}
        }
        if let Some((token, len)) = word_operator(&rest) {
            tokens.push(token);
            i += len;
            continue;
        }

        // A condition: everything up to the next break, with quoted runs taken
        // literally.
        let mut text = String::new();
        while i < chars.len() {
            let c = chars[i];
            if c == '"' {
                i += 1;
                while i < chars.len() {
                    if chars[i] == '"' {
                        // `""` inside a quoted run is one literal quote.
                        if chars.get(i + 1) == Some(&'"') {
                            text.push('"');
                            i += 2;
                            continue;
                        }
                        i += 1;
                        break;
                    }
                    text.push(chars[i]);
                    i += 1;
                }
                continue;
            }
            if BREAKS.contains(&c) {
                break;
            }
            if c.is_whitespace() {
                // A word operator after this whitespace ends the condition; the
                // whitespace itself is trimmed away either way.
                let ahead: String = chars[i..].iter().collect();
                if word_operator(ahead.trim_start()).is_some() {
                    break;
                }
            }
            text.push(c);
            i += 1;
        }
        let text = text.trim();
        if !text.is_empty() {
            tokens.push(Token::Text(text.to_string()));
        }
    }
    tokens
}

// ───────────────────────────────────────────────────────────────────────────
// Conditions
// ───────────────────────────────────────────────────────────────────────────

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

/// Parse one condition. `None` means it constrains nothing: an empty box, a
/// bare operator, or a range still missing its upper bound.
fn parse_condition(text: &str) -> Option<FilterOp> {
    let rest = text.trim();
    if rest.is_empty() {
        return None;
    }

    // A range, but only with something on the left: a leading ".." is not one
    // (CADC tests `indexOf('..') > 0` for the same reason).
    if let Some(at) = rest.find("..") {
        if at > 0 {
            let high = rest[at + 2..].trim();
            if high.is_empty() {
                return None;
            }
            return Some(FilterOp::Range {
                low: rest[..at].trim().to_string(),
                high: high.to_string(),
            });
        }
    }

    for (symbol, build) in OPERATORS {
        if let Some(value) = rest.strip_prefix(symbol) {
            let value = value.trim();
            if value.is_empty() {
                return None;
            }
            return Some(build(value.to_string()));
        }
    }

    Some(FilterOp::Contains(rest.to_string()))
}

// ───────────────────────────────────────────────────────────────────────────
// Parser
// ───────────────────────────────────────────────────────────────────────────

struct Parser<'a> {
    tokens: &'a [Token],
    at: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }

    fn eat(&mut self, token: &Token) -> bool {
        if self.peek() == Some(token) {
            self.at += 1;
            return true;
        }
        false
    }

    /// `any := all ( OR all )*`
    fn any(&mut self) -> Option<FilterExpr> {
        let mut branches: Vec<FilterExpr> = self.all().into_iter().collect();
        while self.eat(&Token::Or) {
            branches.extend(self.all());
        }
        combine(branches, FilterExpr::Any)
    }

    /// `all := unary ( AND unary )*`
    fn all(&mut self) -> Option<FilterExpr> {
        let mut branches: Vec<FilterExpr> = self.unary().into_iter().collect();
        while self.eat(&Token::And) {
            branches.extend(self.unary());
        }
        combine(branches, FilterExpr::All)
    }

    /// `unary := NOT unary | primary`
    fn unary(&mut self) -> Option<FilterExpr> {
        if self.eat(&Token::Not) {
            // `!` with nothing after it constrains nothing, rather than
            // excluding everything — the state you pass through while typing
            // `!tess` should not blank the table.
            return self.unary().map(|inner| FilterExpr::Not(Box::new(inner)));
        }
        self.primary()
    }

    /// `primary := ( any ) | condition`
    fn primary(&mut self) -> Option<FilterExpr> {
        if self.eat(&Token::Open) {
            let inner = self.any();
            self.eat(&Token::Close);
            return inner;
        }
        // An unmatched `)` or a stray operator: skip it rather than stop, so the
        // rest of a half-typed expression still filters.
        match self.peek()?.clone() {
            Token::Text(text) => {
                self.at += 1;
                parse_condition(&text).map(FilterExpr::Condition)
            }
            _ => {
                self.at += 1;
                None
            }
        }
    }
}

/// Fold the branches of an AND/OR: nothing at all is inert, one branch needs no
/// wrapper. Branches that constrain nothing were already dropped on the way in,
/// which is what makes a half-typed `!tess & !` keep showing the `!tess` rows.
fn combine(
    mut branches: Vec<FilterExpr>,
    wrap: fn(Vec<FilterExpr>) -> FilterExpr,
) -> Option<FilterExpr> {
    match branches.len() {
        0 => None,
        1 => branches.pop(),
        _ => Some(wrap(branches)),
    }
}

impl FilterExpr {
    /// Parse a filter box. `None` means it constrains nothing.
    pub fn parse(text: &str) -> Option<Self> {
        let tokens = tokenize(text);
        let mut parser = Parser {
            tokens: &tokens,
            at: 0,
        };
        let mut expr = parser.any();
        // Trailing junk after a closed expression — `a) b` — should still
        // contribute rather than be dropped in silence.
        while parser.at < tokens.len() {
            let more = parser.any();
            if parser.at == tokens.len() && more.is_none() {
                break;
            }
            expr = match (expr, more) {
                (Some(a), Some(b)) => Some(FilterExpr::All(vec![a, b])),
                (a, b) => a.or(b),
            };
        }
        expr
    }

    /// Whether `cell` survives this filter.
    pub fn matches(&self, cell: &str) -> bool {
        match self {
            Self::Condition(op) => op.keeps(cell),
            Self::Not(inner) => !inner.matches(cell),
            Self::All(branches) => branches.iter().all(|b| b.matches(cell)),
            Self::Any(branches) => branches.iter().any(|b| b.matches(cell)),
        }
    }
}

/// Case-insensitive lexical comparison, the fallback whenever the values are
/// not both numeric.
fn lexical(a: &str, b: &str) -> std::cmp::Ordering {
    a.to_uppercase().cmp(&b.to_uppercase())
}

impl FilterOp {
    /// Whether every operand is a number — the switch between numeric and
    /// lexical comparison.
    fn is_numeric(&self) -> bool {
        match self {
            Self::Range { low, high } => {
                parse_number(low).is_some() && parse_number(high).is_some()
            }
            Self::Gt(v) | Self::Ge(v) | Self::Lt(v) | Self::Le(v) | Self::Exact(v) => {
                parse_number(v).is_some()
            }
            Self::Contains(v) => parse_number(v).is_some(),
        }
    }

    fn keeps(&self, cell: &str) -> bool {
        let cell = cell.trim();

        // A numeric condition drops rows that have no value. Without this, `>5`
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

/// The one description of the filter syntax, shown in the results toolbar's
/// help popover and — abbreviated — on each filter box.
///
/// It lives here, beside the parser, so the two cannot drift: a rule added to
/// the grammar has no other place to be written down.
pub const FILTER_SYNTAX: &[(&str, &str)] = &[
    ("text", "contains, ignoring case"),
    ("=text", "matches the whole cell"),
    (
        "10  >10  >=10",
        "compare — as numbers where both sides are numbers",
    ),
    ("<10  <=10", "compare, the other way"),
    ("10..20", "a range, both ends included"),
    ("!text", "NOT — excludes what follows"),
    ("a & b", "AND — both must hold (also `&&`, `AND`)"),
    ("a | b", "OR — either may hold (also `||`, `OR`)"),
    (
        "(a | b) & !c",
        "parentheses group; NOT binds tightest, then AND, then OR",
    ),
    ("\"a & b\"", "quotes make it literal text"),
];

/// The short form, for a filter box's own tooltip.
pub fn filter_tooltip(numeric: bool) -> String {
    let opening = if numeric {
        crate::tr_en!("Number: 10, >=10, or 10..20 for a range.")
    } else {
        crate::tr_en!("Text: matches anywhere in the cell; =text matches all of it.")
    };
    format!(
        "{opening} {}",
        crate::tr_en!("Combine with ! (not), & (and), | (or) and parentheses.")
    )
}

// ───────────────────────────────────────────────────────────────────────────
// Rows
// ───────────────────────────────────────────────────────────────────────────

/// Positions of the rows that survive the per-column filters, in input order.
/// AND across columns.
///
/// Indices rather than rows because the rows are the expensive part: matching
/// 10,000 of them takes ~2 ms, cloning the survivors 15–45 ms (each carries a
/// 41-entry map). Callers that only need a count, a page, or an ordering never
/// have to pay it. `cargo test --release filter_cost -- --ignored --nocapture`
/// prints both halves.
pub fn matching_indices(
    rows: &[SearchResultRow],
    column_filters: &HashMap<String, String>,
) -> Vec<usize> {
    let parsed: Vec<(&String, FilterExpr)> = column_filters
        .iter()
        .filter_map(|(col, text)| FilterExpr::parse(text).map(|e| (col, e)))
        .collect();

    if parsed.is_empty() {
        return (0..rows.len()).collect();
    }

    (0..rows.len())
        .filter(|&i| {
            parsed
                .iter()
                .all(|(col, expr)| expr.matches(rows[i].get(col)))
        })
        .collect()
}

/// Reorder `indices` by one column of `rows`, on the same terms as
/// [`sort_rows`].
pub fn sort_indices(
    rows: &[SearchResultRow],
    indices: &mut [usize],
    column: &str,
    ascending: bool,
) {
    indices.sort_by(|&a, &b| compare_cells(rows[a].get(column), rows[b].get(column), ascending));
}

/// Sort search result rows by a column. Smart: numeric if both parse, else
/// string. Empty values sort last regardless of direction.
/// The ordering of two cells: numeric if both parse, else case-insensitive
/// text; empty last whichever way the sort runs. The one place the rule lives,
/// so sorting rows and sorting indices cannot disagree.
fn compare_cells(va: &str, vb: &str, ascending: bool) -> std::cmp::Ordering {
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

    /// The rows a filter set keeps, by position.
    fn kept(rows: &[SearchResultRow], filters: &[(&str, &str)]) -> Vec<usize> {
        let filters: HashMap<String, String> = filters
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        matching_indices(rows, &filters)
    }

    /// One column's values, in the order the sort puts them.
    fn sorted(rows: &[SearchResultRow], column: &str, ascending: bool) -> Vec<String> {
        let mut indices: Vec<usize> = (0..rows.len()).collect();
        sort_indices(rows, &mut indices, column, ascending);
        indices
            .iter()
            .map(|&i| rows[i].get(column).to_string())
            .collect()
    }

    // ── Booleans: the thing CADC does not have ──────────────────────────────

    #[test]
    fn two_negated_conditions_joined_by_an_ampersand() {
        // Typed into the collection box against a real result set. Read as one
        // literal it matches nothing, and being negated it then keeps EVERY
        // row — the table looks unfiltered and the user cannot tell why.
        assert!(!keeps("!tess & !apass", "TESS"));
        assert!(!keeps("!tess & !apass", "APASS"));
        assert!(keeps("!tess & !apass", "CFHT"));
    }

    #[test]
    fn every_spelling_of_and_and_or() {
        for and in ["&", "&&", "AND"] {
            let filter = format!("hst {and} raw");
            assert!(keeps(&filter, "hst-raw"), "{filter}");
            assert!(!keeps(&filter, "hst-calibrated"), "{filter}");
        }
        for or in ["|", "||", "OR"] {
            let filter = format!("hst {or} cfht");
            assert!(keeps(&filter, "CFHT"), "{filter}");
            assert!(!keeps(&filter, "JCMT"), "{filter}");
        }
        assert!(!keeps("NOT tess", "TESS"));
        assert!(keeps("NOT tess", "CFHT"));
    }

    #[test]
    fn not_binds_tighter_than_and_which_binds_tighter_than_or() {
        // `a | b & !c` is `a OR (b AND NOT c)`.
        assert!(keeps("hst | cfht & !raw", "HST-raw"));
        assert!(keeps("hst | cfht & !raw", "CFHT-cal"));
        assert!(!keeps("hst | cfht & !raw", "CFHT-raw"));
        assert!(!keeps("hst | cfht & !raw", "JCMT-cal"));
    }

    #[test]
    fn parentheses_override_precedence() {
        assert!(!keeps("(hst | cfht) & !raw", "HST-raw"));
        assert!(keeps("(hst | cfht) & !raw", "HST-cal"));
    }

    #[test]
    fn booleans_compose_with_the_comparison_operators() {
        assert!(keeps(">=2 & <=4", "3"));
        assert!(!keeps(">=2 & <=4", "5"));
        assert!(keeps("<2 | >4", "5"));
        assert!(!keeps("<2 | >4", "3"));
        assert!(keeps("1..3 | 7..9", "8"));
        assert!(!keeps("1..3 | 7..9", "5"));
    }

    // ── The operators must not eat real data ────────────────────────────────

    #[test]
    fn a_lower_case_and_in_a_proposal_title_is_just_text() {
        // "Dynamics and Kinematics" must not parse as two conditions. That is
        // the whole reason the word forms are upper-case only.
        let expr = FilterExpr::parse("dynamics and kinematics").expect("parsed");
        assert_eq!(
            expr,
            FilterExpr::Condition(FilterOp::Contains("dynamics and kinematics".into()))
        );
        assert!(keeps("dynamics and kinematics", "Dynamics and Kinematics"));
    }

    #[test]
    fn a_word_operator_must_stand_alone() {
        // "ANDROMEDA" starts with AND and is not an operator.
        let expr = FilterExpr::parse("ANDROMEDA").expect("parsed");
        assert_eq!(
            expr,
            FilterExpr::Condition(FilterOp::Contains("ANDROMEDA".into()))
        );
    }

    #[test]
    fn a_bang_inside_a_word_is_text() {
        let expr = FilterExpr::parse("foo!bar").expect("parsed");
        assert_eq!(
            expr,
            FilterExpr::Condition(FilterOp::Contains("foo!bar".into()))
        );
    }

    #[test]
    fn quotes_make_the_operators_literal() {
        let expr = FilterExpr::parse("\"a & b\"").expect("parsed");
        assert_eq!(
            expr,
            FilterExpr::Condition(FilterOp::Contains("a & b".into()))
        );
        assert!(keeps("\"a & b\"", "xx a & b yy"));
        // A quote inside a quoted run, doubled.
        let expr = FilterExpr::parse("\"say \"\"hi\"\"\"").expect("parsed");
        assert_eq!(
            expr,
            FilterExpr::Condition(FilterOp::Contains("say \"hi\"".into()))
        );
    }

    #[test]
    fn a_multi_word_value_is_one_condition() {
        // Space is not an implicit AND, or "NGC 253" would stop working.
        assert!(keeps("NGC 253", "NGC 253"));
        assert!(!keeps("NGC 253", "NGC 254"));
    }

    // ── Half-typed expressions must never blank the table ───────────────────

    #[test]
    fn no_prefix_of_a_real_expression_blanks_the_table() {
        // Typing "!tess & !apass" one character at a time must never reach a
        // state that rejects every row. That is what a mid-typing `!` or a
        // dangling `&` does if inert branches are treated as conditions, and it
        // makes the table flash empty under the cursor.
        let collections = ["TESS", "APASS", "CFHT", "JCMT", "HST"];
        let target = "!tess & !apass";
        for end in 1..=target.len() {
            let partial = &target[..end];
            let survivors = collections.iter().filter(|c| keeps(partial, c)).count();
            assert!(
                survivors > 0,
                "typing {partial:?} rejects every row: {collections:?}"
            );
        }
        // And the finished expression does what it says.
        let kept: Vec<&&str> = collections.iter().filter(|c| keeps(target, c)).collect();
        assert_eq!(kept, ["CFHT", "JCMT", "HST"].iter().collect::<Vec<_>>());
    }

    #[test]
    fn a_condition_with_no_value_constrains_nothing() {
        assert_eq!(FilterExpr::parse(">"), None);
        assert_eq!(FilterExpr::parse(">= "), None);
        assert_eq!(FilterExpr::parse("="), None);
        assert_eq!(FilterExpr::parse(""), None);
        assert_eq!(FilterExpr::parse("   "), None);
        assert_eq!(FilterExpr::parse("5.."), None);
        assert_eq!(FilterExpr::parse("!"), None);
        assert_eq!(FilterExpr::parse("!>"), None);
        assert_eq!(FilterExpr::parse("&"), None);
        assert_eq!(FilterExpr::parse("()"), None);
    }

    #[test]
    fn an_inert_branch_drops_out_rather_than_swallowing_the_expression() {
        // "!tess & " is "!tess": the trailing operator contributes nothing.
        assert_eq!(FilterExpr::parse("!tess & "), FilterExpr::parse("!tess"));
        assert_eq!(FilterExpr::parse("!tess | >"), FilterExpr::parse("!tess"));
    }

    #[test]
    fn an_unclosed_parenthesis_still_filters() {
        assert!(!keeps("(hst | cfht", "JCMT"));
        assert!(keeps("(hst | cfht", "HST"));
    }

    // ── One condition: the CADC vocabulary ──────────────────────────────────

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
        assert!(keeps("! > 5", "1"));
    }

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
    fn a_leading_double_dot_is_not_a_range() {
        // CADC tests `indexOf('..') > 0`, so `..5` falls through to the text
        // branch rather than becoming a range with an empty lower bound.
        assert_eq!(
            FilterExpr::parse("..5"),
            Some(FilterExpr::Condition(FilterOp::Contains("..5".into())))
        );
    }

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

    #[test]
    fn the_longer_operator_wins() {
        assert_eq!(parse_condition(">=5"), Some(FilterOp::Ge("5".into())));
        assert_eq!(parse_condition("<=5"), Some(FilterOp::Le("5".into())));
        assert_eq!(parse_condition(">5"), Some(FilterOp::Gt("5".into())));
    }

    #[test]
    fn the_tooltip_and_the_help_cover_the_whole_grammar() {
        // Every operator the parser accepts has to appear in the help the user
        // can actually read, or it may as well not exist. The screenshot that
        // prompted the boolean work was someone typing `&` because they assumed
        // it worked — nothing on screen said either way.
        let help: String = FILTER_SYNTAX
            .iter()
            .map(|(a, b)| format!("{a} {b} "))
            .collect();
        for operator in [
            "=", ">", ">=", "<", "<=", "..", "!", "&", "|", "AND", "OR", "(",
        ] {
            assert!(
                help.contains(operator),
                "the filter help never mentions {operator:?}"
            );
        }
        for tip in [filter_tooltip(true), filter_tooltip(false)] {
            for operator in ["!", "&", "|"] {
                assert!(
                    tip.contains(operator),
                    "{tip:?} never mentions {operator:?}"
                );
            }
        }
    }

    // ── filter_rows ─────────────────────────────────────────────────────────

    #[test]
    fn filter_empty_returns_all() {
        let rows = vec![make_row(&[("name", "M31")])];
        assert_eq!(kept(&rows, &[]), vec![0]);
    }

    #[test]
    fn filter_by_column() {
        let rows = vec![
            make_row(&[("name", "M31"), ("collection", "HST")]),
            make_row(&[("name", "M42"), ("collection", "JWST")]),
            make_row(&[("name", "M51"), ("collection", "HST")]),
        ];
        assert_eq!(kept(&rows, &[("collection", "HST")]), vec![0, 2]);
    }

    #[test]
    fn filter_case_insensitive() {
        let rows = vec![make_row(&[("name", "Andromeda")])];
        assert_eq!(kept(&rows, &[("name", "androm")]), vec![0]);
    }

    #[test]
    fn columns_combine_with_and() {
        let rows = vec![
            make_row(&[("collection", "HST"), ("callev", "2")]),
            make_row(&[("collection", "HST"), ("callev", "0")]),
            make_row(&[("collection", "CFHT"), ("callev", "2")]),
        ];
        assert_eq!(
            kept(&rows, &[("collection", "HST"), ("callev", ">=2")]),
            vec![0]
        );
    }

    #[test]
    fn a_half_typed_filter_does_not_blank_the_table() {
        // The keystroke between "" and ">5" must not empty the grid.
        let rows = vec![make_row(&[("callev", "2")]), make_row(&[("callev", "0")])];
        assert_eq!(kept(&rows, &[("callev", ">")]), vec![0, 1]);
    }

    /// What one keystroke in a filter box actually costs.
    ///
    ///     cargo test --release filter_cost -- --ignored --nocapture
    ///
    /// Ignored because it measures rather than asserts: the number moves with
    /// the machine, and a threshold picked here would fail on someone else's.
    /// It exists so the next person to wonder "is the filter the slow part?"
    /// can find out in one command instead of guessing.
    #[test]
    #[ignore = "measurement, not an assertion"]
    fn filter_cost_on_a_full_result_set() {
        let collections = ["TESS", "APASS", "CFHT", "JCMT", "HST", "GEMINI"];
        // 10,000 rows of 41 columns, the shape of a maxed-out CAOM2 result set.
        let rows: Vec<SearchResultRow> = (0..10_000)
            .map(|i| {
                let mut row = SearchResultRow::default();
                for c in 0..41 {
                    row.values
                        .insert(format!("column {c}"), format!("value {i} {c}"));
                }
                row.values.insert(
                    "collection".into(),
                    collections[i % collections.len()].to_string(),
                );
                row
            })
            .collect();

        for filter in ["tess", "!tess & !apass", "(hst | cfht) & !raw"] {
            let mut filters = HashMap::new();
            filters.insert("collection".to_string(), filter.to_string());

            let started = std::time::Instant::now();
            let indices = matching_indices(&rows, &filters);
            let matching = started.elapsed();

            // What the grid used to do on every keystroke, and now does for one
            // page only.
            let started = std::time::Instant::now();
            let materialised: Vec<SearchResultRow> =
                indices.iter().map(|&i| rows[i].clone()).collect();
            println!(
                "{filter:>20} -> {:>5} of {} rows: matching {matching:?}, \
                 cloning them all {:?}",
                materialised.len(),
                rows.len(),
                started.elapsed()
            );
        }
    }

    // ── Sorting ─────────────────────────────────────────────────────────────

    #[test]
    fn sort_numeric() {
        let rows = vec![
            make_row(&[("ra", "100.5")]),
            make_row(&[("ra", "50.2")]),
            make_row(&[("ra", "200.1")]),
        ];
        assert_eq!(sorted(&rows, "ra", true), ["50.2", "100.5", "200.1"]);
    }

    #[test]
    fn sort_string() {
        let rows = vec![
            make_row(&[("name", "Zebra")]),
            make_row(&[("name", "Apple")]),
            make_row(&[("name", "Mango")]),
        ];
        assert_eq!(sorted(&rows, "name", true), ["Apple", "Mango", "Zebra"]);
    }

    #[test]
    fn sort_empty_last() {
        let rows = vec![
            make_row(&[("val", "")]),
            make_row(&[("val", "100")]),
            make_row(&[("val", "50")]),
        ];
        // Empty last, whichever way the sort runs.
        assert_eq!(sorted(&rows, "val", true), ["50", "100", ""]);
        assert_eq!(sorted(&rows, "val", false), ["100", "50", ""]);
    }

    #[test]
    fn sort_descending() {
        let rows = vec![
            make_row(&[("val", "1")]),
            make_row(&[("val", "3")]),
            make_row(&[("val", "2")]),
        ];
        assert_eq!(sorted(&rows, "val", false), ["3", "2", "1"]);
    }
}
