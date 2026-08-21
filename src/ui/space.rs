//! The spacing vocabulary.
//!
//! 522 spacing calls across this UI, in fourteen different values, every one
//! written at the point of use. Nothing said what `12` *meant*, so each new
//! widget picked a number that looked right beside its neighbour — which is how
//! 1, 2, 3, 10 and 18 got in, and why controls sit at different distances from
//! the edges of the dialogs that hold them.
//!
//! Roles are added when a call site needs one, not in advance: a constant
//! nobody uses is a decision nobody has made.
//!
//! These name spacings by **role**, not size. Two things follow: a call site
//! says what it is doing rather than how many pixels it wants, and changing what
//! "the edge of a dialog" means is one edit rather than a search.
//!
//! The values are the ones this UI already used most, so adopting a name rarely
//! moves anything; the visible change comes from the off-scale one-offs folding
//! into the nearest role.

/// The outer margin of a page or a dialog's content — the distance from
/// anything to the edge of the thing that holds it.
pub const EDGE: i32 = 12;

/// Inside a card, a boxed list row, or a group.
pub const CARD: i32 = 12;

/// Between rows within a group.
pub const ROW: i32 = 6;

/// Between a label and the control it names, or between adjacent controls.
pub const CONTROL: i32 = 8;

// Roles may share a value — EDGE and CARD are both 12 today — but the ordering
// has to hold, or the gap between two rows inside a group could exceed the
// group's own inset. Checked when this compiles, not when a test runs.
const _: () = assert!(ROW < CONTROL);
const _: () = assert!(CONTROL <= EDGE);

/// The width of a form control that sits at the trailing edge of a row.
///
/// Not a spacing, but the same kind of decision: a column of content-sized
/// dropdowns has its right edges aligned and its left edges anywhere, which
/// reads as ragged even though every row is individually correct. One width
/// gives the values a column of their own.
pub const FIELD: i32 = 220;

/// Apply [`EDGE`] on all four sides — a dialog's content, a page's root.
///
/// The commonest reason a control sits flush against a window edge is four
/// margin calls where one was written and three were forgotten.
pub fn edge_all(widget: &impl gtk4::prelude::IsA<gtk4::Widget>) {
    inset(widget, EDGE);
}

/// Apply `margin` on all four sides.
pub fn inset(widget: &impl gtk4::prelude::IsA<gtk4::Widget>, margin: i32) {
    use gtk4::prelude::WidgetExt;
    let w = widget.as_ref();
    w.set_margin_start(margin);
    w.set_margin_end(margin);
    w.set_margin_top(margin);
    w.set_margin_bottom(margin);
}

/// A dialog's action row: the buttons along its bottom edge.
///
/// Inset from all four edges, so the buttons are never flush against the window
/// — the failure this exists to prevent is a Done button touching the frame.
pub fn action_row(spacing: i32) -> gtk4::Box {
    use gtk4::prelude::WidgetExt;
    let row = gtk4::Box::new(gtk4::Orientation::Horizontal, spacing);
    row.set_margin_start(EDGE);
    row.set_margin_end(EDGE);
    row.set_margin_top(ROW);
    row.set_margin_bottom(EDGE);
    row
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_value_is_on_a_two_pixel_grid() {
        // Odd values are where the raggedness came from: a 3 beside a 4 reads
        // as a mistake even when nobody can name which one is wrong.
        for (name, v) in [
            ("EDGE", EDGE),
            ("CARD", CARD),
            ("ROW", ROW),
            ("CONTROL", CONTROL),
        ] {
            assert_eq!(v % 2, 0, "{name} = {v} is off the grid");
        }
    }
}

#[cfg(test)]
mod dialog_tests {
    //! A dialog's buttons must be somewhere the window keeps on screen.
    //!
    //! The connect wizard put its Back/Next row inside the scrolling body and
    //! relied on a vexpanding child to hold it down. On a window taller than the
    //! display, the row went past the bottom edge and both buttons were cut in
    //! half — the screenshot that started this work.
    //!
    //! That fix was half of it, and this guard passed while the fault was still
    //! visible. `add_bottom_bar` keeps the actions inside the WINDOW; nothing
    //! kept the window inside the SCREEN. Measured in
    //! `examples/wizard_height_probe.rs`: a hand-rolled dialog asked for 560px
    //! and grew to 2034px to fit its content, carrying its own bottom bar below
    //! the bottom of any ordinary display. The missing half is a scroller over
    //! the content and a cap on the height, which is what `ui::dialog` adds.

    /// Dialogs that build their own action row rather than using
    /// `adw::MessageDialog`, which pads its own.
    ///
    /// Empty now: every one of them went through `ui::dialog::Dialog`, which
    /// satisfies both halves by construction. Kept so the next hand-built one
    /// has somewhere to be listed — and a reason to justify itself.
    const HAND_BUILT: &[(&str, &str)] = &[];

    #[test]
    fn a_dialogs_actions_live_in_a_bottom_bar() {
        for (name, source) in HAND_BUILT {
            let code = crate::testing::code(source);
            assert!(
                code.contains("add_bottom_bar"),
                "{name} packs its actions into the body; a window taller than the \
                 display pushes them past its edge"
            );
        }
    }

    #[test]
    fn a_dialogs_content_is_inset_from_its_edges() {
        for (name, source) in HAND_BUILT {
            let code = crate::testing::code(source);
            assert!(
                code.contains("space::edge_all") || code.contains("space::inset"),
                "{name} sets its content margins by hand, which is how three of \
                 the four get forgotten"
            );
        }
    }

    /// The shell does both halves, so nothing that uses it needs checking.
    ///
    /// This asserts the shell still does them: a bottom bar for the actions and
    /// a scroller with a capped height over the content. Take either away and
    /// every dialog that trusts it regresses at once.
    #[test]
    fn the_shared_shell_pins_its_actions_and_caps_its_height() {
        // Without `without_comments` this matches the module's own prose: the
        // doc comment on `new` explains what `max_content_height` is for, so
        // deleting the CALL left the guard green. Fifth time that trap has
        // caught something in this codebase.
        let code =
            crate::testing::without_comments(crate::testing::code(include_str!("dialog.rs")));
        for needed in [
            "add_bottom_bar",
            "ScrolledWindow::builder",
            "max_content_height",
            "space::edge_all",
        ] {
            assert!(
                code.contains(needed),
                "ui::dialog no longer calls {needed}; every dialog built on it \
                 loses that protection silently"
            );
        }
    }
}
