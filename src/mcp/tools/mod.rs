//! Tool abstractions: the boundary between the MCP server (transport/dispatch)
//! and the concrete tool catalog. Ported from `Mcp/Tools/*` + `Mcp/McpToolRouter.cs`.
//!
//! The server holds an `Arc<dyn ToolRouter>` and only knows this interface, so the
//! wire layer is testable with a `NullRouter` while the real catalog binds to
//! `AppServices` separately.

use serde_json::Value;
use std::future::Future;
use std::pin::Pin;

pub mod ai_compute;
pub mod aiguide_ext;
pub mod aliases;
pub mod caom2_vizier;
pub mod catalog;
pub mod cube;
pub mod fits;
pub mod imagediscovery;
pub mod notebook;
pub mod proposals;
pub mod read;
pub mod research;
pub mod router;
pub mod search_ui;
pub mod sessions;
pub mod viewstate;
pub mod vospace;
pub mod workflows;
pub mod write;

// ─────────────────────────────────────────────────────────────────────────────
// Shared argument accessors
//
// Tool schemas declare arguments in camelCase, matching the reference (whose
// serializer is configured `PropertyNamingPolicy = CamelCase`). The reference
// also deserializes case-insensitively, so an agent that sends `publisher_id`
// where the schema says `publisherId` still works there — and must here too.
// These helpers therefore try the key as given, then its snake_case spelling.
// ─────────────────────────────────────────────────────────────────────────────

/// `some_key` → `someKey`. Leaves an already-camelCase key untouched.
fn camel_case(key: &str) -> String {
    let mut out = String::with_capacity(key.len());
    let mut upper_next = false;
    for c in key.chars() {
        if c == '_' {
            upper_next = true;
        } else if upper_next {
            out.extend(c.to_uppercase());
            upper_next = false;
        } else {
            out.push(c);
        }
    }
    out
}

/// `someKey` → `some_key`. Leaves an already-snake_case key untouched.
fn snake_case(key: &str) -> String {
    let mut out = String::with_capacity(key.len() + 4);
    for c in key.chars() {
        if c.is_ascii_uppercase() {
            out.push('_');
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// Look up `key` in an arguments object, accepting either naming style.
///
/// Tools ask for the canonical (camelCase) name; a caller that sent snake_case
/// still resolves. Returns `None` when neither spelling is present.
pub fn arg<'a>(args: &'a Value, key: &str) -> Option<&'a Value> {
    if let Some(v) = args.get(key) {
        return Some(v);
    }
    let alt = if key.contains('_') {
        camel_case(key)
    } else {
        snake_case(key)
    };
    args.get(&alt)
}

/// A trimmed string argument, or `""` when absent or not a string.
pub fn str_arg(args: &Value, key: &str) -> String {
    arg(args, key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string()
}

/// A trimmed string argument, or `None` when absent, not a string, or blank.
pub fn opt_str_arg(args: &Value, key: &str) -> Option<String> {
    arg(args, key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// An optional boolean argument (`None` when absent or not a boolean).
pub fn opt_bool(args: &Value, key: &str) -> Option<bool> {
    arg(args, key).and_then(Value::as_bool)
}

/// A boolean argument defaulting to `false`.
pub fn bool_arg(args: &Value, key: &str) -> bool {
    opt_bool(args, key).unwrap_or(false)
}

/// An optional unsigned argument.
pub fn opt_u64(args: &Value, key: &str) -> Option<u64> {
    arg(args, key).and_then(Value::as_u64)
}

/// An optional unsigned argument narrowed to `u32`.
pub fn opt_u32(args: &Value, key: &str) -> Option<u32> {
    opt_u64(args, key).map(|n| n as u32)
}

/// A numeric argument, accepting a JSON number OR a numeric string — agents
/// routinely send `"42"` where the schema says `number`.
pub fn num_arg(args: &Value, key: &str) -> Option<f64> {
    let v = arg(args, key)?;
    v.as_f64()
        .or_else(|| v.as_str().and_then(|s| s.trim().parse::<f64>().ok()))
}

/// An optional array-of-strings argument.
///
/// Distinguishes "omitted" (`None`) from "explicitly empty" (`Some(vec![])`) —
/// `set_vospace_acl` relies on that difference to tell "leave this dimension
/// unchanged" from "revoke every group in it".
pub fn opt_str_array(args: &Value, key: &str) -> Option<Vec<String>> {
    arg(args, key).and_then(|v| v.as_array()).map(|arr| {
        arr.iter()
            .filter_map(|item| item.as_str().map(str::to_string))
            .collect()
    })
}

/// Apply an approved proposal by dispatching to the family that owns its `kind`.
/// Each service-backed family exposes `apply(...) -> Option<Result<..>>`; the base
/// write tools handle the rest (and error on a truly unknown kind).
pub async fn apply_any(
    services: &crate::state::AppServices,
    proposal: &proposals::PendingProposal,
) -> Result<String, String> {
    if let Some(r) = vospace::apply(services, proposal).await {
        return r;
    }
    if let Some(r) = research::apply(services, proposal).await {
        return r;
    }
    if let Some(r) = sessions::apply(services, proposal).await {
        return r;
    }
    if let Some(r) = workflows::apply(services, proposal).await {
        return r;
    }
    if let Some(r) = aiguide_ext::apply(services, proposal).await {
        return r;
    }
    if let Some(r) = imagediscovery::apply(services, proposal).await {
        return r;
    }
    if let Some(r) = caom2_vizier::apply(services, proposal).await {
        return r;
    }
    if let Some(r) = ai_compute::apply(services, proposal).await {
        return r;
    }
    if let Some(r) = search_ui::apply(services, proposal).await {
        return r;
    }
    write::apply(services, proposal).await
}

/// All service-backed family descriptors (chained into the router's manifest).
pub fn family_descriptors() -> Vec<ToolDescriptor> {
    let mut v = Vec::new();
    v.extend(vospace::descriptors());
    v.extend(research::descriptors());
    v.extend(sessions::descriptors());
    v.extend(workflows::descriptors());
    v.extend(aiguide_ext::descriptors());
    v.extend(viewstate::descriptors());
    v.extend(cube::descriptors());
    v.extend(notebook::descriptors());
    v.extend(fits::descriptors());
    v.extend(imagediscovery::descriptors());
    v.extend(caom2_vizier::descriptors());
    v.extend(search_ui::descriptors());
    v.extend(ai_compute::descriptors());
    v
}

/// Try each family's dispatch; `None` if no family owns `name`.
pub async fn family_dispatch(
    name: &str,
    services: &crate::state::AppServices,
    args: &serde_json::Value,
    proposals: &std::sync::Arc<proposals::InMemoryProposalStore>,
) -> Option<ToolResult> {
    if let Some(r) = vospace::dispatch(name, services, args, proposals).await {
        return Some(r);
    }
    if let Some(r) = research::dispatch(name, services, args, proposals).await {
        return Some(r);
    }
    if let Some(r) = sessions::dispatch(name, services, args, proposals).await {
        return Some(r);
    }
    if let Some(r) = workflows::dispatch(name, services, args, proposals).await {
        return Some(r);
    }
    if let Some(r) = aiguide_ext::dispatch(name, services, args, proposals).await {
        return Some(r);
    }
    if let Some(r) = viewstate::dispatch(name, services, args, proposals).await {
        return Some(r);
    }
    if let Some(r) = cube::dispatch(name, services, args, proposals).await {
        return Some(r);
    }
    if let Some(r) = notebook::dispatch(name, services, args, proposals).await {
        return Some(r);
    }
    if let Some(r) = fits::dispatch(name, services, args, proposals).await {
        return Some(r);
    }
    if let Some(r) = imagediscovery::dispatch(name, services, args, proposals).await {
        return Some(r);
    }
    if let Some(r) = caom2_vizier::dispatch(name, services, args, proposals).await {
        return Some(r);
    }
    if let Some(r) = search_ui::dispatch(name, services, args, proposals).await {
        return Some(r);
    }
    if let Some(r) = ai_compute::dispatch(name, services, args, proposals).await {
        return Some(r);
    }
    None
}

/// Read tools are agent-safe (no side effects); write tools return proposals and
/// must NEVER auto-apply destructive changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerbClass {
    Read,
    Write,
}

/// A tool's public descriptor as advertised in `tools/list`.
#[derive(Debug, Clone)]
pub struct ToolDescriptor {
    pub name: String,
    pub description: String,
    /// JSON Schema for the tool's arguments.
    pub input_schema: Value,
    pub verb: VerbClass,
    /// Whether an external (agent) caller may invoke this tool directly.
    pub agent_safe: bool,
}

/// Where a tool call originates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperationOrigin {
    /// An external MCP client (an AI agent). Subject to the agent-safe gate.
    External(String),
    /// The app itself (user-initiated). May reach user-only tools.
    Internal,
}

/// Per-call context threaded to a tool.
pub struct ToolContext {
    pub origin: OperationOrigin,
    /// A unique id for this call (audit + proposal correlation).
    pub request_id: String,
    /// Proposal sink for write tools (None for read-only contexts / tests).
    pub proposals: Option<std::sync::Arc<proposals::InMemoryProposalStore>>,
}

impl ToolContext {
    pub fn for_external(client_id: String, request_id: String) -> Self {
        ToolContext {
            origin: OperationOrigin::External(client_id),
            request_id,
            proposals: None,
        }
    }

    pub fn is_external(&self) -> bool {
        matches!(self.origin, OperationOrigin::External(_))
    }

    pub fn client_label(&self) -> &str {
        match &self.origin {
            OperationOrigin::External(c) => c,
            OperationOrigin::Internal => "internal",
        }
    }
}

/// The outcome of a tool call.
pub enum ToolResult {
    /// Structured JSON payload (serialized as text content on the wire).
    Data(Value),
    /// Plain text content.
    Text(String),
    /// A queued write proposal awaiting user approval.
    Proposed(proposals::PendingProposal),
    /// A base64 image with an optional caption.
    Image {
        data_base64: String,
        mime: String,
        caption: Option<String>,
    },
    /// The call failed; `reason` is a human-readable message (maps to isError).
    Failed(String),
}

/// A dyn-compatible async tool router.
pub trait ToolRouter: Send + Sync {
    /// The agent-safe descriptors exposed to external clients via `tools/list`.
    fn external_manifest(&self) -> Vec<ToolDescriptor>;

    /// Dispatch a `tools/call`. Boxed future so the trait stays object-safe.
    fn dispatch<'a>(
        &'a self,
        name: &'a str,
        args: Value,
        ctx: &'a ToolContext,
    ) -> Pin<Box<dyn Future<Output = ToolResult> + Send + 'a>>;
}

/// A router with no tools — used by transport/server unit tests.
pub struct NullRouter;

impl ToolRouter for NullRouter {
    fn external_manifest(&self) -> Vec<ToolDescriptor> {
        Vec::new()
    }

    fn dispatch<'a>(
        &'a self,
        name: &'a str,
        _args: Value,
        _ctx: &'a ToolContext,
    ) -> Pin<Box<dyn Future<Output = ToolResult> + Send + 'a>> {
        let name = name.to_string();
        Box::pin(async move { ToolResult::Failed(format!("no such tool: {}", name)) })
    }
}

#[cfg(test)]
mod arg_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn case_conversions_are_idempotent_and_inverse() {
        assert_eq!(camel_case("publisher_id"), "publisherId");
        assert_eq!(camel_case("max_bytes"), "maxBytes");
        assert_eq!(camel_case("a_b_c"), "aBC");
        // Already camelCase: unchanged.
        assert_eq!(camel_case("publisherId"), "publisherId");

        assert_eq!(snake_case("publisherId"), "publisher_id");
        assert_eq!(snake_case("maxBytes"), "max_bytes");
        // Already snake_case: unchanged.
        assert_eq!(snake_case("publisher_id"), "publisher_id");
    }

    /// The reference deserializes arguments case-insensitively, so an agent may
    /// send either spelling regardless of what the schema declares. Both must
    /// resolve, or a prompt written against one app breaks on the other.
    #[test]
    fn either_naming_style_resolves() {
        let camel = json!({ "publisherId": "ivo://x?1", "maxBytes": 10 });
        // Built key-by-key so a bulk casing pass over this file can never
        // silently turn this fixture into a second camelCase case.
        let mut snake = serde_json::Map::new();
        snake.insert("publisher".to_string() + "_id", json!("ivo://x?1"));
        snake.insert("max".to_string() + "_bytes", json!(10));
        let snake = Value::Object(snake);

        for args in [&camel, &snake] {
            assert_eq!(str_arg(args, "publisherId"), "ivo://x?1");
            assert_eq!(str_arg(args, "publisher_id"), "ivo://x?1");
            assert_eq!(arg(args, "maxBytes").and_then(|v| v.as_u64()), Some(10));
            assert_eq!(arg(args, "max_bytes").and_then(|v| v.as_u64()), Some(10));
        }
    }

    #[test]
    fn absent_and_wrong_type_arguments_are_empty_not_panics() {
        let args = json!({ "count": 3 });
        assert_eq!(str_arg(&args, "missing"), "");
        // Present but not a string.
        assert_eq!(str_arg(&args, "count"), "");
        assert!(arg(&args, "missing").is_none());
    }

    /// Viewer hosts must read tool arguments through [`arg`], never `args.get`.
    ///
    /// This is a source-level check because the failure it catches is invisible
    /// at runtime AND untestable at the unit level: the hosts need a live GTK
    /// widget tree. When tool schemas moved to camelCase, three hosts kept
    /// reading `min_cut` / `north_up` / `center_x` directly, so every one of
    /// those parameters was silently ignored — the call succeeded and did
    /// nothing. `arg` accepts either spelling, so routing through it makes the
    /// mismatch impossible rather than merely fixed.
    #[test]
    fn viewer_hosts_read_arguments_through_the_shared_accessor() {
        let ui = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui");
        let mut offenders: Vec<String> = Vec::new();
        let mut checked = 0usize;

        let mut stack = vec![ui];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue;
                };
                // Only files that actually serve bridge commands.
                if !text.contains("handle_viewer_command") {
                    continue;
                }
                checked += 1;
                if text.contains("args.get(") {
                    offenders.push(path.display().to_string());
                }
            }
        }

        assert!(
            checked > 0,
            "found no viewer hosts to check — did src/ui move?"
        );
        assert!(
            offenders.is_empty(),
            "viewer host(s) read tool arguments with `args.get(...)`, which only \
             matches one spelling; use `crate::mcp::tools::arg(args, ..)`: {offenders:?}"
        );
    }

    #[test]
    fn string_arguments_are_trimmed() {
        let args = json!({ "path": "  data/run1  " });
        assert_eq!(str_arg(&args, "path"), "data/run1");
    }
}
