//! Three dots that travel, for the moments the app is waiting on something else.
//!
//! A static "…" says the same thing, and says it whether or not anything is
//! still happening. These move only while they are told to, so a stopped
//! animation is itself information: the agent has gone quiet.
//!
//! Driven by the frame clock rather than a timer, so the motion is smooth at
//! whatever rate the display actually runs at, and stops dead when the dots are
//! hidden — a timer would keep firing behind a closed sidebar.

use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use std::cell::Cell;
use std::f64::consts::TAU;
use std::rc::Rc;

/// How many dots, how big, and how far apart — in logical pixels.
const DOTS: usize = 3;
const RADIUS: f64 = 2.0;
const SPACING: f64 = 7.0;
/// How far a dot rises above and falls below the line.
const TRAVEL: f64 = 2.5;
/// One full pass of the wave, in seconds. Slow enough to read as a wave rather
/// than a flicker.
const PERIOD: f64 = 1.15;

pub struct WorkingDots {
    area: gtk::DrawingArea,
    /// Whether the wave is running. The draw function reads it too, so a
    /// stopped animation parks the dots on the line instead of wherever the
    /// last frame left them.
    running: Rc<Cell<bool>>,
    /// Seconds since the wave started, advanced by the frame clock.
    phase: Rc<Cell<f64>>,
}

impl WorkingDots {
    pub fn new() -> Self {
        let area = gtk::DrawingArea::new();
        let width = SPACING * (DOTS - 1) as f64 + RADIUS * 2.0 + 2.0;
        area.set_content_width(width.ceil() as i32);
        area.set_content_height((TRAVEL * 2.0 + RADIUS * 2.0 + 2.0).ceil() as i32);
        area.set_valign(gtk::Align::Center);

        let running = Rc::new(Cell::new(false));
        let phase = Rc::new(Cell::new(0.0));

        {
            let running = running.clone();
            let phase = phase.clone();
            area.set_draw_func(move |area, cr, w, h| {
                // The widget's own colour, so the dots follow whatever CSS
                // class the caller put on them and the theme they are in.
                let c = area.color();
                cr.set_source_rgba(
                    f64::from(c.red()),
                    f64::from(c.green()),
                    f64::from(c.blue()),
                    f64::from(c.alpha()),
                );
                let mid = f64::from(h) / 2.0;
                let left = (f64::from(w) - (SPACING * (DOTS - 1) as f64)) / 2.0;
                for i in 0..DOTS {
                    // A third of a turn between neighbours, so the wave reads as
                    // travelling along the row rather than the three of them
                    // bouncing together.
                    let offset = if running.get() {
                        let turns = phase.get() / PERIOD;
                        let at = TAU * (turns - i as f64 / DOTS as f64);
                        -at.sin() * TRAVEL
                    } else {
                        0.0
                    };
                    cr.new_sub_path();
                    cr.arc(left + SPACING * i as f64, mid + offset, RADIUS, 0.0, TAU);
                }
                let _ = cr.fill();
            });
        }

        Self {
            area,
            running,
            phase,
        }
    }

    pub fn widget(&self) -> &gtk::DrawingArea {
        &self.area
    }

    /// Start or stop the wave.
    ///
    /// Starting twice is not two animations: the tick callback is only added on
    /// a real change, and removed on the way back down.
    pub fn set_running(&self, running: bool) {
        if running == self.running.get() {
            return;
        }
        self.running.set(running);
        if !running {
            self.phase.set(0.0);
            self.area.queue_draw();
            return;
        }
        // Respect the desktop's animation setting. Someone who has turned
        // animations off has said what they think of moving dots, and the
        // static three still say "working".
        if !animations_enabled() {
            self.area.queue_draw();
            return;
        }
        let flag = self.running.clone();
        let phase = self.phase.clone();
        let start = Cell::new(None::<i64>);
        self.area.add_tick_callback(move |area, clock| {
            if !flag.get() {
                return glib::ControlFlow::Break;
            }
            let now = clock.frame_time();
            let first = match start.get() {
                Some(t) => t,
                None => {
                    start.set(Some(now));
                    now
                }
            };
            // Frame time is microseconds since an arbitrary origin.
            phase.set((now - first) as f64 / 1_000_000.0);
            area.queue_draw();
            glib::ControlFlow::Continue
        });
    }
}

impl Default for WorkingDots {
    fn default() -> Self {
        Self::new()
    }
}

fn animations_enabled() -> bool {
    gtk::Settings::default().is_none_or(|s| s.is_gtk_enable_animations())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The three dots are out of phase with each other.
    ///
    /// In phase they bounce as a block, which reads as one thing jumping rather
    /// than as a wave passing along a row — and the wave is the whole idea.
    #[test]
    fn the_dots_travel_rather_than_bounce_together() {
        let at = |turns: f64, i: usize| -(TAU * (turns - i as f64 / DOTS as f64)).sin() * TRAVEL;
        for turns in [0.0, 0.12, 0.37, 0.6, 0.9] {
            let ys: Vec<f64> = (0..DOTS).map(|i| at(turns, i)).collect();
            let spread = ys.iter().cloned().fold(f64::MIN, f64::max)
                - ys.iter().cloned().fold(f64::MAX, f64::min);
            assert!(
                spread > 0.5,
                "at {turns} turns the dots are all within {spread:.2} px of each \
                 other, which is a bounce, not a wave"
            );
        }
    }

    /// The wave is bounded, and comes back to where it started.
    ///
    /// A phase that grew without bound would still draw correctly — sine is
    /// periodic — but the value it is computed from is seconds since the start,
    /// so this is what pins that it stays a wave rather than becoming a drift.
    #[test]
    fn a_dot_stays_within_its_travel_and_repeats() {
        let y = |t: f64| -(TAU * (t / PERIOD)).sin() * TRAVEL;
        for step in 0..400 {
            let t = f64::from(step) * 0.01;
            assert!(y(t).abs() <= TRAVEL + 1e-9, "at {t}s a dot left its travel");
        }
        assert!(
            (y(0.0) - y(PERIOD)).abs() < 1e-9,
            "one period does not return the dot to where it began"
        );
    }
}
