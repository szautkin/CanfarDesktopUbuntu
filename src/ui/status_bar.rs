//! What the app is doing, along the bottom of the window.
//!
//! Collapsed it is one line — moving dots and a count — so an idle app costs a
//! strip of chrome and nothing else. Expanded it lists every task the registry
//! knows about with the stage it has reached, how long it has been there, and,
//! for the ones that failed, why.
//!
//! It exists because the app had no answer to "is anything happening?". Each
//! operation reported itself locally or not at all, so a probe that was three
//! minutes into waiting on a Skaha job and one that had failed to submit looked
//! identical from outside the widget that started them — and if that widget was
//! a dialog you had closed, they looked like nothing at all.
//!
//! Reads the registry on a tick and rebuilds only when its sequence moves, so
//! an idle app does no work per frame.

use crate::helpers::tasks::{self, Task, TaskState};
use crate::ui::space;
use crate::ui::working_dots::WorkingDots;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use std::cell::Cell;
use std::rc::Rc;

/// How often to look at the registry's sequence number.
///
/// Cheap enough to be frequent — one atomic load — and the list is only rebuilt
/// when that number has actually moved.
const TICK_MS: u32 = 500;

/// How tall the expanded list may get before it scrolls.
const LIST_MAX_HEIGHT: i32 = 220;

pub struct StatusBar {
    widget: gtk::Box,
    summary: gtk::Label,
    failures: gtk::Label,
    dots: WorkingDots,
    list_box: gtk::ListBox,
    revealer: gtk::Revealer,
    /// The sequence the rendered list was built from.
    rendered_seq: Cell<u64>,
    expanded: Cell<bool>,
}

impl StatusBar {
    pub fn new() -> Rc<Self> {
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);

        // ── The one-line summary ──
        let row = gtk::Box::new(gtk::Orientation::Horizontal, space::CONTROL);
        row.set_margin_start(space::EDGE);
        row.set_margin_end(space::EDGE);
        row.set_margin_top(space::ROW);
        row.set_margin_bottom(space::ROW);

        let dots = WorkingDots::new();
        row.append(dots.widget());

        let summary = gtk::Label::new(Some(crate::tr_en!("Idle")));
        summary.add_css_class("caption");
        summary.add_css_class("dim-label");
        summary.set_halign(gtk::Align::Start);
        summary.set_hexpand(true);
        row.append(&summary);

        // Kept visible only when there is something to say. A permanent
        // "0 failed" is noise; an appearing one is information.
        let failures = gtk::Label::new(None);
        failures.add_css_class("caption");
        failures.add_css_class("error");
        failures.set_visible(false);
        row.append(&failures);

        let toggle = gtk::Button::from_icon_name("pan-up-symbolic");
        toggle.add_css_class("flat");
        toggle.set_tooltip_text(Some(crate::tr_en!("Show what the app is doing")));
        row.append(&toggle);

        widget.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        widget.append(&row);

        // ── The expandable list ──
        let list_box = gtk::ListBox::new();
        list_box.set_selection_mode(gtk::SelectionMode::None);
        list_box.add_css_class("boxed-list");
        space::inset(&list_box, space::EDGE);

        let scroller = gtk::ScrolledWindow::new();
        scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroller.set_propagate_natural_height(true);
        scroller.set_max_content_height(LIST_MAX_HEIGHT);
        scroller.set_child(Some(&list_box));

        let revealer = gtk::Revealer::new();
        revealer.set_transition_type(gtk::RevealerTransitionType::SlideUp);
        revealer.set_child(Some(&scroller));
        revealer.set_reveal_child(false);
        widget.append(&revealer);

        let bar = Rc::new(StatusBar {
            widget,
            summary,
            failures,
            dots,
            list_box,
            revealer,
            rendered_seq: Cell::new(u64::MAX),
            expanded: Cell::new(false),
        });

        {
            let bar = bar.clone();
            let toggle = toggle.clone();
            toggle.connect_clicked(move |btn| {
                let open = !bar.expanded.get();
                bar.expanded.set(open);
                bar.revealer.set_reveal_child(open);
                btn.set_icon_name(if open {
                    "pan-down-symbolic"
                } else {
                    "pan-up-symbolic"
                });
                // Force a rebuild: the list is not kept up to date while hidden.
                bar.rendered_seq.set(u64::MAX);
                bar.refresh();
            });
        }

        // One long-lived tick. A weak ref lets it stop if the bar ever goes.
        {
            let weak = Rc::downgrade(&bar);
            glib::timeout_add_local(
                std::time::Duration::from_millis(TICK_MS as u64),
                move || match weak.upgrade() {
                    Some(bar) => {
                        bar.refresh();
                        glib::ControlFlow::Continue
                    }
                    None => glib::ControlFlow::Break,
                },
            );
        }

        bar.refresh();
        bar
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.widget
    }

    /// Re-read the registry, and rebuild the list only if it changed.
    fn refresh(self: &Rc<Self>) {
        let running = tasks::running_count();
        let failed = tasks::failed_count();

        self.summary.set_text(&if running == 0 {
            crate::tr_en!("Idle").to_string()
        } else {
            crate::tr_plural!(running, "{} task running", "{} tasks running")
        });
        self.dots.set_running(running > 0);

        self.failures.set_visible(failed > 0);
        if failed > 0 {
            self.failures
                .set_text(&crate::tr_plural!(failed, "{} failed", "{} failed"));
        }

        // The list is only worth rebuilding when it is on screen AND something
        // has actually changed.
        let seq = tasks::sequence();
        if !self.expanded.get() || seq == self.rendered_seq.get() {
            return;
        }
        self.rendered_seq.set(seq);
        self.rebuild_list();
    }

    fn rebuild_list(self: &Rc<Self>) {
        while let Some(child) = self.list_box.first_child() {
            self.list_box.remove(&child);
        }

        let mut snapshot = tasks::snapshot();
        // Newest first: what just happened is what a reader is looking for.
        snapshot.reverse();

        if snapshot.is_empty() {
            let empty =
                gtk::Label::new(Some(crate::tr_en!("Nothing has run yet in this session.")));
            empty.add_css_class("dim-label");
            space::inset(&empty, space::CARD);
            let row = gtk::ListBoxRow::new();
            row.set_child(Some(&empty));
            self.list_box.append(&row);
            return;
        }

        for task in &snapshot {
            self.list_box.append(&task_row(task));
        }

        let clear = gtk::Button::with_label(crate::tr_en!("Clear finished"));
        clear.add_css_class("flat");
        space::inset(&clear, space::ROW);
        {
            let bar = self.clone();
            clear.connect_clicked(move |_| {
                tasks::clear_finished();
                bar.rendered_seq.set(u64::MAX);
                bar.refresh();
            });
        }
        let row = gtk::ListBoxRow::new();
        row.set_child(Some(&clear));
        self.list_box.append(&row);
    }
}

/// One task, as a row: what it is, where it got to, and how long that took.
fn task_row(task: &Task) -> gtk::Widget {
    let title = &task.label;
    let elapsed = format_elapsed(task.elapsed());
    // Composition, not prose: each PART is translated, the separator is
    // punctuation. Wrapping the whole thing in `tr_fmt!` only put "{} · {}" in
    // the translation table, where it means nothing to a translator.
    let state_text = match &task.state {
        TaskState::Running if task.stage.is_empty() => task.kind.label().to_string(),
        TaskState::Running => task.stage.clone(),
        TaskState::Succeeded => crate::tr_en!("done").to_string(),
        // The reason, not just the word: a failure with no reason is what this
        // whole module was built to stop.
        TaskState::Failed(why) => why.clone(),
        TaskState::Cancelled => crate::tr_en!("abandoned").to_string(),
    };
    let subtitle = format!("{state_text}  ·  {elapsed}");

    let dot = gtk::Label::new(Some("\u{25cf}"));
    dot.set_valign(gtk::Align::Center);
    dot.add_css_class(match &task.state {
        TaskState::Running => "accent",
        TaskState::Succeeded => "success",
        TaskState::Failed(_) => "error",
        TaskState::Cancelled => "dim-label",
    });

    // The failure text can be long — it carries the tail of a job's own words —
    // so the shared reason row keeps it collapsed with the full text one click
    // away, exactly as the Batch Jobs history does.
    let detail = match &task.state {
        TaskState::Failed(why) => why.as_str(),
        _ => "",
    };
    crate::ui::failure_detail::reason_row(title, &subtitle, detail, Some(&dot.upcast()))
}

/// Compact elapsed time: `4s`, `2m 10s`, `1h 03m`.
fn format_elapsed(d: std::time::Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m {:02}s", secs / 60, secs % 60)
    } else {
        format!("{}h {:02}m", secs / 3600, (secs % 3600) / 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elapsed_reads_at_every_scale() {
        assert_eq!(format_elapsed(std::time::Duration::from_secs(4)), "4s");
        assert_eq!(format_elapsed(std::time::Duration::from_secs(59)), "59s");
        assert_eq!(
            format_elapsed(std::time::Duration::from_secs(130)),
            "2m 10s"
        );
        assert_eq!(
            format_elapsed(std::time::Duration::from_secs(3780)),
            "1h 03m"
        );
    }

    #[test]
    fn the_list_is_only_rebuilt_when_it_is_visible_and_stale() {
        // The bar ticks twice a second for the life of the app. Rebuilding a
        // list nobody has opened, or one nothing has changed, would be a
        // per-frame cost paid by every screen in the app.
        let code =
            crate::testing::without_comments(crate::testing::code(include_str!("status_bar.rs")));
        let at = code.find("fn refresh").expect("refresh is gone");
        let body = &code[at..];
        assert!(
            body.contains("if !self.expanded.get() || seq == self.rendered_seq.get()"),
            "the status bar rebuilds its list unconditionally"
        );
    }
}
