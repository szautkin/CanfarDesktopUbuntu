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

/// How long a UI-marshalled tool call waits before giving up (the reference's
/// 30s budget).
///
/// Right for the operations it was written for — steering a viewer, reading a
/// header — where 30s means something is wrong. Wrong for an archive query,
/// which CADC itself allows 600s: QA watched a `caom2.Observation JOIN Plane`
/// abort here while the same ADQL returned over `curl` in under a second, and
/// the agent was told the viewer had not answered.
pub(crate) const UI_COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

/// The budget for a query the archive is allowed to take its time over.
///
/// Matches TAP's own `executionDuration` default, so this stops being the thing
/// that gives up first.
pub(crate) const QUERY_COMMAND_TIMEOUT: Duration = Duration::from_secs(600);

/// How long `op` on `target` may take.
///
/// One table, because the alternative is a magic number at each call site and
/// no way to see them together. A slow operation is slow because of what it
/// ASKS OF A SERVICE, not because of which widget it belongs to.
pub(crate) fn timeout_for(target: &str, op: &str) -> Duration {
    match (target, op) {
        // Archive queries: a cone search over a large collection, or a JOIN
        // across caom2.Observation and Plane, legitimately runs for minutes.
        (_, "run_search")
        | (_, "execute_adql_query")
        | (_, "load_more_results")
        | (_, "resolve_target_name") => QUERY_COMMAND_TIMEOUT,
        _ => UI_COMMAND_TIMEOUT,
    }
}

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
        /// A local file path, or the id / publisher id of a downloaded
        /// observation. The UI resolves whichever it is.
        target: String,
        reply: oneshot::Sender<OpenFitsOutcome>,
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
    let budget = timeout_for(target, op);
    match tokio::time::timeout(budget, reply_rx).await {
        Ok(reply) => reply.map_err(|_| "viewer did not respond".to_string())?,
        // Says what is known, not what is guessed. It used to assert "the
        // window is blocked by a long-running operation", which sent readers
        // looking at GTK — the real cause was a bridge that ran commands one at
        // a time, so an unrelated slow command starved this one. Commands
        // interleave now, and a timeout here means THIS operation is slow.
        Err(_) => Err(format!(
            "the {} viewer did not answer '{}' within {}s — the operation may still be \
             running; check its own status before retrying.",
            target,
            op,
            budget.as_secs()
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
/// Open a FITS file by path, or by the id of a downloaded observation.
pub async fn open_fits(target: &str) -> OpenFitsOutcome {
    let target = target.to_string();
    send_action(|reply| ViewAction::OpenFits {
        target: target.clone(),
        reply,
    })
    .await
    .unwrap_or_else(|| OpenFitsOutcome::failed(&target, None, "could not dispatch to the UI"))
}

/// What came of an `open_fits_file`: whether a tab actually appeared, the id and
/// path it resolved to, and why not when it did not.
///
/// Port of the reference's `OpenFitsOutcome`. A bare boolean was not enough:
/// the old tool answered `opened: true` for a path that did not exist, an id it
/// could not resolve, and a file that would not parse, because it reported that
/// it had *dispatched* a request rather than that a file was open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenFitsOutcome {
    pub opened: bool,
    /// The observation id it resolved to, or the target as given.
    pub observation_id: String,
    /// Where the file is, once resolved — useful even on failure.
    pub local_path: Option<String>,
    /// Why it did not open. `None` on success.
    pub message: Option<String>,
}

impl OpenFitsOutcome {
    pub fn opened(observation_id: &str, local_path: &str) -> Self {
        Self {
            opened: true,
            observation_id: observation_id.to_string(),
            local_path: Some(local_path.to_string()),
            message: None,
        }
    }

    pub fn failed(observation_id: &str, local_path: Option<&str>, message: &str) -> Self {
        Self {
            opened: false,
            observation_id: observation_id.to_string(),
            local_path: local_path.map(str::to_string),
            message: Some(message.to_string()),
        }
    }
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

    #[test]
    fn an_outcome_carries_why_it_failed() {
        // A bare boolean was the bug: `open_fits_file` answered `opened: true`
        // for a path that did not exist, an id it could not resolve, and a file
        // that would not parse — because it reported that it had DISPATCHED a
        // request, not that a file was open. An agent following that answer
        // went looking for a tab that was never going to appear.
        let bad = OpenFitsOutcome::failed("obs-1", None, "file not found");
        assert!(!bad.opened);
        assert_eq!(bad.message.as_deref(), Some("file not found"));

        let good = OpenFitsOutcome::opened("obs-1", "/home/u/f.fits");
        assert!(good.opened);
        assert_eq!(good.message, None, "a success must not carry a complaint");
        assert_eq!(good.local_path.as_deref(), Some("/home/u/f.fits"));
    }

    #[test]
    fn a_failure_still_says_where_it_looked() {
        // "not downloaded yet" is only actionable if the agent can see which
        // file was expected.
        let outcome = OpenFitsOutcome::failed(
            "obs-1",
            Some("/home/u/research/obs-1.fits"),
            "not downloaded yet — use download_observation first",
        );
        assert_eq!(
            outcome.local_path.as_deref(),
            Some("/home/u/research/obs-1.fits")
        );
    }
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

#[cfg(test)]
mod push_tests {
    //! A snapshot field nobody writes is a field that answers with its default
    //! forever. `get_app_view` reported `isAuthenticated: false` and a null sky
    //! focus for every session the app ever ran, because `set_auth` and
    //! `set_search_focus` existed and nothing called them — visible only as two
    //! lines in the build's dead-code warnings.

    /// Every setter on the pull half, and where the UI is expected to push it.
    const PUSHERS: &[(&str, &str)] = &[
        ("set_view", "ui/main_window.rs"),
        ("set_auth", "ui/main_window.rs"),
        ("set_search_focus", "ui/search_page/mod.rs"),
        ("set_open_fits", "ui/fits_viewer.rs"),
    ];

    #[test]
    fn every_snapshot_field_has_something_writing_it() {
        let sources = crate::testing::rust_sources();
        for (setter, expected) in PUSHERS {
            let callers: Vec<_> = sources
                .iter()
                .filter(|(path, text)| {
                    !path.ends_with("mcp/view_state.rs")
                        && crate::testing::code(text).contains(&format!("view_state::{setter}("))
                })
                .map(|(path, _)| path.display().to_string())
                .collect();
            assert!(
                !callers.is_empty(),
                "nothing calls view_state::{setter} — the field it feeds answers \
                 with its default for the life of the process. It belongs in {expected}."
            );
        }
    }
}

#[cfg(test)]
mod timeout_tests {
    use super::*;

    /// An archive query gets the archive's own budget.
    ///
    /// QA: a `caom2.Observation JOIN Plane` on `proposal_id` aborted at 30s
    /// while the same ADQL returned over `curl` in under a second — the client
    /// gave up before the service had answered, and reported it as the viewer
    /// failing to respond.
    #[test]
    fn a_query_may_take_as_long_as_the_archive_allows() {
        assert_eq!(
            timeout_for("search", "execute_adql_query"),
            QUERY_COMMAND_TIMEOUT
        );
        assert_eq!(timeout_for("search", "run_search"), QUERY_COMMAND_TIMEOUT);
        assert!(
            QUERY_COMMAND_TIMEOUT.as_secs() >= 600,
            "shorter than TAP's own executionDuration default, so this gives up first"
        );
    }

    /// Steering a viewer does not.
    ///
    /// The long budget must not leak onto everything: a viewer that has stopped
    /// answering should be reported in seconds, not in ten minutes.
    #[test]
    fn steering_a_viewer_keeps_the_short_budget() {
        for op in [
            "set_fits_view",
            "get_fits_image",
            "run_cell",
            "get_cube_view",
        ] {
            assert_eq!(
                timeout_for("fits", op),
                UI_COMMAND_TIMEOUT,
                "{op} was given the query budget"
            );
        }
    }

    #[test]
    fn the_two_budgets_are_actually_different() {
        assert!(
            QUERY_COMMAND_TIMEOUT > UI_COMMAND_TIMEOUT,
            "the table exists to distinguish them"
        );
    }
}
