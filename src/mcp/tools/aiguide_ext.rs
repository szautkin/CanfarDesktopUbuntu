//! AI-guide management + a few platform/discovery reads — the "re-tune your own
//! tool surface" family. Ported from `Mcp/Tools/Write/AiGuideWriteTools.cs`,
//! `Mcp/Tools/Read/PlatformReadTools.cs`, and `Mcp/Tools/Read/ImageDiscoveryReadTool.cs`.
//!
//! This module owns a fixed set of tool names and exposes the standard family
//! contract the router chains:
//!  * [`descriptors`] — the tools advertised in `tools/list`.
//!  * [`dispatch`] — run one by name (`None` when the name belongs elsewhere).
//!  * [`apply`] — perform an approved write proposal's real service call.
//!
//! Reads (agent-safe, no side effects):
//!  * `list_guide_tools`   — the user's per-tool description overrides + guide tools.
//!  * `get_platform_load`  — CANFAR Science Platform CPU/RAM headroom + instance counts.
//!  * `list_recent_launches` — the recently launched session presets.
//!  * `list_session_images`  — the container images available to launch.
//!
//! Writes (agent-safe; every write enqueues a proposal, never mutates directly):
//!  * `set_tool_description`   — override a built-in tool's `tools/list` description.
//!  * `clear_tool_description` — revert a tool to its built-in description.
//!  * `add_guide_tool`         — create a read-only user guide tool.
//!  * `delete_guide_tool`      — delete a user guide tool (destructive).
//!
//! The AI-guide appliers invoke the live `AiGuideService`, which the MCP server
//! re-reads on the next `tools/list`, so an approved edit re-tunes the manifest
//! live (matching the Windows reference).

use crate::mcp::tools::proposals::{InMemoryProposalStore, PendingProposal};
use crate::mcp::tools::{ToolDescriptor, ToolResult, VerbClass};
use crate::state::AppServices;
use serde_json::{json, Value};
use std::sync::Arc;

/// Description cap — mirrors `AiGuideService::MAX_DESCRIPTION_CHARS` (600).
const MAX_DESCRIPTION_CHARS: usize = 600;
/// Guide-body cap — mirrors `AiGuideService::MAX_BODY_CHARS` (4000).
const MAX_BODY_CHARS: usize = 4000;

// ─────────────────────────────────────────────────────────────────────────────
// Manifest
// ─────────────────────────────────────────────────────────────────────────────

/// A read-tool descriptor with the invariant fields (`Read` / agent-safe) fixed.
fn read_tool(name: &str, description: &str, input_schema: Value) -> ToolDescriptor {
    ToolDescriptor {
        name: name.to_string(),
        description: description.to_string(),
        input_schema,
        verb: VerbClass::Read,
        agent_safe: true,
    }
}

/// A write-tool descriptor with the invariant fields (`Write` / agent-safe) fixed.
fn write_tool(name: &str, description: &str, input_schema: Value) -> ToolDescriptor {
    ToolDescriptor {
        name: name.to_string(),
        description: description.to_string(),
        input_schema,
        verb: VerbClass::Write,
        agent_safe: true,
    }
}

/// The empty-object JSON Schema shared by no-argument tools.
fn empty_schema() -> Value {
    json!({ "type": "object", "properties": {}, "additionalProperties": false })
}

/// All tool descriptors owned by this family (reads first, then writes).
pub fn descriptors() -> Vec<ToolDescriptor> {
    vec![
        // ── Reads ────────────────────────────────────────────────────────────
        read_tool(
            "list_guide_tools",
            "List your AI-guide state: the per-tool description overrides you set via \
             set_tool_description (tool + text), and the user-authored guide tools (name, \
             description, whether they have a body) you can delete_guide_tool by name.",
            empty_schema(),
        ),
        read_tool(
            "get_platform_load",
            "Report the CANFAR Science Platform load: CPU cores + RAM requested vs available, and \
             the number of running session / desktop-app / headless instances. Useful before \
             proposing launch_session to gauge headroom.",
            empty_schema(),
        ),
        read_tool(
            "list_recent_launches",
            "List the user's recently launched session presets (name, session type, image, \
             cores/RAM/GPUs, timestamp) — a shortlist of what they tend to launch.",
            empty_schema(),
        ),
        read_tool(
            "list_session_images",
            "List the container images available to launch on the Science Platform: each image's \
             id and the Skaha session types it supports (notebook/desktop/carta/…). Requires \
             sign-in.",
            empty_schema(),
        ),
        // ── Writes ───────────────────────────────────────────────────────────
        write_tool(
            "set_tool_description",
            "Override the description another MCP tool advertises in tools/list — re-tune how you \
             read it. Pass the exact tool name (from tools/list) and the new description (max 600 \
             chars). Use clear_tool_description to revert. Queues like other writes.",
            json!({
                "type": "object",
                "properties": {
                    "tool": { "type": "string", "description": "Exact tool name (from tools/list) to re-describe." },
                    "description": { "type": "string", "description": "Replacement description (max 600 chars)." }
                },
                "required": ["tool", "description"],
                "additionalProperties": false
            }),
        ),
        write_tool(
            "clear_tool_description",
            "Revert a tool's description to its built-in default, removing any override set via \
             set_tool_description. Pass the exact tool name (from tools/list).",
            json!({
                "type": "object",
                "properties": {
                    "tool": { "type": "string", "description": "Exact tool name whose override to clear." }
                },
                "required": ["tool"],
                "additionalProperties": false
            }),
        ),
        write_tool(
            "add_guide_tool",
            "Create a new read-only \"guide\" tool that you (and the user) can call to get stored \
             instructions. Pass name (slugged to a wire-safe tool name), a one-line description \
             (shown in tools/list, max 600 chars), and an optional body (returned when the tool is \
             called, max 4000 chars). It appears in tools/list after it applies.",
            json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Display name; slugged into a wire-safe tool name." },
                    "description": { "type": "string", "description": "One-line tools/list description (max 600 chars)." },
                    "body": { "type": "string", "description": "Optional instruction text returned on call (max 4000 chars)." }
                },
                "required": ["name", "description"],
                "additionalProperties": false
            }),
        ),
        write_tool(
            "update_guide_tool",
            "Edit an existing user \"guide\" tool (from list_guide_tools): change its one-line \
             description and/or its body, and optionally rename it. Queues for the user to apply; \
             the updated tool re-appears in tools/list.",
            json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Current name of the guide tool to edit (from list_guide_tools)." },
                    "description": { "type": "string", "description": "New one-line tools/list description (max 600 chars)." },
                    "body": { "type": "string", "description": "New instruction text returned on call (max 4000 chars)." },
                    "new_name": { "type": "string", "description": "Optional new display name (rename)." }
                },
                "required": ["name", "description"],
                "additionalProperties": false
            }),
        ),
        write_tool(
            "delete_guide_tool",
            "Delete a user guide tool by name (from list_guide_tools). Destructive — queues for the \
             user to apply.",
            json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Name (or display form) of the guide tool to delete." }
                },
                "required": ["name"],
                "additionalProperties": false
            }),
        ),
    ]
}

// ─────────────────────────────────────────────────────────────────────────────
// Dispatch — reads return Data; writes validate args + enqueue a proposal
// ─────────────────────────────────────────────────────────────────────────────

/// Handle a tool call owned by this family. Returns `Some(..)` when `name` is one
/// of ours (a `Data`/`Proposed` result, or a `Failed` on a bad request), or `None`
/// so the router can fall through to another catalog.
pub async fn dispatch(
    name: &str,
    services: &AppServices,
    args: &Value,
    proposals: &Arc<InMemoryProposalStore>,
) -> Option<ToolResult> {
    let result = match name {
        // Reads.
        "list_guide_tools" => read_list_guide_tools(services),
        "get_platform_load" => read_platform_load(services).await,
        "list_recent_launches" => read_recent_launches(services),
        "list_session_images" => read_session_images(services).await,
        // Writes.
        "set_tool_description" => propose_set_tool_description(args, proposals),
        "clear_tool_description" => propose_clear_tool_description(args, proposals),
        "add_guide_tool" => propose_add_guide_tool(args, proposals),
        "update_guide_tool" => propose_update_guide_tool(args, proposals),
        "delete_guide_tool" => propose_delete_guide_tool(args, proposals),
        _ => return None,
    };
    Some(result)
}

// ── Reads ────────────────────────────────────────────────────────────────────

fn read_list_guide_tools(services: &AppServices) -> ToolResult {
    let snap = services.ai_guide.snapshot();

    let mut overrides: Vec<Value> = snap
        .overrides
        .iter()
        .map(|(tool, desc)| json!({ "tool": tool, "description": desc }))
        .collect();
    // Stable ordering for agents that diff the output.
    overrides.sort_by(|a, b| {
        a["tool"]
            .as_str()
            .unwrap_or("")
            .cmp(b["tool"].as_str().unwrap_or(""))
    });

    let guides: Vec<Value> = snap
        .guides
        .iter()
        .map(|g| {
            json!({
                "name": g.name,
                "description": g.description,
                "hasBody": !g.body.trim().is_empty(),
            })
        })
        .collect();

    let override_count = overrides.len();
    let guide_count = guides.len();
    ToolResult::Data(json!({
        "overrides": overrides,
        "overrideCount": override_count,
        "guides": guides,
        "guideCount": guide_count,
    }))
}

async fn read_platform_load(services: &AppServices) -> ToolResult {
    let token = match services.get_token().await {
        Some(t) => t,
        None => {
            return ToolResult::Failed("not signed in (sign in to CADC/CANFAR first)".to_string())
        }
    };

    match services.platform.get_stats(&token).await {
        Ok(s) => {
            let (requested_cpu, cpu_available, total_cpu) = match &s.cores {
                Some(c) => (c.requested(), c.available(), c.total()),
                None => (0.0, 0.0, 0.0),
            };
            let (requested_ram, ram_available) = match &s.ram {
                Some(r) => (r.requested_ram.clone(), r.ram_available.clone()),
                None => (None, None),
            };
            let inst = s.instances.as_ref();
            ToolResult::Data(json!({
                "requestedCpuCores": requested_cpu,
                "cpuCoresAvailable": cpu_available,
                "totalCpuCores": total_cpu,
                "requestedRam": requested_ram,
                "ramAvailable": ram_available,
                "sessionInstances": inst.and_then(|i| i.session),
                "desktopAppInstances": inst.and_then(|i| i.desktop_app),
                "headlessInstances": inst.and_then(|i| i.headless),
                "totalInstances": inst.and_then(|i| i.total),
            }))
        }
        Err(e) => ToolResult::Failed(format!("platform stats unavailable: {e}")),
    }
}

fn read_recent_launches(services: &AppServices) -> ToolResult {
    let launches = services.recent_launches.load();
    match serde_json::to_value(&launches) {
        Ok(v) => ToolResult::Data(json!({ "count": launches.len(), "launches": v })),
        Err(e) => ToolResult::Failed(format!("could not serialize recent launches: {e}")),
    }
}

async fn read_session_images(services: &AppServices) -> ToolResult {
    let token = match services.get_token().await {
        Some(t) => t,
        None => {
            return ToolResult::Failed("not signed in (sign in to CADC/CANFAR first)".to_string())
        }
    };

    match services.images.get_images(&token).await {
        Ok(images) => {
            // RawImage is deserialize-only, so project the wire shape by hand.
            let items: Vec<Value> = images
                .iter()
                .map(|img| json!({ "id": img.id, "types": img.types }))
                .collect();
            ToolResult::Data(json!({ "count": items.len(), "images": items }))
        }
        Err(e) => ToolResult::Failed(format!("could not list images: {e}")),
    }
}

// ── Writes (propose only — the real mutation happens in `apply`) ──────────────

fn propose_set_tool_description(
    args: &Value,
    proposals: &Arc<InMemoryProposalStore>,
) -> ToolResult {
    let tool = str_arg(args, "tool");
    let description = str_arg(args, "description");
    if tool.is_empty() {
        return ToolResult::Failed("tool is required".to_string());
    }
    if description.is_empty() {
        return ToolResult::Failed(
            "description is required (use clear_tool_description to reset)".to_string(),
        );
    }
    if description.chars().count() > MAX_DESCRIPTION_CHARS {
        return ToolResult::Failed(format!(
            "description exceeds {MAX_DESCRIPTION_CHARS} characters"
        ));
    }
    let payload = json!({ "tool": tool, "description": description });
    let p = proposals.enqueue(
        "set_tool_description",
        &format!("Override the description of {tool}"),
        false,
        payload,
    );
    ToolResult::Proposed(p)
}

fn propose_clear_tool_description(
    args: &Value,
    proposals: &Arc<InMemoryProposalStore>,
) -> ToolResult {
    let tool = str_arg(args, "tool");
    if tool.is_empty() {
        return ToolResult::Failed("tool is required".to_string());
    }
    let payload = json!({ "tool": tool });
    let p = proposals.enqueue(
        "clear_tool_description",
        &format!("Reset {tool} to its built-in description"),
        false,
        payload,
    );
    ToolResult::Proposed(p)
}

fn propose_add_guide_tool(args: &Value, proposals: &Arc<InMemoryProposalStore>) -> ToolResult {
    let name = str_arg(args, "name");
    let description = str_arg(args, "description");
    let body = str_arg(args, "body");
    if name.is_empty() {
        return ToolResult::Failed("name is required".to_string());
    }
    if description.is_empty() {
        return ToolResult::Failed("description is required".to_string());
    }
    if description.chars().count() > MAX_DESCRIPTION_CHARS {
        return ToolResult::Failed(format!(
            "description exceeds {MAX_DESCRIPTION_CHARS} characters"
        ));
    }
    if body.chars().count() > MAX_BODY_CHARS {
        return ToolResult::Failed(format!("body exceeds {MAX_BODY_CHARS} characters"));
    }
    let payload = json!({ "name": name, "description": description, "body": body });
    let p = proposals.enqueue(
        "add_guide_tool",
        &format!("Add guide tool: {name}"),
        false,
        payload,
    );
    ToolResult::Proposed(p)
}

fn propose_update_guide_tool(args: &Value, proposals: &Arc<InMemoryProposalStore>) -> ToolResult {
    let name = str_arg(args, "name");
    let description = str_arg(args, "description");
    let body = str_arg(args, "body");
    let new_name = str_arg(args, "new_name");
    if name.is_empty() {
        return ToolResult::Failed("name is required".to_string());
    }
    if description.is_empty() {
        return ToolResult::Failed("description is required".to_string());
    }
    if description.chars().count() > MAX_DESCRIPTION_CHARS {
        return ToolResult::Failed(format!(
            "description exceeds {MAX_DESCRIPTION_CHARS} characters"
        ));
    }
    if body.chars().count() > MAX_BODY_CHARS {
        return ToolResult::Failed(format!("body exceeds {MAX_BODY_CHARS} characters"));
    }
    let payload = json!({
        "name": name,
        "description": description,
        "body": body,
        "new_name": new_name,
    });
    let p = proposals.enqueue(
        "update_guide_tool",
        &format!("Update guide tool: {name}"),
        false,
        payload,
    );
    ToolResult::Proposed(p)
}

fn propose_delete_guide_tool(args: &Value, proposals: &Arc<InMemoryProposalStore>) -> ToolResult {
    let name = str_arg(args, "name");
    if name.is_empty() {
        return ToolResult::Failed("name is required".to_string());
    }
    let payload = json!({ "name": name });
    let p = proposals.enqueue(
        "delete_guide_tool",
        &format!("Delete guide tool {name}"),
        true,
        payload,
    );
    ToolResult::Proposed(p)
}

// ─────────────────────────────────────────────────────────────────────────────
// Apply — perform an approved proposal's real AiGuideService call
// ─────────────────────────────────────────────────────────────────────────────

/// Execute an approved proposal owned by this family. Returns `Some(Ok(msg))` /
/// `Some(Err(reason))` when `proposal.kind` is one of ours, or `None` so the
/// integrator can try another family. The `AiGuideService` mutations are
/// synchronous and self-persisting; the MCP server re-reads the snapshot on the
/// next `tools/list`, so an applied edit re-tunes the manifest live.
pub async fn apply(
    services: &AppServices,
    proposal: &PendingProposal,
) -> Option<Result<String, String>> {
    let payload = &proposal.payload;
    let result = match proposal.kind.as_str() {
        "set_tool_description" => {
            let tool = str_arg(payload, "tool");
            let description = str_arg(payload, "description");
            if tool.is_empty() {
                Err("set_tool_description payload missing tool".to_string())
            } else if description.is_empty() {
                Err("set_tool_description payload missing description".to_string())
            } else {
                services.ai_guide.set_override(&tool, &description);
                Ok(format!("Overrode the description of '{tool}'"))
            }
        }
        "clear_tool_description" => {
            let tool = str_arg(payload, "tool");
            if tool.is_empty() {
                Err("clear_tool_description payload missing tool".to_string())
            } else {
                services.ai_guide.clear_override(&tool);
                Ok(format!("Reset '{tool}' to its built-in description"))
            }
        }
        "add_guide_tool" => {
            let name = str_arg(payload, "name");
            let description = str_arg(payload, "description");
            let body = str_arg(payload, "body");
            if name.is_empty() {
                Err("add_guide_tool payload missing name".to_string())
            } else {
                // add_guide does the real slug/uniqueness/length validation.
                match services.ai_guide.add_guide(&name, &description, &body) {
                    Ok(()) => Ok(format!("Added guide tool '{name}'")),
                    Err(e) => Err(e),
                }
            }
        }
        "update_guide_tool" => {
            let current = str_arg(payload, "name");
            let description = str_arg(payload, "description");
            let body = str_arg(payload, "body");
            let new_name = {
                let n = str_arg(payload, "new_name");
                if n.is_empty() {
                    current.clone()
                } else {
                    n
                }
            };
            if current.is_empty() {
                Err("update_guide_tool payload missing name".to_string())
            } else {
                // update_guide does the real slug/uniqueness/length validation.
                match services
                    .ai_guide
                    .update_guide(&current, &new_name, &description, &body)
                {
                    Ok(()) => Ok(format!("Updated guide tool '{current}'")),
                    Err(e) => Err(e),
                }
            }
        }
        "delete_guide_tool" => {
            let name = str_arg(payload, "name");
            if name.is_empty() {
                Err("delete_guide_tool payload missing name".to_string())
            } else {
                services.ai_guide.remove_guide(&name);
                Ok(format!("Deleted guide tool '{name}'"))
            }
        }
        _ => return None,
    };
    Some(result)
}

// ─────────────────────────────────────────────────────────────────────────────
// Argument helpers
// ─────────────────────────────────────────────────────────────────────────────

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
        assert!(!ds.is_empty(), "the family must expose at least one tool");
        let mut seen = HashSet::new();
        for d in &ds {
            assert!(!d.name.is_empty(), "a descriptor has an empty name");
            assert!(
                !d.description.is_empty(),
                "{} has an empty description",
                d.name
            );
            assert!(d.agent_safe, "{} must be agent_safe", d.name);
            assert!(
                seen.insert(d.name.clone()),
                "duplicate tool name: {}",
                d.name
            );
        }
    }

    #[test]
    fn writes_flag_destructive_correctly() {
        let store = Arc::new(InMemoryProposalStore::new());

        // Non-destructive writes (create/update).
        for (result, kind) in [
            (
                propose_set_tool_description(
                    &json!({ "tool": "read_file", "description": "x" }),
                    &store,
                ),
                "set_tool_description",
            ),
            (
                propose_clear_tool_description(&json!({ "tool": "read_file" }), &store),
                "clear_tool_description",
            ),
            (
                propose_add_guide_tool(&json!({ "name": "g", "description": "d" }), &store),
                "add_guide_tool",
            ),
        ] {
            match result {
                ToolResult::Proposed(p) => {
                    assert_eq!(p.kind, kind);
                    assert!(!p.destructive, "{kind} must not be destructive");
                }
                _ => panic!("expected Proposed for {kind}"),
            }
        }

        // Destructive delete.
        match propose_delete_guide_tool(&json!({ "name": "g" }), &store) {
            ToolResult::Proposed(p) => {
                assert_eq!(p.kind, "delete_guide_tool");
                assert!(p.destructive, "delete_guide_tool must be destructive");
            }
            _ => panic!("expected Proposed for delete_guide_tool"),
        }
    }

    #[test]
    fn set_tool_description_validates_fields() {
        let store = Arc::new(InMemoryProposalStore::new());
        // Missing description → failure (steer to clear_tool_description).
        assert!(matches!(
            propose_set_tool_description(&json!({ "tool": "x" }), &store),
            ToolResult::Failed(_)
        ));
        // Missing tool → failure.
        assert!(matches!(
            propose_set_tool_description(&json!({ "description": "y" }), &store),
            ToolResult::Failed(_)
        ));
        // Over-long description → failure.
        let long = "a".repeat(MAX_DESCRIPTION_CHARS + 1);
        assert!(matches!(
            propose_set_tool_description(&json!({ "tool": "x", "description": long }), &store),
            ToolResult::Failed(_)
        ));
        assert_eq!(store.pending_count(), 0);
    }

    #[tokio::test]
    async fn apply_returns_none_for_foreign_kind() {
        // A proposal this family does not own must yield None so the integrator
        // can try the next family's applier.
        let (services, _rx) = AppServices::new(tokio::runtime::Handle::current());
        let foreign = PendingProposal {
            id: "prop-1".to_string(),
            kind: "save_query".to_string(),
            summary: "not ours".to_string(),
            destructive: false,
            payload: json!({}),
            state: crate::mcp::tools::proposals::ProposalState::Pending,
            origin: None,
        };
        assert!(apply(&services, &foreign).await.is_none());
    }
}
