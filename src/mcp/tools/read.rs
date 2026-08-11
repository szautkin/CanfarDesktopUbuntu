//! Read-only, agent-safe MCP tools. Ported from `Mcp/Tools/Read/*.cs` +
//! `Mcp/Tools/Builtin/FoundationalTools.cs`.
//!
//! Every tool here is `verb: Read` / `agent_safe: true` — it has no side effects
//! and never mutates app state, so an external (agent) caller may reach it
//! directly. Mutating operations live in the write module and go through the
//! proposal pipeline instead.
//!
//! This module owns a fixed set of tool names. [`descriptors`] advertises them
//! for `tools/list`; [`dispatch`] runs one by name, returning `None` when the
//! name belongs to another module.

use crate::mcp::tools::{ToolDescriptor, ToolResult, VerbClass};
use crate::models::search_result::SearchFormState;
use serde_json::{json, Value};

/// Hard cap on rows requested from the backend and returned to the caller.
const MAX_ROWS_CAP: u32 = 1000;

/// Default cone radius (degrees) when a spatial target is given without a
/// radius (~1 arcmin), mirroring the C# `SearchObservationsTool`.
const DEFAULT_RADIUS_DEG: f64 = 0.0167;

/// A read tool descriptor with the invariant fields (`Read` / agent-safe) fixed.
fn read_tool(name: &str, description: &str, input_schema: Value) -> ToolDescriptor {
    ToolDescriptor {
        name: name.to_string(),
        description: description.to_string(),
        input_schema,
        verb: VerbClass::Read,
        agent_safe: true,
    }
}

/// The empty-object JSON Schema shared by no-argument tools.
fn empty_schema() -> Value {
    json!({ "type": "object", "properties": {}, "additionalProperties": false })
}

/// All read-only tool descriptors owned by this module.
pub fn descriptors() -> Vec<ToolDescriptor> {
    vec![
        read_tool(
            "describe_app",
            "Describe the Verbinal / CanfarDesktop app: what it is, its version, and what data it \
             can expose over MCP (read-only observation search, Skaha sessions, downloaded research \
             observations, VOSpace storage, FITS/WCS, and service health).",
            empty_schema(),
        ),
        read_tool(
            "get_auth_state",
            "Report whether the user is signed in to CADC/CANFAR, and the signed-in username.",
            empty_schema(),
        ),
        read_tool(
            "search_observations",
            "Search CADC observations. Provide either an explicit ADQL query, or a spatial cone \
             (target name OR ra+dec in degrees, with an optional radius in degrees and collection \
             filter). Returns column names and a capped set of rows (max 1000). If `truncated` is \
             true, more rows match than were returned — refine the query (tighter ADQL / smaller \
             cone / add filters) or you will silently work with a partial set.",
            json!({
                "type": "object",
                "properties": {
                    "adql": {"type": "string", "description": "Explicit ADQL query. If set, the spatial fields are ignored."},
                    "target": {"type": "string", "description": "Target name resolved to RA/Dec for a cone search (e.g. M31)."},
                    "ra": {"type": "number", "description": "ICRS Right Ascension in degrees (with dec)."},
                    "dec": {"type": "number", "description": "ICRS Declination in degrees (with ra)."},
                    "radius": {"type": "number", "description": "Cone radius in degrees (default ~0.0167, i.e. 1 arcmin)."},
                    "collection": {"type": "string", "description": "Optional collection filter (e.g. CFHT, JWST, HST)."},
                    "max": {"type": "integer", "minimum": 1, "maximum": 1000, "description": "Max rows to request/return (capped at 1000)."}
                },
                "additionalProperties": false
            }),
        ),
        read_tool(
            "resolve_target",
            "Resolve an astronomical target name (e.g. M31, NGC 224) to ICRS RA/Dec degrees using \
             the CADC name resolver.",
            json!({
                "type": "object",
                "properties": {
                    "target": {"type": "string", "description": "Target name to resolve (e.g. M31)."},
                    "service": {"type": "string", "description": "Resolver service: ALL (default), NED, SIMBAD, or VIZIER."}
                },
                "required": ["target"],
                "additionalProperties": false
            }),
        ),
        read_tool(
            "list_saved_queries",
            "List the user's saved ADQL queries (name + the ADQL text, ready to run or rewrite).",
            empty_schema(),
        ),
        read_tool(
            "get_saved_query",
            "Get one saved ADQL query by its exact name (the full ADQL text, ready to run via \
             search_observations).",
            json!({
                "type": "object",
                "properties": {"name": {"type": "string", "description": "Exact saved-query name."}},
                "required": ["name"],
                "additionalProperties": false
            }),
        ),
        read_tool(
            "list_recent_searches",
            "List the user's recent searches (summary, ADQL, result count, when run). Newest first; \
             optional limit.",
            json!({
                "type": "object",
                "properties": {"limit": {"type": "integer", "minimum": 1, "description": "Max entries to return."}},
                "additionalProperties": false
            }),
        ),
        read_tool(
            "list_sessions",
            "List the user's active Skaha sessions (id, name, type, status, image, resources, and \
             the connectUrl to open an interactive session in the browser).",
            empty_schema(),
        ),
        read_tool(
            "list_vospace_path",
            "List the contents of a VOSpace/ARC storage folder (name, type, size, last-modified). \
             Defaults to the user's home root when no path is given.",
            json!({
                "type": "object",
                "properties": {"path": {"type": "string", "description": "Folder path under the user's VOSpace home (default: root)."}},
                "additionalProperties": false
            }),
        ),
        read_tool(
            "list_downloaded_observations",
            "List the observations the user has downloaded or bookmarked into their local Research \
             library (publisher id, collection, target, instrument, filter, coordinates, local \
             path, size).",
            empty_schema(),
        ),
        read_tool(
            "get_service_health",
            "Probe the upstream CADC/CANFAR services (TAP search, Skaha sessions, ARC/VOSpace \
             storage, CADC auth) for reachability + round-trip latency. Use it to tell whether a \
             service is up before you depend on it (e.g. before search_observations or \
             launch_session). Per service, `reachable` means the HOST answered (any HTTP status) \
             while `ok` means the service answered sanely (not 404/5xx) — trust `ok`/`healthyCount` \
             for \"can I use it\", with `statusCode` as the detail.",
            empty_schema(),
        ),
    ]
}

/// Dispatch a read tool by name. Returns `Some(result)` when this module owns
/// `name`, `None` otherwise so the router can try another module.
pub async fn dispatch(
    name: &str,
    services: &crate::state::AppServices,
    args: &Value,
) -> Option<ToolResult> {
    let result = match name {
        "describe_app" => describe_app(),
        "get_auth_state" => get_auth_state(services).await,
        "search_observations" => search_observations(services, args).await,
        "resolve_target" => resolve_target(services, args).await,
        "list_saved_queries" => list_saved_queries(services),
        "get_saved_query" => get_saved_query(services, args),
        "list_recent_searches" => list_recent_searches(services, args),
        "list_sessions" => list_sessions(services).await,
        "list_vospace_path" => list_storage(services, args).await,
        "list_downloaded_observations" => list_observations(services),
        "get_service_health" => get_service_health(services).await,
        _ => return None,
    };
    Some(result)
}

// ---------------------------------------------------------------------------
// Argument helpers
// ---------------------------------------------------------------------------

fn arg_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn arg_f64(args: &Value, key: &str) -> Option<f64> {
    args.get(key).and_then(|v| v.as_f64())
}

fn arg_u64(args: &Value, key: &str) -> Option<u64> {
    args.get(key).and_then(|v| v.as_u64())
}

// ---------------------------------------------------------------------------
// Foundational
// ---------------------------------------------------------------------------

fn describe_app() -> ToolResult {
    ToolResult::Data(json!({
        "name": "Verbinal (CanfarDesktop)",
        "version": crate::mcp::constants::SERVER_VERSION,
        "summary": "Native Linux client for CADC / CANFAR (clone of the Windows CanfarDesktop). \
                    Exposes read-only observation search (ADQL + CAOM2 cone), Skaha sessions, a \
                    local research library of downloaded observations, VOSpace/ARC storage, FITS \
                    headers/WCS, and upstream service health.",
        "modules": [
            "observation_search", "sessions", "research_library",
            "vospace_storage", "fits_wcs", "service_health"
        ],
        "capabilities": {
            "readTools": true,
            "writeToolsViaProposals": true,
            "agentSafeReads": true
        }
    }))
}

async fn get_auth_state(services: &crate::state::AppServices) -> ToolResult {
    let username = services.get_username().await;
    ToolResult::Data(json!({
        "isAuthenticated": username.is_some(),
        "username": username,
    }))
}

// ---------------------------------------------------------------------------
// Search / resolve
// ---------------------------------------------------------------------------

async fn search_observations(services: &crate::state::AppServices, args: &Value) -> ToolResult {
    // Cap rows, then over-fetch ONE past the cap so we can detect truncation:
    // TAP returns CSV (no VOTable OVERFLOW flag), so the only way to tell whether
    // more rows matched is to ask for one more than we intend to return.
    let max_rows = match arg_u64(args, "max") {
        Some(n) if n > 0 => (n as u32).min(MAX_ROWS_CAP),
        _ => MAX_ROWS_CAP,
    };
    let probe = max_rows.saturating_add(1);

    let token = services.get_token().await;

    // Build the ADQL: explicit query wins; otherwise a spatial cone.
    let adql = if let Some(adql) = arg_str(args, "adql") {
        adql.to_string()
    } else {
        // Determine RA/Dec from ra+dec, or by resolving a target name.
        let (ra, dec) = match (arg_f64(args, "ra"), arg_f64(args, "dec")) {
            (Some(ra), Some(dec)) => (ra, dec),
            _ => match arg_str(args, "target") {
                Some(target) => {
                    match services
                        .tap
                        .resolve_target(target, "ALL", token.as_deref())
                        .await
                    {
                        Ok(r) => (r.ra, r.dec),
                        Err(e) => {
                            return ToolResult::Failed(format!(
                                "could not resolve target '{target}': {e}"
                            ))
                        }
                    }
                }
                None => {
                    return ToolResult::Failed(
                        "Provide 'adql', or a spatial cone via 'target' or 'ra'+'dec'.".to_string(),
                    )
                }
            },
        };

        let radius = match arg_f64(args, "radius") {
            Some(r) if r > 0.0 => r,
            _ => DEFAULT_RADIUS_DEG,
        };

        // Reuse the app's real ADQL builder (helpers::adql_builder) so the MCP
        // cone query matches what the search UI produces (quality filter,
        // CIRCLE('ICRS', …) INTERSECTS, collection IN clause, SELECT columns).
        let mut state = SearchFormState::new();
        state.resolved_ra = Some(ra);
        state.resolved_dec = Some(dec);
        state.search_radius = radius;
        if let Some(collection) = arg_str(args, "collection") {
            state.collection = collection.to_string();
        }
        state.max_records = probe;
        crate::helpers::adql_builder::build(&state)
    };

    match services
        .tap
        .execute_query(&adql, probe, token.as_deref())
        .await
    {
        Ok(results) => {
            let truncated = results.rows.len() as u32 > max_rows;
            let cols = &results.columns;
            let rows: Vec<Vec<String>> = results
                .rows
                .iter()
                .take(max_rows as usize)
                .map(|r| cols.iter().map(|c| r.get(c).to_string()).collect())
                .collect();

            ToolResult::Data(json!({
                "adql": adql,
                "columns": cols,
                "returnedRows": rows.len(),
                "truncated": truncated,
                "rows": rows,
            }))
        }
        Err(e) => ToolResult::Failed(format!("observation search failed: {e}")),
    }
}

async fn resolve_target(services: &crate::state::AppServices, args: &Value) -> ToolResult {
    let target = match arg_str(args, "target") {
        Some(t) => t,
        None => return ToolResult::Failed("target is required".to_string()),
    };
    let service = arg_str(args, "service").unwrap_or("ALL");
    let token = services.get_token().await;

    match services
        .tap
        .resolve_target(target, service, token.as_deref())
        .await
    {
        Ok(r) => ToolResult::Data(json!({
            "target": r.target,
            "ra": r.ra,
            "dec": r.dec,
            "coordSys": r.coord_sys,
            "objectType": r.object_type,
            "service": r.service,
        })),
        Err(e) => ToolResult::Failed(format!("could not resolve target '{target}': {e}")),
    }
}

// ---------------------------------------------------------------------------
// Saved / recent (local stores — no network, no token)
// ---------------------------------------------------------------------------

fn list_saved_queries(services: &crate::state::AppServices) -> ToolResult {
    let queries: Vec<Value> = services
        .search_store
        .load_saved()
        .into_iter()
        .map(|q| json!({ "name": q.name, "adql": q.adql, "createdAt": q.created_at }))
        .collect();
    ToolResult::Data(json!({ "count": queries.len(), "queries": queries }))
}

fn get_saved_query(services: &crate::state::AppServices, args: &Value) -> ToolResult {
    let name = match arg_str(args, "name") {
        Some(n) => n,
        None => return ToolResult::Failed("name is required".to_string()),
    };
    match services
        .search_store
        .load_saved()
        .into_iter()
        .find(|q| q.name == name)
    {
        Some(q) => ToolResult::Data(json!({
            "name": q.name, "adql": q.adql, "createdAt": q.created_at
        })),
        None => ToolResult::Failed(format!("no saved query named '{name}'")),
    }
}

fn list_recent_searches(services: &crate::state::AppServices, args: &Value) -> ToolResult {
    let mut recent = services.search_store.load_recent();
    if let Some(limit) = arg_u64(args, "limit") {
        recent.truncate(limit as usize);
    }
    let searches: Vec<Value> = recent
        .into_iter()
        .map(|s| {
            json!({
                "summary": s.summary,
                "adql": s.adql,
                "resultCount": s.result_count,
                "searchedAt": s.searched_at,
            })
        })
        .collect();
    ToolResult::Data(json!({ "count": searches.len(), "searches": searches }))
}

// ---------------------------------------------------------------------------
// Sessions / storage (network — require a token)
// ---------------------------------------------------------------------------

async fn list_sessions(services: &crate::state::AppServices) -> ToolResult {
    let token = match services.get_token().await {
        Some(t) => t,
        None => {
            return ToolResult::Failed("not signed in (sign in to CADC/CANFAR first)".to_string())
        }
    };
    match services.sessions.get_sessions(&token).await {
        Ok(sessions) => {
            let items: Vec<Value> = sessions
                .into_iter()
                .map(|s| {
                    json!({
                        "id": s.id,
                        "name": s.name,
                        "type": s.session_type,
                        "status": s.status,
                        "image": s.image,
                        "startedTime": s.start_time,
                        "expiresTime": s.expiry_time,
                        "cpuAllocated": s.requested_cpu_cores,
                        "memoryAllocated": s.requested_ram,
                        "gpuAllocated": s.requested_gpu_cores,
                        "connectUrl": s.connect_url,
                    })
                })
                .collect();
            ToolResult::Data(json!({ "count": items.len(), "sessions": items }))
        }
        Err(e) => ToolResult::Failed(format!("could not list sessions: {e}")),
    }
}

async fn list_storage(services: &crate::state::AppServices, args: &Value) -> ToolResult {
    let token = match services.get_token().await {
        Some(t) => t,
        None => {
            return ToolResult::Failed("not signed in (sign in to CADC/CANFAR first)".to_string())
        }
    };
    let username = match services.get_username().await {
        Some(u) => u,
        None => return ToolResult::Failed("no signed-in username available".to_string()),
    };
    let path = arg_str(args, "path").unwrap_or("");

    match services.vospace.list_nodes(&token, &username, path).await {
        Ok(nodes) => {
            let items: Vec<Value> = nodes
                .into_iter()
                .map(|n| {
                    json!({
                        "name": n.name,
                        "uri": n.uri,
                        "type": if n.is_container() { "container" } else { "data" },
                        "size": n.size,
                        "sizeDisplay": n.size_display(),
                        "date": n.date,
                        "contentType": n.content_type,
                        "isPublic": n.is_public,
                    })
                })
                .collect();
            ToolResult::Data(json!({ "path": path, "count": items.len(), "nodes": items }))
        }
        Err(e) => ToolResult::Failed(format!("could not list storage '{path}': {e}")),
    }
}

// ---------------------------------------------------------------------------
// Research library (local store — no network)
// ---------------------------------------------------------------------------

fn list_observations(services: &crate::state::AppServices) -> ToolResult {
    // Rendered through the shared summary so this list and
    // `get_downloaded_observation` cannot describe the same record differently.
    let items: Vec<Value> = services
        .observation_store
        .load()
        .iter()
        .map(super::research::observation_summary)
        .collect();
    ToolResult::Data(json!({ "count": items.len(), "observations": items }))
}

// ---------------------------------------------------------------------------
// Service health (last-known snapshot from the tracker)
// ---------------------------------------------------------------------------

async fn get_service_health(services: &crate::state::AppServices) -> ToolResult {
    // Probe live rather than replaying the tracker's last-known state: the tool
    // is asked "can I use this right now?", and the tracker carries no status
    // code, URL or latency to answer with.
    let endpoints = services.endpoints.clone();
    let results = services
        .spawn(async move {
            let client = reqwest::Client::new();
            crate::services::probe_core(&client, &endpoints).await
        })
        .await;

    let reachable_count = results.iter().filter(|r| r.reachable).count();
    let healthy_count = results.iter().filter(|r| r.ok).count();
    let entries: Vec<Value> = results
        .iter()
        .map(|r| {
            json!({
                "service": r.name,
                "url": r.url,
                "reachable": r.reachable,
                "ok": r.ok,
                "statusCode": r.status,
                "latencyMs": r.latency_ms,
                "error": r.error,
            })
        })
        .collect();

    ToolResult::Data(json!({
        "count": entries.len(),
        "reachableCount": reachable_count,
        "healthyCount": healthy_count,
        "services": entries,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn descriptors_are_read_and_agent_safe() {
        for d in descriptors() {
            assert_eq!(d.verb, VerbClass::Read, "{} must be a Read tool", d.name);
            assert!(d.agent_safe, "{} must be agent-safe", d.name);
            assert!(
                !d.description.trim().is_empty(),
                "{} needs a description",
                d.name
            );
            assert!(
                d.input_schema.is_object(),
                "{} needs an object schema",
                d.name
            );
        }
    }

    #[test]
    fn descriptor_names_are_unique_and_non_empty() {
        let names: Vec<String> = descriptors().into_iter().map(|d| d.name).collect();
        assert!(!names.is_empty(), "there must be at least one read tool");
        for name in &names {
            assert!(!name.trim().is_empty(), "tool names must be non-empty");
        }
        let unique: HashSet<&String> = names.iter().collect();
        assert_eq!(unique.len(), names.len(), "tool names must be unique");
    }
}
