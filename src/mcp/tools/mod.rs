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
pub mod sessions;
pub mod viewstate;
pub mod vospace;
pub mod workflows;
pub mod write;

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
