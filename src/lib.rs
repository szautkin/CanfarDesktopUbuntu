//! Verbinal — a native CANFAR science portal.
//!
//! A library with a three-line binary in front of it. The modules were declared
//! in `main.rs` and therefore reachable only from inside the binary, so an
//! example could not import them; the probes that found the last two layout
//! bugs had to be handed a temporary `lib.rs` each time to see the real widget
//! tree. Now they can just use it.

pub mod config;
pub mod helpers;
pub mod i18n;
pub mod mcp;
pub mod models;
pub mod services;
pub mod state;
/// Source-scanning helpers for the layout guards. Test-only: nothing in the
/// shipped binary reads its own source.
#[cfg(test)]
pub mod testing;
pub mod ui;

use gtk4::prelude::*;
use libadwaita as adw;
use state::AppServices;

/// Start Verbinal.
///
/// The whole program, so the binary can be three lines. Everything else lives
/// in this library, which is what lets `examples/` measure the real widgets:
/// two layout bugs this week were found by walking the actual tree, and a probe
/// that can only reach a COPY of a component proves nothing about the
/// component.
pub fn run() {
    // MCP bridge mode: `verbinal mcp` runs a thin stdio<->socket relay that an MCP
    // client (Claude Desktop / Claude Code CLI) launches; it forwards to the app's
    // per-user UNIX socket. No GUI in this mode.
    if std::env::args().nth(1).as_deref() == Some("mcp") {
        let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
        let code = match rt.block_on(mcp::bridge::run_stdio_bridge()) {
            Ok(()) => 0,
            Err(_) => 1,
        };
        std::process::exit(code);
    }

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

        let (services, toast_rx) = AppServices::new(handle.clone());

        // Resolve the UI language from settings (system => environment locale).
        // Must happen before any widgets are built so tr!() returns the right text.
        let lang = i18n::lang_from_setting(&services.endpoints.config().language);
        i18n::set_lang(lang);

        // Auto-start the MCP server if it was left enabled, so an AI client
        // (Claude Desktop / Code) can connect on the next launch without the user
        // re-enabling it. Uses the persisted client-approval gate.
        if crate::services::mcp_settings_service::McpSettingsService::new().server_enabled() {
            let gate: std::sync::Arc<dyn crate::mcp::server::ApprovalGate> = std::sync::Arc::new(
                crate::mcp::client_approval::ApprovalStoreGate::new(services.mcp_clients.clone()),
            );
            services.mcp_host.start(services.clone(), gate);
        }

        // Theme is applied inside build_main_window from saved settings
        ui::build_main_window(app, services, toast_rx);
    });

    app.run();
}
