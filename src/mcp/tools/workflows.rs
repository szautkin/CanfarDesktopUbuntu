//! Workflow tools — the research-protocols MCP family. Ported from
//! `Mcp/Tools/Write/WorkflowTools.cs` (`ListWorkflowsTool`, `GetWorkflowTool`,
//! `SaveWorkflowTool`, `UpdateWorkflowTool`, `SetWorkflowStepTool`,
//! `DeleteWorkflowTool` + their appliers).
//!
//! Workflows are research protocols the agent can read, follow, author, and check
//! off. Two tiers are readable: read-only built-in templates and the user's LOCAL
//! working copies (the only tier that carries step check-off progress). Every
//! mutation targets the local tier only — the store's own errors say to make a
//! local copy first when a built-in id is passed to a mutation.
//!
//! Contract (chained by the router): [`descriptors`] advertises the six tools,
//! [`dispatch`] runs one by name (`None` when another module owns it), and
//! [`apply`] performs the real store call for an approved proposal (`None` when
//! another family owns the proposal kind).
//!
//! Reads run synchronous store calls directly; writes NEVER mutate at propose
//! time — they validate arguments and enqueue a [`PendingProposal`] whose payload
//! echoes the arguments, which [`apply`] later decodes to hit the store. Only
//! `delete_workflow` is destructive (always queues for explicit approval); the
//! create/update/check-off writes are non-destructive and may auto-apply.
//!
//! Each call constructs a fresh [`WorkflowStore::new`] (the store is a cheap,
//! stateless handle to the user's `<data_dir>/workflows` directory — no shared
//! service instance to thread through `AppServices`).

use crate::helpers::workflow_format;
use crate::mcp::tools::proposals::{InMemoryProposalStore, PendingProposal};
use crate::mcp::tools::{str_arg, ToolDescriptor, ToolResult, VerbClass};
use crate::models::workflow::{WorkflowInfo, WorkflowSource};
use crate::services::workflow_store::{WorkflowStore, LOCAL_PREFIX};
use crate::state::AppServices;
use serde_json::{json, Value};
use std::sync::Arc;

// ─────────────────────────────────────────────────────────────────────────────
// Manifest
// ─────────────────────────────────────────────────────────────────────────────

/// All workflow tool descriptors. Reads are `verb: Read`; writes are
/// `verb: Write`. Every tool is `agent_safe: true` — proposing a write is safe;
/// the proposal gate governs applying it.
pub fn descriptors() -> Vec<ToolDescriptor> {
    vec![
        ToolDescriptor {
            name: "list_workflows".to_string(),
            description: "List the user's research workflows: built-in templates (read-only \
                protocols for classic CADC/CANFAR research tasks) and their local working copies \
                (which carry step check-off progress). Read one with get_workflow; make a local \
                working copy of a template with save_workflow."
                .to_string(),
            input_schema: json!({
                "type": "object", "properties": {}, "additionalProperties": false
            }),
            verb: VerbClass::Read,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "get_workflow".to_string(),
            description:
                "Read one workflow: metadata plus every step (0-based index, title, body, \
                the agent tools the step uses, the app view it belongs to, done flag) and its raw \
                `.workflow.md` text. To follow a workflow, do the steps in order with the named \
                tools and mark each with set_workflow_step. Ids come from list_workflows."
                    .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "builtin:… or local:… id from list_workflows" }
                },
                "required": ["id"],
                "additionalProperties": false
            }),
            verb: VerbClass::Read,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "save_workflow".to_string(),
            description: "Create a NEW local workflow from markdown-checklist text (e.g. turn the \
                current conversation's plan into a reusable protocol). Format: `# Title`, \
                `> description`, `Tags: a, b`, then steps as `- [ ] **Step title** — what to do` \
                with optional indented `Tool: name1, name2`, `View: search`, `Note: hint` lines. \
                Queues for the user to apply."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "minLength": 1, "description": "Display name for the new workflow." },
                    "text": { "type": "string", "minLength": 1, "description": "Full .workflow.md content." }
                },
                "required": ["name", "text"],
                "additionalProperties": false
            }),
            verb: VerbClass::Write,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "use_workflow".to_string(),
            description: "Make a local working copy of a workflow so you can follow it and check \
                off progress. Give the id of a built-in template (or any workflow) from \
                list_workflows; a fresh local copy is created (progress reset to unchecked). \
                Queues for the user to apply."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "builtin:… (or any) id from list_workflows to instantiate." },
                    "name": { "type": "string", "description": "Optional name for the new local copy (defaults to the template's title)." }
                },
                "required": ["id"],
                "additionalProperties": false
            }),
            verb: VerbClass::Write,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "update_workflow".to_string(),
            description:
                "Replace the full text of a LOCAL workflow (refine a protocol). Built-in \
                templates are read-only — save_workflow a copy first. Queues for the user to apply."
                    .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "local:… id" },
                    "text": { "type": "string", "minLength": 1, "description": "Full replacement .workflow.md content." }
                },
                "required": ["id", "text"],
                "additionalProperties": false
            }),
            verb: VerbClass::Write,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "set_workflow_step".to_string(),
            description: "Mark a step of a LOCAL workflow done (or not done) by its 0-based step \
                index — call this after completing each step so the user sees live progress. \
                Queues for the user to apply."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "local:… id" },
                    "step": { "type": "integer", "minimum": 0, "description": "0-based step index." },
                    "done": { "type": "boolean", "description": "Default true." }
                },
                "required": ["id", "step"],
                "additionalProperties": false
            }),
            verb: VerbClass::Write,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "delete_workflow".to_string(),
            description:
                "Delete a LOCAL workflow file, including its progress. Built-in templates \
                cannot be deleted. Queues for the user's approval (a destructive change)."
                    .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "local:… id" }
                },
                "required": ["id"],
                "additionalProperties": false
            }),
            verb: VerbClass::Write,
            agent_safe: true,
        },
    ]
}

// ─────────────────────────────────────────────────────────────────────────────
// Dispatch — reads run the store directly; writes validate + enqueue a proposal
// ─────────────────────────────────────────────────────────────────────────────

/// Handle a workflow tool call. Returns `Some(..)` when this module owns `name`
/// (a `Data`/`Failed` for reads, a `Proposed`/`Failed` for writes), or `None` so
/// the router can fall through to another catalog. `_services` is unused: the
/// store is constructed fresh per call and writes must not mutate at propose time.
pub async fn dispatch(
    name: &str,
    _services: &AppServices,
    args: &Value,
    proposals: &Arc<InMemoryProposalStore>,
) -> Option<ToolResult> {
    let result = match name {
        "list_workflows" => list_workflows(),
        "get_workflow" => get_workflow(args),
        "save_workflow" => propose_save_workflow(args, proposals),
        "use_workflow" => propose_use_workflow(args, proposals),
        "update_workflow" => propose_update_workflow(args, proposals),
        "set_workflow_step" => propose_set_workflow_step(args, proposals),
        "delete_workflow" => propose_delete_workflow(args, proposals),
        _ => return None,
    };
    Some(result)
}

/// `list_workflows` — built-in templates ++ the user's local working copies, each
/// as a compact summary (id, title, description, tags, source, progress counts).
fn list_workflows() -> ToolResult {
    let store = WorkflowStore::new();
    let items: Vec<Value> = store
        .list_built_in()
        .into_iter()
        .chain(store.list_local())
        .map(|w| summary_json(&w))
        .collect();
    ToolResult::Data(json!({ "count": items.len(), "workflows": items }))
}

/// `get_workflow` — full structured steps + progress + raw text of one workflow.
fn get_workflow(args: &Value) -> ToolResult {
    let id = str_arg(args, "id");
    if id.is_empty() {
        return ToolResult::Failed("id is required".to_string());
    }
    let store = WorkflowStore::new();
    match store.get(&id) {
        Some(info) => ToolResult::Data(workflow_json(&info)),
        None => ToolResult::Failed(format!(
            "no workflow '{}' — call list_workflows for ids",
            id
        )),
    }
}

/// `save_workflow` — validate the markdown-checklist text (must parse to ≥1 step)
/// and enqueue a non-destructive create proposal echoing `{name, text}`.
fn propose_save_workflow(args: &Value, proposals: &Arc<InMemoryProposalStore>) -> ToolResult {
    let name = str_arg(args, "name");
    // Preserve the raw text verbatim — it IS the file content; only validate it.
    let text = args
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if name.is_empty() {
        return ToolResult::Failed("name is required".to_string());
    }
    if text.trim().is_empty() {
        return ToolResult::Failed("text is required".to_string());
    }
    let doc = workflow_format::parse(&text);
    if doc.steps.is_empty() {
        return ToolResult::Failed(
            "the text has no steps — steps are `- [ ] **Step title** — description` lines"
                .to_string(),
        );
    }
    let payload = json!({ "name": name, "text": text });
    let summary = format!("Save workflow \"{}\" ({} steps)", name, doc.steps.len());
    let p = proposals.enqueue("save_workflow", &summary, false, payload);
    ToolResult::Proposed(p)
}

/// `use_workflow` — instantiate any workflow (typically a built-in template) as a
/// NEW local working copy with fresh progress. Resolves the source text at propose
/// time (so a missing id fails fast) and enqueues a non-destructive create.
fn propose_use_workflow(args: &Value, proposals: &Arc<InMemoryProposalStore>) -> ToolResult {
    let id = str_arg(args, "id");
    if id.is_empty() {
        return ToolResult::Failed("id is required".to_string());
    }
    let info = match WorkflowStore::new().get(&id) {
        Some(w) => w,
        None => {
            return ToolResult::Failed(format!(
                "no workflow '{}' — call list_workflows for ids",
                id
            ))
        }
    };
    let name = {
        let n = str_arg(args, "name");
        if n.trim().is_empty() {
            info.doc.title.clone()
        } else {
            n
        }
    };
    let name = if name.trim().is_empty() {
        "Workflow".to_string()
    } else {
        name
    };
    let payload = json!({ "name": name, "text": info.raw_text });
    let summary = format!(
        "Use workflow \"{}\" → new local copy \"{}\"",
        info.doc.title, name
    );
    let p = proposals.enqueue("use_workflow", &summary, false, payload);
    ToolResult::Proposed(p)
}

/// `update_workflow` — replace a LOCAL workflow's full text (non-destructive).
/// Rejects a non-local id up front (templates are read-only).
fn propose_update_workflow(args: &Value, proposals: &Arc<InMemoryProposalStore>) -> ToolResult {
    let id = str_arg(args, "id");
    let text = args
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if !id.starts_with(LOCAL_PREFIX) {
        return ToolResult::Failed(
            "id must be a local:… workflow (templates are read-only — save_workflow a copy first)"
                .to_string(),
        );
    }
    if text.trim().is_empty() {
        return ToolResult::Failed("text is required".to_string());
    }
    let doc = workflow_format::parse(&text);
    let payload = json!({ "id": id, "text": text });
    let summary = format!(
        "Update workflow {} (\"{}\", {} steps)",
        id,
        doc.title,
        doc.steps.len()
    );
    let p = proposals.enqueue("update_workflow", &summary, false, payload);
    ToolResult::Proposed(p)
}

/// `set_workflow_step` — check a step off (or back on) in a LOCAL workflow by its
/// 0-based index (non-destructive). Built-in / out-of-range ids are surfaced by
/// the store at apply time (mirrors the C# tool, which only validates step ≥ 0).
fn propose_set_workflow_step(args: &Value, proposals: &Arc<InMemoryProposalStore>) -> ToolResult {
    let id = str_arg(args, "id");
    if id.is_empty() {
        return ToolResult::Failed("id is required".to_string());
    }
    let step = match args.get("step").and_then(Value::as_i64) {
        Some(n) if n >= 0 => n as usize,
        _ => return ToolResult::Failed("step must be an integer >= 0".to_string()),
    };
    let done = args.get("done").and_then(Value::as_bool).unwrap_or(true);
    let payload = json!({ "id": id, "step": step, "done": done });
    let summary = format!(
        "Mark workflow {} step {} {}",
        id,
        step + 1,
        if done { "done" } else { "not done" }
    );
    let p = proposals.enqueue("set_workflow_step", &summary, false, payload);
    ToolResult::Proposed(p)
}

/// `delete_workflow` — delete a LOCAL workflow file (DESTRUCTIVE: always queues).
/// Rejects a non-local id up front (templates cannot be deleted).
fn propose_delete_workflow(args: &Value, proposals: &Arc<InMemoryProposalStore>) -> ToolResult {
    let id = str_arg(args, "id");
    if !id.starts_with(LOCAL_PREFIX) {
        return ToolResult::Failed(
            "id must be a local:… workflow (templates cannot be deleted)".to_string(),
        );
    }
    let payload = json!({ "id": id });
    let p = proposals.enqueue(
        "delete_workflow",
        &format!("Delete workflow {}", id),
        true,
        payload,
    );
    ToolResult::Proposed(p)
}

// ─────────────────────────────────────────────────────────────────────────────
// Apply — decode an approved proposal's payload + perform the real store call
// ─────────────────────────────────────────────────────────────────────────────

/// Execute an approved workflow proposal by matching on its `kind`. Returns
/// `Some(Ok(msg))` / `Some(Err(e))` when this family owns the kind, or `None` so
/// the router can try another family. `_services` is unused — workflows live in
/// local files, not behind an authenticated service.
pub async fn apply(
    _services: &AppServices,
    proposal: &PendingProposal,
) -> Option<Result<String, String>> {
    let payload = &proposal.payload;
    let result = match proposal.kind.as_str() {
        "save_workflow" => {
            let name = str_arg(payload, "name");
            let text = payload
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if name.is_empty() {
                return Some(Err("save_workflow payload missing name".to_string()));
            }
            WorkflowStore::new()
                .save_new(&name, &text)
                .map(|info| format!("Saved workflow '{}' ({})", name, info.id))
        }
        "use_workflow" => {
            let name = str_arg(payload, "name");
            let text = payload
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if name.is_empty() {
                return Some(Err("use_workflow payload missing name".to_string()));
            }
            WorkflowStore::new()
                .save_new(&name, &text)
                .map(|info| format!("Created workflow '{}' ({}) from a template", name, info.id))
        }
        "update_workflow" => {
            let id = str_arg(payload, "id");
            let text = payload
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if id.is_empty() {
                return Some(Err("update_workflow payload missing id".to_string()));
            }
            WorkflowStore::new()
                .update_text(&id, &text)
                .map(|()| format!("Updated workflow {}", id))
        }
        "set_workflow_step" => {
            let id = str_arg(payload, "id");
            if id.is_empty() {
                return Some(Err("set_workflow_step payload missing id".to_string()));
            }
            let step = payload.get("step").and_then(Value::as_u64).unwrap_or(0) as usize;
            let done = payload.get("done").and_then(Value::as_bool).unwrap_or(true);
            WorkflowStore::new()
                .set_step_done(&id, step, done)
                .map(|info| {
                    format!(
                        "Marked workflow {} step {} {} ({} / {} steps done)",
                        id,
                        step + 1,
                        if done { "done" } else { "not done" },
                        info.doc.done_count(),
                        info.doc.steps.len()
                    )
                })
        }
        "delete_workflow" => {
            let id = str_arg(payload, "id");
            if id.is_empty() {
                return Some(Err("delete_workflow payload missing id".to_string()));
            }
            WorkflowStore::new()
                .delete(&id)
                .map(|()| format!("Deleted workflow {}", id))
        }
        _ => return None,
    };
    Some(result)
}

// ─────────────────────────────────────────────────────────────────────────────
// JSON shaping + argument helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Compact list-row for one workflow.
fn summary_json(w: &WorkflowInfo) -> Value {
    json!({
        "id": w.id,
        "title": w.doc.title,
        "description": w.doc.description,
        "tags": w.doc.tags(),
        "source": source_str(w.source),
        "doneCount": w.doc.done_count(),
        "totalSteps": w.doc.steps.len(),
    })
}

/// Full detail for one workflow, including every step and the raw text.
fn workflow_json(w: &WorkflowInfo) -> Value {
    let steps: Vec<Value> = w
        .doc
        .steps
        .iter()
        .map(|s| {
            json!({
                "index": s.index,
                "title": s.title,
                "body": s.body,
                "tools": s.tools,
                "view": s.view,
                "note": s.note,
                "done": s.done,
            })
        })
        .collect();
    json!({
        "id": w.id,
        "title": w.doc.title,
        "description": w.doc.description,
        "tags": w.doc.tags(),
        "source": source_str(w.source),
        "doneCount": w.doc.done_count(),
        "totalSteps": w.doc.steps.len(),
        "steps": steps,
        "rawText": w.raw_text,
    })
}

/// Stable, lowercase source token aligned with the id prefixes.
fn source_str(source: WorkflowSource) -> &'static str {
    match source {
        WorkflowSource::BuiltIn => "builtin",
        WorkflowSource::Local => "local",
        WorkflowSource::VoSpace => "vospace",
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// A tiny valid `.workflow.md` body used by the propose-path tests (parsed,
    /// never written to disk).
    const SAMPLE: &str = "# Sample Protocol\n> A tiny test protocol.\nTags: test\n\n## Steps\n\n- [ ] **First step** — do the thing.\n- [ ] **Second step** — do another thing.\n";

    /// Names must be unique and non-empty (a duplicate would make dispatch
    /// ambiguous; an empty name is unroutable).
    #[test]
    fn descriptors_names_unique_and_non_empty() {
        let mut seen = HashSet::new();
        for d in descriptors() {
            assert!(!d.name.is_empty(), "empty descriptor name");
            assert!(seen.insert(d.name.clone()), "duplicate name: {}", d.name);
        }
        assert_eq!(seen.len(), 7, "expected seven workflow tools");
    }

    /// Verb + agent-safe invariants: reads are Read, writes are Write, all safe.
    #[test]
    fn descriptors_have_expected_verbs() {
        for d in descriptors() {
            assert!(d.agent_safe, "{} must be agent_safe", d.name);
            let expected = match d.name.as_str() {
                "list_workflows" | "get_workflow" => VerbClass::Read,
                _ => VerbClass::Write,
            };
            assert_eq!(d.verb, expected, "wrong verb for {}", d.name);
        }
    }

    /// save_workflow enqueues exactly one non-destructive proposal (no disk I/O).
    #[test]
    fn save_workflow_enqueues_non_destructive() {
        let store = Arc::new(InMemoryProposalStore::new());
        match propose_save_workflow(&json!({ "name": "My Protocol", "text": SAMPLE }), &store) {
            ToolResult::Proposed(p) => {
                assert_eq!(p.kind, "save_workflow");
                assert!(!p.destructive);
            }
            _ => panic!("expected a Proposed result"),
        }
        assert_eq!(store.pending_count(), 1);
    }

    /// save_workflow rejects text that parses to zero steps, and never queues.
    #[test]
    fn save_workflow_requires_steps_and_name() {
        let store = Arc::new(InMemoryProposalStore::new());
        assert!(matches!(
            propose_save_workflow(&json!({ "name": "X", "text": "# Just a title\n" }), &store),
            ToolResult::Failed(_)
        ));
        assert!(matches!(
            propose_save_workflow(&json!({ "text": SAMPLE }), &store),
            ToolResult::Failed(_)
        ));
        assert_eq!(store.pending_count(), 0);
    }

    /// delete_workflow is destructive and rejects non-local ids.
    #[test]
    fn delete_workflow_is_destructive_and_local_only() {
        let store = Arc::new(InMemoryProposalStore::new());
        match propose_delete_workflow(&json!({ "id": "local:my-protocol" }), &store) {
            ToolResult::Proposed(p) => assert!(p.destructive, "delete must be destructive"),
            _ => panic!("expected Proposed"),
        }
        assert!(matches!(
            propose_delete_workflow(&json!({ "id": "builtin:cfht-imaging-recon" }), &store),
            ToolResult::Failed(_)
        ));
        // update_workflow shares the local-only guard.
        assert!(matches!(
            propose_update_workflow(
                &json!({ "id": "builtin:cfht-imaging-recon", "text": SAMPLE }),
                &store
            ),
            ToolResult::Failed(_)
        ));
    }

    /// set_workflow_step validates the step index and defaults done to true.
    #[test]
    fn set_workflow_step_validates_index() {
        let store = Arc::new(InMemoryProposalStore::new());
        match propose_set_workflow_step(&json!({ "id": "local:x", "step": 0 }), &store) {
            ToolResult::Proposed(p) => {
                assert_eq!(p.kind, "set_workflow_step");
                assert!(!p.destructive);
                assert_eq!(p.payload.get("done").and_then(Value::as_bool), Some(true));
            }
            _ => panic!("expected Proposed"),
        }
        assert!(matches!(
            propose_set_workflow_step(&json!({ "id": "local:x", "step": -1 }), &store),
            ToolResult::Failed(_)
        ));
    }
}
