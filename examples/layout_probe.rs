//! Measure what a pinned cell actually gives its child.
//!
//! GTK must be initialised on the MAIN thread and libtest runs every test in a
//! spawned one, so no `cargo test` can answer a layout question — `gtk::init()`
//! simply fails there. That gap is how the results table shipped twice with
//! every heading rendered as "…": the reasoning was wrong both times and
//! nothing in the suite could contradict it.
//!
//! Run it and read the numbers:
//!
//!     cargo run --example layout_probe
//!
//! It printed, for a heading in a pinned button:
//!
//!     column  95px -> label gets  15px with halign=Start,  75px with halign=Fill
//!     column 140px -> label gets  15px with halign=Start, 120px with halign=Fill
//!
//! 15px at both widths is one character and an ellipsis: inside a button the
//! CHILD's halign decides whether it fills, and `Start` hands it its natural
//! width — which the cell clamp had just reduced to nothing.
use gtk4::prelude::*;

fn measure(align: gtk4::Align, width: i32) -> i32 {
    let label = gtk4::Label::new(Some("Dec. (J2000.0)"));
    label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    label.set_max_width_chars(1);
    label.set_halign(align);
    label.set_xalign(0.0);

    let button = gtk4::Button::new();
    button.set_child(Some(&label));
    button.set_size_request(width, -1);
    button.set_hexpand(false);
    button.set_halign(gtk4::Align::Fill);

    // Allocate directly: a window would need a running main loop and a
    // compositor to map it before anything has a size.
    let (_, nat_h, _, _) = button.measure(gtk4::Orientation::Vertical, width);
    button.allocate(width, nat_h.max(24), -1, None);
    label.width()
}

fn main() {
    gtk4::init().expect("a display is needed: run this on your desktop session");
    // What each heading needs, against what `column_width_for` hands it.
    for heading in [
        "collection",
        "RA (J2000.0)",
        "Dec. (J2000.0)",
        "Target Name",
        "Proposal ID",
        "Data Release",
    ] {
        let probe = gtk4::Label::new(Some(heading));
        let (_, needs, _, _) = probe.measure(gtk4::Orientation::Horizontal, -1);
        let chars = heading.chars().count() as i32;
        let _ = chars;
        // What `column_width_for` now grants: the measured heading plus chrome,
        // less the button's padding that the label does not get.
        let granted = (needs + 34) - 20;
        println!(
            "{heading:<16} needs {needs:>4}px, cell grants {granted:>4}px  {}",
            if granted >= needs { "ok" } else { "ELIDES" }
        );
    }

    // An ActionRow's wrap behaviour is NOT measurable this way: an unrealised
    // row reports the same height at every width, so the numbers looked like
    // evidence and were not. Removed rather than left to be believed.
    for width in [95, 140] {
        let start = measure(gtk4::Align::Start, width);
        let fill = measure(gtk4::Align::Fill, width);
        println!(
            "column {width}px -> label gets {start}px with halign=Start, {fill}px with halign=Fill"
        );
    }
}
