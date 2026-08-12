use crate::models::UserInfo;
use crate::services::TokenStorage;
use crate::state::AppServices;
use crate::ui::cube_tab_host::CubeTabHost;
use crate::ui::dashboard::DashboardView;
use crate::ui::file_panel::{FilePanel, FileType};
use crate::ui::fits_viewer::FitsViewer;
use crate::ui::login_dialog::show_login_dialog;
use crate::ui::notebook_host::NotebookTabHost;
use crate::ui::research_page::ResearchPage;
use crate::ui::search_page::SearchPage;
use crate::ui::settings_page::{self, SettingsPage};
use crate::ui::vospace_browser::VoSpaceBrowser;
use crate::ui::workflows_page::WorkflowsPage;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

pub fn build_main_window(
    app: &adw::Application,
    services: Arc<AppServices>,
    toast_rx: tokio::sync::mpsc::UnboundedReceiver<
        crate::services::notification_service::ToastMessage,
    >,
) {
    // Apply saved theme on startup
    let config = services.settings.load();
    settings_page::apply_theme(&config.theme);

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title(crate::tr_en!("Verbinal - a CANFAR Science Portal"))
        .default_width(1200)
        .default_height(800)
        // Explicit minimum size — required when the window carries breakpoints
        // (the split view collapses to a single pane below the 720sp breakpoint).
        .width_request(480)
        .height_request(360)
        .build();

    // ── Shell headers ────────────────────────────────────────────────────────
    // GNOME HIG shell: a sidebar header carrying the primary (hamburger) menu
    // and a content header carrying only contextual controls. Navigation lives
    // in a sidebar list (NavigationSplitView); app-level entries (Preferences,
    // Help, About) live in the primary menu.
    let sidebar_header = adw::HeaderBar::new();

    let primary_menu = gtk::gio::Menu::new();
    let prefs_section = gtk::gio::Menu::new();
    prefs_section.append(Some(crate::tr_en!("Preferences")), Some("app.preferences"));
    primary_menu.append_section(None, &prefs_section);
    let help_section = gtk::gio::Menu::new();
    help_section.append(Some(crate::tr_en!("Help")), Some("app.help"));
    help_section.append(Some(crate::tr_en!("About Verbinal")), Some("app.about"));
    primary_menu.append_section(None, &help_section);

    let menu_btn = gtk::MenuButton::new();
    menu_btn.set_icon_name("open-menu-symbolic");
    menu_btn.set_primary(true);
    menu_btn.set_tooltip_text(Some(crate::tr_en!("Main Menu")));
    menu_btn.set_menu_model(Some(&primary_menu));
    sidebar_header.pack_end(&menu_btn);

    let content_header = adw::HeaderBar::new();

    // Files panel toggle — contextual, stays in the content header.
    let files_btn = gtk::ToggleButton::new();
    files_btn.set_icon_name("folder-symbolic");
    files_btn.set_tooltip_text(Some(crate::tr_en!("Toggle File Panel (Ctrl+B)")));
    files_btn.add_css_class("flat");
    content_header.pack_start(&files_btn);

    // Auth status caption — lives in the sidebar footer (assembled below).
    let status_label = gtk::Label::new(None);
    status_label.add_css_class("dim-label");
    status_label.add_css_class("caption");
    status_label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    status_label.set_xalign(0.0);

    // --- Transient agent-activity indicator ---
    // Flashes "⚡ agent working…" with a spinner while an MCP agent has invoked
    // a tool in the last few seconds; hidden when the agent is idle. Polled once
    // a second against the global agent-activity log (the MCP router records
    // each dispatch there — see crate::helpers::agent_activity).
    let agent_spinner = gtk::Spinner::new();
    let agent_label = gtk::Label::new(Some(crate::tr_en!("⚡ agent working…")));
    agent_label.add_css_class("caption");
    agent_label.add_css_class("accent");
    let agent_box = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    agent_box.append(&agent_spinner);
    agent_box.append(&agent_label);
    agent_box.set_visible(false);
    agent_box.set_tooltip_text(Some(crate::tr_en!("An AI agent is working")));
    {
        let agent_box = agent_box.clone();
        let agent_spinner = agent_spinner.clone();
        glib::timeout_add_local(std::time::Duration::from_secs(1), move || {
            let active = crate::helpers::agent_activity::is_active_within(5);
            if active != agent_box.is_visible() {
                agent_box.set_visible(active);
                if active {
                    agent_spinner.start();
                } else {
                    agent_spinner.stop();
                }
            }
            glib::ControlFlow::Continue
        });
    }

    // --- Service health indicator ---
    let health_icon = gtk::Image::from_icon_name("network-idle-symbolic");
    health_icon.set_pixel_size(16);
    let health_label = gtk::Label::new(Some(crate::tr_en!("Connected")));
    health_label.add_css_class("caption");
    let health_box = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    health_box.append(&health_icon);
    health_box.append(&health_label);

    // Popover with per-service status rows
    let health_list = gtk::ListBox::new();
    health_list.set_selection_mode(gtk::SelectionMode::None);
    health_list.add_css_class("boxed-list");

    let health_popover_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    health_popover_box.set_margin_start(8);
    health_popover_box.set_margin_end(8);
    health_popover_box.set_margin_top(8);
    health_popover_box.set_margin_bottom(8);
    health_popover_box.append(&health_list);

    let health_popover = gtk::Popover::new();
    health_popover.set_child(Some(&health_popover_box));

    // Service status button — lives in the sidebar footer (assembled below).
    let health_btn = gtk::MenuButton::new();
    health_btn.set_child(Some(&health_box));
    health_btn.set_popover(Some(&health_popover));
    health_btn.add_css_class("flat");
    health_btn.set_tooltip_text(Some(crate::tr_en!("Service status")));

    // --- Content pages ---
    let view_stack = adw::ViewStack::new();

    // --- Content header trailing controls: spinner + proposals badge ---
    let spinner = gtk::Spinner::new();
    spinner.set_visible(false);
    content_header.pack_end(&spinner);

    // Agent-proposals button — shows a pending-count badge when an AI agent has
    // queued destructive writes awaiting review; hidden when there are none.
    let proposals_btn = gtk::Button::new();
    proposals_btn.set_tooltip_text(Some(crate::tr_en!("Agent proposals awaiting review")));
    proposals_btn.add_css_class("suggested-action");
    proposals_btn.set_visible(false);
    content_header.pack_end(&proposals_btn);
    {
        let services = services.clone();
        let window = window.clone();
        proposals_btn.connect_clicked(move |_| {
            crate::ui::agent_proposals_dialog::show_agent_proposals(&window, services.clone());
        });
    }
    // Poll the pending count every 2s to update the badge / visibility.
    {
        let services = services.clone();
        let btn = proposals_btn.clone();
        glib::timeout_add_local(std::time::Duration::from_secs(2), move || {
            let n = crate::ui::agent_proposals_dialog::pending_count(&services);
            if n > 0 {
                btn.set_label(&format!("⚡ {n}"));
                btn.set_visible(true);
            } else {
                btn.set_visible(false);
            }
            glib::ControlFlow::Continue
        });
    }

    // Account controls — live in the sidebar footer (assembled below).
    let login_btn = gtk::Button::with_label(crate::tr_en!("Login"));
    login_btn.add_css_class("suggested-action");
    login_btn.add_css_class("pill");

    let user_menu_btn = gtk::MenuButton::new();
    user_menu_btn.set_visible(false);
    user_menu_btn.set_tooltip_text(Some(crate::tr_en!("Account")));
    let user_menu = gtk::gio::Menu::new();
    user_menu.append(Some(crate::tr_en!("Profile")), Some("app.profile"));
    user_menu.append(Some(crate::tr_en!("Logout")), Some("app.logout"));
    user_menu_btn.set_menu_model(Some(&user_menu));

    // --- File panel (hidden by default) ---
    let file_panel = FilePanel::new();
    file_panel.widget().set_visible(false);

    // --- Toast Overlay wrapping the ViewStack ---
    let toast_overlay = adw::ToastOverlay::new();
    toast_overlay.set_child(Some(&view_stack));

    // Wire cross-thread toast dispatch: any thread can call services.toast.toast("...")
    {
        let overlay = toast_overlay.clone();
        let app_for_toast = app.clone();
        let mut rx = toast_rx;
        glib::spawn_future_local(async move {
            while let Some(msg) = rx.recv().await {
                let toast = adw::Toast::new(&msg.body);
                toast.set_timeout(msg.timeout);

                // Attach an action button if the message has one.  Clicking it
                // activates the named app action (e.g. "navigate-research").
                if let Some(action) = msg.action {
                    toast.set_button_label(Some(&action.label));
                    let app_ref = app_for_toast.clone();
                    let action_name = action.action_name.clone();
                    toast.connect_button_clicked(move |_| {
                        use gtk4::prelude::ActionGroupExt;
                        // Strip "app." prefix if present; gio::ActionGroup
                        // activate_action expects the bare action name.
                        let bare = action_name.strip_prefix("app.").unwrap_or(&action_name);
                        app_ref.activate_action(bare, None);
                    });
                }

                overlay.add_toast(toast);
            }
        });
    }

    // --- Paned: file panel (left) + toast overlay (right) ---
    let paned = gtk::Paned::new(gtk::Orientation::Horizontal);
    paned.set_start_child(Some(file_panel.widget()));
    paned.set_end_child(Some(&toast_overlay));
    paned.set_position(280);
    // Allow squishing end panel but keep file panel at its preferred width
    paned.set_shrink_start_child(false);
    paned.set_shrink_end_child(true);
    paned.set_resize_start_child(false);
    paned.set_resize_end_child(true);

    // --- Degraded mode banner (hidden by default) ---
    let banner = adw::Banner::new(crate::tr_en!(
        "Some services unreachable — working with cached data"
    ));
    banner.set_button_label(Some(crate::tr_en!("Details")));
    banner.set_revealed(false);

    // --- Session-expired banner (hidden by default) ---
    let session_banner = adw::Banner::new(crate::tr_en!(
        "Your session has expired — please sign in again"
    ));
    session_banner.set_button_label(Some(crate::tr_en!("Sign In")));
    session_banner.set_revealed(false);

    // --- Offline hint banner (hidden by default) ---
    // Revealed by the network monitor when internet connectivity is lost, so the
    // user understands why remote actions are failing. Mirrors the Windows
    // MainWindow_OfflineHint status text (NetworkMonitor.StatusChanged).
    let offline_banner = adw::Banner::new(crate::tr_en!(
        "You appear to be offline — some features are unavailable"
    ));
    offline_banner.set_revealed(false);

    // ── Content pane: header + banners above the paned content, hosted in a
    // NavigationView so contextual pages (observation detail) push/pop with an
    // automatic back button.
    let content_root_tv = adw::ToolbarView::new();
    content_root_tv.add_top_bar(&content_header);
    content_root_tv.add_top_bar(&banner);
    content_root_tv.add_top_bar(&session_banner);
    content_root_tv.add_top_bar(&offline_banner);
    content_root_tv.set_content(Some(&paned));

    let content_root_page = adw::NavigationPage::new(&content_root_tv, crate::tr_en!("Home"));
    let content_nav = adw::NavigationView::new();
    content_nav.add(&content_root_page);
    let content_page = adw::NavigationPage::new(&content_nav, "Verbinal");

    // ── Sidebar: navigation list + status/account footer ────────────────────
    let sidebar_list = gtk::ListBox::new();
    sidebar_list.add_css_class("navigation-sidebar");
    sidebar_list.set_selection_mode(gtk::SelectionMode::Single);

    let nav_items: Vec<(&str, &str, &str)> = vec![
        ("home", crate::tr_en!("Home"), "go-home-symbolic"),
        (
            "storage",
            crate::tr_en!("Storage"),
            "drive-multidisk-symbolic",
        ),
        ("search", crate::tr_en!("Search"), "system-search-symbolic"),
        (
            "research",
            crate::tr_en!("Research"),
            "document-open-recent-symbolic",
        ),
        (
            "fits",
            crate::tr_en!("FITS Viewer"),
            "image-x-generic-symbolic",
        ),
        ("cube", crate::tr_en!("Cube Viewer"), "view-paged-symbolic"),
        (
            "notebook",
            crate::tr_en!("Notebook"),
            "accessories-text-editor-symbolic",
        ),
        (
            "workflows",
            crate::tr_en!("Workflows"),
            "view-list-symbolic",
        ),
        (
            "aiguide",
            crate::tr_en!("AI Guide"),
            "applications-science-symbolic",
        ),
    ];
    let nav_keys: Rc<Vec<&'static str>> = Rc::new(nav_items.iter().map(|(k, _, _)| *k).collect());
    for (key, title, icon) in &nav_items {
        let row = adw::ActionRow::new();
        row.set_title(title);
        row.set_activatable(true);
        row.add_prefix(&gtk::Image::from_icon_name(icon));
        if *key == "aiguide" {
            row.set_visible(read_show_ai_guide_tile());
        }
        sidebar_list.append(&row);
    }

    let sidebar_scroll = gtk::ScrolledWindow::new();
    sidebar_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    sidebar_scroll.set_vexpand(true);
    sidebar_scroll.set_child(Some(&sidebar_list));

    // Footer: agent activity, service status, account.
    let sidebar_footer = gtk::Box::new(gtk::Orientation::Vertical, 6);
    sidebar_footer.set_margin_start(12);
    sidebar_footer.set_margin_end(12);
    sidebar_footer.set_margin_top(6);
    sidebar_footer.set_margin_bottom(12);
    agent_box.set_halign(gtk::Align::Start);
    sidebar_footer.append(&agent_box);
    sidebar_footer.append(&health_btn);
    sidebar_footer.append(&login_btn);
    sidebar_footer.append(&user_menu_btn);
    sidebar_footer.append(&status_label);

    let sidebar_tv = adw::ToolbarView::new();
    sidebar_tv.add_top_bar(&sidebar_header);
    sidebar_tv.set_content(Some(&sidebar_scroll));
    sidebar_tv.add_bottom_bar(&sidebar_footer);
    let sidebar_page = adw::NavigationPage::new(&sidebar_tv, "Verbinal");

    let split_view = adw::NavigationSplitView::new();
    split_view.set_sidebar(Some(&sidebar_page));
    split_view.set_content(Some(&content_page));
    split_view.set_min_sidebar_width(220.0);
    split_view.set_max_sidebar_width(280.0);

    // Collapse to a single pane on narrow windows.
    let breakpoint = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
        adw::BreakpointConditionLengthType::MaxWidth,
        720.0,
        adw::LengthUnit::Sp,
    ));
    breakpoint.add_setter(&split_view, "collapsed", Some(&true.to_value()));
    window.add_breakpoint(breakpoint);

    // Sidebar row activation drives the content ViewStack.
    {
        let view_stack = view_stack.clone();
        let split_view = split_view.clone();
        let content_nav = content_nav.clone();
        let content_root_page = content_root_page.clone();
        let nav_keys = nav_keys.clone();
        sidebar_list.connect_row_activated(move |_, row| {
            if let Some(key) = nav_keys.get(row.index() as usize) {
                // Leave any pushed contextual page (observation detail) first.
                content_nav.pop_to_page(&content_root_page);
                view_stack.set_visible_child_name(key);
                split_view.set_show_content(true);
            }
        });
    }

    let toolbar_view = adw::ToolbarView::new();
    toolbar_view.set_content(Some(&split_view));

    // Banner "Details" opens the health popover
    {
        let health_btn = health_btn.clone();
        banner.connect_button_clicked(move |_| {
            health_btn.popup();
        });
    }

    // Session-expired banner "Sign In" reuses the header Login flow.
    {
        let session_banner_c = session_banner.clone();
        let login_btn = login_btn.clone();
        session_banner.connect_button_clicked(move |_| {
            session_banner_c.set_revealed(false);
            login_btn.emit_clicked();
        });
    }

    // Recover from mid-session 401s: on the auth-expired signal, try a silent
    // re-auth (with a 60s cooldown after a failure) and, failing that, reveal the
    // sign-in banner. Bursts of 401s are debounced by the in-progress guard.
    {
        let services = services.clone();
        let session_banner = session_banner.clone();
        let mut rx = crate::services::auth_events::subscribe();
        let last_fail: Rc<RefCell<Option<std::time::Instant>>> = Rc::new(RefCell::new(None));
        let in_progress = Rc::new(std::cell::Cell::new(false));
        glib::spawn_future_local(async move {
            loop {
                match rx.recv().await {
                    Ok(()) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                }
                // Already recovering or already prompting — ignore this 401.
                if in_progress.get() || session_banner.is_revealed() {
                    continue;
                }
                // Cooldown: after a recent silent-reauth failure, go straight to
                // the banner rather than hammering the login endpoint.
                let cooled = last_fail
                    .borrow()
                    .map(|t| t.elapsed().as_secs() < 60)
                    .unwrap_or(false);
                if cooled {
                    session_banner.set_revealed(true);
                    continue;
                }
                in_progress.set(true);
                let svc = services.clone();
                let recovered = services
                    .spawn(async move { svc.try_silent_reauth().await })
                    .await;
                in_progress.set(false);
                if recovered {
                    services.toast.toast(crate::tr_en!("Session refreshed"));
                } else {
                    *last_fail.borrow_mut() = Some(std::time::Instant::now());
                    session_banner.set_revealed(true);
                }
            }
        });
    }

    // Root overlay hosts the shell plus the first-launch Terms-of-Use gate,
    // which is layered on top and blocks all interaction until accepted.
    let root_overlay = gtk::Overlay::new();
    root_overlay.set_child(Some(&toolbar_view));
    window.set_content(Some(&root_overlay));

    // Terms-of-Use gate: a blocking, non-dismissible overlay shown on first
    // launch (or after a terms-version bump). Accepting records the version and
    // frees the shell; "Decline & Exit" quits the app.
    {
        let legal = Rc::new(crate::services::legal_service::LegalAgreementService::new());
        if legal.needs_acceptance() {
            let french = matches!(crate::i18n::current_lang(), crate::i18n::Lang::Fr);
            show_terms_gate(app, &root_overlay, &toolbar_view, legal, french);
        }
    }

    // --- Network monitor: reveal the offline banner when connectivity drops ---
    // The probe runs on the tokio runtime (off the GTK main loop) every 15s; the
    // banner is toggled only on state transitions so it never flickers.
    {
        let services = services.clone();
        let offline_banner = offline_banner.clone();
        let monitor = Rc::new(crate::services::network_monitor::NetworkMonitor::new());
        glib::spawn_future_local(async move {
            loop {
                let online = services
                    .spawn(crate::services::network_monitor::NetworkMonitor::probe())
                    .await;
                if monitor.set_online(online) {
                    offline_banner.set_revealed(!online);
                }
                glib::timeout_future_seconds(15).await;
            }
        });
    }

    // --- Add pages to ViewStack ---
    // Dashboard (added later when logged in)
    // Settings (always available)
    let settings_page = SettingsPage::new(services.clone());

    // VOSpace browser
    let vospace_browser = VoSpaceBrowser::new(services.clone());

    // FITS viewer
    let fits_viewer = FitsViewer::new(services.clone());

    // Add pages — all 6 modules + settings
    // The landing page keeps its auth-gated tiles (Portal, Storage) locked while
    // signed out; `welcome` is retained so login/logout can toggle that lock.
    let welcome = Rc::new(build_welcome_page(
        &view_stack,
        &window,
        &services,
        &login_btn,
        read_show_ai_guide_tile(),
    ));
    let dashboard_placeholder = welcome.root.clone();

    // Search module (real implementation)
    let search_page = SearchPage::new(services.clone(), window.clone());

    // "Search Here" from the FITS crosshair → Search form, prefilled.
    {
        let view_stack = view_stack.clone();
        let search_page = search_page.clone();
        fits_viewer.set_on_search_here(move |ra, dec| {
            view_stack.set_visible_child_name("search");
            search_page.show_search_form(ra, dec);
        });
    }

    // Research module (real implementation)
    let research_page = ResearchPage::new(services.clone());

    // CAOM2 Observation Detail page (opened from Search / Research)
    let obs_detail =
        crate::ui::observation_detail_page::ObservationDetailPage::new(services.clone());
    research_page.set_application(app);

    // Notebook module (real implementation)
    let notebook_host = NotebookTabHost::new(services.clone());

    // Cube Viewer module (3D spectral cubes)
    let cube_host = CubeTabHost::new(services.clone());

    // AI Guide module (tune how an MCP agent sees each tool)
    let ai_guide_page = crate::ui::ai_guide_page::AiGuidePage::new(services.ai_guide.clone());

    // Workflows module (research protocols)
    let workflows_page = WorkflowsPage::new(services.clone());
    {
        // View: deep-links from a workflow step navigate the ViewStack.
        let view_stack = view_stack.clone();
        workflows_page.set_on_navigate(move |view_key| {
            let target = match view_key {
                "search" => "search",
                "research" => "research",
                "storage" => "storage",
                "notebook" => "notebook",
                "fitsViewer" => "fits",
                "workflows" => "workflows",
                "aiGuide" => "aiguide",
                "portal" | "landing" => "home",
                // Any unknown key: no-op (matches Windows tolerance).
                _ => return,
            };
            view_stack.set_visible_child_name(target);
        });
    }

    view_stack.add_titled_with_icon(
        &dashboard_placeholder,
        Some("home"),
        crate::tr_en!("Home"),
        "go-home-symbolic",
    );
    view_stack.add_titled_with_icon(
        vospace_browser.widget(),
        Some("storage"),
        crate::tr_en!("Storage"),
        "drive-multidisk-symbolic",
    );
    view_stack.add_titled_with_icon(
        fits_viewer.widget(),
        Some("fits"),
        crate::tr_en!("FITS Viewer"),
        "image-x-generic-symbolic",
    );
    view_stack.add_titled_with_icon(
        search_page.widget(),
        Some("search"),
        crate::tr_en!("Search"),
        "system-search-symbolic",
    );
    view_stack.add_titled_with_icon(
        research_page.widget(),
        Some("research"),
        crate::tr_en!("Research"),
        "document-open-recent-symbolic",
    );
    // Observation detail is a contextual page pushed onto the content
    // NavigationView (automatic back button), not a hidden stack child.
    let obs_detail_nav_page = {
        let tv = adw::ToolbarView::new();
        tv.add_top_bar(&adw::HeaderBar::new());
        tv.set_content(Some(obs_detail.widget()));
        adw::NavigationPage::new(&tv, crate::tr_en!("Observation Detail"))
    };
    view_stack.add_titled_with_icon(
        notebook_host.widget(),
        Some("notebook"),
        crate::tr_en!("Notebook"),
        "accessories-text-editor-symbolic",
    );
    view_stack.add_titled_with_icon(
        workflows_page.widget(),
        Some("workflows"),
        crate::tr_en!("Workflows"),
        "view-list-symbolic",
    );
    view_stack.add_titled_with_icon(
        cube_host.widget(),
        Some("cube"),
        crate::tr_en!("Cube Viewer"),
        "view-paged-symbolic",
    );
    view_stack.add_titled_with_icon(
        ai_guide_page.widget(),
        Some("aiguide"),
        crate::tr_en!("AI Guide"),
        "applications-science-symbolic",
    );

    let dashboard: Rc<RefCell<Option<DashboardView>>> = Rc::new(RefCell::new(None));
    let cached_user_info: Rc<RefCell<Option<UserInfo>>> = Rc::new(RefCell::new(None));

    // Preferences — the settings page presented as a preferences window
    // (HIG: app-level settings open from the primary menu, not a nav page).
    {
        let prefs = adw::PreferencesWindow::new();
        prefs.set_transient_for(Some(&window));
        prefs.set_hide_on_close(true);
        prefs.set_default_size(720, 640);
        prefs.add(&settings_page.widget);
        let prefs_action = gtk::gio::SimpleAction::new("preferences", None);
        prefs_action.connect_activate(move |_, _| prefs.present());
        app.add_action(&prefs_action);
        app.set_accels_for_action("app.preferences", &["<Primary>comma"]);
    }

    // About + Help (primary-menu entries)
    {
        let window_clone = window.clone();
        let about_action = gtk::gio::SimpleAction::new("about", None);
        about_action.connect_activate(move |_, _| show_about_dialog(&window_clone));
        app.add_action(&about_action);

        let window_clone = window.clone();
        let help_action = gtk::gio::SimpleAction::new("help", None);
        help_action.connect_activate(move |_, _| {
            gtk::UriLauncher::new("https://www.canfar.net").launch(
                Some(&window_clone),
                None::<&gtk::gio::Cancellable>,
                |_| {},
            );
        });
        app.add_action(&help_action);
    }

    // Profile action
    {
        let window_clone = window.clone();
        let cached_user_info = cached_user_info.clone();
        let profile_action = gtk::gio::SimpleAction::new("profile", None);
        profile_action.connect_activate(move |_, _| {
            let info = cached_user_info.borrow().clone();
            if let Some(info) = info {
                show_profile_dialog(&window_clone, &info);
            }
        });
        app.add_action(&profile_action);
    }

    // Give the VOSpace browser a reference to the toast overlay so it can show notifications
    vospace_browser.set_toast_overlay(toast_overlay.clone());

    // Open FITS file action — triggered by VOSpace browser "Open in FITS Viewer"
    {
        let fits_viewer = fits_viewer.clone();
        let view_stack = view_stack.clone();
        let open_fits_action =
            gtk::gio::SimpleAction::new("open-fits-file", Some(glib::VariantTy::STRING));
        open_fits_action.connect_activate(move |_, param| {
            if let Some(path_str) = param.and_then(|v| v.str()) {
                let path = std::path::PathBuf::from(path_str);
                fits_viewer.load_from_path(&path);
                view_stack.set_visible_child_name("fits");
            }
        });
        app.add_action(&open_fits_action);
    }

    // Open cube file action — triggered by VOSpace browser "Open in Cube Viewer"
    {
        let cube_host = cube_host.clone();
        let view_stack = view_stack.clone();
        let open_cube_action =
            gtk::gio::SimpleAction::new("open-cube-file", Some(glib::VariantTy::STRING));
        open_cube_action.connect_activate(move |_, param| {
            if let Some(path_str) = param.and_then(|v| v.str()) {
                let path = std::path::PathBuf::from(path_str);
                view_stack.set_visible_child_name("cube");
                cube_host.open_path(&path);
            }
        });
        app.add_action(&open_cube_action);
    }

    // Open CAOM2 observation detail — triggered from Search results / Research.
    // Pushes the contextual detail page (automatic back button pops it).
    {
        let obs_detail = obs_detail.clone();
        let content_nav = content_nav.clone();
        let obs_detail_nav_page = obs_detail_nav_page.clone();
        let open_detail_action =
            gtk::gio::SimpleAction::new("open-observation-detail", Some(glib::VariantTy::STRING));
        open_detail_action.connect_activate(move |_, param| {
            if let Some(publisher_id) = param.and_then(|v| v.str()) {
                if content_nav.visible_page().as_ref() != Some(&obs_detail_nav_page) {
                    content_nav.push(&obs_detail_nav_page);
                }
                obs_detail.show(publisher_id);
            }
        });
        app.add_action(&open_detail_action);
    }

    // MCP view-state bridge: push view changes into the shared snapshot, and drain
    // agent steering actions (navigate/open/focus) on the GTK main loop.
    {
        // Push the active view into the snapshot whenever it changes.
        {
            let view_stack_ref = view_stack.clone();
            view_stack.connect_visible_child_name_notify(move |_| {
                let key = view_stack_ref
                    .visible_child_name()
                    .map(|s| s.to_string())
                    .unwrap_or_default();
                crate::mcp::view_state::set_view(&key, &key);
            });
        }
        // Install the action channel and run its receiver on the main loop.
        let (vs_tx, mut vs_rx) =
            tokio::sync::mpsc::unbounded_channel::<crate::mcp::view_state::ViewAction>();
        crate::mcp::view_state::install_action_sender(vs_tx);
        {
            let view_stack = view_stack.clone();
            let app = app.clone();
            let search_page = search_page.clone();
            glib::spawn_future_local(async move {
                use crate::mcp::view_state::ViewAction;
                while let Some(action) = vs_rx.recv().await {
                    match action {
                        ViewAction::Navigate { key, reply } => {
                            let ok = match map_view_key(&key) {
                                // Settings now lives in the preferences window.
                                Some("settings") => {
                                    app.activate_action("preferences", None);
                                    true
                                }
                                Some(t) => {
                                    view_stack.set_visible_child_name(t);
                                    true
                                }
                                None => false,
                            };
                            let _ = reply.send(ok);
                        }
                        ViewAction::OpenFits { path, reply } => {
                            app.activate_action(
                                "open-fits-file",
                                Some(&glib::Variant::from(path.as_str())),
                            );
                            let _ = reply.send(true);
                        }
                        ViewAction::SetSearchFocus { ra, dec, reply } => {
                            view_stack.set_visible_child_name("search");
                            search_page.show_search_form(ra, dec);
                            let _ = reply.send(true);
                        }
                        ViewAction::CloseActiveTab { reply } => {
                            // Per-module tab closing is not yet wired; report no-op.
                            let _ = reply.send(false);
                        }
                    }
                }
            });
        }

        // Live per-viewer command channel (cube / notebook / fits MCP tools).
        let (vc_tx, mut vc_rx) =
            tokio::sync::mpsc::unbounded_channel::<crate::mcp::view_state::ViewerCommand>();
        crate::mcp::view_state::install_viewer_sender(vc_tx);
        {
            let cube_host = cube_host.clone();
            let notebook_host = notebook_host.clone();
            let fits_viewer = fits_viewer.clone();
            let search_page = search_page.clone();
            glib::spawn_future_local(async move {
                while let Some(cmd) = vc_rx.recv().await {
                    let result = match cmd.target.as_str() {
                        "cube" => cube_host.handle_viewer_command(&cmd.op, &cmd.args).await,
                        "notebook" => {
                            notebook_host
                                .handle_viewer_command(&cmd.op, &cmd.args)
                                .await
                        }
                        "fits" => fits_viewer.handle_viewer_command(&cmd.op, &cmd.args).await,
                        "search" => search_page.handle_viewer_command(&cmd.op, &cmd.args).await,
                        other => Err(format!("unknown viewer target: {other}")),
                    };
                    let _ = cmd.reply.send(result);
                }
            });
        }
    }

    // Open notebook file action — triggered by VOSpace browser "Open in Notebook"
    {
        let notebook_host = notebook_host.clone();
        let view_stack = view_stack.clone();
        let open_notebook_action =
            gtk::gio::SimpleAction::new("open-notebook-file", Some(glib::VariantTy::STRING));
        open_notebook_action.connect_activate(move |_, param| {
            if let Some(path_str) = param.and_then(|v| v.str()) {
                notebook_host.load_from_path(&std::path::PathBuf::from(path_str));
                view_stack.set_visible_child_name("notebook");
            }
        });
        app.add_action(&open_notebook_action);
    }

    // navigate-research action — invoked from toast action buttons,
    // Ctrl+Shift+R, or the view switcher. Also refreshes the list.
    {
        let view_stack = view_stack.clone();
        let research_page = research_page.clone();
        let nav_action = gtk::gio::SimpleAction::new("navigate-research", None);
        nav_action.connect_activate(move |_, _| {
            view_stack.set_visible_child_name("research");
            research_page.reload();
        });
        app.add_action(&nav_action);
        app.set_accels_for_action("app.navigate-research", &["<Primary><Shift>R"]);
    }

    // navigate-search action — used by Research empty-state CTA
    {
        let view_stack = view_stack.clone();
        let nav_action = gtk::gio::SimpleAction::new("navigate-search", None);
        nav_action.connect_activate(move |_, _| {
            view_stack.set_visible_child_name("search");
        });
        app.add_action(&nav_action);
    }

    // File panel — open-file callback
    {
        let fits_viewer = fits_viewer.clone();
        let view_stack = view_stack.clone();
        let notebook_host = notebook_host.clone();
        file_panel.set_on_open_file(move |path, file_type| match file_type {
            FileType::Fits => {
                fits_viewer.load_from_path(&path);
                view_stack.set_visible_child_name("fits");
            }
            FileType::Notebook => {
                notebook_host.load_from_path(&path);
                view_stack.set_visible_child_name("notebook");
            }
            FileType::Other => {}
        });
    }

    // Files toggle button
    {
        let file_panel = file_panel.clone();
        files_btn.connect_toggled(move |btn| {
            file_panel.widget().set_visible(btn.is_active());
        });
    }

    // Logout action
    {
        let services = services.clone();
        let login_btn = login_btn.clone();
        let user_menu_btn = user_menu_btn.clone();
        let status_label = status_label.clone();
        let view_stack = view_stack.clone();
        let dashboard = dashboard.clone();
        let cached_user_info = cached_user_info.clone();
        let welcome = welcome.clone();

        let logout_action = gtk::gio::SimpleAction::new("logout", None);
        logout_action.connect_activate(move |_, _| {
            let services = services.clone();
            let login_btn = login_btn.clone();
            let user_menu_btn = user_menu_btn.clone();
            let status_label = status_label.clone();
            let view_stack = view_stack.clone();
            let dashboard = dashboard.clone();
            let cached_user_info = cached_user_info.clone();
            let welcome = welcome.clone();

            glib::spawn_future_local(async move {
                let svc = services.clone();
                services.spawn(async move { svc.clear_auth().await }).await;
                services.notifications.clear();
                login_btn.set_visible(true);
                user_menu_btn.set_visible(false);
                user_menu_btn.set_label("");
                status_label.set_text("");
                *cached_user_info.borrow_mut() = None;

                // Re-lock the auth-gated landing tiles, then show the home view.
                welcome.set_authenticated(false);
                view_stack.set_visible_child_name("home");
                *dashboard.borrow_mut() = None;

                services
                    .toast
                    .toast(crate::tr_en!("Logged out successfully"));
            });
        });
        app.add_action(&logout_action);
    }

    // Everything the shell changes on sign-in, shared by every path that can
    // start a session.
    let signed_in = SignedInChrome {
        login_btn: login_btn.clone(),
        user_menu_btn: user_menu_btn.clone(),
        status_label: status_label.clone(),
        view_stack: view_stack.clone(),
        dashboard: dashboard.clone(),
        cached_user_info: cached_user_info.clone(),
        vospace: vospace_browser.clone(),
        welcome: welcome.clone(),
    };

    // Login button
    {
        let window_clone = window.clone();
        let services = services.clone();
        let signed_in = signed_in.clone();

        login_btn.connect_clicked(move |_| {
            let window = window_clone.clone();
            let services = services.clone();
            let signed_in = signed_in.clone();

            glib::spawn_future_local(async move {
                if let Some((_username, _token, user_info)) =
                    show_login_dialog(&window, &services).await
                {
                    signed_in.apply(user_info, &services).await;
                }
            });
        });
    }

    // Signing in from the observation detail page's proprietary-data panel.
    // The reference offers Sign in there rather than Retry, and reloads the
    // observation afterwards — telling someone to go and find the account
    // button, then come back and press Retry, is three steps where one will do.
    {
        let window_clone = window.clone();
        let services = services.clone();
        let signed_in = signed_in.clone();
        let page = obs_detail.clone();

        obs_detail.set_on_sign_in(move || {
            let window = window_clone.clone();
            let services = services.clone();
            let signed_in = signed_in.clone();
            let page = page.clone();

            glib::spawn_future_local(async move {
                if let Some((_username, _token, user_info)) =
                    show_login_dialog(&window, &services).await
                {
                    signed_in.apply(user_info, &services).await;
                    page.reload().await;
                }
            });
        });
    }

    // Try auto-login on startup
    {
        let services = services.clone();
        let status_label = status_label.clone();
        let spinner = spinner.clone();
        let signed_in = signed_in.clone();

        glib::spawn_future_local(async move {
            if let Some(stored_token) = TokenStorage::get_token() {
                spinner.set_visible(true);
                spinner.start();
                status_label.set_text(crate::tr_en!("Checking authentication..."));

                let token_clone = stored_token.clone();
                let svc = services.clone();
                let validate_result = services
                    .spawn(async move { svc.auth.validate_token(&token_clone).await })
                    .await;

                match validate_result {
                    Ok(username) => {
                        let svc = services.clone();
                        let tok = stored_token.clone();
                        let user = username.clone();
                        services
                            .spawn(async move {
                                svc.set_auth(tok, user).await;
                            })
                            .await;

                        let svc = services.clone();
                        let tok = stored_token.clone();
                        let user_info =
                            services
                                .spawn(async move {
                                    svc.auth.get_user_info(&tok).await.unwrap_or_default()
                                })
                                .await;

                        let svc = services.clone();
                        let info = user_info.clone();
                        services
                            .spawn(async move {
                                svc.set_user_info(info).await;
                            })
                            .await;

                        signed_in.apply(user_info, &services).await;
                    }
                    Err(_) => {
                        TokenStorage::clear();
                        status_label.set_text(crate::tr_en!("Session expired. Please login."));
                    }
                }

                spinner.stop();
                spinner.set_visible(false);
            }
        });
    }

    // Periodic health status UI refresh (every 10 seconds)
    {
        use crate::services::health_tracker::{ServiceName, ServiceStatus};

        let services = services.clone();
        let health_icon = health_icon.clone();
        let health_label = health_label.clone();
        let health_list = health_list.clone();
        let banner = banner.clone();

        glib::timeout_add_seconds_local(10, move || {
            let count = services.health.unreachable_count();

            // Update header indicator
            if count == 0 {
                health_icon.set_icon_name(Some("network-idle-symbolic"));
                health_label.set_text(crate::tr_en!("Connected"));
                health_icon.remove_css_class("warning");
                health_icon.remove_css_class("error");
                health_icon.add_css_class("success");
            } else {
                health_icon.set_icon_name(Some("dialog-warning-symbolic"));
                health_label.set_text(&crate::tr_fmt!("{} offline", count));
                health_icon.remove_css_class("success");
                health_icon.add_css_class("warning");
            }

            // Update banner visibility
            banner.set_revealed(count > 0);

            // Rebuild popover list
            while let Some(child) = health_list.first_child() {
                health_list.remove(&child);
            }
            for svc_name in ServiceName::all() {
                let status = services.health.get(svc_name);
                let row = adw::ActionRow::builder()
                    .title(svc_name.to_string())
                    .build();
                row.add_prefix(&gtk::Image::from_icon_name(svc_name.icon_name()));

                let status_lbl = gtk::Label::new(None);
                status_lbl.add_css_class("caption");
                match &status {
                    ServiceStatus::Unknown => {
                        status_lbl.set_text(crate::tr_en!("Unknown"));
                        status_lbl.add_css_class("dim-label");
                    }
                    ServiceStatus::Reachable => {
                        status_lbl.set_text(crate::tr_en!("Online"));
                        status_lbl.add_css_class("success");
                    }
                    ServiceStatus::Unreachable { since, .. } => {
                        let local: chrono::DateTime<chrono::Local> = (*since).into();
                        row.set_subtitle(&crate::tr_fmt!("Last seen {}", local.format("%H:%M")));
                        status_lbl.set_text(crate::tr_en!("Offline"));
                        status_lbl.add_css_class("error");
                    }
                }
                row.add_suffix(&status_lbl);
                health_list.append(&row);
            }

            glib::ControlFlow::Continue
        });
    }

    // Sidebar selection ↔ view sync, content title, and research auto-refresh.
    {
        let research_page_for_nav = research_page.clone();
        let sidebar_list_for_sync = sidebar_list.clone();
        let nav_keys = nav_keys.clone();
        let content_root_page = content_root_page.clone();
        view_stack.connect_notify_local(Some("visible-child-name"), move |vs, _| {
            if let Some(name) = vs.visible_child_name() {
                // Auto-refresh the Research page whenever the user navigates to it
                // so downloads saved from the Search page show up immediately.
                if name.as_str() == "research" {
                    research_page_for_nav.reload();
                }
                // Reflect the active view in the sidebar + the content title.
                if let Some(idx) = nav_keys.iter().position(|k| *k == name.as_str()) {
                    if let Some(row) = sidebar_list_for_sync.row_at_index(idx as i32) {
                        sidebar_list_for_sync.select_row(Some(&row));
                    }
                }
                if let Some(child) = vs.visible_child() {
                    let title = vs.page(&child).title().unwrap_or_default();
                    content_root_page.set_title(&title);
                }
            }
        });
        // Select the initial row (Home).
        if let Some(row) = sidebar_list.row_at_index(0) {
            sidebar_list.select_row(Some(&row));
        }
    }

    // Keyboard shortcuts
    setup_keyboard_shortcuts(
        &window,
        &view_stack,
        &file_panel,
        &files_btn,
        &notebook_host,
        &content_nav,
    );

    window.present();
}

// ---------------------------------------------------------------------------
// Keyboard shortcuts
// ---------------------------------------------------------------------------

fn setup_keyboard_shortcuts(
    window: &adw::ApplicationWindow,
    view_stack: &adw::ViewStack,
    file_panel: &Rc<FilePanel>,
    files_btn: &gtk::ToggleButton,
    notebook_host: &Rc<NotebookTabHost>,
    content_nav: &adw::NavigationView,
) {
    let controller = gtk::EventControllerKey::new();
    let vs = view_stack.clone();
    let fp = Rc::clone(file_panel);
    let fb = files_btn.clone();
    let nh = notebook_host.clone();
    let nav = content_nav.clone();
    let win = window.clone();
    controller.connect_key_pressed(move |_, key, _code, modifier| {
        let ctrl = modifier.contains(gtk4::gdk::ModifierType::CONTROL_MASK);
        let shift = modifier.contains(gtk4::gdk::ModifierType::SHIFT_MASK);
        let alt = modifier.contains(gtk4::gdk::ModifierType::ALT_MASK);
        let on_notebook = vs.visible_child_name().as_deref() == Some("notebook");

        // Alt+Left → pop a pushed contextual page (observation detail)
        if alt && key == gtk4::gdk::Key::Left {
            nav.pop();
            return gtk::glib::Propagation::Stop;
        }

        if ctrl {
            match key {
                gtk4::gdk::Key::comma => {
                    if let Some(app) = win.application() {
                        app.activate_action("preferences", None);
                    }
                    return gtk::glib::Propagation::Stop;
                }
                gtk4::gdk::Key::_1 => {
                    vs.set_visible_child_name("home");
                    return gtk::glib::Propagation::Stop;
                }
                gtk4::gdk::Key::_2 => {
                    vs.set_visible_child_name("storage");
                    return gtk::glib::Propagation::Stop;
                }
                gtk4::gdk::Key::_3 => {
                    vs.set_visible_child_name("fits");
                    return gtk::glib::Propagation::Stop;
                }
                gtk4::gdk::Key::_4 => {
                    vs.set_visible_child_name("search");
                    return gtk::glib::Propagation::Stop;
                }
                gtk4::gdk::Key::_5 => {
                    vs.set_visible_child_name("research");
                    return gtk::glib::Propagation::Stop;
                }
                gtk4::gdk::Key::_6 => {
                    vs.set_visible_child_name("notebook");
                    return gtk::glib::Propagation::Stop;
                }
                gtk4::gdk::Key::_7 => {
                    if let Some(app) = win.application() {
                        app.activate_action("preferences", None);
                    }
                    return gtk::glib::Propagation::Stop;
                }
                gtk4::gdk::Key::b => {
                    // Toggle file panel visibility and keep toggle button in sync
                    let new_visible = !fp.widget().is_visible();
                    fp.widget().set_visible(new_visible);
                    fb.set_active(new_visible);
                    return gtk::glib::Propagation::Stop;
                }
                // Notebook shortcuts: only active when the notebook page is visible
                gtk4::gdk::Key::n if on_notebook && !shift => {
                    nh.trigger_new();
                    return gtk::glib::Propagation::Stop;
                }
                gtk4::gdk::Key::o if on_notebook && !shift => {
                    nh.trigger_open();
                    return gtk::glib::Propagation::Stop;
                }
                gtk4::gdk::Key::s if on_notebook && !shift => {
                    nh.trigger_save();
                    return gtk::glib::Propagation::Stop;
                }
                gtk4::gdk::Key::s if on_notebook && shift => {
                    nh.trigger_save_as();
                    return gtk::glib::Propagation::Stop;
                }
                _ => {}
            }
        }
        gtk::glib::Propagation::Proceed
    });
    window.add_controller(controller);
}

// ---------------------------------------------------------------------------
// Profile dialog
// ---------------------------------------------------------------------------

fn show_profile_dialog(window: &adw::ApplicationWindow, info: &UserInfo) {
    let dialog = adw::Window::builder()
        .title(crate::tr_en!("User Profile"))
        .default_width(360)
        .default_height(300)
        .modal(true)
        .transient_for(window)
        .build();

    let toolbar_view = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    toolbar_view.add_top_bar(&header);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
    content.set_margin_start(24);
    content.set_margin_end(24);
    content.set_margin_top(16);
    content.set_margin_bottom(24);
    content.set_halign(gtk::Align::Center);

    let avatar = adw::Avatar::new(64, Some(&info.display_name()), true);
    avatar.set_halign(gtk::Align::Center);
    avatar.set_margin_bottom(8);
    content.append(&avatar);

    let name_label = gtk::Label::new(Some(&info.display_name()));
    name_label.add_css_class("title-3");
    name_label.set_halign(gtk::Align::Center);
    content.append(&name_label);

    if let Some(ref username) = info.username {
        let lbl = gtk::Label::new(Some(&format!("@{}", username)));
        lbl.add_css_class("dim-label");
        lbl.set_halign(gtk::Align::Center);
        content.append(&lbl);
    }

    let sep = gtk::Separator::new(gtk::Orientation::Horizontal);
    sep.set_margin_top(8);
    sep.set_margin_bottom(8);
    content.append(&sep);

    let group = adw::PreferencesGroup::new();

    if let Some(ref email) = info.email {
        if !email.is_empty() {
            let row = adw::ActionRow::builder()
                .title(crate::tr_en!("Email"))
                .subtitle(email)
                .build();
            row.add_prefix(&gtk::Image::from_icon_name("mail-unread-symbolic"));
            group.add(&row);
        }
    }

    if let Some(ref institute) = info.institute {
        if !institute.is_empty() {
            let row = adw::ActionRow::builder()
                .title(crate::tr_en!("Institute"))
                .subtitle(institute)
                .build();
            row.add_prefix(&gtk::Image::from_icon_name("building-symbolic"));
            group.add(&row);
        }
    }

    if let Some(ref id) = info.internal_id {
        if !id.is_empty() {
            let row = adw::ActionRow::builder()
                .title(crate::tr_en!("Internal ID"))
                .subtitle(id)
                .build();
            row.add_prefix(&gtk::Image::from_icon_name("contact-new-symbolic"));
            group.add(&row);
        }
    }

    content.append(&group);

    toolbar_view.set_content(Some(&content));
    dialog.set_content(Some(&toolbar_view));
    dialog.present();
}

// ---------------------------------------------------------------------------
// Session
// ---------------------------------------------------------------------------

/// Everything the shell changes when a session starts.
///
/// One place because three paths lead here — the Login button, the startup
/// auto-login, and the observation detail page's Sign in — and they must all
/// leave the app in the same state. Two of them held their own copy of this
/// sequence, differing only in a comment; the third would have been a fourth
/// chance to forget one line.
#[derive(Clone)]
struct SignedInChrome {
    login_btn: gtk::Button,
    user_menu_btn: gtk::MenuButton,
    status_label: gtk::Label,
    view_stack: adw::ViewStack,
    dashboard: Rc<RefCell<Option<DashboardView>>>,
    cached_user_info: Rc<RefCell<Option<UserInfo>>>,
    vospace: Rc<VoSpaceBrowser>,
    welcome: Rc<WelcomePage>,
}

impl SignedInChrome {
    /// Swap the shell into its signed-in state for `user_info`.
    async fn apply(&self, user_info: UserInfo, services: &Arc<AppServices>) {
        let display = user_info.display_name();
        self.login_btn.set_visible(false);
        self.user_menu_btn.set_label(&display);
        self.user_menu_btn.set_visible(true);
        self.status_label
            .set_text(&crate::tr_fmt!("Welcome, {}", &display));
        *self.cached_user_info.borrow_mut() = Some(user_info);

        // Unlock the auth-gated landing tiles BEFORE swapping in the dashboard,
        // or the tiles render locked behind a view the user can already use.
        self.welcome.set_authenticated(true);
        navigate_to_dashboard(&self.view_stack, services, &self.dashboard).await;
        self.vospace.refresh().await;

        services
            .toast
            .toast(crate::tr_fmt!("Welcome back, {}!", &display));
    }
}

// ---------------------------------------------------------------------------
// Navigation
// ---------------------------------------------------------------------------

async fn navigate_to_dashboard(
    view_stack: &adw::ViewStack,
    services: &Arc<AppServices>,
    dashboard: &Rc<RefCell<Option<DashboardView>>>,
) {
    // Remove placeholder and replace with real dashboard
    if let Some(placeholder) = view_stack.child_by_name("home") {
        view_stack.remove(&placeholder);
    }

    let view = DashboardView::new(services.clone());
    view_stack.add_titled_with_icon(
        view.widget(),
        Some("home"),
        crate::tr_en!("Dashboard"),
        "view-grid-symbolic",
    );
    view_stack.set_visible_child_name("home");

    view.load_data().await;
    *dashboard.borrow_mut() = Some(view);
}

fn show_about_dialog(window: &adw::ApplicationWindow) {
    let dialog = adw::AboutWindow::builder()
        .application_name("Verbinal")
        .application_icon("net.canfar.Verbinal")
        .version(env!("CARGO_PKG_VERSION"))
        .comments(crate::tr_en!("A CANFAR Science Portal Companion\n\nLaunch, monitor, and manage your interactive computing sessions (Notebook, Desktop, CARTA, Firefly) directly from your desktop without needing a browser.\n\nCANFAR is operated by the Canadian Astronomy Data Centre (CADC) and the Digital Research Alliance of Canada."))
        .website("https://www.canfar.net")
        .license_type(gtk::License::Agpl30)
        .copyright("\u{00a9} 2025 Serhii Zautkin")
        .developers(vec!["Serhii Zautkin"])
        .transient_for(window)
        .modal(true)
        .build();

    dialog.add_legal_section(
        crate::tr_en!("Runtime Info"),
        None,
        gtk::License::Custom,
        Some(&crate::tr_fmt!(
            "Runtime: Rust {}\nPlatform: {}\nFramework: GTK4 + libadwaita",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
        )),
    );

    dialog.present();
}

// ---------------------------------------------------------------------------
// Terms-of-Use gate
// ---------------------------------------------------------------------------

/// Show the blocking, non-dismissible Terms-of-Use gate layered over the shell.
///
/// Port of `ShowTermsGateIfNeeded` in CanfarDesktop's MainWindow. An opaque,
/// theme-aware backdrop covers the whole window and the shell beneath is made
/// insensitive so no control behind the gate is reachable by pointer or Tab.
/// "Accept" records the accepted version (via [`LegalAgreementService::accept`])
/// and frees the shell; "Decline & Exit" quits the app.
///
/// [`LegalAgreementService::accept`]: crate::services::legal_service::LegalAgreementService::accept
fn show_terms_gate(
    app: &adw::Application,
    root_overlay: &gtk::Overlay,
    shell: &adw::ToolbarView,
    legal: Rc<crate::services::legal_service::LegalAgreementService>,
    french: bool,
) {
    // Opaque, theme-aware backdrop so nothing behind the gate shows through.
    let provider = gtk::CssProvider::new();
    provider.load_from_string(".terms-gate-backdrop { background-color: @window_bg_color; }");
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }

    let backdrop = gtk::Box::new(gtk::Orientation::Vertical, 0);
    backdrop.add_css_class("terms-gate-backdrop");
    backdrop.set_hexpand(true);
    backdrop.set_vexpand(true);
    // Capture all input so the shell beneath never receives it.
    backdrop.set_can_target(true);

    // Centered card holding the terms + actions.
    let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
    card.add_css_class("card");
    card.set_halign(gtk::Align::Center);
    card.set_valign(gtk::Align::Center);
    card.set_hexpand(true);
    card.set_vexpand(true);
    card.set_size_request(560, -1);

    let inner = gtk::Box::new(gtk::Orientation::Vertical, 16);
    inner.set_margin_top(24);
    inner.set_margin_bottom(24);
    inner.set_margin_start(24);
    inner.set_margin_end(24);

    let title = gtk::Label::new(Some(crate::helpers::legal_terms::title(french)));
    title.add_css_class("title-2");
    title.set_halign(gtk::Align::Start);
    inner.append(&title);

    let body_scroll = gtk::ScrolledWindow::new();
    body_scroll.set_min_content_height(320);
    body_scroll.set_max_content_height(420);
    body_scroll.set_vexpand(true);
    body_scroll.set_hscrollbar_policy(gtk::PolicyType::Never);
    let body = gtk::Label::new(Some(crate::helpers::legal_terms::body(french)));
    body.set_wrap(true);
    body.set_xalign(0.0);
    body.set_selectable(true);
    body_scroll.set_child(Some(&body));
    inner.append(&body_scroll);

    let button_row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    button_row.set_halign(gtk::Align::End);
    button_row.set_margin_top(8);
    let decline_btn = gtk::Button::with_label(crate::tr_en!("Decline & Exit"));
    let accept_btn = gtk::Button::with_label(crate::tr_en!("Accept"));
    accept_btn.add_css_class("suggested-action");
    button_row.append(&decline_btn);
    button_row.append(&accept_btn);
    inner.append(&button_row);

    card.append(&inner);
    backdrop.append(&card);

    // Block the shell (pointer + focus/Tab order) while the gate is up.
    shell.set_sensitive(false);
    root_overlay.add_overlay(&backdrop);

    {
        let legal = legal.clone();
        let root_overlay = root_overlay.clone();
        let backdrop = backdrop.clone();
        let shell = shell.clone();
        accept_btn.connect_clicked(move |_| {
            legal.accept();
            root_overlay.remove_overlay(&backdrop);
            shell.set_sensitive(true);
        });
    }
    {
        let app = app.clone();
        decline_btn.connect_clicked(move |_| {
            app.quit();
        });
    }

    accept_btn.grab_focus();
}

// ---------------------------------------------------------------------------
// Welcome page with feature tiles
// ---------------------------------------------------------------------------

/// Map an agent-facing view key to a ViewStack child name. `None` = unknown key.
fn map_view_key(key: &str) -> Option<&'static str> {
    match key {
        "home" | "portal" | "landing" => Some("home"),
        "search" => Some("search"),
        "storage" => Some("storage"),
        "fits" | "fitsViewer" => Some("fits"),
        "notebook" => Some("notebook"),
        "research" => Some("research"),
        "cube" => Some("cube"),
        "workflows" => Some("workflows"),
        "aiguide" | "aiGuide" => Some("aiguide"),
        "settings" => Some("settings"),
        _ => None,
    }
}

/// Retained handle to the landing launchpad so the shell can lock/unlock the
/// auth-gated tiles as the sign-in state changes.
struct WelcomePage {
    root: gtk::Box,
    /// One setter per auth-gated tile; `true` = locked (signed out).
    lockers: Vec<Rc<dyn Fn(bool)>>,
}

impl WelcomePage {
    /// Lock (signed out) or unlock (signed in) the auth-gated tiles.
    /// Mirrors `LandingView.SetAuthenticated` in CanfarDesktop.
    fn set_authenticated(&self, authenticated: bool) {
        for lock in &self.lockers {
            lock(!authenticated);
        }
    }
}

/// What a landing tile does when clicked.
enum TileAction {
    /// Navigate to a ViewStack child. `requires_auth` tiles are dimmed + locked
    /// while signed out and route a click to the login flow instead.
    Navigate {
        page: &'static str,
        requires_auth: bool,
    },
    /// Open the AI-Assistant connect wizard.
    AiAssistant,
}

struct TileSpec {
    icon: &'static str,
    title: &'static str,
    desc: &'static str,
    action: TileAction,
}

/// Opt-in read for the AI-Guide landing tile.
///
/// Task rule: the tile is hidden unless the user has explicitly enabled it, so a
/// fresh install (no `mcp_settings.json`) keeps it hidden. We read the shared
/// MCP-settings JSON directly and only treat the tile as visible when the
/// `show_ai_guide_tile` key is explicitly present and `true`.
fn read_show_ai_guide_tile() -> bool {
    let Some(dirs) = directories::ProjectDirs::from("net", "canfar", "Verbinal") else {
        return false;
    };
    let path = dirs.data_dir().join("mcp_settings.json");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return false;
    };
    serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|v| v.get("show_ai_guide_tile").and_then(|b| b.as_bool()))
        .unwrap_or(false)
}

fn build_welcome_page(
    view_stack: &adw::ViewStack,
    window: &adw::ApplicationWindow,
    services: &Arc<AppServices>,
    login_btn: &gtk::Button,
    show_ai_guide_tile: bool,
) -> WelcomePage {
    let page = gtk::Box::new(gtk::Orientation::Vertical, 0);
    page.set_vexpand(true);

    let scrolled = gtk::ScrolledWindow::new();
    scrolled.set_vexpand(true);
    // Never scroll sideways: the tiles reflow to fit the width, so a horizontal
    // bar would only ever mean the layout failed. Vertical stays automatic for
    // short windows.
    scrolled.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 24);
    content.set_margin_start(48);
    content.set_margin_end(48);
    content.set_margin_top(48);
    content.set_margin_bottom(48);
    content.set_valign(gtk::Align::Center);
    content.set_vexpand(true);

    // App icon + title
    let header_box = gtk::Box::new(gtk::Orientation::Vertical, 12);
    header_box.set_halign(gtk::Align::Center);

    let app_icon = load_app_icon(96);
    app_icon.set_halign(gtk::Align::Center);
    header_box.append(&app_icon);

    let title = gtk::Label::new(Some("Verbinal"));
    title.add_css_class("title-1");
    title.set_halign(gtk::Align::Center);
    header_box.append(&title);

    let subtitle = gtk::Label::new(Some(crate::tr_en!("A CANFAR Science Portal Companion")));
    subtitle.add_css_class("dim-label");
    subtitle.set_halign(gtk::Align::Center);
    header_box.append(&subtitle);

    let version_label = gtk::Label::new(Some(&format!("v{}", env!("CARGO_PKG_VERSION"))));
    version_label.add_css_class("dim-label");
    version_label.add_css_class("caption");
    version_label.set_halign(gtk::Align::Center);
    header_box.append(&version_label);

    content.append(&header_box);

    // Feature tiles laid out in a 3-column grid. Portal & Storage are auth-gated
    // (locked while signed out); AI Assistant is always shown; AI Guide is opt-in.
    let mut specs = vec![
        TileSpec {
            icon: "computer-symbolic",
            title: crate::tr_en!("Portal"),
            desc: crate::tr_en!("Manage sessions & data"),
            action: TileAction::Navigate {
                page: "home",
                requires_auth: true,
            },
        },
        TileSpec {
            icon: "system-search-symbolic",
            title: crate::tr_en!("Search"),
            desc: crate::tr_en!("Explore the CADC archive"),
            action: TileAction::Navigate {
                page: "search",
                requires_auth: false,
            },
        },
        TileSpec {
            icon: "document-open-recent-symbolic",
            title: crate::tr_en!("Research"),
            desc: crate::tr_en!("Downloaded observations"),
            action: TileAction::Navigate {
                page: "research",
                requires_auth: false,
            },
        },
        TileSpec {
            icon: "drive-multidisk-symbolic",
            title: crate::tr_en!("Storage"),
            desc: crate::tr_en!("Browse VOSpace files"),
            action: TileAction::Navigate {
                page: "storage",
                requires_auth: true,
            },
        },
        TileSpec {
            icon: "accessories-text-editor-symbolic",
            title: crate::tr_en!("Notebook"),
            desc: crate::tr_en!("Open & run .ipynb files"),
            action: TileAction::Navigate {
                page: "notebook",
                requires_auth: false,
            },
        },
        TileSpec {
            icon: "image-x-generic-symbolic",
            title: crate::tr_en!("FITS Viewer"),
            desc: crate::tr_en!("View astronomical images"),
            action: TileAction::Navigate {
                page: "fits",
                requires_auth: false,
            },
        },
        TileSpec {
            icon: "view-list-symbolic",
            title: crate::tr_en!("Workflows"),
            desc: crate::tr_en!("Research protocols & checklists"),
            action: TileAction::Navigate {
                page: "workflows",
                requires_auth: false,
            },
        },
        TileSpec {
            icon: "view-paged-symbolic",
            title: crate::tr_en!("Cube Viewer"),
            desc: crate::tr_en!("Explore 3D spectral cubes"),
            action: TileAction::Navigate {
                page: "cube",
                requires_auth: false,
            },
        },
        TileSpec {
            icon: "network-workgroup-symbolic",
            title: crate::tr_en!("AI Assistant"),
            desc: crate::tr_en!("Connect an AI agent to help you"),
            action: TileAction::AiAssistant,
        },
    ];
    if show_ai_guide_tile {
        specs.push(TileSpec {
            icon: "applications-science-symbolic",
            title: crate::tr_en!("AI Guide"),
            desc: crate::tr_en!("Pair an AI agent over MCP"),
            action: TileAction::Navigate {
                page: "aiguide",
                requires_auth: false,
            },
        });
    }

    // A FlowBox, not a fixed 3-column Grid: the grid could not reflow, so on a
    // narrow window the tiles were clipped rather than rewrapping, and on a short
    // one the later rows were unreachable. Capped at 3 per line to keep the
    // intended wide layout, down to 1 when there is no room for more.
    let tiles = gtk::FlowBox::new();
    tiles.set_row_spacing(16);
    tiles.set_column_spacing(16);
    tiles.set_homogeneous(true);
    tiles.set_min_children_per_line(1);
    tiles.set_max_children_per_line(3);
    // Tiles are buttons; FlowBox selection would add a second, conflicting
    // notion of "chosen" on top of the button's own activation.
    tiles.set_selection_mode(gtk::SelectionMode::None);
    tiles.set_halign(gtk::Align::Center);

    let mut lockers: Vec<Rc<dyn Fn(bool)>> = Vec::new();
    for spec in specs.iter() {
        let (widget, locker) = make_tile(spec, view_stack, window, services, login_btn);
        if let Some(locker) = locker {
            lockers.push(locker);
        }
        tiles.insert(&widget, -1);
    }

    content.append(&tiles);

    // Login prompt
    let login_prompt = gtk::Label::new(Some(crate::tr_en!(
        "Log in with your CADC credentials to get started"
    )));
    login_prompt.add_css_class("dim-label");
    login_prompt.set_halign(gtk::Align::Center);
    login_prompt.set_margin_top(8);
    content.append(&login_prompt);

    scrolled.set_child(Some(&content));
    page.append(&scrolled);
    WelcomePage {
        root: page,
        lockers,
    }
}

/// A built landing tile: the widget to attach, plus — for auth-gated tiles only —
/// a `Fn(locked)` the shell calls on every sign-in/sign-out to dim the tile and
/// show its lock badge.
type BuiltTile = (gtk::Widget, Option<Rc<dyn Fn(bool)>>);

/// Build one landing tile.
fn make_tile(
    spec: &TileSpec,
    view_stack: &adw::ViewStack,
    window: &adw::ApplicationWindow,
    services: &Arc<AppServices>,
    login_btn: &gtk::Button,
) -> BuiltTile {
    let btn = gtk::Button::new();
    btn.add_css_class("flat");
    btn.add_css_class("card");
    btn.set_size_request(200, 170);
    btn.set_valign(gtk::Align::Fill);
    btn.set_halign(gtk::Align::Fill);

    let inner = gtk::Box::new(gtk::Orientation::Vertical, 8);
    inner.set_margin_start(16);
    inner.set_margin_end(16);
    inner.set_margin_top(24);
    inner.set_margin_bottom(16);
    inner.set_valign(gtk::Align::Center);
    inner.set_halign(gtk::Align::Center);

    let icon = gtk::Image::from_icon_name(spec.icon);
    icon.set_pixel_size(48);
    icon.set_halign(gtk::Align::Center);
    inner.append(&icon);

    let label = gtk::Label::new(Some(spec.title));
    label.add_css_class("title-3");
    label.set_halign(gtk::Align::Center);
    inner.append(&label);

    let desc = gtk::Label::new(Some(spec.desc));
    desc.add_css_class("dim-label");
    desc.add_css_class("caption");
    desc.set_halign(gtk::Align::Center);
    desc.set_justify(gtk::Justification::Center);
    desc.set_wrap(true);
    desc.set_max_width_chars(22);
    inner.append(&desc);

    btn.set_child(Some(&inner));
    btn.set_tooltip_text(Some(spec.desc));

    match spec.action {
        TileAction::AiAssistant => {
            let window = window.clone();
            let services = services.clone();
            btn.connect_clicked(move |_| {
                crate::ui::ai_connect_wizard::show_connect_wizard(&window, services.clone());
            });
            (btn.upcast(), None)
        }
        TileAction::Navigate {
            page,
            requires_auth: false,
        } => {
            let vs = view_stack.clone();
            btn.connect_clicked(move |_| {
                vs.set_visible_child_name(page);
            });
            (btn.upcast(), None)
        }
        TileAction::Navigate {
            page,
            requires_auth: true,
        } => {
            // Auth-gated tile: overlay a lock badge, dim the content while locked,
            // and route clicks to the login flow until signed in.
            let overlay = gtk::Overlay::new();
            overlay.set_child(Some(&btn));

            let lock_img = gtk::Image::from_icon_name("changes-prevent-symbolic");
            lock_img.set_pixel_size(20);
            lock_img.add_css_class("dim-label");
            lock_img.set_halign(gtk::Align::End);
            lock_img.set_valign(gtk::Align::Start);
            lock_img.set_margin_top(10);
            lock_img.set_margin_end(10);
            // Let clicks fall through to the button beneath.
            lock_img.set_can_target(false);
            overlay.add_overlay(&lock_img);

            let locked = Rc::new(std::cell::Cell::new(true));

            {
                let locked = locked.clone();
                let login_btn = login_btn.clone();
                let vs = view_stack.clone();
                btn.connect_clicked(move |_| {
                    if locked.get() {
                        // Signed out: open login; the login flow then continues.
                        login_btn.emit_clicked();
                    } else {
                        vs.set_visible_child_name(page);
                    }
                });
            }

            let locker: Rc<dyn Fn(bool)> = {
                let locked = locked.clone();
                let lock_img = lock_img.clone();
                let inner = inner.clone();
                let btn = btn.clone();
                let default_tip = spec.desc.to_string();
                Rc::new(move |is_locked: bool| {
                    locked.set(is_locked);
                    lock_img.set_visible(is_locked);
                    inner.set_opacity(if is_locked { 0.5 } else { 1.0 });
                    if is_locked {
                        btn.set_tooltip_text(Some(crate::tr_en!("Sign in to access")));
                    } else {
                        btn.set_tooltip_text(Some(&default_tip));
                    }
                })
            };
            // Start locked (the shell is signed out at build time).
            locker(true);

            (overlay.upcast(), Some(locker))
        }
    }
}

fn load_app_icon(pixel_size: i32) -> gtk::Image {
    let bytes = include_bytes!("../../assets/verbinal-256.png");
    let gbytes = gtk::glib::Bytes::from_static(bytes);
    let stream = gtk::gio::MemoryInputStream::from_bytes(&gbytes);
    let pixbuf = gtk::gdk_pixbuf::Pixbuf::from_stream(&stream, gtk::gio::Cancellable::NONE);

    match pixbuf {
        Ok(pb) => {
            let scaled = pb
                .scale_simple(
                    pixel_size,
                    pixel_size,
                    gtk::gdk_pixbuf::InterpType::Bilinear,
                )
                .unwrap_or(pb);
            let texture = gtk::gdk::Texture::for_pixbuf(&scaled);
            let image = gtk::Image::from_paintable(Some(&texture));
            image.set_pixel_size(pixel_size);
            image
        }
        Err(_) => {
            let image = gtk::Image::from_icon_name("help-about-symbolic");
            image.set_pixel_size(pixel_size);
            image
        }
    }
}

#[cfg(test)]
mod session_tests {
    //! The shell's sign-in sequence has no runtime coverage — it needs a GTK
    //! main loop and a live CADC session — so these read the source instead.

    const SOURCE: &str = include_str!("main_window.rs");

    #[test]
    fn only_one_place_puts_the_shell_into_its_signed_in_state() {
        // Two paths held their own copy of this sequence and differed already;
        // the detail page's Sign in would have been a third. A second copy is
        // how one of them ends up forgetting to unlock the landing tiles, or to
        // refresh VOSpace, for one way of signing in but not the others.
        // Assembled at runtime so this guard does not count ITSELF — the scan
        // reads the file it lives in, and the first version failed on its own
        // assertion text.
        let needle = format!("set_authenticated({})", true);
        let unlocks = SOURCE.matches(&needle).count();
        assert_eq!(
            unlocks, 1,
            "the signed-in sequence exists in {unlocks} places; it belongs in SignedInChrome::apply alone"
        );
    }

    #[test]
    fn every_way_of_signing_in_goes_through_it() {
        // The login button, the startup auto-login, and the detail page's
        // proprietary-data panel.
        let applies = SOURCE.matches("signed_in.apply(").count();
        assert!(
            applies >= 3,
            "expected all three sign-in paths to apply the shared chrome, found {applies}"
        );
    }

    #[test]
    fn the_detail_pages_sign_in_reloads_the_observation() {
        // Signing in and being left staring at the same "sign in to view" panel
        // is the bug this whole path exists to avoid.
        let handler = SOURCE
            .split("set_on_sign_in(")
            .nth(1)
            .expect("the detail page's sign-in handler is wired here");
        assert!(
            handler.contains("page.reload()"),
            "the sign-in handler does not reload the observation afterwards"
        );
    }
}
