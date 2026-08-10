//! Catalog assembly: the one place the router is wired to the live services and a
//! fresh shared proposal store. Ported from `Mcp/McpToolCatalog.cs` (`Build`) — in
//! Rust the read/write tool tables are static module functions rather than a list
//! of injected tool objects, so this reduces to constructing the store + router.

use std::sync::Arc;

use super::router::McpToolRouter;
use crate::mcp::tools::proposals::InMemoryProposalStore;
use crate::state::AppServices;

/// Build the live router bound to `services`, plus the shared proposal store it
/// enqueues write proposals into. Both are returned so the host can hand the same
/// store to the review UI / apply path (the router holds its own clone).
pub fn build_router(
    services: Arc<AppServices>,
) -> (Arc<McpToolRouter>, Arc<InMemoryProposalStore>) {
    let proposals = Arc::new(InMemoryProposalStore::new());
    let router = Arc::new(McpToolRouter::new(services, Arc::clone(&proposals)));
    (router, proposals)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::tools::ToolRouter;
    use std::collections::HashSet;

    /// The external manifest must expose unique tool names and NOTHING that isn't
    /// agent-safe (an external agent's whole visible surface). A duplicate name
    /// would make `tools/call` ambiguous; a non-agent-safe leak would breach the
    /// security invariant.
    #[test]
    fn external_manifest_is_unique_and_agent_safe() {
        let rt = tokio::runtime::Runtime::new().expect("build a tokio runtime");
        let (services, _toast_rx) = AppServices::new(rt.handle().clone());

        let (router, _proposals) = build_router(services);
        let manifest = router.external_manifest();

        assert!(
            !manifest.is_empty(),
            "the external manifest should expose at least the read tools"
        );

        let mut seen = HashSet::new();
        for descriptor in &manifest {
            assert!(
                descriptor.agent_safe,
                "manifest tool '{}' is not agent_safe but was exposed externally",
                descriptor.name
            );
            assert!(
                seen.insert(descriptor.name.clone()),
                "duplicate tool name in external manifest: '{}'",
                descriptor.name
            );
        }

        // Parity guard: tools added for Windows-reference parity must actually be
        // wired into the live manifest an agent sees over tools/list.
        for expected in [
            "get_observation_notes",
            "update_observation_note",
            "bulk_update_observation_notes",
            "list_notebooks",
            "use_workflow",
            "update_guide_tool",
            "upload_file_to_vospace",
            "download_vospace_file",
        ] {
            assert!(
                seen.contains(expected),
                "expected parity tool '{expected}' missing from the external manifest"
            );
        }
    }

    /// One agent must never see another agent's queued proposals: lifecycle reads
    /// are scoped by the originating client (defence-in-depth against a second
    /// external client on the shared socket enumerating the first's activity).
    #[test]
    fn lifecycle_reads_are_origin_scoped_per_agent() {
        use crate::mcp::tools::{ToolContext, ToolResult};

        let rt = tokio::runtime::Runtime::new().expect("build a tokio runtime");
        let (services, _toast_rx) = AppServices::new(rt.handle().clone());
        let (router, _proposals) = build_router(services);

        rt.block_on(async {
            // Agent A enqueues a destructive proposal (stays pending for review).
            let ctx_a = ToolContext::for_external("agent-A".into(), "req-1".into());
            let r = router
                .dispatch("delete_node", serde_json::json!({ "path": "/a/x" }), &ctx_a)
                .await;
            assert!(
                matches!(r, ToolResult::Proposed(_)),
                "expected a queued proposal"
            );

            // Agent B (a different external client) sees NONE of A's proposals.
            let ctx_b = ToolContext::for_external("agent-B".into(), "req-2".into());
            let list_b = router
                .dispatch("list_pending_proposals", serde_json::json!({}), &ctx_b)
                .await;
            if let ToolResult::Data(v) = list_b {
                assert_eq!(
                    v["proposals"].as_array().unwrap().len(),
                    0,
                    "agent B leaked A's proposals"
                );
            } else {
                panic!("list_pending_proposals should return Data");
            }

            // Agent A sees its own.
            let list_a = router
                .dispatch("list_pending_proposals", serde_json::json!({}), &ctx_a)
                .await;
            if let ToolResult::Data(v) = list_a {
                assert_eq!(
                    v["proposals"].as_array().unwrap().len(),
                    1,
                    "agent A should see its own proposal"
                );
            } else {
                panic!("list_pending_proposals should return Data");
            }
        });
    }

    /// Name a [`ToolResult`] variant for assertion messages. `ToolResult` has no
    /// `Debug` on purpose — the `Image` variant carries base64 payload bytes that
    /// should never land in a log or panic message.
    fn variant_name(r: &crate::mcp::tools::ToolResult) -> &'static str {
        use crate::mcp::tools::ToolResult;
        match r {
            ToolResult::Data(_) => "Data",
            ToolResult::Text(_) => "Text",
            ToolResult::Image { .. } => "Image",
            ToolResult::Proposed(_) => "Proposed",
            ToolResult::Failed(_) => "Failed",
        }
    }

    /// A deprecated alias must reach the same tool as its canonical name, and the
    /// resulting proposal must be recorded under the CANONICAL kind — the applier
    /// matches on that, so an alias-named kind would enqueue a proposal nothing
    /// could ever apply.
    #[test]
    fn a_deprecated_alias_dispatches_to_its_canonical_tool() {
        use crate::mcp::tools::{ToolContext, ToolResult};

        let rt = tokio::runtime::Runtime::new().expect("build a tokio runtime");
        let (services, _toast_rx) = AppServices::new(rt.handle().clone());
        let (router, _proposals) = build_router(services);

        rt.block_on(async {
            let ctx = ToolContext::for_external("agent".into(), "req".into());
            let args = serde_json::json!({ "path": "/a/x" });

            let via_alias = router.dispatch("delete_node", args.clone(), &ctx).await;
            let via_canonical = router
                .dispatch("delete_vospace_node", args.clone(), &ctx)
                .await;

            for (label, result) in [("alias", via_alias), ("canonical", via_canonical)] {
                match result {
                    ToolResult::Proposed(p) => assert_eq!(
                        p.kind, "delete_vospace_node",
                        "{label} call recorded the wrong proposal kind"
                    ),
                    other => panic!(
                        "{label} call should queue a proposal, got {}",
                        variant_name(&other)
                    ),
                }
            }

            // An unknown name still reports the name the caller actually used.
            let unknown = router
                .dispatch("no_such_tool_at_all", serde_json::json!({}), &ctx)
                .await;
            match unknown {
                ToolResult::Failed(msg) => assert!(
                    msg.contains("no_such_tool_at_all"),
                    "error should echo the requested name, got: {msg}"
                ),
                other => panic!("expected a failure, got {}", variant_name(&other)),
            }
        });
    }
}
