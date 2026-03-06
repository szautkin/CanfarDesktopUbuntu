use crate::log;
use crate::models::SessionLaunchParams;
use crate::state::AppServices;
use crate::ui::delete_dialog::show_delete_dialog;
use crate::ui::launch_form::LaunchFormView;
use crate::ui::platform_load::PlatformLoadView;
use crate::ui::recent_launches::RecentLaunchesView;
use crate::ui::session_card::SessionAction;
use crate::ui::session_events_dialog::show_events_dialog;
use crate::ui::session_list::SessionListView;
use crate::ui::storage_quota::StorageQuotaView;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{self as gtk};
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

        // Top-left: Sessions
        let session_list = SessionListView::new(services.clone());

        // Top-right: Storage
        let storage_quota = StorageQuotaView::new(services.clone());

        // Bottom-left: Launch form
        let launch_form = LaunchFormView::new(services.clone());

        // Bottom-right: Recent Launches + Platform Load
        let right_bottom = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let recent_launches = RecentLaunchesView::new(services.clone());
        let platform_load = PlatformLoadView::new(services.clone());

        right_bottom.append(recent_launches.widget());
        right_bottom.append(platform_load.widget());

        grid.attach(session_list.widget(), 0, 0, 1, 1);
        grid.attach(storage_quota.widget(), 1, 0, 1, 1);
        grid.attach(launch_form.widget(), 0, 1, 1, 1);
        grid.attach(&right_bottom, 1, 1, 1, 1);

        container.append(&grid);

        let dashboard = DashboardView {
            container,
            session_list,
            launch_form,
            platform_load,
            storage_quota,
            recent_launches,
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
                            let result = services.spawn(async move {
                                let token = svc.get_token().await;
                                let Some(token) = token else { return Err("No token".to_string()) };
                                svc.sessions.delete_session(&token, &id_c).await
                            }).await;
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
                    SessionAction::Renew(id, _name) => {
                        let svc = services.clone();
                        let id_c = id.clone();
                        let result = services.spawn(async move {
                            let token = svc.get_token().await;
                            let Some(token) = token else { return Err("No token".to_string()) };
                            svc.sessions.renew_session(&token, &id_c).await
                        }).await;
                        match result {
                            Ok(()) => {
                                session_list.refresh().await;
                            }
                            Err(e) => eprintln!("Renew failed: {}", e),
                        }
                    }
                    SessionAction::Events(id, name) => {
                        let svc = services.clone();
                        let id_c = id.clone();
                        let result = services.spawn(async move {
                            let token = svc.get_token().await;
                            let Some(token) = token else {
                                return ("No auth".to_string(), "No auth".to_string());
                            };
                            let events = svc.sessions.get_events(&token, &id_c).await
                                .unwrap_or_else(|e| format!("Error: {}", e));
                            let logs = svc.sessions.get_logs(&token, &id_c).await
                                .unwrap_or_else(|e| format!("Error: {}", e));
                            (events, logs)
                        }).await;
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
            self.session_list
                .set_on_sessions_changed(move |count| {
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
                    let params = SessionLaunchParams {
                        name: format!(
                            "{}-{}",
                            launch.session_type,
                            chrono::Local::now().format("%H%M%S")
                        ),
                        image: launch.image.clone(),
                        session_type: launch.session_type.clone(),
                        cores: launch.cores,
                        ram: launch.ram,
                        gpus: launch.gpus,
                        cmd: None,
                        env: None,
                        registry_username: None,
                        registry_secret: None,
                    };

                    let svc = services.clone();
                    let result = services.spawn(async move {
                        let token = svc.get_token().await;
                        let Some(token) = token else { return Err("No token".to_string()) };
                        svc.sessions.launch_session(&token, &params).await
                    }).await;

                    match result {
                        Ok(_) => {
                            glib::timeout_future_seconds(2).await;
                            session_list.refresh().await;
                            recent_launches_ref.refresh();
                        }
                        Err(e) => eprintln!("Relaunch failed: {}", e),
                    }
                });
            });
        }
    }

    pub async fn load_data(&self) {
        log("[Dashboard] starting session_list.refresh");
        self.session_list.refresh().await;
        log("[Dashboard] starting storage_quota.refresh");
        self.storage_quota.refresh().await;
        log("[Dashboard] starting platform_load.refresh");
        self.platform_load.refresh().await;
        log("[Dashboard] starting launch_form.load_images");
        self.launch_form.load_images().await;
        log("[Dashboard] done");

        self.recent_launches.refresh();

        update_session_limits(
            &self.session_list,
            &self.launch_form,
            &self.recent_launches,
        );
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
