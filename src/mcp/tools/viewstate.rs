//! Live view-state MCP tools: let an agent see what the user is looking at and
//! steer the app (navigate, open a FITS file, focus a search). Port of
//! `Mcp/Tools/ViewState/ViewStateTools.cs` + `TabTools.cs`.
//!
//! These are UI navigations, not data mutations, so they execute directly
//! (agent-safe) rather than going through the write-proposal pipeline.

use super::{ToolDescriptor, ToolResult, VerbClass};
use crate::mcp::tools::proposals::InMemoryProposalStore;
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

    // `None` when run_code is usable; otherwise why not, in the words the

    // tool itself would have answered with.

    let ai_compute_ready: Option<String> = {
        let settings = crate::services::ai_compute_service::AIComputeService::new();

        if settings.settings().is_enabled() {
            None
        } else {
            Some(
                "No AI compute image configured. Set one in Settings \u{25b8} AI compute."
                    .to_string(),
            )
        }
    };

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
        // Whether run_code can work, BEFORE it is called.
        //
        // It needs a compute image set in Settings ▸ AI compute, and said so
        // only when called — so an agent planning a session had no way to know
        // that half its plan was unavailable until it got there.
        "aiCompute": {
            "ready": ai_compute_ready.is_none(),
            "reason": ai_compute_ready,
        },
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

/// Read and RANGE-CHECK the sky position for `set_search_focus`.
///
/// The reference declares `raDeg`/`decDeg` and refuses anything outside
/// [0, 360] / [-90, 90]; ours took `ra`/`dec` and accepted whatever arrived, so
/// a dec of 200 quietly focused the form on a position that does not exist. The
/// old names still work.
fn sky_focus_args(args: &Value) -> Result<(f64, f64), String> {
    let read = |primary: &str, fallback: &str| {
        super::num_arg(args, primary).or_else(|| super::num_arg(args, fallback))
    };
    let (Some(ra), Some(dec)) = (read("raDeg", "ra"), read("decDeg", "dec")) else {
        return Err("raDeg and decDeg are required".to_string());
    };
    if !(0.0..=360.0).contains(&ra) {
        return Err(format!("raDeg must be in [0, 360] (got {ra})"));
    }
    if !(-90.0..=90.0).contains(&dec) {
        return Err(format!("decDeg must be in [-90, 90] (got {dec})"));
    }
    Ok((ra, dec))
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
            name: "remove_annotation".into(),
            description: "Delete one mark, by id, from whichever viewer holds it. Ids come from \
                          list_fits_annotations or list_cube_annotations. Removing a mark a \
                          PERSON drew is a real deletion of their work — check the `author` \
                          before you do."
                .into(),
            input_schema: json!({
                "type":"object",
                "properties": {"id": {"type":"string","description":"The annotation id."}},
                "required": ["id"],
                "additionalProperties": false
            }),
            verb: VerbClass::Write,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "select_annotation".into(),
            description: "Highlight one mark, so a person looking at the image can see WHICH \
                          one you mean. Works on either viewer — pass `id` from \
                          list_fits_annotations or list_cube_annotations and whichever holds \
                          it lights up; omit `id` to take the highlight away. Only one mark \
                          is lit at a time across the app, so selecting on one viewer clears \
                          the other. The highlighted mark is drawn differently and its row is \
                          picked out in the sidebar list."
                .into(),
            input_schema: json!({
                "type":"object",
                "properties": {
                    "id": {"type":"string","description":"Annotation id. Omit to clear."}
                },
                "additionalProperties": false
            }),
            verb: VerbClass::Write,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "update_annotation".into(),
            description: "Change a mark that is already drawn: its label, where it points, or \
                          how big it is. Works on either viewer — pass `id` from \
                          list_fits_annotations or list_cube_annotations, then any of `text`, \
                          `ra`/`dec`, `x`/`y` (`z` too on a cube, where coordinates are \
                          voxels), `radius`, `kind`. What you leave out is left alone. Correcting a \
                          mark this way keeps its id, so a reference you have already given \
                          someone still points at it."
                .into(),
            input_schema: json!({
                "type":"object",
                "properties": {
                    "id": {"type":"string","description":"The annotation id."},
                    "text": {"type":"string","description":"New label."},
                    "kind": {
                        "type":"string",
                        "enum":["rect","circle","callout","text"],
                        "description":"New shape. Turns a circle into a box or back without \
                                       losing the id, the label or the position."
                    },
                    "ra": {"type":"number","description":"New sky position, degrees."},
                    "dec": {"type":"number"},
                    "x": {"type":"number","description":"New image pixel position."},
                    "y": {"type":"number"},
                    "z": {"type":"number","description":"New channel, on a cube. Voxel coordinates keep whatever you leave out, so you can shift a mark in z alone."},
                    "radius": {
                        "type":"number",
                        "description":"New half-size, in IMAGE PIXELS unless you pass \
                                       `ra`/`dec` in the same call, in which case it is in \
                                       degrees like they are. Same rule as annotate_fits."
                    }
                },
                "required": ["id"],
                "additionalProperties": false
            }),
            verb: VerbClass::Write,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "clear_annotations".into(),
            description: "Delete every mark on the CURRENT tab of a viewer — the user's as \
                          well as yours. Pass `viewer` as \"fits\" or \"cube\". Other open \
                          tabs keep their marks, so the `cleared` count can be smaller than \
                          everything you have drawn; the reply names the file it cleared. \
                          There is no undo, so prefer remove_annotation for your own marks."
                .into(),
            input_schema: json!({
                "type":"object",
                "properties": {
                    "viewer": {"type":"string","enum":["fits","cube"]}
                },
                "required": ["viewer"],
                "additionalProperties": false
            }),
            verb: VerbClass::Write,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "get_job_status".into(),
            description: "Report a background job started by a long-running tool call — a large \
                          download, an export. Pass the `jobId` that call returned. Answers \
                          `status` (running / succeeded / failed), bytes transferred so far when \
                          the transfer reports them, and the result or error once it ends. Omit \
                          `jobId` to list every job still remembered, newest first."
                .into(),
            input_schema: json!({
                "type":"object",
                "properties": {
                    "jobId": { "type":"string", "description":
                        "The jobId from the tool call that started the work. Same value as its \
                         proposalId." }
                },
                "additionalProperties": false
            }),
            verb: VerbClass::Read,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "open_fits_file".into(),
            description: "Open a FITS file in the 2D viewer and switch the app to it — by local \
                          file path, or by the id or publisher id of a DOWNLOADED observation \
                          (from list_downloaded_observations; call download_observation first if \
                          it is not downloaded yet). Answers `opened` only when a tab actually \
                          appeared, with `localPath` and a `message` explaining any failure."
                .into(),
            input_schema: json!({
                "type":"object",
                "properties": {
                    "observationId": { "type":"string", "description":
                        "Id or publisher id of a downloaded observation. The listing gives a \
                         filename, not a path, so this is how you open your own downloads." },
                    "path": { "type":"string", "description":
                        "Local filesystem path, if you have one." }
                },
                "additionalProperties": false
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
            description: "Point the Search form at a sky position (ICRS RA/Dec in degrees) and bring \
                          it into view, so the user can refine an agent-suggested cone. Live-applied \
                          (no proposal)."
                .into(),
            input_schema: json!({
                "type":"object",
                "properties": {
                    "raDeg": { "type":"number", "minimum": 0, "maximum": 360 },
                    "decDeg": { "type":"number", "minimum": -90, "maximum": 90 }
                },
                "required": ["raDeg","decDeg"], "additionalProperties": false
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
        // One tool per operation for both viewers rather than one per viewer.
        // An id identifies a mark uniquely, so the caller should not have to
        // know which viewer is holding it — and would often not.
        "remove_annotation" => Some(on_whichever_viewer_holds("remove_annotation", args).await),
        // Was hardcoded to "fits", so a mark on a cube could be drawn and
        // listed but never corrected — an agent's only repair was to delete it
        // and draw another, which changes the id it had already quoted.
        "update_annotation" => Some(on_whichever_viewer_holds("update_annotation", args).await),
        "select_annotation" => Some(on_whichever_viewer_holds("select_annotation", args).await),
        "clear_annotations" => {
            let viewer = args
                .get("viewer")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_lowercase();
            if !matches!(viewer.as_str(), "fits" | "cube") {
                return Some(ToolResult::Failed(
                    "viewer must be \"fits\" or \"cube\"".to_string(),
                ));
            }
            Some(
                match crate::mcp::view_state::viewer_command(
                    &viewer,
                    "clear_annotations",
                    json!({}),
                )
                .await
                {
                    Ok(v) => ToolResult::Data(v),
                    Err(e) => ToolResult::Failed(e),
                },
            )
        }
        "get_job_status" => {
            let id = args
                .get("jobId")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if id.is_empty() {
                return Some(ToolResult::Data(json!({ "jobs": services.jobs.recent() })));
            }
            match services.jobs.get(&id) {
                Some(job) => Some(ToolResult::Data(
                    serde_json::to_value(job).unwrap_or(Value::Null),
                )),
                // A job the registry no longer holds is not the same as one
                // that failed, and saying so saves a caller from waiting on it.
                None => Some(ToolResult::Failed(format!(
                    "no job '{id}' — it may have finished long enough ago to be forgotten, or \
                     never started"
                ))),
            }
        }
        "open_fits_file" => {
            // Either spelling; the UI works out which it was given.
            let target = args
                .get("path")
                .and_then(|v| v.as_str())
                .filter(|s| !s.trim().is_empty())
                .or_else(|| args.get("observationId").and_then(|v| v.as_str()))
                .unwrap_or("")
                .trim()
                .to_string();
            if target.is_empty() {
                return Some(ToolResult::Failed(
                    "path or observationId is required".to_string(),
                ));
            }
            // Only when it came in as a path: an observationId is not one, and
            // resolving it is the UI's job.
            if args.get("path").is_some() {
                if let Err(e) = crate::helpers::local_path::reject_remote(
                    &target,
                    crate::helpers::local_path::FETCH_IT_FIRST,
                ) {
                    return Some(ToolResult::Failed(e));
                }
            }
            let outcome = view_state::open_fits(&target).await;
            Some(ToolResult::Data(json!({
                "opened": outcome.opened,
                "observationId": outcome.observation_id,
                "localPath": outcome.local_path,
                "message": outcome.message,
            })))
        }
        "close_active_tab" => {
            let ok = view_state::close_active_tab().await;
            // Silence was the complaint: this answered `closed: false` for
            // every call, with nothing to say why, and the documented
            // switch-then-close sequence could not work because
            // `switch_fits_tab` moves the viewer's focus and not the app's.
            // Per-module closing is still unwired; the FITS viewer has its own
            // tool, and saying so beats a bare false.
            let mut out = json!({ "closed": ok });
            if !ok {
                out["message"] = json!(
                    "close_active_tab does not reach a module's own tabs. To close a FITS tab \
                     use close_fits_tab (it takes a tabIndex, or acts on the active tab)."
                );
            }
            Some(ToolResult::Data(out))
        }
        "set_search_focus" => Some(match sky_focus_args(args) {
            Ok((ra, dec)) => {
                let applied = view_state::set_search_focus_action(ra, dec).await;
                // `applied`/`raDeg`/`decDeg` — the reference's Output record.
                ToolResult::Data(json!({ "applied": applied, "raDeg": ra, "decDeg": dec }))
            }
            Err(e) => ToolResult::Failed(e),
        }),
        _ => None,
    }
}

/// Run an id-addressed annotation op on whichever viewer is holding the mark.
///
/// An id identifies a mark uniquely, so a caller should not have to know which
/// viewer has it — and often could not, since a cube and a FITS can be open at
/// once. Each viewer answers `Err` for an id it does not hold, so the first
/// real success is the right one.
///
/// `select_annotation` alone accepts no id: that clears the highlight, and it
/// clears it on BOTH viewers, so "take your highlight back" cannot leave one
/// lit on the viewer the caller was not thinking about. For the same reason a
/// successful select clears the other viewer: one highlighted mark across the
/// app, because the highlight means "this is the one I mean".
async fn on_whichever_viewer_holds(op: &str, args: &Value) -> ToolResult {
    let id = args
        .get("id")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let Some(id) = id else {
        if op != "select_annotation" {
            return ToolResult::Failed(
                "id is required — get one from list_fits_annotations or list_cube_annotations"
                    .to_string(),
            );
        }
        for viewer in ["fits", "cube"] {
            let _ = view_state::viewer_command(viewer, op, args.clone()).await;
        }
        return ToolResult::Data(json!({ "selected": Value::Null }));
    };

    for viewer in ["fits", "cube"] {
        let Ok(v) = view_state::viewer_command(viewer, op, args.clone()).await else {
            continue;
        };
        // remove_annotation reports a miss in its payload rather than as an
        // error, so an Ok is not on its own proof the viewer held the mark.
        if v.get("removed").and_then(|r| r.as_bool()) == Some(false) {
            continue;
        }
        if op == "select_annotation" {
            let other = if viewer == "fits" { "cube" } else { "fits" };
            let _ = view_state::viewer_command(other, op, json!({})).await;
        }
        return ToolResult::Data(v);
    }
    ToolResult::Failed(format!(
        "no annotation '{id}' in either viewer — it may already be gone; \
         list_fits_annotations or list_cube_annotations show what is there"
    ))
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
    fn a_sky_focus_outside_the_celestial_sphere_is_refused() {
        // Previously accepted verbatim, so `decDeg: 200` focused the Search form
        // on a declination that does not exist.
        assert!(sky_focus_args(&json!({ "raDeg": 10.0, "decDeg": 200.0 })).is_err());
        assert!(sky_focus_args(&json!({ "raDeg": -1.0, "decDeg": 0.0 })).is_err());
        assert!(sky_focus_args(&json!({ "raDeg": 361.0, "decDeg": 0.0 })).is_err());
        assert!(sky_focus_args(&json!({ "raDeg": 10.0 })).is_err());

        // The poles and the RA wrap-point are legal, not off-by-one rejections.
        assert_eq!(
            sky_focus_args(&json!({ "raDeg": 0.0, "decDeg": -90.0 })).unwrap(),
            (0.0, -90.0)
        );
        assert_eq!(
            sky_focus_args(&json!({ "raDeg": 360.0, "decDeg": 90.0 })).unwrap(),
            (360.0, 90.0)
        );
    }

    #[test]
    fn the_sky_focus_accepts_the_reference_names_and_our_older_ones() {
        assert_eq!(
            sky_focus_args(&json!({ "raDeg": 10.68, "decDeg": 41.27 })).unwrap(),
            (10.68, 41.27)
        );
        assert_eq!(
            sky_focus_args(&json!({ "ra": 10.68, "dec": 41.27 })).unwrap(),
            (10.68, 41.27)
        );
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

    /// Everything a person can change about a mark, an agent can change too.
    ///
    /// The list is the UI's: move it, resize it, rename it, change its shape.
    /// `kind` was the one missing — an agent could draw a box or a circle and
    /// then never change its mind, while a person could not either, so it went
    /// unnoticed until the two surfaces were compared deliberately.
    ///
    /// Asserted against the SCHEMA because that is what an agent reads. A
    /// field the handler honours but never advertises may as well not exist.
    #[test]
    fn an_agent_can_change_everything_about_a_mark_that_a_person_can() {
        let d = descriptors();
        let update = d
            .iter()
            .find(|t| t.name == "update_annotation")
            .expect("update_annotation is advertised");
        let props = update.input_schema["properties"]
            .as_object()
            .expect("an object schema");
        for field in ["id", "text", "kind", "ra", "dec", "x", "y", "z", "radius"] {
            assert!(
                props.contains_key(field),
                "update_annotation cannot change `{field}`"
            );
        }
        // And the shapes it accepts are the model's, not a shorter list.
        let kinds = update.input_schema["properties"]["kind"]["enum"]
            .as_array()
            .expect("kind is an enum");
        for k in ["rect", "circle", "callout", "text"] {
            assert!(
                kinds.iter().any(|v| v == k),
                "update_annotation refuses the kind `{k}`"
            );
        }
    }

    /// The id-addressed mark tools reach either viewer.
    ///
    /// Each was once hardcoded to "fits", so a mark on a cube could be drawn
    /// and listed but never corrected or pointed out. The dispatch is shared
    /// now; this pins that they all still go through it.
    #[test]
    fn the_mark_tools_are_not_bound_to_one_viewer() {
        let d = descriptors();
        for name in [
            "update_annotation",
            "select_annotation",
            "remove_annotation",
        ] {
            let t = d
                .iter()
                .find(|t| t.name == name)
                .unwrap_or_else(|| panic!("{name} is advertised"));
            let text = format!("{} {}", t.description, t.input_schema);
            assert!(
                text.contains("list_cube_annotations"),
                "{name} does not tell an agent it works on a cube"
            );
        }
    }
}
