//! The shape both image viewers use: an image on the left, a docked column of
//! controls on the right.
//!
//! The cube viewer had this first — grouped sections, each control under its own
//! caption, the whole column scrollable and resizable. The FITS viewer put the
//! same kind of controls in a horizontal toolbar, which cannot label them, cannot
//! group them, and runs out of width on a laptop; that pressure is what pushed
//! eleven of its controls into a popover nobody opened. Both viewers build their
//! column from here now, so a control looks and behaves the same whichever one
//! you are in.

use gtk4::glib::prelude::ToValue;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use libadwaita as adw;
use libadwaita::prelude::*;

/// Width of a viewer's control column.
///
/// One number for both, or the app has two ideas of how wide "the controls" are.
pub const COLUMN_WIDTH: i32 = 280;

/// A section heading inside a control column (`DISPLAY`, `IMAGE`, `COMPARE`).
pub fn section_header(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.add_css_class("caption-heading");
    label.add_css_class("dim-label");
    label.set_halign(gtk::Align::Start);
    label
}

/// A caption label stacked above its control.
pub fn labeled(text: &str, child: &impl IsA<gtk::Widget>) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let label = gtk::Label::new(Some(text));
    label.add_css_class("caption");
    label.set_halign(gtk::Align::Start);
    b.append(&label);
    b.append(child);
    b
}

/// A caption label and its control side by side, for a control that is small
/// enough not to need a row of its own (a toggle, a short button).
pub fn labeled_row(text: &str, child: &impl IsA<gtk::Widget>) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let label = gtk::Label::new(Some(text));
    label.add_css_class("caption");
    label.set_halign(gtk::Align::Start);
    label.set_hexpand(true);
    label.set_xalign(0.0);
    row.append(&label);
    row.append(child);
    row
}

/// An empty control column: the box to append sections to, and the scroller that
/// hosts it.
pub fn control_column() -> (gtk::Box, gtk::ScrolledWindow) {
    let column = gtk::Box::new(gtk::Orientation::Vertical, 12);
    column.set_margin_start(12);
    column.set_margin_end(12);
    column.set_margin_top(12);
    column.set_margin_bottom(12);
    column.set_width_request(COLUMN_WIDTH);

    let scroll = gtk::ScrolledWindow::new();
    scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroll.set_child(Some(&column));
    scroll.set_vexpand(true);
    (column, scroll)
}

/// Below this width the column stops taking space from the image and floats
/// over it instead.
///
/// Chosen so the image keeps roughly two thirds of a small laptop's width:
/// below it, a 280 px column is taking a third of the picture.
const COLLAPSE_WIDTH_SP: f64 = 900.0;

/// Put the image and the control column side by side.
///
/// An `OverlaySplitView` rather than a `Paned`: on a wide window the column is
/// docked beside the image, and on a narrow one it floats over the image
/// instead of squeezing it. A fixed 280 px column on a 1024-wide laptop takes a
/// third of the picture, which is the wrong third to give up.
///
/// The breakpoint lives in an `adw::BreakpointBin` so the rule is the shell's
/// own: a viewer is a page inside a window it does not own, and asking the
/// window to know about a viewer's sidebar would be the coupling this module
/// exists to avoid.
pub struct ViewerShell {
    /// The whole shell — put this in the page.
    pub widget: adw::BreakpointBin,
    /// Shows and hides the control column. Useful at any width, and the only way
    /// back to the controls once the window is narrow enough to hide them.
    pub sidebar_toggle: gtk::ToggleButton,
}

pub fn shell(image: &impl IsA<gtk::Widget>, column: &gtk::ScrolledWindow) -> ViewerShell {
    let split = adw::OverlaySplitView::new();
    split.set_content(Some(image));
    split.set_sidebar(Some(column));
    split.set_sidebar_position(gtk::PackType::End);
    split.set_show_sidebar(true);
    split.set_hexpand(true);
    split.set_vexpand(true);

    let bin = adw::BreakpointBin::new();
    bin.set_hexpand(true);
    bin.set_vexpand(true);
    // A BreakpointBin refuses to allocate smaller than its child's minimum, so
    // the child must be allowed to shrink to the width the breakpoint watches
    // for — otherwise the condition can never be met.
    bin.set_size_request(360, 200);
    bin.set_child(Some(&split));

    let breakpoint = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
        adw::BreakpointConditionLengthType::MaxWidth,
        COLLAPSE_WIDTH_SP,
        adw::LengthUnit::Sp,
    ));
    breakpoint.add_setter(&split, "collapsed", Some(&true.to_value()));
    // Hidden as well as collapsed: a narrow window should give the whole width
    // to the image, and the toggle brings the controls back over it. Overlaying
    // them the moment the window narrows would cover the picture uninvited.
    breakpoint.add_setter(&split, "show-sidebar", Some(&false.to_value()));
    bin.add_breakpoint(breakpoint);

    let sidebar_toggle = gtk::ToggleButton::new();
    sidebar_toggle.set_icon_name("sidebar-show-right-symbolic");
    sidebar_toggle.add_css_class("flat");
    sidebar_toggle.set_valign(gtk::Align::Center);
    sidebar_toggle.set_tooltip_text(Some(crate::tr_en!("Show or hide the controls")));
    // Bound both ways: the button reflects a collapse the breakpoint caused, and
    // pressing it moves the same property. A separate bool would be a second
    // opinion about whether the column is on screen.
    split
        .bind_property("show-sidebar", &sidebar_toggle, "active")
        .bidirectional()
        .sync_create()
        .build();

    ViewerShell {
        widget: bin,
        sidebar_toggle,
    }
}

#[cfg(test)]
mod tests {
    //! Both viewers must build their column from here, or the shared shape is
    //! only a shape one of them happens to have.

    const CUBE: &str = include_str!("cube_viewer.rs");
    const FITS: &str = include_str!("fits_viewer.rs");

    #[test]
    fn both_viewers_build_their_column_from_this_module() {
        for (name, source) in [("cube", CUBE), ("fits", FITS)] {
            assert!(
                source.contains("viewer_shell::control_column()"),
                "the {name} viewer should build its control column from viewer_shell"
            );
            assert!(
                source.contains("viewer_shell::section_header"),
                "the {name} viewer should use the shared section heading"
            );
        }
    }

    #[test]
    fn neither_viewer_hides_a_control_behind_an_unlabelled_popover() {
        // The failure this whole module exists to prevent: a control that works,
        // that an agent can drive, and that a person cannot find. A popover is
        // allowed — the thing that OPENS it must carry a word.
        for (name, source) in [("cube", CUBE), ("fits", FITS)] {
            for (at, _) in source.match_indices("set_popover(Some(") {
                // Look back at the button this popover belongs to: within the
                // preceding few lines it must set a label.
                let start = at.saturating_sub(700);
                let context = &source[start..at];
                assert!(
                    context.contains("set_label(") || context.contains("set_title("),
                    "a popover in the {name} viewer is opened by a control with \
                     no visible word on it"
                );
            }
        }
    }

    #[test]
    fn both_viewers_use_the_same_tab_machinery() {
        // The FITS viewer used a `gtk::Notebook` where the cube used an
        // `adw::TabView` — two tab strips that looked and behaved differently in
        // one application, and the Notebook needed a hand-rolled close button
        // that walked every page comparing label widgets to find its own.
        const HOSTS: &[(&str, &str)] = &[
            ("cube", include_str!("cube_tab_host.rs")),
            ("fits", include_str!("fits_viewer.rs")),
        ];
        for (name, source) in HOSTS {
            assert!(
                source.contains("adw::TabView::new()"),
                "the {name} viewer should host its tabs in an adw::TabView"
            );
            assert!(
                source.contains("adw::TabBar::new()"),
                "the {name} viewer should show the adw::TabBar strip"
            );
        }
        // And nobody has gone back.
        for (name, source) in HOSTS {
            assert!(
                !crate::testing::code(source).contains("gtk::Notebook::new()"),
                "the {name} viewer is back on a Notebook"
            );
        }
    }

    #[test]
    fn both_viewers_can_get_their_controls_back() {
        // The column hides itself on a narrow window, so both viewers must show
        // the toggle that brings it back — a hidden sidebar with no way to
        // reopen it is a viewer with no controls at all.
        for (name, source) in [("cube", CUBE), ("fits", FITS)] {
            assert!(
                source.contains("shell.sidebar_toggle"),
                "the {name} viewer never places the control-column toggle"
            );
        }
    }

    #[test]
    fn neither_viewer_keeps_its_own_copy_of_the_helpers() {
        for (name, source) in [("cube", CUBE), ("fits", FITS)] {
            // Tests stripped: this assertion names the very thing it forbids,
            // and would otherwise find itself. See `crate::testing::code`.
            assert!(
                !crate::testing::code(source)
                    .contains("fn section_header(text: &str) -> gtk::Label"),
                "the {name} viewer has its own section header again; it will drift"
            );
        }
    }
}
