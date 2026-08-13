//! Guided "Connect an AI agent" wizard: Enable → Pick your client → Configure →
//! Verify. A GTK4 / libadwaita port of the Windows `AiConnectWizardDialog`, built
//! over the existing plumbing: [`McpHost`](crate::mcp::host::McpHost) to run the
//! server, [`crate::mcp::config`] to register Verbinal with Claude Desktop / Code,
//! and [`crate::mcp::selftest`] to prove the round trip.
//!
//! The four steps are pages of a [`gtk::Stack`]; a bottom Back/Next bar drives
//! navigation. Like the reference, the wizard resumes at the furthest sensible
//! step: straight to Verify when the server is up and a client is already
//! configured, to Pick-client when only the server is up, otherwise Enable.

use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use crate::state::AppServices;

const TITLES: [&str; 4] = ["Enable", "Pick your client", "Configure", "Verify"];
const PAGES: [&str; 4] = ["enable", "client", "configure", "verify"];

/// Present the modal connect wizard, transient for `parent`'s toplevel window.
///
/// Fire-and-forget: the window owns itself once presented. Starting the MCP
/// server is intentionally durable — it keeps running after the wizard closes.
pub fn show_connect_wizard(parent: &impl IsA<gtk::Widget>, services: Arc<AppServices>) {
    let window = adw::Window::builder()
        .title(crate::tr_en!("Connect an AI agent"))
        .default_width(540)
        .default_height(560)
        .width_request(400)
        .height_request(480)
        .resizable(true)
        .modal(true)
        .build();

    if let Some(root) = parent.root().and_downcast::<gtk::Window>() {
        window.set_transient_for(Some(&root));
    }

    let toolbar_view = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    toolbar_view.add_top_bar(&header);

    // ── Body: a step heading plus the stack of panels. ──────────────────────
    let body = gtk::Box::new(gtk::Orientation::Vertical, 12);
    body.set_margin_start(18);
    body.set_margin_end(18);
    body.set_margin_top(12);
    body.set_margin_bottom(6);

    let header_label = gtk::Label::new(None);
    header_label.set_xalign(0.0);
    header_label.add_css_class("title-4");
    body.append(&header_label);

    let stack = gtk::Stack::new();
    stack.set_transition_type(gtk::StackTransitionType::SlideLeftRight);
    stack.set_vexpand(true);

    // ── Step 1: Enable ──────────────────────────────────────────────────────
    let status_label = gtk::Label::new(None);
    status_label.set_xalign(0.0);
    status_label.set_wrap(true);
    status_label.add_css_class("dim-label");
    let start_btn = gtk::Button::with_label(crate::tr_en!("Start MCP server"));
    start_btn.add_css_class("suggested-action");
    let enable_page = {
        let page = gtk::Box::new(gtk::Orientation::Vertical, 12);
        let intro = gtk::Label::new(Some(crate::tr_en!(
            "The Model Context Protocol (MCP) lets an AI agent such as Claude talk to \
Verbinal — browsing your CADC storage, running searches, and preparing session \
launches on your behalf. Start the local MCP server so Verbinal becomes reachable."
        )));
        intro.set_wrap(true);
        intro.set_xalign(0.0);
        page.append(&intro);
        let btn_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        btn_row.set_halign(gtk::Align::Start);
        btn_row.append(&start_btn);
        page.append(&btn_row);
        page.append(&status_label);
        page
    };
    stack.add_named(&enable_page, Some(PAGES[0]));

    // ── Step 2: Pick your client ────────────────────────────────────────────
    let desktop_radio = gtk::CheckButton::with_label(crate::tr_en!("Claude Desktop"));
    let code_radio = gtk::CheckButton::with_label(crate::tr_en!("Claude Code CLI"));
    code_radio.set_group(Some(&desktop_radio));
    desktop_radio.set_active(true);
    let client_page = {
        let page = gtk::Box::new(gtk::Orientation::Vertical, 12);
        let intro = gtk::Label::new(Some(crate::tr_en!(
            "Which AI client will connect to Verbinal?"
        )));
        intro.set_wrap(true);
        intro.set_xalign(0.0);
        page.append(&intro);
        page.append(&desktop_radio);
        page.append(&code_radio);
        page
    };
    stack.add_named(&client_page, Some(PAGES[1]));

    // ── Step 3: Configure ───────────────────────────────────────────────────
    let cfg_path_str = crate::mcp::config::claude_desktop_config_path()
        .to_string_lossy()
        .to_string();
    let code_cmd = crate::mcp::config::claude_code_add_command();

    let config_result = gtk::Label::new(None);
    config_result.set_xalign(0.0);
    config_result.set_wrap(true);
    config_result.add_css_class("dim-label");

    // Desktop sub-panel: write the config file for the user.
    let write_btn = gtk::Button::with_label(crate::tr_en!("Write config"));
    write_btn.add_css_class("suggested-action");
    let desktop_box = {
        let b = gtk::Box::new(gtk::Orientation::Vertical, 8);
        let intro = gtk::Label::new(Some(crate::tr_en!(
            "Register Verbinal in Claude Desktop's configuration file. Claude Desktop \
picks this up the next time it launches."
        )));
        intro.set_wrap(true);
        intro.set_xalign(0.0);
        b.append(&intro);
        let path_label = gtk::Label::new(Some(&cfg_path_str));
        path_label.set_xalign(0.0);
        path_label.set_wrap(true);
        path_label.set_selectable(true);
        path_label.add_css_class("monospace");
        path_label.add_css_class("dim-label");
        b.append(&path_label);
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.set_halign(gtk::Align::Start);
        row.append(&write_btn);
        b.append(&row);
        b
    };

    // Code sub-panel: show the command for the user to run.
    let copy_btn = gtk::Button::with_label(crate::tr_en!("Copy command"));
    let code_box = {
        let b = gtk::Box::new(gtk::Orientation::Vertical, 8);
        let intro = gtk::Label::new(Some(crate::tr_en!(
            "Add Verbinal to Claude Code by running this command in your terminal:"
        )));
        intro.set_wrap(true);
        intro.set_xalign(0.0);
        b.append(&intro);
        let cmd_label = gtk::Label::new(Some(&code_cmd));
        cmd_label.set_xalign(0.0);
        cmd_label.set_wrap(true);
        cmd_label.set_selectable(true);
        cmd_label.add_css_class("monospace");
        cmd_label.set_margin_start(8);
        cmd_label.set_margin_end(8);
        cmd_label.set_margin_top(8);
        cmd_label.set_margin_bottom(8);
        let frame = gtk::Frame::new(None);
        frame.set_child(Some(&cmd_label));
        b.append(&frame);
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.set_halign(gtk::Align::Start);
        row.append(&copy_btn);
        b.append(&row);
        b
    };

    let configure_page = {
        let page = gtk::Box::new(gtk::Orientation::Vertical, 12);
        page.append(&desktop_box);
        page.append(&code_box);
        page.append(&config_result);
        page
    };
    stack.add_named(&configure_page, Some(PAGES[2]));

    // ── Step 4: Verify ──────────────────────────────────────────────────────
    let test_btn = gtk::Button::with_label(crate::tr_en!("Test connection"));
    test_btn.add_css_class("suggested-action");
    let spinner = gtk::Spinner::new();
    spinner.set_visible(false);
    let verify_label = gtk::Label::new(None);
    verify_label.set_xalign(0.0);
    verify_label.set_wrap(true);
    verify_label.add_css_class("dim-label");
    let verify_page = {
        let page = gtk::Box::new(gtk::Orientation::Vertical, 12);
        let intro = gtk::Label::new(Some(crate::tr_en!(
            "Dial the MCP server the way your AI client will, and confirm it answers."
        )));
        intro.set_wrap(true);
        intro.set_xalign(0.0);
        page.append(&intro);
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.set_halign(gtk::Align::Start);
        row.append(&test_btn);
        row.append(&spinner);
        page.append(&row);
        page.append(&verify_label);
        page
    };
    stack.add_named(&verify_page, Some(PAGES[3]));

    body.append(&stack);

    // ── Footer: Back / Next ─────────────────────────────────────────────────
    // Pinned at the bottom of the body (below the vexpanding step stack) so the
    // proceed button is always visible even on short / HiDPI-scaled displays.
    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    footer.set_margin_top(6);
    let back_btn = gtk::Button::with_label(crate::tr_en!("Back"));
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    let next_btn = gtk::Button::with_label(crate::tr_en!("Next"));
    next_btn.add_css_class("suggested-action");
    footer.append(&back_btn);
    footer.append(&spacer);
    footer.append(&next_btn);
    body.append(&footer);

    toolbar_view.set_content(Some(&body));
    window.set_content(Some(&toolbar_view));

    // ── Navigation state + step renderer ────────────────────────────────────
    let step = Rc::new(Cell::new(0i32));
    let apply_step: Rc<dyn Fn(i32)> = {
        let header_label = header_label.clone();
        let stack = stack.clone();
        let back_btn = back_btn.clone();
        let next_btn = next_btn.clone();
        let desktop_radio = desktop_radio.clone();
        let desktop_box = desktop_box.clone();
        let code_box = code_box.clone();
        let step = step.clone();
        Rc::new(move |n: i32| {
            let n = n.clamp(0, (TITLES.len() - 1) as i32);
            step.set(n);
            header_label.set_text(&crate::tr_fmt!(
                "Step {} of {} — {}",
                n + 1,
                TITLES.len(),
                TITLES[n as usize]
            ));
            stack.set_visible_child_name(PAGES[n as usize]);
            back_btn.set_sensitive(n > 0);
            next_btn.set_label(if n as usize == TITLES.len() - 1 {
                "Done"
            } else {
                "Next"
            });
            if n == 2 {
                // Show only the sub-panel for the chosen client.
                let desktop = desktop_radio.is_active();
                desktop_box.set_visible(desktop);
                code_box.set_visible(!desktop);
            }
        })
    };

    // Back: step back, never below 0.
    back_btn.connect_clicked({
        let apply_step = apply_step.clone();
        let step = step.clone();
        move |_| {
            let s = step.get();
            if s > 0 {
                apply_step(s - 1);
            }
        }
    });

    // Next: gate step 0 on the server actually running; Done closes the window.
    next_btn.connect_clicked({
        let apply_step = apply_step.clone();
        let step = step.clone();
        let window = window.clone();
        let services = services.clone();
        let status_label = status_label.clone();
        move |_| {
            let s = step.get();
            if s == 0 && !services.mcp_host.is_running() {
                status_label.add_css_class("error");
                status_label.set_text(crate::tr_en!("Start the MCP server to continue."));
                return;
            }
            if s as usize >= TITLES.len() - 1 {
                window.close();
                return;
            }
            apply_step(s + 1);
        }
    });

    // Start MCP server (step 1).
    start_btn.connect_clicked({
        let services = services.clone();
        let status_label = status_label.clone();
        move |btn| {
            let gate: Arc<dyn crate::mcp::server::ApprovalGate> = Arc::new(
                crate::mcp::client_approval::ApprovalStoreGate::new(services.mcp_clients.clone()),
            );
            services.mcp_host.start(services.clone(), gate);
            // Remember it's on so it auto-starts on the next launch.
            crate::services::mcp_settings_service::McpSettingsService::new()
                .set_server_enabled(true);
            status_label.remove_css_class("error");
            status_label.set_text(crate::tr_en!("MCP server is running."));
            btn.set_sensitive(false);
            btn.set_label(crate::tr_en!("Server running"));
        }
    });

    // Write Claude Desktop config (step 3, desktop panel).
    write_btn.connect_clicked({
        let services = services.clone();
        let config_result = config_result.clone();
        move |_| match crate::mcp::config::apply_to_claude_desktop() {
            Ok(()) => {
                services.toast.toast(crate::tr_en!(
                    "Claude Desktop configured — restart it to connect."
                ));
                config_result.remove_css_class("error");
                config_result.set_text(crate::tr_en!("✓ Configuration written."));
            }
            Err(e) => {
                services
                    .toast
                    .toast(crate::tr_fmt!("Couldn't write config: {}", e));
                config_result.add_css_class("error");
                config_result.set_text(&format!("✗ {e}"));
            }
        }
    });

    // Copy Claude Code command (step 3, code panel).
    copy_btn.connect_clicked({
        let services = services.clone();
        let cmd = code_cmd.clone();
        move |btn| {
            btn.display().clipboard().set_text(&cmd);
            services
                .toast
                .toast(crate::tr_en!("Command copied to clipboard."));
        }
    });

    // Run the self-test (step 4). Bridge tokio → glib via services.spawn.
    test_btn.connect_clicked({
        let services = services.clone();
        let verify_label = verify_label.clone();
        let spinner = spinner.clone();
        move |btn| {
            btn.set_sensitive(false);
            spinner.set_visible(true);
            spinner.start();
            verify_label.remove_css_class("error");
            verify_label.set_text(crate::tr_en!("Testing…"));
            let services = services.clone();
            let verify_label = verify_label.clone();
            let spinner = spinner.clone();
            let btn = btn.clone();
            glib::spawn_future_local(async move {
                let result = services.spawn(crate::mcp::selftest::run_self_test()).await;
                spinner.stop();
                spinner.set_visible(false);
                btn.set_sensitive(true);
                if result.ok {
                    let name = result
                        .server_name
                        .unwrap_or_else(|| "MCP server".to_string());
                    let tools = match result.tool_count {
                        Some(n) => format!(" — {} tool{}", n, if n == 1 { "" } else { "s" }),
                        None => String::new(),
                    };
                    verify_label.remove_css_class("error");
                    verify_label.set_text(&format!("✓ {name}{tools}"));
                } else {
                    let err = result
                        .error
                        .unwrap_or_else(|| "server unreachable".to_string());
                    verify_label.add_css_class("error");
                    verify_label.set_text(&format!("✗ {err}"));
                }
            });
        }
    });

    // Reflect current server state on the Enable step before first paint.
    let running = services.mcp_host.is_running();
    if running {
        status_label.set_text(crate::tr_en!("MCP server is running."));
        start_btn.set_sensitive(false);
        start_btn.set_label(crate::tr_en!("Server running"));
    } else {
        status_label.set_text(crate::tr_en!("MCP server is stopped."));
    }

    // Resume at the furthest sensible step, mirroring the Windows wizard.
    let initial = if running {
        if crate::mcp::config::is_configured() {
            3
        } else {
            1
        }
    } else {
        0
    };
    apply_step(initial);

    window.present();
}
