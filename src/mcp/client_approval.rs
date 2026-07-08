//! Persisted MCP client allow-list + seen-clients registry, and the real
//! [`ApprovalGate`] that consults it.
//!
//! Linux port of `Mcp/McpClientApprovalStore.cs` + `Mcp/LocalSettingsApprovalStorage.cs`.
//! The Windows reference splits the gate logic ([`McpClientApprovalStore`]) from a
//! `LocalSettings`-backed storage seam (`LocalSettingsApprovalStorage`); here the two
//! collapse into one store that persists a tiny JSON document at
//! `ProjectDirs("net","canfar","Verbinal").data_dir()/mcp_clients.json`:
//!
//! ```json
//! { "require_approval": false, "allow": ["agent-a"], "seen": ["agent-b"] }
//! ```
//!
//! Identity here is attribution-only (the real boundary is the owner-only 0700
//! UNIX socket), so this drives visibility + opt-in lockdown + revocation, not
//! authentication. The default policy is **allow-all** (`require_approval = false`)
//! so existing setups are unchanged until a user turns lockdown on. The app's own
//! loopback self-test client is always permitted (and never recorded) so the
//! connection-wizard Verify step works under any policy.
//!
//! State lives behind a `Mutex` (connections dispatch concurrently) and every
//! mutator persists atomically (write to a `.tmp` sibling, then rename), mirroring
//! `services::observation_note_store`.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

use crate::mcp::server::ApprovalGate;

/// The app's own loopback self-test client id. MUST match `SELF_TEST_CLIENT` in
/// `crate::mcp::selftest` (that constant is module-private, so it is mirrored
/// here). The self-test probe is internal, not an external agent: it is always
/// permitted and never recorded as a seen client.
const SELF_TEST_CLIENT_ID: &str = "verbinal-selftest";

/// True if `client_id` is the internal self-test probe — either the bare id or a
/// `verbinal-selftest/<version>` form (the C# `IsInternalClient` also matched the
/// `name/version` shape).
fn is_internal_client(client_id: &str) -> bool {
    client_id == SELF_TEST_CLIENT_ID
        || client_id.starts_with(&format!("{SELF_TEST_CLIENT_ID}/"))
}

/// The persisted document. `Default` gives the allow-all, empty-lists baseline,
/// which is also what a missing or corrupt file falls back to.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct StoreState {
    /// When true, only clients on `allow` may connect; default false (allow all).
    #[serde(default)]
    require_approval: bool,
    /// Persisted allow-list of client ids (insertion order, de-duplicated).
    #[serde(default)]
    allow: Vec<String>,
    /// Client ids that have been observed (currently, denied) connecting.
    #[serde(default)]
    seen: Vec<String>,
}

/// Persisted allow-list + seen-clients registry for external MCP clients.
///
/// Thread-safe; every mutator writes the whole document back atomically. Cheap to
/// wrap in an `Arc` and share with [`ApprovalStoreGate`] and the settings UI.
pub struct McpClientApprovalStore {
    path: PathBuf,
    state: Mutex<StoreState>,
}

impl McpClientApprovalStore {
    /// Load from the standard per-user location
    /// (`data_dir()/mcp_clients.json`), falling back to a relative path if the
    /// platform dirs can't be resolved. A missing/corrupt file yields defaults.
    pub fn load() -> Self {
        let path = ProjectDirs::from("net", "canfar", "Verbinal")
            .map(|dirs| dirs.data_dir().join("mcp_clients.json"))
            .unwrap_or_else(|| PathBuf::from("mcp_clients.json"));
        Self::with_path(path)
    }

    /// Load from an explicit path. Primarily a test seam (point the store at a
    /// throwaway file), but also usable for a custom location.
    pub fn with_path(path: PathBuf) -> Self {
        let state = Self::read_state(&path);
        McpClientApprovalStore {
            path,
            state: Mutex::new(state),
        }
    }

    /// When true, only approved clients may connect. Persisted.
    pub fn require_approval(&self) -> bool {
        self.state.lock().unwrap().require_approval
    }

    /// Turn the require-approval lockdown on or off. Persists on change.
    pub fn set_require_approval(&self, v: bool) {
        let mut state = self.state.lock().unwrap();
        if state.require_approval == v {
            return;
        }
        state.require_approval = v;
        self.persist(&state);
    }

    /// True if `id` is on the persisted allow-list.
    pub fn is_approved(&self, id: &str) -> bool {
        self.state.lock().unwrap().allow.iter().any(|c| c == id)
    }

    /// Add `id` to the allow-list. No-op for an empty id or one already present;
    /// otherwise persists.
    pub fn approve(&self, id: &str) {
        if id.is_empty() {
            return;
        }
        let mut state = self.state.lock().unwrap();
        if state.allow.iter().any(|c| c == id) {
            return;
        }
        state.allow.push(id.to_string());
        self.persist(&state);
    }

    /// Remove `id` from the allow-list. No-op (no write) if it wasn't present.
    pub fn revoke(&self, id: &str) {
        let mut state = self.state.lock().unwrap();
        let before = state.allow.len();
        state.allow.retain(|c| c != id);
        if state.allow.len() == before {
            return;
        }
        self.persist(&state);
    }

    /// Record that `id` connected. No-op for an empty id or one already recorded;
    /// otherwise persists. The self-test probe is exempted at the gate, so it is
    /// never passed here in practice.
    pub fn mark_seen(&self, id: &str) {
        if id.is_empty() {
            return;
        }
        let mut state = self.state.lock().unwrap();
        if state.seen.iter().any(|c| c == id) {
            return;
        }
        state.seen.push(id.to_string());
        self.persist(&state);
    }

    /// Snapshot of every client id observed connecting (insertion order).
    pub fn seen_clients(&self) -> Vec<String> {
        self.state.lock().unwrap().seen.clone()
    }

    /// Snapshot of the current allow-list (insertion order).
    pub fn approved_clients(&self) -> Vec<String> {
        self.state.lock().unwrap().allow.clone()
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    fn read_state(path: &Path) -> StoreState {
        match std::fs::read_to_string(path) {
            Ok(raw) => serde_json::from_str(&raw).unwrap_or_default(),
            Err(_) => StoreState::default(),
        }
    }

    /// Best-effort atomic persist (write to a `.tmp` sibling, then rename).
    /// Mutators return `()`, matching the C# void setters, so a write error is
    /// swallowed rather than surfaced — the in-memory state stays authoritative.
    fn persist(&self, state: &StoreState) {
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let json = match serde_json::to_string_pretty(state) {
            Ok(j) => j,
            Err(_) => return,
        };
        let tmp = self.path.with_extension("json.tmp");
        if std::fs::write(&tmp, json.as_bytes()).is_ok() {
            let _ = std::fs::rename(&tmp, &self.path);
        }
    }
}

impl Default for McpClientApprovalStore {
    fn default() -> Self {
        Self::load()
    }
}

/// The wired [`ApprovalGate`]: consults an [`McpClientApprovalStore`] at
/// `initialize`. Permits the self-test probe unconditionally, permits everyone
/// while lockdown is off, and otherwise admits only allow-listed clients —
/// recording any denied client as "seen" so the user can review and approve it.
pub struct ApprovalStoreGate {
    store: Arc<McpClientApprovalStore>,
}

impl ApprovalStoreGate {
    pub fn new(store: Arc<McpClientApprovalStore>) -> Self {
        ApprovalStoreGate { store }
    }
}

impl ApprovalGate for ApprovalStoreGate {
    fn permit<'a>(&'a self, client_id: &'a str) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>> {
        Box::pin(async move {
            // The app's own self-test probe is internal — always allow, never list.
            if is_internal_client(client_id) {
                return true;
            }
            // Lockdown off (default) → allow all, unchanged from pre-hardening.
            if !self.store.require_approval() {
                return true;
            }
            // Lockdown on → only allow-listed clients pass.
            if self.store.is_approved(client_id) {
                return true;
            }
            // Denied: record it so the user can see + approve it later.
            self.store.mark_seen(client_id);
            false
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// A throwaway JSON path (unique per call), removed on drop along with its
    /// `.tmp` sibling — tests never touch the user's real `mcp_clients.json`.
    struct TempPath(PathBuf);

    impl TempPath {
        fn new() -> Self {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "verbinal_mcp_clients_test_{}_{}_{}.json",
                std::process::id(),
                nanos,
                n
            ));
            TempPath(path)
        }

        fn store(&self) -> McpClientApprovalStore {
            McpClientApprovalStore::with_path(self.0.clone())
        }
    }

    impl Drop for TempPath {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
            let _ = std::fs::remove_file(self.0.with_extension("json.tmp"));
        }
    }

    #[test]
    fn default_policy_is_allow_all() {
        let tmp = TempPath::new();
        let store = tmp.store();
        assert!(!store.require_approval());
        assert!(store.approved_clients().is_empty());
        assert!(store.seen_clients().is_empty());
    }

    #[tokio::test]
    async fn unknown_client_admitted_when_lockdown_off() {
        let tmp = TempPath::new();
        let store = Arc::new(tmp.store());
        let gate = ApprovalStoreGate::new(Arc::clone(&store));

        // Lockdown off → permitted, and NOT recorded as seen (only denials are).
        assert!(gate.permit("agent-x").await);
        assert!(store.seen_clients().is_empty());
    }

    #[tokio::test]
    async fn unseen_client_denied_and_recorded_when_lockdown_on() {
        let tmp = TempPath::new();
        let store = Arc::new(tmp.store());
        store.set_require_approval(true);
        let gate = ApprovalStoreGate::new(Arc::clone(&store));

        assert!(!gate.permit("agent-x").await);
        // Denial records the client so the user can review + approve it.
        assert_eq!(store.seen_clients(), vec!["agent-x".to_string()]);
        assert!(!store.is_approved("agent-x"));
    }

    #[tokio::test]
    async fn approved_client_permitted_and_not_listed_as_seen() {
        let tmp = TempPath::new();
        let store = Arc::new(tmp.store());
        store.set_require_approval(true);
        store.approve("agent-x");
        let gate = ApprovalStoreGate::new(Arc::clone(&store));

        assert!(gate.permit("agent-x").await);
        // Permitted clients are not added to the seen list.
        assert!(store.seen_clients().is_empty());
    }

    #[tokio::test]
    async fn self_test_id_always_permitted_even_under_lockdown() {
        let tmp = TempPath::new();
        let store = Arc::new(tmp.store());
        store.set_require_approval(true);
        let gate = ApprovalStoreGate::new(Arc::clone(&store));

        assert!(gate.permit(SELF_TEST_CLIENT_ID).await);
        assert!(gate.permit(&format!("{SELF_TEST_CLIENT_ID}/1")).await);
        // The internal probe is never recorded.
        assert!(store.seen_clients().is_empty());
    }

    #[test]
    fn approve_is_idempotent_and_revoke_removes() {
        let tmp = TempPath::new();
        let store = tmp.store();

        store.approve("a");
        store.approve("a"); // duplicate — no second entry
        assert_eq!(store.approved_clients(), vec!["a".to_string()]);
        assert!(store.is_approved("a"));

        store.revoke("a");
        assert!(!store.is_approved("a"));
        assert!(store.approved_clients().is_empty());

        // Empty id is ignored.
        store.approve("");
        assert!(store.approved_clients().is_empty());
    }

    #[test]
    fn mark_seen_dedupes() {
        let tmp = TempPath::new();
        let store = tmp.store();
        store.mark_seen("b");
        store.mark_seen("b");
        store.mark_seen("");
        assert_eq!(store.seen_clients(), vec!["b".to_string()]);
    }

    #[test]
    fn persistence_round_trip() {
        let tmp = TempPath::new();
        {
            let store = tmp.store();
            store.set_require_approval(true);
            store.approve("agent-a");
            store.mark_seen("agent-b");
        }
        // A fresh store over the same path recovers every mutation.
        let reloaded = tmp.store();
        assert!(reloaded.require_approval());
        assert!(reloaded.is_approved("agent-a"));
        assert_eq!(reloaded.approved_clients(), vec!["agent-a".to_string()]);
        assert_eq!(reloaded.seen_clients(), vec!["agent-b".to_string()]);
    }

    #[test]
    fn corrupt_file_falls_back_to_defaults() {
        let tmp = TempPath::new();
        std::fs::write(&tmp.0, b"{ this is not json").unwrap();
        let store = tmp.store();
        assert!(!store.require_approval());
        assert!(store.approved_clients().is_empty());
        assert!(store.seen_clients().is_empty());
    }
}
