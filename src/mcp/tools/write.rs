//! Write tools — the "propose, never mutate" half of the MCP catalog. Ported from
//! `Mcp/Tools/Write/SavedQueryWriteTools.cs` + `SessionWriteTools.cs` and the
//! proposal appliers in `Mcp/Tools/Proposals/`.
//!
//! A write tool NEVER touches `AppServices` directly. It validates its arguments,
//! enqueues a [`PendingProposal`] describing the intended change, and returns
//! [`ToolResult::Proposed`]. The change only happens later, when the user (or the
//! auto-apply policy for non-destructive kinds) approves the proposal and the host
//! calls [`apply`], which decodes the proposal payload and performs the real
//! service call.
//!
//! Destructive kinds (`delete_*` / `launch_*`) are flagged `destructive: true` so
//! the host never auto-applies them. All write tools are `agent_safe: true`: an
//! external agent may *propose* a change, but the proposal gate keeps it from
//! taking effect without a human in the loop.

use crate::helpers::agent_attribution::AgentAttribution;
use crate::mcp::tools::proposals::{InMemoryProposalStore, PendingProposal};
use crate::mcp::tools::{opt_u32, str_arg, ToolDescriptor, ToolResult, VerbClass};
use crate::models::search_result::SavedQuery;
use crate::models::SessionLaunchParams;
use crate::state::AppServices;
use serde_json::{json, Value};
use std::sync::Arc;

/// Interactive Skaha session types accepted by `launch_session` (mirrors the C#
/// `SessionWriteHelpers.InteractiveTypes`).
const INTERACTIVE_TYPES: &[&str] = &["notebook", "desktop", "carta", "contributed", "firefly"];

use crate::models::session_launch_params::{
    agent_session_name, DEFAULT_CORES, DEFAULT_GPUS, DEFAULT_RAM_GB,
};

// ─────────────────────────────────────────────────────────────────────────────
// Manifest
// ─────────────────────────────────────────────────────────────────────────────

/// The write-tool descriptors advertised to clients. Every entry is `verb: Write`
/// and `agent_safe: true` — proposing is safe; the proposal gate governs applying.
pub fn descriptors() -> Vec<ToolDescriptor> {
    vec![
        ToolDescriptor {
            name: "save_query".to_string(),
            description:
                "Propose saving a named ADQL query to the user's saved queries (overwrites \
                an existing query with the same name). Queues for the user to apply."
                    .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Name to save the query under." },
                    "adql": { "type": "string", "description": "The ADQL query text." }
                },
                "required": ["name", "adql"],
                "additionalProperties": false
            }),
            verb: VerbClass::Write,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "delete_saved_query".to_string(),
            description: "Propose deleting a saved query by name. Queues for the user to apply \
                (a destructive change)."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Name of the saved query to delete." }
                },
                "required": ["name"],
                "additionalProperties": false
            }),
            verb: VerbClass::Write,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "launch_session".to_string(),
            description: "Propose launching an interactive Skaha session \
                (notebook/desktop/carta/contributed/firefly) from a container image, with an \
                optional name and CPU/RAM(GB). Queues for the user to apply; after it applies, \
                find the new session via list_sessions."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "kind": {
                        "type": "string",
                        "enum": ["notebook", "desktop", "carta", "contributed", "firefly"],
                        "description": "Interactive session type."
                    },
                    "image": { "type": "string", "description": "Container image id to launch." },
                    "name": { "type": "string", "description": "Optional display name for the session." },
                    "cores": { "type": "integer", "minimum": 1, "description": "CPU cores (default 2)." },
                    "ram": { "type": "integer", "minimum": 1, "description": "RAM in GB (default 8)." },
                    "gpus": { "type": "integer", "minimum": 0, "description": "GPUs (default 0)." }
                },
                "required": ["kind", "image"],
                "additionalProperties": false
            }),
            verb: VerbClass::Write,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "delete_session".to_string(),
            description: "Propose terminating a running Skaha session by its id. Queues for the \
                user to apply (a destructive change). Get ids from list_sessions."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Session id to terminate." }
                },
                "required": ["id"],
                "additionalProperties": false
            }),
            verb: VerbClass::Write,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "renew_session".to_string(),
            description: "Propose renewing (extending the expiry of) a running Skaha session by \
                its id. Queues for the user to apply."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "Session id to renew." }
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
// Dispatch — validate args + enqueue a proposal (no mutation)
// ─────────────────────────────────────────────────────────────────────────────

/// Handle a write-tool call. Returns `Some(ToolResult::Proposed)` on success,
/// `Some(ToolResult::Failed)` on a validation error, or `None` if `name` is not a
/// write tool (so the router can fall through to other catalogs).
///
/// `_services` is intentionally unused: write tools must not read or mutate app
/// state at propose time — the real service call happens in [`apply`].
pub async fn dispatch(
    name: &str,
    _services: &AppServices,
    args: &Value,
    proposals: &Arc<InMemoryProposalStore>,
) -> Option<ToolResult> {
    let result = match name {
        "save_query" => propose_save_query(args, proposals),
        "delete_saved_query" => propose_delete_saved_query(args, proposals),
        "launch_session" => propose_launch_session(args, proposals),
        "delete_session" => propose_delete_session(args, proposals),
        "renew_session" => propose_renew_session(args, proposals),
        _ => return None,
    };
    Some(result)
}

fn propose_save_query(args: &Value, proposals: &Arc<InMemoryProposalStore>) -> ToolResult {
    let name = str_arg(args, "name");
    let adql = str_arg(args, "adql");
    if name.is_empty() {
        return ToolResult::Failed("name is required".to_string());
    }
    if adql.is_empty() {
        return ToolResult::Failed("adql is required".to_string());
    }
    let payload = json!({ "name": name, "adql": adql });
    let p = proposals.enqueue(
        "save_query",
        &format!("Save query: {}", name),
        false,
        payload,
    );
    ToolResult::Proposed(p)
}

fn propose_delete_saved_query(args: &Value, proposals: &Arc<InMemoryProposalStore>) -> ToolResult {
    let name = str_arg(args, "name");
    if name.is_empty() {
        return ToolResult::Failed("name is required".to_string());
    }
    let payload = json!({ "name": name });
    let p = proposals.enqueue(
        "delete_saved_query",
        &format!("Delete saved query: {}", name),
        true,
        payload,
    );
    ToolResult::Proposed(p)
}

fn propose_launch_session(args: &Value, proposals: &Arc<InMemoryProposalStore>) -> ToolResult {
    let kind = str_arg(args, "kind").to_lowercase();
    if !INTERACTIVE_TYPES.contains(&kind.as_str()) {
        return ToolResult::Failed(format!(
            "kind must be one of: {}",
            INTERACTIVE_TYPES.join(", ")
        ));
    }
    let image = str_arg(args, "image");
    if image.is_empty() {
        return ToolResult::Failed("image is required".to_string());
    }
    // Validate optional resources up front so a bad size is rejected at propose
    // time rather than surfacing as an opaque launch failure at apply time.
    let cores = match validated_resource(args, "cores") {
        Ok(v) => v,
        Err(e) => return ToolResult::Failed(e),
    };
    let ram = match validated_resource(args, "ram") {
        Ok(v) => v,
        Err(e) => return ToolResult::Failed(e),
    };
    let name = str_arg(args, "name");
    let gpus = crate::mcp::tools::opt_u32(args, "gpus");

    let mut payload = json!({ "kind": kind, "image": image });
    if !name.is_empty() {
        payload["name"] = json!(name);
    }
    if let Some(c) = cores {
        payload["cores"] = json!(c);
    }
    if let Some(g) = gpus {
        payload["gpus"] = json!(g);
    }
    if let Some(r) = ram {
        payload["ram"] = json!(r);
    }

    let label = if name.is_empty() { &image } else { &name };
    let p = proposals.enqueue(
        "launch_session",
        &format!("Launch {} session: {}", kind, label),
        true,
        payload,
    );
    ToolResult::Proposed(p)
}

fn propose_delete_session(args: &Value, proposals: &Arc<InMemoryProposalStore>) -> ToolResult {
    let id = str_arg(args, "id");
    if id.is_empty() {
        return ToolResult::Failed("id is required".to_string());
    }
    let payload = json!({ "id": id });
    let p = proposals.enqueue(
        "delete_session",
        &format!("Terminate session {}", id),
        true,
        payload,
    );
    ToolResult::Proposed(p)
}

fn propose_renew_session(args: &Value, proposals: &Arc<InMemoryProposalStore>) -> ToolResult {
    let id = str_arg(args, "id");
    if id.is_empty() {
        return ToolResult::Failed("id is required".to_string());
    }
    let payload = json!({ "id": id });
    let p = proposals.enqueue(
        "renew_session",
        &format!("Renew session {}", id),
        false,
        payload,
    );
    ToolResult::Proposed(p)
}

// ─────────────────────────────────────────────────────────────────────────────
// Apply — decode an approved proposal's payload + perform the real service call
// ─────────────────────────────────────────────────────────────────────────────

/// Execute an approved proposal by matching on its `kind`. Returns a short
/// success string, or `Err` on a missing token, empty payload field, or a failed
/// service call. Never mutates unless the proposal has been approved by the host.
pub async fn apply(services: &AppServices, proposal: &PendingProposal) -> Result<String, String> {
    let payload = &proposal.payload;
    match proposal.kind.as_str() {
        "save_query" => {
            let name = str_arg(payload, "name");
            let adql = str_arg(payload, "adql");
            if name.is_empty() {
                return Err("save_query payload missing name".to_string());
            }
            let query = SavedQuery {
                name: name.clone(),
                adql,
                created_at: chrono::Utc::now().to_rfc3339(),
                // Stamp provenance when this apply originated from an external
                // agent proposal; user-originated proposals get no badge.
                agent_attribution: AgentAttribution::for_applied_proposal(proposal),
            };
            services.search_store.save_query(query)?;
            Ok(format!("Saved query '{}'", name))
        }
        "delete_saved_query" => {
            let name = str_arg(payload, "name");
            if name.is_empty() {
                return Err("delete_saved_query payload missing name".to_string());
            }
            services.search_store.delete_saved(&name)?;
            Ok(format!("Deleted saved query '{}'", name))
        }
        "launch_session" => {
            let token = services
                .get_token()
                .await
                .ok_or_else(|| "not signed in".to_string())?;
            let kind = str_arg(payload, "kind");
            let image = str_arg(payload, "image");
            if kind.is_empty() || image.is_empty() {
                return Err("launch_session payload missing kind/image".to_string());
            }
            let name = agent_session_name(&str_arg(payload, "name"), &kind);
            let cores = opt_u32(payload, "cores").unwrap_or(DEFAULT_CORES);
            let ram = opt_u32(payload, "ram").unwrap_or(DEFAULT_RAM_GB);
            let gpus = opt_u32(payload, "gpus").unwrap_or(DEFAULT_GPUS);
            let params = SessionLaunchParams {
                name: name.clone(),
                image,
                session_type: kind,
                cores,
                ram,
                gpus,
                cmd: None,
                env: None,
                registry_username: None,
                registry_secret: None,
                args: None,
                replicas: None,
            };
            let id = services.sessions.launch_session(&token, &params).await?;
            Ok(format!("Launched session '{}' (id {})", name, id))
        }
        "delete_session" => {
            let token = services
                .get_token()
                .await
                .ok_or_else(|| "not signed in".to_string())?;
            let id = str_arg(payload, "id");
            if id.is_empty() {
                return Err("delete_session payload missing id".to_string());
            }
            services.sessions.delete_session(&token, &id).await?;
            Ok(format!("Terminated session {}", id))
        }
        "renew_session" => {
            let token = services
                .get_token()
                .await
                .ok_or_else(|| "not signed in".to_string())?;
            let id = str_arg(payload, "id");
            if id.is_empty() {
                return Err("renew_session payload missing id".to_string());
            }
            services.sessions.renew_session(&token, &id).await?;
            Ok(format!("Renewed session {}", id))
        }
        other => Err(format!("no applier for proposal kind '{}'", other)),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Agent attribution
// ─────────────────────────────────────────────────────────────────────────────

// ─────────────────────────────────────────────────────────────────────────────
// Argument helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Validate an optional resource field: absent → `Ok(None)`, present and `>= 1` →
/// `Ok(Some(n))`, otherwise an error string.
fn validated_resource(args: &Value, key: &str) -> Result<Option<u32>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => match v.as_u64() {
            Some(n) if n >= 1 => Ok(Some(n as u32)),
            _ => Err(format!("{} must be an integer >= 1", key)),
        },
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_query_dispatch_enqueues_one_pending() {
        let store = Arc::new(InMemoryProposalStore::new());
        let result = propose_save_query(
            &json!({ "name": "M31", "adql": "SELECT * FROM caom2.Observation" }),
            &store,
        );
        match result {
            ToolResult::Proposed(p) => {
                assert_eq!(p.kind, "save_query");
                assert!(!p.destructive);
            }
            _ => panic!("expected a Proposed result"),
        }
        assert_eq!(store.pending_count(), 1);
    }

    #[test]
    fn save_query_requires_fields() {
        let store = Arc::new(InMemoryProposalStore::new());
        assert!(matches!(
            propose_save_query(&json!({ "adql": "SELECT 1" }), &store),
            ToolResult::Failed(_)
        ));
        assert_eq!(store.pending_count(), 0);
    }

    #[test]
    fn destructive_kinds_are_flagged() {
        let store = Arc::new(InMemoryProposalStore::new());
        let del = propose_delete_session(&json!({ "id": "abc123" }), &store);
        match del {
            ToolResult::Proposed(p) => assert!(p.destructive, "delete_session must be destructive"),
            _ => panic!("expected Proposed"),
        }
        let launch =
            propose_launch_session(&json!({ "kind": "notebook", "image": "img:1" }), &store);
        match launch {
            ToolResult::Proposed(p) => assert!(p.destructive, "launch_session must be destructive"),
            _ => panic!("expected Proposed"),
        }
    }

    #[test]
    fn launch_session_rejects_bad_kind_and_size() {
        let store = Arc::new(InMemoryProposalStore::new());
        assert!(matches!(
            propose_launch_session(&json!({ "kind": "bogus", "image": "img:1" }), &store),
            ToolResult::Failed(_)
        ));
        assert!(matches!(
            propose_launch_session(
                &json!({ "kind": "notebook", "image": "img:1", "cores": 0 }),
                &store
            ),
            ToolResult::Failed(_)
        ));
        assert_eq!(store.pending_count(), 0);
    }

    #[test]
    fn attribution_for_stamps_only_agent_origin() {
        let store = InMemoryProposalStore::new();
        // User-originated proposal (origin None) → no badge.
        let user = store.enqueue("save_query", "Save query: M31", false, json!({}));
        assert!(AgentAttribution::for_applied_proposal(&user).is_none());

        // Agent-originated proposal (origin set by the router) → stamped.
        let agent = store.enqueue("save_query", "Save query: NGC 224", false, json!({}));
        store.set_origin(&agent.id, Some("Claude Desktop".to_string()));
        let agent = store.get(&agent.id).unwrap();
        let attr =
            AgentAttribution::for_applied_proposal(&agent).expect("agent origin must be stamped");
        assert_eq!(attr.origin, "Claude Desktop");
        assert_eq!(attr.proposal_id, agent.id);
        assert_eq!(attr.summary, "Save query: NGC 224");
        assert!(!attr.applied_at.is_empty());
    }

    #[test]
    fn descriptors_are_all_write_and_agent_safe() {
        for d in descriptors() {
            assert_eq!(d.verb, VerbClass::Write);
            assert!(d.agent_safe, "{} must be agent_safe", d.name);
        }
    }
}
