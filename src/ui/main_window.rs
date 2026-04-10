use crate::models::UserInfo;
use crate::services::TokenStorage;
use crate::state::AppServices;
use crate::ui::dashboard::DashboardView;
use crate::ui::file_panel::{FilePanel, FileType};
use crate::ui::fits_viewer::FitsViewer;
use crate::ui::login_dialog::show_login_dialog;
use crate::ui::notebook_host::NotebookTabHost;
use crate::ui::research_page::ResearchPage;
use crate::ui::search_page::SearchPage;
use crate::ui::settings_page::{self, SettingsPage};
use crate::ui::vospace_browser::VoSpaceBrowser;
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
    toast_rx: tokio::sync::mpsc::UnboundedReceiver<crate::services::notification_service::ToastMessage>,
) {
    // Apply saved theme on startup
    let config = services.settings.load();
    settings_page::apply_theme(&config.theme);

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Verbinal - a CANFAR Science Portal")
        .default_width(1200)
        .default_height(800)
        .build();

    let header = adw::HeaderBar::new();

    // --- Left side: Back, Files toggle, About button + status ---
    let back_btn = gtk::Button::from_icon_name("go-previous-symbolic");
    back_btn.set_tooltip_text(Some("Back (Alt+Left)"));
    back_btn.add_css_class("flat");
    back_btn.set_sensitive(false);
    header.pack_start(&back_btn);

    let files_btn = gtk::ToggleButton::new();
    files_btn.set_icon_name("folder-symbolic");
    files_btn.set_tooltip_text(Some("Toggle File Panel (Ctrl+B)"));
    files_btn.add_css_class("flat");
    header.pack_start(&files_btn);

    let about_btn = gtk::Button::from_icon_name("help-about-symbolic");
    about_btn.set_tooltip_text(Some("About"));
    header.pack_start(&about_btn);

    let status_label = gtk::Label::new(None);
    status_label.add_css_class("dim-label");
    status_label.add_css_class("caption");
    header.pack_start(&status_label);

    // --- Service health indicator ---
    let health_icon = gtk::Image::from_icon_name("network-idle-symbolic");
    health_icon.set_pixel_size(16);
    let health_label = gtk::Label::new(Some("Connected"));
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

    let health_btn = gtk::MenuButton::new();
    health_btn.set_child(Some(&health_box));
    health_btn.set_popover(Some(&health_popover));
    health_btn.add_css_class("flat");
    health_btn.set_tooltip_text(Some("Service status"));
    header.pack_start(&health_btn);

    // --- Center: ViewSwitcher ---
    let view_stack = adw::ViewStack::new();
    let switcher = adw::ViewSwitcher::new();
    switcher.set_stack(Some(&view_stack));
    switcher.set_policy(adw::ViewSwitcherPolicy::Wide);
    header.set_title_widget(Some(&switcher));

    // --- Right side: spinner, login, user menu, settings ---
    let spinner = gtk::Spinner::new();
    spinner.set_visible(false);
    header.pack_end(&spinner);

    let settings_btn = gtk::Button::from_icon_name("emblem-system-symbolic");
    settings_btn.set_tooltip_text(Some("Settings"));
    header.pack_end(&settings_btn);

    let login_btn = gtk::Button::with_label("Login");
    login_btn.add_css_class("suggested-action");
    header.pack_end(&login_btn);

    let user_menu_btn = gtk::MenuButton::new();
    user_menu_btn.set_visible(false);
    user_menu_btn.set_tooltip_text(Some("Account"));
    let user_menu = gtk::gio::Menu::new();
    user_menu.append(Some("Profile"), Some("app.profile"));
    user_menu.append(Some("Logout"), Some("app.logout"));
    user_menu_btn.set_menu_model(Some(&user_menu));
    header.pack_end(&user_menu_btn);

    // --- File panel (hidden by default) ---
    let file_panel = FilePanel::new();
    file_panel.widget().set_visible(false);

    // --- Toast Overlay wrapping the ViewStack ---
    let toast_overlay = adw::ToastOverlay::new();
    toast_overlay.set_child(Some(&view_stack));

    // Wire cross-thread toast dispatch: any thread can call services.toast.toast("...")
    {
        let overlay = toast_overlay.clone();
        let mut rx = toast_rx;
        glib::spawn_future_local(async move {
            while let Some(msg) = rx.recv().await {
                let toast = adw::Toast::new(&msg.body);
                toast.set_timeout(msg.timeout);
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
    let banner = adw::Banner::new("Some services unreachable — working with cached data");
    banner.set_button_label(Some("Details"));
    banner.set_revealed(false);

    let toolbar_view = adw::ToolbarView::new();
    toolbar_view.add_top_bar(&header);
    toolbar_view.add_top_bar(&banner);
    toolbar_view.set_content(Some(&paned));

    // Banner "Details" opens the health popover
    {
        let health_btn = health_btn.clone();
        banner.connect_button_clicked(move |_| {
            health_btn.popup();
        });
    }

    window.set_content(Some(&toolbar_view));

    // --- Add pages to ViewStack ---
    // Dashboard (added later when logged in)
    // Settings (always available)
    let settings_page = SettingsPage::new(services.clone());

    // VOSpace browser
    let vospace_browser = VoSpaceBrowser::new(services.clone());

    // FITS viewer
    let fits_viewer = FitsViewer::new(services.clone());

    // Add pages — all 6 modules + settings
    let dashboard_placeholder = build_welcome_page(&view_stack);

    // Search module (real implementation)
    let search_page = SearchPage::new(services.clone());

    // Research module (real implementation)
    let research_page = ResearchPage::new();
    research_page.set_application(app);

    // Notebook module (real implementation)
    let notebook_host = NotebookTabHost::new(services.clone());

    view_stack.add_titled_with_icon(
        &dashboard_placeholder,
        Some("home"),
        "Home",
        "go-home-symbolic",
    );
    view_stack.add_titled_with_icon(
        vospace_browser.widget(),
        Some("storage"),
        "Storage",
        "drive-multidisk-symbolic",
    );
    view_stack.add_titled_with_icon(
        fits_viewer.widget(),
        Some("fits"),
        "FITS Viewer",
        "image-x-generic-symbolic",
    );
    view_stack.add_titled_with_icon(
        search_page.widget(),
        Some("search"),
        "Search",
        "system-search-symbolic",
    );
    view_stack.add_titled_with_icon(
        research_page.widget(),
        Some("research"),
        "Research",
        "document-open-recent-symbolic",
    );
    view_stack.add_titled_with_icon(
        notebook_host.widget(),
        Some("notebook"),
        "Notebook",
        "accessories-text-editor-symbolic",
    );
    view_stack.add_titled_with_icon(
        &settings_page.widget,
        Some("settings"),
        "Settings",
        "emblem-system-symbolic",
    );

    let dashboard: Rc<RefCell<Option<DashboardView>>> = Rc::new(RefCell::new(None));
    let cached_user_info: Rc<RefCell<Option<UserInfo>>> = Rc::new(RefCell::new(None));

    // Settings button navigates to settings page
    {
        let view_stack = view_stack.clone();
        settings_btn.connect_clicked(move |_| {
            view_stack.set_visible_child_name("settings");
        });
    }

    // About action
    {
        let window_clone = window.clone();
        about_btn.connect_clicked(move |_| {
            show_about_dialog(&window_clone);
        });
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

        let logout_action = gtk::gio::SimpleAction::new("logout", None);
        logout_action.connect_activate(move |_, _| {
            let services = services.clone();
            let login_btn = login_btn.clone();
            let user_menu_btn = user_menu_btn.clone();
            let status_label = status_label.clone();
            let view_stack = view_stack.clone();
            let dashboard = dashboard.clone();
            let cached_user_info = cached_user_info.clone();

            glib::spawn_future_local(async move {
                let svc = services.clone();
                services.spawn(async move { svc.clear_auth().await }).await;
                services.notifications.clear();
                login_btn.set_visible(true);
                user_menu_btn.set_visible(false);
                user_menu_btn.set_label("");
                status_label.set_text("");
                *cached_user_info.borrow_mut() = None;

                // Replace dashboard with placeholder
                view_stack.set_visible_child_name("home");
                *dashboard.borrow_mut() = None;

                services.toast.toast("Logged out successfully");
            });
        });
        app.add_action(&logout_action);
    }

    // Login button
    {
        let window_clone = window.clone();
        let services = services.clone();
        let login_btn_clone = login_btn.clone();
        let user_menu_btn = user_menu_btn.clone();
        let status_label = status_label.clone();
        let view_stack = view_stack.clone();
        let dashboard = dashboard.clone();
        let cached_user_info = cached_user_info.clone();
        let vospace = vospace_browser.clone();

        login_btn.connect_clicked(move |_| {
            let window = window_clone.clone();
            let services = services.clone();
            let login_btn = login_btn_clone.clone();
            let user_menu_btn = user_menu_btn.clone();
            let status_label = status_label.clone();
            let view_stack = view_stack.clone();
            let dashboard = dashboard.clone();
            let cached_user_info = cached_user_info.clone();
            let vospace = vospace.clone();

            glib::spawn_future_local(async move {
                if let Some((_username, _token, user_info)) =
                    show_login_dialog(&window, &services).await
                {
                    let display = user_info.display_name();
                    login_btn.set_visible(false);
                    user_menu_btn.set_label(&display);
                    user_menu_btn.set_visible(true);
                    status_label.set_text(&format!("Welcome, {}", &display));
                    *cached_user_info.borrow_mut() = Some(user_info);

                    navigate_to_dashboard(&view_stack, &services, &dashboard).await;
                    vospace.refresh().await;

                    services.toast.toast(&format!("Welcome back, {}!", &display));
                }
            });
        });
    }

    // Try auto-login on startup
    {
        let services = services.clone();
        let login_btn = login_btn.clone();
        let user_menu_btn = user_menu_btn.clone();
        let status_label = status_label.clone();
        let spinner = spinner.clone();
        let view_stack = view_stack.clone();
        let dashboard = dashboard.clone();
        let cached_user_info = cached_user_info.clone();
        let vospace = vospace_browser.clone();

        glib::spawn_future_local(async move {
            if let Some(stored_token) = TokenStorage::get_token() {
                spinner.set_visible(true);
                spinner.start();
                status_label.set_text("Checking authentication...");

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

                        let display = user_info.display_name();
                        login_btn.set_visible(false);
                        user_menu_btn.set_label(&display);
                        user_menu_btn.set_visible(true);
                        status_label.set_text(&format!("Welcome, {}", &display));
                        *cached_user_info.borrow_mut() = Some(user_info);

                        navigate_to_dashboard(&view_stack, &services, &dashboard).await;
                        vospace.refresh().await;

                        services.toast.toast(&format!("Welcome back, {}!", &display));
                    }
                    Err(_) => {
                        TokenStorage::clear();
                        status_label.set_text("Session expired. Please login.");
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
                health_label.set_text("Connected");
                health_icon.remove_css_class("warning");
                health_icon.remove_css_class("error");
                health_icon.add_css_class("success");
            } else {
                health_icon.set_icon_name(Some("dialog-warning-symbolic"));
                health_label.set_text(&format!("{} offline", count));
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
                    .title(&svc_name.to_string())
                    .build();
                row.add_prefix(&gtk::Image::from_icon_name(svc_name.icon_name()));

                let status_lbl = gtk::Label::new(None);
                status_lbl.add_css_class("caption");
                match &status {
                    ServiceStatus::Unknown => {
                        status_lbl.set_text("Unknown");
                        status_lbl.add_css_class("dim-label");
                    }
                    ServiceStatus::Reachable => {
                        status_lbl.set_text("Online");
                        status_lbl.add_css_class("success");
                    }
                    ServiceStatus::Unreachable { since, .. } => {
                        let local: chrono::DateTime<chrono::Local> = (*since).into();
                        row.set_subtitle(&format!("Last seen {}", local.format("%H:%M")));
                        status_lbl.set_text("Offline");
                        status_lbl.add_css_class("error");
                    }
                }
                row.add_suffix(&status_lbl);
                health_list.append(&row);
            }

            glib::ControlFlow::Continue
        });
    }

    // Navigation history (back button)
    let nav_history: Rc<NavHistory> = Rc::new(NavHistory::new(32));
    // Seed with the current page so the first navigation pushes correctly
    if let Some(current) = view_stack.visible_child_name() {
        nav_history.set_current(current.as_str());
    }
    {
        let nav = nav_history.clone();
        let back_btn_clone = back_btn.clone();
        let view_stack_clone = view_stack.clone();
        view_stack.connect_notify_local(Some("visible-child-name"), move |vs, _| {
            if nav.is_suppressed() {
                return;
            }
            if let Some(name) = vs.visible_child_name() {
                nav.push(name.as_str());
                back_btn_clone.set_sensitive(nav.can_go_back());
            }
            let _ = &view_stack_clone; // keep clone alive inside closure
        });
    }
    {
        let nav = nav_history.clone();
        let back_btn_clone = back_btn.clone();
        let view_stack_clone = view_stack.clone();
        back_btn.connect_clicked(move |_| {
            if let Some(prev) = nav.go_back() {
                nav.suppress(true);
                view_stack_clone.set_visible_child_name(&prev);
                nav.suppress(false);
                back_btn_clone.set_sensitive(nav.can_go_back());
            }
        });
    }

    // Keyboard shortcuts
    setup_keyboard_shortcuts(
        &window,
        &view_stack,
        &file_panel,
        &files_btn,
        &notebook_host,
        &back_btn,
    );

    window.present();
}

// ---------------------------------------------------------------------------
// Navigation history
// ---------------------------------------------------------------------------

struct NavHistory {
    stack: RefCell<Vec<String>>,
    current: RefCell<Option<String>>,
    suppressed: RefCell<bool>,
    max_len: usize,
}

impl NavHistory {
    fn new(max_len: usize) -> Self {
        Self {
            stack: RefCell::new(Vec::new()),
            current: RefCell::new(None),
            suppressed: RefCell::new(false),
            max_len,
        }
    }

    fn set_current(&self, page: &str) {
        *self.current.borrow_mut() = Some(page.to_string());
    }

    fn push(&self, new_page: &str) {
        let mut current_slot = self.current.borrow_mut();
        if let Some(prev) = current_slot.take() {
            if prev != new_page {
                let mut stack = self.stack.borrow_mut();
                stack.push(prev);
                // Cap history size
                if stack.len() > self.max_len {
                    stack.remove(0);
                }
            }
        }
        *current_slot = Some(new_page.to_string());
    }

    fn go_back(&self) -> Option<String> {
        let mut stack = self.stack.borrow_mut();
        let prev = stack.pop()?;
        *self.current.borrow_mut() = Some(prev.clone());
        Some(prev)
    }

    fn can_go_back(&self) -> bool {
        !self.stack.borrow().is_empty()
    }

    fn suppress(&self, value: bool) {
        *self.suppressed.borrow_mut() = value;
    }

    fn is_suppressed(&self) -> bool {
        *self.suppressed.borrow()
    }
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
    back_btn: &gtk::Button,
) {
    let controller = gtk::EventControllerKey::new();
    let vs = view_stack.clone();
    let fp = Rc::clone(file_panel);
    let fb = files_btn.clone();
    let nh = notebook_host.clone();
    let bb = back_btn.clone();
    controller.connect_key_pressed(move |_, key, _code, modifier| {
        let ctrl = modifier.contains(gtk4::gdk::ModifierType::CONTROL_MASK);
        let shift = modifier.contains(gtk4::gdk::ModifierType::SHIFT_MASK);
        let alt = modifier.contains(gtk4::gdk::ModifierType::ALT_MASK);
        let on_notebook = vs.visible_child_name().as_deref() == Some("notebook");

        // Alt+Left → back
        if alt && key == gtk4::gdk::Key::Left {
            if bb.is_sensitive() {
                bb.emit_clicked();
            }
            return gtk::glib::Propagation::Stop;
        }

        if ctrl {
            match key {
                gtk4::gdk::Key::comma => {
                    vs.set_visible_child_name("settings");
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
                    vs.set_visible_child_name("settings");
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
        .title("User Profile")
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
                .title("Email")
                .subtitle(email)
                .build();
            row.add_prefix(&gtk::Image::from_icon_name("mail-unread-symbolic"));
            group.add(&row);
        }
    }

    if let Some(ref institute) = info.institute {
        if !institute.is_empty() {
            let row = adw::ActionRow::builder()
                .title("Institute")
                .subtitle(institute)
                .build();
            row.add_prefix(&gtk::Image::from_icon_name("building-symbolic"));
            group.add(&row);
        }
    }

    if let Some(ref id) = info.internal_id {
        if !id.is_empty() {
            let row = adw::ActionRow::builder()
                .title("Internal ID")
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
        "Dashboard",
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
        .comments("A CANFAR Science Portal Companion\n\nLaunch, monitor, and manage your interactive computing sessions (Notebook, Desktop, CARTA, Firefly) directly from your desktop without needing a browser.\n\nCANFAR is operated by the Canadian Astronomy Data Centre (CADC) and the Digital Research Alliance of Canada.")
        .website("https://www.canfar.net")
        .license_type(gtk::License::Agpl30)
        .copyright("\u{00a9} 2025 Serhii Zautkin")
        .developers(vec!["Serhii Zautkin"])
        .transient_for(window)
        .modal(true)
        .build();

    dialog.add_legal_section(
        "Runtime Info",
        None,
        gtk::License::Custom,
        Some(&format!(
            "Runtime: Rust {}\nPlatform: {}\nFramework: GTK4 + libadwaita",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
        )),
    );

    dialog.present();
}

// ---------------------------------------------------------------------------
// Welcome page with feature tiles
// ---------------------------------------------------------------------------

fn build_welcome_page(view_stack: &adw::ViewStack) -> gtk::Box {
    let page = gtk::Box::new(gtk::Orientation::Vertical, 0);
    page.set_vexpand(true);

    let scrolled = gtk::ScrolledWindow::new();
    scrolled.set_vexpand(true);

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

    let subtitle = gtk::Label::new(Some("A CANFAR Science Portal Companion"));
    subtitle.add_css_class("dim-label");
    subtitle.set_halign(gtk::Align::Center);
    header_box.append(&subtitle);

    let version_label = gtk::Label::new(Some(&format!("v{}", env!("CARGO_PKG_VERSION"))));
    version_label.add_css_class("dim-label");
    version_label.add_css_class("caption");
    version_label.set_halign(gtk::Align::Center);
    header_box.append(&version_label);

    content.append(&header_box);

    // Feature tiles in a 3x2 grid (matching Windows 6-tile layout)
    let grid = gtk::Grid::new();
    grid.set_row_spacing(16);
    grid.set_column_spacing(16);
    grid.set_row_homogeneous(true);
    grid.set_column_homogeneous(true);
    grid.set_halign(gtk::Align::Center);

    // Row 1: Portal, Search, Research
    grid.attach(
        &feature_tile(
            view_stack,
            "computer-symbolic",
            "Portal",
            "Manage sessions & data",
            "home",
        ),
        0,
        0,
        1,
        1,
    );
    grid.attach(
        &feature_tile(
            view_stack,
            "system-search-symbolic",
            "Search",
            "Explore the CADC archive",
            "search",
        ),
        1,
        0,
        1,
        1,
    );
    grid.attach(
        &feature_tile(
            view_stack,
            "document-open-recent-symbolic",
            "Research",
            "Downloaded observations",
            "research",
        ),
        2,
        0,
        1,
        1,
    );

    // Row 2: Storage, Notebook, FITS Viewer
    grid.attach(
        &feature_tile(
            view_stack,
            "drive-multidisk-symbolic",
            "Storage",
            "Browse VOSpace files",
            "storage",
        ),
        0,
        1,
        1,
        1,
    );
    grid.attach(
        &feature_tile(
            view_stack,
            "accessories-text-editor-symbolic",
            "Notebook",
            "Open & run .ipynb files",
            "notebook",
        ),
        1,
        1,
        1,
        1,
    );
    grid.attach(
        &feature_tile(
            view_stack,
            "image-x-generic-symbolic",
            "FITS Viewer",
            "View astronomical images",
            "fits",
        ),
        2,
        1,
        1,
        1,
    );

    content.append(&grid);

    // Login prompt
    let login_prompt = gtk::Label::new(Some("Log in with your CADC credentials to get started"));
    login_prompt.add_css_class("dim-label");
    login_prompt.set_halign(gtk::Align::Center);
    login_prompt.set_margin_top(8);
    content.append(&login_prompt);

    scrolled.set_child(Some(&content));
    page.append(&scrolled);
    page
}

fn feature_tile(
    view_stack: &adw::ViewStack,
    icon_name: &str,
    title: &str,
    description: &str,
    target_page: &str,
) -> gtk::Button {
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

    let icon = gtk::Image::from_icon_name(icon_name);
    icon.set_pixel_size(48);
    icon.set_halign(gtk::Align::Center);
    inner.append(&icon);

    let label = gtk::Label::new(Some(title));
    label.add_css_class("title-3");
    label.set_halign(gtk::Align::Center);
    inner.append(&label);

    let desc = gtk::Label::new(Some(description));
    desc.add_css_class("dim-label");
    desc.add_css_class("caption");
    desc.set_halign(gtk::Align::Center);
    desc.set_justify(gtk::Justification::Center);
    desc.set_wrap(true);
    desc.set_max_width_chars(22);
    inner.append(&desc);

    btn.set_child(Some(&inner));

    // Navigate to module on click
    let vs = view_stack.clone();
    let target = target_page.to_string();
    btn.connect_clicked(move |_| {
        vs.set_visible_child_name(&target);
    });

    btn
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
