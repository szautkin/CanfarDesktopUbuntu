//! Live MCP tools for the CADC Archive Search page — full UI coverage.
//!
//! Port of CanfarDesktop 1.3.3's Search MCP family (`Mcp/Tools/Read/SearchUiReadTools.cs`,
//! `Mcp/Tools/Read/SearchExecTools.cs`, `Mcp/Tools/Write/SearchUiTools.cs`,
//! `Mcp/Tools/Write/RecentSearchWriteTools.cs`). Together these let an agent do
//! everything a person can on the page: fill the form, narrow the Additional
//! Constraints facets, run a search or raw ADQL, steer the results table, export,
//! and use the side-panel pickers.
//!
//! Two dispatch styles live here, and the split is deliberate:
//!
//! * **Live steering** forwards over the view-state bridge to the open page
//!   (target `"search"`), so the agent drives the very widgets the user sees.
//!   Nothing is queued — these are the agent's equivalent of clicking.
//! * **`remove_recent_search` / `clear_recent_searches`** are Destructive
//!   proposals instead. They delete the user's own history, which no autonomy
//!   setting should ever apply unattended.
//!
//! `remove_recent_search` resolves its index to a stable `(searchedAt, summary)`
//! key at PROPOSE time. Between proposing and approving, a new search can shift
//! every index by one — resolving late would delete a different entry than the
//! one the user reviewed.

use super::{opt_u64, str_arg, ToolDescriptor, ToolResult, VerbClass};
use crate::mcp::tools::proposals::{InMemoryProposalStore, PendingProposal};
use crate::mcp::view_state;
use crate::services::SearchStoreService;
use crate::state::AppServices;
use serde_json::{json, Value};
use std::sync::Arc;

/// Tools forwarded verbatim to the page as bridge ops — the tool name IS the op,
/// so there is one vocabulary rather than a translation table to drift.
const LIVE_TOOLS: &[&str] = &[
    "get_search_form",
    "set_search_form",
    "reset_search_form",
    "get_search_constraints",
    "set_search_constraints",
    "run_search",
    "set_adql_query",
    "execute_adql_query",
    "get_search_results",
    "set_search_results_view",
    "show_search_row_detail",
    "export_search_results",
    "load_recent_search",
    "run_saved_query",
];

/// Proposal kinds owned by this module.
const KIND_REMOVE_RECENT: &str = "remove_recent_search";
const KIND_CLEAR_RECENT: &str = "clear_recent_searches";

fn read_tool(name: &str, description: &str, input_schema: Value) -> ToolDescriptor {
    ToolDescriptor {
        name: name.to_string(),
        description: description.to_string(),
        input_schema,
        verb: VerbClass::Read,
        agent_safe: true,
    }
}

fn write_tool(name: &str, description: &str, input_schema: Value) -> ToolDescriptor {
    ToolDescriptor {
        name: name.to_string(),
        description: description.to_string(),
        input_schema,
        verb: VerbClass::Write,
        agent_safe: true,
    }
}

fn no_args() -> Value {
    json!({ "type": "object", "properties": {}, "additionalProperties": false })
}

/// The form fields `set_search_form` accepts, shared by its schema so the
/// documented surface and the accepted surface cannot diverge.
/// The `set_search_form` argument schema.
///
/// `pub(crate)` so the Search page can assert its numeric widgets accept
/// everything advertised here — a SpinButton silently clamps, so a tighter
/// widget quietly rewrites an agent's value.
pub(crate) fn form_properties() -> Value {
    json!({
        "observationId": {"type": "string", "description": "Observation ID — an EXACT match (case-insensitive). Use `*` as a wildcard for a prefix or partial id, e.g. `jw01345*`."},
        "piName": {"type": "string", "description": "Principal investigator name."},
        "proposalId": {"type": "string", "description": "Proposal / program ID."},
        "proposalTitle": {"type": "string", "description": "Words from the proposal title."},
        "keywords": {"type": "string", "description": "Proposal keywords."},
        "dataRelease": {"type": "string", "description": "Data-release date constraint."},
        "publicOnly": {"type": "boolean", "description": "Restrict to public data."},
        "intent": {"type": "string", "enum": crate::ui::search_page::INTENTS, "description": "Observation intent."},
        "target": {"type": "string", "description": "Target name (resolved to RA/Dec), or coordinates."},
        "resolver": {"type": "string", "enum": crate::ui::search_page::RESOLVER_SERVICES, "description": "Name-resolver service. NONE searches by name only, with no coordinate constraint."},
        "radius": {"type": "number", "minimum": crate::ui::search_page::RADIUS_RANGE_DEG.0, "maximum": crate::ui::search_page::RADIUS_RANGE_DEG.1, "description": "Cone radius in degrees."},
        "pixelScale": {"type": "string", "description": "Pixel scale. Accepts range syntax: `0.1..1.0`, `> 0.2`, `<= 5`. A bare value means EQUALS."},
        "pixelScaleUnit": {"type": "string", "enum": crate::helpers::unit_converter::PIXEL_SCALE_UNITS},
        "spatialCutout": {"type": "boolean", "description": "Restrict to data supporting spatial cutouts."},
        "observationDate": {"type": "string", "description": "Observation date or range (`2020-01-01..2021-01-01`)."},
        "datePreset": {"type": "string", "enum": crate::helpers::date_presets::VALUES, "description": "Relative date window. Applied BEFORE observationDate, so an explicit date in the same call wins."},
        "integrationTime": {"type": "string", "description": "Integration time. Range syntax accepted."},
        "timeSpan": {"type": "string", "description": "Time span. Range syntax accepted."},
        "timeUnit": {"type": "string", "enum": crate::helpers::unit_converter::TIME_UNITS},
        "spectralCoverage": {"type": "string", "description": "Spectral coverage. Range syntax accepted."},
        "spectralSampling": {"type": "string", "description": "Spectral sampling. Range syntax accepted."},
        "resolvingPower": {"type": "string", "description": "Resolving power. Range syntax accepted."},
        "bandpassWidth": {"type": "string", "description": "Bandpass width. Range syntax accepted."},
        "restFrameEnergy": {"type": "string", "description": "Rest-frame energy. Range syntax accepted."},
        "spectralUnit": {"type": "string", "enum": crate::helpers::unit_converter::SPECTRAL_UNITS, "description": "Unit for spectralCoverage. Also applies to the other spectral fields unless they name their own below."},
        "spectralSamplingUnit": {"type": "string", "enum": crate::helpers::unit_converter::SPECTRAL_UNITS},
        "bandpassWidthUnit": {"type": "string", "enum": crate::helpers::unit_converter::SPECTRAL_UNITS},
        "restFrameEnergyUnit": {"type": "string", "enum": crate::helpers::unit_converter::SPECTRAL_UNITS},
        "spectralCutout": {"type": "boolean", "description": "Restrict to data supporting spectral cutouts."},
        "maxRecords": {"type": "integer", "minimum": crate::ui::search_page::MAX_RECORDS_RANGE.0 as u64, "maximum": crate::ui::search_page::MAX_RECORDS_RANGE.1 as u64, "description": "Row limit (MAXREC)."}
    })
}

/// A facet's argument schema: an array REPLACING that facet's whole selection.
fn facet_property(what: &str) -> Value {
    json!({
        "type": "array",
        "items": {"type": "string"},
        "description": format!("{what}. Replaces this facet's entire selection; omit to leave it unchanged, or pass [] to clear it.")
    })
}

pub fn descriptors() -> Vec<ToolDescriptor> {
    vec![
        read_tool(
            "get_search_form",
            "Read every field of the CADC Archive Search form as the user currently sees it: all \
             four constraint columns, the resolver status and any resolved RA/Dec, the staged ADQL, \
             the row limit, and whether a search is running. Call this before set_search_form to \
             see what you are changing.",
            no_args(),
        ),
        write_tool(
            "set_search_form",
            "Fill in the search form. Only the fields you pass change — everything else is left \
             alone, so this is a patch, not a replacement. Applies live to the page the user is \
             looking at and lands on the Search Form tab. Setting `target` triggers the same \
             debounced name resolution as typing it. Does NOT run the search: call run_search.",
            json!({
                "type": "object",
                "properties": form_properties(),
                "additionalProperties": false
            }),
        ),
        write_tool(
            "reset_search_form",
            "Clear the entire search form — every constraint field AND the Additional Constraints \
             facet selections — back to defaults, and land on the Search Form tab.",
            no_args(),
        ),
        read_tool(
            "get_search_constraints",
            "Read the Additional Constraints facets (band, collection, instrument, filter, \
             calibration level, data type, observation type). Each reports the values still \
             AVAILABLE under the current cascade plus the ones SELECTED, and how many there \
             are in `availableCount`. Long lists are shortened — CADC has thousands of \
             instruments — so pass `facet` with one facet's name to get that one in full. \
             Loads the data train first if it has not arrived yet, so an empty list always \
             means 'no such value', never 'not fetched'.",
            json!({
                "type": "object",
                "properties": {
                    "facet": {
                        "type": "string",
                        "description": "Return this one facet's values in full \
                                        (band, collection, instrument, filter, calLevel, \
                                        dataType, obsType). Omit for a shortened view of all."
                    }
                },
                "additionalProperties": false
            }),
        ),
        write_tool(
            "set_search_constraints",
            "Select Additional Constraints facet values. Each facet you pass REPLACES that facet's \
             whole selection; omit a facet to leave it alone, or pass [] to clear it. The cascade \
             then narrows the others — values your combination makes impossible are dropped and \
             named back to you in `dropped`, so check it.",
            json!({
                "type": "object",
                "properties": {
                    "clearAll": {"type": "boolean", "description": "Clear every facet first, then apply the ones given."},
                    "band": facet_property("Energy bands"),
                    "collection": facet_property("Collections (e.g. CFHT, JWST)"),
                    "instrument": facet_property("Instruments"),
                    "filter": facet_property("Filters"),
                    "calLevel": facet_property("Calibration levels"),
                    "dataType": facet_property("Data product types"),
                    "obsType": facet_property("Observation types")
                },
                "additionalProperties": false
            }),
        ),
        write_tool(
            "run_search",
            "Press Search: build ADQL from the current form + facets, run it, record a Recent \
             Search, and land on the Results tab. Fails if a search is already running.",
            no_args(),
        ),
        write_tool(
            "set_adql_query",
            &format!(
                "Put ADQL into the editor and switch to the ADQL tab WITHOUT running it — use \
                 when the user should review a query first. Call execute_adql_query to run it.{}",
                crate::helpers::adql_builder::DIALECT_NOTE
            ),
            json!({
                "type": "object",
                "properties": {"adql": {"type": "string", "description": "The ADQL query text."}},
                "required": ["adql"],
                "additionalProperties": false
            }),
        ),
        write_tool(
            "execute_adql_query",
            &format!(
                "Run ADQL directly, bypassing the form. Pass `adql` to stage and run in one \
                 step, or omit it to run whatever is already in the editor. Lands on the \
                 Results tab.{}",
                crate::helpers::adql_builder::DIALECT_NOTE
            ),
            json!({
                "type": "object",
                "properties": {"adql": {"type": "string", "description": "ADQL to stage and run. Omit to run the editor's current contents."}},
                "additionalProperties": false
            }),
        ),
        write_tool(
            "show_search_row_detail",
            "Open the detail dialog for the highlighted results row — every column of it, the \
             same window a person gets by clicking the row. Select the row first with \
             set_search_results_view's selectRow. To READ the same values instead, without \
             opening anything, call get_search_results with allColumns.",
            json!({ "type": "object", "properties": {}, "additionalProperties": false }),
        ),
        read_tool(
            "get_search_results",
            "Read the results table: status, the ADQL that produced it, totalRows and \
             filteredRows, pagination as currentPage (0-based), totalPages, rowsPerPage and a \
             human-readable pageStatus, rowsPerPageOptions, selectedRows, \
             sortColumn/sortAscending, active \
             per-column filters, \
             columnUnits, the column set with visibility, and — by default — the current \
             page's RAW cell values (capped at 500 rows) for the columns the grid is \
             showing — pass allColumns to get every column instead, which is what the \
             row-detail dialog shows. Values are raw, not display-formatted, so you can \
             compute on them.",
            json!({
                "type": "object",
                "properties": {
                    "includeRows": {"type": "boolean", "description": "Include the current page's cells (default true)."},
                    "maxRows": {"type": "integer", "minimum": 1, "maximum": 500, "description": "Cap on returned rows (default and hard cap 500)."},
                    "allColumns": {"type": "boolean", "description": "Return EVERY column the query produced rather than the ones the grid is showing — what the row-detail dialog displays when a person clicks a row. Default false."}
                },
                "additionalProperties": false
            }),
        ),
        write_tool(
            "set_search_results_view",
            "Steer the results table: filter, sort, show/hide columns, switch a column's display \
             unit, change page size, and paginate. Column keys are the cleaned lower-case names \
             `get_search_results` reports (\"targetname\", \"ra(j20000)\"); the case you write does \
             not matter, and an unknown one is refused with the full list. Every key is validated \
             first, so a command naming a bad column changes nothing rather than applying half of \
             itself. A rejected display unit names the units that column does take. \
             Set applyFiltersToAdql to promote the active client-side filters into the ADQL query.",
            json!({
                "type": "object",
                "properties": {
                    "clearFilters": {"type": "boolean", "description": "Drop all per-column filters first."},
                    "setFilters": {"type": "object", "description": "Column key → filter text. One condition is CADC Advanced Search syntax: bare text is a case-insensitive substring; '=v' matches the whole cell; '>v' '>=v' '<v' '<=v' compare (numerically when both sides are numbers, otherwise as case-insensitive text); 'a..b' is an inclusive range. Conditions combine with '!' (not), '&' (and), '|' (or) and parentheses — NOT binds tightest, then AND, then OR; the word forms AND/OR/NOT work too but must be upper-case, and double quotes make a run literal. Different columns always combine with AND. A numeric condition drops rows whose cell is empty; a condition with no value constrains nothing. An empty string clears that column's filter.", "additionalProperties": {"type": "string"}},
                    "sortColumn": {"type": "string", "description": "Column key to sort by."},
                    "sortAscending": {"type": "boolean", "description": "Sort direction (default true). Only meaningful with sortColumn."},
                    "showColumns": {"type": "array", "items": {"type": "string"}, "description": "Column keys to reveal."},
                    "hideColumns": {"type": "array", "items": {"type": "string"}, "description": "Column keys to hide."},
                    "columnUnits": {"type": "object", "description": "Column key → display unit id. get_search_results lists each column's own `units`, so there is no need to guess; a rejected one is refused with the list. An empty string restores that column's default.", "additionalProperties": {"type": "string"}},
                    "rowsPerPage": {"type": "integer", "minimum": 1, "maximum": 1000, "description": "Page size; resets to the first page. Any whole number from 1 to 1000 — the Rows/page dropdown's own entries are reported as rowsPerPageOptions by get_search_results, and a size that is not one of them is still applied, leaving the dropdown showing nothing selected."},
                    "pageAction": {"type": "string", "enum": ["first", "previous", "next", "last"]},
                    "page": {"type": "integer", "minimum": 0, "description": "Absolute 0-based page, clamped to the last page."},
                    "applyFiltersToAdql": {"type": "boolean", "description": "Rewrite the ADQL query to include the active filters, and switch to the ADQL tab."},
                    "selectRow": {"type": ["integer", "null"], "minimum": 0, "description": "Highlight one row and page to it, so a person looking at the window sees which row you mean. The index counts the FILTERED rows — the same one `rows` is indexed by and `selectedRow` reports. null clears the highlight."}
                },
                "additionalProperties": false
            }),
        ),
        write_tool(
            "export_search_results",
            "Write the FULL result set to a local CSV or TSV file: every row (not just the visible \
             page), every column (not just the visible ones), and the RAW values as TAP returned \
             them — decimal degrees, not the grid's sexagesimal — so the file is ready for astropy \
             or TOPCAT. Headers are the TAP column names. No file picker; defaults to \
             ~/Downloads/Verbinal/search_results_<timestamp>.<ext>.",
            json!({
                "type": "object",
                "properties": {
                    "format": {"type": "string", "enum": ["csv", "tsv"], "description": "Default csv."},
                    "path": {"type": "string", "description": "Destination path on the LOCAL filesystem — not a VOSpace path. Defaults to the Downloads folder."}
                },
                "additionalProperties": false
            }),
        ),
        write_tool(
            "load_recent_search",
            "Restore a recent search into the form by its index in list_recent_searches (0 = most \
             recent), including its Additional Constraints. Lands on the Search Form tab so the \
             user can review or tweak before re-running.",
            json!({
                "type": "object",
                "properties": {"index": {"type": "integer", "minimum": 0, "description": "0-based index into list_recent_searches (default 0)."}},
                "additionalProperties": false
            }),
        ),
        write_tool(
            "run_saved_query",
            "Run one of the user's saved queries by its exact name and land on the Results tab.",
            json!({
                "type": "object",
                "properties": {"name": {"type": "string", "description": "Exact saved-query name (see list_saved_queries)."}},
                "required": ["name"],
                "additionalProperties": false
            }),
        ),
        write_tool(
            "remove_recent_search",
            "Delete one entry from the user's Recent Searches by its index in \
             list_recent_searches. Destructive: always queued for the user to approve.",
            json!({
                "type": "object",
                "properties": {"index": {"type": "integer", "minimum": 0, "description": "0-based index into list_recent_searches."}},
                "required": ["index"],
                "additionalProperties": false
            }),
        ),
        write_tool(
            "clear_recent_searches",
            "Delete the user's ENTIRE Recent Searches history. Destructive: always queued for the \
             user to approve.",
            no_args(),
        ),
    ]
}

pub async fn dispatch(
    name: &str,
    services: &AppServices,
    args: &Value,
    proposals: &Arc<InMemoryProposalStore>,
) -> Option<ToolResult> {
    if LIVE_TOOLS.contains(&name) {
        // Tool name == bridge op; the page matches these verbatim.
        return Some(
            match view_state::viewer_command("search", name, args.clone()).await {
                Ok(v) => ToolResult::Data(v),
                Err(e) => ToolResult::Failed(e),
            },
        );
    }
    match name {
        KIND_REMOVE_RECENT => Some(propose_remove_recent(
            &services.search_store,
            args,
            proposals,
        )),
        KIND_CLEAR_RECENT => Some(propose_clear_recent(&services.search_store, proposals)),
        _ => None,
    }
}

/// Resolve the index to a stable identity NOW, not at apply time — see the
/// module docs for why.
fn propose_remove_recent(
    store: &SearchStoreService,
    args: &Value,
    proposals: &Arc<InMemoryProposalStore>,
) -> ToolResult {
    let Some(index) = opt_u64(args, "index").map(|n| n as usize) else {
        return ToolResult::Failed("index is required".to_string());
    };
    let all = store.load_recent();
    let Some(entry) = all.get(index) else {
        return ToolResult::Failed(format!(
            "no recent search at index {index} ({} available)",
            all.len()
        ));
    };
    let payload = json!({
        "searchedAt": entry.searched_at,
        "summary": entry.summary,
    });
    let p = proposals.enqueue(
        KIND_REMOVE_RECENT,
        &format!("Remove recent search: {}", entry.summary),
        true,
        payload,
    );
    ToolResult::Proposed(p)
}

fn propose_clear_recent(
    store: &SearchStoreService,
    proposals: &Arc<InMemoryProposalStore>,
) -> ToolResult {
    let count = store.load_recent().len();
    if count == 0 {
        return ToolResult::Failed("there are no recent searches to clear".to_string());
    }
    let p = proposals.enqueue(
        KIND_CLEAR_RECENT,
        &format!("Clear all {count} recent searches"),
        true,
        json!({}),
    );
    ToolResult::Proposed(p)
}

/// Execute an approved recent-search proposal.
pub async fn apply(
    services: &AppServices,
    proposal: &PendingProposal,
) -> Option<Result<String, String>> {
    apply_to_store(&services.search_store, proposal)
}

/// The real work, taking only the store — so it is testable without building a
/// whole `AppServices` (and without any chance of touching the real one).
fn apply_to_store(
    store: &SearchStoreService,
    proposal: &PendingProposal,
) -> Option<Result<String, String>> {
    let payload = &proposal.payload;
    let out = match proposal.kind.as_str() {
        KIND_REMOVE_RECENT => {
            let searched_at = str_arg(payload, "searchedAt");
            let summary = str_arg(payload, "summary");
            let before = store.load_recent();
            let after: Vec<_> = before
                .iter()
                .filter(|r| !(r.searched_at == searched_at && r.summary == summary))
                .cloned()
                .collect();
            if after.len() == before.len() {
                Err(format!("recent search {summary:?} is no longer present"))
            } else {
                match store.save_all_recent(&after) {
                    Ok(()) => Ok(format!("Removed recent search: {summary}")),
                    Err(e) => Err(format!("could not save recent searches: {e}")),
                }
            }
        }
        KIND_CLEAR_RECENT => match store.clear_recent() {
            Ok(()) => Ok("Cleared all recent searches".to_string()),
            Err(e) => Err(format!("could not clear recent searches: {e}")),
        },
        _ => return None,
    };
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The advertised unit enums must be the SAME lists the form renders and the
    /// converter understands.
    ///
    /// They were three separate hand-written copies, and the schema's had gone
    /// stale: it offered four spectral units where the converter handles
    /// fourteen, so an agent asking to search in GHz was told that argument was
    /// invalid. Binding them to one source is the fix; this proves the binding
    /// survives, since a literal here would compile perfectly well.
    #[test]
    fn the_advertised_units_are_the_ones_the_app_supports() {
        use crate::helpers::unit_converter::{PIXEL_SCALE_UNITS, SPECTRAL_UNITS, TIME_UNITS};

        let props = form_properties();
        let enum_of = |key: &str| -> Vec<String> {
            props[key]["enum"]
                .as_array()
                .unwrap_or_else(|| panic!("`{key}` should declare an enum"))
                .iter()
                .map(|v| v.as_str().unwrap_or_default().to_string())
                .collect()
        };

        // Every spectral field carries its own unit, so every one of them has to
        // offer the same list — an agent told GHz is valid for coverage but not
        // for sampling would have no way to tell which.
        for key in [
            "spectralUnit",
            "spectralSamplingUnit",
            "bandpassWidthUnit",
            "restFrameEnergyUnit",
        ] {
            assert_eq!(enum_of(key), SPECTRAL_UNITS.to_vec(), "{key}");
        }
        assert_eq!(enum_of("timeUnit"), TIME_UNITS.to_vec());
        assert_eq!(enum_of("pixelScaleUnit"), PIXEL_SCALE_UNITS.to_vec());

        // And every advertised spectral unit must actually convert, or the agent
        // is offered an option that silently drops its constraint.
        for unit in enum_of("spectralUnit") {
            assert!(
                crate::helpers::unit_converter::to_metres(1.0, &unit).is_some(),
                "`{unit}` is advertised but does not convert"
            );
        }
    }

    fn store() -> Arc<InMemoryProposalStore> {
        Arc::new(InMemoryProposalStore::new())
    }

    #[test]
    fn every_live_tool_has_a_descriptor_and_vice_versa() {
        // The LIVE_TOOLS list drives dispatch while `descriptors` drives
        // tools/list; a name in one and not the other is either an advertised
        // tool that does nothing, or a working tool nobody can discover.
        let described: std::collections::HashSet<String> =
            descriptors().into_iter().map(|d| d.name).collect();
        for name in LIVE_TOOLS {
            assert!(
                described.contains(*name),
                "`{name}` dispatches but is not advertised"
            );
        }
        let proposal_tools = [KIND_REMOVE_RECENT, KIND_CLEAR_RECENT];
        for name in described {
            assert!(
                LIVE_TOOLS.contains(&name.as_str()) || proposal_tools.contains(&name.as_str()),
                "`{name}` is advertised but nothing dispatches it"
            );
        }
    }

    #[test]
    fn recent_search_deletes_are_destructive_proposals() {
        // These wipe the user's own history, so no autonomy setting may apply
        // them unattended.
        for d in descriptors() {
            if d.name == KIND_REMOVE_RECENT || d.name == KIND_CLEAR_RECENT {
                assert_eq!(d.verb, VerbClass::Write, "{} must be a write", d.name);
            }
        }
    }

    /// A store rooted in a private temp dir, cleaned up on drop.
    ///
    /// These tests read and DELETE recent searches, so they must never see the
    /// real store — one of them would otherwise wipe the developer's own history.
    struct TempStore {
        dir: std::path::PathBuf,
        svc: SearchStoreService,
    }

    impl TempStore {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "verbinal_search_ui_{}_{}",
                tag,
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            let svc = SearchStoreService::with_data_dir(dir.clone());
            TempStore { dir, svc }
        }
    }

    impl Drop for TempStore {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn recent(adql: &str, summary: &str, at: &str) -> crate::models::search_result::RecentSearch {
        crate::models::search_result::RecentSearch {
            adql: adql.to_string(),
            summary: summary.to_string(),
            searched_at: at.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn remove_recent_requires_an_index() {
        let t = TempStore::new("noindex");
        match propose_remove_recent(&t.svc, &json!({}), &store()) {
            ToolResult::Failed(msg) => assert!(msg.contains("index is required"), "{msg}"),
            _ => panic!("expected a failure without an index"),
        }
    }

    #[test]
    fn remove_recent_rejects_an_out_of_range_index() {
        let t = TempStore::new("range");
        match propose_remove_recent(&t.svc, &json!({ "index": 9 }), &store()) {
            ToolResult::Failed(msg) => {
                assert!(msg.contains("no recent search at index 9"), "{msg}")
            }
            _ => panic!("expected a failure for an out-of-range index"),
        }
    }

    #[test]
    fn remove_recent_pins_the_entry_identity_at_propose_time() {
        // The payload must carry a stable key, NOT the index: between proposing
        // and approving, a new search shifts every index by one and a late
        // lookup would delete a different entry than the user reviewed.
        let t = TempStore::new("identity");
        t.svc
            .save_all_recent(&[recent("SELECT 1", "M31 cone", "2026-08-10T00:00:00Z")])
            .unwrap();

        match propose_remove_recent(&t.svc, &json!({ "index": 0 }), &store()) {
            ToolResult::Proposed(p) => {
                assert_eq!(p.kind, KIND_REMOVE_RECENT);
                assert!(p.destructive, "deleting history must never auto-apply");
                assert_eq!(p.payload["summary"], "M31 cone");
                assert_eq!(p.payload["searchedAt"], "2026-08-10T00:00:00Z");
                assert!(
                    p.payload.get("index").is_none(),
                    "the index must not survive into the payload"
                );
            }
            _ => panic!("expected a queued proposal"),
        }
    }

    #[test]
    fn applying_a_remove_deletes_the_pinned_entry_even_after_the_list_shifted() {
        // The whole reason the payload carries an identity: a search recorded
        // between propose and apply shifts every index, so an index-based applier
        // would delete the wrong row.
        let t = TempStore::new("apply");
        t.svc
            .save_all_recent(&[recent("SELECT 1", "target", "t1")])
            .unwrap();
        let ToolResult::Proposed(p) =
            propose_remove_recent(&t.svc, &json!({ "index": 0 }), &store())
        else {
            panic!("expected a queued proposal");
        };

        // A newer search arrives first, pushing the target from index 0 to 1.
        t.svc
            .save_all_recent(&[
                recent("SELECT 2", "newer", "t2"),
                recent("SELECT 1", "target", "t1"),
            ])
            .unwrap();

        apply_to_store(&t.svc, &p)
            .expect("this module owns the kind")
            .unwrap();

        let left: Vec<String> = t.svc.load_recent().into_iter().map(|r| r.summary).collect();
        assert_eq!(
            left,
            vec!["newer"],
            "the pinned entry should be the one removed"
        );
    }

    #[test]
    fn applying_a_remove_reports_an_entry_that_is_already_gone() {
        // Silently succeeding would tell the user something was deleted when
        // nothing was.
        let t = TempStore::new("gone");
        t.svc
            .save_all_recent(&[recent("SELECT 1", "target", "t1")])
            .unwrap();
        let ToolResult::Proposed(p) =
            propose_remove_recent(&t.svc, &json!({ "index": 0 }), &store())
        else {
            panic!("expected a queued proposal");
        };
        t.svc.clear_recent().unwrap();

        let err = apply_to_store(&t.svc, &p)
            .expect("this module owns the kind")
            .expect_err("a vanished entry must not report success");
        assert!(err.contains("no longer present"), "{err}");
    }

    #[test]
    fn applying_a_clear_empties_the_history() {
        let t = TempStore::new("clearapply");
        t.svc
            .save_all_recent(&[recent("SELECT 1", "a", "t1")])
            .unwrap();
        let ToolResult::Proposed(p) = propose_clear_recent(&t.svc, &store()) else {
            panic!("expected a queued proposal");
        };
        apply_to_store(&t.svc, &p)
            .expect("this module owns the kind")
            .unwrap();
        assert!(t.svc.load_recent().is_empty());
    }

    #[test]
    fn apply_declines_a_kind_this_module_does_not_own() {
        // Returning Some(Err) instead of None would swallow another family's
        // proposal and report it as a search failure.
        let t = TempStore::new("other");
        let p = store().enqueue("delete_vospace_node", "not ours", true, json!({}));
        assert!(apply_to_store(&t.svc, &p).is_none());
    }

    #[test]
    fn clear_recent_refuses_when_there_is_nothing_to_clear() {
        // Better an explicit "nothing to clear" than a proposal the user approves
        // that turns out to be a no-op.
        let t = TempStore::new("empty");
        match propose_clear_recent(&t.svc, &store()) {
            ToolResult::Failed(msg) => assert!(msg.contains("no recent searches"), "{msg}"),
            _ => panic!("expected a failure with an empty history"),
        }
    }

    #[test]
    fn clear_recent_queues_a_destructive_proposal_naming_the_count() {
        let t = TempStore::new("count");
        t.svc
            .save_all_recent(&[recent("SELECT 1", "a", "t1"), recent("SELECT 2", "b", "t2")])
            .unwrap();

        match propose_clear_recent(&t.svc, &store()) {
            ToolResult::Proposed(p) => {
                assert!(p.destructive);
                assert!(
                    p.summary.contains('2'),
                    "summary should name the count: {}",
                    p.summary
                );
            }
            _ => panic!("expected a queued proposal"),
        }
    }
}
