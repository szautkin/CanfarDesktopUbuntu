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

pub fn descriptors() -> Vec<ToolDescriptor> {
    let empty = json!({"type":"object","properties":{},"additionalProperties":false});
    vec![
        ToolDescriptor {
            name: "get_current_view".into(),
            description: "What the user is currently looking at: view, title, auth, search focus, open documents.".into(),
            input_schema: empty.clone(),
            verb: VerbClass::Read,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "list_open_tabs".into(),
            description: "The FITS files, notebooks and cubes currently open. `fitsTabs` / `cubeTabs` / \
                          `notebookTabs` carry each tab's 0-based index, display name and whether it is \
                          ACTIVE — switch_fits_tab, switch_cube_tab and blink_fits_tabs all address a tab \
                          by that index, and blink needs a partner different from the active one."
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
    _services: &AppServices,
    args: &Value,
    _proposals: &Arc<InMemoryProposalStore>,
) -> Option<ToolResult> {
    match name {
        "get_current_view" => {
            let snap = view_state::capture();
            Some(ToolResult::Data(
                serde_json::to_value(&snap).unwrap_or(json!({})),
            ))
        }
        "list_open_tabs" => {
            let snap = view_state::capture();
            // Each list is reported BOTH flat (the long-standing shape) and as
            // structured tabs carrying the index, display name and active flag —
            // switch_fits_tab / switch_cube_tab / blink_fits_tabs all address a
            // tab by index, and blink needs to know which one is already active.
            let tabs = |paths: &[String], active: Option<usize>| -> Vec<serde_json::Value> {
                paths
                    .iter()
                    .enumerate()
                    .map(|(i, p)| {
                        json!({
                            "index": i,
                            "path": p,
                            "name": std::path::Path::new(p)
                                .file_name()
                                .map(|n| n.to_string_lossy().to_string())
                                .unwrap_or_else(|| p.clone()),
                            "active": active == Some(i),
                        })
                    })
                    .collect()
            };
            Some(ToolResult::Data(json!({
                "fits": snap.open_fits_paths,
                "notebooks": snap.open_notebooks,
                "cubes": snap.open_cubes,
                "fitsTabs": tabs(&snap.open_fits_paths, snap.active_fits),
                "notebookTabs": tabs(&snap.open_notebooks, snap.active_notebook),
                "cubeTabs": tabs(&snap.open_cubes, snap.active_cube),
            })))
        }
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
}
