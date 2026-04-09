/// Range parsing for ADQL constraints.
/// Supports: "A..B" (between), ">= X", "<= X", "> X", "< X", "X" (equals)

#[derive(Debug, Clone, PartialEq)]
pub enum RangeOp {
    Equals,
    LessThan,
    LessThanOrEqual,
    GreaterThan,
    GreaterThanOrEqual,
    Between,
}

#[derive(Debug, Clone)]
pub struct ParsedRange {
    pub op: RangeOp,
    pub value1: String,
    pub value2: Option<String>,
}

/// Parse a range expression. Returns None if input is empty/whitespace.
pub fn parse_range(input: &str) -> Option<ParsedRange> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }

    // Between: "A..B"
    if let Some(dot_idx) = input.find("..") {
        let left = input[..dot_idx].trim().to_string();
        let right = input[dot_idx + 2..].trim().to_string();
        if !left.is_empty() && !right.is_empty() {
            return Some(ParsedRange {
                op: RangeOp::Between,
                value1: left,
                value2: Some(right),
            });
        }
    }

    // <=
    if let Some(rest) = input.strip_prefix("<=") {
        let v = rest.trim().to_string();
        if !v.is_empty() {
            return Some(ParsedRange {
                op: RangeOp::LessThanOrEqual,
                value1: v,
                value2: None,
            });
        }
    }

    // >=
    if let Some(rest) = input.strip_prefix(">=") {
        let v = rest.trim().to_string();
        if !v.is_empty() {
            return Some(ParsedRange {
                op: RangeOp::GreaterThanOrEqual,
                value1: v,
                value2: None,
            });
        }
    }

    // <
    if let Some(rest) = input.strip_prefix('<') {
        let v = rest.trim().to_string();
        if !v.is_empty() {
            return Some(ParsedRange {
                op: RangeOp::LessThan,
                value1: v,
                value2: None,
            });
        }
    }

    // >
    if let Some(rest) = input.strip_prefix('>') {
        let v = rest.trim().to_string();
        if !v.is_empty() {
            return Some(ParsedRange {
                op: RangeOp::GreaterThan,
                value1: v,
                value2: None,
            });
        }
    }

    // Plain value (equals)
    Some(ParsedRange {
        op: RangeOp::Equals,
        value1: input.to_string(),
        value2: None,
    })
}

/// Extract trailing unit suffix from a value string.
/// Returns (numeric_part, unit) or (original, None).
pub fn extract_unit_suffix(raw: &str) -> (&str, Option<&str>) {
    let suffixes = [
        "GHz", "MHz", "kHz", "GeV", "MeV", "keV", "arcsec", "arcmin", "nm", "um", "mm", "cm", "Hz",
        "eV", "deg", "s", "m", "h", "d",
    ];
    let trimmed = raw.trim();
    for suffix in &suffixes {
        if let Some(prefix) = trimmed.strip_suffix(suffix) {
            let prefix = prefix.trim();
            if !prefix.is_empty() {
                return (prefix, Some(suffix));
            }
        }
    }
    (trimmed, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_between() {
        let r = parse_range("100..200").unwrap();
        assert_eq!(r.op, RangeOp::Between);
        assert_eq!(r.value1, "100");
        assert_eq!(r.value2.as_deref(), Some("200"));
    }

    #[test]
    fn parse_greater_than() {
        let r = parse_range("> 50").unwrap();
        assert_eq!(r.op, RangeOp::GreaterThan);
        assert_eq!(r.value1, "50");
    }

    #[test]
    fn parse_less_equal() {
        let r = parse_range("<= 1000").unwrap();
        assert_eq!(r.op, RangeOp::LessThanOrEqual);
        assert_eq!(r.value1, "1000");
    }

    #[test]
    fn parse_equals() {
        let r = parse_range("42").unwrap();
        assert_eq!(r.op, RangeOp::Equals);
        assert_eq!(r.value1, "42");
    }

    #[test]
    fn parse_empty() {
        assert!(parse_range("").is_none());
        assert!(parse_range("   ").is_none());
    }

    #[test]
    fn extract_suffix_nm() {
        let (num, unit) = extract_unit_suffix("500nm");
        assert_eq!(num, "500");
        assert_eq!(unit, Some("nm"));
    }

    #[test]
    fn extract_suffix_none() {
        let (num, unit) = extract_unit_suffix("500");
        assert_eq!(num, "500");
        assert_eq!(unit, None);
    }

    #[test]
    fn extract_suffix_arcsec() {
        let (num, unit) = extract_unit_suffix("0.5arcsec");
        assert_eq!(num, "0.5");
        assert_eq!(unit, Some("arcsec"));
    }
}
