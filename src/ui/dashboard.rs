use crate::models::SessionLaunchParams;
use crate::state::AppServices;
use crate::ui::batch_jobs_dialog::show_batch_jobs_dialog;
use crate::ui::batch_jobs_view::BatchJobsView;
use crate::ui::canfar_images::CanfarImagesView;
use crate::ui::delete_dialog::show_delete_dialog;
use crate::ui::launch_dialog::show_launch_dialog;
use crate::ui::launch_form::LaunchFormView;
use crate::ui::platform_load::PlatformLoadView;
use crate::ui::recent_launches::RecentLaunchesView;
use crate::ui::session_card::SessionAction;
use crate::ui::session_events_dialog::show_events_dialog;
use crate::ui::session_list::SessionListView;
use crate::ui::storage_quota::StorageQuotaView;
use crate::ui::template_manager::TemplateManager;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use libadwaita as adw;
use libadwaita::prelude::*;
use std::rc::Rc;
use std::sync::Arc;

const MAX_SESSIONS: usize = 3;

pub struct DashboardView {
    container: gtk::Box,
    session_list: Rc<SessionListView>,
    launch_form: Rc<LaunchFormView>,
    platform_load: Rc<PlatformLoadView>,
    storage_quota: Rc<StorageQuotaView>,
    recent_launches: Rc<RecentLaunchesView>,
    template_manager: Rc<TemplateManager>,
    batch_jobs: Rc<BatchJobsView>,
    canfar_images: Rc<CanfarImagesView>,
    services: Arc<AppServices>,
}

impl DashboardView {
    pub fn new(services: Arc<AppServices>) -> Self {
        let container = gtk::Box::new(gtk::Orientation::Vertical, 0);
        container.set_vexpand(true);

        // Main grid: 2x2 layout
        let grid = gtk::Grid::new();
        grid.set_row_homogeneous(false);
        grid.set_column_homogeneous(true);
        grid.set_vexpand(true);
        grid.set_row_spacing(12);
        grid.set_column_spacing(12);

        // Top-left: Sessions
        let session_list = SessionListView::new(services.clone());

        // Top-right: Storage
        let storage_quota = StorageQuotaView::new(services.clone());

        // Bottom-left: Launch form
        let launch_form = LaunchFormView::new(services.clone(), session_list.sessions_ref());

        // Bottom-right: Batch Jobs + Recent Launches + Platform Load + Template Manager
        let right_bottom = gtk::Box::new(gtk::Orientation::Vertical, 12);
        let batch_jobs = BatchJobsView::new(services.clone());
        let recent_launches = RecentLaunchesView::new(services.clone());
        let platform_load = PlatformLoadView::new(services.clone());
        let template_manager = TemplateManager::new(services.clone());

        right_bottom.append(batch_jobs.widget());
        right_bottom.append(recent_launches.widget());
        right_bottom.append(platform_load.widget());
        right_bottom.append(template_manager.widget());

        grid.attach(session_list.widget(), 0, 0, 1, 1);
        grid.attach(storage_quota.widget(), 1, 0, 1, 1);
        grid.attach(launch_form.widget(), 0, 1, 1, 1);
        grid.attach(&right_bottom, 1, 1, 1, 1);

        // Full-width CANFAR Images widget below the 2×2 grid.
        let canfar_images = CanfarImagesView::new(services.clone());

        // Wrap the grid + images card in a scroller so the page never clips the
        // full-width card on a short window.
        let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
        content.set_margin_start(12);
        content.set_margin_end(12);
        content.set_margin_top(12);
        content.set_margin_bottom(12);
        content.append(&grid);
        content.append(canfar_images.widget());

        let scrolled = gtk::ScrolledWindow::new();
        scrolled.set_vscrollbar_policy(gtk::PolicyType::Automatic);
        scrolled.set_hscrollbar_policy(gtk::PolicyType::Never);
        scrolled.set_vexpand(true);
        scrolled.set_child(Some(&content));
        container.append(&scrolled);

        let dashboard = DashboardView {
            container,
            session_list,
            launch_form,
            platform_load,
            storage_quota,
            recent_launches,
            template_manager,
            batch_jobs,
            canfar_images,
            services,
        };

        dashboard.setup_callbacks();
        dashboard
    }

    fn setup_callbacks(&self) {
        // Session actions (open, delete, renew, events)
        let services = self.services.clone();
        let session_list = self.session_list.clone();
        let launch_form = self.launch_form.clone();
        let recent_launches = self.recent_launches.clone();
        let template_manager = self.template_manager.clone();

        self.session_list.set_on_action(move |action| {
            let services = services.clone();
            let session_list = session_list.clone();
            let launch_form = launch_form.clone();
            let recent_launches = recent_launches.clone();

            glib::spawn_future_local(async move {
                match action {
                    SessionAction::Open(url) => {
                        let _ = open::that(&url);
                    }
                    SessionAction::Delete(id, name) => {
                        let widget = session_list.widget();
                        if show_delete_dialog(widget, &name).await {
                            let svc = services.clone();
                            let id_c = id.clone();
                            let result = services
                                .spawn(async move {
                                    let token = svc.get_token().await;
                                    let Some(token) = token else {
                                        return Err("No token".to_string());
                                    };
                                    svc.sessions.delete_session(&token, &id_c).await
                                })
                                .await;
                            match result {
                                Ok(()) => {
                                    glib::timeout_future_seconds(3).await;
                                    session_list.refresh().await;
                                    update_session_limits(
                                        &session_list,
                                        &launch_form,
                                        &recent_launches,
                                    );
                                }
                                Err(e) => eprintln!("Delete failed: {}", e),
                            }
                        }
                    }
                    SessionAction::Renew(id, name) => {
                        // Progress + result dialog (mirrors ShowRenewDialogAsync):
                        // a spinner while renewing, then a success/error message.
                        let content = gtk::Box::new(gtk::Orientation::Horizontal, 12);
                        let spinner = gtk::Spinner::new();
                        spinner.start();
                        let status =
                            gtk::Label::new(Some(&crate::tr_fmt!("Renewing session '{}'…", name)));
                        status.set_wrap(true);
                        status.set_xalign(0.0);
                        content.append(&spinner);
                        content.append(&status);

                        let dialog = adw::MessageDialog::new(
                            session_list
                                .widget()
                                .root()
                                .and_downcast::<gtk::Window>()
                                .as_ref(),
                            Some(crate::tr_en!("Renew Session")),
                            None,
                        );
                        dialog.set_extra_child(Some(&content));
                        dialog.add_response("close", crate::tr_en!("Close"));
                        dialog.set_close_response("close");
                        dialog.present();

                        let svc = services.clone();
                        let id_c = id.clone();
                        let result = services
                            .spawn(async move {
                                let token = svc.get_token().await;
                                let Some(token) = token else {
                                    return Err("No token".to_string());
                                };
                                svc.sessions.renew_session(&token, &id_c).await
                            })
                            .await;

                        spinner.stop();
                        spinner.set_visible(false);

                        match result {
                            Ok(()) => {
                                dialog.set_heading(Some(crate::tr_en!("Session renewed")));
                                status.set_text(&crate::tr_fmt!(
                                    "'{}' renewed. Its expiry has been extended.",
                                    name
                                ));
                                session_list.refresh().await;
                                update_session_limits(
                                    &session_list,
                                    &launch_form,
                                    &recent_launches,
                                );
                                // Auto-close shortly after success (reference waits 2s).
                                let dialog_ref = dialog.downgrade();
                                glib::spawn_future_local(async move {
                                    glib::timeout_future_seconds(2).await;
                                    if let Some(d) = dialog_ref.upgrade() {
                                        d.close();
                                    }
                                });
                            }
                            Err(e) => {
                                dialog.set_heading(Some(crate::tr_en!("Renew failed")));
                                status.set_text(&crate::tr_fmt!("Renew failed: {}", e));
                            }
                        }
                    }
                    SessionAction::Events(id, name) => {
                        let svc = services.clone();
                        let id_c = id.clone();
                        let result = services
                            .spawn(async move {
                                let token = svc.get_token().await;
                                let Some(token) = token else {
                                    return ("No auth".to_string(), "No auth".to_string());
                                };
                                let events = svc
                                    .sessions
                                    .get_events(&token, &id_c)
                                    .await
                                    .unwrap_or_else(|e| crate::tr_fmt!("Error: {}", e));
                                let logs = svc
                                    .sessions
                                    .get_logs(&token, &id_c)
                                    .await
                                    .unwrap_or_else(|e| crate::tr_fmt!("Error: {}", e));
                                (events, logs)
                            })
                            .await;
                        let widget = session_list.widget();
                        show_events_dialog(widget, &name, &result.0, &result.1).await;
                    }
                }
            });
        });

        // Session count changes -> update limits
        {
            let launch_form = self.launch_form.clone();
            let recent_launches = self.recent_launches.clone();
            self.session_list.set_on_sessions_changed(move |count| {
                let reached = count >= MAX_SESSIONS;
                launch_form.set_session_limit_reached(reached);
                recent_launches.set_session_limit_reached(reached);
            });
        }

        // Launch completed -> refresh
        {
            let session_list = self.session_list.clone();
            let recent_launches = self.recent_launches.clone();
            self.launch_form.set_on_launched(move || {
                let session_list = session_list.clone();
                let recent_launches = recent_launches.clone();
                glib::spawn_future_local(async move {
                    glib::timeout_future_seconds(2).await;
                    session_list.refresh().await;
                    recent_launches.refresh();
                });
            });
        }

        // Relaunch from recent
        {
            let services = self.services.clone();
            let session_list = self.session_list.clone();
            let recent_launches_ref = self.recent_launches.clone();
            self.recent_launches.set_on_relaunch(move |launch| {
                let services = services.clone();
                let session_list = session_list.clone();
                let recent_launches_ref = recent_launches_ref.clone();

                glib::spawn_future_local(async move {
                    // Faithful relaunch (mirrors RelaunchAsync): reuse the recorded
                    // session name and reproduce the saved configuration. A
                    // flexible record zeroes cores/ram/gpus (platform-managed);
                    // a headless record replays cmd/args/replicas and routes
                    // through the headless launch (its session_type drives the
                    // batch-job form fields), never the interactive path.
                    let name = launch.name.clone();
                    let params = launch.to_launch_params(name.clone());

                    let svc = services.clone();
                    let params_clone = params.clone();
                    let result = services
                        .spawn(async move {
                            let token = svc.get_token().await;
                            let Some(token) = token else {
                                return Err("No token".to_string());
                            };
                            svc.sessions.launch_session(&token, &params_clone).await
                        })
                        .await;

                    let image_display = match params.image.rsplit_once('/') {
                        Some((_, tail)) => tail.to_string(),
                        None => params.image.clone(),
                    };

                    show_launch_dialog(
                        recent_launches_ref.widget(),
                        &name,
                        &image_display,
                        &params.session_type,
                        params.cores,
                        params.ram,
                        params.gpus,
                        result.clone(),
                    )
                    .await;

                    if result.is_ok() {
                        glib::timeout_future_seconds(2).await;
                        session_list.refresh().await;
                        recent_launches_ref.refresh();
                    }
                });
            });
        }

        // Launch from template
        {
            let services = self.services.clone();
            let session_list = self.session_list.clone();
            let template_manager_ref = template_manager.clone();
            template_manager.set_on_launch(move |template| {
                let services = services.clone();
                let session_list = session_list.clone();
                let template_manager_ref = template_manager_ref.clone();

                glib::spawn_future_local(async move {
                    let type_count = session_list.session_count_by_type(&template.session_type);
                    let name = format!("{}{}", template.session_type, type_count + 1);
                    let params = SessionLaunchParams {
                        name: name.clone(),
                        image: template.image.clone(),
                        session_type: template.session_type.clone(),
                        cores: template.cores,
                        ram: template.ram,
                        gpus: template.gpus,
                        cmd: None,
                        env: None,
                        registry_username: None,
                        registry_secret: None,
                        args: None,
                        replicas: None,
                    };

                    let svc = services.clone();
                    let result = services
                        .spawn(async move {
                            let token = svc.get_token().await;
                            let Some(token) = token else {
                                return Err("No token".to_string());
                            };
                            svc.sessions.launch_session(&token, &params).await
                        })
                        .await;

                    let image_display = match template.image.rsplit_once('/') {
                        Some((_, tail)) => tail.to_string(),
                        None => template.image.clone(),
                    };

                    show_launch_dialog(
                        template_manager_ref.widget(),
                        &name,
                        &image_display,
                        &template.session_type,
                        template.cores,
                        template.ram,
                        template.gpus,
                        result.clone(),
                    )
                    .await;

                    if result.is_ok() {
                        glib::timeout_future_seconds(2).await;
                        session_list.refresh().await;
                    }
                });
            });
        }

        // Batch Jobs — state-click opens the drill-down dialog
        {
            let services = self.services.clone();
            let batch_jobs = self.batch_jobs.clone();
            self.batch_jobs.set_on_state_click(move |state, jobs| {
                let services = services.clone();
                let parent = batch_jobs.widget().clone();
                glib::spawn_future_local(async move {
                    show_batch_jobs_dialog(&parent, services, jobs, state).await;
                });
            });
        }

        // Canfar Images — "Use this image" selects it in the launch form
        // (mirrors DashboardPage.OnUseImageRequested → SelectImageById).
        //
        // Straight to the form, which this same dashboard owns and shows two
        // cards away. It used to activate a `use-launch-image` app action that
        // NOBODY REGISTERED, so both buttons that reach here — the one on each
        // image row, and the one in the find-by-package dialog when that dialog
        // is opened from this card — did nothing whatsoever. GIO logs an
        // unregistered activation and carries on, so there was not even an
        // error to notice.
        {
            let launch_form = self.launch_form.clone();
            let services = self.services.clone();
            self.canfar_images.set_on_use_image(move |image_id| {
                if !launch_form.select_image_by_id(&image_id) {
                    // It went to the Advanced tab's custom-image field instead.
                    // Say so: the Standard tab is where the eye goes, and a
                    // selection that landed elsewhere looks like nothing
                    // happened.
                    services.toast.toast(crate::tr_fmt!(
                        "{} is not offered for a session type — put it in the Advanced tab",
                        image_id
                    ));
                }
            });
        }
    }

    pub async fn load_data(&self) {
        self.session_list.refresh().await;
        self.storage_quota.refresh().await;
        self.platform_load.refresh().await;
        self.launch_form.load_images().await;
        self.batch_jobs.refresh().await;
        self.canfar_images.refresh().await;

        self.recent_launches.refresh();

        update_session_limits(&self.session_list, &self.launch_form, &self.recent_launches);
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.container
    }
}

fn update_session_limits(
    session_list: &Rc<SessionListView>,
    launch_form: &Rc<LaunchFormView>,
    recent_launches: &Rc<RecentLaunchesView>,
) {
    let count = session_list.session_count();
    let reached = count >= MAX_SESSIONS;
    launch_form.set_session_limit_reached(reached);
    recent_launches.set_session_limit_reached(reached);
}

#[cfg(test)]
mod use_image_tests {
    const SOURCE: &str = include_str!("dashboard.rs");

    #[test]
    fn use_this_image_reaches_the_launch_form() {
        // It used to activate a `use-launch-image` app action that nobody
        // registered, so every button routing here — the one on each image row,
        // and the one in the find-by-package dialog when that dialog is opened
        // from this card — did nothing at all. The dashboard owns the launch
        // form two cards away; there was never a reason to go out through GIO
        // and back.
        let code = crate::testing::code(SOURCE);
        let at = code
            .find("set_on_use_image")
            .expect("nothing handles Use this image");
        let handler = &code[at..(at + 900).min(code.len())];
        assert!(
            handler.contains("select_image_by_id"),
            "Use this image no longer selects anything in the launch form"
        );
        assert!(
            !handler.contains("activate_action"),
            "the request is going back out through an app action"
        );
    }
}
