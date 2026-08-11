//! The live tool router: the immutable tool table + dispatcher that binds the
//! read/write tool catalogs to the running [`AppServices`]. Ported 1-to-1 from
//! `Mcp/McpToolRouter.cs`.
//!
//! Two invariants it enforces (defence-in-depth — the wire layer already gates on
//! `initialize`, but the router is the last line before a tool runs):
//!  * **External gate** — an external (agent) caller may reach ONLY agent-safe
//!    tools; a call to a known-but-not-agent-safe tool is refused (and never
//!    dispatched), so a write can't be driven straight through by an agent.
//!  * **Manifest scoping** — `tools/list` exposes agent-safe descriptors only.
//!
//! The router itself is stateless and shared (by `Arc`) across every connection;
//! per-call state (origin, request id, the proposal sink) rides in [`ToolContext`].

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::Value;

use super::{read, write, ToolContext, ToolDescriptor, ToolResult, ToolRouter, VerbClass};
use crate::mcp::audit::{payload_hash, AuditRecord, AuditSink, RingAuditSink};
use crate::mcp::budget::ProposalBudget;
use crate::mcp::tools::proposals::InMemoryProposalStore;
use crate::state::AppServices;

/// Immutable dispatcher binding the read + write tool catalogs to the live
/// services. Cheap to clone (`Arc` fields); one instance backs all connections.
pub struct McpToolRouter {
    services: Arc<AppServices>,
    proposals: Arc<InMemoryProposalStore>,
    /// Runaway-loop backstop: caps how many proposals an external agent may have
    /// pending at once.
    budget: ProposalBudget,
    /// PII-safe per-dispatch audit ring (payloads stored only as SHA-256 hashes).
    audit: Arc<RingAuditSink>,
}

impl McpToolRouter {
    pub fn new(services: Arc<AppServices>, proposals: Arc<InMemoryProposalStore>) -> Self {
        McpToolRouter {
            services,
            proposals,
            budget: ProposalBudget::default(),
            audit: Arc::new(RingAuditSink::default()),
        }
    }

    /// The PII-safe audit ring (for diagnostics / a future audit viewer).
    pub fn audit(&self) -> Arc<RingAuditSink> {
        Arc::clone(&self.audit)
    }

    /// The shared proposal store this router enqueues write proposals into. The
    /// host hands the same `Arc` to the review UI / apply path.
    pub fn proposals(&self) -> Arc<InMemoryProposalStore> {
        Arc::clone(&self.proposals)
    }

    /// The FULL descriptor set (read ++ write ++ lifecycle), including
    /// non-agent-safe tools. Used by the external gate to look up a tool's
    /// `agent_safe` flag; the public `external_manifest` filters to agent-safe.
    pub(crate) fn all_descriptors() -> Vec<ToolDescriptor> {
        let canonical = Self::canonical_descriptors();
        // Advertised aliases are real callable names, so they belong here too —
        // both to appear in `tools/list` and to be found by the agent-safe gate.
        // Deprecated aliases are deliberately absent: they still dispatch (see
        // `aliases::canonical`) but must never widen the advertised name set.
        let alias_descriptors = super::aliases::advertised_descriptors(&canonical);
        canonical.into_iter().chain(alias_descriptors).collect()
    }

    /// The descriptors owned by the tool modules themselves — one entry per tool,
    /// under its canonical name, with no aliases mixed in. This is what an alias
    /// resolves *to*, so alias bookkeeping must be checked against this set.
    pub(crate) fn canonical_descriptors() -> Vec<ToolDescriptor> {
        read::descriptors()
            .into_iter()
            .chain(write::descriptors())
            .chain(super::family_descriptors())
            .chain(lifecycle_descriptors())
            .collect()
    }

    /// Handle a proposal-lifecycle tool (they read/manage the router's own store).
    /// Returns `None` when `name` isn't a lifecycle tool.
    ///
    /// External callers are scoped to their OWN proposals/events by origin, so one
    /// agent can never inspect or withdraw another agent's activity; internal (UI)
    /// callers see everything.
    fn handle_lifecycle(&self, name: &str, args: &Value, ctx: &ToolContext) -> Option<ToolResult> {
        use super::proposals::ProposalState;
        // `Some(label)` for an external caller (scope to it); `None` for internal.
        let scope: Option<String> = ctx.is_external().then(|| ctx.client_label().to_string());
        let visible = |origin: &Option<String>| match &scope {
            None => true, // internal sees all
            Some(label) => origin.as_deref() == Some(label.as_str()),
        };
        match name {
            "list_pending_proposals" => {
                let pending: Vec<_> = self
                    .proposals
                    .pending()
                    .into_iter()
                    .filter(|p| visible(&p.origin))
                    .map(|p| {
                        serde_json::json!({
                            "id": p.id,
                            "toolName": p.tool_name,
                            "kind": p.kind,
                            "summary": p.summary,
                            "createdAtISO": p.created_at,
                            // The reference's `Origin.Label`: the client id for an
                            // external caller, "user" for the app itself.
                            "originTag": p.origin.clone().unwrap_or_else(|| "user".into()),
                            // Beyond the reference. Kept because it cannot be
                            // derived from the other fields and decides whether the
                            // proposal can auto-apply — the single most useful thing
                            // to know about a queued write.
                            "destructive": p.destructive,
                        })
                    })
                    .collect();
                Some(ToolResult::Data(
                    serde_json::json!({ "count": pending.len(), "proposals": pending }),
                ))
            }
            "get_proposal_state" => {
                let id = args.get("id").and_then(|v| v.as_str()).unwrap_or("");
                match self.proposals.get(id) {
                    // Don't leak another agent's proposal — report it as unknown.
                    Some(p) if visible(&p.origin) => {
                        let state = match p.state {
                            ProposalState::Pending => "pending",
                            ProposalState::Applying => "applying",
                            ProposalState::Applied => "applied",
                            ProposalState::Rejected => "rejected",
                            ProposalState::Withdrawn => "withdrawn",
                        };
                        // `{id, state}` and nothing else, as the reference does —
                        // including on the unknown path below, so a client never
                        // has to branch on which keys came back.
                        Some(ToolResult::Data(
                            serde_json::json!({ "id": p.id, "state": state }),
                        ))
                    }
                    _ => Some(ToolResult::Data(
                        serde_json::json!({ "id": id, "state": "unknown" }),
                    )),
                }
            }
            "withdraw_proposal" => {
                let id = args.get("id").and_then(|v| v.as_str()).unwrap_or("");
                // Only the owner may withdraw (external callers scoped by origin).
                let owned = self
                    .proposals
                    .get(id)
                    .map(|p| visible(&p.origin))
                    .unwrap_or(false);
                if !owned {
                    return Some(ToolResult::Failed(format!("proposal {id} is not pending")));
                }
                match self.proposals.resolve(id, ProposalState::Withdrawn) {
                    Some(_) => Some(ToolResult::Data(
                        serde_json::json!({ "id": id, "withdrew": true }),
                    )),
                    None => Some(ToolResult::Failed(format!("proposal {id} is not pending"))),
                }
            }
            "list_events" => {
                // Tokens cross the wire as STRINGS, as in the reference: a u64
                // seq near the top of the range does not survive a round trip
                // through a JSON parser that stores numbers as doubles.
                let since = parse_since_token(args);
                let log = self.proposals.events();
                // Loss detection: if the caller's token predates the retained
                // window, events between it and the oldest retained were evicted.
                let expired = since > 0 && log.oldest_seq().map(|o| o > since + 1).unwrap_or(false);
                let (events, next) = log.since(since);
                let events_json: Vec<_> = events
                    .iter()
                    .filter(|e| visible(&e.origin))
                    .map(|e| {
                        serde_json::json!({
                            "token": e.seq.to_string(),
                            "occurredAtISO": e.occurred_at,
                            // Serialized from the enum, whose variants ARE the
                            // wire kinds (`proposalArrived`, …). A hand-written
                            // match here previously emitted a shortened set that
                            // nothing else in the codebase agreed with.
                            "kind": e.kind,
                            "proposalID": e.proposal_id,
                            "proposalKind": e.proposal_kind,
                            "originKind": e.origin_kind(),
                            // Beyond the reference: the one-line summary, so an
                            // agent polling the feed can report what happened
                            // without a second call per event.
                            "summary": e.summary,
                        })
                    })
                    .collect();
                Some(ToolResult::Data(serde_json::json!({
                    "events": events_json,
                    "nextToken": next.to_string(),
                    "expired": expired,
                })))
            }
            _ => None,
        }
    }
}

/// Read the `list_events` resume token.
///
/// The reference declares it as `since_token` (explicitly snake_case, unlike
/// every other argument) and carries it as a string. Absent, blank or malformed
/// means "from the start of the retained buffer" rather than an error — a client
/// that lost its token should re-baseline, not fail. `cursor` is accepted too:
/// Verbinal shipped that name first, and it costs one line to keep working.
fn parse_since_token(args: &Value) -> u64 {
    for key in ["since_token", "cursor"] {
        let Some(v) = super::arg(args, key) else {
            continue;
        };
        if let Some(n) = v.as_u64() {
            return n;
        }
        if let Some(n) = v.as_str().and_then(|s| s.trim().parse::<u64>().ok()) {
            return n;
        }
    }
    0
}

/// Descriptors for the proposal-lifecycle tools (agent-safe: they only inspect /
/// withdraw the agent's own queued proposals).
fn lifecycle_descriptors() -> Vec<ToolDescriptor> {
    let id_schema = serde_json::json!({
        "type": "object",
        "properties": { "id": { "type": "string", "description": "The proposal id" } },
        "required": ["id"], "additionalProperties": false
    });
    let empty = serde_json::json!({"type":"object","properties":{},"additionalProperties":false});
    vec![
        ToolDescriptor {
            name: "list_pending_proposals".into(),
            description: "List the write proposals awaiting user approval.".into(),
            input_schema: empty,
            verb: VerbClass::Read,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "list_events".into(),
            description: "Poll the proposal-lifecycle event feed (proposalArrived / proposalApplied / \
                          proposalRejected / proposalWithdrawn). Pass the `nextToken` from the previous \
                          call to get only newer events; omit it to read from the start of the retained \
                          buffer. `expired` means events were evicted before you polled — you have a gap."
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": { "since_token": { "type": "string", "description": "Return events after this token (omit to read from the start)." } },
                "additionalProperties": false
            }),
            verb: VerbClass::Read,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "get_proposal_state".into(),
            description: "Get the state (pending/applied/rejected/withdrawn/unknown) of a proposal by id.".into(),
            input_schema: id_schema.clone(),
            verb: VerbClass::Read,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "withdraw_proposal".into(),
            description: "Withdraw a still-pending proposal you queued.".into(),
            input_schema: id_schema,
            verb: VerbClass::Write,
            agent_safe: true,
        },
    ]
}

impl ToolRouter for McpToolRouter {
    fn external_manifest(&self) -> Vec<ToolDescriptor> {
        // AI Guide: apply the user's per-tool description overrides live, and
        // append their read-only guide tools — read per request so edits re-tune
        // tools/list without a reconnect.
        let snapshot = self.services.ai_guide.snapshot();
        let mut manifest: Vec<ToolDescriptor> = Self::all_descriptors()
            .into_iter()
            .filter(|d| d.agent_safe)
            .map(|mut d| {
                d.description = snapshot.description_for_tool(&d.name, &d.description);
                d
            })
            .collect();

        // Built-in names win — a guide may never shadow one (would be a duplicate
        // name in tools/list, an MCP-spec violation).
        let builtin: std::collections::HashSet<String> =
            manifest.iter().map(|d| d.name.clone()).collect();
        for g in snapshot.guides {
            if !builtin.contains(&g.name) {
                manifest.push(ToolDescriptor {
                    name: g.name,
                    description: g.description,
                    input_schema: serde_json::json!({
                        "type": "object", "properties": {}, "additionalProperties": false
                    }),
                    verb: super::VerbClass::Read,
                    agent_safe: true,
                });
            }
        }

        // Deterministic ordering (parity with the C# `OrderBy(Name)`), which also
        // makes `tools/list` stable across runs for agents that diff the manifest.
        manifest.sort_by(|a, b| a.name.cmp(&b.name));
        manifest
    }

    fn dispatch<'a>(
        &'a self,
        name: &'a str,
        args: Value,
        ctx: &'a ToolContext,
    ) -> Pin<Box<dyn Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let started = std::time::Instant::now();
            // Resolve alternate spellings once, here, so no tool module has to know
            // an alias exists. `name` is kept as the caller typed it — the audit
            // trail and the "no such tool" message must both echo that back, not a
            // name the caller never used.
            let resolved = super::aliases::canonical(name);
            // Record agent activity so the UI can show a transient "agent working"
            // indicator (external callers only — internal/UI calls aren't "the agent").
            if ctx.is_external() {
                crate::helpers::agent_activity::record(name);
                // Follow-the-agent: navigate the UI to the module this tool acts on.
                if self
                    .services
                    .mcp_follow_activity
                    .load(std::sync::atomic::Ordering::Relaxed)
                {
                    if let Some(module) = crate::mcp::view_state::module_for_tool(resolved) {
                        crate::mcp::view_state::navigate_fire(module);
                    }
                }
            }
            let result: ToolResult = async {
                // AI Guide tools aren't in the router — calling one returns the user's
                // stored instruction text. Built-ins always win, so only short-circuit
                // for a non-built-in name.
                let is_builtin = Self::all_descriptors().iter().any(|d| d.name == resolved);
                if !is_builtin {
                    if let Some(body) = self.services.ai_guide.snapshot().guide_body(name) {
                        return ToolResult::Text(body);
                    }
                }

                // GATE: an external caller may not reach a known non-agent-safe tool.
                // An unknown name falls through to the "no such tool" tail below (so we
                // don't answer "not permitted" for a tool that doesn't exist).
                if ctx.is_external() {
                    let denied = Self::all_descriptors()
                        .iter()
                        .any(|d| d.name == resolved && !d.agent_safe);
                    if denied {
                        return ToolResult::Failed(
                            "tool not permitted for external clients".to_string(),
                        );
                    }
                }

                // Proposal-lifecycle tools operate on the router's own store.
                if let Some(result) = self.handle_lifecycle(resolved, &args, ctx) {
                    return result;
                }

                // Reads first (side-effect-free), then writes (proposal-enqueuing).
                // Each catalog returns `None` when it doesn't own the name.
                if let Some(result) = read::dispatch(resolved, &self.services, &args).await {
                    return result;
                }

                // Service-backed families (vospace/research/sessions/workflows/ai-guide).
                // Their write tools also enqueue proposals, so honour the same
                // auto-apply-non-destructive policy below via a shared handler.
                let mut family_result =
                    super::family_dispatch(resolved, &self.services, &args, &self.proposals).await;
                if family_result.is_none() {
                    family_result =
                        write::dispatch(resolved, &self.services, &args, &self.proposals).await;
                }
                if let Some(result) = family_result {
                    // Stamp the originating client on the freshly-enqueued proposal and
                    // emit its arrival event (scoped to that origin), before any policy.
                    if let ToolResult::Proposed(p) = &result {
                        let origin = ctx.is_external().then(|| ctx.client_label().to_string());
                        // `resolved` is the canonical name, so a proposal made
                        // through an alias still reports the tool it really ran.
                        self.proposals.stamp_source(&p.id, resolved, origin.clone());
                        self.proposals.events().emit(
                            crate::mcp::agent_events::AgentEventKind::ProposalArrived,
                            &p.id,
                            &p.kind,
                            &p.summary,
                            origin.as_deref(),
                        );
                    }
                    // Auto-apply policy: a non-destructive (reversible) proposal is
                    // applied immediately; a DESTRUCTIVE one stays pending for explicit
                    // user review — destructive writes NEVER auto-apply.
                    if let ToolResult::Proposed(p) = &result {
                        let auto_apply_on = self
                            .services
                            .mcp_auto_apply
                            .load(std::sync::atomic::Ordering::Relaxed);
                        if !p.destructive && auto_apply_on {
                            // Atomically claim before applying so the applier runs at most
                            // once (a concurrent UI Apply on the same id can't double-run it).
                            if self.proposals.claim(&p.id).is_some() {
                                match super::apply_any(&self.services, p).await {
                                    Ok(msg) => {
                                        self.proposals.settle(
                                            &p.id,
                                            super::proposals::ProposalState::Applied,
                                        );
                                        return ToolResult::Data(serde_json::json!({
                                            "applied": true,
                                            "proposalId": p.id,
                                            "kind": p.kind,
                                            "result": msg,
                                        }));
                                    }
                                    Err(e) => {
                                        self.proposals.settle(
                                            &p.id,
                                            super::proposals::ProposalState::Rejected,
                                        );
                                        return ToolResult::Failed(format!("apply failed: {e}"));
                                    }
                                }
                            }
                        }
                    }
                    return result;
                }

                ToolResult::Failed(format!("no such tool: {}", name))
            }
            .await;

            // Budget backstop: a destructive proposal from an EXTERNAL agent that
            // pushes the pending queue past the cap is withdrawn immediately, so a
            // runaway loop can't flood the review queue. (Non-destructive proposals
            // auto-apply above and never accumulate.)
            let mut result = result;
            if let ToolResult::Proposed(p) = &result {
                if ctx.is_external() && self.proposals.pending_count() > self.budget.cap() {
                    let id = p.id.clone();
                    self.proposals
                        .resolve(&id, super::proposals::ProposalState::Withdrawn);
                    result = ToolResult::Failed(format!(
                        "proposal budget exhausted (cap {}); apply or reject a pending proposal first",
                        self.budget.cap()
                    ));
                }
            }

            // PII-safe audit record for every dispatch (payload stored as a hash).
            // The verb comes from the resolved tool; the record still names the
            // tool as the caller spelled it.
            let verb = Self::all_descriptors()
                .iter()
                .find(|d| d.name == resolved)
                .map(|d| match d.verb {
                    VerbClass::Read => "read",
                    VerbClass::Write => "write",
                })
                .unwrap_or("unknown");
            let outcome = match &result {
                ToolResult::Data(_) | ToolResult::Text(_) | ToolResult::Image { .. } => "ok",
                ToolResult::Proposed(_) => "proposed",
                ToolResult::Failed(_) => "error",
            };
            self.audit.record(AuditRecord {
                request_id: ctx.request_id.clone(),
                origin: ctx.client_label().to_string(),
                tool: name.to_string(),
                verb: verb.to_string(),
                outcome: outcome.to_string(),
                duration_ms: started.elapsed().as_millis() as u64,
                payload_sha256: payload_hash(&args),
            });

            result
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_resume_token_is_read_from_a_string_as_the_reference_sends_it() {
        assert_eq!(
            parse_since_token(&serde_json::json!({"since_token": "42"})),
            42
        );
    }

    #[test]
    fn a_numeric_token_is_accepted_too() {
        // Not what the reference emits, but an agent that treats the token as a
        // number and echoes it back should not silently restart from zero.
        assert_eq!(
            parse_since_token(&serde_json::json!({"since_token": 42})),
            42
        );
    }

    #[test]
    fn the_original_cursor_argument_still_resumes() {
        assert_eq!(parse_since_token(&serde_json::json!({"cursor": 7})), 7);
        assert_eq!(parse_since_token(&serde_json::json!({"cursor": "7"})), 7);
    }

    #[test]
    fn a_missing_or_unusable_token_reads_from_the_start() {
        // Re-baselining beats erroring: a client that lost its token still gets
        // the retained window, which is exactly what `expired` is there to flag.
        for args in [
            serde_json::json!({}),
            serde_json::json!({"since_token": ""}),
            serde_json::json!({"since_token": "  "}),
            serde_json::json!({"since_token": "not-a-number"}),
            serde_json::json!({"since_token": null}),
            serde_json::json!({"since_token": -1}),
        ] {
            assert_eq!(parse_since_token(&args), 0, "args: {args}");
        }
    }

    #[test]
    fn since_token_wins_over_the_legacy_cursor() {
        // A client sending both is most likely written against the reference and
        // carrying `cursor` by accident; the documented argument decides.
        let args = serde_json::json!({"since_token": "9", "cursor": 3});
        assert_eq!(parse_since_token(&args), 9);
    }
}
