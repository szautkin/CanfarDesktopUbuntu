//! How often the app asks CANFAR what changed.
//!
//! The session strip and the Batch Jobs card both learn about events by
//! polling, and every notification they raise is a side effect of a poll. So
//! the poll interval IS the notification delay: a session that came up, or a
//! job that failed, was announced up to 15 or 45 seconds after it happened —
//! long enough that the notification reads as being about something else.
//!
//! Worse, a job that started and finished inside one 45-second window was never
//! seen in a non-terminal state at all, so no transition was detected and no
//! notification was ever raised.
//!
//! Polling flat-out is not the answer either. A headless job can run for hours,
//! and a fixed five-second interval would ask about it some two thousand times
//! to deliver one notification — on a shared platform, at a cost paid by
//! everyone.
//!
//! So the cadence follows the evidence. Something just changed, or was just
//! submitted? The user is in a live moment and the next change is probably
//! close, so ask again soon. Poll after poll of nothing? Ease off, doubling
//! each time, up to a ceiling. Nothing in flight at all — no pending session,
//! no unfinished job? There is no notification to be timely about, so drop to
//! [`IDLE_SECS`] and stop spending requests on it.
//!
//! The ceiling is per-surface and is at most the interval that surface already
//! ran at, which makes the change one-directional: no notification can arrive
//! later than it would have before, and the ones near a real event arrive much
//! sooner.

use gtk4::glib;
use gtk4::{self as gtk};

/// The quickest the app will ask, and so the shortest a notification delay can
/// be. Used for the poll right after something changed.
pub const BUSY_SECS: u32 = 5;

/// Ceiling for the Batch Jobs card while a job of the user's is still running.
///
/// Below the 45 seconds the card used to run at unconditionally, which is the
/// interval that made a finished job's notification read as being about
/// something else. A job can run for hours, so this is also the steady-state
/// cost of watching one: one list call every twenty seconds, and only while
/// there is something to watch.
pub const JOBS_WATCH_SECS: u32 = 20;

/// Ceiling for the session strip while a session is pending.
///
/// Well under the jobs ceiling, and under the flat 15s the strip used to run
/// at, because this loop only runs while something is pending — a bounded
/// window, usually a minute or two, at the end of which sits the single
/// most-awaited notification in the app. Someone is watching the screen for it,
/// so the extra handful of list calls buys the thing worth buying.
pub const SESSION_WATCH_SECS: u32 = 8;

/// The interval when nothing is in flight at all.
///
/// No pending session and no unfinished job means no transition to report, so
/// there is nothing to be timely about and a frequent poll is pure load. It
/// still happens, because a session can be launched from another machine.
pub const IDLE_SECS: u32 = 45;

/// How long to wait before asking again.
///
/// A value rather than a bare number so the backoff rule lives in one place —
/// both pollers need it and neither should be reimplementing the arithmetic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cadence {
    secs: u32,
    /// The slowest this surface will ask while something is still in flight.
    watch_ceiling: u32,
}

impl Cadence {
    /// A cadence that eases off to `watch_ceiling` while work is in flight.
    ///
    /// Starts quick: the first poll is what discovers whether anything is in
    /// flight, and starting slow would mean starting slow on exactly the case
    /// that needs to be fast — the one where the user just launched something
    /// and is watching for it.
    pub fn new(watch_ceiling: u32) -> Self {
        Cadence {
            secs: BUSY_SECS.min(watch_ceiling),
            watch_ceiling,
        }
    }

    /// Seconds to wait before the next poll.
    pub fn secs(self) -> u32 {
        self.secs
    }

    /// Fold in what the poll just saw.
    ///
    /// `in_flight`: can anything still change state — is a notification even
    /// possible? `changed`: did anything actually move since last time?
    pub fn observe(&mut self, in_flight: bool, changed: bool) {
        self.secs = if !in_flight {
            IDLE_SECS
        } else if changed {
            BUSY_SECS.min(self.watch_ceiling)
        } else {
            (self.secs * 2).min(self.watch_ceiling)
        };
    }
}

/// Count `secs` down in `label`, a second at a time.
///
/// Shared because both pollers drew the same countdown and each had its own
/// copy of the loop; with the interval now varying, two copies would be two
/// places to get the new arithmetic wrong.
pub async fn countdown(label: &gtk::Label, secs: u32) {
    for remaining in (1..=secs).rev() {
        label.set_text(&crate::tr_fmt!("refresh in {}s", remaining));
        glib::timeout_future_seconds(1).await;
    }
    label.set_text(crate::tr_en!("refreshing…"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_poll_comes_quickly() {
        // A freshly built card must not open with a 45-second wait: the common
        // case is that it was built because the user just arrived at the Portal
        // to look at something.
        assert_eq!(Cadence::new(JOBS_WATCH_SECS).secs(), BUSY_SECS);
        assert_eq!(Cadence::new(SESSION_WATCH_SECS).secs(), BUSY_SECS);
    }

    #[test]
    fn a_change_puts_it_back_on_the_fast_lane() {
        let mut c = Cadence::new(JOBS_WATCH_SECS);
        c.observe(true, false);
        c.observe(true, false);
        assert!(c.secs() > BUSY_SECS, "it never eased off");

        // A job just finished, or a new one appeared. Something else is likely
        // about to happen, and this is when delay is most visible.
        c.observe(true, true);
        assert_eq!(c.secs(), BUSY_SECS);
    }

    #[test]
    fn a_quiet_watch_eases_off_to_its_ceiling() {
        // A headless job can run for hours. Asking every five seconds for all
        // of it would be thousands of requests to deliver one notification.
        for ceiling in [JOBS_WATCH_SECS, SESSION_WATCH_SECS] {
            let mut c = Cadence::new(ceiling);
            for _ in 0..20 {
                c.observe(true, false);
                assert!(
                    c.secs() <= ceiling,
                    "backoff overshot its ceiling to {}",
                    c.secs()
                );
            }
            assert_eq!(c.secs(), ceiling, "backoff settled below its ceiling");
        }
    }

    #[test]
    fn no_notification_can_arrive_later_than_it_used_to() {
        // The point of the whole change. Each surface's ceiling is at most the
        // fixed interval it already ran at — 45s for the Batch Jobs card, 15s
        // for the session strip — so every reachable interval, from any history
        // of polls, is one the surface would have used anyway.
        const WAS_FIXED_AT: [(u32, u32); 2] = [(JOBS_WATCH_SECS, 45), (SESSION_WATCH_SECS, 15)];
        for (ceiling, before) in WAS_FIXED_AT {
            let mut c = Cadence::new(ceiling);
            for step in 0..50 {
                // Alternate so both the eased and the quickened branches are
                // exercised from every reachable state.
                c.observe(true, step % 7 == 0);
                assert!(
                    c.secs() <= before,
                    "a notification can now be {}s late, up from {before}s",
                    c.secs()
                );
            }
        }
    }

    #[test]
    fn nothing_in_flight_costs_no_more_than_it_used_to() {
        // No pending session and no unfinished job: nothing to report, so the
        // poll buys nothing and should cost what the slowest surface always
        // cost. Anything faster here would be a sustained load increase for
        // zero benefit — an idle Portal left open all day.
        let mut c = Cadence::new(JOBS_WATCH_SECS);
        c.observe(false, false);
        assert_eq!(c.secs(), IDLE_SECS);
    }

    /// The two surfaces that poll CANFAR and notify from what they see.
    const POLLERS: [(&str, &str); 2] = [
        ("session_list.rs", include_str!("session_list.rs")),
        ("batch_jobs_view.rs", include_str!("batch_jobs_view.rs")),
    ];

    #[test]
    fn every_poller_takes_its_interval_from_here() {
        // A hard-coded interval in a poller is the bug this module exists to
        // fix: it is silently also the notification delay, and the two facts do
        // not look related at the call site. Scanning raw source on purpose —
        // `testing::code` cuts a file at its first `#[cfg(test)]`.
        for (name, src) in POLLERS {
            assert!(
                src.contains("poll::Cadence::new("),
                "{name} polls on some interval of its own rather than a Cadence"
            );
            assert!(
                src.contains("cadence.observe("),
                "{name} never folds in what it saw, so its cadence never adapts"
            );
        }
    }

    #[test]
    fn a_background_poll_does_not_announce_itself() {
        // The pollers now run for the life of the window and as often as every
        // few seconds. Anything they draw for a *user-initiated* refresh — a
        // spinner, an "unreachable" toast — becomes, on that schedule, the app
        // flickering and interrupting on its own. `load(false)` is what keeps
        // the quiet path quiet; `refresh()` is the announced one.
        for (name, src) in POLLERS {
            assert!(
                src.contains("load(false).await"),
                "{name}'s poll loop announces itself on every tick"
            );
            assert!(
                src.contains("self.load(true).await"),
                "{name} no longer has an announced refresh for user actions"
            );
        }
    }

    #[test]
    fn the_floor_still_checks_back() {
        // Backing off is not the same as stopping: a session can be started
        // from another machine, or a job reaped, and the Portal has to notice
        // eventually. A compile-time check, because the operands are consts and
        // there is no reason to wait for a test run to find this out.
        const { assert!(IDLE_SECS <= 60, "an idle Portal has stopped looking") };
    }
}
