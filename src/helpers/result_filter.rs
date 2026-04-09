use crate::models::search_result::SearchResultRow;
use std::collections::HashMap;

/// Filter search result rows by per-column substring match (case-insensitive AND logic).
pub fn filter_rows(
    rows: &[SearchResultRow],
    column_filters: &HashMap<String, String>,
) -> Vec<SearchResultRow> {
    if column_filters.is_empty() || column_filters.values().all(|v| v.is_empty()) {
        return rows.to_vec();
    }

    rows.iter()
        .filter(|row| {
            column_filters.iter().all(|(col, filter_text)| {
                if filter_text.is_empty() {
                    return true;
                }
                let cell = row.get(col).to_lowercase();
                let needle = filter_text.to_lowercase();
                cell.contains(&needle)
            })
        })
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
