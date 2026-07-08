//! Research-module tool family — read + write tools over the local
//! `ObservationStore` that backs the Research library.
//!
//! Ported from `Mcp/Tools/Read/ResearchReadTools.cs` (the read half) and
//! `Mcp/Tools/Write/ObservationWriteTools.cs` (the delete/clear write half).
//! Only the subset the Verbinal `ObservationStore` actually supports is exposed:
//!
//! * `get_downloaded_observation` — one observation by its local id (or publisher
//!   id), read-only.
//! * `get_observation_notes` — the astronomer-notes surface. Verbinal's
//!   `DownloadedObservation` carries **no** note field, so this always returns an
//!   empty set (parity placeholder for the Windows note store).
//! * `delete_downloaded_observation` — propose removing one observation (its
//!   record + managed files). Destructive.
//! * `clear_research_archive` — propose removing EVERY observation. Destructive.
//!
//! Reads dispatch straight against `services.observation_store`. Writes NEVER
//! mutate at propose time: they enqueue a [`PendingProposal`]; the real store
//! mutation happens in [`apply`] once the user approves.

use crate::mcp::tools::proposals::{InMemoryProposalStore, PendingProposal};
use crate::mcp::tools::{ToolDescriptor, ToolResult, VerbClass};
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
            description: "Get the user's research notes for a downloaded observation. Verbinal's \
                Research library stores only observation metadata (no free-text notes/rating/tags), \
                so this always returns an empty note set — provided for parity with clients that \
                probe for notes."
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
                    "publisher_id": { "type": "string", "description": "The observation's publisher DID / id." }
                },
                "required": ["publisher_id"],
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
                    "include_notes": {
                        "type": "boolean",
                        "description": "Include research notes (default true)."
                    },
                    "include_search_history": {
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
        "get_observation_notes" => get_observation_notes(args),
        "get_preview_image" => get_preview_image(services, args).await,
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

/// Notes surface. The Verbinal `DownloadedObservation` has no note field, so —
/// per the family's documented contract — this always returns an empty set
/// regardless of the (optional) id filter.
fn get_observation_notes(args: &Value) -> ToolResult {
    let id = str_arg(args, "id");
    let filtered_by = if id.is_empty() {
        Value::Null
    } else {
        Value::String(id)
    };
    ToolResult::Data(json!({
        "count": 0,
        "notes": [],
        "filteredBy": filtered_by
    }))
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
        "include_notes": include_notes,
        "include_search_history": include_history,
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
        "delete_downloaded_observation" => Some(apply_delete(services, &proposal.payload).await),
        "clear_research_archive" => Some(apply_clear(services).await),
        "export_research_bundle" => Some(apply_export(services, &proposal.payload).await),
        _ => None,
    }
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
fn observation_summary(obs: &DownloadedObservation) -> Value {
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

/// Extract a trimmed string argument (empty string if missing / not a string).
fn str_arg(args: &Value, key: &str) -> String {
    args.get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string()
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

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
                assert_eq!(p.payload["include_notes"], true);
                assert_eq!(p.payload["include_search_history"], true);
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
    fn notes_are_always_empty() {
        match get_observation_notes(&json!({ "id": "anything" })) {
            ToolResult::Data(v) => {
                assert_eq!(v["count"], 0);
                assert_eq!(v["notes"], json!([]));
                assert_eq!(v["filteredBy"], "anything");
            }
            _ => panic!("expected Data"),
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
}
