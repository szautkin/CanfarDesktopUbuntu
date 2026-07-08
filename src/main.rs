mod config;
mod helpers;
mod i18n;
mod mcp;
mod models;
mod services;
mod state;
mod ui;

use gtk4::prelude::*;
use libadwaita as adw;
use state::AppServices;

fn main() {
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
