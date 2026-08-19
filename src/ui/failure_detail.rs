//! How a failure reason is shown, in the one place both callers read it from.
//!
//! A recorded failure is a short diagnosis followed by the tail of the job's
//! logs and events — useful, and far too long for a subtitle. Two screens show
//! it (the Batch Jobs history and the image-discovery detail pane), and they
//! were about to grow two different treatments: one row that clipped it and one
//! that let it push the dialog off the screen.

use gtk4::prelude::*;
use gtk4::{self as gtk};
use libadwaita as adw;
use libadwaita::prelude::*;

/// Height of the scrollable excerpt. Tall enough for a short traceback, short
/// enough that the row it lives in is still a row.
const DETAIL_HEIGHT: i32 = 180;

/// The first line of a reason — the diagnosis, without the evidence.
///
/// Every reason is written summary-first precisely so this is meaningful.
pub fn summary_line(reason: &str) -> &str {
    reason
        .trim()
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
}

/// Whether a reason has anything beyond its first line worth expanding.
pub fn has_detail(reason: &str) -> bool {
    reason
        .trim()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .count()
        > 1
}

/// The full reason, monospaced, selectable, and scrolling inside its own box.
///
/// Monospace and selectable because this is program output to be read and
/// pasted into a bug report, not prose.
pub fn detail_view(reason: &str) -> gtk::ScrolledWindow {
    let text = gtk::TextView::new();
    text.set_editable(false);
    text.set_monospace(true);
    text.set_wrap_mode(gtk::WrapMode::WordChar);
    text.buffer().set_text(reason.trim());
    crate::ui::space::inset(&text, crate::ui::space::CARD);

    let scroll = gtk::ScrolledWindow::new();
    scroll.set_min_content_height(DETAIL_HEIGHT);
    scroll.set_child(Some(&text));
    scroll
}

/// A row for `reason`: expandable when there is evidence under the diagnosis,
/// a plain row when there is not.
///
/// `prefix`, when given, is placed before the title — a status dot, usually.
pub fn reason_row(
    title: &str,
    subtitle: &str,
    reason: &str,
    prefix: Option<&gtk::Widget>,
) -> gtk::Widget {
    if !has_detail(reason) {
        let row = adw::ActionRow::builder()
            .title(title)
            .subtitle(if subtitle.is_empty() {
                summary_line(reason)
            } else {
                subtitle
            })
            .build();
        row.set_subtitle_lines(0);
        row.set_title_lines(0);
        if let Some(widget) = prefix {
            row.add_prefix(widget);
        }
        return row.upcast();
    }

    let row = adw::ExpanderRow::builder()
        .title(title)
        .subtitle(if subtitle.is_empty() {
            summary_line(reason)
        } else {
            subtitle
        })
        .build();
    if let Some(widget) = prefix {
        row.add_prefix(widget);
    }
    row.add_row(&detail_view(reason));
    row.upcast()
}

#[cfg(test)]
mod tests {
    use super::*;

    const REASON: &str = "job ended in failed state: Failed\n\n\
                          --- job logs ---\nbash: syft: command not found\n";

    #[test]
    fn the_summary_is_the_diagnosis_alone() {
        assert_eq!(summary_line(REASON), "job ended in failed state: Failed");
    }

    #[test]
    fn a_leading_blank_line_does_not_become_the_summary() {
        // The reason is assembled by joining sections, and a leading newline is
        // one refactor away. An empty summary reads as "no reason recorded".
        assert_eq!(summary_line("\n\nreal reason\nmore"), "real reason");
    }

    #[test]
    fn an_empty_reason_summarises_to_nothing_rather_than_panicking() {
        assert_eq!(summary_line(""), "");
        assert_eq!(summary_line("   \n  "), "");
    }

    #[test]
    fn evidence_under_the_diagnosis_is_what_makes_a_row_expandable() {
        assert!(has_detail(REASON));
        assert!(!has_detail("Probe submit failed: not signed in"));
        assert!(!has_detail(""));
        // Blank lines are not evidence.
        assert!(!has_detail("one line\n\n\n"));
    }
}
