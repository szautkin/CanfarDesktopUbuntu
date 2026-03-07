use crate::models::StorageQuota;
use crate::state::AppServices;
use crate::ui::card_header::card_header;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

pub struct StorageQuotaView {
    pub container: gtk::Box,
    content_box: gtk::Box,
    status_label: gtk::Label,
    loading_spinner: gtk::Spinner,
    refresh_btn: gtk::Button,
    data: Rc<RefCell<Option<StorageQuota>>>,
    services: Arc<AppServices>,
}

impl StorageQuotaView {
    pub fn new(services: Arc<AppServices>) -> Rc<Self> {
        let container = gtk::Box::new(gtk::Orientation::Vertical, 8);
        container.add_css_class("card");
        container.set_margin_start(8);
        container.set_margin_end(8);
        container.set_margin_top(8);

        let (header, loading_spinner, refresh_btn) = card_header("User Home Storage");
        container.append(&header);

        let status_label = gtk::Label::new(None);
        status_label.set_halign(gtk::Align::Start);
        status_label.add_css_class("dim-label");
        status_label.add_css_class("caption");
        status_label.set_margin_start(16);
        status_label.set_visible(false);
        container.append(&status_label);

        let content_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
        content_box.set_margin_start(16);
        content_box.set_margin_end(16);
        content_box.set_margin_bottom(12);
        container.append(&content_box);

        let view = Rc::new(StorageQuotaView {
            container,
            content_box,
            status_label,
            loading_spinner,
            refresh_btn: refresh_btn.clone(),
            data: Rc::new(RefCell::new(None)),
            services,
        });

        {
            let view = view.clone();
            refresh_btn.connect_clicked(move |btn| {
                btn.set_sensitive(false);
                view.loading_spinner.set_visible(true);
                view.loading_spinner.start();

                let view = view.clone();
                glib::spawn_future_local(async move {
                    view.fetch_and_update().await;
                });
            });
        }

        view
    }

    async fn fetch_and_update(&self) {
        let svc = self.services.clone();
        let result = self
            .services
            .spawn(async move {
                let token = svc.get_token().await;
                let username = svc.get_username().await;
                match (token, username) {
                    (Some(t), Some(u)) => match svc.storage.get_quota(&t, &u).await {
                        Ok(q) => Ok(q),
                        Err(e) => Err(e),
                    },
                    _ => Err("Not authenticated".to_string()),
                }
            })
            .await;

        match result {
            Ok(quota) => {
                self.update_data(quota);
                self.status_label.set_visible(false);
            }
            Err(e) => {
                self.status_label.set_text(&format!("Error: {}", e));
                self.status_label.set_visible(true);
            }
        }

        self.loading_spinner.stop();
        self.loading_spinner.set_visible(false);
        self.refresh_btn.set_sensitive(true);
    }

    pub async fn refresh(&self) {
        self.loading_spinner.set_visible(true);
        self.loading_spinner.start();
        self.refresh_btn.set_sensitive(false);
        self.fetch_and_update().await;
    }

    fn update_data(&self, quota: StorageQuota) {
        while let Some(child) = self.content_box.first_child() {
            self.content_box.remove(&child);
        }

        let used_gb = quota.used_gb();
        let quota_gb = quota.quota_gb();
        let pct = quota.usage_percent();

        let progress = gtk::ProgressBar::new();
        progress.set_fraction((pct / 100.0).clamp(0.0, 1.0));
        if quota.is_warning() {
            progress.add_css_class("error");
        } else if pct > 70.0 {
            progress.add_css_class("warning");
        }
        self.content_box.append(&progress);

        let used_label = gtk::Label::new(Some(&format!("Used: {:.1} GB", used_gb)));
        used_label.set_halign(gtk::Align::Start);
        used_label.add_css_class("caption");
        self.content_box.append(&used_label);

        let quota_label = gtk::Label::new(Some(&format!("Quota: {:.1} GB", quota_gb)));
        quota_label.set_halign(gtk::Align::Start);
        quota_label.add_css_class("caption");
        self.content_box.append(&quota_label);

        let percent_label = gtk::Label::new(Some(&format!("Usage: {:.1}%", pct)));
        percent_label.set_halign(gtk::Align::Start);
        percent_label.add_css_class("caption");
        self.content_box.append(&percent_label);

        if let Some(ref date) = quota.last_update {
            let date_label = gtk::Label::new(Some(&format!("last update: {}", date)));
            date_label.set_halign(gtk::Align::Start);
            date_label.add_css_class("dim-label");
            date_label.add_css_class("caption");
            self.content_box.append(&date_label);
        }

        *self.data.borrow_mut() = Some(quota);
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.container
    }
}
