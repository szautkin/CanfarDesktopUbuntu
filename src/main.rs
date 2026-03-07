mod config;
mod helpers;
mod models;
mod services;
mod state;
mod ui;

use gtk4::prelude::*;
use libadwaita as adw;
use state::AppServices;

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

        // Register the Verbinal icon so GTK can find it by name
        let display = gtk4::gdk::Display::default().expect("Could not get default display");
        let theme = gtk4::IconTheme::for_display(&display);
        theme.add_search_path(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("assets")
                .join("icons"),
        );

        let services = AppServices::new(handle.clone());
        ui::build_main_window(app, services);
    });

    app.run();
}
