//! Convert active per-column client-side filters into an ADQL `WHERE` fragment.
//!
//! Port of `CanfarDesktop/Helpers/FilterToAdqlConverter.cs`. Pure, no UI
//! dependencies, fully testable. The Search page uses this to turn the
//! narrow-to-value / typed column filters into a server-side query via the
//! "Apply filters to ADQL" button.
//!
//! Column keys are the cleaned header ids produced by
//! [`crate::models::search_result::clean_key`]; they are mapped back to the
//! real CAOM2 ADQL columns (including the few computed columns that use ADQL
//! functions such as `COORD1(CENTROID(...))`).

use crate::helpers::result_filter::{FilterExpr, FilterOp};
use crate::models::search_result::clean_key;
use std::collections::HashMap;

/// Map a cleaned column key to its qualified ADQL column expression.
/// Mirrors `FilterToAdqlConverter.ColumnToAdql`.
fn column_to_adql(cleaned_key: &str) -> Option<&'static str> {
    Some(match cleaned_key {
        "observationid" => "Observation.observationID",
        "collection" => "Observation.collection",
        "targetname" => "Observation.target_name",
        "instrument" => "Observation.instrument_name",
        "filter" => "Plane.energy_bandpassName",
        "callev" => "Plane.calibrationLevel",
        "obstype" => "Observation.type",
        "proposalid" => "Observation.proposal_id",
        "piname" => "Observation.proposal_pi",
        "obsid" => "Observation.observationID",
        "datatype" => "Plane.dataProductType",
        "band" => "Plane.energy_emBand",
        "intent" => "Observation.intent",
        "ra(j20000)" => "COORD1(CENTROID(Plane.position_bounds))",
        "dec(j20000)" => "COORD2(CENTROID(Plane.position_bounds))",
        "startdate" => "Plane.time_bounds_lower",
        "enddate" => "Plane.time_bounds_upper",
        "inttime" => "Plane.time_exposure",
        "minwavelength" => "Plane.energy_bounds_lower",
        "maxwavelength" => "Plane.energy_bounds_upper",
        "pixelscale" => "Plane.position_sampleSize",
        "resolvingpower" => "Plane.energy_resolvingPower",
        "fieldofview" => "AREA(Plane.position_bounds)",
        _ => return None,
    })
}

/// An ADQL string literal: single quotes doubled, lower-cased for the
/// case-insensitive comparisons.
fn literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''").to_lowercase())
}

/// A LIKE pattern: the metacharacters escaped as well, exactly as the reference
/// converter does.
fn like_pattern(value: &str) -> String {
    let escaped = value
        .replace('\'', "''")
        .replace('%', "\\%")
        .replace('_', "\\_")
        .to_lowercase();
    format!("'%{}%'", escaped)
}

/// Render one operand as a numeric literal if it is one, otherwise as a string
/// literal. `{}` yields the shortest round-tripping decimal (no scientific
/// notation for normal ranges) — good enough for a filter predicate.
fn operand(value: &str) -> String {
    match value.parse::<f64>() {
        Ok(num) if num.is_finite() => format!("{num}"),
        _ => literal(value),
    }
}

/// One side of a comparison, matched to how the operand renders: a numeric
/// operand compares against the column, a string operand against `lower(col)`
/// so the comparison is case-insensitive like the grid's.
fn comparison(adql_col: &str, symbol: &str, value: &str) -> String {
    match value.parse::<f64>() {
        Ok(num) if num.is_finite() => format!("{adql_col} {symbol} {num}"),
        _ => format!("lower({adql_col}) {symbol} {}", literal(value)),
    }
}

/// Build a single WHERE clause for one column and one *parsed* filter.
///
/// This has to share `FilterExpr` with the grid rather than re-read the text.
/// It used to take the raw string and turn anything non-numeric into
/// `LIKE '%…%'`, so once the grid learned `!raw`, "Apply filters to ADQL" would
/// have queried for rows *containing* `!raw` — the exact opposite of the rows
/// on screen, with no error to notice.
fn build_clause(adql_col: &str, expr: &FilterExpr) -> String {
    let body = match &expr.op {
        FilterOp::Range { low, high } => {
            format!("{adql_col} BETWEEN {} AND {}", operand(low), operand(high))
        }
        FilterOp::Gt(v) => comparison(adql_col, ">", v),
        FilterOp::Ge(v) => comparison(adql_col, ">=", v),
        FilterOp::Lt(v) => comparison(adql_col, "<", v),
        FilterOp::Le(v) => comparison(adql_col, "<=", v),
        FilterOp::Exact(v) => comparison(adql_col, "=", v),
        FilterOp::Contains(v) => match v.parse::<f64>() {
            // A bare number stays an exact match, as the reference converter
            // has always done — `2` in the Cal. Lev. box means level 2.
            Ok(num) if num.is_finite() => format!("{adql_col} = {num}"),
            _ => format!("lower({adql_col}) LIKE {}", like_pattern(v)),
        },
    };
    if expr.negated {
        format!("NOT ({body})")
    } else {
        body
    }
}

/// Convert the active client-side column filters into an ADQL `WHERE` body
/// (the part after `WHERE`/`AND`, joined by `\nAND `). Returns an empty string
/// when nothing maps to a real column.
///
/// `filters` maps cleaned column keys → filter text. `columns` is the set of
/// raw result headers currently in the grid; it is used purely as a sanity gate
/// so a stale filter for a column that is not present in the result set is
/// ignored (never as a fallback source of column names, since the result
/// headers are SELECT aliases, not filterable table columns).
pub fn filters_to_where(filters: &HashMap<String, String>, columns: &[String]) -> String {
    if filters.is_empty() {
        return String::new();
    }

    // Cleaned keys of the columns actually present in the current results.
    let present: std::collections::HashSet<String> = columns.iter().map(|c| clean_key(c)).collect();

    // Deterministic clause ordering (HashMap iteration order is unspecified).
    let mut entries: Vec<(&String, &String)> = filters.iter().collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));

    let mut clauses: Vec<String> = Vec::new();
    for (key, text) in entries {
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        let cleaned = clean_key(key);
        let Some(adql_col) = column_to_adql(&cleaned) else {
            continue;
        };
        // If we know the result columns, only emit clauses for columns that are
        // actually part of this result set.
        if !present.is_empty() && !present.contains(&cleaned) {
            continue;
        }
        // A filter that constrains nothing on the grid must constrain nothing
        // here either — a half-typed `>` should not become a clause.
        let Some(expr) = FilterExpr::parse(text) else {
            continue;
        };
        clauses.push(build_clause(adql_col, &expr));
    }

    clauses.join("\nAND ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn empty_filters_yield_empty_where() {
        assert_eq!(filters_to_where(&HashMap::new(), &[]), "");
    }

    #[test]
    fn text_filter_becomes_case_insensitive_like() {
        let f = map(&[("collection", "CFHT")]);
        assert_eq!(
            filters_to_where(&f, &[]),
            "lower(Observation.collection) LIKE '%cfht%'"
        );
    }

    /// One filter, one column, rendered.
    fn where_for(key: &str, text: &str) -> String {
        filters_to_where(&map(&[(key, text)]), &[])
    }

    #[test]
    fn the_grammar_the_grid_speaks_is_the_grammar_the_query_speaks() {
        // The whole point of sharing `FilterExpr`: every operator the results
        // table honours has to survive "Apply filters to ADQL". Before this,
        // `!raw` became LIKE '%!raw%' — rows CONTAINING it, the exact opposite
        // of the rows on screen, with nothing to warn you.
        assert_eq!(where_for("callev", ">=2"), "Plane.calibrationLevel >= 2");
        assert_eq!(where_for("callev", "<2"), "Plane.calibrationLevel < 2");
        assert_eq!(
            where_for("inttime", "10..20"),
            "Plane.time_exposure BETWEEN 10 AND 20"
        );
        assert_eq!(
            where_for("collection", "=CFHT"),
            "lower(Observation.collection) = 'cfht'"
        );
        assert_eq!(
            where_for("collection", "!CFHT"),
            "NOT (lower(Observation.collection) LIKE '%cfht%')"
        );
        assert_eq!(
            where_for("callev", "!>=2"),
            "NOT (Plane.calibrationLevel >= 2)"
        );
    }

    #[test]
    fn a_text_comparison_is_case_insensitive_on_both_sides() {
        // The grid upper-cases both operands; `lower(col) > 'm'` is the same
        // question asked in SQL. Comparing a raw column against a lower-cased
        // literal would silently disagree with the table.
        assert_eq!(
            where_for("targetname", ">m"),
            "lower(Observation.target_name) > 'm'"
        );
    }

    #[test]
    fn a_half_typed_filter_produces_no_clause() {
        // It narrows nothing on the grid, so it must narrow nothing in the
        // query — not `LIKE '%>%'`.
        assert_eq!(where_for("callev", ">"), "");
        assert_eq!(where_for("inttime", "10.."), "");
        assert_eq!(where_for("collection", "!"), "");
    }

    #[test]
    fn a_quote_in_a_filter_cannot_break_out_of_the_literal() {
        assert_eq!(
            where_for("collection", "=O'Brien"),
            "lower(Observation.collection) = 'o''brien'"
        );
    }

    #[test]
    fn numeric_filter_becomes_exact_match() {
        let f = map(&[("callev", "2")]);
        assert_eq!(filters_to_where(&f, &[]), "Plane.calibrationLevel = 2");
    }

    #[test]
    fn multiple_filters_are_joined_with_and_in_stable_order() {
        let f = map(&[("piname", "Smith"), ("collection", "JWST")]);
        // Sorted by key: collection before piname.
        assert_eq!(
            filters_to_where(&f, &[]),
            "lower(Observation.collection) LIKE '%jwst%'\n\
             AND lower(Observation.proposal_pi) LIKE '%smith%'"
        );
    }

    #[test]
    fn unmapped_key_is_skipped() {
        let f = map(&[("something_unknown", "x")]);
        assert_eq!(filters_to_where(&f, &[]), "");
    }

    #[test]
    fn presence_gate_drops_filter_for_absent_column() {
        // Filter targets "collection" but the result set only has target name.
        let f = map(&[("collection", "CFHT")]);
        let cols = vec!["Target Name".to_string()];
        assert_eq!(filters_to_where(&f, &cols), "");
    }

    #[test]
    fn presence_gate_keeps_filter_for_present_aliased_column() {
        let f = map(&[("ra(j20000)", "150.5")]);
        let cols = vec!["RA (J2000.0)".to_string()];
        assert_eq!(
            filters_to_where(&f, &cols),
            "COORD1(CENTROID(Plane.position_bounds)) = 150.5"
        );
    }

    #[test]
    fn quotes_and_wildcards_are_escaped() {
        let f = map(&[("targetname", "O'Ryan_50%")]);
        assert_eq!(
            filters_to_where(&f, &[]),
            "lower(Observation.target_name) LIKE '%o''ryan\\_50\\%%'"
        );
    }
}
