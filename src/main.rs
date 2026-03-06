mod config;
mod helpers;
mod models;
mod services;
mod state;
mod ui;

use gtk4::prelude::*;
use libadwaita as adw;
use state::AppServices;

/// Debug log to file (visible without terminal)
pub fn log(msg: &str) {
    use std::io::Write;
    let path = std::env::temp_dir().join("canfar-debug.log");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let now = chrono::Local::now().format("%H:%M:%S%.3f");
        let _ = writeln!(f, "[{}] {}", now, msg);
    }
}

fn main() {
    // Start a background tokio runtime for async HTTP
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Failed to create tokio runtime");
    let handle = rt.handle().clone();

    // Keep runtime alive for the lifetime of the app
    let _rt_guard = rt;

    let app = adw::Application::builder()
        .application_id("net.canfar.Verbinal")
        .build();

    app.connect_activate(move |app| {
        // Load CSS
        let css = gtk4::CssProvider::new();
        css.load_from_string(include_str!("style.css"));
        gtk4::style_context_add_provider_for_display(
            &gtk4::gdk::Display::default().expect("Could not get default display"),
            &css,
            gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );

        let services = AppServices::new(handle.clone());
        ui::build_main_window(app, services);
    });

    app.run();
}
