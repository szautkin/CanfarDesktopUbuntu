//! Do a dialog's actions survive content that is far too tall?
//!
//! The shell in `ui::dialog` claims two things: content scrolls, actions do
//! not. This puts 40 tall rows into a 560px dialog — content several times the
//! window — and checks that the action row is still allocated its full height
//! and still inside the window's bounds.
//!
//! The hand-rolled shape is measured beside it, since that is what seventeen
//! dialogs still do.
//!
//!     cargo run --example wizard_height_probe
use gtk4::prelude::*;
use gtk4::{self as gtk};
use libadwaita as adw;
use libadwaita::prelude::*;
use verbinal::ui::dialog::Dialog;
use verbinal::ui::{fit, space};

const HEIGHT: i32 = 560;
const ROWS: i32 = 40;

fn tall_content() -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 8);
    for i in 0..ROWS {
        let l = gtk::Label::new(Some(&format!("row {i} — content that keeps going")));
        l.set_height_request(40);
        b.append(&l);
    }
    b
}

/// Bottom of the action row, and its allocated height.
fn action_geometry(win: &gtk::Window, marker: &str) -> Option<(i32, i32)> {
    fn find(w: &gtk::Widget, marker: &str) -> Option<gtk::Widget> {
        if let Some(b) = w.downcast_ref::<gtk::Button>() {
            if b.label().map(|l| l == marker).unwrap_or(false) {
                return w.parent();
            }
        }
        let mut c = w.first_child();
        while let Some(ch) = c {
            if let Some(f) = find(&ch, marker) {
                return Some(f);
            }
            c = ch.next_sibling();
        }
        None
    }
    let row = find(&win.clone().upcast::<gtk::Widget>(), marker)?;
    let bounds = row.compute_bounds(win)?;
    Some((row.height(), (bounds.y() + bounds.height()) as i32))
}

fn main() {
    let app = adw::Application::builder()
        .application_id("net.canfar.verbinal.dialogshellprobe")
        .build();

    app.connect_activate(|app| {
        // ── The shell ───────────────────────────────────────────────────────
        let d = Dialog::new("shell", fit::FORM, HEIGHT);
        d.content().append(&tall_content());
        let ok = gtk::Button::with_label("ShellDone");
        d.add_secondary_action(&gtk::Button::with_label("ShellBack"));
        d.add_action(&ok);
        d.window.set_application(Some(app));
        d.window.present();

        // ── The hand-rolled shape: no scroller under the content ────────────
        let hand = adw::Window::builder()
            .title("hand-rolled")
            .default_width(fit::FORM)
            .default_height(HEIGHT)
            .build();
        let tb = adw::ToolbarView::new();
        tb.add_top_bar(&adw::HeaderBar::new());
        let body = gtk::Box::new(gtk::Orientation::Vertical, space::CARD);
        space::edge_all(&body);
        body.append(&tall_content());
        tb.set_content(Some(&body));
        let footer = space::action_row(space::CONTROL);
        footer.append(&gtk::Button::with_label("HandDone"));
        tb.add_bottom_bar(&footer);
        hand.set_content(Some(&tb));
        hand.set_application(Some(app));
        hand.present();

        gtk4::glib::timeout_add_local_once(std::time::Duration::from_millis(900), move || {
            for (what, win, marker) in [
                (
                    "ui::dialog shell",
                    d.window.clone().upcast::<gtk::Window>(),
                    "ShellDone",
                ),
                (
                    "hand-rolled",
                    hand.clone().upcast::<gtk::Window>(),
                    "HandDone",
                ),
            ] {
                let h = win.height();
                match action_geometry(&win, marker) {
                    Some((alloc, bottom)) => {
                        // The question is not whether the actions are inside
                        // the WINDOW — they always are, because the window
                        // grows to hold them. It is whether the window stayed
                        // the size it was asked for. A window that grew to
                        // 2034px to fit its content has put its own bottom
                        // past the bottom of any ordinary display, and the
                        // buttons went with it.
                        let kept = h <= HEIGHT;
                        println!(
                            "{what:>18}: asked for {HEIGHT}px, got {h}px; actions {alloc}px \
                             tall ending at {bottom}px -> {}",
                            if kept {
                                "held its size, content scrolled"
                            } else {
                                "GREW PAST THE SCREEN, taking the buttons with it"
                            }
                        );
                    }
                    None => println!("{what:>18}: action row not found"),
                }
            }
            std::process::exit(0);
        });
    });

    app.run_with_args::<&str>(&[]);
}
