//! Turning a TAP rejection into something an agent can act on.
//!
//! The service is precise and unhelpful in equal measure. Asked for a column
//! that is not there it answers
//!
//! ```text
//! validateColumnNonAlias: Column: [collection_name] does not exist.
//! ```
//!
//! which says what is wrong and nothing about what is right, so the caller's
//! next move is another guess. Two guesses in a row is how an agent ends up
//! reaching for `curl`.
//!
//! Both failures seen in QA are diagnosable from the text alone:
//!
//!  * **A string literal in double quotes.** `WHERE collection LIKE "%JWST%"`
//!    is not a comparison against text — in ADQL, as in SQL, double quotes
//!    delimit an IDENTIFIER, so the service looks for a column named `%JWST%`
//!    and reports it missing. Verified against CADC: it answers
//!    `Column: [""%JWST%""] does not exist`. Single quotes are the fix.
//!  * **The other schema's column names.** `collection_name`, `obs_id`,
//!    `calibration_level` are ObsCore spellings; `caom2.Observation` calls them
//!    `collection` and `observationID`, and `calibrationLevel` lives on
//!    `caom2.Plane` — a different table, needing a JOIN. Nothing here guesses
//!    the right name: `describe_tap_schema` knows it, and this points at it,
//!    naming the table the query actually used.
//!
//! No column list is hard-coded. A table of names copied out of CADC would be
//! wrong eventually and confidently, which is worse than saying "ask the
//! service".

/// The column a `does not exist` rejection names, if it is one.
fn missing_column(raw: &str) -> Option<String> {
    let at = raw.find("Column: [")? + "Column: [".len();
    let rest = &raw[at..];
    let end = rest.find(']')?;
    Some(rest[..end].trim_matches('"').trim().to_string())
}

/// Whether `name` looks like text someone meant as a value, not a column.
///
/// A wildcard or a space never appears in a real column name, and both are
/// everywhere in the literals people write.
fn looks_like_a_literal(name: &str) -> bool {
    name.contains('%') || name.contains(' ') || name.contains('*')
}

/// The tables an ADQL statement selects from.
///
/// Deliberately shallow: enough to say "ask about THIS table", not a parser.
/// A name it misses costs a slightly vaguer message and nothing else.
fn tables_in(adql: &str) -> Vec<String> {
    let lower = adql.to_lowercase();
    let mut out = Vec::new();
    for keyword in ["from ", "join "] {
        let mut from = 0;
        while let Some(at) = lower[from..].find(keyword) {
            let start = from + at + keyword.len();
            let name: String = adql[start..]
                .chars()
                .skip_while(|c| c.is_whitespace())
                .take_while(|c| c.is_alphanumeric() || *c == '.' || *c == '_')
                .collect();
            if !name.is_empty() && !out.contains(&name) {
                out.push(name);
            }
            from = start;
        }
    }
    out
}

/// A TAP error with the next step attached.
///
/// Returns `raw` unchanged when there is nothing useful to add — an error that
/// gains a paragraph of guesswork is worse than the error.
pub fn explain(raw: &str, adql: &str) -> String {
    let Some(column) = missing_column(raw) else {
        return raw.to_string();
    };

    if looks_like_a_literal(&column) {
        return format!(
            "{raw}\nADQL read `{column}` as a column name because it is in double quotes. \
             Double quotes mark an IDENTIFIER; text values take single quotes — \
             write '{column}', not \"{column}\"."
        );
    }

    let tables = tables_in(adql);
    let which = match tables.len() {
        0 => "the table you queried".to_string(),
        1 => format!("`{}`", tables[0]),
        _ => format!(
            "`{}`",
            tables
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join("` and `")
        ),
    };
    format!(
        "{raw}\nThere is no `{column}` in {which}. Call describe_tap_schema for that table's \
         real column names rather than guessing again — caom2 and ivoa.ObsCore spell the same \
         ideas differently (ObsCore's `obs_collection` / `calib_level` are caom2's `collection` \
         / `calibrationLevel`), and several caom2 columns live on `caom2.Plane`, which needs a \
         JOIN from `caom2.Observation`."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact rejection from the QA run.
    const REAL: &str = "validateColumnNonAlias: Column: [collection_name] does not exist.\n";

    #[test]
    fn a_missing_column_is_recognised() {
        assert_eq!(missing_column(REAL).as_deref(), Some("collection_name"));
        assert_eq!(missing_column("something else entirely"), None);
    }

    /// The message names the table the query used, and where to look.
    #[test]
    fn a_wrong_column_name_points_at_the_schema_tool() {
        let out = explain(
            REAL,
            "SELECT TOP 50 collection_name FROM caom2.Observation WHERE x = 1",
        );
        assert!(out.contains("collection_name"), "{out}");
        assert!(out.contains("caom2.Observation"), "{out}");
        assert!(out.contains("describe_tap_schema"), "{out}");
        // The original text survives — the caller may be matching on it.
        assert!(out.starts_with(REAL), "{out}");
    }

    /// A double-quoted literal is diagnosed as quoting, not as a bad column.
    ///
    /// Verified against CADC: `WHERE collection LIKE "%JWST%"` answers
    /// `Column: [""%JWST%""] does not exist`, which reads like a schema problem
    /// and is a punctuation one.
    #[test]
    fn a_double_quoted_literal_is_diagnosed_as_quoting() {
        let raw = "validateColumnNonAlias: Column: [\"%JWST%\"] does not exist.";
        let out = explain(
            raw,
            "SELECT collection FROM caom2.Observation WHERE collection LIKE \"%JWST%\"",
        );
        assert!(out.contains("single quotes"), "{out}");
        assert!(
            !out.contains("describe_tap_schema"),
            "a quoting mistake was sent to the schema tool: {out}"
        );
    }

    #[test]
    fn a_join_names_every_table() {
        let out = explain(
            REAL,
            "SELECT x FROM caom2.Observation AS o JOIN caom2.Plane AS p ON o.obsID = p.obsID",
        );
        assert!(out.contains("caom2.Observation"), "{out}");
        assert!(out.contains("caom2.Plane"), "{out}");
    }

    /// An error that is not about a column is left alone.
    #[test]
    fn an_unrelated_error_is_not_embellished() {
        let raw = "Server error (500): unexpected exception";
        assert_eq!(explain(raw, "SELECT 1"), raw);
    }

    /// A query whose FROM cannot be read still gets the useful half.
    #[test]
    fn an_unparsed_query_still_names_the_remedy() {
        let out = explain(REAL, "");
        assert!(out.contains("describe_tap_schema"), "{out}");
        assert!(out.contains("the table you queried"), "{out}");
    }

    #[test]
    fn a_literal_is_told_apart_from_a_column() {
        assert!(looks_like_a_literal("%JWST%"));
        assert!(looks_like_a_literal("NGC 5194"));
        assert!(!looks_like_a_literal("collection_name"));
        assert!(!looks_like_a_literal("targetPosition_coordinates_cval1"));
    }
}
