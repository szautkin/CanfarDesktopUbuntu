//! Is "Check images" actually in the CANFAR Images header?
//!
//! The source guard proves the sync has one caller and that sign-in is not it.
//! It cannot prove the button exists — a handler wired to a widget never added
//! to the header would satisfy every test and appear nowhere.
//!
//!     cargo run --example images_button_probe
use gtk4::prelude::*;
use gtk4::{self as gtk};
use libadwaita as adw;
use libadwaita::prelude::*;
use verbinal::state::AppServices;
use verbinal::ui::canfar_images::CanfarImagesView;

/// Every button label in the widget tree, in order.
fn button_labels(w: &gtk::Widget, out: &mut Vec<String>) {
    if let Some(b) = w.downcast_ref::<gtk::Button>() {
        if let Some(l) = b.label() {
            out.push(l.to_string());
        }
    }
    let mut c = w.first_child();
    while let Some(child) = c {
        button_labels(&child, out);
        c = child.next_sibling();
    }
}

fn main() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let handle = rt.handle().clone();
    let _guard = rt.enter();

    let app = adw::Application::builder()
        .application_id("net.canfar.verbinal.imagesbuttonprobe")
        .build();

    app.connect_activate(move |app| {
        let (services, _rx) = AppServices::new(handle.clone());
        let view = CanfarImagesView::new(services);

        let win = adw::ApplicationWindow::builder()
            .application(app)
            .default_width(700)
            .default_height(500)
            .build();
        win.set_content(Some(view.widget()));
        win.present();

        gtk4::glib::timeout_add_local_once(std::time::Duration::from_millis(400), move || {
            let mut labels = Vec::new();
            button_labels(&view.widget().clone().upcast::<gtk::Widget>(), &mut labels);
            println!("buttons in the card: {labels:?}");

            let present = labels.iter().any(|l| l == "Check images");
            println!(
                "\n{}",
                if present {
                    "PASS: the Check images button is in the header"
                } else {
                    "FAIL: no Check images button"
                }
            );
            std::process::exit(if present { 0 } else { 1 });
        });
    });

    app.run_with_args::<&str>(&[]);
}
