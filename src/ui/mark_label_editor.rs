//! The little field you name a mark in.
//!
//! An overlay row — entry, a tick to confirm, a bin to delete — that sits on
//! the image next to the mark it belongs to. Extracted from the FITS viewer so
//! the cube can have the same thing rather than a second, different way of
//! naming a mark.
//!
//! **Not a popover.** A popover is its own surface: with autohide on it
//! dismissed itself the moment the pointer went back to the image, and with
//! autohide off one was seen floating above an unrelated application's window.
//! This is an ordinary widget, so whoever opens it clips it to their own
//! canvas and can move it as the mark moves.
//!
//! The host decides WHERE it goes, because only the host knows where the mark
//! is on screen. Everything else — how it looks, what its buttons do, which
//! keys it answers to — lives here, so the two viewers cannot drift into
//! naming a mark differently.

use gtk4::prelude::*;
use gtk4::{self as gtk};
use std::rc::Rc;

pub struct MarkLabelEditor {
    row: gtk::Box,
    entry: gtk::Entry,
}

impl MarkLabelEditor {
    /// Build an editor for a mark whose current label is `initial`.
    ///
    /// `on_commit` gets the typed text — from the tick, or from Enter, which is
    /// what a person does without thinking about it. `on_delete` is the bin.
    /// `on_cancel` is Escape: it must leave the mark alone, because Escape that
    /// silently saves is worse than one that does nothing.
    pub fn new(
        initial: &str,
        on_commit: impl Fn(String) + 'static,
        on_delete: impl Fn() + 'static,
        on_cancel: impl Fn() + 'static,
    ) -> Rc<Self> {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        row.add_css_class("osd");
        row.add_css_class("toolbar");
        row.set_margin_end(6);

        let entry = gtk::Entry::new();
        entry.set_placeholder_text(Some(crate::tr_en!("What is this?")));
        entry.set_width_chars(16);
        row.append(&entry);
        if !initial.is_empty() {
            entry.set_text(initial);
            entry.select_region(0, -1);
        }

        // Confirm sits LEFT of the bin: the safe action is the one your hand
        // reaches first, and the destructive one is not where a mis-click
        // lands.
        let done = gtk::Button::from_icon_name("object-select-symbolic");
        done.add_css_class("suggested-action");
        done.set_tooltip_text(Some(crate::tr_en!("Done")));
        row.append(&done);

        let bin = gtk::Button::from_icon_name("user-trash-symbolic");
        bin.add_css_class("destructive-action");
        bin.set_tooltip_text(Some(crate::tr_en!("Delete this mark")));
        row.append(&bin);

        let commit: Rc<dyn Fn()> = {
            let entry = entry.clone();
            let on_commit = Rc::new(on_commit);
            Rc::new(move || on_commit(entry.text().to_string()))
        };
        {
            let commit = commit.clone();
            entry.connect_activate(move |_| commit());
        }
        {
            let commit = commit.clone();
            done.connect_clicked(move |_| commit());
        }
        bin.connect_clicked(move |_| on_delete());
        {
            let keys = gtk::EventControllerKey::new();
            keys.connect_key_pressed(move |_, key, _, _| {
                if key == gtk::gdk::Key::Escape {
                    on_cancel();
                    return gtk::glib::Propagation::Stop;
                }
                gtk::glib::Propagation::Proceed
            });
            entry.add_controller(keys);
        }

        Rc::new(Self { row, entry })
    }

    /// The widget to place over a canvas.
    pub fn widget(&self) -> &gtk::Box {
        &self.row
    }

    /// Put the cursor in the field, so naming a mark needs no extra click.
    pub fn focus(&self) {
        self.entry.grab_focus();
    }
}
