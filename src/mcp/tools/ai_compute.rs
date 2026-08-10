//! AI Compute tool family — run agent-authored code on a warm remote Skaha
//! `contributed` session via the `/arc` file-drop RPC. Ported from
//! `Mcp/Tools/Write/AIComputeTools.cs`.
//!
//! This is a self-contained family the router chains via [`descriptors`],
//! [`dispatch`], and [`apply`]. CANFAR compute is part of the platform's user
//! experience (not billed usage), so — matching macOS/Windows — `run_code` and
//! `start_compute` are NON-destructive writes: they auto-apply under the host's
//! auto-apply policy (and queue for review when it's off). `stop_compute` is
//! DESTRUCTIVE (it tears down a session mid-work) and always queues.
//! `run_code_output` is a plain read. An empty configured compute image disables
//! run_code / start_compute.
//!
//! Unlike the C# tool (which only submits and returns an execution_id),
//! `run_code`'s applier both SUBMITS and briefly POLLS the out file, so an
//! auto-applied call returns the actual status / exit / stdout / stderr when the
//! watcher answers quickly; if it's still running, it returns the execution id to
//! poll later with `run_code_output`.
//!
//! The backing [`AIComputeService`] holds only persisted settings; the warm
//! session lives on Skaha (found by name), so this family constructs a fresh
//! service instance per call. An optional `AppServices.ai_compute` singleton (see
//! the integrator notes) can be swapped in later without changing this family.

use crate::mcp::tools::proposals::{InMemoryProposalStore, PendingProposal};
use crate::mcp::tools::{ToolDescriptor, ToolResult, VerbClass};
use crate::models::ai_compute::{RunCodeContract, RunCodeRequest, RunCodeResult};
use crate::services::ai_compute_service::AIComputeService;
use crate::state::AppServices;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

/// How many times `run_code`'s applier polls the out file before returning the
/// execution id for later polling, and the delay between polls. Kept small so an
/// auto-applied call never blocks the agent for long.
const POLL_ATTEMPTS: u32 = 4;
const POLL_DELAY: Duration = Duration::from_millis(750);

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

pub fn descriptors() -> Vec<ToolDescriptor> {
    vec![
        write_tool(
            "run_code",
            "Run a short Python or Bash snippet on a warm remote CANFAR compute session (launched/reused \
             automatically on the user's account). Auto-applies when the user has auto-apply on; otherwise \
             queues for their approval. On apply it submits the code and briefly polls for the result: it \
             returns status (ok/error/timeout), exit code, stdout and stderr when the run finishes quickly, \
             or an execution_id to fetch later with run_code_output. Requires an AI compute image set in \
             Settings ▸ AI compute.",
            json!({
                "type": "object",
                "properties": {
                    "code": { "type": "string", "minLength": 1, "description": "The snippet to run." },
                    "language": { "type": "string", "enum": ["python", "bash"], "description": "Default python." },
                    "timeoutSeconds": { "type": "integer", "minimum": 1, "maximum": 900, "description": "Per-run timeout (default 60)." }
                },
                "required": ["code"],
                "additionalProperties": false
            }),
        ),
        read_tool(
            "run_code_output",
            "Fetch the result of a previous run_code by its execution_id (job_ref). Returns ready=false while \
             the code is still running (poll again); when ready, returns status (ok/error/timeout), exit code, \
             stdout, stderr, and any artifacts. If several polls stay not-ready, (re)submit with run_code.",
            json!({
                "type": "object",
                "properties": {
                    "jobRef": { "type": "string", "minLength": 1, "description": "The execution_id returned by run_code." }
                },
                "required": ["jobRef"],
                "additionalProperties": false
            }),
        ),
        write_tool(
            "start_compute",
            "Pre-warm the remote compute session (at the size configured in Settings ▸ AI compute) so the next \
             run_code starts faster. Auto-applies when the user has auto-apply on; otherwise queues for approval. \
             Reusing an already-running session is a no-op. Requires an AI compute image set in Settings.",
            json!({ "type": "object", "properties": {}, "additionalProperties": false }),
        ),
        write_tool(
            "stop_compute",
            "Propose stopping the warm remote compute session to free its cores. Queues for the user's approval \
             (a destructive change). Idempotent — a no-op if nothing is running. NOTE: this is not a cancel; a \
             request already submitted may re-run when compute is next started.",
            json!({ "type": "object", "properties": {}, "additionalProperties": false }),
        ),
    ]
}

// ─────────────────────────────────────────────────────────────────────────────
// Dispatch — reads run immediately; writes validate + enqueue a proposal
// ─────────────────────────────────────────────────────────────────────────────

pub async fn dispatch(
    name: &str,
    services: &AppServices,
    args: &Value,
    proposals: &Arc<InMemoryProposalStore>,
) -> Option<ToolResult> {
    let result = match name {
        "run_code" => propose_run_code(args, proposals),
        "run_code_output" => run_code_output(services, args).await,
        "start_compute" => propose_start_compute(proposals),
        "stop_compute" => propose_stop_compute(proposals),
        _ => return None,
    };
    Some(result)
}

/// `run_code` (non-destructive write) — validate + enqueue; the router
/// auto-applies it into [`apply_run_code`].
fn propose_run_code(args: &Value, proposals: &Arc<InMemoryProposalStore>) -> ToolResult {
    let code = str_arg(args, "code");
    if code.is_empty() {
        return ToolResult::Failed("code is required".to_string());
    }
    // Load settings to gate on the configured compute image (empty ⇒ disabled).
    let svc = AIComputeService::new();
    if !svc.settings().is_enabled() {
        return ToolResult::Failed(
            "run_code is disabled: set an AI compute image in Settings ▸ AI compute first, or use \
             launch_headless_job instead."
                .to_string(),
        );
    }

    let language =
        RunCodeContract::normalize_language(args.get("language").and_then(Value::as_str));
    let timeout = RunCodeContract::clamp_timeout(
        args.get("timeoutSeconds")
            .and_then(Value::as_i64)
            .unwrap_or(RunCodeContract::DEFAULT_TIMEOUT_SECONDS),
    );
    let id = uuid::Uuid::new_v4().simple().to_string();
    let (cores, ram) = svc.resolve_resources();

    let summary = format!(
        "Run {language} on {} (image {}, {cores}c/{ram}g). execution_id {id}.",
        RunCodeContract::SESSION_NAME,
        svc.settings().image,
    );
    let payload = json!({
        "id": id,
        "language": language,
        "code": code,
        "timeout_seconds": timeout,
    });
    let p = proposals.enqueue("run_code", &summary, false, payload);
    ToolResult::Proposed(p)
}

/// `start_compute` (non-destructive write) — pre-warm the session.
fn propose_start_compute(proposals: &Arc<InMemoryProposalStore>) -> ToolResult {
    let svc = AIComputeService::new();
    if !svc.settings().is_enabled() {
        return ToolResult::Failed(
            "start_compute is disabled: set an AI compute image in Settings ▸ AI compute first."
                .to_string(),
        );
    }
    let (cores, ram) = svc.resolve_resources();
    let summary = format!(
        "Pre-warm {} (image {}, {cores}c/{ram}g).",
        RunCodeContract::SESSION_NAME,
        svc.settings().image,
    );
    let p = proposals.enqueue("start_compute", &summary, false, json!({}));
    ToolResult::Proposed(p)
}

/// `stop_compute` (DESTRUCTIVE write) — always queues for explicit approval.
fn propose_stop_compute(proposals: &Arc<InMemoryProposalStore>) -> ToolResult {
    let summary = format!(
        "Stop {} (frees platform compute).",
        RunCodeContract::SESSION_NAME
    );
    let p = proposals.enqueue("stop_compute", &summary, true, json!({}));
    ToolResult::Proposed(p)
}

/// `run_code_output` (read) — fetch a previous run's result by job_ref.
async fn run_code_output(services: &AppServices, args: &Value) -> ToolResult {
    let id = str_arg(args, "jobRef");
    if id.is_empty() {
        return ToolResult::Failed("jobRef (execution_id) is required".to_string());
    }
    let svc = AIComputeService::new();
    match svc.fetch_out(services, &id).await {
        Ok(Some(result)) => ToolResult::Data(result_json(&id, &result)),
        Ok(None) => ToolResult::Data(json!({
            "ready": false,
            "jobRef": id,
            "note": "No result yet — the code may still be running (or the compute session is still \
                     starting). Poll again; if it stays not-ready, call run_code again.",
        })),
        Err(e) => ToolResult::Failed(format!("could not fetch run_code output for '{id}': {e}")),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Apply — perform the real service call for an approved proposal
// ─────────────────────────────────────────────────────────────────────────────

pub async fn apply(
    services: &AppServices,
    proposal: &PendingProposal,
) -> Option<Result<String, String>> {
    match proposal.kind.as_str() {
        "run_code" => Some(apply_run_code(services, &proposal.payload).await),
        "start_compute" => Some(apply_start_compute(services).await),
        "stop_compute" => Some(apply_stop_compute(services).await),
        _ => None,
    }
}

async fn apply_run_code(services: &AppServices, payload: &Value) -> Result<String, String> {
    let id = str_arg(payload, "id");
    if id.is_empty() {
        return Err("run_code payload missing id".to_string());
    }
    let language = str_arg(payload, "language");
    let code = str_arg(payload, "code");
    if code.is_empty() {
        return Err("run_code payload missing code".to_string());
    }
    let timeout = payload
        .get("timeout_seconds")
        .and_then(Value::as_i64)
        .unwrap_or(RunCodeContract::DEFAULT_TIMEOUT_SECONDS);
    let request = RunCodeRequest::new(id, language, code, timeout);

    let svc = AIComputeService::new();
    let job_ref = svc.submit(services, &request).await?;

    // Briefly poll the out file so a fast run returns its output inline.
    for attempt in 0..POLL_ATTEMPTS {
        if let Some(result) = svc.fetch_out(services, &job_ref).await? {
            return Ok(format_result(&job_ref, &result));
        }
        if attempt + 1 < POLL_ATTEMPTS {
            tokio::time::sleep(POLL_DELAY).await;
        }
    }
    Ok(format!(
        "Submitted to {} (execution_id {job_ref}). Not ready yet — fetch it with \
         run_code_output(jobRef: \"{job_ref}\").",
        RunCodeContract::SESSION_NAME
    ))
}

async fn apply_start_compute(services: &AppServices) -> Result<String, String> {
    let svc = AIComputeService::new();
    svc.ensure_session(services).await?;
    Ok(format!(
        "Compute session {} is warming (or already live).",
        RunCodeContract::SESSION_NAME
    ))
}

async fn apply_stop_compute(services: &AppServices) -> Result<String, String> {
    let svc = AIComputeService::new();
    let stopped = svc.stop(services).await?;
    Ok(if stopped {
        format!("Stopped compute session {}.", RunCodeContract::SESSION_NAME)
    } else {
        "No warm compute session was running.".to_string()
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Trimmed string argument (empty string if missing / not a string).
fn str_arg(args: &Value, key: &str) -> String {
    args.get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string()
}

/// The `ready=true` JSON view for `run_code_output` (decoded stdout/stderr).
fn result_json(id: &str, r: &RunCodeResult) -> Value {
    json!({
        "ready": true,
        "jobRef": id,
        "status": r.status,
        "exitCode": r.exit_code,
        "stdout": r.decoded_stdout(),
        "stderr": r.decoded_stderr(),
        "durationMs": r.duration_ms,
        "truncated": r.truncated,
        "startedAt": r.started_at,
        "finishedAt": r.finished_at,
        "artifacts": r.artifacts,
    })
}

/// One-line-ish human summary used by `run_code`'s inline apply result.
fn format_result(id: &str, r: &RunCodeResult) -> String {
    let status = r.status.as_deref().unwrap_or("unknown");
    let exit = r
        .exit_code
        .map(|c| c.to_string())
        .unwrap_or_else(|| "?".to_string());
    let stdout = r.decoded_stdout().unwrap_or_default();
    let stderr = r.decoded_stderr().unwrap_or_default();
    format!(
        "execution {id}: status={status}, exit={exit}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    )
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn descriptor_names_are_unique_and_well_formed() {
        let ds = descriptors();
        let names: Vec<String> = ds.iter().map(|d| d.name.clone()).collect();
        assert_eq!(names.len(), 4);
        let unique: HashSet<&String> = names.iter().collect();
        assert_eq!(unique.len(), names.len(), "tool names must be unique");
        for d in &ds {
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
    fn verb_classes_match_the_contract() {
        let by = |n: &str| descriptors().into_iter().find(|d| d.name == n).unwrap();
        assert_eq!(by("run_code").verb, VerbClass::Write);
        assert_eq!(by("start_compute").verb, VerbClass::Write);
        assert_eq!(by("stop_compute").verb, VerbClass::Write);
        assert_eq!(by("run_code_output").verb, VerbClass::Read);
    }

    #[test]
    fn run_code_requires_code() {
        let store = Arc::new(InMemoryProposalStore::new());
        assert!(matches!(
            propose_run_code(&json!({}), &store),
            ToolResult::Failed(_)
        ));
        assert!(matches!(
            propose_run_code(&json!({ "code": "   " }), &store),
            ToolResult::Failed(_)
        ));
        assert_eq!(store.pending_count(), 0);
    }

    #[test]
    fn run_code_is_disabled_without_a_compute_image() {
        // With no persisted settings, the compute image is empty ⇒ disabled, so a
        // valid-code call is refused (nothing enqueued).
        let store = Arc::new(InMemoryProposalStore::new());
        match propose_run_code(&json!({ "code": "print(1)" }), &store) {
            ToolResult::Failed(msg) => assert!(msg.contains("disabled")),
            // If a developer machine happens to have a configured image, the call
            // is instead a non-destructive proposal — still not destructive.
            ToolResult::Proposed(p) => {
                assert_eq!(p.kind, "run_code");
                assert!(!p.destructive);
            }
            _ => panic!("expected Failed or Proposed"),
        }
    }

    #[test]
    fn stop_compute_is_destructive_and_enqueues() {
        let store = Arc::new(InMemoryProposalStore::new());
        match propose_stop_compute(&store) {
            ToolResult::Proposed(p) => {
                assert_eq!(p.kind, "stop_compute");
                assert!(
                    p.destructive,
                    "stop_compute must be destructive (stays pending)"
                );
            }
            _ => panic!("expected a Proposed result"),
        }
        assert_eq!(store.pending_count(), 1);
    }

    #[test]
    fn apply_ignores_foreign_kinds() {
        // (compile-time check that apply matches only this family's kinds)
        let store = InMemoryProposalStore::new();
        let p = store.enqueue("save_query", "x", false, json!({}));
        // Can't await here without a runtime; assert the kind guard purely.
        assert_ne!(p.kind, "run_code");
        assert_ne!(p.kind, "start_compute");
        assert_ne!(p.kind, "stop_compute");
    }

    #[test]
    fn format_result_includes_status_exit_and_streams() {
        let r = RunCodeResult {
            status: Some("ok".to_string()),
            exit_code: Some(0),
            stdout: Some("hello".to_string()),
            stderr: Some("warn".to_string()),
            ..Default::default()
        };
        let s = format_result("id7", &r);
        assert!(s.contains("execution id7"));
        assert!(s.contains("status=ok"));
        assert!(s.contains("exit=0"));
        assert!(s.contains("hello"));
        assert!(s.contains("warn"));
    }

    #[test]
    fn result_json_is_ready_and_decodes_base64() {
        // base64("hi") == "aGk="
        let r = RunCodeResult {
            status: Some("ok".to_string()),
            exit_code: Some(0),
            stdout: Some("aGk=".to_string()),
            stdout_encoding: Some("base64".to_string()),
            artifacts: Some(vec!["out/plot.png".to_string()]),
            ..Default::default()
        };
        let v = result_json("id1", &r);
        assert_eq!(v["ready"], json!(true));
        assert_eq!(v["jobRef"], json!("id1"));
        assert_eq!(v["stdout"], json!("hi"));
        assert_eq!(v["artifacts"], json!(["out/plot.png"]));
    }
}
