//! Does `Editable::delegate()` actually reach an `AdwEntryRow`'s text?
//!
//! The source guard in `settings_page.rs` proves every field *calls*
//! `with_example`. It cannot prove the call does anything: if `delegate()`
//! returned `None`, or the inner widget were not a `GtkText`, the helper would
//! quietly do nothing and every test would still pass while the fields stayed
//! as bare as before. That is the failure this probe rules out — it sets a
//! placeholder the way the app does and reads it back off the real widget.
//!
//! Needs a display: run it as `cargo run --example placeholder_probe`.

use gtk4::prelude::*;
use gtk4::{self as gtk};
use libadwaita as adw;
use libadwaita::prelude::*;

fn set_example(row: &impl IsA<gtk::Editable>, example: &str) {
    if let Some(text) = row.as_ref().delegate().and_downcast::<gtk::Text>() {
        text.set_placeholder_text(Some(example));
    }
}

fn read_back(row: &impl IsA<gtk::Editable>) -> Option<String> {
    row.as_ref()
        .delegate()
        .and_downcast::<gtk::Text>()
        .and_then(|t| t.placeholder_text())
        .map(|s| s.to_string())
}

fn main() {
    let app = adw::Application::builder()
        .application_id("net.canfar.verbinal.placeholderprobe")
        .build();

    app.connect_activate(|app| {
        let entry = adw::EntryRow::new();
        entry.set_title("Registry repository (project)");
        set_example(&entry, "e.g. private-test");

        let secret = adw::PasswordEntryRow::new();
        secret.set_title("Registry secret (Harbor CLI secret)");
        set_example(&secret, "The CLI secret from your Harbor user profile");

        let mut failures = 0;
        for (what, got) in [
            ("AdwEntryRow", read_back(&entry)),
            ("AdwPasswordEntryRow", read_back(&secret)),
        ] {
            match got {
                Some(text) => println!("{what}: placeholder = {text:?}"),
                None => {
                    println!("{what}: NO PLACEHOLDER — delegate() did not reach a GtkText");
                    failures += 1;
                }
            }
        }

        // The row must still show its title: a placeholder that replaced the
        // title would trade one missing piece of information for another.
        println!("title still set: {:?}", entry.title());

        let win = adw::ApplicationWindow::builder()
            .application(app)
            .default_width(420)
            .build();
        let list = gtk::ListBox::new();
        list.add_css_class("boxed-list");
        list.append(&entry);
        list.append(&secret);
        win.set_content(Some(&list));
        win.present();

        glib_timeout(move || {
            println!(
                "{}",
                if failures == 0 {
                    "PROBE OK"
                } else {
                    "PROBE FAILED"
                }
            );
            std::process::exit(failures);
        });
    });

    app.run_with_args::<&str>(&[]);
}

fn glib_timeout(f: impl FnOnce() + 'static) {
    let mut f = Some(f);
    gtk4::glib::timeout_add_local(std::time::Duration::from_millis(600), move || {
        if let Some(f) = f.take() {
            f();
        }
        gtk4::glib::ControlFlow::Break
    });
}
