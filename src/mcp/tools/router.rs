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
use crate::mcp::audit::{payload_hash, AuditRecord, AuditSink, LoggingAuditSink};
use crate::mcp::tools::proposals::InMemoryProposalStore;
use crate::state::AppServices;

/// Immutable dispatcher binding the read + write tool catalogs to the live
/// services. Cheap to clone (`Arc` fields); one instance backs all connections.
pub struct McpToolRouter {
    services: Arc<AppServices>,
    proposals: Arc<InMemoryProposalStore>,
    /// PII-safe per-dispatch audit log (payloads appear only as SHA-256 hashes).
    audit: Arc<LoggingAuditSink>,
}

impl McpToolRouter {
    /// Run an apply as a background job, on the runtime rather than in the
    /// request.
    ///
    /// The registry is keyed by the proposal id, so the caller needs no second
    /// identifier and the applier can report progress against the id it was
    /// already given.
    fn start_background_apply(&self, claimed: super::proposals::PendingProposal) {
        self.services
            .jobs
            .start(&claimed.id, &claimed.kind, &claimed.summary);

        let services = Arc::clone(&self.services);
        let proposals = Arc::clone(&self.proposals);
        tokio::spawn(async move {
            let outcome = super::apply_any(&services, &claimed).await;
            // Settle the proposal exactly as the synchronous path does, so the
            // review UI and `list_proposals` do not leave it pending forever.
            proposals.settle(
                &claimed.id,
                match outcome {
                    Ok(_) => super::proposals::ProposalState::Applied,
                    Err(_) => super::proposals::ProposalState::Rejected,
                },
            );
            services.jobs.finish(&claimed.id, outcome);
        });
    }

    pub fn new(services: Arc<AppServices>, proposals: Arc<InMemoryProposalStore>) -> Self {
        McpToolRouter {
            services,
            proposals,
            audit: Arc::new(LoggingAuditSink),
        }
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
                let Some(id) = proposal_id(args) else {
                    // Distinct from "unknown": nothing was asked about. A
                    // missing argument used to read as a missing proposal.
                    return Some(ToolResult::Failed(
                        "id (or proposalId) is required".to_string(),
                    ));
                };
                let id = id.as_str();
                match self.proposals.get(id) {
                    // Don't leak another agent's proposal — report it as unknown.
                    Some(p) if visible(&p.origin) => {
                        let state = match p.state {
                            ProposalState::Pending => "pending",
                            ProposalState::Applying => "applying",
                            ProposalState::Applied => "applied",
                            ProposalState::Rejected => "rejected",
                            ProposalState::Failed => "failed",
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
                let Some(id) = proposal_id(args) else {
                    return Some(ToolResult::Failed(
                        "id (or proposalId) is required".to_string(),
                    ));
                };
                let id = id.as_str();
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

/// Argument keys a tool does not declare.
///
/// The schemas all say `additionalProperties: false` and nothing enforced it,
/// so a misspelled or invented argument was accepted and ignored. Three
/// separate misreadings in one QA session came from that: `get_job_status`
/// with an `executionId` returned the whole job list as if called with `{}`,
/// `set_fits_view` silently ignored a `tabIndex` (which changed the reporter's
/// diagnosis of an unrelated bug), and `create_analysis_notebook` was thought
/// to ignore a `title` it never had.
///
/// Both spellings of a declared name are accepted, because `arg` bridges
/// camelCase and snake_case at read time — rejecting the spelling the tool
/// itself would have honoured would break callers that work today.
fn undeclared_arguments(schema: &Value, args: &Value) -> Vec<String> {
    if schema.get("additionalProperties") != Some(&Value::Bool(false)) {
        return Vec::new();
    }
    let Some(props) = schema.get("properties").and_then(|p| p.as_object()) else {
        return Vec::new();
    };
    let Some(given) = args.as_object() else {
        return Vec::new();
    };

    let mut declared: std::collections::HashSet<String> = std::collections::HashSet::new();
    for name in props.keys() {
        declared.insert(name.clone());
        declared.insert(super::camel_case(name));
        declared.insert(super::snake_case(name));
    }

    let mut unknown: Vec<String> = given
        .keys()
        .filter(|k| !declared.contains(k.as_str()))
        .cloned()
        .collect();
    unknown.sort();
    unknown
}

/// The proposal id from a lifecycle call, under either spelling.
///
/// Queueing answers `proposalId`; these tools asked for `id`. An agent passing
/// back the key it was just handed got `{"id": "", "state": "unknown"}` — the
/// same answer as for a proposal that never existed, so a polling loop could
/// not tell "waiting for approval" from "gone". `arg` bridges case, not two
/// different words.
fn proposal_id(args: &Value) -> Option<String> {
    ["id", "proposalId"]
        .iter()
        .find_map(|k| super::arg(args, k).and_then(|v| v.as_str()))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Descriptors for the proposal-lifecycle tools (agent-safe: they only inspect /
/// withdraw the agent's own queued proposals).
fn lifecycle_descriptors() -> Vec<ToolDescriptor> {
    let id_schema = serde_json::json!({
        "type": "object",
        "properties": {
            "id": { "type": "string", "description": "The proposal id" },
            "proposalId": {
                "type": "string",
                "description": "The same thing, under the name the queueing call answered with. Pass either."
            }
        },
        "additionalProperties": false
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

    fn subscribe_manifest_changed(&self) -> Option<tokio::sync::broadcast::Receiver<()>> {
        // The built-in descriptors are fixed at compile time; the only thing
        // that moves is the user's AI Guide, which `external_manifest` reads
        // live on every call.
        Some(self.services.ai_guide.subscribe_tool_list_changed())
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

                // An argument the tool does not declare is a mistake worth naming.
                // Only for names we actually know: an unknown tool must still
                // answer "no such tool" rather than complaining about its args.
                if let Some(d) = Self::all_descriptors().iter().find(|d| d.name == resolved) {
                    let unknown = undeclared_arguments(&d.input_schema, &args);
                    if !unknown.is_empty() {
                        let declared: Vec<&str> = d
                            .input_schema
                            .get("properties")
                            .and_then(|p| p.as_object())
                            .map(|p| p.keys().map(String::as_str).collect())
                            .unwrap_or_default();
                        return ToolResult::Failed(format!(
                            "{name}: unknown argument(s) {unknown:?}; it takes {declared:?}"
                        ));
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
                // Before `read`, which owns `describe_app` and answers the
                // no-argument overview. With an `app` the catalog answers it
                // instead, and only this scope has the advertised manifest to
                // answer it FROM. Ordered the other way, `read` replied with the
                // overview and the argument was silently ignored.
                if let Some(result) =
                    super::apps::dispatch(resolved, &args, self.external_manifest())
                {
                    return result;
                }
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
                            //
                            // Apply the CLAIMED copy, not `p`. `stamp_source` above wrote
                            // the origin into the store; `p` is the value the tool
                            // returned before that, with `origin: None`. The applier asks
                            // exactly that field to decide whether an agent made this —
                            // so every artefact an agent created was recorded as the
                            // user's, and none of them ever showed the agent badge.
                            if let Some(claimed) = self.proposals.claim(&p.id) {
                                // A slow apply does not get to hold the request
                                // open. A 332 MB download did: the client timed
                                // out at its own limit and the transfer carried
                                // on unseen — no id, no progress, no error, and
                                // the caller could not tell whether it was still
                                // running. It now answers with the proposal id,
                                // which `get_job_status` reports on.
                                if claimed.long_running {
                                    self.start_background_apply(claimed);
                                    return ToolResult::Data(serde_json::json!({
                                        "applied": false,
                                        "started": true,
                                        "jobId": p.id,
                                        "proposalId": p.id,
                                        "kind": p.kind,
                                        "note": "Running in the background — poll get_job_status \
                                                 with this jobId for progress and the result.",
                                    }));
                                }
                                match super::apply_any(&self.services, &claimed).await {
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
                                        // `Failed`, not `Rejected`: this one was
                                        // approved and attempted, and the service
                                        // said no. Recording it as rejected made
                                        // an upstream 400 look like a local policy
                                        // refusal, and the caller could not tell
                                        // whether its request had ever been sent.
                                        self.proposals
                                            .settle(&p.id, super::proposals::ProposalState::Failed);
                                        return ToolResult::Failed(format!(
                                            "{} was approved and applied, and the service \
                                             refused it: {e}",
                                            p.kind
                                        ));
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
                // Through `can_accept`, which is the tested rule. Inlining it as
                // `pending_count() > cap()` was a SECOND rule that disagreed by
                // one: it accepted the proposal that took the queue to cap, so
                // the queue settled one over the cap it advertises. The count
                // here already includes the proposal just enqueued, so the
                // question is whether it could have been accepted.
                if ctx.is_external()
                    && !self
                        .services
                        .proposal_budget
                        .can_accept(self.proposals.pending_count().saturating_sub(1))
                {
                    let id = p.id.clone();
                    self.proposals
                        .resolve(&id, super::proposals::ProposalState::Withdrawn);
                    result = ToolResult::Failed(format!(
                        "proposal budget exhausted (cap {}); apply or reject a pending proposal first",
                        self.services.proposal_budget.cap()
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
mod undeclared_argument_tests {
    use super::undeclared_arguments;
    use serde_json::json;

    fn schema() -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "publisherId": {"type": "string"},
                "template": {"type": "string"}
            },
            "required": ["publisherId"],
            "additionalProperties": false
        })
    }

    /// An argument the tool never declared is named, not ignored.
    #[test]
    fn an_invented_argument_is_reported() {
        let unknown = undeclared_arguments(&schema(), &json!({"publisherId": "x", "title": "T"}));
        assert_eq!(unknown, vec!["title".to_string()]);
    }

    /// Both spellings of a DECLARED name are fine.
    ///
    /// `arg` bridges camelCase and snake_case at read time, so a tool honours
    /// `publisher_id` even though its schema says `publisherId`. Rejecting it
    /// here would break callers that work today — the whole point of checking
    /// is to catch mistakes, not to un-fix the aliasing.
    #[test]
    fn either_spelling_of_a_declared_argument_is_accepted() {
        assert!(undeclared_arguments(&schema(), &json!({"publisher_id": "x"})).is_empty());
        assert!(undeclared_arguments(&schema(), &json!({"publisherId": "x"})).is_empty());
    }

    /// A schema that permits extras still permits them.
    #[test]
    fn a_permissive_schema_is_left_alone() {
        let open = json!({"type": "object", "properties": {"a": {"type": "string"}}});
        assert!(undeclared_arguments(&open, &json!({"anything": 1})).is_empty());
    }

    /// Nothing to check is not an error.
    #[test]
    fn empty_and_non_object_arguments_are_fine() {
        assert!(undeclared_arguments(&schema(), &json!({})).is_empty());
        assert!(undeclared_arguments(&schema(), &json!(null)).is_empty());
    }

    /// Every reported miss is listed, in a stable order.
    #[test]
    fn every_unknown_key_is_named() {
        let unknown = undeclared_arguments(
            &schema(),
            &json!({"zeta": 1, "alpha": 2, "template": "image"}),
        );
        assert_eq!(unknown, vec!["alpha".to_string(), "zeta".to_string()]);
    }
}

#[cfg(test)]
mod proposal_id_tests {
    use super::proposal_id;
    use serde_json::json;

    /// The key we hand out is a key we accept.
    ///
    /// Queueing answers `proposalId`; these tools asked for `id`. Passing back
    /// the key you were just given produced
    /// `{"id": "", "state": "unknown"}` — indistinguishable from a proposal
    /// that never existed, so an agent polling for approval could not tell
    /// "still waiting" from "gone".
    #[test]
    fn either_spelling_resolves_to_the_same_proposal() {
        assert_eq!(
            proposal_id(&json!({"id": "prop-13"})).as_deref(),
            Some("prop-13")
        );
        assert_eq!(
            proposal_id(&json!({"proposalId": "prop-13"})).as_deref(),
            Some("prop-13")
        );
        // And the camel/snake bridge still applies on top.
        assert_eq!(
            proposal_id(&json!({"proposal_id": "prop-13"})).as_deref(),
            Some("prop-13")
        );
    }

    /// Nothing asked about is not the same as nothing found.
    #[test]
    fn a_missing_id_is_absent_rather_than_empty() {
        assert!(proposal_id(&json!({})).is_none());
        // Whitespace is not an id either; it used to look up "" and miss.
        assert!(proposal_id(&json!({"id": "   "})).is_none());
    }

    /// `id` wins when both are present, so the documented name stays primary.
    #[test]
    fn the_documented_name_takes_precedence() {
        assert_eq!(
            proposal_id(&json!({"id": "a", "proposalId": "b"})).as_deref(),
            Some("a")
        );
    }
}

#[cfg(test)]
mod tests {

    /// A long apply must not be awaited inside the request.
    ///
    /// A 332 MB observation download was: the client timed out at its own
    /// limit, the transfer carried on unseen, and the caller was left with no
    /// id, no progress and no error — it could not even tell whether the thing
    /// was still running.
    #[test]
    fn a_long_running_apply_is_started_not_awaited() {
        const SOURCE: &str = include_str!("router.rs");
        let code = crate::testing::without_comments(crate::testing::code(SOURCE));

        let at = code
            .find("if claimed.long_running")
            .expect("the router no longer distinguishes a long apply from a quick one");
        let branch = &code[at..(at + 600).min(code.len())];
        assert!(
            branch.contains("start_background_apply"),
            "a long apply is being run inline again"
        );
        assert!(
            branch.contains("\"jobId\""),
            "the caller is not told how to ask about the work it started"
        );

        // And the spawner must settle the proposal, or `list_proposals` shows
        // it pending forever after the work is done.
        let spawn_at = code
            .find("fn start_background_apply")
            .expect("start_background_apply is gone");
        let spawner = &code[spawn_at..(spawn_at + 900).min(code.len())];
        assert!(
            spawner.contains("tokio::spawn"),
            "it still blocks the request"
        );
        assert!(
            spawner.contains("proposals.settle"),
            "the proposal is left pending"
        );
        assert!(
            spawner.contains("jobs.finish"),
            "the job never reaches a terminal state"
        );
    }
    use super::*;

    /// An agent's write is recorded as an agent's.
    ///
    /// The router stamps the origin into the STORE and then applied the value
    /// the tool had returned a moment earlier, whose `origin` is still `None`.
    /// `AgentAttribution::for_applied_proposal` asks exactly that field, so
    /// every artefact an agent created — saved queries, workflows, bookmarks,
    /// notes — was attributed to the user and none of them showed the badge.
    #[test]
    fn the_applier_sees_the_origin_the_router_stamped() {
        let store = InMemoryProposalStore::new();
        let p = store.enqueue("save_query", "Save M51", false, serde_json::json!({}));
        assert!(p.origin.is_none(), "fresh proposals carry no origin");

        store.stamp_source(&p.id, "save_query", Some("Claude Desktop".into()));

        // What the applier is handed must be the stamped copy.
        let claimed = store.claim(&p.id).expect("claimable");
        assert_eq!(claimed.origin.as_deref(), Some("Claude Desktop"));
        assert!(
            crate::helpers::agent_attribution::AgentAttribution::for_applied_proposal(&claimed)
                .is_some(),
            "a claimed agent proposal must earn a badge"
        );

        // And the pre-stamp value would not have earned one — which is the bug.
        assert!(
            crate::helpers::agent_attribution::AgentAttribution::for_applied_proposal(&p).is_none()
        );
    }

    /// The cap is a cap on what the queue HOLDS.
    ///
    /// The router used to ask `pending_count() > cap`, having tested
    /// `can_accept` (`pending < cap`) instead — two rules, disagreeing by one,
    /// so a queue capped at N settled at N+1. This pins the boundary in the
    /// units the router works in: the count already includes the proposal that
    /// has just been enqueued.
    #[test]
    fn the_queue_may_reach_the_cap_and_not_pass_it() {
        let budget = crate::mcp::budget::ProposalBudget::new(3);
        let would_withdraw = |pending_including_new: usize| {
            !budget.can_accept(pending_including_new.saturating_sub(1))
        };
        assert!(!would_withdraw(1));
        assert!(
            !would_withdraw(3),
            "the third proposal fills the cap, it does not break it"
        );
        assert!(would_withdraw(4), "the fourth is one past the cap");
    }

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
