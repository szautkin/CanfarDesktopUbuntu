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
    /// An argument the tool does not declare is refused by name.
    ///
    /// Every schema says `additionalProperties: false` and nothing enforced it,
    /// so an invented argument was accepted and ignored. That silence cost a QA
    /// session three misreadings: `get_job_status` with an `executionId`
    /// answered with the whole job list as if called with `{}`,
    /// `set_fits_view` dropped a `tabIndex` — which changed their diagnosis of
    /// an unrelated viewer bug — and `create_analysis_notebook` was reported as
    /// ignoring a `title` it never had.
    ///
    /// Driven through `dispatch`, because the validator being right is not the
    /// same as it being wired in.
    #[test]
    fn an_undeclared_argument_is_refused_rather_than_ignored() {
        use crate::mcp::tools::{ToolContext, ToolResult};

        let rt = tokio::runtime::Runtime::new().expect("build a tokio runtime");
        let (services, _toast_rx) = AppServices::new(rt.handle().clone());
        let (router, _proposals) = build_router(services);

        rt.block_on(async {
            let ctx = ToolContext::for_external("agent".into(), "req-1".into());

            let r = router
                .dispatch(
                    "get_job_status",
                    serde_json::json!({ "executionId": "abc" }),
                    &ctx,
                )
                .await;
            match r {
                ToolResult::Failed(msg) => {
                    assert!(
                        msg.contains("executionId"),
                        "the bad key is not named: {msg}"
                    );
                    assert!(msg.contains("unknown argument"), "{msg}");
                }
                _ => panic!("an invented argument was accepted instead of refused"),
            }

            // And a call with no surprises still runs.
            let ok = router
                .dispatch("get_job_status", serde_json::json!({}), &ctx)
                .await;
            assert!(
                !matches!(ok, ToolResult::Failed(_)),
                "a valid call was refused"
            );
        });
    }

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

    /// The proposal-lifecycle family is how an agent tracks its own writes, so
    /// its payload shape has to be the one the reference documents — a client
    /// polling for `withdrew` or reading `nextToken` gets nothing otherwise.
    #[test]
    fn lifecycle_payloads_match_the_reference_records() {
        use crate::mcp::tools::{ToolContext, ToolResult};

        let rt = tokio::runtime::Runtime::new().expect("build a tokio runtime");
        let (services, _toast_rx) = AppServices::new(rt.handle().clone());
        let (router, _proposals) = build_router(services);

        rt.block_on(async {
            let ctx = ToolContext::for_external("agent-A".into(), "req-1".into());
            let proposal_id = match router
                .dispatch("delete_node", serde_json::json!({ "path": "/a/x" }), &ctx)
                .await
            {
                ToolResult::Proposed(p) => p.id,
                other => panic!("expected a queued proposal, got {}", variant_name(&other)),
            };

            // ── list_pending_proposals → Output(Count, [Item]) ──────────────
            let ToolResult::Data(listed) = router
                .dispatch("list_pending_proposals", serde_json::json!({}), &ctx)
                .await
            else {
                panic!("list_pending_proposals should return Data");
            };
            assert_eq!(listed["count"], 1, "the reference's Output carries a count");
            let item = &listed["proposals"][0];
            for key in [
                "id",
                "toolName",
                "kind",
                "summary",
                "createdAtISO",
                "originTag",
            ] {
                assert!(!item[key].is_null(), "Item.{key} is missing or null");
            }
            // Called through the `delete_node` alias, reported as the canonical
            // name — the reference resolves an alias to its inner tool before
            // stamping `Descriptor.Name`, so both apps name the real tool.
            assert_eq!(item["toolName"], "delete_vospace_node");
            assert_eq!(
                item["originTag"], "agent-A",
                "originTag is the external client's label"
            );

            // ── list_events → Output(Events, NextToken, Expired) ────────────
            let ToolResult::Data(events) = router
                .dispatch("list_events", serde_json::json!({}), &ctx)
                .await
            else {
                panic!("list_events should return Data");
            };
            assert!(events["nextToken"].is_string(), "tokens cross as strings");
            assert_eq!(events["expired"], false);
            let ev = &events["events"][0];
            assert!(
                ev["token"].is_string(),
                "the per-event token is a string too"
            );
            assert_eq!(
                ev["kind"], "proposalArrived",
                "the wire kind is the full name, not a shortened 'arrived'"
            );
            assert_eq!(ev["proposalID"], proposal_id);
            assert_eq!(
                ev["originKind"], "external",
                "an agent-queued proposal arrived externally"
            );
            assert!(!ev["occurredAtISO"].as_str().unwrap_or("").is_empty());

            // Resuming from the returned token yields nothing new — the round
            // trip through a string token has to actually work.
            let next = events["nextToken"].as_str().unwrap().to_string();
            let ToolResult::Data(again) = router
                .dispatch(
                    "list_events",
                    serde_json::json!({ "since_token": next }),
                    &ctx,
                )
                .await
            else {
                panic!("list_events should return Data");
            };
            assert!(
                again["events"].as_array().unwrap().is_empty(),
                "resuming from nextToken must not replay events already seen"
            );

            // ── get_proposal_state → Output(Id, State) ──────────────────────
            let ToolResult::Data(state) = router
                .dispatch(
                    "get_proposal_state",
                    serde_json::json!({ "id": proposal_id }),
                    &ctx,
                )
                .await
            else {
                panic!("get_proposal_state should return Data");
            };
            assert_eq!(state["state"], "pending");
            let keys: Vec<&String> = state.as_object().unwrap().keys().collect();
            assert_eq!(keys.len(), 2, "the reference returns exactly id + state");

            // ── withdraw_proposal → Output(Id, Withdrew) ────────────────────
            let ToolResult::Data(withdrawn) = router
                .dispatch(
                    "withdraw_proposal",
                    serde_json::json!({ "id": proposal_id }),
                    &ctx,
                )
                .await
            else {
                panic!("withdraw_proposal should return Data");
            };
            assert_eq!(
                withdrawn["withdrew"], true,
                "the reference's field is `withdrew`, not `withdrawn`"
            );
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

#[cfg(test)]
mod read_back_tests {
    //! What an agent can SET, it must be able to READ.
    //!
    //! Not pedantry: an agent that changes the user's view is expected to put it
    //! back, and it can only do that if the getter reports what the setter took.
    //! The cube's snapshot carried a comment promising exactly this — "read back
    //! everything set_cube_view can change" — while omitting density, MIP, the
    //! background and auto-orbit.
    //!
    //! Actions are exempt by name: `reset`, `clearCrosshair` and their like do
    //! something rather than hold a value, and there is nothing to read back.

    use super::*;
    use crate::mcp::tools::ToolRouter;

    /// (setter tool, the source that builds the matching getter's payload).
    const PAIRS: &[(&str, &str)] = &[
        ("set_fits_view", include_str!("../../ui/fits_viewer.rs")),
        ("set_cube_view", include_str!("../../ui/cube_tab_host.rs")),
        (
            "set_search_form",
            include_str!("../../ui/search_page/mcp.rs"),
        ),
    ];

    // `set_search_results_view` and `set_search_constraints` are deliberately
    // NOT here, and the reason is worth writing down because adding them looks
    // like an improvement: both report their state, under different names.
    // `page` reads back as `currentPage`, `setFilters` as `filters`,
    // `showColumns`/`hideColumns` as `columns[].visible`, and the seven facets
    // are keys the snapshot builds by iterating `FACETS` rather than literals.
    // A name-matching scan calls all of that missing, and "fixing" it would mean
    // renaming payload keys the reference already fixes.

    /// Arguments that DO something instead of holding a value.
    const ACTIONS: &[&str] = &[
        "reset",
        "resetCamera",
        "clearCrosshair",
        "windowPreset",
        "runSearch",
    ];

    #[test]
    fn every_settable_control_can_be_read_back() {
        let rt = tokio::runtime::Runtime::new().expect("build a tokio runtime");
        let (services, _toast_rx) = AppServices::new(rt.handle().clone());
        let (router, _proposals) = build_router(services);
        let manifest = router.external_manifest();

        let mut missing: Vec<String> = Vec::new();
        for (setter, snapshot_source) in PAIRS {
            let tool = manifest
                .iter()
                .find(|d| d.name == *setter)
                .unwrap_or_else(|| panic!("`{setter}` is declared"));
            let props = tool.input_schema["properties"]
                .as_object()
                .unwrap_or_else(|| panic!("`{setter}` declares properties"));

            for name in props.keys() {
                if ACTIONS.contains(&name.as_str()) {
                    continue;
                }
                // The payload is built with `"name":` literals, so that is what
                // the scan looks for — the snapshot function runs against a live
                // widget no test can build.
                let key = format!("\"{name}\":");
                if !snapshot_source.contains(&key) {
                    missing.push(format!("{setter}.{name}"));
                }
            }
        }

        assert!(
            missing.is_empty(),
            "these controls can be set but never read back, so an agent cannot \
             restore what it changed: {missing:#?}"
        );
    }
}

#[cfg(test)]
mod payload_contract_tests {
    //! A proposal's payload is written by one function and read by another,
    //! usually hundreds of lines apart. Nothing connects them.
    //!
    //! `apply_launch_headless_job` read `replicas` from its payload and no
    //! proposer ever wrote one, so the field was permanently `None` — an entire
    //! capability the launch form offers, the service supports and the reference
    //! advertises, unreachable through the tool that pretends to expose it.

    /// Files where a propose/apply pair lives.
    const PAYLOAD_FILES: &[(&str, &str)] = &[
        ("sessions.rs", include_str!("sessions.rs")),
        ("vospace.rs", include_str!("vospace.rs")),
        ("research.rs", include_str!("research.rs")),
        ("workflows.rs", include_str!("workflows.rs")),
        ("write.rs", include_str!("write.rs")),
        ("ai_compute.rs", include_str!("ai_compute.rs")),
        ("aiguide_ext.rs", include_str!("aiguide_ext.rs")),
        ("imagediscovery.rs", include_str!("imagediscovery.rs")),
        ("search_ui.rs", include_str!("search_ui.rs")),
        ("caom2_vizier.rs", include_str!("caom2_vizier.rs")),
    ];

    /// The file with the schema declarations and the appliers removed — what is
    /// left is the propose side and the helpers it builds payloads with.
    ///
    /// Scoping matters more than it looks: scanning the whole file counts the
    /// tool SCHEMA's own property names as writes, so a payload key nothing
    /// writes is found in the schema and the guard passes. That is exactly how
    /// this test's first version failed to catch the bug it was written for.
    /// Scoping the other way — to `propose_*` bodies alone — was too tight, and
    /// reported three keys written by a shared helper.
    fn without_body_of(source: &str, marker: &str) -> String {
        let mut out = String::new();
        let mut rest = source;
        while let Some(at) = rest.find(marker) {
            out.push_str(&rest[..at]);
            let after = &rest[at..];
            let Some(open) = after.find('{') else { break };
            let mut depth = 0usize;
            let mut end = after.len();
            for (i, c) in after[open..].char_indices() {
                match c {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            end = open + i + 1;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            rest = &after[end..];
        }
        out.push_str(rest);
        out
    }

    /// Everything that can WRITE a payload: the file minus its schemas and minus
    /// its appliers.
    fn write_side(source: &str) -> String {
        let mut out = without_body_of(source, "fn descriptors(");
        while out.contains("fn apply") {
            let next = without_body_of(&out, "fn apply");
            if next == out {
                break;
            }
            out = next;
        }
        out
    }

    /// Keys a file writes into any JSON object, plus `payload["k"] =` stores.
    fn written_keys(source: &str) -> Vec<String> {
        let mut keys: Vec<String> = Vec::new();
        for (at, _) in source.match_indices("\":") {
            // Walk back over the key and its opening quote.
            let head = &source[..at];
            if let Some(open) = head.rfind('"') {
                let name = &head[open + 1..];
                if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                    keys.push(name.to_string());
                }
            }
        }
        // `something["key"] = …` — the payload is not always the binding called
        // `payload`; `download_item` builds one named `item`.
        for (at, _) in source.match_indices("[\"") {
            let rest = &source[at + 2..];
            if let Some(end) = rest.find('"') {
                let name = &rest[..end];
                if rest[end..].starts_with("\"]")
                    && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                {
                    keys.push(name.to_string());
                }
            }
        }
        keys
    }

    /// Keys a file reads back OUT of a proposal payload.
    /// A key an applier reads, and whether the read goes through `arg()`.
    ///
    /// The distinction is not academic. `arg()` bridges camelCase and
    /// snake_case, so `str_arg(payload, "include_notes")` does receive
    /// `includeNotes`; `payload.get("include_notes")` does NOT. Treating both
    /// alike is how this test passed over an export whose includeNotes and
    /// includeSearchHistory options were silently ignored — every bundle carried
    /// notes and history whatever the caller asked for.
    struct PayloadRead {
        key: String,
        bridged: bool,
    }

    fn read_keys(source: &str) -> Vec<PayloadRead> {
        let mut keys = Vec::new();
        // `(payload, "x")` is an arg-family helper; `.get("x")` is a raw map
        // lookup that spells the key exactly.
        for (marker, bridged) in [
            ("payload, \"", true),
            ("p.payload, \"", true),
            ("payload.get(\"", false),
        ] {
            for (at, _) in source.match_indices(marker) {
                let rest = &source[at + marker.len()..];
                if let Some(end) = rest.find('"') {
                    keys.push(PayloadRead {
                        key: rest[..end].to_string(),
                        bridged,
                    });
                }
            }
        }
        keys
    }

    #[test]
    fn every_payload_key_an_applier_reads_is_one_a_proposer_writes() {
        let mut orphans: Vec<String> = Vec::new();
        for (name, source) in PAYLOAD_FILES {
            let source = super::advertised_argument_tests::without_test_modules(source);
            let written = written_keys(&write_side(&source));
            for read in read_keys(&source) {
                let found = written.iter().any(|w| {
                    *w == read.key
                        || (read.bridged
                            && (*w == crate::mcp::tools::camel_case(&read.key)
                                || *w == crate::mcp::tools::snake_case(&read.key)))
                });
                if !found {
                    let how = if read.bridged {
                        ""
                    } else {
                        " (read with .get(), which does NOT bridge camelCase)"
                    };
                    orphans.push(format!("{name}: {}{how}", read.key));
                }
            }
        }

        assert!(
            orphans.is_empty(),
            "these payload keys are read when a proposal is applied but never \
             written when one is made, so the value is always absent: {orphans:#?}"
        );
    }
}

#[cfg(test)]
mod advertised_argument_tests {
    //! Every argument a tool advertises must be an argument something READS.
    //!
    //! This exists because `change_cell_type` advertised `cellType`, required
    //! it, and forbade additional properties — while the applier read only
    //! `cell_type`. A compliant client could not call the tool at all, and no
    //! test noticed, because a schema and the code behind it are checked by
    //! nothing but a reader's memory. `get_fits_view`/`set_fits_view` had split
    //! the same way over `zoomPercent`.
    //!
    //! A source scan, deliberately coarse: it asks only whether the NAME appears
    //! as a string literal in the argument-reading code. That is enough to catch
    //! a name that exists on one side only, which is the whole failure mode.
    //!
    //! What it cannot see: WHICH function reads it. A tool whose proposer reads
    //! the wrong spelling while its applier reads the right one still passes,
    //! because the name is read somewhere in the scanned set. Scoping the scan
    //! per handler would need to know which function belongs to which tool, and
    //! that map is exactly the sort of second copy these tests exist to avoid.

    use super::*;
    use crate::mcp::tools::ToolRouter;

    /// Every file that reads tool arguments — the live viewer-command handlers
    /// and the tool modules themselves.
    const ARGUMENT_READERS: &[&str] = &[
        include_str!("../../ui/notebook_host.rs"),
        include_str!("../../ui/cube_tab_host.rs"),
        include_str!("../../ui/cube_viewer.rs"),
        include_str!("../../ui/fits_viewer.rs"),
        include_str!("../../ui/search_page/mcp.rs"),
        include_str!("../../ui/search_page/mod.rs"),
        include_str!("../../ui/workflows_page.rs"),
        include_str!("viewstate.rs"),
        include_str!("read.rs"),
        include_str!("write.rs"),
        include_str!("vospace.rs"),
        include_str!("sessions.rs"),
        include_str!("research.rs"),
        include_str!("caom2_vizier.rs"),
        include_str!("fits.rs"),
        include_str!("cube.rs"),
        include_str!("notebook.rs"),
        include_str!("workflows.rs"),
        include_str!("search_ui.rs"),
        include_str!("imagediscovery.rs"),
        include_str!("aiguide_ext.rs"),
        include_str!("ai_compute.rs"),
        include_str!("router.rs"),
    ];

    /// The source with every `#[cfg(test)]` module removed.
    ///
    /// Tests name arguments constantly — asserting on them, building payloads
    /// out of them — and every one of those mentions would answer "yes, it is
    /// read" for code that reads nothing.
    pub(super) fn without_test_modules(source: &str) -> String {
        let mut out = String::with_capacity(source.len());
        let mut rest = source;
        while let Some(at) = rest.find("#[cfg(test)]") {
            out.push_str(&rest[..at]);
            let after = &rest[at..];
            // Skip to the module's opening brace, then past its matching close.
            let Some(open) = after.find('{') else { break };
            let mut depth = 0usize;
            let mut end = after.len();
            for (i, c) in after[open..].char_indices() {
                match c {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            end = open + i + 1;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            rest = &after[end..];
        }
        out.push_str(rest);
        out
    }

    /// The source with every `json!(...)` region removed.
    ///
    /// Without this the scan is worthless: a schema declares its own argument
    /// names, so a name that exists ONLY in the schema would find itself and
    /// pass. What is left after stripping is the code that reads arguments and
    /// the code that builds payloads — and an input name appearing only in an
    /// output payload is exactly the split this looks for.
    fn without_json_literals(source: &str) -> String {
        let mut out = String::with_capacity(source.len());
        let mut rest = source;
        while let Some(at) = rest.find("json!(") {
            out.push_str(&rest[..at]);
            let after = &rest[at + "json!(".len()..];
            let mut depth = 1usize;
            let mut end = after.len();
            for (i, c) in after.char_indices() {
                match c {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            end = i + 1;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            rest = &after[end..];
        }
        out.push_str(rest);
        out
    }

    #[test]
    fn every_advertised_argument_is_read_somewhere() {
        let rt = tokio::runtime::Runtime::new().expect("build a tokio runtime");
        let (services, _toast_rx) = AppServices::new(rt.handle().clone());
        let (router, _proposals) = build_router(services);

        let readers: Vec<String> = ARGUMENT_READERS
            .iter()
            .map(|src| without_json_literals(&without_test_modules(src)))
            .collect();

        let mut unread: Vec<String> = Vec::new();
        for tool in router.external_manifest() {
            let Some(props) = tool.input_schema["properties"].as_object() else {
                continue;
            };
            for name in props.keys() {
                // `arg()` bridges the two spellings, so a reader asking for
                // `max_bytes` does receive `maxBytes`. The scan has to model
                // that or it reports ten tools that work perfectly — which is
                // how the first version of this test nearly sent me off to
                // "fix" them.
                //
                // The literal must sit where a LOOKUP puts it — `arg(args, "x")`,
                // `text("x")`, `.get("x")` — so that prose in a tool description
                // naming its own argument does not count as reading it. That is
                // what let the zoomPercent split survive an earlier version of
                // this scan.
                // A lookup that goes through `arg()` accepts either spelling;
                // a raw `.get("x")` accepts only the one written. Conflating
                // them let an export tool advertise `includeNotes` while reading
                // `include_notes` off the map — the documented call was ignored,
                // and this test said the argument was read.
                let read = |src: &String, spelling: &str, bridged_only: bool| {
                    let needle = format!("\"{spelling}\"");
                    src.match_indices(&needle).any(|(at, _)| {
                        let before = src[..at].trim_end();
                        // `(` and `,` cover a direct lookup; `[` covers the
                        // table-driven readers — the seven search facets and the
                        // event-cursor aliases are read by iterating a list, and
                        // a rule that only understood direct calls reported both
                        // as unread.
                        let in_lookup_position =
                            before.ends_with('(') || before.ends_with(',') || before.ends_with('[');
                        if !in_lookup_position {
                            return false;
                        }
                        !bridged_only || !before.ends_with(".get(")
                    })
                };
                let exact = name.clone();
                let bridged = [
                    crate::mcp::tools::camel_case(name),
                    crate::mcp::tools::snake_case(name),
                ];
                let found = readers.iter().any(|src| {
                    read(src, &exact, false)
                        || bridged
                            .iter()
                            .any(|spelling| *spelling != exact && read(src, spelling, true))
                });
                if !found {
                    unread.push(format!("{}.{}", tool.name, name));
                }
            }
        }

        assert!(
            unread.is_empty(),
            "these arguments are advertised but never read — a client sending them \
             is ignored, or rejected by its own schema check: {unread:#?}"
        );
    }
}
