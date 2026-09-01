//! A watchdog for the one thread that must never be busy.
//!
//! GTK is single-threaded: every widget, every draw, every click and every
//! `spawn_future_local` share one thread. Anything that occupies it for longer
//! than a frame is a stutter, and anything that occupies it for longer than a
//! moment is what a person calls "the app hung".
//!
//! Nothing reports that on its own. A blocking read, a parse of a large
//! response, a rebuild of a few thousand widgets — none of them raise an error;
//! the window simply stops repainting and starts again later. So this measures
//! it: a timer that should fire every [`INTERVAL`], and a note of how late it
//! actually was.
//!
//! Off unless `VERBINAL_WATCH_MAIN_THREAD` is set, because a stall detector
//! that is itself always running is one more thing on the thread it is
//! watching.

use gtk4::glib;
use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

/// How often the watchdog expects to be called.
///
/// Short enough to catch a stall a person would notice, long enough that the
/// check itself is nothing: a timer at 100 ms costs ten wakeups a second.
pub const INTERVAL: Duration = Duration::from_millis(100);

/// A gap longer than this is reported.
///
/// Two frames at 60 Hz is 33 ms and nobody sees it. A quarter of a second is
/// where a click stops feeling connected to what it did.
pub const STALL: Duration = Duration::from_millis(250);

/// How late a tick was, given when it was expected.
///
/// Split out from the timer so the arithmetic is testable: the interesting part
/// is the subtraction, and a test cannot run a GTK main loop.
pub fn lateness(elapsed: Duration) -> Option<Duration> {
    elapsed
        .checked_sub(INTERVAL)
        .filter(|late| *late + INTERVAL >= STALL)
}

/// Start watching, if the environment asks for it.
///
/// Returns whether it did, so the caller can say so rather than leaving the
/// person wondering whether the variable took.
pub fn start_if_asked() -> bool {
    if std::env::var_os("VERBINAL_WATCH_MAIN_THREAD").is_none() {
        return false;
    }
    let last = Rc::new(Cell::new(Instant::now()));
    let worst = Rc::new(Cell::new(Duration::ZERO));
    glib::timeout_add_local(INTERVAL, move || {
        let now = Instant::now();
        let elapsed = now.duration_since(last.get());
        last.set(now);
        if let Some(late) = lateness(elapsed) {
            if late > worst.get() {
                worst.set(late);
            }
            eprintln!(
                "[main-thread] blocked for {:.0} ms (worst so far {:.0} ms)",
                elapsed.as_secs_f64() * 1000.0,
                (worst.get() + INTERVAL).as_secs_f64() * 1000.0,
            );
        }
        glib::ControlFlow::Continue
    });
    eprintln!(
        "[main-thread] watching; anything over {} ms will be reported",
        STALL.as_millis()
    );
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tick that arrives on time is not a stall.
    #[test]
    fn punctual_ticks_say_nothing() {
        assert_eq!(lateness(INTERVAL), None);
        assert_eq!(lateness(INTERVAL + Duration::from_millis(20)), None);
    }

    /// A gap at the threshold is reported, and reported as the whole gap.
    ///
    /// The number a person cares about is how long the window was frozen, not
    /// how much of that was over budget — so the threshold is applied to the
    /// gap and the report quotes the gap.
    #[test]
    fn a_quarter_second_gap_is_a_stall() {
        assert!(lateness(STALL).is_some());
        assert!(lateness(Duration::from_secs(3)).is_some());
        assert_eq!(
            lateness(Duration::from_secs(3)).map(|l| l + INTERVAL),
            Some(Duration::from_secs(3))
        );
    }

    /// A tick that arrives EARLY is not an underflow.
    ///
    /// Monotonic clocks do not go backwards, but a timer coalesced by the
    /// scheduler can fire fractionally early, and `Duration` subtraction panics
    /// rather than going negative.
    #[test]
    fn an_early_tick_is_not_a_panic() {
        assert_eq!(lateness(Duration::from_millis(1)), None);
        assert_eq!(lateness(Duration::ZERO), None);
    }
}
