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
/// Everything one cone search needs.
///
/// A struct rather than an eighth positional parameter: the call already took
/// seven, five of them adjacent strings and numbers that are easy to transpose,
/// and `clippy::too_many_arguments` is right about where that ends.
#[derive(Debug, Clone)]
pub struct ConeQuery<'a> {
    pub catalogue: &'a str,
    pub ra_deg: f64,
    pub dec_deg: f64,
    pub radius_deg: f64,
    pub ra_column: &'a str,
    pub dec_column: &'a str,
    pub max_rec: usize,
    /// Columns to return. Empty means every column, which is what the tool did
    /// before this existed — a Gaia DR3 cone returns ~230 columns per row, and
    /// 500 of those rows is ~760 KB, past what an agent can hold.
    pub columns: &'a [String],
}

/// The SELECT list: the named columns, or `*` when none are named.
///
/// Names are quoted because VizieR has columns an unquoted ADQL identifier
/// cannot spell — `_r`, the computed distance from the cone centre, is the one
/// callers ask for most. Anything that could end the quoting is removed rather
/// than escaped: these strings come from an agent, they end up in a query, and
/// a column name has no legitimate use for a quote or a parenthesis.
fn projection(columns: &[String]) -> String {
    let safe: Vec<String> = columns
        .iter()
        .map(|c| {
            c.chars()
                .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
                .collect::<String>()
        })
        .filter(|c| !c.is_empty())
        .map(|c| format!("\"{c}\""))
        .collect();
    if safe.is_empty() {
        "*".to_string()
    } else {
        safe.join(", ")
    }
}

/// Parsed cone-search response: the CSV header row plus data rows.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VizierConeResult {
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

/// Distinguishes "this host is broken, try the next mirror" from "this query is
/// wrong, every mirror will say the same".
enum QueryError {
    /// Transport failure, timeout, 404, 429, 5xx — rotate to the next mirror.
    Host(String),
    /// The query itself is wrong and every mirror will agree. Only 400 and 403.
    Query(String),
}

/// Whether an HTTP status means "no other mirror will do better".
///
/// It used to be `code >= 500`, which made **404 definitive** — and a 404 from a
/// TAP endpoint is the one status that most clearly is not about the query. It
/// means the PATH is not on that host: mirror layout, not ADQL. `china-vo`
/// answers 404 for `/tap/sync` whatever you ask it, including catalogues that
/// exist, so a chain that reached it stopped there and reported
/// "looks like a query problem" about a query that was fine.
///
/// A genuinely bad query comes back 400 with a VOTable error document, and 403
/// is an authorisation answer no other mirror will change. Everything else —
/// 404, 429, 5xx, transport — is worth asking the next host.
fn is_definitive(code: u16) -> bool {
    matches!(code, 400 | 403)
}

pub struct VizierService {
    client: Client,
    /// Where the mirror list comes from. The user can reorder or replace it in
    /// Settings — these hostnames have gone away before, and a constant in the
    /// binary left nobody a way to route around it.
    endpoints: std::sync::Arc<crate::config::ApiEndpoints>,
}

impl VizierService {
    pub fn new(client: Client, endpoints: std::sync::Arc<crate::config::ApiEndpoints>) -> Self {
        VizierService { client, endpoints }
    }

    /// The mirrors to try, in order, as configured.
    fn mirrors(&self) -> Vec<String> {
        self.endpoints.vizier_mirrors()
    }

    /// The canonical VizieR cone-search ADQL.
    ///
    /// Byte-compatible with the reference C#/macOS clients when no columns are
    /// named — `SELECT TOP n *` is still exactly what they send. A projection
    /// only appears when the caller asks for one.
    pub fn build_adql(q: &ConeQuery) -> String {
        let ConeQuery {
            catalogue,
            ra_deg,
            dec_deg,
            radius_deg,
            ra_column,
            dec_column,
            max_rec,
            columns,
        } = q;
        let select = projection(columns);
        format!(
            "SELECT TOP {max_rec} {select}\n\
             FROM \"{catalogue}\"\n\
             WHERE 1 = CONTAINS(\n    \
             POINT('ICRS', {ra_column}, {dec_column}),\n    \
             CIRCLE('ICRS', {ra_deg}, {dec_deg}, {radius_deg})\n)"
        )
    }

    /// Cone-search a VizieR catalogue, rotating through the configured mirrors on
    /// host-specific failures. Returns the parsed rows, or a human-readable
    /// error string describing the failover path.
    pub async fn cone_search(&self, q: &ConeQuery<'_>) -> Result<VizierConeResult, String> {
        let adql = Self::build_adql(q);
        let max_rec = q.max_rec;

        let mut attempts: Vec<(String, String)> = Vec::new();
        for url in self.mirrors() {
            let host = host_of(&url);
            match self.query_once(&url, &adql, max_rec).await {
                Ok(csv) => return Ok(parse_csv(&csv)),
                Err(QueryError::Query(msg)) => {
                    return Err(format!(
                        "vizier_cone_search at {host}: {msg} — not retrying other mirrors \
                         (the query itself is refused, and every mirror will agree)."
                    ));
                }
                Err(QueryError::Host(msg)) => {
                    attempts.push((host, msg));
                }
            }
        }

        // Every host's own reason, not just the last one's. The last mirror is
        // the plain-HTTP fallback, so "last error" was reliably its 404 — which
        // said nothing about the two TLS hosts that had actually failed, and
        // pointed at the query rather than at VizieR being down.
        let detail = attempts
            .iter()
            .map(|(h, e)| format!("{h}: {e}"))
            .collect::<Vec<_>>()
            .join("; ");
        Err(format!(
            "vizier_cone_search exhausted all VizieR mirrors — {detail}. VizieR may be globally \
             degraded (all mirrors share one operator); retry in a few minutes, or use astroquery \
             against vizier.cds.unistra.fr from inside a Skaha session, which is a different \
             service and stays up when TAP does not."
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
            if is_definitive(code) {
                Err(QueryError::Query(msg))
            } else {
                Err(QueryError::Host(msg))
            }
        }
    }
}

/// The host part of a URL, for error messages. Falls back to the whole URL
/// rather than guessing, since it only ever gets shown to a person.
fn host_of(url: &str) -> String {
    url.split("://")
        .nth(1)
        .and_then(|rest| rest.split('/').next())
        .unwrap_or(url)
        .to_string()
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

/// The mirrors as a fresh install gets them.
#[cfg(test)]
fn default_mirrors() -> Vec<String> {
    crate::config::ApiEndpoints::new(crate::config::AppConfig::default()).vizier_mirrors()
}

#[cfg(test)]
mod failover_tests {
    use super::{default_mirrors, is_definitive};

    /// A 404 sends us to the next mirror; it is not an answer about the query.
    ///
    /// This was `code >= 500`, so 404 counted as definitive and stopped the
    /// chain. `vizier.china-vo.org` answers 404 on `/tap/sync` for every
    /// catalogue — including ones that exist — and it is the LAST mirror, so a
    /// search that had already lost two hosts ended there and reported
    /// "looks like a query problem, not a host problem" about a valid query.
    #[test]
    fn a_404_is_about_the_mirror_not_the_query() {
        assert!(!is_definitive(404), "404 must fail over");
        // Rate limiting and outages are the same: ask someone else.
        for code in [408, 429, 500, 502, 503, 504] {
            assert!(!is_definitive(code), "{code} must fail over");
        }
    }

    /// Bad ADQL and a refusal are worth reporting once, not four times.
    #[test]
    fn a_bad_query_stops_at_the_first_mirror() {
        assert!(is_definitive(400));
        assert!(is_definitive(403));
    }

    /// Every mirror is a hostname that resolves.
    ///
    /// `tap.cds.unistra.fr` and `tapvizier.esac.esa.int` were NXDOMAIN — the
    /// first and third entries, so every cone search opened with two certain
    /// failures. This is checked by shape, not by DNS: a test that resolves
    /// hostnames fails on an aeroplane. The rule it enforces is that a mirror
    /// is written down once, deliberately, and reviewed when it changes.
    #[test]
    fn the_mirror_list_is_ordered_and_distinct() {
        let mirrors = default_mirrors();
        assert!(!mirrors.is_empty(), "no mirrors at all");
        let mut seen = std::collections::HashSet::new();
        for url in &mirrors {
            assert!(
                seen.insert(url.clone()),
                "{url} appears twice — one wasted round trip per search"
            );
            assert!(url.ends_with("/sync"), "{url} is not a TAP sync endpoint");
        }
        // TLS first, plain HTTP last: the HTTP mirror exists for callers who
        // cannot complete a handshake, not as a general-purpose alternative.
        if let Some(at) = mirrors.iter().position(|u| u.starts_with("http://")) {
            assert_eq!(
                at,
                mirrors.len() - 1,
                "a plain-HTTP mirror is ahead of a TLS one"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::default_mirrors;
    use super::*;

    fn query<'a>(columns: &'a [String]) -> ConeQuery<'a> {
        ConeQuery {
            catalogue: "V/97/catalog",
            ra_deg: 10.5,
            dec_deg: 41.2,
            radius_deg: 0.05,
            ra_column: "RAJ2000",
            dec_column: "DEJ2000",
            max_rec: 500,
            columns,
        }
    }

    /// Naming no columns sends exactly what it always sent.
    ///
    /// The projection is a new feature; the existing query is not. Any drift
    /// here changes what every caller that does not use `columns` receives.
    #[test]
    fn adql_is_canonical() {
        let adql = VizierService::build_adql(&query(&[]));
        assert!(adql.starts_with("SELECT TOP 500 *"));
        assert!(adql.contains("FROM \"V/97/catalog\""));
        assert!(adql.contains("POINT('ICRS', RAJ2000, DEJ2000)"));
        assert!(adql.contains("CIRCLE('ICRS', 10.5, 41.2, 0.05)"));
    }

    /// Named columns become the SELECT list, quoted.
    ///
    /// Quoted because VizieR has columns an unquoted ADQL identifier cannot
    /// spell: `_r` is the computed distance from the cone centre and the one
    /// callers ask for most.
    #[test]
    fn named_columns_become_the_select_list() {
        let cols = ["RA_ICRS".to_string(), "Plx".to_string(), "_r".to_string()];
        let adql = VizierService::build_adql(&query(&cols));
        assert!(
            adql.starts_with("SELECT TOP 500 \"RA_ICRS\", \"Plx\", \"_r\"\n"),
            "{adql}"
        );
        // The rest of the query is untouched by the projection.
        assert!(adql.contains("FROM \"V/97/catalog\""));
        assert!(adql.contains("POINT('ICRS', RAJ2000, DEJ2000)"));
    }

    /// A column name cannot end the quoting and start something else.
    ///
    /// These strings come from an agent and end up inside a query. A real
    /// column name has no use for a quote, a parenthesis or a semicolon, so
    /// they are removed rather than escaped.
    #[test]
    fn a_column_name_cannot_smuggle_in_more_query() {
        let cols = [
            "RA\"; DROP TABLE x; --".to_string(),
            "ok_name".to_string(),
            "(SELECT 1)".to_string(),
        ];
        let adql = VizierService::build_adql(&query(&cols));
        let select = adql
            .lines()
            .next()
            .and_then(|l| l.strip_prefix("SELECT TOP 500 "))
            .expect("a select list");
        // Every quoted region is a bare column name, and quoting is balanced.
        assert_eq!(select.matches('"').count() % 2, 0, "{select}");
        for quoted in select.split('"').skip(1).step_by(2) {
            assert!(
                quoted
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')),
                "{quoted:?} escaped its quotes in {select}"
            );
        }
        assert!(!select.contains(';'), "{select}");
        assert!(!select.contains('('), "{select}");
    }

    /// Columns that sanitize away entirely fall back to every column, rather
    /// than producing `SELECT TOP 500 ` and a syntax error.
    #[test]
    fn columns_that_are_all_punctuation_fall_back_to_star() {
        let cols = ["***".to_string(), "  ".to_string()];
        let adql = VizierService::build_adql(&query(&cols));
        assert!(adql.starts_with("SELECT TOP 500 *\n"), "{adql}");
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

    /// The two hostnames that do not exist stay gone.
    ///
    /// `tap.cds.unistra.fr` and `tapvizier.esac.esa.int` are NXDOMAIN — checked
    /// against a public resolver, not just this machine's. They were the first
    /// and third of four, so every cone search opened with two certain failures
    /// and reached the last mirror with the real reason already discarded. The
    /// old test asserted the first entry WAS `tap.cds.unistra.fr`, which is how
    /// a dead host stays pinned.
    #[test]
    fn the_defaults_contain_no_hostname_that_does_not_resolve() {
        let mirrors = default_mirrors().join(" ");
        for dead in ["tap.cds.unistra.fr", "tapvizier.esac.esa.int"] {
            assert!(
                !mirrors.contains(dead),
                "{dead} is NXDOMAIN and back in the list"
            );
        }
    }

    /// A user who empties the field gets the shipped list, not no mirrors.
    #[test]
    fn clearing_the_setting_restores_the_defaults() {
        let cfg = crate::config::AppConfig {
            vizier_mirrors: "   ".to_string(),
            ..Default::default()
        };
        let restored = crate::config::ApiEndpoints::new(cfg).vizier_mirrors();
        assert_eq!(restored, default_mirrors());
    }

    /// Nonsense is dropped rather than prefixed onto a request.
    #[test]
    fn a_mirror_that_is_not_a_url_is_ignored() {
        let cfg = crate::config::AppConfig {
            vizier_mirrors: "not-a-url https://example.org/tap/sync ftp://old.example/tap"
                .to_string(),
            ..Default::default()
        };
        let kept = crate::config::ApiEndpoints::new(cfg).vizier_mirrors();
        assert_eq!(kept, vec!["https://example.org/tap/sync".to_string()]);
    }
}
