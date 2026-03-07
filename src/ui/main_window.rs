use crate::models::UserInfo;
use crate::services::TokenStorage;
use crate::state::AppServices;
use crate::ui::dashboard::DashboardView;
use crate::ui::login_dialog::show_login_dialog;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

pub fn build_main_window(app: &adw::Application, services: Arc<AppServices>) {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Verbinal - a CANFAR Science Portal")
        .default_width(1200)
        .default_height(800)
        .build();

    let header = adw::HeaderBar::new();
    header.set_show_title(true);

    let title_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    title_box.set_valign(gtk::Align::Center);

    let app_icon = load_app_icon(24);
    title_box.append(&app_icon);

    let title_label = gtk::Label::new(Some("Verbinal - a CANFAR Science Portal"));
    title_label.add_css_class("title");
    title_box.append(&title_label);
    header.set_title_widget(Some(&title_box));

    let about_btn = gtk::Button::from_icon_name("help-about-symbolic");
    about_btn.set_tooltip_text(Some("About"));
    header.pack_start(&about_btn);

    let status_label = gtk::Label::new(None);
    status_label.add_css_class("dim-label");
    status_label.add_css_class("caption");
    header.pack_start(&status_label);

    let spinner = gtk::Spinner::new();
    spinner.set_visible(false);
    header.pack_end(&spinner);

    // Login button — visible when not authenticated
    let login_btn = gtk::Button::with_label("Login");
    login_btn.add_css_class("suggested-action");
    header.pack_end(&login_btn);

    // User menu button with Profile / Logout items
    let user_menu_btn = gtk::MenuButton::new();
    user_menu_btn.set_visible(false);
    user_menu_btn.set_tooltip_text(Some("Account"));
    let user_menu = gtk::gio::Menu::new();
    user_menu.append(Some("Profile"), Some("app.profile"));
    user_menu.append(Some("Logout"), Some("app.logout"));
    user_menu_btn.set_menu_model(Some(&user_menu));
    header.pack_end(&user_menu_btn);

    let content_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let toolbar_view = adw::ToolbarView::new();
    toolbar_view.add_top_bar(&header);
    toolbar_view.set_content(Some(&content_box));

    window.set_content(Some(&toolbar_view));

    let dashboard: Rc<RefCell<Option<DashboardView>>> = Rc::new(RefCell::new(None));
    let cached_user_info: Rc<RefCell<Option<UserInfo>>> = Rc::new(RefCell::new(None));

    // About action
    {
        let window_clone = window.clone();
        about_btn.connect_clicked(move |_| {
            show_about_dialog(&window_clone);
        });
    }

    // Profile action — show user info dialog
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

    // Logout action
    {
        let services = services.clone();
        let login_btn = login_btn.clone();
        let user_menu_btn = user_menu_btn.clone();
        let status_label = status_label.clone();
        let content_box = content_box.clone();
        let dashboard = dashboard.clone();
        let cached_user_info = cached_user_info.clone();

        let logout_action = gtk::gio::SimpleAction::new("logout", None);
        logout_action.connect_activate(move |_, _| {
            let services = services.clone();
            let login_btn = login_btn.clone();
            let user_menu_btn = user_menu_btn.clone();
            let status_label = status_label.clone();
            let content_box = content_box.clone();
            let dashboard = dashboard.clone();
            let cached_user_info = cached_user_info.clone();

            glib::spawn_future_local(async move {
                let svc = services.clone();
                services.spawn(async move { svc.clear_auth().await }).await;
                login_btn.set_visible(true);
                user_menu_btn.set_visible(false);
                user_menu_btn.set_label("");
                status_label.set_text("");
                *cached_user_info.borrow_mut() = None;
                if let Some(child) = content_box.first_child() {
                    content_box.remove(&child);
                }
                *dashboard.borrow_mut() = None;
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
        let content_box = content_box.clone();
        let dashboard = dashboard.clone();
        let cached_user_info = cached_user_info.clone();

        login_btn.connect_clicked(move |_| {
            let window = window_clone.clone();
            let services = services.clone();
            let login_btn = login_btn_clone.clone();
            let user_menu_btn = user_menu_btn.clone();
            let status_label = status_label.clone();
            let content_box = content_box.clone();
            let dashboard = dashboard.clone();
            let cached_user_info = cached_user_info.clone();

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

                    navigate_to_dashboard(&content_box, &services, &dashboard).await;
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
        let content_box = content_box.clone();
        let dashboard = dashboard.clone();
        let cached_user_info = cached_user_info.clone();

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

                        // Fetch user profile
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

                        navigate_to_dashboard(&content_box, &services, &dashboard).await;
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

    window.present();
}

// ---------------------------------------------------------------------------
// Profile dialog (overlay)
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

    // Avatar
    let avatar = adw::Avatar::new(64, Some(&info.display_name()), true);
    avatar.set_halign(gtk::Align::Center);
    avatar.set_margin_bottom(8);
    content.append(&avatar);

    // Display name
    let name_label = gtk::Label::new(Some(&info.display_name()));
    name_label.add_css_class("title-3");
    name_label.set_halign(gtk::Align::Center);
    content.append(&name_label);

    // Username
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

    // Details group
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
// Navigation & About
// ---------------------------------------------------------------------------

async fn navigate_to_dashboard(
    content_box: &gtk::Box,
    services: &Arc<AppServices>,
    dashboard: &Rc<RefCell<Option<DashboardView>>>,
) {
    while let Some(child) = content_box.first_child() {
        content_box.remove(&child);
    }

    let view = DashboardView::new(services.clone());
    content_box.append(view.widget());
    view.load_data().await;
    *dashboard.borrow_mut() = Some(view);
}

fn show_about_dialog(window: &adw::ApplicationWindow) {
    let dialog = adw::AboutWindow::builder()
        .application_name("Verbinal")
        .application_icon("net.canfar.Verbinal")
        .version("1.0.0")
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
