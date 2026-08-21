//! A sentence in a row's suffix sets a floor under the whole dialog.
//!
//! Settings warned "AdwToolbarView exceeds AdwBreakpointBin width: requested
//! 784 px, 720 px available" and clipped its own controls. Walking the real
//! page found one leaf responsible: the suffix `GtkLabel` reading "A secret is
//! stored. Type a new one to replace it, or leave blank to keep it." measured
//! 473px MINIMUM — a label neither wraps nor ellipsizes by default, so its
//! minimum is the full rendered width of its text, and a minimum propagates all
//! the way up.
//!
//! Three suspects were measured and cleared first: the placeholders added to
//! every field (+15px, flat), long `AdwActionRow` subtitles (43px — they wrap),
//! and `AdwPasswordEntryRow` itself (139px). Guessing would have fixed one of
//! those.
//!
//!     cargo run --example row_width_probe

use gtk4::prelude::*;
use gtk4::{self as gtk};
use libadwaita as adw;
use libadwaita::prelude::*;

/// `main_window` presents Settings at this width.
const AVAILABLE: i32 = 720;

const SENTENCE: &str =
    "A secret is stored. Type a new one to replace it, or leave blank to keep it.";

fn minimum_of(row: &impl IsA<gtk::Widget>) -> i32 {
    let list = gtk::ListBox::new();
    list.add_css_class("boxed-list");
    list.append(row);
    list.measure(gtk::Orientation::Horizontal, -1).0
}

/// The row as Settings builds it: apply button, status label, Remove button.
fn secret_row(status: &gtk::Label) -> adw::PasswordEntryRow {
    let row = adw::PasswordEntryRow::new();
    row.set_title("Registry secret (Harbor CLI secret)");
    row.set_show_apply_button(true);
    let remove = gtk::Button::with_label("Remove secret");
    remove.add_css_class("flat");
    remove.set_valign(gtk::Align::Center);
    row.add_suffix(status);
    row.add_suffix(&remove);
    row
}

fn plain(text: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.add_css_class("dim-label");
    l
}

/// What `ui::dialog_fit::status_label` does.
fn fitted(text: &str) -> gtk::Label {
    let l = plain(text);
    l.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    l.set_max_width_chars(24);
    l
}

fn main() {
    let app = adw::Application::builder()
        .application_id("net.canfar.verbinal.rowwidthprobe")
        .build();

    app.connect_activate(|app| {
        let cases: [(&str, gtk::Label); 4] = [
            ("sentence, as shipped", plain(SENTENCE)),
            ("sentence, ellipsized", fitted(SENTENCE)),
            ("short status, as shipped", plain("Secret stored")),
            ("short status, ellipsized", fitted("Secret stored")),
        ];

        println!("{:>28}  {:>8}", "suffix label", "row min");
        for (what, label) in cases {
            let min = minimum_of(&secret_row(&label));
            let flag = if min > AVAILABLE {
                "  <-- wider than the dialog"
            } else {
                ""
            };
            println!("{what:>28}  {min:>6}px{flag}");
        }
        println!("\nthe dialog has {AVAILABLE}px; the warning reported 784px");

        let win = adw::ApplicationWindow::builder().application(app).build();
        win.set_content(Some(&gtk::Label::new(Some("probe"))));
        gtk4::glib::timeout_add_local_once(std::time::Duration::from_millis(200), || {
            std::process::exit(0);
        });
        win.present();
    });

    app.run_with_args::<&str>(&[]);
}
