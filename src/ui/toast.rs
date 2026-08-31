//! Tell the user something happened, in the nearest toast overlay.
//!
//! Walks up from a widget to whichever `AdwToastOverlay` contains it, so a
//! caller does not have to be handed one. Falls back to stderr rather than
//! failing silently: a message nobody sees is worse than one in a log.

use gtk4::prelude::*;
use gtk4::{self as gtk};
use libadwaita as adw;

pub fn show(near: &impl IsA<gtk::Widget>, message: &str) {
    let widget: gtk::Widget = near.clone().upcast::<gtk::Widget>();
    match widget
        .ancestor(adw::ToastOverlay::static_type())
        .and_downcast::<adw::ToastOverlay>()
    {
        Some(overlay) => overlay.add_toast(adw::Toast::new(message)),
        None => eprintln!("[verbinal] {message}"),
    }
}
