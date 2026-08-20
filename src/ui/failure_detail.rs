//! How a failure reason is shown, in the one place both callers read it from.
//!
//! A recorded failure is a short diagnosis followed by the tail of the job's
//! logs and events — useful, and far too long for a subtitle. Two screens show
//! it (the Batch Jobs history and the image-discovery detail pane), and they
//! were about to grow two different treatments: one row that clipped it and one
//! that let it push the dialog off the screen.

use crate::helpers::job_diagnostics::{has_detail, summary_line};
use gtk4::prelude::*;
use gtk4::{self as gtk};
use libadwaita as adw;
use libadwaita::prelude::*;

/// Height of the scrollable excerpt. Tall enough for a short traceback, short
/// enough that the row it lives in is still a row.
const DETAIL_HEIGHT: i32 = 180;

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
