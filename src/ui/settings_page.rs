use crate::config::{api_endpoint_defaults as dflt, AppConfig};
use crate::helpers::registry_credential_test::{test_registry_credentials, CredTestResult};
use crate::models::session::INTERACTIVE_SESSION_TYPES;
use crate::services::ai_compute_service::AIComputeService;
use crate::services::image_discovery_settings_service::ImageDiscoverySettingsService;
use crate::services::mcp_settings_service::{McpSettingsService, PortalDefaultsService};
use crate::state::AppServices;
use crate::ui::fit;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

/// Accessor/mutator pairs for the eight editable endpoint bases. Plain `fn`
/// pointers so they can be freely copied into signal closures.
type Getter = fn(&AppConfig) -> &String;
type Setter = fn(&mut AppConfig, String);

const ENDPOINT_FIELDS: [(&str, &str, Getter, Setter); 9] = [
    // Several mirrors, whitespace-separated and tried in order. Editable for the
    // same reason as the rest: two of the hostnames shipped previously had
    // become NXDOMAIN, and a constant in the binary left nobody a way around it.
    (
        "VizieR TAP mirrors",
        dflt::VIZIER_MIRRORS,
        |c| &c.vizier_mirrors,
        |c, v| c.vizier_mirrors = v,
    ),
    (
        "CADC login (ac)",
        dflt::LOGIN_BASE,
        |c| &c.login_base,
        |c, v| c.login_base = v,
    ),
    (
        "Skaha science platform",
        dflt::SKAHA_BASE,
        |c| &c.skaha_base,
        |c, v| c.skaha_base = v,
    ),
    (
        "User info (ac)",
        dflt::AC_BASE,
        |c| &c.ac_base,
        |c, v| c.ac_base = v,
    ),
    (
        "ARC nodes (VOSpace metadata)",
        dflt::ARC_NODES,
        |c| &c.arc_nodes,
        |c, v| c.arc_nodes = v,
    ),
    (
        "ARC files (VOSpace transfer)",
        dflt::ARC_FILES,
        |c| &c.arc_files,
        |c, v| c.arc_files = v,
    ),
    (
        "TAP (archive search)",
        dflt::TAP_BASE,
        |c| &c.tap_base,
        |c, v| c.tap_base = v,
    ),
    (
        "CAOM2 ops (DataLink)",
        dflt::CAOM2OPS_BASE,
        |c| &c.caom2ops_base,
        |c, v| c.caom2ops_base = v,
    ),
    (
        "Target resolver",
        dflt::RESOLVER_BASE,
        |c| &c.resolver_base,
        |c, v| c.resolver_base = v,
    ),
];

pub struct SettingsPage {
    pub widget: adw::PreferencesPage,
    services: Arc<AppServices>,
    config: Rc<RefCell<AppConfig>>,
}

/// Show `example` inside an empty entry row, as placeholder text.
///
/// `AdwEntryRow` has no placeholder of its own — an empty one shows its title
/// and nothing else, and a title is not an example. "Registry repository
/// (project)" gave no hint that it wanted `private-test` rather than
/// `private-test/verbinal-execution:0.0.1`, so a whole configuration read as
/// complete — credentials verified and all — with run_code switched off.
///
/// Takes any `Editable` so the password rows are covered by the same function:
/// `AdwPasswordEntryRow` is not an `AdwEntryRow`, but a secret is still a field
/// somebody has to fill in, and it still needs to say what it wants.
///
/// Reached through `Editable::delegate()`, the documented way to the row's
/// inner `GtkText`. Walking its children would be a guess about libadwaita's
/// internals that stops working without warning.
fn with_example(row: &impl IsA<gtk::Editable>, example: &str) {
    if let Some(text) = row.as_ref().delegate().and_downcast::<gtk::Text>() {
        text.set_placeholder_text(Some(example));
    }
}

impl SettingsPage {
    pub fn new(services: Arc<AppServices>) -> Self {
        let config = Rc::new(RefCell::new(services.settings.load()));
        let widget = adw::PreferencesPage::new();
        widget.set_title(crate::tr_en!("Settings"));
        widget.set_icon_name(Some("emblem-system-symbolic"));

        let page = SettingsPage {
            widget,
            services,
            config,
        };
        page.build_appearance_group();
        page.build_defaults_group();
        page.build_ai_group();
        page.build_connection_group();
        page.build_image_discovery_group();
        page.build_ai_compute_group();
        page.build_about_group();
        page
    }

    fn build_appearance_group(&self) {
        let group = adw::PreferencesGroup::new();
        group.set_title(crate::tr_en!("Appearance"));

        let theme_row = adw::ComboRow::new();
        theme_row.set_title(crate::tr_en!("Theme"));
        theme_row.set_subtitle(crate::tr_en!("Choose how Verbinal looks"));
        let themes = gtk::StringList::new(&[
            crate::tr_en!("System"),
            crate::tr_en!("Light"),
            crate::tr_en!("Dark"),
        ]);
        theme_row.set_model(Some(&themes));

        // Set initial value
        let current = self.config.borrow().theme.clone();
        let idx = match current.as_str() {
            "Light" => 1,
            "Dark" => 2,
            _ => 0,
        };
        theme_row.set_selected(idx);

        let config = self.config.clone();
        let services = self.services.clone();
        theme_row.connect_selected_notify(move |row| {
            let theme = match row.selected() {
                1 => "Light",
                2 => "Dark",
                _ => "System",
            };
            config.borrow_mut().theme = theme.to_string();
            apply_theme(theme);
            let _ = services.settings.save(&config.borrow());
        });

        group.add(&theme_row);

        // Language (applied after restart, matching the Windows reference).
        let lang_row = adw::ComboRow::new();
        lang_row.set_title(crate::tr_en!("Language"));
        lang_row.set_subtitle(crate::tr_en!("Applied after restart"));
        let langs = gtk::StringList::new(&[
            crate::tr_en!("System default"),
            crate::tr_en!("English"),
            "Français",
        ]);
        lang_row.set_model(Some(&langs));
        let current_lang = self.config.borrow().language.clone();
        let lang_idx = match current_lang.as_str() {
            "en" => 1,
            "fr" => 2,
            _ => 0,
        };
        lang_row.set_selected(lang_idx);

        let config = self.config.clone();
        let services = self.services.clone();
        lang_row.connect_selected_notify(move |row| {
            let lang = match row.selected() {
                1 => "en",
                2 => "fr",
                _ => "system",
            };
            config.borrow_mut().language = lang.to_string();
            let _ = services.settings.save(&config.borrow());
            services.toast.toast(crate::tr_en!(
                "Language change will take effect after restart"
            ));
        });
        group.add(&lang_row);

        // Agent sounds. On by default: the point of a cue is that it reaches
        // someone who is looking at something else, and a cue nobody has turned
        // on reaches nobody.
        let sound_row = adw::SwitchRow::new();
        sound_row.set_title(crate::tr_en!("Agent sounds"));
        sound_row.set_subtitle(crate::tr_en!(
            "A short sound when an AI agent starts and stops working"
        ));
        sound_row.set_active(self.config.borrow().agent_sounds);
        {
            let config = self.config.clone();
            let services = self.services.clone();
            sound_row.connect_active_notify(move |row| {
                let on = row.is_active();
                config.borrow_mut().agent_sounds = on;
                let _ = services.settings.save(&config.borrow());
                // Play the cue the switch just enabled, so "on" is something
                // you hear rather than something you take on trust.
                if on {
                    crate::ui::sound::play(crate::ui::sound::Cue::AgentStarted);
                }
            });
        }
        group.add(&sound_row);

        self.widget.add(&group);
    }

    fn build_defaults_group(&self) {
        let group = adw::PreferencesGroup::new();
        group.set_title(crate::tr_en!("Session Defaults"));
        group.set_description(Some(crate::tr_en!(
            "Default values for new session launches"
        )));

        // Session type / cores / RAM persist in AppConfig; the resource preset and
        // GPU count are not AppConfig fields (grepped), so they live in a sibling
        // JSON store shared across these closures behind an Rc.
        let portal = Rc::new(PortalDefaultsService::new());

        // Default session type (AppConfig).
        let type_row = adw::ComboRow::new();
        type_row.set_title(crate::tr_en!("Session Type"));
        let types = gtk::StringList::new(&INTERACTIVE_SESSION_TYPES);
        type_row.set_model(Some(&types));

        let current_type = self.config.borrow().default_session_type.clone();
        let type_names = INTERACTIVE_SESSION_TYPES;
        let type_idx = type_names
            .iter()
            .position(|&t| t == current_type)
            .unwrap_or(0) as u32;
        type_row.set_selected(type_idx);

        {
            let config = self.config.clone();
            let services = self.services.clone();
            type_row.connect_selected_notify(move |row| {
                let selected = row.selected() as usize;
                if selected < type_names.len() {
                    config.borrow_mut().default_session_type = type_names[selected].to_string();
                    let _ = services.settings.save(&config.borrow());
                }
            });
        }
        group.add(&type_row);

        // Resource preset (PortalDefaults): none / flexible / fixed. The explicit
        // cores + RAM rows are only meaningful for "fixed", so they are shown only
        // then — matching the macOS/Windows Portal tab.
        const PRESET_TAGS: [&str; 3] = ["none", "flexible", "fixed"];
        let preset_row = adw::ComboRow::new();
        preset_row.set_title(crate::tr_en!("Resource preset"));
        preset_row.set_subtitle(crate::tr_en!("\"Fixed\" reveals explicit cores and RAM"));
        let presets = gtk::StringList::new(&[
            crate::tr_en!("None"),
            crate::tr_en!("Flexible"),
            crate::tr_en!("Fixed"),
        ]);
        preset_row.set_model(Some(&presets));
        let current_preset = portal.resource_type();
        let preset_idx = PRESET_TAGS
            .iter()
            .position(|&t| t == current_preset)
            .unwrap_or(0) as u32;
        preset_row.set_selected(preset_idx);
        group.add(&preset_row);

        // Default CPU cores (AppConfig) — cap raised to 64.
        let cores_row = adw::SpinRow::new(
            Some(&gtk::Adjustment::new(
                self.config.borrow().default_cores as f64,
                1.0,
                64.0,
                1.0,
                1.0,
                0.0,
            )),
            1.0,
            0,
        );
        cores_row.set_title(crate::tr_en!("Default CPU Cores"));
        {
            let config = self.config.clone();
            let services = self.services.clone();
            cores_row.connect_value_notify(move |row| {
                config.borrow_mut().default_cores = row.value() as u32;
                let _ = services.settings.save(&config.borrow());
            });
        }
        group.add(&cores_row);

        // Default RAM (AppConfig).
        let ram_row = adw::SpinRow::new(
            Some(&gtk::Adjustment::new(
                self.config.borrow().default_ram as f64,
                1.0,
                256.0,
                1.0,
                4.0,
                0.0,
            )),
            1.0,
            0,
        );
        ram_row.set_title(crate::tr_en!("Default RAM (GB)"));
        {
            let config = self.config.clone();
            let services = self.services.clone();
            ram_row.connect_value_notify(move |row| {
                config.borrow_mut().default_ram = row.value() as u32;
                let _ = services.settings.save(&config.borrow());
            });
        }
        group.add(&ram_row);

        // Default GPUs (PortalDefaults) — always visible.
        let gpus_row = adw::SpinRow::new(
            Some(&gtk::Adjustment::new(
                portal.gpus() as f64,
                0.0,
                16.0,
                1.0,
                1.0,
                0.0,
            )),
            1.0,
            0,
        );
        gpus_row.set_title(crate::tr_en!("Default GPUs"));
        {
            let portal = portal.clone();
            gpus_row.connect_value_notify(move |row| {
                portal.set_gpus(row.value() as u32);
            });
        }
        group.add(&gpus_row);

        // The fixed cores/RAM rows track the preset. Seed the visibility now and
        // update it (plus persist the tag) whenever the preset changes.
        let update_fixed = {
            let cores_row = cores_row.clone();
            let ram_row = ram_row.clone();
            move |tag: &str| {
                let fixed = tag == "fixed";
                cores_row.set_visible(fixed);
                ram_row.set_visible(fixed);
            }
        };
        update_fixed(&current_preset);
        {
            let portal = portal.clone();
            let update_fixed = update_fixed.clone();
            preset_row.connect_selected_notify(move |row| {
                let tag = PRESET_TAGS
                    .get(row.selected() as usize)
                    .copied()
                    .unwrap_or("none");
                portal.set_resource_type(tag);
                update_fixed(tag);
            });
        }

        // Clear all defaults — restore the built-in session-launch defaults.
        let clear_row = adw::ActionRow::new();
        clear_row.set_title(crate::tr_en!("Clear all defaults"));
        clear_row.set_subtitle(crate::tr_en!(
            "Restore the built-in session-launch defaults"
        ));
        let clear_btn = gtk::Button::with_label(crate::tr_en!("Clear"));
        clear_btn.add_css_class("destructive-action");
        clear_btn.set_valign(gtk::Align::Center);
        {
            let config = self.config.clone();
            let services = self.services.clone();
            let portal = portal.clone();
            let type_row = type_row.clone();
            let preset_row = preset_row.clone();
            let cores_row = cores_row.clone();
            let ram_row = ram_row.clone();
            let gpus_row = gpus_row.clone();
            clear_btn.connect_clicked(move |_| {
                // Reset the AppConfig-backed knobs in one scoped borrow, then save.
                {
                    let mut c = config.borrow_mut();
                    c.default_session_type = "notebook".to_string();
                    c.default_cores = 2;
                    c.default_ram = 8;
                }
                let _ = services.settings.save(&config.borrow());
                portal.clear();
                // Push the values back into the widgets. Each set may re-fire the
                // notify handler (an idempotent re-save); no borrow is held here.
                type_row.set_selected(0); // notebook
                preset_row.set_selected(0); // none
                cores_row.set_value(2.0);
                ram_row.set_value(8.0);
                gpus_row.set_value(0.0);
            });
        }
        clear_row.add_suffix(&clear_btn);
        clear_row.set_activatable_widget(Some(&clear_btn));
        group.add(&clear_row);

        self.widget.add(&group);
    }

    fn build_ai_group(&self) {
        let group = adw::PreferencesGroup::new();
        group.set_title(crate::tr_en!("AI Assistant (MCP)"));
        group.set_description(Some(crate::tr_en!(
            "Let an AI agent (Claude Desktop / Claude Code) drive Verbinal over MCP"
        )));

        // Persisted agent-autonomy knobs (auto-apply / follow-activity / guide tile),
        // shared across the toggle closures behind an Rc.
        let mcp_settings = Rc::new(McpSettingsService::new());

        // Enable/disable the MCP server.
        let server_row = adw::SwitchRow::new();
        server_row.set_title(crate::tr_en!("MCP server"));
        server_row.set_subtitle(crate::tr_en!("Listens on a private per-user socket"));
        server_row.set_active(self.services.mcp_host.is_running());
        group.add(&server_row);

        // Running status + the socket path the bridge connects to.
        let status_row = adw::ActionRow::new();
        status_row.set_title(crate::tr_en!("Server status"));
        status_row.add_prefix(&gtk::Image::from_icon_name("network-server-symbolic"));
        status_row.set_subtitle_lines(0);
        group.add(&status_row);

        // A reusable status refresher — called now and after each server toggle.
        let refresh_status = {
            let services = self.services.clone();
            let status_row = status_row.clone();
            move || {
                if services.mcp_host.is_running() {
                    let sock = crate::mcp::socket_path::socket_path();
                    status_row.set_use_markup(false);
                    status_row.set_subtitle(&format!(
                        "{} — {}",
                        crate::tr_en!("Running"),
                        sock.display()
                    ));
                } else {
                    status_row.set_subtitle(crate::tr_en!("Stopped"));
                }
            }
        };
        refresh_status();

        {
            let services = self.services.clone();
            let refresh_status = refresh_status.clone();
            server_row.connect_active_notify(move |row| {
                if row.is_active() {
                    let gate: std::sync::Arc<dyn crate::mcp::server::ApprovalGate> =
                        std::sync::Arc::new(crate::mcp::client_approval::ApprovalStoreGate::new(
                            services.mcp_clients.clone(),
                        ));
                    services.mcp_host.start(services.clone(), gate);
                    crate::services::mcp_settings_service::McpSettingsService::new()
                        .set_server_enabled(true);
                    services.toast.toast(crate::tr_en!("MCP server started"));
                } else {
                    services.mcp_host.stop();
                    crate::services::mcp_settings_service::McpSettingsService::new()
                        .set_server_enabled(false);
                    services.toast.toast(crate::tr_en!("MCP server stopped"));
                }
                refresh_status();
            });
        }

        // Auto-apply agent writes (McpSettings).
        let auto_apply_row = adw::SwitchRow::new();
        auto_apply_row.set_title(crate::tr_en!("Auto-apply agent writes"));
        auto_apply_row.set_subtitle(crate::tr_en!(
            "Apply agent write proposals immediately instead of queuing them for review"
        ));
        auto_apply_row.set_active(mcp_settings.auto_apply_enabled());
        {
            let mcp_settings = mcp_settings.clone();
            let services = self.services.clone();
            auto_apply_row.connect_active_notify(move |row| {
                mcp_settings.set_auto_apply_enabled(row.is_active());
                // Update the live flag the router consults (mirrors the persisted value).
                services
                    .mcp_auto_apply
                    .store(row.is_active(), std::sync::atomic::Ordering::Relaxed);
            });
        }
        group.add(&auto_apply_row);

        // Require approval for new clients (McpClientApprovalStore).
        let require_row = adw::SwitchRow::new();
        require_row.set_title(crate::tr_en!("Require approval for new clients"));
        require_row.set_subtitle(crate::tr_en!(
            "Only approved clients may connect; new ones are held for review"
        ));
        require_row.set_active(self.services.mcp_clients.require_approval());
        {
            let services = self.services.clone();
            require_row.connect_active_notify(move |row| {
                services.mcp_clients.set_require_approval(row.is_active());
            });
        }
        group.add(&require_row);

        // Follow agent activity (McpSettings).
        let follow_row = adw::SwitchRow::new();
        follow_row.set_title(crate::tr_en!("Follow agent activity"));
        follow_row.set_subtitle(crate::tr_en!("Navigate to the view an agent just changed"));
        follow_row.set_active(mcp_settings.follow_activity_enabled());
        {
            let mcp_settings = mcp_settings.clone();
            let services = self.services.clone();
            follow_row.connect_active_notify(move |row| {
                mcp_settings.set_follow_activity_enabled(row.is_active());
                services
                    .mcp_follow_activity
                    .store(row.is_active(), std::sync::atomic::Ordering::Relaxed);
            });
        }
        group.add(&follow_row);

        // Show the AI-Guide tile on the landing launchpad (McpSettings).
        let guide_row = adw::SwitchRow::new();
        guide_row.set_title(crate::tr_en!("Show AI Guide tile"));
        guide_row.set_subtitle(crate::tr_en!("Display the AI Guide tile on the launchpad"));
        guide_row.set_active(mcp_settings.show_ai_guide_tile());
        {
            let mcp_settings = mcp_settings.clone();
            guide_row.connect_active_notify(move |row| {
                mcp_settings.set_show_ai_guide_tile(row.is_active());
            });
        }
        group.add(&guide_row);

        // Launch the guided connect wizard.
        let wizard_row = adw::ActionRow::new();
        wizard_row.set_title(crate::tr_en!("Connect an agent…"));
        wizard_row.set_subtitle(crate::tr_en!("Pair Claude Desktop or Claude Code CLI"));
        let connect_btn = gtk::Button::with_label(crate::tr_en!("Connect"));
        connect_btn.add_css_class("suggested-action");
        connect_btn.set_valign(gtk::Align::Center);
        let services = self.services.clone();
        let page_widget = self.widget.clone();
        connect_btn.connect_clicked(move |_| {
            crate::ui::ai_connect_wizard::show_connect_wizard(&page_widget, services.clone());
        });
        wizard_row.add_suffix(&connect_btn);
        wizard_row.set_activatable_widget(Some(&connect_btn));
        group.add(&wizard_row);

        // Diagnostics — check the server, socket, bridge, and client config.
        let diag_row = adw::ActionRow::new();
        diag_row.set_title(crate::tr_en!("Diagnostics"));
        diag_row.set_subtitle(crate::tr_en!(
            "Check the MCP server, socket, bridge, and Claude client configuration"
        ));
        let diag_btn = gtk::Button::with_label(crate::tr_en!("Run"));
        diag_btn.add_css_class("flat");
        diag_btn.set_valign(gtk::Align::Center);
        let services = self.services.clone();
        let page_widget = self.widget.clone();
        diag_btn.connect_clicked(move |_| {
            show_mcp_diagnostics(&page_widget, &services);
        });
        diag_row.add_suffix(&diag_btn);
        diag_row.set_activatable_widget(Some(&diag_btn));
        group.add(&diag_row);

        self.widget.add(&group);

        // Connected clients (seen ∪ approved) with per-row Approve/Revoke, then
        // the recent agent-activity feed. Rendered as their own groups.
        self.build_mcp_clients_group();
        self.build_mcp_activity_group();
    }

    /// The connected-clients group: the union of approved + seen client ids, each
    /// with an Approve/Revoke button that mutates the allow-list and rebuilds the
    /// list in place. Mirrors `McpServerSettingsPanel.LoadClients`.
    fn build_mcp_clients_group(&self) {
        let group = adw::PreferencesGroup::new();
        group.set_title(crate::tr_en!("Connected clients"));
        group.set_description(Some(crate::tr_en!(
            "Agents that have connected; approve or revoke each"
        )));

        // A placeholder shown only when no client has been observed.
        let empty_row = adw::ActionRow::new();
        empty_row.set_title(crate::tr_en!("No clients yet"));
        empty_row.set_subtitle(crate::tr_en!("Connect an agent to see it here"));
        group.add(&empty_row);

        // Dynamic rows are tracked so a rebuild can remove the previous batch.
        let rows: Rc<RefCell<Vec<adw::ActionRow>>> = Rc::new(RefCell::new(Vec::new()));
        // Trampoline: a row's Approve/Revoke button re-invokes the refresh.
        let refresh_slot: crate::ui::SharedCallbackSlot<dyn Fn()> = Rc::new(RefCell::new(None));

        let refresh: Rc<dyn Fn()> = {
            let services = self.services.clone();
            let group = group.clone();
            let rows = rows.clone();
            let empty_row = empty_row.clone();
            let refresh_slot = refresh_slot.clone();
            Rc::new(move || {
                for old in rows.borrow_mut().drain(..) {
                    group.remove(&old);
                }
                // Union of approved + seen, de-duplicated, approved-first.
                let mut ids: Vec<String> = services.mcp_clients.approved_clients();
                for id in services.mcp_clients.seen_clients() {
                    if !ids.contains(&id) {
                        ids.push(id);
                    }
                }
                empty_row.set_visible(ids.is_empty());

                for id in ids {
                    let approved = services.mcp_clients.is_approved(&id);
                    let row = adw::ActionRow::new();
                    row.set_use_markup(false);
                    row.set_title(&id);
                    row.set_subtitle(if approved {
                        crate::tr_en!("Approved")
                    } else {
                        crate::tr_en!("Awaiting approval")
                    });
                    let btn = gtk::Button::with_label(if approved {
                        crate::tr_en!("Revoke")
                    } else {
                        crate::tr_en!("Approve")
                    });
                    btn.set_valign(gtk::Align::Center);
                    btn.add_css_class(if approved {
                        "destructive-action"
                    } else {
                        "suggested-action"
                    });
                    {
                        let services = services.clone();
                        let refresh_slot = refresh_slot.clone();
                        let id = id.clone();
                        btn.connect_clicked(move |_| {
                            if services.mcp_clients.is_approved(&id) {
                                services.mcp_clients.revoke(&id);
                            } else {
                                services.mcp_clients.approve(&id);
                            }
                            if let Some(f) = refresh_slot.borrow().clone() {
                                f();
                            }
                        });
                    }
                    row.add_suffix(&btn);
                    row.set_activatable_widget(Some(&btn));
                    group.add(&row);
                    rows.borrow_mut().push(row);
                }
            })
        };
        *refresh_slot.borrow_mut() = Some(refresh.clone());
        refresh();

        self.widget.add(&group);
    }

    /// The recent agent-activity feed: the last ten MCP tool calls, newest first.
    /// Mirrors `McpServerSettingsPanel.LoadActivity`.
    fn build_mcp_activity_group(&self) {
        let group = adw::PreferencesGroup::new();
        group.set_title(crate::tr_en!("Recent agent activity"));
        group.set_description(Some(crate::tr_en!(
            "The last few MCP tool calls made by an agent"
        )));

        let entries = crate::helpers::agent_activity::recent(10);
        if entries.is_empty() {
            let row = adw::ActionRow::new();
            row.set_title(crate::tr_en!("No recent activity"));
            group.add(&row);
        } else {
            for e in entries {
                let row = adw::ActionRow::new();
                row.set_use_markup(false);
                row.set_title(&e.tool);
                row.set_subtitle(&e.at);
                row.add_prefix(&gtk::Image::from_icon_name("document-open-recent-symbolic"));
                group.add(&row);
            }
        }

        self.widget.add(&group);
    }

    fn build_connection_group(&self) {
        let group = adw::PreferencesGroup::new();
        group.set_title(crate::tr_en!("Connection"));
        group.set_description(Some(crate::tr_en!("CANFAR / CADC service endpoints")));

        // Collapsible "Service endpoints" editor with one row per base.
        let expander = adw::ExpanderRow::new();
        expander.set_title(crate::tr_en!("Service endpoints"));
        expander.set_subtitle(crate::tr_en!(
            "Advanced — repoint the app at another deployment"
        ));

        let rows: Rc<RefCell<Vec<adw::EntryRow>>> = Rc::new(RefCell::new(Vec::new()));

        for &(title, default, getter, setter) in ENDPOINT_FIELDS.iter() {
            let row = adw::EntryRow::new();
            row.set_title(title);
            // The shipped default, as the example. An endpoint someone has
            // cleared then shows what it was — the one thing needed to put it
            // back, and already here in the table.
            with_example(&row, default);
            row.set_text(getter(&self.config.borrow()));

            let config = self.config.clone();
            let services = self.services.clone();
            row.connect_changed(move |r| {
                let text = r.text().to_string();
                setter(&mut config.borrow_mut(), text);
                let _ = services.settings.save(&config.borrow());
                // Live-apply: the next request from any service uses the new host.
                services.endpoints.apply_from(&config.borrow());
            });
            expander.add_row(&row);
            rows.borrow_mut().push(row);
        }
        group.add(&expander);

        // Buttons: Reset to defaults (endpoints only) + Test connections.
        let button_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        button_box.set_margin_top(8);
        button_box.set_halign(gtk::Align::Start);

        let reset_btn = gtk::Button::with_label(crate::tr_en!("Reset endpoints"));
        reset_btn.add_css_class("destructive-action");
        let test_btn = gtk::Button::with_label(crate::tr_en!("Test connections"));
        test_btn.add_css_class("suggested-action");
        button_box.append(&reset_btn);
        button_box.append(&test_btn);
        group.add(&button_box);

        // Results group (populated by the self-test).
        let results_group = adw::PreferencesGroup::new();
        results_group.set_title(crate::tr_en!("Connection test"));
        results_group.set_visible(false);
        self.widget.add(&group);
        self.widget.add(&results_group);
        let result_rows: Rc<RefCell<Vec<adw::ActionRow>>> = Rc::new(RefCell::new(Vec::new()));

        // Reset restores ONLY the endpoint fields — theme/language/session defaults survive.
        {
            let config = self.config.clone();
            let services = self.services.clone();
            let rows = rows.clone();
            reset_btn.connect_clicked(move |_| {
                config.borrow_mut().reset_endpoints();
                let _ = services.settings.save(&config.borrow());
                services.endpoints.reset_endpoints();
                // Snapshot the values without holding the borrow across set_text
                // (set_text re-fires connect_changed, which borrows config).
                let values: Vec<String> = {
                    let c = config.borrow();
                    ENDPOINT_FIELDS
                        .iter()
                        .map(|(_, _, g, _)| g(&c).clone())
                        .collect()
                };
                for (row, value) in rows.borrow().iter().zip(values.iter()) {
                    row.set_text(value);
                }
            });
        }

        // Test connections: probe all endpoints in parallel and render results.
        {
            let services = self.services.clone();
            let results_group = results_group.clone();
            let result_rows = result_rows.clone();
            test_btn.connect_clicked(move |btn| {
                btn.set_sensitive(false);
                btn.set_label(crate::tr_en!("Testing all endpoints…"));
                let services = services.clone();
                let btn = btn.clone();
                let results_group = results_group.clone();
                let result_rows = result_rows.clone();
                glib::spawn_future_local(async move {
                    let endpoints = services.endpoints.clone();
                    let results = services
                        .spawn(async move {
                            let client = reqwest::Client::new();
                            crate::services::probe_all(&client, &endpoints).await
                        })
                        .await;

                    // Clear any previous result rows.
                    for old in result_rows.borrow_mut().drain(..) {
                        results_group.remove(&old);
                    }
                    for r in &results {
                        let row = adw::ActionRow::new();
                        row.set_use_markup(false);
                        row.set_title(&r.name);
                        row.set_subtitle(&r.url);
                        let (icon, detail) = if r.ok {
                            (
                                "emblem-ok-symbolic",
                                crate::tr_fmt!(
                                    "reachable — {} ({} ms)",
                                    r.status.map(|s| s.to_string()).unwrap_or_default(),
                                    r.latency_ms
                                ),
                            )
                        } else if r.reachable {
                            // The host answered, but with a 404/5xx: the endpoint is
                            // wrong or the service is down. Reporting that as OK was
                            // exactly the reference's QA-F3 bug.
                            (
                                "dialog-warning-symbolic",
                                crate::tr_fmt!(
                                    "host up, service failed — HTTP {} ({} ms)",
                                    r.status.map(|s| s.to_string()).unwrap_or_default(),
                                    r.latency_ms
                                ),
                            )
                        } else {
                            (
                                "dialog-error-symbolic",
                                crate::tr_fmt!(
                                    "unreachable — {}",
                                    r.error.clone().unwrap_or_else(|| "error".to_string())
                                ),
                            )
                        };
                        row.add_prefix(&gtk::Image::from_icon_name(icon));
                        // The detail carries a transport error of unknown
                        // length; the tooltip keeps all of it.
                        let label = fit::status_label();
                        fit::set_status(&label, &detail);
                        row.add_suffix(&label);
                        results_group.add(&row);
                        result_rows.borrow_mut().push(row);
                    }
                    results_group.set_visible(true);
                    btn.set_sensitive(true);
                    btn.set_label(crate::tr_en!("Test connections"));
                });
            });
        }
    }

    /// "Image discovery" group — registry credentials + the inspector host
    /// image used to probe container images. Persists through a
    /// [`ImageDiscoverySettingsService`] (non-secret knobs as JSON, the secret
    /// in the OS keychain) and offers a "Test credentials" button that runs the
    /// Docker V2 token-auth dance off the GLib loop via [`AppServices::spawn`].
    ///
    /// Mirrors `Views/Dialogs/ImageDiscoverySettingsPanel.xaml(.cs)`.
    fn build_image_discovery_group(&self) {
        // The service is its own store (data_dir JSON + keychain), independent
        // of AppConfig. Shared across the row closures behind Rc<RefCell<…>>
        // because the setters take &mut self.
        let service = Rc::new(RefCell::new(ImageDiscoverySettingsService::new()));

        let group = adw::PreferencesGroup::new();
        group.set_title(crate::tr_en!("Image discovery"));
        group.set_description(Some(crate::tr_en!(
            "Registry credentials and the inspector host image used to probe container images"
        )));

        // Snapshot the persisted values to seed the rows.
        let (host0, repo0, user0, inspector0) = {
            let s = service.borrow();
            let st = s.settings();
            (
                st.registry_host.clone(),
                st.registry_repository.clone(),
                st.username.clone(),
                st.inspector_image.clone(),
            )
        };

        // Inspector image. Blank resets to the default (handled in the setter).
        let inspector_row = adw::EntryRow::new();
        inspector_row.set_title(crate::tr_en!("Inspector image"));
        with_example(
            &inspector_row,
            crate::tr_en!("e.g. skaha/terminal:1.1.2 — a headless image that can inspect others"),
        );
        inspector_row.set_text(&inspector0);
        {
            let service = service.clone();
            inspector_row.connect_changed(move |r| {
                service.borrow_mut().set_inspector_image(&r.text());
            });
        }
        group.add(&inspector_row);

        // Registry host.
        let host_row = adw::EntryRow::new();
        host_row.set_title(crate::tr_en!("Registry host"));
        with_example(&host_row, crate::tr_en!("e.g. images.canfar.net"));
        host_row.set_text(&host0);
        {
            let service = service.clone();
            host_row.connect_changed(move |r| {
                service.borrow_mut().set_registry_host(&r.text());
            });
        }
        group.add(&host_row);

        // Registry repository (project/namespace).
        let repo_row = adw::EntryRow::new();
        repo_row.set_title(crate::tr_en!("Registry repository (project)"));
        with_example(
            &repo_row,
            crate::tr_en!("e.g. private-test — the project only, no image name"),
        );
        repo_row.set_text(&repo0);
        {
            let service = service.clone();
            repo_row.connect_changed(move |r| {
                service.borrow_mut().set_registry_repository(&r.text());
            });
        }
        group.add(&repo_row);

        // Registry username.
        let username_row = adw::EntryRow::new();
        username_row.set_title(crate::tr_en!("Registry username"));
        with_example(&username_row, crate::tr_en!("Your CADC username"));
        username_row.set_text(&user0);
        {
            let service = service.clone();
            username_row.connect_changed(move |r| {
                service.borrow_mut().set_username(&r.text());
            });
        }
        group.add(&username_row);

        // Registry secret — never pre-filled; stored in the OS keychain. A
        // suffix label reports whether one is on file, plus a Remove button.
        let secret_row = adw::PasswordEntryRow::new();
        secret_row.set_title(crate::tr_en!("Registry secret (Harbor CLI secret)"));
        // Not the CADC password: Harbor issues a separate CLI secret, and
        // typing the account password here fails with a plain "unauthorized".
        with_example(
            &secret_row,
            crate::tr_en!("The CLI secret from your Harbor user profile — not your CADC password"),
        );
        secret_row.set_show_apply_button(true);

        // A suffix label's minimum width is its text, and this one's text is a
        // sentence — see `ui::fit`.
        let secret_status = fit::status_label();
        let remove_btn = gtk::Button::with_label(crate::tr_en!("Remove"));
        remove_btn.add_css_class("flat");
        remove_btn.set_valign(gtk::Align::Center);
        secret_row.add_suffix(&secret_status);
        secret_row.add_suffix(&remove_btn);

        // Reflect the keychain state into the status label + Remove button.
        let refresh_status = {
            let service = service.clone();
            let secret_status = secret_status.clone();
            let remove_btn = remove_btn.clone();
            move || {
                let has = service.borrow().has_secret();
                fit::set_status(
                    &secret_status,
                    if has {
                        crate::tr_en!("secret stored")
                    } else {
                        crate::tr_en!("no secret")
                    },
                );
                remove_btn.set_visible(has);
            }
        };
        refresh_status();

        // Persist the typed secret when the apply (✓) button is pressed, then
        // clear the field so the raw value never lingers in the widget.
        {
            let service = service.clone();
            let services = self.services.clone();
            let refresh_status = refresh_status.clone();
            secret_row.connect_apply(move |r| {
                let text = r.text().to_string();
                let res = service.borrow().set_secret(&text);
                match res {
                    Ok(()) => {
                        r.set_text("");
                        refresh_status();
                        services.toast.toast(crate::tr_en!("Registry secret saved"));
                    }
                    Err(e) => services.toast.toast(e),
                }
            });
        }

        // Remove the stored secret.
        {
            let service = service.clone();
            let services = self.services.clone();
            let refresh_status = refresh_status.clone();
            let secret_row = secret_row.clone();
            remove_btn.connect_clicked(move |_| {
                service.borrow().clear_secret();
                secret_row.set_text("");
                refresh_status();
                services
                    .toast
                    .toast(crate::tr_en!("Registry secret removed"));
            });
        }
        group.add(&secret_row);

        // Test credentials + an inline result row (hidden until first run).
        let test_row = adw::ActionRow::new();
        test_row.set_title(crate::tr_en!("Test credentials"));
        test_row.set_subtitle(crate::tr_en!(
            "Verify the registry secret before launching a probe job"
        ));
        let test_btn = gtk::Button::with_label(crate::tr_en!("Test"));
        test_btn.add_css_class("suggested-action");
        test_btn.set_valign(gtk::Align::Center);
        test_row.add_suffix(&test_btn);
        test_row.set_activatable_widget(Some(&test_btn));
        group.add(&test_row);

        let result_row = adw::ActionRow::new();
        result_row.set_visible(false);
        let result_icon = gtk::Image::new();
        result_row.add_prefix(&result_icon);
        group.add(&result_row);

        {
            let service = service.clone();
            let services = self.services.clone();
            let host_row = host_row.clone();
            let username_row = username_row.clone();
            let secret_row = secret_row.clone();
            let refresh_status = refresh_status.clone();
            let result_row = result_row.clone();
            let result_icon = result_icon.clone();
            test_btn.connect_clicked(move |btn| {
                // Flush host/username edits so the test uses what's on screen.
                {
                    let mut svc = service.borrow_mut();
                    svc.set_registry_host(&host_row.text());
                    svc.set_username(&username_row.text());
                }

                // If the user typed a secret, persist it and use it for the
                // test; otherwise a stored secret must be re-entered to verify.
                let typed = secret_row.text().to_string();
                if !typed.trim().is_empty() {
                    let res = service.borrow().set_secret(&typed);
                    if let Err(e) = res {
                        services.toast.toast(e);
                        return;
                    }
                    secret_row.set_text("");
                    refresh_status();
                } else if service.borrow().has_secret() {
                    services
                        .toast
                        .toast(crate::tr_en!("Re-enter your registry secret to test it"));
                    return;
                }

                let (host, username) = {
                    let s = service.borrow();
                    let st = s.settings();
                    (st.registry_host.clone(), st.username.clone())
                };
                let secret = typed.trim().to_string();

                btn.set_sensitive(false);
                btn.set_label(crate::tr_en!("Testing…"));
                let btn = btn.clone();
                let services = services.clone();
                let result_row = result_row.clone();
                let result_icon = result_icon.clone();
                glib::spawn_future_local(async move {
                    // reqwest needs the tokio runtime — bridge via AppServices::spawn.
                    let result =
                        services
                            .spawn(async move {
                                test_registry_credentials(&host, &username, &secret).await
                            })
                            .await;

                    let (icon, css, title, message): (&str, &str, &str, String) = match &result {
                        CredTestResult::Success => (
                            "emblem-ok-symbolic",
                            "success",
                            crate::tr_en!("Credentials valid"),
                            crate::tr_en!("The registry accepted the credentials.").to_string(),
                        ),
                        CredTestResult::Unauthorized => (
                            "dialog-warning-symbolic",
                            "warning",
                            crate::tr_en!("Credentials rejected"),
                            crate::tr_en!("The registry rejected the username or secret.")
                                .to_string(),
                        ),
                        CredTestResult::MissingConfiguration => (
                            "dialog-warning-symbolic",
                            "warning",
                            crate::tr_en!("Configuration incomplete"),
                            crate::tr_en!("Set a registry host, username, and secret first.")
                                .to_string(),
                        ),
                        CredTestResult::InvalidChallenge => (
                            "dialog-error-symbolic",
                            "error",
                            crate::tr_en!("Unexpected registry response"),
                            crate::tr_en!("The registry's auth challenge could not be parsed.")
                                .to_string(),
                        ),
                        CredTestResult::NetworkError(msg) => (
                            "dialog-error-symbolic",
                            "error",
                            crate::tr_en!("Network error"),
                            msg.clone(),
                        ),
                    };

                    result_icon.set_icon_name(Some(icon));
                    for c in ["success", "warning", "error"] {
                        result_icon.remove_css_class(c);
                    }
                    result_icon.add_css_class(css);
                    result_row.set_title(title);
                    result_row.set_use_markup(false);
                    result_row.set_subtitle(&message);
                    result_row.set_subtitle_lines(0);
                    result_row.set_visible(true);
                    services.toast.toast(format!("{title} — {message}"));

                    btn.set_sensitive(true);
                    btn.set_label(crate::tr_en!("Test"));
                });
            });
        }

        self.widget.add(&group);
    }

    /// "AI compute" section — configures the remote compute the agent `run_code`
    /// tool uses: the compute container image (an EMPTY image DISABLES run_code),
    /// the instance size (cores / RAM GB), and the registry credentials used to
    /// pull a private compute image. Persists through an [`AIComputeService`]
    /// (non-secret knobs as JSON, the secret in the OS keychain under a service
    /// name DISTINCT from Image Discovery) with a "Test credentials" registry
    /// probe and a "Reset to defaults" affordance. Live-applies every edit,
    /// matching the other settings groups (there is no explicit Save step).
    ///
    /// Mirrors `Views/Dialogs/AIComputeSettingsPanel.xaml(.cs)`.
    fn build_ai_compute_group(&self) {
        // The service is its own store (data_dir JSON + keychain), independent of
        // AppConfig. Shared across the row closures behind Rc<RefCell<…>> because
        // the setters take &mut self.
        let service = Rc::new(RefCell::new(AIComputeService::new()));

        // Snapshot the persisted values to seed the rows.
        let (image0, cores0, ram0, host0, repo0, user0) = {
            let s = service.borrow();
            let st = s.settings();
            (
                st.image.clone(),
                st.cores,
                st.ram,
                st.registry_host.clone(),
                st.registry_repository.clone(),
                st.registry_username.clone(),
            )
        };

        // ── Compute image + instance size ─────────────────────────────────
        let group = adw::PreferencesGroup::new();
        group.set_title(crate::tr_en!("AI compute")); // Settings_ComputeHeader
        group.set_description(Some(crate::tr_en!(
            "Configure the remote compute the agent's run_code tool uses. Leave the image blank to disable run_code. run_code runs agent-authored code on your CADC account: with auto-apply on it runs immediately; with it off each call queues for your approval."
        )));

        // Compute image. Blank disables run_code / start_compute (setter trims).
        let image_row = adw::EntryRow::new();
        image_row.set_title(crate::tr_en!("Compute image"));
        with_example(
            &image_row,
            crate::tr_en!("e.g. verbinal-compute:1.0 or project/name:tag"),
        );
        image_row.set_text(&image0);

        // What run_code will actually launch, and whether it can.
        //
        // Named apart from the other groups' `refresh_status` on purpose: this
        // one answers a different question and they sit in the same file.
        //
        // Every other row here reports on itself — the credentials even say
        // "Credentials valid" — while the single thing that decides whether
        // run_code works stayed invisible until it was called.
        let compute_status_row = adw::ActionRow::new();
        compute_status_row.add_css_class("property");
        let refresh_compute_status = {
            let service = service.clone();
            let row = compute_status_row.clone();
            Rc::new(move || {
                let (ready, resolved) = {
                    let s = service.borrow();
                    let st = s.settings();
                    (st.is_enabled(), st.resolve_image())
                };
                if ready {
                    row.set_title(crate::tr_en!("run_code is ready"));
                    row.set_use_markup(false);
                    row.set_subtitle(&resolved);
                } else {
                    row.set_title(crate::tr_en!("run_code is off"));
                    row.set_subtitle(crate::tr_en!(
                        "Set a compute image above — a name like verbinal-compute:1.0, or a \
                         full project/name:tag reference."
                    ));
                }
            })
        };
        refresh_compute_status();
        {
            let service = service.clone();
            let refresh_compute_status = refresh_compute_status.clone();
            image_row.connect_changed(move |r| {
                service.borrow_mut().set_image(&r.text());
                refresh_compute_status();
            });
        }
        group.add(&image_row);
        group.add(&compute_status_row);

        // Cores (1–64; the setter clamps).
        let cores_row = adw::SpinRow::new(
            Some(&gtk::Adjustment::new(
                cores0 as f64,
                1.0,
                64.0,
                1.0,
                1.0,
                0.0,
            )),
            1.0,
            0,
        );
        cores_row.set_title(crate::tr_en!("Cores"));
        {
            let service = service.clone();
            cores_row.connect_value_notify(move |r| {
                service.borrow_mut().set_cores(r.value() as u32);
            });
        }
        group.add(&cores_row);

        // RAM in GB (1–256; the setter clamps).
        let ram_row = adw::SpinRow::new(
            Some(&gtk::Adjustment::new(
                ram0 as f64,
                1.0,
                256.0,
                1.0,
                4.0,
                0.0,
            )),
            1.0,
            0,
        );
        ram_row.set_title(crate::tr_en!("RAM (GB)"));
        {
            let service = service.clone();
            ram_row.connect_value_notify(move |r| {
                service.borrow_mut().set_ram(r.value() as u32);
            });
        }
        group.add(&ram_row);

        self.widget.add(&group);

        // ── Registry credentials ──────────────────────────────────────────
        let reg_group = adw::PreferencesGroup::new();
        reg_group.set_title(crate::tr_en!("Registry credentials"));
        reg_group.set_description(Some(crate::tr_en!(
            "Used to pull a private compute image. Stored separately from the Image Discovery tab's credentials."
        )));

        // Registry host — blank restores the default in the setter.
        let host_row = adw::EntryRow::new();
        host_row.set_title(crate::tr_en!("Registry host"));
        with_example(&host_row, crate::tr_en!("e.g. images.canfar.net"));
        host_row.set_text(&host0);
        {
            let service = service.clone();
            host_row.connect_changed(move |r| {
                service.borrow_mut().set_registry_host(&r.text());
            });
        }
        reg_group.add(&host_row);

        // Registry repository/project — prefixes a short compute image name.
        let repo_row = adw::EntryRow::new();
        repo_row.set_title(crate::tr_en!("Registry repository (project)"));
        with_example(
            &repo_row,
            crate::tr_en!("e.g. private-test — the project only, no image name"),
        );
        repo_row.set_text(&repo0);
        {
            let service = service.clone();
            repo_row.connect_changed(move |r| {
                service.borrow_mut().set_registry_repository(&r.text());
            });
        }
        reg_group.add(&repo_row);

        // Registry username.
        let username_row = adw::EntryRow::new();
        username_row.set_title(crate::tr_en!("Registry username"));
        with_example(&username_row, crate::tr_en!("Your CADC username"));
        username_row.set_text(&user0);
        {
            let service = service.clone();
            username_row.connect_changed(move |r| {
                service.borrow_mut().set_username(&r.text());
            });
        }
        reg_group.add(&username_row);

        // Registry secret — never pre-filled; stored in the OS keychain. A suffix
        // label reports whether one is on file, plus a Remove button.
        let secret_row = adw::PasswordEntryRow::new();
        secret_row.set_title(crate::tr_en!("Registry secret (Harbor CLI secret)"));
        // Not the CADC password: Harbor issues a separate CLI secret, and
        // typing the account password here fails with a plain "unauthorized".
        with_example(
            &secret_row,
            crate::tr_en!("The CLI secret from your Harbor user profile — not your CADC password"),
        );
        secret_row.set_show_apply_button(true);

        // A suffix label's minimum width is its text, and this one's text is a
        // sentence — see `ui::fit`.
        let secret_status = fit::status_label();
        let remove_btn = gtk::Button::with_label(crate::tr_en!("Remove secret"));
        remove_btn.add_css_class("flat");
        remove_btn.set_valign(gtk::Align::Center);
        secret_row.add_suffix(&secret_status);
        secret_row.add_suffix(&remove_btn);

        // Reflect the keychain state into the status label + Remove button.
        let refresh_status = {
            let service = service.clone();
            let secret_status = secret_status.clone();
            let remove_btn = remove_btn.clone();
            move || {
                let has = service.borrow().settings().has_secret;
                fit::set_status(
                    &secret_status,
                    if has {
                        crate::tr_en!(
                        "A secret is stored. Type a new one to replace it, or leave blank to keep it."
                    )
                    } else {
                        crate::tr_en!("No secret stored.")
                    },
                );
                remove_btn.set_visible(has);
            }
        };
        refresh_status();

        // Persist the typed secret when the apply (✓) button is pressed, then
        // clear the field so the raw value never lingers in the widget.
        {
            let service = service.clone();
            let services = self.services.clone();
            let refresh_status = refresh_status.clone();
            secret_row.connect_apply(move |r| {
                let text = r.text().to_string();
                let res = service.borrow_mut().set_secret(&text);
                match res {
                    Ok(()) => {
                        r.set_text("");
                        refresh_status();
                        services
                            .toast
                            .toast(crate::tr_en!("AI compute settings were saved."));
                    }
                    Err(e) => services.toast.toast(e),
                }
            });
        }

        // Remove the stored secret.
        {
            let service = service.clone();
            let services = self.services.clone();
            let refresh_status = refresh_status.clone();
            let secret_row = secret_row.clone();
            remove_btn.connect_clicked(move |_| {
                service.borrow_mut().clear_secret();
                secret_row.set_text("");
                refresh_status();
                services
                    .toast
                    .toast(crate::tr_en!("The stored registry secret was deleted."));
            });
        }
        reg_group.add(&secret_row);

        // Test credentials + an inline result row (hidden until first run).
        let test_row = adw::ActionRow::new();
        test_row.set_title(crate::tr_en!("Test credentials"));
        test_row.set_subtitle(crate::tr_en!(
            "Verify the registry secret before launching a compute job"
        ));
        let test_btn = gtk::Button::with_label(crate::tr_en!("Test credentials"));
        test_btn.add_css_class("suggested-action");
        test_btn.set_valign(gtk::Align::Center);
        test_row.add_suffix(&test_btn);
        test_row.set_activatable_widget(Some(&test_btn));
        reg_group.add(&test_row);

        let result_row = adw::ActionRow::new();
        result_row.set_visible(false);
        let result_icon = gtk::Image::new();
        result_row.add_prefix(&result_icon);
        reg_group.add(&result_row);

        {
            let service = service.clone();
            let services = self.services.clone();
            let host_row = host_row.clone();
            let username_row = username_row.clone();
            let secret_row = secret_row.clone();
            let refresh_status = refresh_status.clone();
            let result_row = result_row.clone();
            let result_icon = result_icon.clone();
            test_btn.connect_clicked(move |btn| {
                // Flush host/username edits so the test uses what's on screen.
                {
                    let mut svc = service.borrow_mut();
                    svc.set_registry_host(&host_row.text());
                    svc.set_username(&username_row.text());
                }

                // If the user typed a secret, persist it so the test uses it.
                let typed = secret_row.text().to_string();
                if !typed.trim().is_empty() {
                    let res = service.borrow_mut().set_secret(&typed);
                    if let Err(e) = res {
                        services.toast.toast(e);
                        return;
                    }
                    secret_row.set_text("");
                    refresh_status();
                }

                // Read the host + (username, secret) the service holds — the AI
                // compute keychain lets the stored secret be read back to test it.
                let (host, username, secret) = {
                    let s = service.borrow();
                    let (u, sec) = s.registry_credentials();
                    (s.settings().registry_host.clone(), u, sec)
                };

                btn.set_sensitive(false);
                btn.set_label(crate::tr_en!("Testing…"));
                let btn = btn.clone();
                let services = services.clone();
                let result_row = result_row.clone();
                let result_icon = result_icon.clone();
                glib::spawn_future_local(async move {
                    // reqwest needs the tokio runtime — bridge via AppServices::spawn.
                    let result =
                        services
                            .spawn(async move {
                                test_registry_credentials(&host, &username, &secret).await
                            })
                            .await;

                    let (icon, css, title, message): (&str, &str, &str, String) = match &result {
                        CredTestResult::Success => (
                            "emblem-ok-symbolic",
                            "success",
                            crate::tr_en!("Credentials valid"),
                            crate::tr_en!("The registry accepted the credentials.").to_string(),
                        ),
                        CredTestResult::Unauthorized => (
                            "dialog-warning-symbolic",
                            "warning",
                            crate::tr_en!("Credentials rejected"),
                            crate::tr_en!("The registry rejected the username or secret.")
                                .to_string(),
                        ),
                        CredTestResult::MissingConfiguration => (
                            "dialog-warning-symbolic",
                            "warning",
                            crate::tr_en!("Configuration incomplete"),
                            crate::tr_en!("Set a registry host, username, and secret first.")
                                .to_string(),
                        ),
                        CredTestResult::InvalidChallenge => (
                            "dialog-error-symbolic",
                            "error",
                            crate::tr_en!("Unexpected registry response"),
                            crate::tr_en!("The registry's auth challenge could not be parsed.")
                                .to_string(),
                        ),
                        CredTestResult::NetworkError(msg) => (
                            "dialog-error-symbolic",
                            "error",
                            crate::tr_en!("Network error"),
                            msg.clone(),
                        ),
                    };

                    result_icon.set_icon_name(Some(icon));
                    for c in ["success", "warning", "error"] {
                        result_icon.remove_css_class(c);
                    }
                    result_icon.add_css_class(css);
                    result_row.set_title(title);
                    result_row.set_use_markup(false);
                    result_row.set_subtitle(&message);
                    result_row.set_subtitle_lines(0);
                    result_row.set_visible(true);
                    services.toast.toast(format!("{title} — {message}"));

                    btn.set_sensitive(true);
                    btn.set_label(crate::tr_en!("Test credentials"));
                });
            });
        }

        // Reset to defaults — clears the secret and restores every field. Mirrors
        // the Windows confirm body as the row subtitle (there is no modal step).
        let reset_row = adw::ActionRow::new();
        reset_row.set_title(crate::tr_en!("Reset to defaults")); // Compute_ResetButton
        reset_row.set_subtitle(crate::tr_en!(
            "Reset the compute image, cores/RAM, registry host/repository/username, and the stored secret to defaults? This disables run_code until you set an image again."
        ));
        reset_row.set_subtitle_lines(0);
        let reset_btn = gtk::Button::with_label(crate::tr_en!("Reset to defaults"));
        reset_btn.add_css_class("destructive-action");
        reset_btn.set_valign(gtk::Align::Center);
        reset_row.add_suffix(&reset_btn);
        reset_row.set_activatable_widget(Some(&reset_btn));
        reg_group.add(&reset_row);

        {
            let service = service.clone();
            let services = self.services.clone();
            let image_row = image_row.clone();
            let cores_row = cores_row.clone();
            let ram_row = ram_row.clone();
            let host_row = host_row.clone();
            let repo_row = repo_row.clone();
            let username_row = username_row.clone();
            let secret_row = secret_row.clone();
            let refresh_status = refresh_status.clone();
            let result_row = result_row.clone();
            reset_btn.connect_clicked(move |_| {
                // Reset (clears the secret + restores defaults), then snapshot the
                // fresh values WITHOUT holding the borrow across the widget setters
                // (set_text/set_value re-fire the change handlers, which borrow_mut).
                let (image, cores, ram, host, repo, user) = {
                    let mut svc = service.borrow_mut();
                    svc.reset_to_defaults();
                    let st = svc.settings();
                    (
                        st.image.clone(),
                        st.cores,
                        st.ram,
                        st.registry_host.clone(),
                        st.registry_repository.clone(),
                        st.registry_username.clone(),
                    )
                };
                image_row.set_text(&image);
                cores_row.set_value(cores as f64);
                ram_row.set_value(ram as f64);
                host_row.set_text(&host);
                repo_row.set_text(&repo);
                username_row.set_text(&user);
                secret_row.set_text("");
                refresh_status();
                result_row.set_visible(false);
                services.toast.toast(crate::tr_en!(
                    "AI compute settings were reset to defaults (run_code is now disabled)."
                ));
            });
        }

        self.widget.add(&reg_group);
    }

    fn build_about_group(&self) {
        let group = adw::PreferencesGroup::new();
        group.set_title(crate::tr_en!("About"));

        let version_row = adw::ActionRow::builder()
            .title(crate::tr_en!("Version"))
            .subtitle(env!("CARGO_PKG_VERSION"))
            .build();
        version_row.add_prefix(&gtk::Image::from_icon_name("dialog-information-symbolic"));
        group.add(&version_row);

        let platform_row = adw::ActionRow::builder()
            .title(crate::tr_en!("Platform"))
            .subtitle(format!(
                "{} / GTK4 + libadwaita / Rust",
                std::env::consts::OS
            ))
            .build();
        platform_row.add_prefix(&gtk::Image::from_icon_name("computer-symbolic"));
        group.add(&platform_row);

        self.widget.add(&group);
    }
}

pub fn apply_theme(theme: &str) {
    let manager = adw::StyleManager::default();
    let scheme = match theme {
        "Light" => adw::ColorScheme::ForceLight,
        "Dark" => adw::ColorScheme::ForceDark,
        _ => adw::ColorScheme::PreferLight,
    };
    manager.set_color_scheme(scheme);
}

/// Run the MCP diagnostics battery and show the results in a modal window (one
/// row per check with a pass/fail icon, detail, and a fix hint on failure).
fn show_mcp_diagnostics(
    parent: &impl IsA<gtk::Widget>,
    services: &std::sync::Arc<crate::state::AppServices>,
) {
    let rows = crate::mcp::diagnostics::run_diagnostics(services);

    let dialog = adw::Window::builder()
        .title(crate::tr_en!("MCP Diagnostics"))
        .default_width(crate::ui::fit::FORM)
        .default_height(440)
        .modal(true)
        .build();
    if let Some(root) = parent.root().and_downcast::<gtk::Window>() {
        dialog.set_transient_for(Some(&root));
    }

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_vexpand(true);
    scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);

    let group = adw::PreferencesGroup::new();
    group.set_margin_start(12);
    group.set_margin_end(12);
    group.set_margin_top(12);
    group.set_margin_bottom(12);
    for r in &rows {
        let row = adw::ActionRow::new();
        row.set_use_markup(false);
        row.set_title(&r.label);
        let mut subtitle = r.detail.clone();
        if !r.ok {
            if let Some(fix) = &r.fix_hint {
                subtitle = format!("{subtitle}\n→ {fix}");
            }
        }
        row.set_subtitle(&subtitle);
        row.set_subtitle_lines(0);
        let icon = if r.ok {
            "emblem-ok-symbolic"
        } else {
            "dialog-warning-symbolic"
        };
        let img = gtk::Image::from_icon_name(icon);
        img.add_css_class(if r.ok { "success" } else { "warning" });
        row.add_prefix(&img);
        group.add(&row);
    }
    scroll.set_child(Some(&group));
    toolbar.set_content(Some(&scroll));
    dialog.set_content(Some(&toolbar));
    dialog.present();
}

#[cfg(test)]
mod settings_hint_tests {
    /// Every text field in Settings shows an example of what belongs in it.
    ///
    /// This is the guard for the configuration that read as complete and was
    /// not: `registry_repository` held `private-test/verbinal-execution:0.0.1`
    /// because the row said "Registry repository (project)" and nothing said
    /// what a project looks like. A title names a field; only an example
    /// settles its shape.
    ///
    /// A source scan rather than a widget test because the answer has to hold
    /// for the field somebody adds next year, and that field will be added
    /// here — one `EntryRow::new()` at a time.
    ///
    /// Matched by position, not by variable name. The image-discovery group and
    /// the AI-compute group each have a `host_row`, a `repo_row` and a
    /// `username_row`; searching the file for the name lets either group's hint
    /// answer for the other, and the first version of this test passed with a
    /// hint deleted.
    #[test]
    fn every_entry_row_in_settings_shows_an_example() {
        const WINDOW: usize = 12; // construction, title, apply button, hint

        let source = include_str!("settings_page.rs");
        let code = crate::testing::without_comments(crate::testing::code(source));
        let lines: Vec<&str> = code.lines().collect();

        let mut hintless = Vec::new();
        let mut rows = 0usize;
        for (i, line) in lines.iter().enumerate() {
            let built = line.contains("= adw::EntryRow::new()")
                || line.contains("= adw::PasswordEntryRow::new()");
            if !built {
                continue;
            }
            rows += 1;
            let end = (i + WINDOW).min(lines.len());
            if !lines[i..end].iter().any(|l| l.contains("with_example(")) {
                hintless.push(format!("line {}: {}", i + 1, line.trim()));
            }
        }

        assert!(rows >= 10, "only {rows} entry rows found — scan broken");
        assert!(
            hintless.is_empty(),
            "Settings field(s) with a title but no example of what goes in \
             them: {hintless:#?}"
        );
    }
}
