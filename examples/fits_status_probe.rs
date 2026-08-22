//! Does the status line follow the active tab?
//!
//! Reported: a 720x360 image shows "64x64 pixels" after a tab switch. The
//! status line is viewer-wide, not per-tab, and nothing refreshed it when the
//! selection changed — so it kept describing the tab you left, and
//! `get_fits_view` reported that text as the new tab's status.
//!
//!     cargo run --example fits_status_probe -- <a.fits> <b.fits>
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use serde_json::json;
use verbinal::state::AppServices;
use verbinal::ui::fits_viewer::FitsViewer;

fn main() {
    let a = std::env::args().nth(1).expect("a.fits");
    let b = std::env::args().nth(2).expect("b.fits");
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let handle = rt.handle().clone();
    let _g = rt.enter();

    let app = adw::Application::builder()
        .application_id("net.canfar.verbinal.fitsstatusprobe")
        .build();

    app.connect_activate(move |app| {
        let (services, _rx) = AppServices::new(handle.clone());
        let viewer = FitsViewer::new(services);
        let win = adw::ApplicationWindow::builder()
            .application(app)
            .default_width(800)
            .default_height(600)
            .build();
        win.set_content(Some(viewer.widget()));
        win.present();

        let (a, b) = (a.clone(), b.clone());
        let viewer = viewer.clone();
        gtk4::glib::spawn_future_local(async move {
            viewer
                .load_from_path(std::path::Path::new(&a))
                .expect("load a");
            viewer
                .load_from_path(std::path::Path::new(&b))
                .expect("load b");

            let mut bad = 0;
            for (index, expect) in [(0usize, "64x64"), (1, "720x360"), (0, "64x64")] {
                let v = viewer
                    .handle_viewer_command("switch_fits_tab", &json!({ "index": index }))
                    .await
                    .expect("switch");
                let status = v["status"].as_str().unwrap_or("").to_string();
                let file = v["fileName"].as_str().unwrap_or("").to_string();
                let ok = status.contains(expect);
                if !ok {
                    bad += 1;
                }
                println!(
                    "tab {index} ({file:<12}) status={status:<28} expected {expect:<9} {}",
                    if ok { "ok" } else { "STALE" }
                );
            }
            println!(
                "\n{}",
                if bad == 0 {
                    "PASS: the status line describes the tab you switched to"
                } else {
                    "FAIL: the status line is stale"
                }
            );
            std::process::exit(if bad == 0 { 0 } else { 1 });
        });
    });

    app.run_with_args::<&str>(&[]);
}
