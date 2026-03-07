use gtk4::prelude::*;
use gtk4::{self as gtk};

/// Creates a standard card header with title, spinner, and refresh button.
/// Spinner is placed inline in the header row, to the right of the title.
pub fn card_header(title: &str) -> (gtk::Box, gtk::Spinner, gtk::Button) {
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    header.set_margin_start(16);
    header.set_margin_end(16);
    header.set_margin_top(12);

    let label = gtk::Label::new(Some(title));
    label.add_css_class("title-4");
    label.set_halign(gtk::Align::Start);
    label.set_hexpand(true);
    header.append(&label);

    let spinner = gtk::Spinner::new();
    spinner.set_visible(false);
    header.append(&spinner);

    let refresh_btn = gtk::Button::from_icon_name("view-refresh-symbolic");
    refresh_btn.set_tooltip_text(Some("Refresh"));
    header.append(&refresh_btn);

    (header, spinner, refresh_btn)
}
