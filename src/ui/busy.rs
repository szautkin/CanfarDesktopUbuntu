//! "I heard you" — a button that shows it is working, in place.
//!
//! Every Portal action that goes to the network took the click and then looked
//! exactly as it had before: Relaunch on a recent launch, Renew and Delete on a
//! session card, Events. A launch takes a second or two to come back, and for
//! that whole time the only honest reading of the screen was that nothing had
//! happened — so people pressed again.
//!
//! The feedback belongs ON the control that was pressed, not in a status line
//! somewhere else: that is where the eye already is, and it is the only place
//! that says WHICH of four identical icon buttons is the one working.
//!
//! Held as a guard rather than started and stopped by hand, so an early return
//! or an error path cannot leave a button spinning forever — the button comes
//! back when the guard goes out of scope, whichever way it got there.

use gtk4::prelude::*;
use gtk4::{self as gtk};

/// A button held in its working state. Restores the button when dropped.
///
/// Keep it alive for as long as the work runs:
///
/// ```ignore
/// let _busy = Busy::start(&button);
/// do_the_slow_thing().await;
/// // button restored here
/// ```
pub struct Busy {
    button: gtk::Button,
    /// What the button was showing before, put back on drop.
    child: Option<gtk::Widget>,
    was_sensitive: bool,
    tooltip: Option<gtk4::glib::GString>,
}

impl Busy {
    /// Show `button` as working: a spinner in place of its label or icon, and
    /// insensitive so the same request cannot be sent twice.
    pub fn start(button: &gtk::Button) -> Self {
        let child = button.child();
        let was_sensitive = button.is_sensitive();
        let tooltip = button.tooltip_text();

        render_busy(button);

        Busy {
            button: button.clone(),
            child,
            was_sensitive,
            tooltip,
        }
    }
}

/// Spinner size, matching the symbolic icons these buttons carry.
const SPINNER_PX: i32 = 16;

/// Draw `button` as working: a spinner where its label or icon was, and
/// insensitive so the same request cannot be sent twice.
///
/// Public and restore-free because a REBUILT row has to render the same state:
/// a list that redraws while a probe is running would otherwise put back a
/// fresh, enabled "Inspect" button over work that is still going. `Busy` is
/// this plus the undo.
pub fn render_busy(button: &gtk::Button) {
    let spinner = gtk::Spinner::new();
    spinner.start();
    // Sized to the icon it replaces, so the button does not resize and the row
    // does not reflow under the pointer.
    spinner.set_size_request(SPINNER_PX, SPINNER_PX);
    spinner.set_halign(gtk::Align::Center);
    spinner.set_valign(gtk::Align::Center);

    button.set_child(Some(&spinner));
    button.set_sensitive(false);
    button.set_tooltip_text(Some(crate::tr_en!("Working…")));
}

impl Drop for Busy {
    fn drop(&mut self) {
        self.button.set_child(self.child.as_ref());
        self.button.set_sensitive(self.was_sensitive);
        self.button.set_tooltip_text(self.tooltip.as_deref());
    }
}

/// A button that is working, and the task that work is.
///
/// Two things always went together at these call sites — the pressed control
/// showing it was busy, and the registry knowing the work exists — and they
/// were threaded separately: `Busy` handed through the callback, `tasks::begin`
/// called somewhere else. Two mechanisms for one fact, which is two chances for
/// them to disagree, and it showed: the session card's buttons had a spinner
/// and no task, so the status bar could not see them at all.
///
/// One guard. Report the outcome or drop it; the button comes back and the task
/// is recorded either way.
#[must_use = "dropping this immediately restores the button and records the work as cancelled"]
pub struct Working {
    /// Restores the button on drop. Order matters: declared first so it is
    /// dropped last, after the task's outcome is recorded.
    _busy: Busy,
    task: Option<crate::helpers::tasks::TaskHandle>,
}

impl Working {
    pub fn start(
        button: &gtk::Button,
        kind: crate::helpers::tasks::TaskKind,
        label: impl Into<String>,
    ) -> Self {
        Working {
            _busy: Busy::start(button),
            task: Some(crate::helpers::tasks::begin(kind, label)),
        }
    }

    /// Say where the work has got to.
    pub fn stage(&self, stage: impl Into<String>) {
        if let Some(task) = &self.task {
            task.stage(stage);
        }
    }

    /// It worked.
    pub fn succeed(mut self) {
        if let Some(task) = self.task.take() {
            task.succeed();
        }
    }

    /// It did not, and this is why.
    pub fn fail(mut self, why: impl Into<String>) {
        if let Some(task) = self.task.take() {
            task.fail(why);
        }
    }

    /// Record the outcome of a `Result` without unwrapping it at the call site.
    pub fn finish<T, E: std::fmt::Display>(self, result: &Result<T, E>) {
        match result {
            Ok(_) => self.succeed(),
            Err(e) => self.fail(e.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    const SOURCE: &str = include_str!("busy.rs");

    /// The restore has to be a `Drop`, not a call at the end of the happy path.
    ///
    /// Every one of these buttons is used from an async block with early
    /// returns — a cancelled confirmation, a missing token, a failed request.
    /// A restore written as the last statement is skipped by all of them, and
    /// the button is left disabled with a spinner in it for the life of the
    /// window.
    /// A button-initiated action reports itself in one place, not two.
    ///
    /// `Busy` (the spinner) and `tasks::begin` (the registry entry) were
    /// threaded through these call sites separately, so they could — and did —
    /// diverge: the session card's buttons had a spinner and no registry entry,
    /// which meant the one screen that answers "what is the app doing" could
    /// not see four of the Portal's actions.
    #[test]
    fn a_button_action_carries_its_task_with_it() {
        let mut split: Vec<String> = Vec::new();
        for (path, text) in crate::testing::rust_sources() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name == "busy.rs" {
                continue; // where the two are joined
            }
            let code = crate::testing::without_comments(crate::testing::code(&text));
            if code.contains("busy::Busy::start(") {
                split.push(name.to_string());
            }
        }
        assert!(
            split.is_empty(),
            "a button is made busy without the work being registered, so the \
             status bar cannot see it: {split:?}"
        );
    }

    #[test]
    fn the_button_is_restored_by_a_guard_not_by_the_happy_path() {
        let code = crate::testing::without_comments(crate::testing::code(SOURCE));
        assert!(
            code.contains("impl Drop for Busy"),
            "Busy no longer restores the button on drop, so any early return \
             leaves it spinning"
        );
        assert!(
            code.contains("_busy: Busy,"),
            "Working no longer holds a Busy, so the button is not restored"
        );
        for restored in [
            "self.button.set_child(",
            "self.button.set_sensitive(",
            "self.button.set_tooltip_text(",
        ] {
            assert!(code.contains(restored), "drop does not put back {restored}");
        }
    }
}
