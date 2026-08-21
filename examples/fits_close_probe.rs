//! Can a FITS tab be closed programmatically?
//!
//! QA report bug 7: `close_active_tab` answered `closed: false` four times over
//! on four zombie tabs, with no error text, and the documented
//! `switch_fits_tab` + `close_active_tab` sequence could not work —
//! `switch_fits_tab` moves the VIEWER's focus, not the app's.
//!
//!     cargo run --example fits_close_probe -- <file.fits>
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;
use serde_json::json;
use verbinal::state::AppServices;
use verbinal::ui::fits_viewer::FitsViewer;

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: fits_close_probe <file.fits>");
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let handle = rt.handle().clone();
    let _guard = rt.enter();

    let app = adw::Application::builder()
        .application_id("net.canfar.verbinal.fitscloseprobe")
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

        let path = path.clone();
        let viewer = viewer.clone();
        gtk4::glib::spawn_future_local(async move {
            // Three tabs on the same file, as the report's zombie tabs were.
            for _ in 0..3 {
                viewer
                    .load_from_path(std::path::Path::new(&path))
                    .expect("load");
            }
            let before = viewer
                .handle_viewer_command("get_fits_view", &json!({}))
                .await
                .expect("view");
            println!("opened:        tabCount={}", before["tabCount"]);

            // Close a specific tab.
            let closed = viewer
                .handle_viewer_command("close_fits_tab", &json!({ "tabIndex": 0 }))
                .await;
            match &closed {
                Ok(v) => println!(
                    "close index 0: closed={} tabCount={}",
                    v["closed"], v["tabCount"]
                ),
                Err(e) => println!("close index 0: ERROR {e}"),
            }

            // And the active one, with no index.
            let closed2 = viewer
                .handle_viewer_command("close_fits_tab", &json!({}))
                .await;
            match &closed2 {
                Ok(v) => println!(
                    "close active:  closed={} tabCount={}",
                    v["closed"], v["tabCount"]
                ),
                Err(e) => println!("close active:  ERROR {e}"),
            }

            // Out of range must say so rather than refuse silently.
            let bad = viewer
                .handle_viewer_command("close_fits_tab", &json!({ "tabIndex": 99 }))
                .await;
            println!(
                "close index 99: {}",
                match &bad {
                    Ok(v) => format!("accepted?! {v}"),
                    Err(e) => format!("refused with a reason: {e}"),
                }
            );

            let after = viewer
                .handle_viewer_command("get_fits_view", &json!({}))
                .await;
            let remaining = after
                .map(|v| v["tabCount"].as_u64().unwrap_or(9))
                .unwrap_or(0);
            println!("remaining:     tabCount={remaining}");

            let ok = remaining == 1 && bad.is_err();
            println!(
                "\n{}",
                if ok {
                    "PASS: tabs close, and a bad index is refused with a reason"
                } else {
                    "FAIL"
                }
            );
            std::process::exit(if ok { 0 } else { 1 });
        });
    });

    app.run_with_args::<&str>(&[]);
}
