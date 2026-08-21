//! Does switching HDU destroy the viewer?
//!
//! QA report: `set_fits_view {"hdu": 2}` on a multi-extension file returned
//! `isError: false` with stale primary-HDU data and `tabCount: 0`, and every
//! call afterwards answered "no FITS open" — while the page was still on
//! screen. An INVALID hdu was harmless, which is the tell: the failure is on
//! the success path.
//!
//! Drives `handle_viewer_command`, the same entry point the MCP bridge uses.
//!
//!     cargo run --example fits_hdu_probe -- <path-to-multi-extension.fits>

use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use serde_json::json;
use verbinal::state::AppServices;
use verbinal::ui::fits_viewer::FitsViewer;

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: fits_hdu_probe <file.fits>");

    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let handle = rt.handle().clone();
    let _guard = rt.enter();

    let app = adw::Application::builder()
        .application_id("net.canfar.verbinal.fitshduprobe")
        .build();

    app.connect_activate(move |app| {
        let (services, _rx) = AppServices::new(handle.clone());
        let viewer = FitsViewer::new(services);

        let win = adw::ApplicationWindow::builder()
            .application(app)
            .default_width(900)
            .default_height(700)
            .build();
        win.set_content(Some(viewer.widget()));
        win.present();

        let path = path.clone();
        let viewer = viewer.clone();
        gtk4::glib::spawn_future_local(async move {
            if let Err(e) = viewer.load_from_path(std::path::Path::new(&path)) {
                println!("load failed: {e}");
                std::process::exit(2);
            }

            let before = viewer
                .handle_viewer_command("get_fits_view", &json!({}))
                .await
                .expect("get_fits_view after open");
            println!(
                "after open:   tabCount={}  hdu={}  hdus={}",
                before["tabCount"],
                before["hdu"],
                before["hdus"].as_array().map(|a| a.len()).unwrap_or(0)
            );

            // A DIFFERENT image HDU from the one already shown — switching to
            // the current one short-circuits and exercises nothing.
            let current = before["hdu"].as_u64().unwrap_or(0);
            let target = before["hdus"]
                .as_array()
                .expect("hdus")
                .iter()
                .filter(|h| h["isImage"].as_bool().unwrap_or(false))
                .filter_map(|h| h["index"].as_u64())
                .find(|i| *i != current)
                .expect("a second image HDU to switch to");
            // An explicit target, so the same probe can exercise the SUCCESS
            // path on old code — where the failure is the destroyed tab, not
            // the off-by-one that guards it.
            let target = std::env::args()
                .nth(2)
                .and_then(|a| a.parse::<u64>().ok())
                .unwrap_or(target);
            println!("switching {current} -> {target}");

            let switched = viewer
                .handle_viewer_command("set_fits_view", &json!({ "hdu": target }))
                .await;
            match &switched {
                Ok(v) => println!(
                    "set hdu:      tabCount={}  hdu={}  status={}",
                    v["tabCount"], v["hdu"], v["status"]
                ),
                Err(e) => println!("set hdu:      ERROR {e}"),
            }

            // The state the report found destroyed.
            let after = viewer
                .handle_viewer_command("get_fits_view", &json!({}))
                .await;
            match after {
                Ok(v) => {
                    let count = v["tabCount"].as_u64().unwrap_or(0);
                    let hdu = v["hdu"].as_u64().unwrap_or(999);
                    println!("after switch: tabCount={count}  hdu={hdu}");
                    let ok = count == 1 && hdu == target;
                    println!(
                        "\n{}",
                        if ok {
                            "PASS: the tab survived and the HDU actually changed"
                        } else {
                            "FAIL: the viewer lost its tab, or the HDU did not change"
                        }
                    );
                    std::process::exit(if ok { 0 } else { 1 });
                }
                Err(e) => {
                    println!("after switch: ERROR {e}");
                    println!("\nFAIL: the viewer was destroyed by a valid HDU switch");
                    std::process::exit(1);
                }
            }
        });
    });

    app.run_with_args::<&str>(&[]);
}
