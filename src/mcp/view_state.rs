//! The app view-state bridge between the MCP serve tasks (tokio threads) and the
//! GTK main thread. Port of `Mcp/AppViewStateService.cs`.
//!
//! Two halves:
//!  * **Pull** — the UI pushes a plain, `Send` snapshot of "what the user is
//!    looking at" (current view, title, auth, search focus, open documents) into
//!    a shared [`ViewSnapshot`]; view-state read tools read it directly, no
//!    thread-hop needed.
//!  * **Push** — steering actions (`navigate_to`, `open_fits_file`,
//!    `close_active_tab`) can't touch widgets from a tokio thread, so they send a
//!    [`ViewAction`] (with a oneshot reply) over a channel the UI drains on the
//!    GTK main loop, and await the result.

use once_cell::sync::Lazy;
use std::sync::RwLock;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

/// How long a UI-marshalled tool call waits for the GTK main loop before giving
/// up with a typed "UI busy" error (mirrors the reference's 30s budget).
const UI_COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

/// A `Send` snapshot of the current UI state, pushed by the UI and read by tools.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ViewSnapshot {
    /// The active view key (home/search/storage/fits/notebook/research/cube/workflows/aiguide/settings).
    pub view: String,
    pub title: String,
    pub authenticated: bool,
    pub username: Option<String>,
    pub search_focus_ra: Option<f64>,
    pub search_focus_dec: Option<f64>,
    pub open_fits_paths: Vec<String>,
    pub open_notebooks: Vec<String>,
    pub open_cubes: Vec<String>,
    /// 0-based index of the ACTIVE tab in each list, when one is open.
    ///
    /// `list_open_tabs` has to report this: `blink_fits_tabs` requires a partner
    /// tab DIFFERENT from the active one, and every other viewer tool acts on
    /// whichever tab is active — without it an agent is guessing.
    pub active_fits: Option<usize>,
    pub active_notebook: Option<usize>,
    pub active_cube: Option<usize>,
}

static STATE: Lazy<RwLock<ViewSnapshot>> = Lazy::new(|| RwLock::new(ViewSnapshot::default()));

/// A steering request the UI applies on the GTK main thread; each carries its
/// own oneshot reply channel.
pub enum ViewAction {
    Navigate {
        key: String,
        reply: oneshot::Sender<bool>,
    },
    OpenFits {
        path: String,
        reply: oneshot::Sender<bool>,
    },
    CloseActiveTab {
        reply: oneshot::Sender<bool>,
    },
    SetSearchFocus {
        ra: f64,
        dec: f64,
        reply: oneshot::Sender<bool>,
    },
}

static ACTION_TX: Lazy<RwLock<Option<mpsc::UnboundedSender<ViewAction>>>> =
    Lazy::new(|| RwLock::new(None));

/// A live per-viewer command (cube/notebook/fits): the UI runs it on the GTK main
/// thread against the open viewer and replies with a JSON result. This is the
/// generic spine behind the cube/notebook/fits MCP tool families.
pub struct ViewerCommand {
    /// "cube" | "notebook" | "fits".
    pub target: String,
    /// The operation name, e.g. "get_view" / "set_view" / "run_cell".
    pub op: String,
    pub args: serde_json::Value,
    pub reply: oneshot::Sender<Result<serde_json::Value, String>>,
}

static VIEWER_TX: Lazy<RwLock<Option<mpsc::UnboundedSender<ViewerCommand>>>> =
    Lazy::new(|| RwLock::new(None));

/// Install the viewer-command channel (UI drains it on the GTK main loop).
pub fn install_viewer_sender(tx: mpsc::UnboundedSender<ViewerCommand>) {
    *VIEWER_TX.write().unwrap() = Some(tx);
}

/// Tool-side helper: run a command against a live viewer and await its JSON reply.
pub async fn viewer_command(
    target: &str,
    op: &str,
    args: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let tx = VIEWER_TX
        .read()
        .unwrap()
        .clone()
        .ok_or_else(|| "viewer bridge not available (no window open)".to_string())?;
    let (reply_tx, reply_rx) = oneshot::channel();
    tx.send(ViewerCommand {
        target: target.to_string(),
        op: op.to_string(),
        args,
        reply: reply_tx,
    })
    .map_err(|_| "viewer bridge closed".to_string())?;
    // Bound the wait: the reply only arrives once the GTK main loop drains the
    // command queue, so a saturated UI thread would otherwise hang the tool call
    // until the transport itself gave up. Fail with a descriptive error instead.
    match tokio::time::timeout(UI_COMMAND_TIMEOUT, reply_rx).await {
        Ok(reply) => reply.map_err(|_| "viewer did not respond".to_string())?,
        Err(_) => Err(format!(
            "UI busy: the {} viewer did not answer '{}' within {}s (the window is blocked by a \
             long-running operation) — retry once it is idle.",
            target,
            op,
            UI_COMMAND_TIMEOUT.as_secs()
        )),
    }
}

// ── Pull half: the UI pushes, tools read ────────────────────────────────────

pub fn set_view(view: &str, title: &str) {
    let mut s = STATE.write().unwrap();
    s.view = view.to_string();
    s.title = title.to_string();
}

pub fn set_auth(authenticated: bool, username: Option<String>) {
    let mut s = STATE.write().unwrap();
    s.authenticated = authenticated;
    s.username = username;
}

pub fn set_search_focus(ra: Option<f64>, dec: Option<f64>) {
    let mut s = STATE.write().unwrap();
    s.search_focus_ra = ra;
    s.search_focus_dec = dec;
}

pub fn set_open_fits(paths: Vec<String>, active: Option<usize>) {
    let mut s = STATE.write().unwrap();
    s.open_fits_paths = paths;
    s.active_fits = active;
}

pub fn set_open_notebooks(paths: Vec<String>, active: Option<usize>) {
    let mut s = STATE.write().unwrap();
    s.open_notebooks = paths;
    s.active_notebook = active;
}

pub fn set_open_cubes(paths: Vec<String>, active: Option<usize>) {
    let mut s = STATE.write().unwrap();
    s.open_cubes = paths;
    s.active_cube = active;
}

/// A snapshot of the current view state (for `get_current_view` / `list_open_tabs`).
pub fn capture() -> ViewSnapshot {
    STATE.read().unwrap().clone()
}

// ── Push half: tools steer the UI ───────────────────────────────────────────

/// Install the channel the UI drains on the GTK main loop. Called once at startup.
pub fn install_action_sender(tx: mpsc::UnboundedSender<ViewAction>) {
    *ACTION_TX.write().unwrap() = Some(tx);
}

async fn send_action<T>(make: impl FnOnce(oneshot::Sender<T>) -> ViewAction) -> Option<T> {
    let tx = ACTION_TX.read().unwrap().clone()?;
    let (reply_tx, reply_rx) = oneshot::channel();
    tx.send(make(reply_tx)).ok()?;
    reply_rx.await.ok()
}

/// Fire-and-forget navigation (used by "follow the agent" — no reply awaited, so
/// it never adds latency to the agent's tool call).
pub fn navigate_fire(key: &str) {
    if let Some(tx) = ACTION_TX.read().unwrap().clone() {
        let (reply, _drop) = oneshot::channel();
        let _ = tx.send(ViewAction::Navigate {
            key: key.to_string(),
            reply,
        });
    }
}

/// Map an MCP tool name to the view it operates on (for follow-the-agent nav).
pub fn module_for_tool(name: &str) -> Option<&'static str> {
    let m = match name {
        n if n.contains("notebook") || n.contains("cell") || n.contains("kernel") => "notebook",
        n if n.contains("cube") => "cube",
        n if n.contains("fits") => "fits",
        n if n.contains("storage")
            || n.contains("vospace")
            || n.contains("node")
            || n.contains("folder")
            || n.contains("upload") =>
        {
            "storage"
        }
        n if n.contains("observation")
            || n.contains("research")
            || n.contains("caom2")
            || n.contains("preview") =>
        {
            "research"
        }
        n if n.contains("session") || n.contains("headless") || n.contains("image") => "portal",
        n if n.contains("workflow") => "workflows",
        n if n.contains("search") || n.contains("vizier") => "search",
        _ => return None,
    };
    Some(m)
}

/// Navigate to a view key. Returns `false` if the bridge isn't installed or the
/// key is unknown.
pub async fn navigate_to(key: &str) -> bool {
    let key = key.to_string();
    send_action(|reply| ViewAction::Navigate { key, reply })
        .await
        .unwrap_or(false)
}

/// Open a local FITS file in the FITS viewer.
pub async fn open_fits(path: &str) -> bool {
    let path = path.to_string();
    send_action(|reply| ViewAction::OpenFits { path, reply })
        .await
        .unwrap_or(false)
}

/// Close the active tab of the current module.
pub async fn close_active_tab() -> bool {
    send_action(|reply| ViewAction::CloseActiveTab { reply })
        .await
        .unwrap_or(false)
}

/// Navigate to Search and prefill a sky position.
pub async fn set_search_focus_action(ra: f64, dec: f64) -> bool {
    send_action(|reply| ViewAction::SetSearchFocus { ra, dec, reply })
        .await
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_round_trips() {
        set_view("search", "Search");
        set_auth(true, Some("alice".into()));
        set_search_focus(Some(10.0), Some(20.0));
        let s = capture();
        assert_eq!(s.view, "search");
        assert!(s.authenticated);
        assert_eq!(s.username.as_deref(), Some("alice"));
        assert_eq!(s.search_focus_ra, Some(10.0));
    }

    #[tokio::test]
    async fn navigate_without_bridge_is_false() {
        // No sender installed in a unit test → steering fails gracefully.
        *ACTION_TX.write().unwrap() = None;
        assert!(!navigate_to("home").await);
    }
}
