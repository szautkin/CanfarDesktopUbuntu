//! Verbinal — a native CANFAR science portal.
//!
//! A library with a three-line binary in front of it. The modules were declared
//! in `main.rs` and therefore reachable only from inside the binary, so an
//! example could not import them; the probes that found the last two layout
//! bugs had to be handed a temporary `lib.rs` each time to see the real widget
//! tree. Now they can just use it.

pub mod config;
pub mod desktop_entry;
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
/// `assets/icons` in the build tree the running binary came out of, if any.
///
/// Walks up from the executable looking for the directory, because how deep it
/// is depends on what is running: `target/release/verbinal` is three levels
/// down, `target/debug/examples/some_probe` is four, and a test binary in
/// `target/debug/deps/` is four as well. A fixed number of `parent()` calls
/// would be right for exactly one of them.
///
/// An installed binary is `/usr/bin/verbinal`; no ancestor of it holds an
/// `assets/icons`, so the answer is `None` and nothing is added — which is
/// correct, since the package puts the icons in the system theme.
///
/// Runtime, not compile time, deliberately. `CARGO_MANIFEST_DIR` bakes the
/// build machine's layout into a shipped binary, and gating that behind
/// `debug_assertions` drops the path from `cargo build --release` — a
/// configuration people actually run, and how the AI Guide lost its icon in
/// the sidebar and on the home page.
pub fn source_tree_icons() -> Option<std::path::PathBuf> {
    source_tree_asset("icons")
}

/// `assets/<kind>` in the build tree the running binary came out of.
///
/// The same walk for icons and for sounds, because it answers the same
/// question: is this a binary running out of a checkout, and if so where is the
/// checkout.
pub fn source_tree_asset(kind: &str) -> Option<std::path::PathBuf> {
    source_tree_asset_from(&std::env::current_exe().ok()?, kind)
}

/// The search, split out so it can be tested against a path rather than
/// against wherever the test binary happens to live.
fn source_tree_asset_from(exe: &std::path::Path, kind: &str) -> Option<std::path::PathBuf> {
    // Bounded: an unbounded walk ends at `/`, and `/assets/icons` on a machine
    // that happened to have one is not this application's icon theme.
    exe.ancestors()
        .skip(1)
        .take(5)
        .map(|dir| dir.join("assets").join(kind))
        .find(|dir| dir.is_dir())
}

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

        // Let GTK find our own icons by name when the app is running from a
        // build tree, where they are not in the system theme yet.
        //
        // Derived from the RUNNING executable, not from `CARGO_MANIFEST_DIR`.
        // That constant is baked in at compile time, so in a released binary it
        // names a directory on the machine that built it — and gating it behind
        // `debug_assertions` instead, as this once did, silently dropped the
        // path from `cargo build --release`, which is how `verbinal-agent-symbolic`
        // stopped rendering and the AI Guide lost its icon in the sidebar and on
        // the home page. `target/<profile>/verbinal` is two levels below the
        // tree either way, so one rule covers both profiles and embeds nothing.
        //
        // An installed binary in `/usr/bin` finds no such directory and the call
        // is skipped, which is correct: the package puts the icons in the system
        // theme, where they are found without any of this.
        //
        // Note this never fixes the WINDOW's icon: on Wayland the shell
        // resolves that by matching the window's app_id to an installed
        // desktop entry, which no amount of in-process theme search can stand
        // in for. See `dev-install.sh`.
        if let Some(icons) = source_tree_icons() {
            let display = gtk4::gdk::Display::default().expect("Could not get default display");
            gtk4::IconTheme::for_display(&display).add_search_path(icons);
        }

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

#[cfg(test)]
mod icon_path_tests {
    use super::source_tree_asset_from;
    use std::path::Path;

    /// Found from every place cargo puts a binary.
    ///
    /// The regression this exists for was invisible in `cargo test` and in
    /// every probe, because those run from `target/debug/...` where the old
    /// fixed-depth lookup happened to be wrong in a way nobody was checking —
    /// and it only showed up as a blank icon in a release build run from the
    /// tree.
    #[test]
    fn the_icons_are_found_from_wherever_cargo_puts_the_binary() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        for rel in [
            "target/release/verbinal",
            "target/debug/verbinal",
            "target/debug/examples/icon_render_probe",
            "target/debug/deps/verbinal-0123456789abcdef",
        ] {
            let found = source_tree_asset_from(&root.join(rel), "icons");
            assert_eq!(
                found.as_deref(),
                Some(root.join("assets").join("icons").as_path()),
                "the icons were not found from {rel}"
            );
        }
    }

    /// An installed binary finds nothing, and asks for nothing.
    ///
    /// The path is a development convenience. On a user's machine the package
    /// has already put the icons in the system theme, and a search path
    /// pointing at a directory that does not exist is dead weight at best.
    #[test]
    fn an_installed_binary_adds_no_search_path() {
        assert_eq!(
            source_tree_asset_from(Path::new("/usr/bin/verbinal"), "icons"),
            None
        );
    }
}
