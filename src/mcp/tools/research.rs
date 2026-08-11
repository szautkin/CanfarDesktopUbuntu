//! Research-module tool family — read + write tools over the local
//! `ObservationStore` that backs the Research library.
//!
//! Ported from `Mcp/Tools/Read/ResearchReadTools.cs` (the read half) and
//! `Mcp/Tools/Write/ObservationWriteTools.cs` (the delete/clear write half).
//! Only the subset the Verbinal `ObservationStore` actually supports is exposed:
//!
//! * `get_downloaded_observation` — one observation by its local id (or publisher
//!   id), read-only.
//! * `get_observation_notes` — the astronomer-notes surface, read from the
//!   standalone [`ObservationNoteStore`] (the same store the Research page and the
//!   `update_observation_note` / `bulk_update_observation_notes` writes use).
//! * `delete_downloaded_observation` — propose removing one observation (its
//!   record + managed files). Destructive.
//! * `clear_research_archive` — propose removing EVERY observation. Destructive.
//!
//! Reads dispatch straight against `services.observation_store`. Writes NEVER
//! mutate at propose time: they enqueue a [`PendingProposal`]; the real store
//! mutation happens in [`apply`] once the user approves.

use crate::helpers::agent_attribution::AgentAttribution;
use crate::mcp::tools::proposals::{InMemoryProposalStore, PendingProposal};
use crate::mcp::tools::{opt_u64, str_arg, ToolDescriptor, ToolResult, VerbClass};
use crate::models::observation_note::ObservationNote;
use crate::services::observation_note_store::ObservationNoteStore;
use crate::services::observation_store::DownloadedObservation;
use crate::state::AppServices;
use chrono::Utc;
use serde_json::{json, Value};
use std::sync::Arc;

// ─────────────────────────────────────────────────────────────────────────────
// Manifest
// ─────────────────────────────────────────────────────────────────────────────

/// Descriptors advertised for the research family. Reads are `verb: Read` /
/// `agent_safe: true`; writes are `verb: Write` / `agent_safe: true` (the proposal
/// gate — not the manifest — keeps them from taking effect without a human).
pub fn descriptors() -> Vec<ToolDescriptor> {
    vec![
        ToolDescriptor {
            name: "download_observation".to_string(),
            description: "Download a CADC observation's FITS file into the user's Research library by \
                          its publisher id (from search_observations). Optional `artifactIndex` picks a \
                          SPECIFIC product from the observation's DataLink set — a moment map, an \
                          integrated spectrum — instead of the default science file; read the set with \
                          get_data_links first. Proprietary or embargoed collections require the user to \
                          be signed in to CADC. Queues for the user to apply; once applied it appears in \
                          list_downloaded_observations."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "publisherId": {"type": "string", "description": "Publisher DID, e.g. ivo://cadc.nrc.ca/CFHT?1234567p"},
                    "artifactIndex": {"type": "integer", "minimum": 0, "description": "0-based index into the observation's resolved DataLink artifacts"}
                },
                "required": ["publisherId"],
                "additionalProperties": false
            }),
            verb: VerbClass::Write,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "download_observations_bulk".to_string(),
            description: "Download up to 50 observations as ONE proposal, so the user approves a batch \
                          with a single click. Each item takes the same shape as download_observation. \
                          The applier fetches them in sequence and reports how many succeeded; a failure \
                          part-way stops the rest rather than pressing on silently. Large batches of big \
                          FITS files can outlast the MCP request timeout — prefer groups of ~10."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "items": {
                        "type": "array", "minItems": 1, "maxItems": 50,
                        "items": {
                            "type": "object",
                            "properties": {
                                "publisherId": {"type": "string"},
                                "artifactIndex": {"type": "integer", "minimum": 0}
                            },
                            "required": ["publisherId"],
                            "additionalProperties": false
                        }
                    }
                },
                "required": ["items"],
                "additionalProperties": false
            }),
            verb: VerbClass::Write,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "get_downloaded_observation".to_string(),
            description: "Get one observation from the user's Research library by its local id \
                (from list_downloaded_observations) or by its CADC publisher id. Returns the \
                stored metadata (target, collection, instrument, filter, coordinates, local \
                filename and size)."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Local observation id, or the CADC publisher id."
                    }
                },
                "required": ["id"],
                "additionalProperties": false
            }),
            verb: VerbClass::Read,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "get_observation_notes".to_string(),
            description: "Get the user's research notes (rating + free-text note + tags) for \
                downloaded observations. With no id it returns every note; with an id (a CADC \
                publisher id or a local id) it returns just that observation's note."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Optional: restrict to one observation's local or publisher id."
                    }
                },
                "additionalProperties": false
            }),
            verb: VerbClass::Read,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "update_observation_note".to_string(),
            description: "Set the user's research note for a downloaded observation: a 0–5 star \
                rating, a free-text note, and tags. Identify the observation by its CADC publisher \
                id (or a local id from list_downloaded_observations). Only the fields you pass are \
                changed; the rest are kept. Queues a reversible write for approval — on apply it \
                upserts the note (an all-blank note clears it)."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "The observation's CADC publisher id, or a local id." },
                    "rating": { "type": "integer", "minimum": 0, "maximum": 5, "description": "Star rating 0–5 (0 = unrated)." },
                    "note": { "type": "string", "description": "Free-text note body." },
                    "tags": { "type": "array", "items": { "type": "string" }, "description": "Tag list (replaces the existing tags)." }
                },
                "required": ["id"],
                "additionalProperties": false
            }),
            verb: VerbClass::Write,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "bulk_update_observation_notes".to_string(),
            description: "Set research notes for several observations at once (1–50). Each item \
                targets one observation by publisher/local id and may set rating, note, and tags. \
                Queues a single reversible write for approval; on apply each note is upserted."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "items": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": 50,
                        "items": {
                            "type": "object",
                            "properties": {
                                "id": { "type": "string" },
                                "rating": { "type": "integer", "minimum": 0, "maximum": 5 },
                                "note": { "type": "string" },
                                "tags": { "type": "array", "items": { "type": "string" } }
                            },
                            "required": ["id"],
                            "additionalProperties": false
                        }
                    }
                },
                "required": ["items"],
                "additionalProperties": false
            }),
            verb: VerbClass::Write,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "delete_downloaded_observation".to_string(),
            description: "Propose removing one observation from the Research library by its local \
                id (from list_downloaded_observations) or its publisher id — deletes both the \
                record and its managed files. Queues for the user to apply (a destructive change)."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Local observation id, or the CADC publisher id."
                    }
                },
                "required": ["id"],
                "additionalProperties": false
            }),
            verb: VerbClass::Write,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "clear_research_archive".to_string(),
            description: "Propose removing ALL observations from the Research library — every \
                record and its managed files (file deletion is best-effort). Queues for the user \
                to apply (a destructive change)."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
            verb: VerbClass::Write,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "get_preview_image".to_string(),
            description: "Fetch a CADC observation's preview image (resolves its DataLink #preview \
                URL and returns the image itself, authenticated + size-bounded)."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "publisherId": { "type": "string", "description": "The observation's publisher DID / id." }
                },
                "required": ["publisherId"],
                "additionalProperties": false
            }),
            verb: VerbClass::Read,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "export_research_bundle".to_string(),
            description: "Export a Claude-friendly research bundle — the user's downloaded \
                observations, research notes, and saved/recent searches rendered as JSON + markdown \
                (with fenced sql query blocks) — packed into a single store-only .zip at the given \
                path. Non-destructive: queues for the user to apply, then writes the archive \
                (creating parent folders). Set include_notes / include_search_history to false to \
                omit those sections."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Full local path for the output .zip (parent folders are created)."
                    },
                    "includeNotes": {
                        "type": "boolean",
                        "description": "Include research notes (default true)."
                    },
                    "includeSearchHistory": {
                        "type": "boolean",
                        "description": "Include saved + recent searches (default true)."
                    }
                },
                "required": ["path"],
                "additionalProperties": false
            }),
            verb: VerbClass::Write,
            agent_safe: true,
        },
    ]
}

/// Preview fetches are bounded so a hostile/oversized URL can't exhaust memory.
const MAX_PREVIEW_BYTES: usize = 16 * 1024 * 1024;

// ─────────────────────────────────────────────────────────────────────────────
// Dispatch — reads hit the store directly; writes enqueue a proposal
// ─────────────────────────────────────────────────────────────────────────────

/// Handle a research-family call. Returns `Some(..)` if `name` is one of this
/// family's tools (so the router stops chaining), or `None` otherwise.
pub async fn dispatch(
    name: &str,
    services: &AppServices,
    args: &Value,
    proposals: &Arc<InMemoryProposalStore>,
) -> Option<ToolResult> {
    let result = match name {
        "get_downloaded_observation" => get_downloaded_observation(services, args).await,
        "get_observation_notes" => get_observation_notes(services, args).await,
        "get_preview_image" => get_preview_image(services, args).await,
        "update_observation_note" => propose_update_note(args, proposals),
        "bulk_update_observation_notes" => propose_bulk_update_notes(args, proposals),
        "download_observation" => propose_download(args, proposals),
        "download_observations_bulk" => propose_download_bulk(args, proposals),
        "delete_downloaded_observation" => propose_delete(args, proposals),
        "clear_research_archive" => propose_clear(proposals),
        "export_research_bundle" => propose_export(args, proposals),
        _ => return None,
    };
    Some(result)
}

async fn get_preview_image(services: &AppServices, args: &Value) -> ToolResult {
    let pid = str_arg(args, "publisher_id");
    if pid.is_empty() {
        return ToolResult::Failed("publisher_id is required".to_string());
    }
    match crate::mcp::preview::fetch_observation_preview(services, &pid, MAX_PREVIEW_BYTES).await {
        Ok((bytes, mime)) => {
            use base64::Engine as _;
            let data_base64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
            ToolResult::Image {
                data_base64,
                mime,
                caption: Some(format!("Preview of {pid}")),
            }
        }
        Err(e) => ToolResult::Failed(format!("preview fetch failed: {e}")),
    }
}

async fn get_downloaded_observation(services: &AppServices, args: &Value) -> ToolResult {
    let id = str_arg(args, "id");
    if id.is_empty() {
        return ToolResult::Failed("id is required".to_string());
    }
    let list = services.observation_store.load_async().await;
    match find_observation(&list, &id) {
        Some(obs) => ToolResult::Data(observation_summary(obs)),
        None => ToolResult::Failed(format!("no downloaded observation with id '{}'", id)),
    }
}

/// Notes surface — reads the standalone [`ObservationNoteStore`] (the same store
/// the Research page writes to). With no `id` it returns every note; with an `id`
/// it resolves a local id to its publisher id (falling back to treating the id as
/// a publisher id) and returns just that observation's note. Mirrors the Windows
/// `GetObservationNotesTool` (which reads `ObservationNoteStore.All()`).
async fn get_observation_notes(services: &AppServices, args: &Value) -> ToolResult {
    let id = str_arg(args, "id");
    // Resolve an optional filter to a publisher id (accepting a local id too).
    let filter_pub: Option<String> = if id.is_empty() {
        None
    } else {
        let list = services.observation_store.load_async().await;
        Some(
            find_observation(&list, &id)
                .map(|o| o.publisher_id.clone())
                .unwrap_or(id),
        )
    };

    let notes: Vec<Value> = ObservationNoteStore::new()
        .all()
        .iter()
        .filter(|n| filter_pub.as_ref().is_none_or(|p| &n.publisher_id == p))
        .map(note_summary)
        .collect();

    ToolResult::Data(json!({
        "count": notes.len(),
        "notes": notes,
        "filteredBy": filter_pub.map(Value::String).unwrap_or(Value::Null),
    }))
}

/// Compact JSON view of one research note.
fn note_summary(n: &ObservationNote) -> Value {
    json!({
        "publisherId": n.publisher_id,
        "rating": n.rating,
        "note": n.note,
        "tags": n.tags,
        // `updatedUtc` in the reference's NoteView — the name carries the
        // timezone, which a bare `updated` does not.
        "updatedUtc": n.updated,
        "agentAttribution": serde_json::to_value(&n.agent_attribution).unwrap_or(Value::Null),
    })
}

/// Queue a single note upsert (reversible — routes through the proposal gate so
/// an agent write is still user-approvable / attributed).
fn propose_update_note(args: &Value, proposals: &Arc<InMemoryProposalStore>) -> ToolResult {
    let id = str_arg(args, "id");
    if id.is_empty() {
        return ToolResult::Failed("id is required".to_string());
    }
    // Preserve only the fields the caller actually set (apply merges the rest).
    let mut payload = serde_json::Map::new();
    payload.insert("id".to_string(), json!(id));
    for key in ["rating", "note", "tags"] {
        if let Some(v) = args.get(key) {
            payload.insert(key.to_string(), v.clone());
        }
    }
    let p = proposals.enqueue(
        "update_observation_note",
        &format!("Set research note for {}", id),
        false,
        Value::Object(payload),
    );
    ToolResult::Proposed(p)
}

/// Queue a bulk note upsert (1–50 items).
fn propose_bulk_update_notes(args: &Value, proposals: &Arc<InMemoryProposalStore>) -> ToolResult {
    let items = match args.get("items").and_then(Value::as_array) {
        Some(items) if !items.is_empty() => items.clone(),
        _ => return ToolResult::Failed("items (a non-empty array) is required".to_string()),
    };
    if items.len() > 50 {
        return ToolResult::Failed("at most 50 items may be updated at once".to_string());
    }
    let p = proposals.enqueue(
        "bulk_update_observation_notes",
        &format!("Set research notes for {} observations", items.len()),
        false,
        json!({ "items": items }),
    );
    ToolResult::Proposed(p)
}

/// Cap on one bulk envelope, matching the reference. A larger batch is not
/// refused arbitrarily: the applier runs the transfers in sequence, so the
/// in-flight time grows with the count and eventually outlasts the transport.
const MAX_BULK_DOWNLOADS: usize = 50;

/// Read `{publisherId, artifactIndex?}` from one item, validating the id.
fn download_item(value: &Value) -> Result<Value, String> {
    let pid = str_arg(value, "publisherId");
    if pid.is_empty() {
        return Err("publisherId is required".to_string());
    }
    let mut item = json!({ "publisherId": pid });
    if let Some(index) = opt_u64(value, "artifactIndex") {
        item["artifactIndex"] = json!(index);
    }
    Ok(item)
}

fn propose_download(args: &Value, proposals: &Arc<InMemoryProposalStore>) -> ToolResult {
    let payload = match download_item(args) {
        Ok(p) => p,
        Err(e) => return ToolResult::Failed(e),
    };
    let pid = payload["publisherId"].as_str().unwrap_or_default();
    let summary = match payload.get("artifactIndex").and_then(|v| v.as_u64()) {
        Some(i) => format!("Download observation {pid} (artifact #{i})"),
        None => format!("Download observation {pid}"),
    };
    // Non-destructive: it ADDS to the library, so the auto-apply policy may run
    // it when the user has enabled autonomy.
    let p = proposals.enqueue("download_observation", &summary, false, payload);
    ToolResult::Proposed(p)
}

fn propose_download_bulk(args: &Value, proposals: &Arc<InMemoryProposalStore>) -> ToolResult {
    let Some(raw) = crate::mcp::tools::arg(args, "items").and_then(|v| v.as_array()) else {
        return ToolResult::Failed("items is required (an array of observations)".to_string());
    };
    if raw.is_empty() {
        return ToolResult::Failed("items is empty".to_string());
    }
    if raw.len() > MAX_BULK_DOWNLOADS {
        return ToolResult::Failed(format!(
            "max {MAX_BULK_DOWNLOADS} items per bulk download, got {}",
            raw.len()
        ));
    }
    // Validate EVERY item before queuing: a batch that fails half way through
    // leaves the user with a partial download they never asked to approve.
    let mut items = Vec::with_capacity(raw.len());
    for (i, value) in raw.iter().enumerate() {
        match download_item(value) {
            Ok(item) => items.push(item),
            Err(e) => return ToolResult::Failed(format!("item {i}: {e}")),
        }
    }
    let summary = format!(
        "Download {} observation{}",
        items.len(),
        if items.len() == 1 { "" } else { "s" }
    );
    let p = proposals.enqueue(
        "download_observations_bulk",
        &summary,
        false,
        json!({ "items": items }),
    );
    ToolResult::Proposed(p)
}

async fn apply_download(
    services: &AppServices,
    proposal: &PendingProposal,
) -> Result<String, String> {
    let payload = &proposal.payload;
    let pid = str_arg(payload, "publisherId");
    let index = opt_u64(payload, "artifactIndex").map(|n| n as usize);
    let attribution = AgentAttribution::for_applied_proposal(proposal);
    crate::services::observation_download::download_and_register(services, &pid, index, attribution)
        .await
}

async fn apply_download_bulk(
    services: &AppServices,
    proposal: &PendingProposal,
) -> Result<String, String> {
    let attribution = AgentAttribution::for_applied_proposal(proposal);
    let items = proposal
        .payload
        .get("items")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let mut done = 0usize;
    for (i, item) in items.iter().enumerate() {
        let pid = str_arg(item, "publisherId");
        let index = opt_u64(item, "artifactIndex").map(|n| n as usize);
        match crate::services::observation_download::download_and_register(
            services,
            &pid,
            index,
            attribution.clone(),
        )
        .await
        {
            Ok(_) => done += 1,
            // Stop rather than press on: the remaining transfers would very
            // likely fail the same way (expired session, network down), and the
            // user gets a truthful count of what actually landed.
            Err(e) => {
                return Err(format!(
                    "downloaded {done} of {}; item {i} ({pid}) failed: {e}",
                    items.len()
                ))
            }
        }
    }
    Ok(format!("Downloaded {done} observation(s)"))
}

fn propose_delete(args: &Value, proposals: &Arc<InMemoryProposalStore>) -> ToolResult {
    let id = str_arg(args, "id");
    if id.is_empty() {
        return ToolResult::Failed("id is required".to_string());
    }
    let payload = json!({ "id": id });
    let p = proposals.enqueue(
        "delete_downloaded_observation",
        &format!("Delete downloaded observation {}", id),
        true,
        payload,
    );
    ToolResult::Proposed(p)
}

fn propose_clear(proposals: &Arc<InMemoryProposalStore>) -> ToolResult {
    let p = proposals.enqueue(
        "clear_research_archive",
        "Clear ALL research archive records",
        true,
        json!({}),
    );
    ToolResult::Proposed(p)
}

/// Propose exporting the combined research + search bundle to `path`. The write
/// is non-destructive (it only creates a new archive), but it still routes
/// through the proposal gate so the user confirms *where* it lands.
fn propose_export(args: &Value, proposals: &Arc<InMemoryProposalStore>) -> ToolResult {
    let path = str_arg(args, "path");
    if path.is_empty() {
        return ToolResult::Failed("path is required".to_string());
    }
    let include_notes = args
        .get("include_notes")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let include_history = args
        .get("include_search_history")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let payload = json!({
        "path": path,
        "includeNotes": include_notes,
        "includeSearchHistory": include_history,
    });
    let p = proposals.enqueue(
        "export_research_bundle",
        &format!("Export research bundle to {}", path),
        false,
        payload,
    );
    ToolResult::Proposed(p)
}

// ─────────────────────────────────────────────────────────────────────────────
// Apply — decode an approved proposal's payload + perform the real store mutation
// ─────────────────────────────────────────────────────────────────────────────

/// Execute an approved research-family proposal. Returns `Some(..)` if
/// `proposal.kind` belongs to this family (with `Ok`/`Err` for the outcome), or
/// `None` so the router can try another family's applier.
pub async fn apply(
    services: &AppServices,
    proposal: &PendingProposal,
) -> Option<Result<String, String>> {
    match proposal.kind.as_str() {
        "update_observation_note" => Some(apply_update_note(services, proposal).await),
        "bulk_update_observation_notes" => Some(apply_bulk_update_notes(services, proposal).await),
        "download_observation" => Some(apply_download(services, proposal).await),
        "download_observations_bulk" => Some(apply_download_bulk(services, proposal).await),
        "delete_downloaded_observation" => Some(apply_delete(services, &proposal.payload).await),
        "clear_research_archive" => Some(apply_clear(services).await),
        "export_research_bundle" => Some(apply_export(services, &proposal.payload).await),
        _ => None,
    }
}

/// Resolve a caller-supplied id (local OR publisher) to a publisher id; falls
/// back to treating the id as a publisher id when no downloaded record matches
/// (an agent may annotate an observation it hasn't downloaded).
async fn resolve_publisher_id(services: &AppServices, id: &str) -> String {
    let list = services.observation_store.load_async().await;
    find_observation(&list, id)
        .map(|o| o.publisher_id.clone())
        .unwrap_or_else(|| id.to_string())
}

/// Upsert one note into the standalone [`ObservationNoteStore`], merging only the
/// fields present in `spec` over any existing note, stamping agent provenance.
fn upsert_note_blocking(pub_id: String, spec: &Value, attribution: Option<AgentAttribution>) {
    let store = ObservationNoteStore::new();
    let mut note = store.get(&pub_id).unwrap_or_default();
    note.publisher_id = pub_id;
    if let Some(r) = spec.get("rating").and_then(Value::as_u64) {
        note.rating = r.min(5) as u8;
    }
    if let Some(t) = spec.get("note").and_then(Value::as_str) {
        note.note = t.to_string();
    }
    if let Some(tags) = spec.get("tags").and_then(Value::as_array) {
        note.tags = tags
            .iter()
            .filter_map(Value::as_str)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
    }
    note.updated = Utc::now().to_rfc3339();
    note.agent_attribution = attribution;
    // Best-effort: a save failure is surfaced by the caller's Result.
    let _ = store.save(note);
}

async fn apply_update_note(
    services: &AppServices,
    proposal: &PendingProposal,
) -> Result<String, String> {
    let id = str_arg(&proposal.payload, "id");
    if id.is_empty() {
        return Err("update_observation_note payload missing id".to_string());
    }
    let pub_id = resolve_publisher_id(services, &id).await;
    let attribution = AgentAttribution::for_applied_proposal(proposal);
    let spec = proposal.payload.clone();
    let pub_for_msg = pub_id.clone();
    tokio::task::spawn_blocking(move || upsert_note_blocking(pub_id, &spec, attribution))
        .await
        .map_err(|e| format!("note task failed: {e}"))?;
    Ok(format!("Set research note for {pub_for_msg}"))
}

async fn apply_bulk_update_notes(
    services: &AppServices,
    proposal: &PendingProposal,
) -> Result<String, String> {
    let items = proposal
        .payload
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if items.is_empty() {
        return Err("bulk_update_observation_notes payload missing items".to_string());
    }
    // Resolve ids up front (needs the async store), then do all the note writes
    // together on the blocking pool.
    let mut resolved: Vec<(String, Value)> = Vec::with_capacity(items.len());
    for item in items {
        let id = str_arg(&item, "id");
        if id.is_empty() {
            continue;
        }
        let pub_id = resolve_publisher_id(services, &id).await;
        resolved.push((pub_id, item));
    }
    let count = resolved.len();
    let attribution = AgentAttribution::for_applied_proposal(proposal);
    tokio::task::spawn_blocking(move || {
        for (pub_id, spec) in resolved {
            upsert_note_blocking(pub_id, &spec, attribution.clone());
        }
    })
    .await
    .map_err(|e| format!("bulk note task failed: {e}"))?;
    Ok(format!(
        "Set research notes for {count} observation{}",
        if count == 1 { "" } else { "s" }
    ))
}

async fn apply_delete(services: &AppServices, payload: &Value) -> Result<String, String> {
    let id = str_arg(payload, "id");
    if id.is_empty() {
        return Err("delete_downloaded_observation payload missing id".to_string());
    }
    let list = services.observation_store.load_async().await;
    let record = find_observation(&list, &id)
        .ok_or_else(|| format!("no downloaded observation with id '{}'", id))?;
    // The managed subdirectory is keyed by the record's local id.
    let record_id = record.id.clone();
    let label = observation_label(record);

    // Remove the managed directory first (best-effort, off the async executor),
    // then the store record.
    delete_managed_dir_blocking(&record_id).await;
    services.observation_store.remove_async(&record_id).await?;
    Ok(format!("Removed observation {} from Research", label))
}

async fn apply_clear(services: &AppServices) -> Result<String, String> {
    let list = services.observation_store.load_async().await;
    let count = list.len();
    for obs in &list {
        // Best-effort per-item file cleanup: a missing/locked managed dir must
        // never abort the clear.
        delete_managed_dir_blocking(&obs.id).await;
        // Best-effort record removal; keep clearing even if one write fails.
        let _ = services.observation_store.remove_async(&obs.id).await;
    }
    Ok(format!(
        "Cleared {} observation{} from Research",
        count,
        if count == 1 { "" } else { "s" }
    ))
}

/// Assemble the combined research + search bundle and write it as one store-only
/// `.zip`. Observations come from `observation_store`; saved/recent searches from
/// `search_store`; notes from the standalone `ObservationNoteStore` (not wired
/// into `AppServices`, so it is opened directly — same as the Research page's
/// own exporter). All rendering + the zip write happen on the blocking pool.
async fn apply_export(services: &AppServices, payload: &Value) -> Result<String, String> {
    let path_str = str_arg(payload, "path");
    if path_str.is_empty() {
        return Err("export_research_bundle payload missing path".to_string());
    }
    let include_notes = payload
        .get("include_notes")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let include_history = payload
        .get("include_search_history")
        .and_then(Value::as_bool)
        .unwrap_or(true);

    let observations = services.observation_store.load_async().await;
    let saved = services.search_store.load_saved();
    let recent = services.search_store.load_recent();

    let obs_count = observations.len();
    let saved_count = saved.len();
    let recent_count = if include_history { recent.len() } else { 0 };

    let path = std::path::PathBuf::from(&path_str);
    let now = Utc::now();

    // Notes are a tiny JSON file; the render + zip write are blocking. Do the
    // whole build off the async executor.
    let write_result = tokio::task::spawn_blocking(move || {
        let notes = if include_notes {
            crate::services::observation_note_store::ObservationNoteStore::new().all()
        } else {
            Vec::new()
        };
        let rb = crate::helpers::research_exporter::build_bundle(&observations, &notes, now);
        let search_files = crate::helpers::search_exporter::build_search_bundle(
            &saved,
            &recent,
            include_history,
            now,
        );

        let mut entries: Vec<(&str, &[u8])> = vec![
            ("observations.json", rb.observations_json.as_bytes()),
            ("notes.json", rb.notes_json.as_bytes()),
            ("notes.md", rb.notes_md.as_bytes()),
        ];
        for (name, content) in &search_files {
            entries.push((name.as_str(), content.as_bytes()));
        }
        crate::helpers::research_exporter::write_store_zip(&path, &entries)
    })
    .await
    .map_err(|e| format!("export task failed: {e}"))?;

    write_result?;

    Ok(format!(
        "Exported research bundle to {} ({} observation{}, {} saved quer{}, {} recent search{})",
        path_str,
        obs_count,
        if obs_count == 1 { "" } else { "s" },
        saved_count,
        if saved_count == 1 { "y" } else { "ies" },
        recent_count,
        if recent_count == 1 { "" } else { "es" },
    ))
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Find an observation by local id first, falling back to publisher id.
fn find_observation<'a>(
    list: &'a [DownloadedObservation],
    id: &str,
) -> Option<&'a DownloadedObservation> {
    list.iter()
        .find(|o| o.id == id)
        .or_else(|| list.iter().find(|o| o.publisher_id == id))
}

/// Delete a managed subdirectory on the blocking pool (the removal is sync fs I/O).
async fn delete_managed_dir_blocking(record_id: &str) {
    let id = record_id.to_string();
    let _ = tokio::task::spawn_blocking(move || {
        crate::services::observation_store::delete_managed_dir(&id);
    })
    .await;
}

/// A short human label for messages: prefer target name, else observation id, else
/// the record id.
fn observation_label(obs: &DownloadedObservation) -> String {
    if !obs.target_name.is_empty() {
        obs.target_name.clone()
    } else if !obs.observation_id.is_empty() {
        obs.observation_id.clone()
    } else {
        obs.id.clone()
    }
}

/// Compact JSON view of a downloaded observation (mirrors the C# `ObservationSummary`).
///
/// The single source of truth for how an observation appears on the wire —
/// `get_downloaded_observation` and `list_downloaded_observations` both render
/// through it, so the two can never drift apart again.
///
/// "Compact" is a deliberate constraint inherited from the reference: every
/// field is read straight off the record, and none of them touches the
/// filesystem. A `hasFits`-style existence check would cost one stat per row,
/// which on a /arc mount is one network round trip per observation.
pub(super) fn observation_summary(obs: &DownloadedObservation) -> Value {
    let filename = if obs.local_path.is_empty() {
        String::new()
    } else {
        std::path::Path::new(&obs.local_path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    json!({
        "id": obs.id,
        "publisherId": obs.publisher_id,
        "collection": obs.collection,
        "observationId": obs.observation_id,
        "targetName": obs.target_name,
        "instrument": obs.instrument,
        "filter": obs.filter,
        "ra": obs.ra,
        "dec": obs.dec,
        "startDate": obs.start_date,
        "calLevel": obs.cal_level,
        "filename": filename,
        "fileSizeBytes": obs.file_size,
        "downloadedAt": obs.downloaded_at,
        "bookmarkedOnly": obs.is_bookmarked()
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn store() -> Arc<InMemoryProposalStore> {
        Arc::new(InMemoryProposalStore::new())
    }

    #[test]
    fn download_requires_a_publisher_id() {
        match propose_download(&json!({}), &store()) {
            ToolResult::Failed(m) => assert!(m.contains("publisherId is required"), "{m}"),
            _ => panic!("expected a failure"),
        }
    }

    #[test]
    fn download_is_non_destructive_and_carries_the_artifact_index() {
        // Non-destructive: it ADDS to the library, so autonomy may apply it.
        match propose_download(
            &json!({ "publisherId": "ivo://x?1", "artifactIndex": 2 }),
            &store(),
        ) {
            ToolResult::Proposed(p) => {
                assert_eq!(p.kind, "download_observation");
                assert!(!p.destructive);
                assert_eq!(p.payload["publisherId"], "ivo://x?1");
                assert_eq!(p.payload["artifactIndex"], 2);
                assert!(p.summary.contains("artifact #2"), "{}", p.summary);
            }
            _ => panic!("expected a queued proposal"),
        }
    }

    #[test]
    fn download_omits_the_artifact_index_when_none_was_given() {
        // An absent index must stay absent, not become 0 — index 0 addresses a
        // real artifact, so defaulting would silently pick one.
        match propose_download(&json!({ "publisherId": "ivo://x?1" }), &store()) {
            ToolResult::Proposed(p) => {
                assert!(p.payload.get("artifactIndex").is_none());
            }
            _ => panic!("expected a queued proposal"),
        }
    }

    #[test]
    fn bulk_download_rejects_an_empty_or_oversized_batch() {
        match propose_download_bulk(&json!({ "items": [] }), &store()) {
            ToolResult::Failed(m) => assert!(m.contains("empty"), "{m}"),
            _ => panic!("expected a failure for an empty batch"),
        }
        let too_many: Vec<Value> = (0..MAX_BULK_DOWNLOADS + 1)
            .map(|i| json!({ "publisherId": format!("ivo://x?{i}") }))
            .collect();
        match propose_download_bulk(&json!({ "items": too_many }), &store()) {
            ToolResult::Failed(m) => assert!(m.contains("max 50"), "{m}"),
            _ => panic!("expected a failure for an oversized batch"),
        }
    }

    #[test]
    fn bulk_download_validates_every_item_before_queuing() {
        // A batch that fails half way through leaves the user with a partial
        // download they never approved, so a bad item must be caught up front —
        // and named, so the caller can fix it.
        let items = json!({ "items": [
            { "publisherId": "ivo://x?1" },
            { "publisherId": "  " },
        ]});
        match propose_download_bulk(&items, &store()) {
            ToolResult::Failed(m) => {
                assert!(
                    m.contains("item 1"),
                    "the failing item should be named: {m}"
                );
                assert!(m.contains("publisherId is required"), "{m}");
            }
            _ => panic!("expected a failure"),
        }
    }

    #[test]
    fn bulk_download_queues_one_proposal_for_the_whole_batch() {
        // One envelope = one user click, which is the point of the bulk tool.
        let st = store();
        let items = json!({ "items": [
            { "publisherId": "ivo://x?1" },
            { "publisherId": "ivo://x?2", "artifactIndex": 1 },
        ]});
        match propose_download_bulk(&items, &st) {
            ToolResult::Proposed(p) => {
                assert_eq!(p.kind, "download_observations_bulk");
                assert!(!p.destructive);
                assert_eq!(p.payload["items"].as_array().unwrap().len(), 2);
                assert!(p.summary.contains('2'), "{}", p.summary);
            }
            _ => panic!("expected a queued proposal"),
        }
        assert_eq!(st.pending_count(), 1, "the batch must be ONE proposal");
    }

    #[test]
    fn descriptor_names_unique_and_non_empty() {
        let ds = descriptors();
        assert!(!ds.is_empty(), "descriptors() must not be empty");
        let mut seen = HashSet::new();
        for d in &ds {
            assert!(!d.name.is_empty(), "descriptor name must be non-empty");
            assert!(!d.description.is_empty(), "{} needs a description", d.name);
            assert!(
                seen.insert(d.name.clone()),
                "duplicate descriptor name: {}",
                d.name
            );
        }
    }

    #[test]
    fn reads_are_read_writes_are_write_all_agent_safe() {
        for d in descriptors() {
            assert!(d.agent_safe, "{} must be agent_safe", d.name);
            let expected = if d.name.starts_with("get_") {
                VerbClass::Read
            } else {
                VerbClass::Write
            };
            assert_eq!(d.verb, expected, "{} has the wrong verb class", d.name);
        }
    }

    #[test]
    fn delete_and_clear_are_destructive_proposals() {
        let store = Arc::new(InMemoryProposalStore::new());
        match propose_delete(&json!({ "id": "abc" }), &store) {
            ToolResult::Proposed(p) => {
                assert_eq!(p.kind, "delete_downloaded_observation");
                assert!(p.destructive);
            }
            _ => panic!("expected Proposed"),
        }
        match propose_clear(&store) {
            ToolResult::Proposed(p) => {
                assert_eq!(p.kind, "clear_research_archive");
                assert!(p.destructive);
            }
            _ => panic!("expected Proposed"),
        }
        assert_eq!(store.pending_count(), 2);
    }

    #[test]
    fn export_is_non_destructive_proposal_and_requires_path() {
        let store = Arc::new(InMemoryProposalStore::new());
        // Missing path is rejected without enqueuing.
        assert!(matches!(
            propose_export(&json!({}), &store),
            ToolResult::Failed(_)
        ));
        assert_eq!(store.pending_count(), 0);

        match propose_export(&json!({ "path": "/tmp/bundle.zip" }), &store) {
            ToolResult::Proposed(p) => {
                assert_eq!(p.kind, "export_research_bundle");
                assert!(!p.destructive, "export must be non-destructive");
                assert_eq!(p.payload["path"], "/tmp/bundle.zip");
                // Defaults applied.
                assert_eq!(p.payload["includeNotes"], true);
                assert_eq!(p.payload["includeSearchHistory"], true);
            }
            _ => panic!("expected Proposed"),
        }
    }

    #[test]
    fn export_descriptor_is_write_and_agent_safe() {
        let d = descriptors()
            .into_iter()
            .find(|d| d.name == "export_research_bundle")
            .expect("export_research_bundle descriptor present");
        assert_eq!(d.verb, VerbClass::Write);
        assert!(d.agent_safe);
    }

    #[test]
    fn delete_requires_id() {
        let store = Arc::new(InMemoryProposalStore::new());
        assert!(matches!(
            propose_delete(&json!({}), &store),
            ToolResult::Failed(_)
        ));
        assert_eq!(store.pending_count(), 0);
    }

    #[test]
    fn update_note_requires_id() {
        let store = Arc::new(InMemoryProposalStore::new());
        assert!(matches!(
            propose_update_note(&json!({}), &store),
            ToolResult::Failed(_)
        ));
        assert_eq!(store.pending_count(), 0);
    }

    #[test]
    fn update_note_enqueues_non_destructive_with_only_given_fields() {
        let store = Arc::new(InMemoryProposalStore::new());
        // `note` is omitted, so it must NOT appear in the payload (apply merges).
        match propose_update_note(
            &json!({ "id": "ivo://x?1", "rating": 4, "tags": ["a"] }),
            &store,
        ) {
            ToolResult::Proposed(p) => {
                assert_eq!(p.kind, "update_observation_note");
                assert!(!p.destructive, "a note edit is reversible");
                assert_eq!(p.payload["id"], "ivo://x?1");
                assert_eq!(p.payload["rating"], 4);
                assert_eq!(p.payload["tags"], json!(["a"]));
                assert!(p.payload.get("note").is_none(), "unset fields stay absent");
            }
            _ => panic!("expected Proposed"),
        }
    }

    #[test]
    fn bulk_update_requires_non_empty_items() {
        let store = Arc::new(InMemoryProposalStore::new());
        assert!(matches!(
            propose_bulk_update_notes(&json!({}), &store),
            ToolResult::Failed(_)
        ));
        assert!(matches!(
            propose_bulk_update_notes(&json!({ "items": [] }), &store),
            ToolResult::Failed(_)
        ));
        assert_eq!(store.pending_count(), 0);
    }

    #[test]
    fn bulk_update_enqueues_items() {
        let store = Arc::new(InMemoryProposalStore::new());
        match propose_bulk_update_notes(
            &json!({ "items": [ { "id": "a", "rating": 5 }, { "id": "b", "note": "hi" } ] }),
            &store,
        ) {
            ToolResult::Proposed(p) => {
                assert_eq!(p.kind, "bulk_update_observation_notes");
                assert!(!p.destructive);
                assert_eq!(p.payload["items"].as_array().unwrap().len(), 2);
            }
            _ => panic!("expected Proposed"),
        }
    }

    #[test]
    fn find_matches_id_then_publisher() {
        let obs = DownloadedObservation {
            id: "local-1".into(),
            publisher_id: "ivo://cadc/CFHT?9".into(),
            collection: "CFHT".into(),
            observation_id: "obs-9".into(),
            target_name: "M31".into(),
            instrument: "MegaCam".into(),
            filter: "g".into(),
            ra: "10.6".into(),
            dec: "41.2".into(),
            start_date: "2020-01-01".into(),
            cal_level: "2".into(),
            local_path: "/data/x.fits".into(),
            file_size: 2048,
            downloaded_at: "2024-01-01T00:00:00Z".into(),
            thumbnail_url: String::new(),
            preview_url: String::new(),
            local_preview_path: String::new(),
            agent_attribution: None,
        };
        let list = vec![obs];
        assert!(find_observation(&list, "local-1").is_some());
        assert!(find_observation(&list, "ivo://cadc/CFHT?9").is_some());
        assert!(find_observation(&list, "nope").is_none());

        let summary = observation_summary(&list[0]);
        assert_eq!(summary["filename"], "x.fits");
        assert_eq!(summary["fileSizeBytes"], 2048);
        assert_eq!(summary["targetName"], "M31");
    }

    /// Build a fully populated record for wire-shape assertions.
    fn sample_observation() -> DownloadedObservation {
        DownloadedObservation {
            id: "local-1".into(),
            publisher_id: "ivo://cadc/CFHT?9".into(),
            collection: "CFHT".into(),
            observation_id: "obs-9".into(),
            target_name: "M31".into(),
            instrument: "MegaCam".into(),
            filter: "g".into(),
            ra: "10.6".into(),
            dec: "41.2".into(),
            start_date: "2020-01-01".into(),
            cal_level: "2".into(),
            local_path: "/data/x.fits".into(),
            file_size: 2048,
            downloaded_at: "2024-01-01T00:00:00Z".into(),
            thumbnail_url: String::new(),
            preview_url: String::new(),
            local_preview_path: String::new(),
            agent_attribution: None,
        }
    }

    /// Every field of the reference's `ObservationSummary` record, camelCased by
    /// its serializer (`JsonNamingPolicy.CamelCase`). Transcribed from
    /// `Mcp/Tools/Read/ResearchReadTools.cs`.
    const REFERENCE_SUMMARY_FIELDS: &[&str] = &[
        "id",
        "publisherId",
        "collection",
        "observationId",
        "targetName",
        "instrument",
        "filter",
        "ra",
        "dec",
        "startDate",
        "calLevel",
        "filename",
        "fileSizeBytes",
        "downloadedAt",
    ];

    #[test]
    fn the_summary_carries_every_field_the_reference_promises() {
        let summary = observation_summary(&sample_observation());
        let obj = summary.as_object().expect("an object");
        for field in REFERENCE_SUMMARY_FIELDS {
            assert!(
                obj.contains_key(*field),
                "the reference's ObservationSummary has `{field}`; an agent written \
                 against the Windows app will look for it"
            );
        }
    }

    #[test]
    fn the_summary_adds_nothing_beyond_one_documented_extra() {
        // Divergence is only allowed deliberately. `bookmarkedOnly` earns its
        // place: it disambiguates an empty `filename` (never downloaded) from a
        // record whose path lost its basename. Anything ELSE appearing here is
        // drift — most likely a field that quietly probes the filesystem, which
        // is exactly what the compact view exists to avoid.
        let summary = observation_summary(&sample_observation());
        let allowed: HashSet<&str> = REFERENCE_SUMMARY_FIELDS
            .iter()
            .copied()
            .chain(["bookmarkedOnly"])
            .collect();

        for key in summary.as_object().expect("an object").keys() {
            assert!(
                allowed.contains(key.as_str()),
                "`{key}` is not in the reference's ObservationSummary — add it to \
                 the documented extras only if it is free of filesystem access"
            );
        }
    }

    #[test]
    fn a_bookmark_reports_no_file_rather_than_a_stray_path() {
        // A metadata-only record has no file: the reference derives `filename`
        // from an empty LocalPath as "", and the size stays 0.
        let mut obs = sample_observation();
        obs.local_path = String::new();
        obs.file_size = 0;

        let summary = observation_summary(&obs);
        assert_eq!(summary["filename"], "");
        assert_eq!(summary["fileSizeBytes"], 0);
        assert_eq!(summary["bookmarkedOnly"], true);
    }
}
