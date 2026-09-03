//! How late a notification can be, before and after the adaptive cadence.
//!
//! The Portal learns that a job finished or a session came up only by polling,
//! so the poll schedule IS the notification delay. This walks both schedules
//! against a job that finishes at every second of a long run and reports the
//! distribution of "how long after the event did the user hear about it".
//!
//! Run: `cargo run --example notification_delay_probe`

use verbinal::ui::poll::{Cadence, BUSY_SECS, IDLE_SECS, JOBS_WATCH_SECS, SESSION_WATCH_SECS};

/// The poll instants of a fixed-interval schedule, out to `horizon` seconds.
fn fixed_schedule(interval: u32, horizon: u32) -> Vec<u32> {
    (1..)
        .map(|n| n * interval)
        .take_while(|t| *t <= horizon)
        .collect()
}

/// The poll instants of the adaptive schedule, out to `horizon` seconds.
///
/// Modelled as the real loop runs it: work is in flight for the whole window
/// (the job has not finished yet), and nothing else is moving, so every poll
/// after the first sees no change and the cadence eases off.
fn adaptive_schedule(ceiling: u32, horizon: u32) -> Vec<u32> {
    let mut cadence = Cadence::new(ceiling);
    let mut t = 0;
    let mut out = Vec::new();
    loop {
        t += cadence.secs();
        if t > horizon {
            return out;
        }
        out.push(t);
        cadence.observe(true, false);
    }
}

/// How long after an event at `event` the next poll happens.
fn delay(schedule: &[u32], event: u32) -> u32 {
    schedule
        .iter()
        .find(|t| **t >= event)
        .map(|t| t - event)
        .expect("horizon too short")
}

fn report(what: &str, before: u32, ceiling: u32, horizon: u32) {
    let old = fixed_schedule(before, horizon + before);
    let new = adaptive_schedule(ceiling, horizon + before);

    let mut old_total = 0u64;
    let mut new_total = 0u64;
    let mut old_worst = 0;
    let mut new_worst = 0;
    // Events where this particular new poll instant happens to fall further
    // from the event than the old one did. Not a regression — the two are
    // different grids, and a fixed 45s grid lands right next to *some* events
    // by luck. What matters is the guarantee, which is the worst case.
    let mut unluckier = 0;

    for event in 0..horizon {
        let o = delay(&old, event);
        let n = delay(&new, event);
        if n > o {
            unluckier += 1;
        }
        old_total += o as u64;
        new_total += n as u64;
        old_worst = old_worst.max(o);
        new_worst = new_worst.max(n);
    }

    let n = horizon as u64;
    println!("\n{what}  (event anywhere in the first {horizon}s of a watch)");
    println!(
        "  polls in that window   before {:>4}   after {:>4}",
        old.len(),
        new.len()
    );
    println!("  worst-case delay       before {old_worst:>3}s   after {new_worst:>3}s");
    println!(
        "  mean delay             before {:>3}s   after {:>3}s",
        old_total / n,
        new_total / n
    );
    println!(
        "  unluckier phase        {unluckier} of {horizon} events, none past the old {before}s bound"
    );

    // The user-facing guarantee: nothing waits longer than the app's old worst
    // case. Phase can make an individual event unluckier; it can never push one
    // past the bound the app already promised.
    assert!(
        new_worst <= old_worst,
        "{what}: worst-case delay rose from {old_worst}s to {new_worst}s"
    );
    assert!(
        new_total <= old_total,
        "{what}: mean delay rose"
    );
}

fn main() {
    println!(
        "cadence: {BUSY_SECS}s after a change, doubling to the ceiling, \
         {IDLE_SECS}s when nothing is in flight"
    );

    // The Batch Jobs card. A job the user submits and waits on; five minutes is
    // a typical short run, and the delay is what they notice.
    report("Batch Jobs card", 45, JOBS_WATCH_SECS, 300);

    // The session strip. Only polls while a session is pending, which is
    // usually under two minutes.
    report("Session strip", 15, SESSION_WATCH_SECS, 120);

    // The long tail: a job left running for an hour. This is the case the
    // ceiling exists to protect — the cost of watching, not the delay.
    report("Batch Jobs card, one-hour job", 45, JOBS_WATCH_SECS, 3600);
}
