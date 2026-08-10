//! Lightweight ADQL parser that extracts a human-readable summary.
//!
//! Used by the search page to show a meaningful preview of saved queries
//! and recent searches instead of dumping the raw query text.

/// A parsed summary of an ADQL query.
#[derive(Debug, Clone, Default)]
pub struct AdqlSummary {
    pub table: Option<String>,
    pub filter_count: usize,
    pub first_filter: Option<String>,
    pub has_order_by: bool,
}

/// Parse an ADQL query into a summary.
pub fn parse(adql: &str) -> AdqlSummary {
    let lower = adql.to_lowercase();
    let mut summary = AdqlSummary::default();

    // Extract the first identifier after " from "
    if let Some(from_idx) = find_keyword(&lower, "from") {
        let after = &adql[from_idx..];
        // Skip leading whitespace
        let trimmed = after.trim_start();
        // Grab identifier characters (letters, digits, underscore, dot)
        let table: String = trimmed
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '.')
            .collect();
        if !table.is_empty() {
            summary.table = Some(table);
        }
    }

    // Extract the WHERE clause up to ORDER BY / GROUP BY / HAVING / end-of-query
    if let Some(where_start) = find_keyword(&lower, "where") {
        let rest_lower = &lower[where_start..];
        let rest_orig = &adql[where_start..];
        let mut end = rest_lower.len();
        for stop in &["order by", "group by", "having"] {
            if let Some(idx) = find_keyword(rest_lower, stop) {
                if idx < end {
                    end = idx;
                }
            }
        }
        let where_clause = rest_orig[..end].trim();

        if !where_clause.is_empty() {
            // Count top-level AND/OR keywords (naive — doesn't track parens/strings,
            // but adequate for preview purposes).
            let clause_lower = where_clause.to_lowercase();
            let and_count = count_word_occurrences(&clause_lower, "and");
            let or_count = count_word_occurrences(&clause_lower, "or");
            summary.filter_count = 1 + and_count + or_count;

            // First condition — everything up to the first AND/OR
            let first_sep = clause_lower
                .find(" and ")
                .or_else(|| clause_lower.find(" or "));
            let first = match first_sep {
                Some(i) => where_clause[..i].trim().to_string(),
                None => where_clause.trim().to_string(),
            };
            // Collapse whitespace for display
            let compact = first.split_whitespace().collect::<Vec<_>>().join(" ");
            if !compact.is_empty() {
                summary.first_filter = Some(compact);
            }
        }
    }

    if find_keyword(&lower, "order by").is_some() {
        summary.has_order_by = true;
    }

    summary
}

/// Produce a compact one-line summary suitable for a subtitle.
pub fn short_summary(adql: &str) -> String {
    let s = parse(adql);

    let table = s.table.as_deref().unwrap_or("query");
    let table = simplify_table(table);

    if s.filter_count == 0 {
        return table.to_string();
    }

    let filter_suffix = if s.filter_count == 1 {
        "1 filter".to_string()
    } else {
        format!("{} filters", s.filter_count)
    };

    format!("{} · {}", table, filter_suffix)
}

/// Format an RFC3339 timestamp as "HH:MM" in local time, or fall back to the raw string.
pub fn format_saved_at(rfc3339: &str) -> String {
    use chrono::{DateTime, Local};

    match rfc3339.parse::<DateTime<chrono::Utc>>() {
        Ok(utc) => {
            let local: DateTime<Local> = utc.into();
            local.format("%b %d, %H:%M").to_string()
        }
        Err(_) => rfc3339.to_string(),
    }
}

/// Simplify "caom2.Observation" to just "Observation" for a tighter display.
fn simplify_table(full: &str) -> &str {
    full.rsplit_once('.').map(|(_, t)| t).unwrap_or(full)
}

/// Find the byte index of the first occurrence of a keyword as a whole word.
/// Case-insensitive — `lower` must be the lowercased haystack.
/// Returns the index *after* the keyword on success.
fn find_keyword(lower: &str, keyword: &str) -> Option<usize> {
    let kw_len = keyword.len();
    let mut search_start = 0;
    while let Some(idx) = lower[search_start..].find(keyword) {
        let abs = search_start + idx;
        let before_ok = abs == 0 || is_word_boundary(&lower[..abs], true);
        let after_idx = abs + kw_len;
        let after_ok = after_idx == lower.len() || is_word_boundary(&lower[after_idx..], false);
        if before_ok && after_ok {
            return Some(after_idx);
        }
        search_start = abs + 1;
    }
    None
}

fn is_word_boundary(slice: &str, end: bool) -> bool {
    let ch = if end {
        slice.chars().next_back()
    } else {
        slice.chars().next()
    };
    match ch {
        None => true,
        Some(c) => !c.is_alphanumeric() && c != '_',
    }
}

/// Count occurrences of `word` as a whole word inside `lower`.
fn count_word_occurrences(lower: &str, word: &str) -> usize {
    let mut count = 0;
    let mut start = 0;
    while let Some(idx) = lower[start..].find(word) {
        let abs = start + idx;
        let before_ok = abs == 0 || is_word_boundary(&lower[..abs], true);
        let after_idx = abs + word.len();
        let after_ok = after_idx == lower.len() || is_word_boundary(&lower[after_idx..], false);
        if before_ok && after_ok {
            count += 1;
        }
        start = abs + 1;
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_extraction_basic() {
        let s = parse("SELECT * FROM caom2.Observation WHERE 1=1");
        assert_eq!(s.table.as_deref(), Some("caom2.Observation"));
    }

    #[test]
    fn counts_multiple_filters() {
        let s = parse("SELECT * FROM t WHERE a = 1 AND b = 2 AND c = 3 ORDER BY a");
        assert_eq!(s.filter_count, 3);
        assert!(s.has_order_by);
    }

    #[test]
    fn first_filter_extracted() {
        let s = parse("SELECT * FROM obs WHERE target_name = 'M31' AND instrument='ACS'");
        assert_eq!(s.first_filter.as_deref(), Some("target_name = 'M31'"));
    }

    #[test]
    fn zero_filters_no_where() {
        let s = parse("SELECT * FROM obs");
        assert_eq!(s.filter_count, 0);
        assert!(s.first_filter.is_none());
    }

    #[test]
    fn short_summary_format() {
        assert_eq!(short_summary("SELECT * FROM obs"), "obs");
        assert_eq!(
            short_summary("SELECT * FROM caom2.Observation WHERE target='M31'"),
            "Observation · 1 filter"
        );
        assert_eq!(
            short_summary("SELECT * FROM t WHERE a=1 AND b=2 AND c=3"),
            "t · 3 filters"
        );
    }

    #[test]
    fn keyword_boundaries_respected() {
        // "format" contains "for" but is not the FROM keyword
        let s = parse("SELECT format(x) FROM obs WHERE 1=1");
        assert_eq!(s.table.as_deref(), Some("obs"));
    }

    #[test]
    fn stops_at_order_by() {
        let s = parse("SELECT * FROM obs WHERE a=1 ORDER BY a DESC");
        assert_eq!(s.filter_count, 1);
        assert!(s.has_order_by);
    }
}
