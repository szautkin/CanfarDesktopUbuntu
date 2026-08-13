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

use gtk4::prelude::*;
use gtk4::{self as gtk};

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

/// Put the image and the control column side by side.
///
/// The column keeps its width when the window resizes — the image takes the
/// change, because the image is what the reader is looking at.
pub fn shell(image: &impl IsA<gtk::Widget>, column: &gtk::ScrolledWindow) -> gtk::Paned {
    let paned = gtk::Paned::new(gtk::Orientation::Horizontal);
    paned.set_hexpand(true);
    paned.set_vexpand(true);
    paned.set_wide_handle(true);
    paned.set_start_child(Some(image));
    paned.set_end_child(Some(column));
    paned.set_resize_end_child(false);
    paned.set_shrink_end_child(false);
    paned
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
        let legacy = format!("gtk::{}::new()", "Notebook");
        for (name, source) in HOSTS {
            assert!(
                !source.contains(&legacy),
                "the {name} viewer is back on a Notebook"
            );
        }
    }

    #[test]
    fn neither_viewer_keeps_its_own_copy_of_the_helpers() {
        // Assembled at runtime so this guard does not match itself.
        let local = format!("fn {}(text: &str) -> gtk::Label", "section_header");
        for (name, source) in [("cube", CUBE), ("fits", FITS)] {
            assert!(
                !source.contains(&local),
                "the {name} viewer has its own section header again; it will drift"
            );
        }
    }
}
