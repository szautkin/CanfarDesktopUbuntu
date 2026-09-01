//! Catch the ADQL mistakes the service would reject, before the round trip.
//!
//! Not a parser, and deliberately not: ADQL is a dialect of SQL and a parser
//! for it that is 95% right is a parser that refuses good queries, which is
//! worse than sending them. This reads two things out of the text — what the
//! FROM clause names, and every `qualifier.column` reference — and applies the
//! rules the service applies to those.
//!
//! The rule that prompted it, confirmed against CADC:
//!
//! ```text
//! FROM caom2.Observation JOIN caom2.Plane ON Plane.obsID=Observation.obsID
//!   → Server error (400): Column [obsID] is ambiguous.
//!
//! FROM caom2.Observation AS o JOIN caom2.Plane AS p ON p.obsID=o.obsID   → ok
//! FROM caom2.Observation JOIN caom2.Plane ON caom2.Plane.obsID=…         → ok
//! ```
//!
//! A bare table name is an acceptable qualifier when the column it names
//! belongs to only one of the joined tables — `Observation.observationID` in
//! that same query was fine — so "always write an alias" is not the rule and
//! reporting it as one would flag working queries.
//!
//! **Only confident problems are reported.** Anything this cannot resolve —
//! a subquery, a function, a table the schema has not been fetched for — is
//! left alone. A false positive here disables the Execute button on a query
//! that would have worked, which is a worse failure than the one being
//! prevented.

use crate::services::tap_schema_service::TapSchema;

/// One thing wrong with a query, and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// Byte range of the offending text, for the editor to mark.
    pub start: usize,
    pub end: usize,
    /// What is wrong, in the words a person needs.
    pub message: String,
    /// What to write instead, when there is a single obvious answer.
    pub fix: Option<String>,
}

/// A table named in FROM/JOIN: how it was written, and its alias if it has one.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FromEntry {
    /// As written — `caom2.Plane`.
    written: String,
    /// The part after the last dot — `Plane`.
    bare: String,
    alias: Option<String>,
}

/// Everything wrong with `adql`, in the order it appears.
///
/// An empty list means "nothing this can be sure about", not "valid".
pub fn problems(adql: &str, schema: &TapSchema) -> Vec<Problem> {
    // Nothing is knowable without the schema, and "unknown table" for every
    // table is the loudest possible way to say "I have not loaded yet". The
    // service's own tables arrive asynchronously, so this is the state the
    // editor is in for the first second of every session.
    if schema.tables.is_empty() {
        return Vec::new();
    }
    let from = from_entries(adql);
    if from.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();

    // Unknown tables first: every later rule reads columns off them, and
    // reporting a missing column of a table that does not exist is noise.
    for entry in &from {
        if schema.table(&entry.written).is_none() {
            let Some(at) = find_identifier(adql, &entry.written) else {
                continue;
            };
            out.push(Problem {
                start: at,
                end: at + entry.written.len(),
                message: format!("no table {:?} in this service", entry.written),
                fix: nearest(
                    &entry.written,
                    schema.tables.iter().map(|t| t.name.as_str()),
                ),
            });
        }
    }
    if !out.is_empty() {
        return out;
    }

    for (start, qualifier, column) in qualified_references(adql) {
        let end = start + qualifier.len() + 1 + column.len();
        // An alias resolves to its table; anything else must be a table name.
        let by_alias = from
            .iter()
            .find(|e| e.alias.as_deref().is_some_and(|a| eq(a, &qualifier)));
        let table = match by_alias {
            Some(e) => e.written.clone(),
            None => {
                let full = from.iter().find(|e| eq(&e.written, &qualifier));
                let bare = from.iter().find(|e| eq(&e.bare, &qualifier));
                match (full, bare) {
                    (Some(e), _) => e.written.clone(),
                    // A bare table name — legal only while the column it names
                    // is unambiguous across the joined tables.
                    (None, Some(e)) => {
                        let owners: Vec<&str> = from
                            .iter()
                            .filter(|other| has_column(schema, &other.written, &column))
                            .map(|other| other.written.as_str())
                            .collect();
                        if owners.len() > 1 {
                            out.push(Problem {
                                start,
                                end,
                                message: format!(
                                    "{qualifier}.{column} is ambiguous — {column} is in {}",
                                    owners.join(" and ")
                                ),
                                fix: Some(format!("{}.{column}", e.written)),
                            });
                            continue;
                        }
                        e.written.clone()
                    }
                    // Not an alias and not a table in FROM: a subquery, a
                    // function, or a genuine typo. Not confident either way.
                    (None, None) => continue,
                }
            }
        };
        if let Some(t) = schema.table(&table) {
            if !t.columns.iter().any(|c| eq(&c.name, &column)) {
                out.push(Problem {
                    start,
                    end,
                    message: format!("{table} has no column {column:?}"),
                    fix: nearest(&column, t.columns.iter().map(|c| c.name.as_str()))
                        .map(|n| format!("{qualifier}.{n}")),
                });
            }
        }
    }
    out
}

fn eq(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

fn has_column(schema: &TapSchema, table: &str, column: &str) -> bool {
    schema
        .table(table)
        .is_some_and(|t| t.columns.iter().any(|c| eq(&c.name, column)))
}

/// The closest known name to `given`, when one is clearly closest.
///
/// A prefix or a case difference only — not an edit distance. "Did you mean"
/// on a guess is worse than no suggestion, and the two mistakes people
/// actually make here are the wrong case and a truncated name.
fn nearest<'a>(given: &str, known: impl Iterator<Item = &'a str>) -> Option<String> {
    let lower = given.to_ascii_lowercase();
    let mut hits: Vec<&str> = known
        .filter(|k| {
            let k = k.to_ascii_lowercase();
            k == lower || k.starts_with(&lower) || lower.starts_with(&k)
        })
        .collect();
    hits.sort_unstable();
    hits.dedup();
    (hits.len() == 1).then(|| hits[0].to_string())
}

/// The tables a query selects from, with their aliases.
///
/// Reads the words after FROM and each JOIN. Stops at the first keyword that
/// ends the clause, so a WHERE or an ON is not mistaken for a table.
fn from_entries(adql: &str) -> Vec<FromEntry> {
    const ENDS: &[&str] = &[
        "where", "on", "group", "order", "having", "limit", "using", "select",
    ];
    let mut out = Vec::new();
    let words: Vec<&str> = adql.split_whitespace().collect();
    let mut i = 0;
    while i < words.len() {
        let w = words[i].trim_matches(|c: char| c == '(' || c == ')');
        if eq(w, "from") || eq(w, "join") {
            i += 1;
            let Some(name) = words.get(i) else { break };
            let written = name.trim_matches(|c: char| c == '(' || c == ')' || c == ',');
            if written.is_empty() || ENDS.iter().any(|k| eq(k, written)) {
                continue;
            }
            // `AS x`, or a bare `x` that is not a keyword.
            let mut alias = None;
            if let Some(next) = words.get(i + 1) {
                if eq(next, "as") {
                    alias = words.get(i + 2).map(|a| a.trim_matches(',').to_string());
                    i += 2;
                } else {
                    let n = next.trim_matches(',');
                    if !n.is_empty()
                        && !ENDS.iter().any(|k| eq(k, n))
                        && !eq(n, "join")
                        && !eq(n, "inner")
                        && !eq(n, "left")
                        && !eq(n, "right")
                        && n.chars().all(|c| c.is_alphanumeric() || c == '_')
                    {
                        alias = Some(n.to_string());
                        i += 1;
                    }
                }
            }
            let bare = written.rsplit('.').next().unwrap_or(written).to_string();
            out.push(FromEntry {
                written: written.to_string(),
                bare,
                alias,
            });
        }
        i += 1;
    }
    out
}

/// Every `qualifier.column` in the text, with the byte offset of the qualifier.
///
/// A three-part `caom2.Plane.obsID` yields `caom2.Plane` as the qualifier,
/// which is what the service treats it as.
fn qualified_references(adql: &str) -> Vec<(usize, String, String)> {
    let bytes = adql.as_bytes();
    let ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if !ident(bytes[i]) || (i > 0 && (ident(bytes[i - 1]) || bytes[i - 1] == b'.')) {
            i += 1;
            continue;
        }
        let start = i;
        let mut parts: Vec<&str> = Vec::new();
        loop {
            let s = i;
            while i < bytes.len() && ident(bytes[i]) {
                i += 1;
            }
            if i == s {
                break;
            }
            parts.push(&adql[s..i]);
            if i < bytes.len() && bytes[i] == b'.' {
                i += 1;
            } else {
                break;
            }
        }
        if parts.len() >= 2 {
            let column = parts[parts.len() - 1].to_string();
            let qualifier = parts[..parts.len() - 1].join(".");
            out.push((start, qualifier, column));
        }
    }
    out
}

/// The identifier `needle` in `haystack`, as a whole word.
fn find_identifier(haystack: &str, needle: &str) -> Option<usize> {
    let lower = haystack.to_ascii_lowercase();
    let target = needle.to_ascii_lowercase();
    let mut from = 0;
    while let Some(rel) = lower[from..].find(&target) {
        let at = from + rel;
        let before_ok = at == 0 || !lower.as_bytes()[at - 1].is_ascii_alphanumeric();
        let after = at + target.len();
        let after_ok = after >= lower.len() || !lower.as_bytes()[after].is_ascii_alphanumeric();
        if before_ok && after_ok {
            return Some(at);
        }
        from = at + 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::tap_schema_service::{TapColumn, TapSchema, TapTable};

    fn col(name: &str) -> TapColumn {
        TapColumn {
            name: name.to_string(),
            datatype: String::new(),
            description: String::new(),
            unit: String::new(),
            ucd: String::new(),
        }
    }

    /// The two CAOM2 tables this is all about, with the columns that matter.
    fn caom2() -> TapSchema {
        TapSchema {
            tables: vec![
                TapTable {
                    name: "caom2.Observation".into(),
                    description: String::new(),
                    columns: vec![col("obsID"), col("observationID"), col("collection")],
                },
                TapTable {
                    name: "caom2.Plane".into(),
                    description: String::new(),
                    columns: vec![col("obsID"), col("planeID"), col("energy_bandpassName")],
                },
            ],
            keys: Vec::new(),
        }
    }

    /// The query from the report, and the reason the service refused it.
    ///
    /// `obsID` is on both tables, so a bare `Plane.obsID` names a column the
    /// service cannot resolve to one of them.
    #[test]
    fn a_bare_table_name_on_a_shared_column_is_ambiguous() {
        let adql = "SELECT TOP 5 Observation.observationID FROM caom2.Observation \
                    JOIN caom2.Plane ON Plane.obsID=Observation.obsID \
                    WHERE Observation.collection='JWST'";
        let found = problems(adql, &caom2());
        assert_eq!(
            found.len(),
            2,
            "both halves of the ON are ambiguous: {found:#?}"
        );
        assert!(found[0].message.contains("ambiguous"), "{:?}", found[0]);
        assert_eq!(found[0].fix.as_deref(), Some("caom2.Plane.obsID"));
        assert_eq!(&adql[found[0].start..found[0].end], "Plane.obsID");
    }

    /// `Observation.observationID` in that same query is FINE.
    ///
    /// The service accepted it, because `observationID` is on one table only.
    /// Reporting "always write an alias" would flag a working reference, and a
    /// validator that refuses good queries is worse than the error it prevents.
    #[test]
    fn a_bare_table_name_on_a_unique_column_is_left_alone() {
        let adql = "SELECT Observation.observationID FROM caom2.Observation \
                    JOIN caom2.Plane ON caom2.Plane.obsID=caom2.Observation.obsID";
        assert_eq!(problems(adql, &caom2()), Vec::new());
    }

    /// The two spellings the service accepts are both accepted here.
    #[test]
    fn aliases_and_full_names_are_accepted() {
        for adql in [
            "SELECT o.observationID FROM caom2.Observation AS o \
             JOIN caom2.Plane AS p ON p.obsID=o.obsID",
            "SELECT caom2.Observation.observationID FROM caom2.Observation \
             JOIN caom2.Plane ON caom2.Plane.obsID=caom2.Observation.obsID",
            // Alias without AS.
            "SELECT o.obsID FROM caom2.Observation o JOIN caom2.Plane p ON p.obsID=o.obsID",
        ] {
            assert_eq!(problems(adql, &caom2()), Vec::new(), "refused: {adql}");
        }
    }

    /// A column that is not on the table it is qualified by.
    #[test]
    fn a_column_the_table_does_not_have_is_named() {
        let adql = "SELECT o.bandpass FROM caom2.Observation AS o";
        let found = problems(adql, &caom2());
        assert_eq!(found.len(), 1, "{found:#?}");
        assert!(found[0].message.contains("no column"), "{:?}", found[0]);
        assert_eq!(&adql[found[0].start..found[0].end], "o.bandpass");
    }

    /// A table the service does not have, with the near miss named.
    #[test]
    fn an_unknown_table_is_named_with_the_closest_real_one() {
        let adql = "SELECT p.obsID FROM caom2.Plan AS p";
        let found = problems(adql, &caom2());
        assert_eq!(found.len(), 1, "{found:#?}");
        assert!(found[0].message.contains("no table"), "{:?}", found[0]);
        assert_eq!(found[0].fix.as_deref(), Some("caom2.Plane"));
    }

    /// Anything it cannot resolve is left alone.
    ///
    /// A subquery alias, a function call, a schema that has not been fetched:
    /// each would be a false positive, and a false positive here greys out the
    /// Execute button on a query that works.
    #[test]
    fn what_it_cannot_resolve_it_does_not_report() {
        let empty = TapSchema::default();
        // No schema yet: nothing is knowable.
        assert_eq!(
            problems("SELECT x.y FROM caom2.Observation AS x", &empty),
            Vec::new()
        );
        // A qualifier that is neither an alias nor a table in FROM.
        assert_eq!(
            problems(
                "SELECT sub.obsID FROM (SELECT obsID FROM caom2.Plane) AS whatever",
                &caom2()
            ),
            Vec::new()
        );
        // No FROM at all.
        assert_eq!(problems("SELECT 1", &caom2()), Vec::new());
    }

    /// Case is not a mistake. ADQL identifiers are case-insensitive unquoted.
    #[test]
    fn case_does_not_make_a_query_wrong() {
        let adql = "select O.OBSID from CAOM2.OBSERVATION as O";
        assert_eq!(problems(adql, &caom2()), Vec::new());
    }
}
