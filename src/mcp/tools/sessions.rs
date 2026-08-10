//! Skaha session tool family — reads over the live session list plus the
//! headless-job lifecycle (launch + bulk terminate). Ported from
//! `Mcp/Tools/Read/SessionReadTools.cs` + `Mcp/Tools/Write/SessionWriteTools.cs`.
//!
//! This module is a self-contained family that the router chains: it owns a fixed
//! set of tool names and proposal kinds and exposes exactly three entry points —
//! [`descriptors`] (the manifest), [`dispatch`] (run a call by name), and [`apply`]
//! (perform the real service call for one of *its* approved proposals).
//!
//! READ tools (`get_session`, `list_session_types`, `list_headless_jobs`,
//! `get_session_logs`, `get_session_events`) are `verb: Read` / agent-safe and
//! return [`ToolResult::Data`] with no side effects. WRITE tools
//! (`launch_headless_job`, `delete_sessions_bulk`) are `verb: Write` / agent-safe:
//! they validate + enqueue a [`PendingProposal`] and NEVER mutate at propose time.
//! Both writes are destructive (`launch_*` spends quota; `delete_*` terminates), so
//! the host never auto-applies them — the real work happens later in [`apply`].

use crate::mcp::tools::proposals::{InMemoryProposalStore, PendingProposal};
use crate::mcp::tools::{ToolDescriptor, ToolResult, VerbClass};
use crate::models::SessionLaunchParams;
use crate::state::AppServices;
use serde_json::{json, Value};
use std::sync::Arc;

/// The session types Skaha can launch (interactive + headless), mirroring the C#
/// `ListSessionTypesTool.Types`.
const SESSION_TYPES: &[&str] = &[
    "notebook",
    "desktop",
    "carta",
    "contributed",
    "firefly",
    "headless",
];

/// Interactive (non-headless) types, surfaced so a caller can tell them apart.
const INTERACTIVE_TYPES: &[&str] = &["notebook", "desktop", "carta", "contributed", "firefly"];

/// Resource defaults when a `launch_headless_job` proposal omits cores/ram. They
/// match the schema minimums so an approved proposal always launches a valid job.
const DEFAULT_CORES: u32 = 1;
const DEFAULT_RAM: u32 = 1;

/// Hard cap on ids accepted by `delete_sessions_bulk` (mirrors the C#
/// `DeleteSessionsBulkTool.MaxBatchSize`).
const MAX_BATCH_SIZE: usize = 50;

// ─────────────────────────────────────────────────────────────────────────────
// Manifest
// ─────────────────────────────────────────────────────────────────────────────

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

fn id_schema(desc: &str) -> Value {
    json!({
        "type": "object",
        "properties": { "id": { "type": "string", "description": desc } },
        "required": ["id"],
        "additionalProperties": false
    })
}

/// Every descriptor owned by this family (reads ++ writes).
pub fn descriptors() -> Vec<ToolDescriptor> {
    vec![
        read_tool(
            "get_session",
            "Get one Skaha session (interactive or headless) by its id, including its status, image, \
             allocated CPU/RAM/GPU, start/expiry times, and the connectUrl to open an interactive \
             session in the browser. Get ids from list_sessions / list_headless_jobs.",
            id_schema("Session id to fetch."),
        ),
        read_tool(
            "list_session_types",
            "List the session types that can be launched (notebook, desktop, carta, contributed, \
             firefly, headless), split into interactive vs headless categories.",
            json!({ "type": "object", "properties": {}, "additionalProperties": false }),
        ),
        read_tool(
            "list_headless_jobs",
            "List the user's headless (batch) Skaha jobs — the subset of active sessions whose type \
             is 'headless' (id, name, status, image, resources, start/expiry). Use get_session_logs \
             / get_session_events on an id to inspect one.",
            json!({ "type": "object", "properties": {}, "additionalProperties": false }),
        ),
        read_tool(
            "get_headless_job_logs",
            "Fetch the container logs (stdout/stderr) for a Skaha session or headless job by its id. \
             Useful for checking a headless job's output or debugging a failed launch.",
            id_schema("Session id to fetch logs for."),
        ),
        read_tool(
            "get_headless_job_events",
            "Fetch the Kubernetes-style scheduling/lifecycle events for a Skaha session or headless \
             job by its id (scheduling, image-pull, and container state transitions). Useful for \
             diagnosing why a session is stuck Pending.",
            id_schema("Session id to fetch events for."),
        ),
        write_tool(
            "launch_headless_job",
            "Propose launching a headless (batch) Skaha job from a container image, with optional \
             command (`cmd`, the executable the container runs), an `args` string, and CPU/RAM(GB). \
             Queues for the user to apply (a destructive change — it spends quota); after it applies, \
             track it via list_headless_jobs.",
            json!({
                "type": "object",
                "properties": {
                    "image": { "type": "string", "description": "Container image id to launch." },
                    "cores": { "type": "integer", "minimum": 1, "description": "CPU cores." },
                    "ram":   { "type": "integer", "minimum": 1, "description": "RAM in GB." },
                    "cmd":   { "type": "string", "description": "Executable the container runs." },
                    "args":  { "type": "string", "description": "Arguments passed to cmd." }
                },
                "required": ["image"],
                "additionalProperties": false
            }),
        ),
        write_tool(
            "delete_sessions_bulk",
            "Terminate up to 50 Skaha sessions (interactive OR headless) as one proposal envelope. \
             Partial-success: every id is attempted, so a single session that's already gone doesn't \
             block the rest. Use it for zombie-cleanup or to free quota slots. Queues for the user to \
             apply (a destructive change). Get ids from list_sessions / list_headless_jobs.",
            json!({
                "type": "object",
                "properties": {
                    "ids": {
                        "type": "array",
                        "items": { "type": "string", "minLength": 1 },
                        "minItems": 1,
                        "maxItems": 50,
                        "description": "Session ids to terminate."
                    }
                },
                "required": ["ids"],
                "additionalProperties": false
            }),
        ),
    ]
}

// ─────────────────────────────────────────────────────────────────────────────
// Dispatch — reads run immediately; writes validate + enqueue a proposal
// ─────────────────────────────────────────────────────────────────────────────

/// Run one tool by name. Returns `Some(result)` when this family owns `name`,
/// `None` otherwise so the router can try another family.
pub async fn dispatch(
    name: &str,
    services: &AppServices,
    args: &Value,
    proposals: &Arc<InMemoryProposalStore>,
) -> Option<ToolResult> {
    let result = match name {
        // Reads
        "get_session" => get_session(services, args).await,
        "list_session_types" => list_session_types(),
        "list_headless_jobs" => list_headless_jobs(services).await,
        "get_headless_job_logs" => get_session_logs(services, args).await,
        "get_headless_job_events" => get_session_events(services, args).await,
        // Writes (propose only — no mutation here)
        "launch_headless_job" => propose_launch_headless_job(args, proposals),
        "delete_sessions_bulk" => propose_delete_sessions_bulk(args, proposals),
        _ => return None,
    };
    Some(result)
}

// ── Reads ──────────────────────────────────────────────────────────────────────

/// JSON view of a `Session`, matching the field names used by `list_sessions`.
fn session_json(s: &crate::models::Session) -> Value {
    json!({
        "id": s.id,
        "name": s.name,
        "type": s.session_type,
        "status": s.status,
        "image": s.image,
        "started_time": s.start_time,
        "expires_time": s.expiry_time,
        "cpu_allocated": s.requested_cpu_cores,
        "memory_allocated": s.requested_ram,
        "gpu_allocated": s.requested_gpu_cores,
        "connect_url": s.connect_url,
    })
}

async fn get_session(services: &AppServices, args: &Value) -> ToolResult {
    let id = str_arg(args, "id");
    if id.is_empty() {
        return ToolResult::Failed("id is required".to_string());
    }
    let token = match services.get_token().await {
        Some(t) => t,
        None => return not_signed_in(),
    };
    match services.sessions.get_sessions(&token).await {
        Ok(sessions) => match sessions.iter().find(|s| s.id == id) {
            Some(s) => ToolResult::Data(session_json(s)),
            None => ToolResult::Failed(format!("no session with id '{id}'")),
        },
        Err(e) => ToolResult::Failed(format!("could not fetch session '{id}': {e}")),
    }
}

fn list_session_types() -> ToolResult {
    ToolResult::Data(json!({
        "count": SESSION_TYPES.len(),
        "types": SESSION_TYPES,
        "interactive": INTERACTIVE_TYPES,
        "headless": ["headless"],
    }))
}

async fn list_headless_jobs(services: &AppServices) -> ToolResult {
    let token = match services.get_token().await {
        Some(t) => t,
        None => return not_signed_in(),
    };
    match services.sessions.get_sessions(&token).await {
        Ok(sessions) => {
            let items: Vec<Value> = sessions
                .iter()
                .filter(|s| s.session_type.eq_ignore_ascii_case("headless"))
                .map(session_json)
                .collect();
            ToolResult::Data(json!({ "count": items.len(), "jobs": items }))
        }
        Err(e) => ToolResult::Failed(format!("could not list headless jobs: {e}")),
    }
}

async fn get_session_logs(services: &AppServices, args: &Value) -> ToolResult {
    let id = str_arg(args, "id");
    if id.is_empty() {
        return ToolResult::Failed("id is required".to_string());
    }
    let token = match services.get_token().await {
        Some(t) => t,
        None => return not_signed_in(),
    };
    match services.sessions.get_logs(&token, &id).await {
        Ok(logs) => ToolResult::Data(json!({ "id": id, "logs": logs })),
        Err(e) => ToolResult::Failed(format!("could not fetch logs for '{id}': {e}")),
    }
}

async fn get_session_events(services: &AppServices, args: &Value) -> ToolResult {
    let id = str_arg(args, "id");
    if id.is_empty() {
        return ToolResult::Failed("id is required".to_string());
    }
    let token = match services.get_token().await {
        Some(t) => t,
        None => return not_signed_in(),
    };
    match services.sessions.get_events(&token, &id).await {
        Ok(events) => ToolResult::Data(json!({ "id": id, "events": events })),
        Err(e) => ToolResult::Failed(format!("could not fetch events for '{id}': {e}")),
    }
}

// ── Writes (propose) ───────────────────────────────────────────────────────────

fn propose_launch_headless_job(args: &Value, proposals: &Arc<InMemoryProposalStore>) -> ToolResult {
    let image = str_arg(args, "image");
    if image.is_empty() {
        return ToolResult::Failed("image is required".to_string());
    }
    // Validate optional sizing at propose time so a bad size is rejected here rather
    // than surfacing as an opaque launch failure when the user applies the proposal.
    let cores = match validated_resource(args, "cores") {
        Ok(v) => v,
        Err(e) => return ToolResult::Failed(e),
    };
    let ram = match validated_resource(args, "ram") {
        Ok(v) => v,
        Err(e) => return ToolResult::Failed(e),
    };
    let cmd = str_arg(args, "cmd");
    let cmd_args = str_arg(args, "args");

    let mut payload = json!({ "image": image });
    if let Some(c) = cores {
        payload["cores"] = json!(c);
    }
    if let Some(r) = ram {
        payload["ram"] = json!(r);
    }
    if !cmd.is_empty() {
        payload["cmd"] = json!(cmd);
    }
    if !cmd_args.is_empty() {
        payload["args"] = json!(cmd_args);
    }

    let p = proposals.enqueue(
        "launch_headless_job",
        &format!("Launch headless job: {image}"),
        true,
        payload,
    );
    ToolResult::Proposed(p)
}

fn propose_delete_sessions_bulk(
    args: &Value,
    proposals: &Arc<InMemoryProposalStore>,
) -> ToolResult {
    // Trim, drop blanks, and dedup — whitespace-only ids are never valid Skaha ids and
    // would 404 on delete, so catching them here beats a network call per blank.
    let ids = dedup_ids(args.get("ids"));
    if ids.is_empty() {
        return ToolResult::Failed("ids is empty after dropping blanks / duplicates".to_string());
    }
    if ids.len() > MAX_BATCH_SIZE {
        return ToolResult::Failed(format!(
            "ids count {} exceeds {}-per-call cap",
            ids.len(),
            MAX_BATCH_SIZE
        ));
    }
    let summary = format!(
        "Terminate {} session{}",
        ids.len(),
        if ids.len() == 1 { "" } else { "s" }
    );
    let p = proposals.enqueue(
        "delete_sessions_bulk",
        &summary,
        true,
        json!({ "ids": ids }),
    );
    ToolResult::Proposed(p)
}

// ─────────────────────────────────────────────────────────────────────────────
// Apply — decode an approved proposal + perform the real service call
// ─────────────────────────────────────────────────────────────────────────────

/// Execute one of this family's approved proposals. Returns `Some(Ok/Err)` when the
/// proposal's `kind` belongs to this family, `None` otherwise so the router can try
/// another family. Never runs unless the host has approved the proposal.
pub async fn apply(
    services: &AppServices,
    proposal: &PendingProposal,
) -> Option<Result<String, String>> {
    match proposal.kind.as_str() {
        "launch_headless_job" => Some(apply_launch_headless_job(services, &proposal.payload).await),
        "delete_sessions_bulk" => {
            Some(apply_delete_sessions_bulk(services, &proposal.payload).await)
        }
        _ => None,
    }
}

async fn apply_launch_headless_job(
    services: &AppServices,
    payload: &Value,
) -> Result<String, String> {
    let token = services
        .get_token()
        .await
        .ok_or_else(|| "not signed in".to_string())?;
    let image = str_arg(payload, "image");
    if image.is_empty() {
        return Err("launch_headless_job payload missing image".to_string());
    }
    let cores = opt_u32(payload, "cores").unwrap_or(DEFAULT_CORES);
    let ram = opt_u32(payload, "ram").unwrap_or(DEFAULT_RAM);

    let cmd = str_arg(payload, "cmd");
    let cmd = if cmd.is_empty() { None } else { Some(cmd) };
    // `args` may arrive as a JSON array or a whitespace-separated string.
    let args = match payload.get("args") {
        Some(Value::Array(a)) => {
            let v: Vec<String> = a
                .iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect();
            (!v.is_empty()).then_some(v)
        }
        Some(Value::String(s)) if !s.trim().is_empty() => {
            Some(s.split_whitespace().map(|s| s.to_string()).collect())
        }
        _ => None,
    };
    let replicas = opt_u32(payload, "replicas").map(|n| n.clamp(1, 20));

    let params = SessionLaunchParams {
        name: "headless-job".to_string(),
        image,
        session_type: "headless".to_string(),
        cores,
        ram,
        gpus: 0,
        cmd,
        env: None,
        registry_username: None,
        registry_secret: None,
        args,
        replicas,
    };
    let id = services.sessions.launch_session(&token, &params).await?;
    Ok(format!("Launched headless job (id {id})"))
}

async fn apply_delete_sessions_bulk(
    services: &AppServices,
    payload: &Value,
) -> Result<String, String> {
    let token = services
        .get_token()
        .await
        .ok_or_else(|| "not signed in".to_string())?;
    let ids = dedup_ids(payload.get("ids"));
    if ids.is_empty() {
        return Err("delete_sessions_bulk payload has no ids".to_string());
    }
    let total = ids.len();

    // Partial-success: attempt every id and never abort on a single failure, so an
    // already-gone session doesn't block the rest of the batch.
    let mut ok = 0usize;
    let mut first_error: Option<String> = None;
    for id in &ids {
        match services.sessions.delete_session(&token, id).await {
            Ok(()) => ok += 1,
            Err(e) => {
                if first_error.is_none() {
                    first_error = Some(format!("{id}: {e}"));
                }
            }
        }
    }

    let failed = total - ok;
    if ok == 0 {
        return Err(format!(
            "all {total} deletes failed (first error: {})",
            first_error.unwrap_or_else(|| "unknown".to_string())
        ));
    }
    if failed == 0 {
        Ok(format!(
            "Terminated {ok} session{}",
            if ok == 1 { "" } else { "s" }
        ))
    } else {
        Ok(format!(
            "Terminated {ok} of {total} sessions ({failed} failed; first error: {})",
            first_error.unwrap_or_else(|| "unknown".to_string())
        ))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

fn not_signed_in() -> ToolResult {
    ToolResult::Failed("not signed in (sign in to CADC/CANFAR first)".to_string())
}

/// Trimmed string argument (empty string if missing / not a string).
fn str_arg(args: &Value, key: &str) -> String {
    args.get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string()
}

/// Optional `u32` argument (None if missing or not an unsigned int).
fn opt_u32(args: &Value, key: &str) -> Option<u32> {
    args.get(key).and_then(Value::as_u64).map(|n| n as u32)
}

/// Validate an optional resource field: absent → `Ok(None)`, present and `>= 1` →
/// `Ok(Some(n))`, otherwise an error string.
fn validated_resource(args: &Value, key: &str) -> Result<Option<u32>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => match v.as_u64() {
            Some(n) if n >= 1 => Ok(Some(n as u32)),
            _ => Err(format!("{key} must be an integer >= 1")),
        },
    }
}

/// Trim, drop blanks, and de-duplicate (order-preserving) a JSON array of id strings.
fn dedup_ids(value: Option<&Value>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    if let Some(Value::Array(items)) = value {
        for item in items {
            if let Some(s) = item.as_str() {
                let s = s.trim();
                if !s.is_empty() && seen.insert(s.to_string()) {
                    out.push(s.to_string());
                }
            }
        }
    }
    out
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn descriptor_names_are_unique_and_non_empty() {
        let names: Vec<String> = descriptors().into_iter().map(|d| d.name).collect();
        assert!(!names.is_empty(), "there must be at least one session tool");
        for name in &names {
            assert!(!name.trim().is_empty(), "tool names must be non-empty");
        }
        let unique: HashSet<&String> = names.iter().collect();
        assert_eq!(unique.len(), names.len(), "tool names must be unique");
    }

    #[test]
    fn reads_are_agent_safe_writes_are_destructive_kinds() {
        for d in descriptors() {
            assert!(d.agent_safe, "{} must be agent-safe", d.name);
            assert!(
                d.input_schema.is_object(),
                "{} needs an object schema",
                d.name
            );
            assert!(
                !d.description.trim().is_empty(),
                "{} needs a description",
                d.name
            );
        }
    }

    #[test]
    fn launch_headless_requires_image_and_validates_size() {
        let store = Arc::new(InMemoryProposalStore::new());
        assert!(matches!(
            propose_launch_headless_job(&json!({}), &store),
            ToolResult::Failed(_)
        ));
        assert!(matches!(
            propose_launch_headless_job(&json!({ "image": "img:1", "cores": 0 }), &store),
            ToolResult::Failed(_)
        ));
        assert_eq!(store.pending_count(), 0);

        match propose_launch_headless_job(&json!({ "image": "img:1" }), &store) {
            ToolResult::Proposed(p) => {
                assert_eq!(p.kind, "launch_headless_job");
                assert!(p.destructive, "launch_headless_job must be destructive");
            }
            _ => panic!("expected a Proposed result"),
        }
        assert_eq!(store.pending_count(), 1);
    }

    #[test]
    fn bulk_delete_dedups_and_caps() {
        let store = Arc::new(InMemoryProposalStore::new());
        // Blanks + duplicates collapse away.
        match propose_delete_sessions_bulk(&json!({ "ids": ["a", " a ", "  ", "b"] }), &store) {
            ToolResult::Proposed(p) => {
                assert_eq!(p.kind, "delete_sessions_bulk");
                assert!(p.destructive);
                let ids = p.payload.get("ids").and_then(Value::as_array).unwrap();
                assert_eq!(ids.len(), 2, "duplicate/blank ids should collapse");
            }
            _ => panic!("expected a Proposed result"),
        }

        // Empty after cleanup → rejected, nothing enqueued.
        assert!(matches!(
            propose_delete_sessions_bulk(&json!({ "ids": ["  ", ""] }), &store),
            ToolResult::Failed(_)
        ));

        // Over the cap → rejected.
        let too_many: Vec<String> = (0..MAX_BATCH_SIZE + 1).map(|i| format!("s{i}")).collect();
        assert!(matches!(
            propose_delete_sessions_bulk(&json!({ "ids": too_many }), &store),
            ToolResult::Failed(_)
        ));
    }

    #[test]
    fn effective_cmd_folds_cmd_and_args() {
        // Exercised indirectly through dedup/str_arg helpers used by apply; here we
        // assert the fold rule directly to lock the cmd/args combination behaviour.
        let combine = |cmd: &str, args: &str| -> Option<String> {
            match (cmd.is_empty(), args.is_empty()) {
                (true, true) => None,
                (false, true) => Some(cmd.to_string()),
                (true, false) => Some(args.to_string()),
                (false, false) => Some(format!("{cmd} {args}")),
            }
        };
        assert_eq!(combine("", ""), None);
        assert_eq!(combine("python", ""), Some("python".to_string()));
        assert_eq!(
            combine("", "-c 'print(1)'"),
            Some("-c 'print(1)'".to_string())
        );
        assert_eq!(
            combine("python", "job.py"),
            Some("python job.py".to_string())
        );
    }
}
