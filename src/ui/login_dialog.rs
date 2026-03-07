use crate::models::UserInfo;
use crate::services::TokenStorage;
use crate::state::AppServices;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

/// Returns (username, token, user_info) on success.
pub async fn show_login_dialog(
    parent: &adw::ApplicationWindow,
    services: &Arc<AppServices>,
) -> Option<(String, String, UserInfo)> {
    let dialog = adw::Window::builder()
        .title("Login to CANFAR")
        .default_width(400)
        .default_height(380)
        .modal(true)
        .transient_for(parent)
        .build();

    let toolbar_view = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    toolbar_view.add_top_bar(&header);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 16);
    content.set_margin_start(24);
    content.set_margin_end(24);
    content.set_margin_top(16);
    content.set_margin_bottom(24);

    let title = gtk::Label::new(Some("Sign in with your CADC credentials"));
    title.add_css_class("title-4");
    content.append(&title);

    let username_row = adw::EntryRow::builder().title("Username").build();

    let password_row = adw::PasswordEntryRow::builder().title("Password").build();

    let prefs_group = adw::PreferencesGroup::new();
    prefs_group.add(&username_row);
    prefs_group.add(&password_row);
    content.append(&prefs_group);

    let remember_check = gtk::CheckButton::with_label("Remember me");
    remember_check.set_active(true);
    content.append(&remember_check);

    let error_label = gtk::Label::new(None);
    error_label.add_css_class("error");
    error_label.set_visible(false);
    error_label.set_wrap(true);
    content.append(&error_label);

    let progress = gtk::ProgressBar::new();
    progress.set_visible(false);
    content.append(&progress);

    let button_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    button_box.set_halign(gtk::Align::End);

    let cancel_btn = gtk::Button::with_label("Cancel");
    let login_btn = gtk::Button::with_label("Login");
    login_btn.add_css_class("suggested-action");

    button_box.append(&cancel_btn);
    button_box.append(&login_btn);
    content.append(&button_box);

    toolbar_view.set_content(Some(&content));
    dialog.set_content(Some(&toolbar_view));

    let result: Rc<RefCell<Option<(String, String, UserInfo)>>> = Rc::new(RefCell::new(None));

    {
        let dialog = dialog.clone();
        cancel_btn.connect_clicked(move |_| {
            dialog.close();
        });
    }

    {
        let dialog_c = dialog.clone();
        let services_c = services.clone();
        let result_c = result.clone();
        let username_row_c = username_row.clone();
        let password_row_c = password_row.clone();
        let remember_check_c = remember_check.clone();
        let error_label_c = error_label.clone();
        let progress_c = progress.clone();
        let login_btn_c = login_btn.clone();

        let make_login_fn = move || {
            let dialog = dialog_c.clone();
            let services = services_c.clone();
            let result = result_c.clone();
            let username_row = username_row_c.clone();
            let password_row = password_row_c.clone();
            let remember_check = remember_check_c.clone();
            let error_label = error_label_c.clone();
            let progress = progress_c.clone();
            let login_btn = login_btn_c.clone();

            glib::spawn_future_local(async move {
                let username = username_row.text().to_string();
                let password = password_row.text().to_string();

                if username.is_empty() || password.is_empty() {
                    error_label.set_text("Please enter username and password");
                    error_label.set_visible(true);
                    return;
                }

                login_btn.set_sensitive(false);
                error_label.set_visible(false);
                progress.set_visible(true);
                progress.pulse();

                // Run HTTP on tokio runtime, await result on glib loop
                let auth_result = {
                    let svc = services.clone();
                    let u = username.clone();
                    let p = password.clone();
                    services
                        .spawn(async move { svc.auth.login(&u, &p).await })
                        .await
                };

                if auth_result.success {
                    if let Some(ref token) = auth_result.token {
                        let remember = remember_check.is_active();
                        if remember {
                            let _ = TokenStorage::save_token(token);
                            let _ = TokenStorage::save_username(&username);
                        }
                        let tok = token.clone();
                        let user = username.clone();
                        let svc = services.clone();
                        services
                            .spawn(async move {
                                svc.set_auth(tok, user).await;
                            })
                            .await;

                        // Fetch user profile info
                        let svc = services.clone();
                        let tok = token.clone();
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

                        *result.borrow_mut() = Some((username, token.clone(), user_info));
                        dialog.close();
                    }
                } else {
                    let msg = auth_result
                        .error
                        .unwrap_or_else(|| "Login failed".to_string());
                    error_label.set_text(&msg);
                    error_label.set_visible(true);
                }

                progress.set_visible(false);
                login_btn.set_sensitive(true);
            });
        };

        let login_fn_1 = make_login_fn.clone();
        login_btn.connect_clicked(move |_| {
            login_fn_1();
        });

        let login_fn_2 = make_login_fn;
        password_row.connect_apply(move |_| {
            login_fn_2();
        });
    }

    let (sender, receiver) = tokio::sync::oneshot::channel::<()>();
    let sender = Rc::new(RefCell::new(Some(sender)));

    dialog.connect_close_request(move |_| {
        if let Some(s) = sender.borrow_mut().take() {
            let _ = s.send(());
        }
        glib::Propagation::Proceed
    });

    dialog.present();
    username_row.grab_focus();

    let _ = receiver.await;
    let val = result.borrow().clone();
    val
}
