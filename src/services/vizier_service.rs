//! Public VizieR cone-search with mirror failover.
//!
//! Port of `Services/VizierService.cs`. Builds the canonical `CIRCLE`+`CONTAINS`
//! ADQL against a catalogue's RA/Dec columns and rotates through the public
//! VizieR TAP mirrors when a host is unreachable. Only host-specific errors
//! (transport failures, timeouts, 5xx) trigger rotation — a 4xx (bad ADQL /
//! unknown catalogue) gives the same answer on every mirror, so it raises
//! immediately without wasting the remaining per-host budgets. Public service,
//! no auth.

use reqwest::Client;
use std::time::Duration;

/// Per-host budget before failover rotates to the next mirror. Two hosts fit
/// comfortably inside the tool's ~90s deadline without false-failing a slow but
/// working primary.
const PER_HOST_TIMEOUT: Duration = Duration::from_secs(20);

/// One public VizieR TAP mirror — `host` (for error messages) + canonical
/// `/sync` URL.
pub struct VizierEndpoint {
    pub host: &'static str,
    pub sync_url: &'static str,
}

/// Ordered fallback list of VizieR TAP mirrors: primary CDS, CDS's legacy alias
/// (different DNS zone), ESAC (geographically distinct operator), then the
/// China-VO HTTP mirror (last resort for when TLS itself is broken). All four
/// mirror the same catalogue corpus.
pub const ENDPOINTS: &[VizierEndpoint] = &[
    VizierEndpoint {
        host: "tap.cds.unistra.fr",
        sync_url: "https://tap.cds.unistra.fr/tap/sync",
    },
    VizierEndpoint {
        host: "tapvizier.u-strasbg.fr",
        sync_url: "https://tapvizier.u-strasbg.fr/TAPVizieR/tap/sync",
    },
    VizierEndpoint {
        host: "tapvizier.esac.esa.int",
        sync_url: "https://tapvizier.esac.esa.int/TAPVizieR/tap/sync",
    },
    VizierEndpoint {
        host: "vizier.china-vo.org",
        sync_url: "http://vizier.china-vo.org/tap/sync",
    },
];

/// Parsed cone-search response: the CSV header row plus data rows.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VizierConeResult {
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

/// Distinguishes "this host is broken, try the next mirror" from "this query is
/// wrong, every mirror will say the same".
enum QueryError {
    /// Transport failure / timeout / 5xx — rotate to the next mirror.
    Host(String),
    /// 4xx / definitive — stop, don't retry other mirrors.
    Query(String),
}

pub struct VizierService {
    client: Client,
}

impl VizierService {
    pub fn new(client: Client) -> Self {
        VizierService { client }
    }

    /// The canonical VizieR cone-search ADQL (byte-compatible with the reference
    /// C#/macOS clients).
    pub fn build_adql(
        catalogue: &str,
        ra_deg: f64,
        dec_deg: f64,
        radius_deg: f64,
        ra_column: &str,
        dec_column: &str,
        max_rec: usize,
    ) -> String {
        format!(
            "SELECT TOP {max_rec} *\n\
             FROM \"{catalogue}\"\n\
             WHERE 1 = CONTAINS(\n    \
             POINT('ICRS', {ra_column}, {dec_column}),\n    \
             CIRCLE('ICRS', {ra_deg}, {dec_deg}, {radius_deg})\n)"
        )
    }

    /// Cone-search a VizieR catalogue, rotating through [`ENDPOINTS`] on
    /// host-specific failures. Returns the parsed rows, or a human-readable
    /// error string describing the failover path.
    pub async fn cone_search(
        &self,
        catalogue: &str,
        ra_deg: f64,
        dec_deg: f64,
        radius_deg: f64,
        ra_column: &str,
        dec_column: &str,
        max_rec: usize,
    ) -> Result<VizierConeResult, String> {
        let adql = Self::build_adql(
            catalogue, ra_deg, dec_deg, radius_deg, ra_column, dec_column, max_rec,
        );

        let mut attempts: Vec<(String, String)> = Vec::new();
        for ep in ENDPOINTS {
            match self.query_once(ep.sync_url, &adql, max_rec).await {
                Ok(csv) => return Ok(parse_csv(&csv)),
                Err(QueryError::Query(msg)) => {
                    return Err(format!(
                        "vizier_cone_search at {}: {} — not retrying other mirrors \
                         (looks like a query problem, not a host problem).",
                        ep.host, msg
                    ));
                }
                Err(QueryError::Host(msg)) => {
                    attempts.push((ep.host.to_string(), msg));
                }
            }
        }

        let tried = attempts
            .iter()
            .map(|(h, _)| h.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let last_reason = attempts
            .last()
            .map(|(_, e)| e.as_str())
            .unwrap_or("unknown");
        Err(format!(
            "vizier_cone_search exhausted all VizieR mirrors [{tried}]; last error: {last_reason}. \
             VizieR may be globally degraded — retry in a few minutes, or use astroquery from \
             inside a Skaha session as a workaround."
        ))
    }

    /// One sync TAP POST against one mirror (same form encoding as the reference
    /// TAP clients: `REQUEST` is mandatory on TAPVizieR).
    async fn query_once(
        &self,
        url: &str,
        adql: &str,
        max_rec: usize,
    ) -> Result<String, QueryError> {
        let max_rec_str = max_rec.to_string();
        let form: Vec<(&str, &str)> = vec![
            ("REQUEST", "doQuery"),
            ("LANG", "ADQL"),
            ("FORMAT", "csv"),
            ("MAXREC", max_rec_str.as_str()),
            ("QUERY", adql),
        ];

        let resp = self
            .client
            .post(url)
            .form(&form)
            .timeout(PER_HOST_TIMEOUT)
            .send()
            .await
            // Transport / timeout: this host is the problem, try the next.
            .map_err(|e| QueryError::Host(e.to_string()))?;

        let status = resp.status();
        if status.is_success() {
            resp.text()
                .await
                .map_err(|e| QueryError::Host(e.to_string()))
        } else {
            let code = status.as_u16();
            let body = resp.text().await.unwrap_or_default();
            let msg = format!("VizieR query failed ({code}): {}", clip(&body));
            if code >= 500 {
                Err(QueryError::Host(msg))
            } else {
                Err(QueryError::Query(msg))
            }
        }
    }
}

/// Clip an error body so a hostile/verbose server can't blow up the message.
fn clip(s: &str) -> String {
    // Clip on a char boundary (byte-slicing a multi-byte body would panic).
    if s.chars().count() <= 300 {
        s.to_string()
    } else {
        s.chars().take(300).collect()
    }
}

/// Parse a TAP CSV response into headers + rows. Rows whose column count differs
/// from the header count are skipped (matches the reference parser).
fn parse_csv(csv: &str) -> VizierConeResult {
    let normalized = csv.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = normalized.split('\n').filter(|l| !l.is_empty()).collect();
    if lines.is_empty() {
        return VizierConeResult::default();
    }

    let headers = parse_csv_line(lines[0]);
    let mut rows = Vec::new();
    for line in &lines[1..] {
        let values = parse_csv_line(line);
        if values.len() == headers.len() {
            rows.push(values);
        }
    }
    VizierConeResult { headers, rows }
}

/// Parse one CSV line: quoted fields, `""` escapes an embedded quote, fields are
/// trimmed.
fn parse_csv_line(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut in_quotes = false;
    let mut field = String::new();
    let chars: Vec<char> = line.chars().collect();

    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '"' {
            if in_quotes && i + 1 < chars.len() && chars[i + 1] == '"' {
                field.push('"');
                i += 1;
            } else {
                in_quotes = !in_quotes;
            }
        } else if c == ',' && !in_quotes {
            fields.push(field.trim().to_string());
            field.clear();
        } else {
            field.push(c);
        }
        i += 1;
    }
    fields.push(field.trim().to_string());
    fields
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adql_is_canonical() {
        let adql =
            VizierService::build_adql("V/97/catalog", 10.5, 41.2, 0.05, "RAJ2000", "DEJ2000", 500);
        assert!(adql.starts_with("SELECT TOP 500 *"));
        assert!(adql.contains("FROM \"V/97/catalog\""));
        assert!(adql.contains("POINT('ICRS', RAJ2000, DEJ2000)"));
        assert!(adql.contains("CIRCLE('ICRS', 10.5, 41.2, 0.05)"));
    }

    #[test]
    fn parse_csv_headers_and_rows() {
        let csv = "RAJ2000,DEJ2000,Vmag\r\n10.5,41.2,12.3\r\n11.0,40.9,13.1\r\n";
        let result = parse_csv(csv);
        assert_eq!(result.headers, vec!["RAJ2000", "DEJ2000", "Vmag"]);
        assert_eq!(result.rows.len(), 2);
        assert_eq!(result.rows[0], vec!["10.5", "41.2", "12.3"]);
        assert_eq!(result.rows[1][2], "13.1");
    }

    #[test]
    fn parse_csv_skips_ragged_rows() {
        let csv = "a,b,c\n1,2,3\n4,5\n6,7,8,9\n";
        let result = parse_csv(csv);
        // Only the well-formed 3-column row survives.
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0], vec!["1", "2", "3"]);
    }

    #[test]
    fn parse_csv_handles_quoted_and_escaped_fields() {
        let csv = "name,note\n\"M 31\",\"has, comma\"\n\"a\"\"b\",plain\n";
        let result = parse_csv(csv);
        assert_eq!(result.rows[0], vec!["M 31", "has, comma"]);
        assert_eq!(result.rows[1], vec!["a\"b", "plain"]);
    }

    #[test]
    fn parse_csv_empty_is_empty() {
        assert_eq!(parse_csv(""), VizierConeResult::default());
        assert_eq!(parse_csv("\n\n"), VizierConeResult::default());
    }

    #[test]
    fn clip_truncates_long_bodies_on_char_boundary() {
        let long = "é".repeat(400);
        let clipped = clip(&long);
        assert_eq!(clipped.chars().count(), 300);
        assert_eq!(clip("short"), "short");
    }

    #[test]
    fn endpoints_cover_the_expected_mirrors() {
        assert_eq!(ENDPOINTS.len(), 4);
        assert_eq!(ENDPOINTS[0].host, "tap.cds.unistra.fr");
    }
}
