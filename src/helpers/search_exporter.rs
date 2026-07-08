//! Build a Claude-friendly export of the user's saved ADQL queries + recent
//! searches, as `saved_queries.json` + `recent_searches.json` + `queries.md`.
//!
//! Rust port of `Services/Export/SearchExporter.cs` + `SearchExportBuilder.cs`.
//! The ADQL is placed in fenced ```sql``` blocks so an LLM can parse / execute /
//! rewrite the user's queries, and each recent search carries a **resolver
//! provenance** line (SCI-9-3): which name-resolution service produced the cone
//! coordinates, so a bundle reader can reproduce or trust the search footprint.
//!
//! ## Relationship to `research_exporter`
//!
//! This module only *renders* the payload — the store-only ZIP writer lives in
//! [`crate::helpers::research_exporter`], which exposes the shared
//! [`crate::helpers::research_exporter::write_store_zip`] entry point. The
//! combined research + search bundle is assembled by the
//! `export_research_bundle` MCP tool, which concatenates this module's files
//! with the research files and writes one archive.
//!
//! ## Port notes
//!
//! The provenance line matches the reference: `resolver_service_used` (falling
//! back to `resolver_service`) at `resolution_epoch` (or "unknown epoch") →
//! resolved RA/Dec. Those fields live on the search's `form_state`; the
//! denormalised copies on [`RecentSearch`] itself are consulted as a fallback so
//! provenance renders whichever the search page populated. JSON is emitted with
//! `serde`'s default (snake_case) field names to match exactly what
//! `SearchStoreService` already persists to disk, rather than the reference's
//! camelCase.

use crate::models::search_result::{RecentSearch, SavedQuery};
use chrono::{DateTime, Utc};

/// Render the search export payload from in-memory data. Pure — no I/O.
///
/// Returns `(filename, contents)` pairs ready to hand to the store-only ZIP
/// writer:
///
/// * `saved_queries.json` — pretty JSON of every saved query (always present).
/// * `recent_searches.json` — pretty JSON of the recent searches (only when
///   `include_history`).
/// * `queries.md` — human/LLM-readable markdown with fenced ```sql``` blocks.
///
/// `now` is stamped into the markdown header so callers can pass a fixed clock
/// in tests.
pub fn build_search_bundle(
    saved: &[SavedQuery],
    recent: &[RecentSearch],
    include_history: bool,
    now: DateTime<Utc>,
) -> Vec<(String, String)> {
    let mut files: Vec<(String, String)> = Vec::new();

    let saved_json = serde_json::to_string_pretty(saved).unwrap_or_else(|_| "[]".to_string());
    files.push(("saved_queries.json".to_string(), saved_json));

    // Recent searches are history: only exported (JSON *and* markdown) when the
    // caller opts in — mirrors `ExportOptions.IncludeSearchHistory`.
    let recent_for_render: &[RecentSearch] = if include_history { recent } else { &[] };
    if include_history {
        let recent_json =
            serde_json::to_string_pretty(recent).unwrap_or_else(|_| "[]".to_string());
        files.push(("recent_searches.json".to_string(), recent_json));
    }

    files.push((
        "queries.md".to_string(),
        render_markdown(saved, recent_for_render, now),
    ));

    files
}

// ---------------------------------------------------------------------------
// Markdown rendering (1-to-1 with SearchExportBuilder.RenderMarkdown)
// ---------------------------------------------------------------------------

fn render_markdown(saved: &[SavedQuery], recent: &[RecentSearch], now: DateTime<Utc>) -> String {
    let mut md = String::new();
    md.push_str("# Search Queries\n\n");
    md.push_str(&format!("Exported {}\n\n", iso_utc(now)));
    md.push_str(&format!(
        "- {} saved quer{}\n",
        saved.len(),
        if saved.len() == 1 { "y" } else { "ies" }
    ));
    md.push_str(&format!(
        "- {} recent search{}\n\n",
        recent.len(),
        if recent.len() == 1 { "" } else { "es" }
    ));
    md.push_str("---\n\n");

    if !saved.is_empty() {
        md.push_str("## Saved ADQL Queries\n\n");
        for q in saved {
            md.push_str(&format!("### {}\n\n", q.name));
            md.push_str(&format!("Saved {}\n\n", iso_or_raw(&q.created_at)));
            push_sql_block(&mut md, &q.adql);
        }
        md.push_str("---\n\n");
    }

    if !recent.is_empty() {
        md.push_str("## Recent Searches\n\n");
        for s in recent {
            let heading = if s.summary.is_empty() {
                "(search)"
            } else {
                s.summary.as_str()
            };
            md.push_str(&format!("### {}\n\n", heading));
            md.push_str(&format!("- **Searched:** {}\n", iso_or_raw(&s.searched_at)));
            append_resolver_provenance(&mut md, s);
            md.push_str(&format!("- **Results:** {}\n\n", s.result_count));
            if !s.adql.trim().is_empty() {
                push_sql_block(&mut md, &s.adql);
            }
        }
    }

    if saved.is_empty() && recent.is_empty() {
        md.push_str("_No saved queries or recent searches yet._\n");
    }

    md
}

/// Append a fenced ```sql``` block for `adql`, guaranteeing a trailing newline
/// inside the fence (matches the reference, which appends `\n` if the query has
/// none).
fn push_sql_block(md: &mut String, adql: &str) {
    md.push_str("```sql\n");
    md.push_str(adql);
    if !adql.ends_with('\n') {
        md.push('\n');
    }
    md.push_str("```\n\n");
}

/// Freeze the name-resolution provenance (SCI-9-3): which resolver produced the
/// coordinates for a name-based cone search, and when. SIMBAD/NED can disagree
/// at the arcsec level, so a bundle reader needs this to reproduce or trust the
/// search footprint. Mirrors the reference `AppendResolverProvenance`:
/// `resolver_service_used` (else `resolver_service`) at `resolution_epoch` (else
/// "unknown epoch"). The fields live on `form_state`; the denormalised copies on
/// [`RecentSearch`] are a fallback.
fn append_resolver_provenance(md: &mut String, recent: &RecentSearch) {
    let fs = &recent.form_state;
    if fs.target.trim().is_empty() {
        return;
    }
    let (ra, dec) = match (fs.resolved_ra, fs.resolved_dec) {
        (Some(ra), Some(dec)) => (ra, dec),
        _ => return,
    };

    let non_blank = |s: &str| !s.trim().is_empty();
    let svc = fs
        .resolver_service_used
        .as_deref()
        .filter(|s| non_blank(s))
        .or_else(|| recent.resolver_service_used.as_deref().filter(|s| non_blank(s)))
        .unwrap_or_else(|| {
            if fs.resolver_service.trim().is_empty() {
                "unknown resolver"
            } else {
                fs.resolver_service.as_str()
            }
        });

    let epoch = fs
        .resolution_epoch
        .as_deref()
        .filter(|s| non_blank(s))
        .or_else(|| recent.resolution_epoch.as_deref().filter(|s| non_blank(s)))
        .map(iso_or_raw)
        .unwrap_or_else(|| "unknown epoch".to_string());

    md.push_str(&format!(
        "- **Resolved:** '{}' via {} at {} → RA {:.5}, Dec {:.5}\n",
        fs.target, svc, epoch, ra, dec
    ));
}

/// Format a UTC instant as `yyyy-MM-ddTHH:mm:ssZ`.
fn iso_utc(dt: DateTime<Utc>) -> String {
    dt.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// Normalise an RFC-3339 timestamp to `yyyy-MM-ddTHH:mm:ssZ`; if it does not
/// parse, return it unchanged (records may carry pre-existing free-form dates).
fn iso_or_raw(s: &str) -> String {
    match DateTime::parse_from_rfc3339(s) {
        Ok(dt) => iso_utc(dt.with_timezone(&Utc)),
        Err(_) => s.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::search_result::SearchFormState;

    fn saved(name: &str, adql: &str) -> SavedQuery {
        SavedQuery {
            name: name.to_string(),
            adql: adql.to_string(),
            created_at: "2024-03-04T05:06:07Z".to_string(),
            agent_attribution: None,
        }
    }

    fn recent(summary: &str, adql: &str, count: usize, fs: SearchFormState) -> RecentSearch {
        RecentSearch {
            resolver_service_used: fs.resolver_service_used.clone(),
            resolution_epoch: fs.resolution_epoch.clone(),
            summary: summary.to_string(),
            adql: adql.to_string(),
            form_state: fs,
            result_count: count,
            searched_at: "2024-04-05T06:07:08Z".to_string(),
        }
    }

    fn resolved_form(target: &str, service: &str, ra: f64, dec: f64) -> SearchFormState {
        let mut fs = SearchFormState::new();
        fs.target = target.to_string();
        fs.resolver_service = service.to_string();
        fs.resolver_service_used = Some(service.to_string());
        fs.resolution_epoch = Some("2026-07-08T00:00:00Z".to_string());
        fs.resolved_ra = Some(ra);
        fs.resolved_dec = Some(dec);
        fs
    }

    fn fixed_now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-07-08T09:10:11Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn bundle_always_has_saved_json_and_markdown() {
        let files = build_search_bundle(&[], &[], false, fixed_now());
        let names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"saved_queries.json"));
        assert!(names.contains(&"queries.md"));
        // History excluded => no recent_searches.json.
        assert!(!names.contains(&"recent_searches.json"));
    }

    #[test]
    fn history_toggle_controls_recent_file_and_markdown() {
        let r = recent(
            "M31 cone",
            "SELECT * FROM caom2.Observation",
            42,
            SearchFormState::new(),
        );
        // With history off, the recent file is absent and markdown reports 0.
        let off = build_search_bundle(&[], std::slice::from_ref(&r), false, fixed_now());
        assert!(!off.iter().any(|(n, _)| n == "recent_searches.json"));
        let md_off = &off.iter().find(|(n, _)| n == "queries.md").unwrap().1;
        assert!(md_off.contains("- 0 recent searches"));
        assert!(!md_off.contains("M31 cone"));

        // With history on, the recent file is present and markdown includes it.
        let on = build_search_bundle(&[], std::slice::from_ref(&r), true, fixed_now());
        let recent_json = on.iter().find(|(n, _)| n == "recent_searches.json").unwrap();
        assert!(recent_json.1.contains("M31 cone"));
        let md_on = &on.iter().find(|(n, _)| n == "queries.md").unwrap().1;
        assert!(md_on.contains("- 1 recent search\n"));
        assert!(md_on.contains("### M31 cone"));
        assert!(md_on.contains("- **Results:** 42"));
        assert!(md_on.contains("```sql\nSELECT * FROM caom2.Observation\n```"));
    }

    #[test]
    fn saved_queries_render_with_fenced_sql() {
        let files = build_search_bundle(
            &[saved("Bright stars", "SELECT TOP 10 * FROM x")],
            &[],
            true,
            fixed_now(),
        );
        let md = &files.iter().find(|(n, _)| n == "queries.md").unwrap().1;
        assert!(md.contains("Exported 2026-07-08T09:10:11Z"));
        assert!(md.contains("- 1 saved query\n"));
        assert!(md.contains("## Saved ADQL Queries"));
        assert!(md.contains("### Bright stars"));
        assert!(md.contains("Saved 2024-03-04T05:06:07Z"));
        assert!(md.contains("```sql\nSELECT TOP 10 * FROM x\n```"));
        // The saved JSON round-trips.
        let saved_json = &files.iter().find(|(n, _)| n == "saved_queries.json").unwrap().1;
        let _: serde_json::Value = serde_json::from_str(saved_json).unwrap();
        assert!(saved_json.contains("Bright stars"));
    }

    #[test]
    fn resolver_provenance_line_present_when_resolved() {
        let r = recent(
            "NGC 5128",
            "SELECT 1",
            3,
            resolved_form("NGC 5128", "SIMBAD", 201.36506, -43.01911),
        );
        let files = build_search_bundle(&[], &[r], true, fixed_now());
        let md = &files.iter().find(|(n, _)| n == "queries.md").unwrap().1;
        assert!(md.contains(
            "- **Resolved:** 'NGC 5128' via SIMBAD at 2026-07-08T00:00:00Z → RA 201.36506, Dec -43.01911"
        ));
    }

    #[test]
    fn resolver_provenance_omitted_without_coordinates() {
        // Target set but no resolved coordinates => no provenance line.
        let mut fs = SearchFormState::new();
        fs.target = "unresolvable".to_string();
        let r = recent("q", "SELECT 1", 0, fs);
        let files = build_search_bundle(&[], &[r], true, fixed_now());
        let md = &files.iter().find(|(n, _)| n == "queries.md").unwrap().1;
        assert!(!md.contains("**Resolved:**"));
    }

    #[test]
    fn empty_bundle_notes_nothing_saved() {
        let files = build_search_bundle(&[], &[], true, fixed_now());
        let md = &files.iter().find(|(n, _)| n == "queries.md").unwrap().1;
        assert!(md.contains("_No saved queries or recent searches yet._"));
    }
}
