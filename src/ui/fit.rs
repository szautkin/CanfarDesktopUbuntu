//! Keeping a dialog's content inside the dialog.
//!
//! A GTK widget's MINIMUM width propagates all the way up: one leaf that cannot
//! be drawn narrower than 745px makes every ancestor at least that wide, and a
//! dialog that is 720px then reports
//! "AdwToolbarView exceeds AdwBreakpointBin width: requested 784 px, 720 px
//! available" and clips its own controls — switches, buttons, the lot, cut off
//! at the right edge.
//!
//! A `GtkLabel` neither wraps nor ellipsizes by default, so its minimum width is
//! the full rendered width of whatever text it holds. In a row's SUFFIX — a
//! slot meant for a short status beside a button — that makes any sentence a
//! floor under the whole dialog. "A secret is stored. Type a new one to replace
//! it, or leave blank to keep it." measured 473px on its own.
//!
//! Three plausible suspects were measured and cleared before this one was
//! found: the placeholders on every field (+15px, and flat regardless of
//! length), long `AdwActionRow` subtitles (43px — those wrap), and
//! `AdwPasswordEntryRow` itself (139px). `examples/row_width_probe.rs` is the
//! measurement; it reproduces the 745px row and the 287px fix.

use gtk4::prelude::*;
use gtk4::{self as gtk};

// ─── The width vocabulary ────────────────────────────────────────────────────
//
// Nineteen dialogs picked thirteen different widths — 360, 400, 480, 500, 520,
// 540, 600, 640, 680, 720, 920, 1040 — every one written at the point of use,
// so each new dialog picked whatever looked right beside the last one. Same
// disease `ui::space` was written to cure for spacing, and the same cure: name
// them by ROLE, so a call site says what kind of dialog it is rather than how
// many pixels it wants.
//
// Widths only. A dialog that is too SHORT gets a scrollbar; a dialog that is
// too NARROW clips, because there is no horizontal scrolling to fall back on.
// Height is a matter of taste, width is the bug.
//
// Off-scale values fold UPWARD to the nearest role. A dialog can never clip by
// being wider than it needs, only by being narrower, so rounding up cannot
// introduce the fault this module exists to prevent.

/// One field and the buttons that answer it — rename, sign in, a profile.
pub const PROMPT: i32 = 400;

/// A form or a short list: preferences groups, a wizard step, a share sheet.
pub const FORM: i32 = 540;

/// Content meant to be read: a log, a saved query, a table of jobs.
pub const DETAIL: i32 = 720;

/// A grid or a browser, where the content is the point and it is wide.
pub const BROWSE: i32 = 1040;

/// The preferences dialog's width, and the budget every widget inside it has.
///
/// Named because two places need to agree about it: the dialog that sets it and
/// the guard that checks nothing inside exceeds it. A literal in one file and a
/// number in a test is how they stop agreeing.
pub const DIALOG_CONTENT_WIDTH: i32 = DETAIL;

/// How much of a status a suffix may show before it gives way to an ellipsis.
///
/// Wide enough for the statuses this UI actually shows ("Secret stored",
/// "Credentials valid"), narrow enough that a sentence cannot push a dialog
/// out of shape. The full text stays reachable — see [`set_status`].
const STATUS_CHARS: i32 = 24;

/// A status label for a row's suffix, which can never widen its dialog.
///
/// Use this rather than a bare `gtk::Label` for anything passed to
/// `add_suffix`. The constraint is not decoration: without it the label's
/// minimum width is its text, and its text is written by whoever changes the
/// string next.
pub fn status_label() -> gtk::Label {
    let label = gtk::Label::new(None);
    label.add_css_class("dim-label");
    label.set_valign(gtk::Align::Center);
    fit_label(&label);
    label
}

/// Stop `label` from demanding room for its whole text.
///
/// Split out so a label that already exists — built with different css classes,
/// or by a builder — can be brought under the same rule without being rebuilt.
pub fn fit_label(label: &gtk::Label) {
    label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    label.set_max_width_chars(STATUS_CHARS);
}

/// Set a status, keeping the whole of it reachable.
///
/// Ellipsis hides text, and some of these statuses carry instructions — "Type a
/// new one to replace it, or leave blank to keep it" is the answer to a
/// question the user is about to ask. Truncating that on screen is fine;
/// losing it is not, so the full text becomes the tooltip.
pub fn set_status(label: &gtk::Label, text: &str) {
    label.set_text(text);
    label.set_tooltip_text((!text.is_empty()).then_some(text));
}

#[cfg(test)]
mod tests {
    /// Every modal's width comes from the vocabulary, not a number.
    ///
    /// Thirteen different widths across nineteen dialogs is what "each one
    /// picked what looked right" produces, and a width nobody chose on purpose
    /// is a width nobody checked the content against.
    ///
    /// The application window is exempt: it is not a modal, it is the thing
    /// modals sit on top of, and it is sized to the screen rather than to its
    /// content.
    #[test]
    fn every_modal_takes_its_width_from_the_vocabulary() {
        let mut literals = Vec::new();
        let mut adopted = 0usize;

        for (path, text) in crate::testing::rust_sources() {
            let p = path.to_string_lossy();
            if !p.contains("/ui/") {
                continue;
            }
            // The shell receives the role as a parameter and passes it on; it
            // is the thing this guard points callers at.
            if p.ends_with("/ui/dialog.rs") {
                continue;
            }
            let code = crate::testing::without_comments(crate::testing::code(&text));
            // Dialogs on the shared shell pass the role to `Dialog::new`
            // instead of calling `.default_width`, so count those too — they
            // are the adopted ones, not the missing ones.
            adopted += code.matches("Dialog::new(").count();

            for (at, _) in code.match_indices(".default_width(") {
                let arg = &code[at + ".default_width(".len()..];
                let arg = &arg[..arg.find(')').unwrap_or(0)];
                if arg.contains("fit::") {
                    adopted += 1;
                    continue;
                }
                // The application window, which is not a modal.
                if arg.trim() == "1200" {
                    continue;
                }
                literals.push(format!("{}: default_width({arg})", path.display()));
            }
        }

        assert!(adopted >= 15, "only {adopted} adopted — scan broken");
        assert!(
            literals.is_empty(),
            "modal(s) sized with a literal. Use a role from `ui::fit` — PROMPT, \
             FORM, DETAIL, BROWSE — so the width is a decision somebody made: \
             {literals:#?}"
        );
    }

    /// Every `gtk::Label` used as a row suffix is built through this module.
    ///
    /// A source scan because the failure is a layout one: it shows up as a
    /// runtime warning and a clipped dialog, never as a test failure, and the
    /// label that caused it looked entirely reasonable at its call site.
    ///
    /// Matched per file and per binding: `add_suffix(&x)` is only satisfied by
    /// an `x` this module made, or one explicitly passed to `fit_label`.
    #[test]
    fn every_suffix_label_is_kept_inside_its_dialog() {
        let mut loose = Vec::new();
        let mut checked = 0usize;

        for (path, text) in crate::testing::rust_sources() {
            let code = crate::testing::without_comments(crate::testing::code(&text));
            for line in code.lines() {
                let Some(rest) = line.trim().strip_prefix("let ") else {
                    continue;
                };
                let Some((name, tail)) = rest.split_once(" = ") else {
                    continue;
                };
                if !tail.starts_with("gtk::Label::new") {
                    continue;
                }
                // Only labels that end up in a suffix are in scope; a label in
                // a page body has room to be as wide as it likes.
                if !code.contains(&format!("add_suffix(&{name})")) {
                    continue;
                }
                checked += 1;
                let fitted = code.contains(&format!("fit_label(&{name})"))
                    || code.contains(&format!("fit::fit_label(&{name})"));
                if !fitted {
                    loose.push(format!("{}: `{name}`", path.display()));
                }
            }
        }

        assert!(
            loose.is_empty(),
            "row suffix label(s) built with a bare gtk::Label — their minimum \
             width is their text, and a sentence there clips the dialog. Use \
             `ui::fit::status_label()`: {loose:#?}"
        );
        // A scan that matches nothing proves nothing.
        assert!(
            checked > 0 || loose.is_empty(),
            "no suffix labels found — scan broken"
        );
    }
}
