//! VOSpace / ARC storage tool family. Ported from
//! `Mcp/Tools/Read/VoSpaceFitsReadTools.cs` + `Mcp/Tools/Write/VoSpaceWriteTools.cs`.
//!
//! Reads (`get_node`, `read_file`, `get_quota`) are side-effect-free and perform
//! the real service call at dispatch time, returning [`ToolResult::Data`]. Writes
//! (`upload_text`, `create_folder`, `set_acl`, `delete_node`) never touch app
//! state at propose time — they enqueue a [`PendingProposal`] and the real
//! service call happens in [`apply`] after the user (or the non-destructive
//! auto-apply policy) approves it.
//!
//! `path` is always relative to the caller's VOSpace home (`/home/<username>/…`);
//! a leading slash is tolerated and stripped.

use base64::Engine;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::mcp::tools::proposals::{InMemoryProposalStore, PendingProposal};
use crate::mcp::tools::{ToolDescriptor, ToolResult, VerbClass};
use crate::models::vospace_node::NodeType;
use crate::state::AppServices;

/// Hard cap on the number of bytes `read_file` will return inline (1 MiB).
const MAX_READ_BYTES: usize = 1024 * 1024;
/// Default `read_file` slice when the caller doesn't specify `max_bytes` (64 KiB).
const DEFAULT_READ_BYTES: usize = 64 * 1024;
/// Hard cap on the UTF-8 byte size of an `upload_text` blob (1 MiB).
const MAX_UPLOAD_TEXT_BYTES: usize = 1024 * 1024;

// ─────────────────────────────────────────────────────────────────────────────
// Descriptors
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

/// All tool descriptors owned by the VOSpace family.
pub fn descriptors() -> Vec<ToolDescriptor> {
    vec![
        read_tool(
            "get_node",
            "Fetch metadata for one VOSpace/ARC node (file or folder) by its path relative to your \
             storage home: type, size, content-type, last-modified, and sharing (isPublic + the GMS \
             groups granted read/write).",
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "VOSpace path relative to your home, e.g. \"data/run1/out.fits\"."}
                },
                "required": ["path"],
                "additionalProperties": false
            }),
        ),
        read_tool(
            "read_file",
            "Read a bounded slice of a VOSpace/ARC file and return it as utf8 text (default) or \
             base64. Binary files that aren't valid UTF-8 are returned as base64 with a note. \
             Default 65536 bytes, hard cap 1048576.",
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "VOSpace file path relative to your home."},
                    "max_bytes": {"type": "integer", "minimum": 1, "maximum": 1048576, "description": "Max bytes to return (default 65536, hard cap 1048576)."},
                    "encoding": {"type": "string", "enum": ["utf8", "base64"], "description": "How to return the bytes (default utf8)."}
                },
                "required": ["path"],
                "additionalProperties": false
            }),
        ),
        read_tool(
            "get_quota",
            "Report the user's VOSpace/ARC storage quota: total and used bytes (and GB), the \
             usage percentage, and whether usage is in the warning band (>90%).",
            json!({ "type": "object", "properties": {}, "additionalProperties": false }),
        ),
        write_tool(
            "upload_text",
            "Propose writing a text blob (e.g. a script or config) to a VOSpace/ARC file path \
             relative to your home (overwrites if it exists; up to 1 MB). Queues for the user to apply.",
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Destination VOSpace file path relative to your home."},
                    "content": {"type": "string", "description": "The text content to write."},
                    "content_type": {"type": "string", "description": "MIME type (default text/plain)."}
                },
                "required": ["path", "content"],
                "additionalProperties": false
            }),
        ),
        write_tool(
            "create_folder",
            "Propose creating a folder at a VOSpace/ARC path relative to your home. Queues for the \
             user to apply.",
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Full folder path to create, relative to your home, e.g. \"data/run1\"."}
                },
                "required": ["path"],
                "additionalProperties": false
            }),
        ),
        write_tool(
            "set_acl",
            "Propose changing WHO can access a VOSpace/ARC node (its sharing). Each dimension is \
             independent and REPLACES the whole list: OMIT a field to leave it unchanged, pass [] to \
             revoke all groups in that dimension, or pass full GMS group URIs \
             (ivo://cadc.nrc.ca/gms?Group) to set them. is_public toggles world-readability. To ADD \
             or REMOVE one group you must re-send the full desired list (read the current groups from \
             get_node first). Reversible; queues for the user to apply.",
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "VOSpace node path relative to your home."},
                    "is_public": {"type": "boolean", "description": "true = world-readable, false = not public. OMIT to leave unchanged."},
                    "group_read": {"type": "array", "items": {"type": "string"}, "description": "Full GMS group URIs granted READ. OMIT = unchanged; [] = revoke all. REPLACES the whole read list."},
                    "group_write": {"type": "array", "items": {"type": "string"}, "description": "Full GMS group URIs granted WRITE. OMIT = unchanged; [] = revoke all. REPLACES the whole write list."}
                },
                "required": ["path"],
                "additionalProperties": false
            }),
        ),
        write_tool(
            "delete_node",
            "Propose deleting a VOSpace/ARC file or folder by its path relative to your home. Queues \
             for the user to apply (a destructive change).",
            json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "VOSpace node path relative to your home."}
                },
                "required": ["path"],
                "additionalProperties": false
            }),
        ),
    ]
}

// ─────────────────────────────────────────────────────────────────────────────
// Dispatch — reads run now; writes enqueue a proposal
// ─────────────────────────────────────────────────────────────────────────────

/// Run a VOSpace-family tool by name. Returns `Some(..)` when this module owns
/// `name`, or `None` so the router can fall through to another catalog.
pub async fn dispatch(
    name: &str,
    services: &AppServices,
    args: &Value,
    proposals: &Arc<InMemoryProposalStore>,
) -> Option<ToolResult> {
    let result = match name {
        // Reads
        "get_node" => read_get_node(services, args).await,
        "read_file" => read_file(services, args).await,
        "get_quota" => read_get_quota(services).await,
        // Writes
        "upload_text" => propose_upload_text(args, proposals),
        "create_folder" => propose_create_folder(args, proposals),
        "set_acl" => propose_set_acl(args, proposals),
        "delete_node" => propose_delete_node(args, proposals),
        _ => return None,
    };
    Some(result)
}

async fn read_get_node(services: &AppServices, args: &Value) -> ToolResult {
    let (token, username) = match auth(services).await {
        Ok(v) => v,
        Err(e) => return ToolResult::Failed(e),
    };
    let path = norm_path(&str_arg(args, "path"));
    if path.is_empty() {
        return ToolResult::Failed("path is required".to_string());
    }
    match services.vospace.get_node(&token, &username, &path).await {
        Ok(node) => ToolResult::Data(node_to_json(&path, &node)),
        Err(e) => ToolResult::Failed(format!("could not read node {}: {}", path, e)),
    }
}

async fn read_file(services: &AppServices, args: &Value) -> ToolResult {
    let (token, username) = match auth(services).await {
        Ok(v) => v,
        Err(e) => return ToolResult::Failed(e),
    };
    let path = norm_path(&str_arg(args, "path"));
    if path.is_empty() {
        return ToolResult::Failed("path is required".to_string());
    }

    let encoding = {
        let e = str_arg(args, "encoding").to_lowercase();
        if e.is_empty() {
            "utf8".to_string()
        } else if e == "utf8" || e == "base64" {
            e
        } else {
            return ToolResult::Failed("encoding must be 'utf8' or 'base64'".to_string());
        }
    };

    let max_bytes = match opt_u64(args, "max_bytes") {
        Some(0) => return ToolResult::Failed("max_bytes must be a positive integer".to_string()),
        Some(n) => (n as usize).min(MAX_READ_BYTES),
        None => DEFAULT_READ_BYTES,
    };

    let bytes = match services.vospace.download_bytes(&token, &username, &path).await {
        Ok(b) => b,
        Err(e) => return ToolResult::Failed(format!("could not read file {}: {}", path, e)),
    };

    let total = bytes.len();
    let truncated = total > max_bytes;
    let slice = &bytes[..total.min(max_bytes)];

    // base64 explicitly requested, or a utf8 request over bytes that aren't valid
    // UTF-8 (true binary, or a slice that cut a multi-byte char) — fall back so the
    // caller still gets the data rather than an error.
    if encoding == "base64" {
        return ToolResult::Data(json!({
            "path": path,
            "encoding": "base64",
            "content_base64": base64::engine::general_purpose::STANDARD.encode(slice),
            "bytes_returned": slice.len(),
            "total_bytes": total,
            "truncated": truncated,
        }));
    }

    match std::str::from_utf8(slice) {
        Ok(text) => ToolResult::Data(json!({
            "path": path,
            "encoding": "utf8",
            "content": text,
            "bytes_returned": slice.len(),
            "total_bytes": total,
            "truncated": truncated,
        })),
        Err(_) => ToolResult::Data(json!({
            "path": path,
            "encoding": "base64",
            "note": "content is not valid UTF-8; returned as base64",
            "content_base64": base64::engine::general_purpose::STANDARD.encode(slice),
            "bytes_returned": slice.len(),
            "total_bytes": total,
            "truncated": truncated,
        })),
    }
}

async fn read_get_quota(services: &AppServices) -> ToolResult {
    let (token, username) = match auth(services).await {
        Ok(v) => v,
        Err(e) => return ToolResult::Failed(e),
    };
    match services.storage.get_quota(&token, &username).await {
        Ok(q) => ToolResult::Data(json!({
            "quota_bytes": q.quota_bytes,
            "used_bytes": q.used_bytes,
            "quota_gb": q.quota_gb(),
            "used_gb": q.used_gb(),
            "usage_percent": q.usage_percent(),
            "is_warning": q.is_warning(),
            "last_update": q.last_update,
        })),
        Err(e) => ToolResult::Failed(format!("could not read quota: {}", e)),
    }
}

fn propose_upload_text(args: &Value, proposals: &Arc<InMemoryProposalStore>) -> ToolResult {
    let path = norm_path(&str_arg(args, "path"));
    if path.is_empty() {
        return ToolResult::Failed("path is required".to_string());
    }
    let content = args
        .get("content")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    if content.is_empty() {
        return ToolResult::Failed("content is required".to_string());
    }
    if content.len() > MAX_UPLOAD_TEXT_BYTES {
        return ToolResult::Failed(format!(
            "content exceeds the {} KB limit",
            MAX_UPLOAD_TEXT_BYTES / 1024
        ));
    }
    let content_type = str_arg(args, "content_type");

    let mut payload = json!({ "path": path, "content": content });
    if !content_type.is_empty() {
        payload["content_type"] = json!(content_type);
    }
    let p = proposals.enqueue(
        "upload_text",
        &format!("Write {} bytes to {}", content.len(), path),
        false,
        payload,
    );
    ToolResult::Proposed(p)
}

fn propose_create_folder(args: &Value, proposals: &Arc<InMemoryProposalStore>) -> ToolResult {
    let path = norm_path(&str_arg(args, "path"));
    if path.is_empty() {
        return ToolResult::Failed("path is required".to_string());
    }
    let payload = json!({ "path": path });
    let p = proposals.enqueue("create_folder", &format!("Create folder {}", path), false, payload);
    ToolResult::Proposed(p)
}

fn propose_set_acl(args: &Value, proposals: &Arc<InMemoryProposalStore>) -> ToolResult {
    let path = norm_path(&str_arg(args, "path"));
    if path.is_empty() {
        return ToolResult::Failed("path is required".to_string());
    }
    let group_read = opt_str_array(args, "group_read");
    let group_write = opt_str_array(args, "group_write");
    let is_public = opt_bool(args, "is_public");

    // Build an explicit, complete summary of the resulting ACL — this is what the
    // user reviews before applying a change that could expose data.
    let mut parts: Vec<String> = Vec::new();
    if let Some(gr) = &group_read {
        parts.push(if gr.is_empty() {
            "read: revoke all groups".to_string()
        } else {
            format!("read: {}", gr.join(", "))
        });
    }
    if let Some(gw) = &group_write {
        parts.push(if gw.is_empty() {
            "write: revoke all groups".to_string()
        } else {
            format!("write: {}", gw.join(", "))
        });
    }
    if let Some(p) = is_public {
        parts.push(format!("public: {}", if p { "yes (world-readable)" } else { "no" }));
    }
    if parts.is_empty() {
        return ToolResult::Failed(
            "specify at least one of group_read, group_write, or is_public to change".to_string(),
        );
    }

    // Echo only the provided dimensions so apply() preserves the
    // "omit = unchanged" vs "[] = revoke" distinction VOSpace setNode requires.
    let mut payload = json!({ "path": path });
    if let Some(gr) = group_read {
        payload["group_read"] = json!(gr);
    }
    if let Some(gw) = group_write {
        payload["group_write"] = json!(gw);
    }
    if let Some(p) = is_public {
        payload["is_public"] = json!(p);
    }

    let p = proposals.enqueue(
        "set_acl",
        &format!("Set ACL on {} -> {}", path, parts.join("; ")),
        false,
        payload,
    );
    ToolResult::Proposed(p)
}

fn propose_delete_node(args: &Value, proposals: &Arc<InMemoryProposalStore>) -> ToolResult {
    let path = norm_path(&str_arg(args, "path"));
    if path.is_empty() {
        return ToolResult::Failed("path is required".to_string());
    }
    let payload = json!({ "path": path });
    let p = proposals.enqueue("delete_node", &format!("Delete {}", path), true, payload);
    ToolResult::Proposed(p)
}

// ─────────────────────────────────────────────────────────────────────────────
// Apply — decode an approved proposal's payload + perform the real service call
// ─────────────────────────────────────────────────────────────────────────────

/// Execute an approved VOSpace-family proposal. Returns `Some(Ok(msg))` /
/// `Some(Err(e))` when this module owns `proposal.kind`, or `None` otherwise.
pub async fn apply(
    services: &AppServices,
    proposal: &PendingProposal,
) -> Option<Result<String, String>> {
    let payload = &proposal.payload;
    let out = match proposal.kind.as_str() {
        "upload_text" => {
            let (token, username) = match auth(services).await {
                Ok(v) => v,
                Err(e) => return Some(Err(e)),
            };
            let path = norm_path(&str_arg(payload, "path"));
            if path.is_empty() {
                return Some(Err("upload_text payload missing path".to_string()));
            }
            let content = payload
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let content_type = {
                let c = str_arg(payload, "content_type");
                if c.is_empty() {
                    "text/plain".to_string()
                } else {
                    c
                }
            };
            let bytes = content.into_bytes();
            let n = bytes.len();
            match services
                .vospace
                .upload_file(&token, &username, &path, bytes, &content_type)
                .await
            {
                Ok(()) => Ok(format!("Wrote {} bytes to {}", n, path)),
                Err(e) => Err(format!("upload failed: {}", e)),
            }
        }
        "create_folder" => {
            let (token, username) = match auth(services).await {
                Ok(v) => v,
                Err(e) => return Some(Err(e)),
            };
            let path = norm_path(&str_arg(payload, "path"));
            if path.is_empty() {
                return Some(Err("create_folder payload missing path".to_string()));
            }
            match services.vospace.create_folder(&token, &username, &path).await {
                Ok(()) => Ok(format!("Created folder {}", path)),
                Err(e) => Err(format!("create folder failed: {}", e)),
            }
        }
        "set_acl" => {
            let (token, username) = match auth(services).await {
                Ok(v) => v,
                Err(e) => return Some(Err(e)),
            };
            let path = norm_path(&str_arg(payload, "path"));
            if path.is_empty() {
                return Some(Err("set_acl payload missing path".to_string()));
            }
            // setNode merges by property and needs the node's existing type echoed
            // back, so read the node first to learn it.
            let node = match services.vospace.get_node(&token, &username, &path).await {
                Ok(n) => n,
                Err(e) => return Some(Err(format!("could not read node {}: {}", path, e))),
            };
            let group_read = opt_str_array(payload, "group_read");
            let group_write = opt_str_array(payload, "group_write");
            let is_public = opt_bool(payload, "is_public");
            match services
                .vospace
                .set_node_acl(
                    &token,
                    &username,
                    &path,
                    &node.node_type,
                    group_read,
                    group_write,
                    is_public,
                )
                .await
            {
                Ok(()) => Ok(format!("Updated ACL on {}", path)),
                Err(e) => Err(format!("set ACL failed: {}", e)),
            }
        }
        "delete_node" => {
            let (token, username) = match auth(services).await {
                Ok(v) => v,
                Err(e) => return Some(Err(e)),
            };
            let path = norm_path(&str_arg(payload, "path"));
            if path.is_empty() {
                return Some(Err("delete_node payload missing path".to_string()));
            }
            match services.vospace.delete_node(&token, &username, &path).await {
                Ok(()) => Ok(format!("Deleted {}", path)),
                Err(e) => Err(format!("delete failed: {}", e)),
            }
        }
        _ => return None,
    };
    Some(out)
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Fetch the current token + username, or a "not signed in" error string.
async fn auth(services: &AppServices) -> Result<(String, String), String> {
    let token = services.get_token().await.ok_or("not signed in")?;
    let username = services.get_username().await.ok_or("not signed in")?;
    Ok((token, username))
}

/// A trimmed string arg (or empty string when absent / not a string).
fn str_arg(args: &Value, key: &str) -> String {
    args.get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// Normalize a VOSpace path: trim whitespace and a leading slash so the caller
/// can pass either "data/x" or "/data/x" (both are relative to their home).
fn norm_path(s: &str) -> String {
    s.trim().trim_start_matches('/').to_string()
}

fn opt_bool(args: &Value, key: &str) -> Option<bool> {
    args.get(key).and_then(|v| v.as_bool())
}

fn opt_u64(args: &Value, key: &str) -> Option<u64> {
    args.get(key).and_then(|v| v.as_u64())
}

/// An array-of-strings arg: `Some(list)` when the key is present as a JSON array
/// (possibly empty), or `None` when the key is absent — preserving the
/// "omit = unchanged" vs "[] = revoke" distinction for ACL dimensions.
fn opt_str_array(args: &Value, key: &str) -> Option<Vec<String>> {
    args.get(key).and_then(|v| v.as_array()).map(|arr| {
        arr.iter()
            .filter_map(|item| item.as_str().map(|s| s.to_string()))
            .collect()
    })
}

/// Type tag string for a node's [`NodeType`].
fn type_str(t: &NodeType) -> &'static str {
    match t {
        NodeType::Container => "container",
        NodeType::Data => "data",
        NodeType::Link => "link",
    }
}

/// Serialize a node into the compact MCP shape (including its ACL/sharing).
fn node_to_json(path: &str, node: &crate::models::VoSpaceNode) -> Value {
    json!({
        "path": path,
        "name": node.name,
        "uri": node.uri,
        "type": type_str(&node.node_type),
        "size_bytes": node.size,
        "size_display": node.size_display(),
        "content_type": node.content_type,
        "date": node.date,
        "is_public": node.is_public,
        "group_read": node.group_read,
        "group_write": node.group_write,
    })
}

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
            assert!(!d.name.is_empty(), "a descriptor has an empty name");
            assert!(
                !d.description.is_empty(),
                "descriptor {} has an empty description",
                d.name
            );
            assert!(seen.insert(d.name.clone()), "duplicate descriptor name: {}", d.name);
        }
    }

    #[test]
    fn reads_are_read_writes_are_write() {
        for d in descriptors() {
            let expected = match d.name.as_str() {
                "get_node" | "read_file" | "get_quota" => VerbClass::Read,
                _ => VerbClass::Write,
            };
            assert_eq!(d.verb, expected, "wrong verb class for {}", d.name);
            assert!(d.agent_safe, "{} should be agent_safe", d.name);
        }
    }

    #[test]
    fn norm_path_strips_leading_slash() {
        assert_eq!(norm_path("/data/x"), "data/x");
        assert_eq!(norm_path("  data/x  "), "data/x");
        assert_eq!(norm_path("data/x"), "data/x");
    }

    #[test]
    fn opt_str_array_distinguishes_omit_from_empty() {
        let args = json!({ "group_read": [], "group_write": ["ivo://g"] });
        assert_eq!(opt_str_array(&args, "group_read"), Some(vec![]));
        assert_eq!(
            opt_str_array(&args, "group_write"),
            Some(vec!["ivo://g".to_string()])
        );
        assert_eq!(opt_str_array(&args, "is_public"), None);
    }
}
