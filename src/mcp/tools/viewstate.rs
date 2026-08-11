//! Live view-state MCP tools: let an agent see what the user is looking at and
//! steer the app (navigate, open a FITS file, focus a search). Port of
//! `Mcp/Tools/ViewState/ViewStateTools.cs` + `TabTools.cs`.
//!
//! These are UI navigations, not data mutations, so they execute directly
//! (agent-safe) rather than going through the write-proposal pipeline.

use super::{ToolDescriptor, ToolResult, VerbClass};
use crate::mcp::tools::proposals::{InMemoryProposalStore, PendingProposal};
use crate::mcp::view_state;
use crate::state::AppServices;
use serde_json::{json, Value};
use std::sync::Arc;

/// Build the `get_current_view` payload — the reference's `AppViewSnapshot`.
///
/// Written out by hand rather than serialized from [`view_state::ViewSnapshot`]:
/// the internal snapshot is the UI's own state, named for Rust, and serializing
/// it directly put snake_case keys (`open_fits_paths`, `search_focus_ra`) and
/// Rust names (`view`, `title`, `authenticated`) on the wire, where the
/// reference promises `openFitsPaths`, `searchFocusRA`, `mode`, `modeTitle` and
/// `isAuthenticated`. Half the payload also does not live in the snapshot at
/// all — the autonomy flags and budget are what let an agent decide whether its
/// next write will apply or queue, and how many it may still make.
fn current_view_payload(
    snap: &view_state::ViewSnapshot,
    services: &AppServices,
    proposals: &Arc<InMemoryProposalStore>,
) -> Value {
    use std::sync::atomic::Ordering;

    let pending = proposals.pending_count();
    let budget = services.proposal_budget;
    json!({
        "mode": snap.view,
        "modeTitle": snap.title,
        "isAuthenticated": snap.authenticated,
        // The reference sends "" rather than null when signed out.
        "username": snap.username.clone().unwrap_or_default(),
        "searchFocusRA": snap.search_focus_ra,
        "searchFocusDec": snap.search_focus_dec,
        "openFitsPaths": snap.open_fits_paths,
        "agentsEnabled": crate::services::mcp_settings_service::McpSettingsService::new()
            .server_enabled(),
        "autoApplyEnabled": services.mcp_auto_apply.load(Ordering::Relaxed),
        "followAgentActivityEnabled": services.mcp_follow_activity.load(Ordering::Relaxed),
        "pendingProposalsCount": pending,
        "proposalBudget": { "cap": budget.cap(), "remaining": budget.remaining(pending) },
        // Beyond the reference, whose FITS viewer is the only multi-document
        // surface. Ours also opens notebooks and cubes, and an agent orienting
        // itself should not have to call list_open_tabs to learn they exist.
        "openNotebooks": snap.open_notebooks,
        "openCubes": snap.open_cubes,
    })
}

/// Build the `list_open_tabs` payload — the reference's `OpenTabsState`.
///
/// `notebooks` / `fitsViewers` / `cubes` are COUNTS. They previously held the
/// path arrays, which collides with the reference on the same key names: an
/// agent testing `notebooks > 0` compared against an array, and one reading
/// `.length` on the reference's integer got nothing. The paths are not lost —
/// each tab entry carries its own, and `get_current_view` lists them.
fn open_tabs_payload(snap: &view_state::ViewSnapshot) -> Value {
    // The index, display name and active flag are what the switch/blink tools
    // address a tab by; `path` is ours, since a display name is not unique
    // enough to open the same file again.
    let tabs = |paths: &[String], active: Option<usize>| -> Vec<Value> {
        paths
            .iter()
            .enumerate()
            .map(|(i, p)| {
                json!({
                    "index": i,
                    "name": std::path::Path::new(p)
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| p.clone()),
                    "active": active == Some(i),
                    "path": p,
                })
            })
            .collect()
    };

    json!({
        "notebooks": snap.open_notebooks.len(),
        "fitsViewers": snap.open_fits_paths.len(),
        "cubes": snap.open_cubes.len(),
        "cubeTabs": tabs(&snap.open_cubes, snap.active_cube),
        "fitsTabs": tabs(&snap.open_fits_paths, snap.active_fits),
        // Beyond the reference, which has no notebook tab-switching tool.
        "notebookTabs": tabs(&snap.open_notebooks, snap.active_notebook),
    })
}

pub fn descriptors() -> Vec<ToolDescriptor> {
    let empty = json!({"type":"object","properties":{},"additionalProperties":false});
    vec![
        ToolDescriptor {
            name: "get_current_view".into(),
            description: "What the user is currently looking at: the mode and its title, auth state + \
                          username, the Search form's sky focus (RA/Dec), the open FITS / notebook / cube \
                          paths, plus the autonomy state — agentsEnabled, autoApplyEnabled (do your writes \
                          apply immediately or queue for review?), followAgentActivityEnabled, the pending- \
                          proposal count, and your remaining proposal budget. Read-only."
                .into(),
            input_schema: empty.clone(),
            verb: VerbClass::Read,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "list_open_tabs".into(),
            description: "Count the viewer tabs currently open (notebooks, fitsViewers, cubes). \
                          `fitsTabs` / `cubeTabs` / `notebookTabs` carry each tab's 0-based index, display \
                          name, whether it is ACTIVE, and its path — switch_fits_tab, switch_cube_tab and \
                          blink_fits_tabs all address a tab by that index, and blink needs a partner \
                          different from the active one. Use with close_active_tab to clean up tabs an \
                          automated run accumulated."
                .into(),
            input_schema: empty.clone(),
            verb: VerbClass::Read,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "navigate_to".into(),
            description: "Switch the app to a view. One of: home, portal, search, storage, fits, notebook, research, cube, workflows, aiguide, settings.".into(),
            input_schema: json!({
                "type":"object",
                "properties": { "view": { "type":"string" } },
                "required": ["view"], "additionalProperties": false
            }),
            verb: VerbClass::Write,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "open_fits_file".into(),
            description: "Open a local FITS file path in the FITS viewer.".into(),
            input_schema: json!({
                "type":"object",
                "properties": { "path": { "type":"string" } },
                "required": ["path"], "additionalProperties": false
            }),
            verb: VerbClass::Write,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "close_active_tab".into(),
            description: "Close the active tab of the current module.".into(),
            input_schema: empty,
            verb: VerbClass::Write,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "set_search_focus".into(),
            description: "Navigate to Search and pre-fill a sky position (RA/Dec in degrees).".into(),
            input_schema: json!({
                "type":"object",
                "properties": { "ra": { "type":"number" }, "dec": { "type":"number" } },
                "required": ["ra","dec"], "additionalProperties": false
            }),
            verb: VerbClass::Write,
            agent_safe: true,
        },
    ]
}

pub async fn dispatch(
    name: &str,
    services: &AppServices,
    args: &Value,
    proposals: &Arc<InMemoryProposalStore>,
) -> Option<ToolResult> {
    match name {
        "get_current_view" => Some(ToolResult::Data(current_view_payload(
            &view_state::capture(),
            services,
            proposals,
        ))),
        "list_open_tabs" => Some(ToolResult::Data(open_tabs_payload(&view_state::capture()))),
        "navigate_to" => {
            let view = args.get("view").and_then(|v| v.as_str()).unwrap_or("");
            let ok = view_state::navigate_to(view).await;
            Some(ToolResult::Data(json!({ "navigated": ok, "view": view })))
        }
        "open_fits_file" => {
            let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let ok = view_state::open_fits(path).await;
            Some(ToolResult::Data(json!({ "opened": ok, "path": path })))
        }
        "close_active_tab" => {
            let ok = view_state::close_active_tab().await;
            Some(ToolResult::Data(json!({ "closed": ok })))
        }
        "set_search_focus" => {
            let ra = args.get("ra").and_then(|v| v.as_f64());
            let dec = args.get("dec").and_then(|v| v.as_f64());
            match (ra, dec) {
                (Some(ra), Some(dec)) => {
                    let ok = view_state::set_search_focus_action(ra, dec).await;
                    Some(ToolResult::Data(
                        json!({ "focused": ok, "ra": ra, "dec": dec }),
                    ))
                }
                _ => Some(ToolResult::Failed("ra and dec are required".into())),
            }
        }
        _ => None,
    }
}

/// View-state tools never enqueue proposals — they execute directly.
pub async fn apply(
    _services: &AppServices,
    _proposal: &PendingProposal,
) -> Option<Result<String, String>> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptors_unique_and_agent_safe() {
        let d = descriptors();
        assert!(!d.is_empty());
        let mut names: Vec<_> = d.iter().map(|x| x.name.clone()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), d.len());
        assert!(d.iter().all(|x| x.agent_safe));
    }

    /// Every field of the reference's `AppViewSnapshot`, camelCased by its
    /// serializer. Transcribed from `Mcp/Tools/ViewState/ViewStateTools.cs`.
    const REFERENCE_SNAPSHOT_FIELDS: &[&str] = &[
        "mode",
        "modeTitle",
        "isAuthenticated",
        "username",
        "searchFocusRA",
        "searchFocusDec",
        "openFitsPaths",
        "agentsEnabled",
        "autoApplyEnabled",
        "followAgentActivityEnabled",
        "pendingProposalsCount",
        "proposalBudget",
    ];

    fn sample_snapshot() -> view_state::ViewSnapshot {
        view_state::ViewSnapshot {
            view: "search".into(),
            title: "Search".into(),
            authenticated: true,
            username: Some("astro".into()),
            search_focus_ra: Some(10.68),
            search_focus_dec: Some(41.27),
            open_fits_paths: vec!["/data/a.fits".into()],
            open_notebooks: vec!["/nb/one.ipynb".into()],
            open_cubes: vec![],
            active_fits: Some(0),
            active_notebook: Some(0),
            active_cube: None,
        }
    }

    #[test]
    fn the_view_snapshot_carries_every_field_the_reference_promises() {
        let rt = tokio::runtime::Runtime::new().expect("build a tokio runtime");
        let (services, _rx) = AppServices::new(rt.handle().clone());
        let proposals = Arc::new(InMemoryProposalStore::new());

        let payload = current_view_payload(&sample_snapshot(), &services, &proposals);
        let obj = payload.as_object().expect("an object");
        for field in REFERENCE_SNAPSHOT_FIELDS {
            assert!(
                obj.contains_key(*field),
                "`{field}` is missing — an agent written against the Windows app reads it"
            );
        }
        // Serializing the internal snapshot used to emit these instead.
        for leaked in [
            "view",
            "title",
            "authenticated",
            "open_fits_paths",
            "search_focus_ra",
        ] {
            assert!(
                !obj.contains_key(leaked),
                "`{leaked}` is the internal Rust field name, not the wire contract"
            );
        }
    }

    #[test]
    fn the_snapshot_reports_the_values_an_agent_throttles_against() {
        let rt = tokio::runtime::Runtime::new().expect("build a tokio runtime");
        let (services, _rx) = AppServices::new(rt.handle().clone());
        let proposals = Arc::new(InMemoryProposalStore::new());
        proposals.enqueue("save_query", "one", false, json!({}));
        proposals.enqueue("save_query", "two", false, json!({}));

        let payload = current_view_payload(&sample_snapshot(), &services, &proposals);
        assert_eq!(payload["pendingProposalsCount"], 2);

        // The budget must be quoted from the SAME policy the router enforces —
        // an agent that trusts a larger remaining count than the router allows
        // walks straight into a refusal it was told would not happen.
        let cap = services.proposal_budget.cap();
        assert_eq!(payload["proposalBudget"]["cap"], cap);
        assert_eq!(payload["proposalBudget"]["remaining"], cap - 2);
    }

    #[test]
    fn open_tab_counts_are_numbers_not_path_arrays() {
        // The reference types these as ints. They held arrays here, so an agent
        // testing `notebooks > 0` was comparing against an array and one reading
        // `.length` on the reference's int got nothing — a silent disagreement
        // on the same three key names.
        let payload = open_tabs_payload(&sample_snapshot());
        assert_eq!(payload["notebooks"], 1);
        assert_eq!(payload["fitsViewers"], 1);
        assert_eq!(payload["cubes"], 0);
        assert!(
            payload["fits"].is_null(),
            "`fits` was our name for the FITS count; the reference calls it fitsViewers"
        );
    }

    #[test]
    fn a_tab_entry_carries_what_the_switch_tools_address_it_by() {
        let payload = open_tabs_payload(&sample_snapshot());
        let tab = &payload["fitsTabs"][0];
        assert_eq!(tab["index"], 0);
        assert_eq!(tab["name"], "a.fits", "the display name is the basename");
        assert_eq!(tab["active"], true);
        assert_eq!(tab["path"], "/data/a.fits");
    }

    #[test]
    fn a_signed_out_user_reports_an_empty_username_not_null() {
        // The reference's record types Username as a non-null string and sends
        // "" when signed out; a null would break a strictly-typed client.
        let rt = tokio::runtime::Runtime::new().expect("build a tokio runtime");
        let (services, _rx) = AppServices::new(rt.handle().clone());
        let proposals = Arc::new(InMemoryProposalStore::new());

        let mut snap = sample_snapshot();
        snap.authenticated = false;
        snap.username = None;

        let payload = current_view_payload(&snap, &services, &proposals);
        assert_eq!(payload["username"], "");
        assert_eq!(payload["isAuthenticated"], false);
    }
}
