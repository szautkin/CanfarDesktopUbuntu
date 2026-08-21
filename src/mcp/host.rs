//! App-side owner of the MCP server. Assembles the router from the live tool
//! catalog, runs the UNIX-domain-socket [`listener`](crate::mcp::listener), and
//! hands back the shared proposal store so the app's review UI can drive the
//! apply/reject lifecycle. Ported from `Mcp/McpHost.cs`.
//!
//! A single stateless router (shared across connections) backs a fresh per-
//! connection server inside the listener. The host owns the accept-loop task
//! handle so it can stop the server; [`start`](McpHost::start) is idempotent
//! (a second call returns the already-running store rather than binding twice).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tokio::task::JoinHandle;

use crate::mcp::server::ApprovalGate;
use crate::mcp::tools::catalog::{build_router_with, proposal_journal_path};
use crate::mcp::tools::proposals::{InMemoryProposalStore, ProposalState};
use crate::mcp::tools::ToolRouter;
use crate::state::AppServices;

/// Owns the running MCP listener task and the write-proposal store shared with
/// the app UI. Registered as an app-lifetime singleton; started on launch (when
/// the user opts in) and stopped on shutdown.
pub struct McpHost {
    /// Whether the listener accept loop is live.
    running: AtomicBool,
    /// The accept-loop task; aborted on [`stop`](McpHost::stop).
    handle: Mutex<Option<JoinHandle<()>>>,
    /// The store the running router enqueues proposals into — held so a repeat
    /// `start` returns the SAME store (the UI is already bound to it).
    proposals: Mutex<Option<Arc<InMemoryProposalStore>>>,
}

impl Default for McpHost {
    fn default() -> Self {
        Self::new()
    }
}

impl McpHost {
    pub fn new() -> Self {
        McpHost {
            running: AtomicBool::new(false),
            handle: Mutex::new(None),
            proposals: Mutex::new(None),
        }
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Start the listener bound to `services`, gated by `gate`, and return the
    /// shared proposal store. Idempotent: if already running, no second socket is
    /// bound — the existing store is returned so the caller stays bound to it.
    pub fn start(
        &self,
        services: Arc<AppServices>,
        gate: Arc<dyn ApprovalGate>,
    ) -> Arc<InMemoryProposalStore> {
        // Hold the handle lock for the whole start so two concurrent starts can't
        // both bind the socket; the running check + spawn happen atomically here.
        let mut handle_guard = self.handle.lock().unwrap();
        if self.running.load(Ordering::SeqCst) {
            if let Some(existing) = self.proposals.lock().unwrap().clone() {
                return existing;
            }
        }

        // Journalled: a restart used to destroy every proposal awaiting human
        // review, including ones already approved.
        let (router, proposals) =
            build_router_with(Arc::clone(&services), Some(proposal_journal_path()));
        // Unsize the concrete router into the trait object the listener consumes.
        let router: Arc<dyn ToolRouter> = router;

        // Spawn the accept loop on the app's tokio runtime. `listener::run` only
        // returns on an accept error; whatever it yields we drop — stopping is
        // done by aborting this task, not by it returning.
        let handle = services.rt.spawn(async move {
            let _ = crate::mcp::listener::run(router, gate).await;
        });

        *handle_guard = Some(handle);
        *self.proposals.lock().unwrap() = Some(Arc::clone(&proposals));
        self.running.store(true, Ordering::SeqCst);
        proposals
    }

    /// The shared proposal store, if the server is running — so the app's review
    /// UI can list pending proposals and drive apply/reject.
    pub fn proposals(&self) -> Option<Arc<InMemoryProposalStore>> {
        self.proposals.lock().unwrap().clone()
    }

    /// Apply a user-approved pending proposal: run its applier against the live
    /// services and mark it Applied (or Rejected on a failed apply). Returns the
    /// applier's success message.
    pub async fn apply_proposal(&self, services: &AppServices, id: &str) -> Result<String, String> {
        let store = self.proposals().ok_or("MCP server is not running")?;
        // Atomically claim the proposal (Pending → Applying) so the applier runs at
        // most once and can't race a concurrent apply or reject. `None` means it was
        // already claimed/resolved.
        let proposal = store
            .claim(id)
            .ok_or("proposal is not pending (already applied, rejected, or in progress)")?;
        let result = crate::mcp::tools::apply_any(services, &proposal).await;
        match &result {
            Ok(_) => {
                store.settle(id, ProposalState::Applied);
            }
            Err(_) => {
                // A failed apply settles to Rejected, not left dangling in Applying.
                store.settle(id, ProposalState::Rejected);
            }
        }
        result
    }

    /// Reject (withdraw) a pending proposal without applying it.
    pub fn reject_proposal(&self, id: &str) -> Result<(), String> {
        let store = self.proposals().ok_or("MCP server is not running")?;
        store
            .resolve(id, ProposalState::Withdrawn)
            .map(|_| ())
            .ok_or_else(|| "proposal is not pending".to_string())
    }

    /// Stop the listener (idempotent). Aborts the accept-loop task and drops the
    /// shared store so a subsequent `start` builds a fresh one.
    pub fn stop(&self) {
        if let Some(handle) = self.handle.lock().unwrap().take() {
            handle.abort();
        }
        *self.proposals.lock().unwrap() = None;
        self.running.store(false, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // NOTE: no `start()` test here — `start` spawns the real listener, which binds
    // (and first removes any stale) per-user control socket. Running that under
    // `cargo test` would hijack a live app instance's socket. Idempotency /
    // stop-resets is covered by the wire-level integration path instead.
    #[test]
    fn new_host_is_not_running() {
        let host = McpHost::new();
        assert!(!host.is_running());
    }
}
