//! What the app is doing right now, in one place.
//!
//! Every long operation used to be a detached future with whatever feedback the
//! widget that started it happened to offer: a row subtitle here, a status
//! label there, a toast, or — often — nothing at all. Three consequences, all
//! observed rather than imagined:
//!
//!   * a seven-stage probe reported as one boolean, so three rows read
//!     "Discovering…" while two jobs existed and there was no way to tell which
//!     row was which;
//!   * outcomes dropped on the floor (`let _ = …`), so a probe that could not
//!     even be submitted looked exactly like one nobody had asked for;
//!   * work stranded in a running state whenever a completion path was missed —
//!     three separate times in one afternoon.
//!
//! So: one registry, stages instead of a boolean, and completion by [`Drop`] so
//! "still running forever" is not representable.
//!
//! Shaped like the two global logs already here — [`crate::helpers::store_events`]
//! and [`crate::helpers::agent_activity`]: a static behind a mutex, free
//! functions, and a sequence number the UI can poll cheaply.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::SystemTime;

/// What kind of work it is, for grouping and for the icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskKind {
    /// Inspecting a container image.
    Discovery,
    /// Launching a session or batch job.
    Launch,
    /// Acting on an existing session.
    Session,
    /// Reading or writing VOSpace.
    Storage,
    /// Background reconciliation.
    Sync,
}

impl TaskKind {
    pub fn label(self) -> &'static str {
        match self {
            TaskKind::Discovery => crate::tr_en!("Image inspection"),
            TaskKind::Launch => crate::tr_en!("Launch"),
            TaskKind::Session => crate::tr_en!("Session"),
            TaskKind::Storage => crate::tr_en!("Storage"),
            TaskKind::Sync => crate::tr_en!("Sync"),
        }
    }
}

/// Where a task got to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskState {
    Running,
    Succeeded,
    /// Carries the reason, because a failure with no reason is the thing this
    /// module exists to stop.
    Failed(String),
    /// The handle was dropped without an outcome — the future was abandoned.
    Cancelled,
}

impl TaskState {
    pub fn is_finished(&self) -> bool {
        !matches!(self, TaskState::Running)
    }
}

/// One unit of work, as the status bar shows it.
#[derive(Debug, Clone)]
pub struct Task {
    pub id: u64,
    pub kind: TaskKind,
    /// What it is: "Inspect skaha/base:1.0".
    pub label: String,
    /// Where it has got to: "waiting for job vi-abc". Empty until set.
    pub stage: String,
    pub state: TaskState,
    pub started: SystemTime,
    pub finished: Option<SystemTime>,
}

impl Task {
    /// How long it ran, or has been running.
    pub fn elapsed(&self) -> std::time::Duration {
        let end = self.finished.unwrap_or_else(SystemTime::now);
        end.duration_since(self.started).unwrap_or_default()
    }
}

/// How many tasks to remember.
///
/// Finished ones are kept so a failure can be read after the fact — a toast
/// that has faded is no record. Bounded so a catalogue sweep of three hundred
/// probes cannot grow this without limit.
const MAX_TASKS: usize = 60;

static TASKS: Mutex<Vec<Task>> = Mutex::new(Vec::new());
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Bumped on every change, so a poller can tell "nothing happened" from
/// "something did" without copying the list.
static SEQ: AtomicU64 = AtomicU64::new(0);

fn with_tasks<R>(f: impl FnOnce(&mut Vec<Task>) -> R) -> R {
    let mut guard = TASKS.lock().unwrap_or_else(|e| e.into_inner());
    let out = f(&mut guard);
    SEQ.fetch_add(1, Ordering::Relaxed);
    out
}

/// Start tracking a piece of work. The returned handle owns its outcome.
pub fn begin(kind: TaskKind, label: impl Into<String>) -> TaskHandle {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let task = Task {
        id,
        kind,
        label: label.into(),
        stage: String::new(),
        state: TaskState::Running,
        started: SystemTime::now(),
        finished: None,
    };
    with_tasks(|tasks| {
        tasks.push(task);
        // Drop the oldest FINISHED entries first: a running task is the thing
        // the reader most needs to see, and must never be evicted by newer work.
        while tasks.len() > MAX_TASKS {
            match tasks.iter().position(|t| t.state.is_finished()) {
                Some(at) => {
                    tasks.remove(at);
                }
                None => break,
            }
        }
    });
    TaskHandle { id }
}

/// A snapshot for the UI, oldest first.
pub fn snapshot() -> Vec<Task> {
    TASKS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .cloned()
        .collect()
}

/// The change counter. Poll it; re-read [`snapshot`] only when it moves.
pub fn sequence() -> u64 {
    SEQ.load(Ordering::Relaxed)
}

/// How many are still running.
pub fn running_count() -> usize {
    TASKS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .filter(|t| !t.state.is_finished())
        .count()
}

/// How many finished badly (failed or abandoned).
pub fn failed_count() -> usize {
    TASKS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .filter(|t| matches!(t.state, TaskState::Failed(_) | TaskState::Cancelled))
        .count()
}

/// Forget everything that has finished, leaving the running ones alone.
pub fn clear_finished() {
    with_tasks(|tasks| tasks.retain(|t| !t.state.is_finished()));
}

/// A running task. Report its outcome, or dropping it records that nobody did.
///
/// `Send` on purpose: the discovery coordinator runs on the Tokio runtime while
/// the status bar reads on the GTK thread.
#[must_use = "a task handle that is dropped immediately is recorded as cancelled"]
pub struct TaskHandle {
    id: u64,
}

impl TaskHandle {
    /// Say where the work has got to.
    ///
    /// The whole point: "Inspect x" spends most of its life somewhere specific
    /// — looking for a published manifest, waiting on a job — and a reader who
    /// can see WHICH can tell a slow probe from a stuck one.
    pub fn stage(&self, stage: impl Into<String>) {
        let stage = stage.into();
        with_tasks(|tasks| {
            if let Some(t) = tasks.iter_mut().find(|t| t.id == self.id) {
                t.stage = stage;
            }
        });
    }

    /// It worked.
    pub fn succeed(self) {
        self.finish(TaskState::Succeeded);
    }

    /// It did not, and this is why.
    pub fn fail(self, why: impl Into<String>) {
        self.finish(TaskState::Failed(why.into()));
    }

    fn finish(self, state: TaskState) {
        set_state(self.id, state);
        // Already recorded; skip the Drop that would call it Cancelled.
        std::mem::forget(self);
    }
}

fn set_state(id: u64, state: TaskState) {
    with_tasks(|tasks| {
        if let Some(t) = tasks.iter_mut().find(|t| t.id == id) {
            t.state = state;
            t.finished = Some(SystemTime::now());
        }
    });
}

impl Drop for TaskHandle {
    fn drop(&mut self) {
        // Nobody reported an outcome. That is itself worth recording: it means
        // the future was abandoned — a closed window, a cancelled dialog, a
        // path that returned early — and the alternative is a task that reads
        // as running for the rest of the session.
        set_state(self.id, TaskState::Cancelled);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tests share one global registry, so they take turns.
    static LOCK: Mutex<()> = Mutex::new(());

    fn fresh() -> std::sync::MutexGuard<'static, ()> {
        let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        with_tasks(|t| t.clear());
        guard
    }

    /// Every surface that does slow, user-visible work registers it.
    ///
    /// The status bar and `get_current_view`'s `activity` are only as complete
    /// as this list: a page that runs a network operation without a task is a
    /// page the app cannot report on, which is the state the whole registry was
    /// built to end. Listed explicitly rather than inferred, so ADDING a
    /// surface is a deliberate decision rather than an omission nobody notices.
    #[test]
    fn every_slow_surface_registers_its_work() {
        const REGISTERED: &[&str] = &[
            // the probe pipeline, stage by stage
            "services/image_discovery_coordinator.rs",
            // launching, relaunching, and acting on a session.
            //
            // The card and the recents list are what REGISTER: they own the
            // pressed button, so they create the `Working` that carries both
            // the spinner and the task. `dashboard.rs` only consumes one, which
            // is why it is not listed — it is the handler, not the source.
            "ui/launch_form.rs",
            "ui/session_card.rs",
            "ui/recent_launches.rs",
            // pulling manifests back from CANFAR storage
            "ui/canfar_images.rs",
            // uploads and deletes against VOSpace
            "ui/vospace_browser.rs",
        ];
        let sources = crate::testing::rust_sources();
        for expected in REGISTERED {
            let found = sources.iter().any(|(path, text)| {
                // Raw text, deliberately. `testing::code` cuts a file at its
                // first `#[cfg(test)]`, and the coordinator has a test-only
                // accessor near the top — which hides the whole pipeline below
                // it from any scan. That has now cost five separate guards a
                // false failure; when in doubt here, scan raw.
                path.to_string_lossy()
                    .replace('\\', "/")
                    .ends_with(expected)
                    && (text.contains("tasks::begin(") || text.contains("Working::start("))
            });
            assert!(
                found,
                "{expected} no longer registers its work, so neither the status \
                 bar nor `get_current_view` can report it"
            );
        }
    }

    #[test]
    fn a_dropped_handle_is_cancelled_not_left_running() {
        // The bug this exists to make impossible: a widget starts work, the
        // future is abandoned, and the app shows it as running forever.
        let _g = fresh();
        {
            let _h = begin(TaskKind::Discovery, "Inspect x");
            assert_eq!(running_count(), 1);
        }
        assert_eq!(running_count(), 0);
        let t = &snapshot()[0];
        assert_eq!(t.state, TaskState::Cancelled);
        assert!(t.finished.is_some());
    }

    #[test]
    fn an_outcome_wins_over_the_drop() {
        let _g = fresh();
        begin(TaskKind::Launch, "Launch a").succeed();
        begin(TaskKind::Launch, "Launch b").fail("no quota");
        let s = snapshot();
        assert_eq!(s[0].state, TaskState::Succeeded);
        assert_eq!(s[1].state, TaskState::Failed("no quota".into()));
        assert_eq!(running_count(), 0);
        assert_eq!(failed_count(), 1, "a success is not a failure");
    }

    #[test]
    fn stages_are_visible_while_it_runs() {
        let _g = fresh();
        let h = begin(TaskKind::Discovery, "Inspect y");
        h.stage("looking for a published manifest");
        assert_eq!(snapshot()[0].stage, "looking for a published manifest");
        h.stage("waiting for job vi-abc");
        assert_eq!(snapshot()[0].stage, "waiting for job vi-abc");
        h.succeed();
    }

    #[test]
    fn the_sequence_moves_on_every_change() {
        let _g = fresh();
        let before = sequence();
        let h = begin(TaskKind::Sync, "Sync");
        let after_begin = sequence();
        assert!(after_begin > before, "begin did not register");
        h.stage("one");
        assert!(sequence() > after_begin, "a stage change went unnoticed");
        let at_stage = sequence();
        h.succeed();
        assert!(sequence() > at_stage, "the outcome went unnoticed");
    }

    #[test]
    fn a_running_task_is_never_evicted_by_newer_work() {
        // A catalogue sweep is hundreds of probes. The cap must drop finished
        // entries, never the one thing the reader is waiting on.
        let _g = fresh();
        let keep = begin(TaskKind::Discovery, "the one still going");
        for i in 0..MAX_TASKS * 2 {
            begin(TaskKind::Discovery, format!("done {i}")).succeed();
        }
        let s = snapshot();
        assert!(s.len() <= MAX_TASKS, "the registry grew past its cap");
        assert!(
            s.iter().any(|t| t.label == "the one still going"),
            "the running task was evicted by finished ones"
        );
        keep.succeed();
    }

    #[test]
    fn clearing_leaves_the_running_ones() {
        let _g = fresh();
        let running = begin(TaskKind::Storage, "uploading");
        begin(TaskKind::Storage, "done").succeed();
        clear_finished();
        let s = snapshot();
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].label, "uploading");
        running.succeed();
    }
}
