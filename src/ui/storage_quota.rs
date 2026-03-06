use crate::state::AppServices;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use std::rc::Rc;
use std::sync::Arc;

pub struct StorageQuotaView {
    pub container: gtk::Box,
    used_label: gtk::Label,
    quota_label: gtk::Label,
    percent_label: gtk::Label,
    progress: gtk::ProgressBar,
    date_label: gtk::Label,
    loading_spinner: gtk::Spinner,
    services: Arc<AppServices>,
}

impl StorageQuotaView {
    pub fn new(services: Arc<AppServices>) -> Rc<Self> {
        // === Container — same as SessionListView ===
        let container = gtk::Box::new(gtk::Orientation::Vertical, 8);
        container.add_css_class("card");
        container.set_margin_start(8);
        container.set_margin_end(8);
        container.set_margin_top(8);

        // === Header — same as SessionListView ===
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        header.set_margin_start(16);
        header.set_margin_end(16);
        header.set_margin_top(12);

        let title = gtk::Label::new(Some("User Home Storage"));
        title.add_css_class("title-4");
        title.set_halign(gtk::Align::Start);
        title.set_hexpand(true);
        header.append(&title);

        let refresh_btn = gtk::Button::from_icon_name("view-refresh-symbolic");
        refresh_btn.set_tooltip_text(Some("Refresh"));
        header.append(&refresh_btn);

        container.append(&header);

        // === Loading spinner — EXACTLY like SessionListView ===
        let loading_spinner = gtk::Spinner::new();
        loading_spinner.set_visible(false);
        loading_spinner.set_margin_top(8);
        container.append(&loading_spinner);

        // === Content (unique to this view) ===
        let progress = gtk::ProgressBar::new();
        progress.set_fraction(0.0);
        progress.set_margin_start(16);
        progress.set_margin_end(16);
        container.append(&progress);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 6);
        content.set_margin_start(16);
        content.set_margin_end(16);
        content.set_margin_bottom(12);

        let used_label = gtk::Label::new(Some("Used: \u{2014}"));
        used_label.set_halign(gtk::Align::Start);
        used_label.add_css_class("caption");
        content.append(&used_label);

        let quota_label = gtk::Label::new(Some("Quota: \u{2014}"));
        quota_label.set_halign(gtk::Align::Start);
        quota_label.add_css_class("caption");
        content.append(&quota_label);

        let percent_label = gtk::Label::new(Some("Usage: \u{2014}"));
        percent_label.set_halign(gtk::Align::Start);
        percent_label.add_css_class("caption");
        content.append(&percent_label);

        let date_label = gtk::Label::new(None);
        date_label.set_halign(gtk::Align::Start);
        date_label.add_css_class("dim-label");
        date_label.add_css_class("caption");
        content.append(&date_label);

        container.append(&content);

        // === Rc<Self> — same as SessionListView ===
        let view = Rc::new(StorageQuotaView {
            container,
            used_label,
            quota_label,
            percent_label,
            progress,
            date_label,
            loading_spinner,
            services,
        });

        // === Refresh button — COPIED from SessionListView line 96-104 ===
        {
            let view = view.clone();
            refresh_btn.connect_clicked(move |_| {
                crate::log("[StorageQuota] refresh button CLICKED");
                let view = view.clone();
                glib::spawn_future_local(async move {
                    crate::log("[StorageQuota] spawn_future_local ENTERED");
                    view.refresh().await;
                    crate::log("[StorageQuota] spawn_future_local DONE");
                });
            });
        }

        view
    }

    // === refresh — same structure as SessionListView.refresh() ===
    pub async fn refresh(&self) {
        self.loading_spinner.set_visible(true);
        self.loading_spinner.start();
        crate::log("[StorageQuota] refresh: spinner started");

        let svc = self.services.clone();
        let result = self.services.spawn(async move {
            let token = svc.get_token().await;
            let username = svc.get_username().await;
            crate::log(&format!("[StorageQuota] token={}, username={}", token.is_some(), username.is_some()));
            match (token, username) {
                (Some(t), Some(u)) => svc.storage.get_quota(&t, &u).await.ok(),
                _ => None,
            }
        }).await;

        if let Some(quota) = result {
            let used_gb = quota.used_gb();
            let quota_gb = quota.quota_gb();
            let pct = quota.usage_percent();

            self.used_label.set_text(&format!("Used: {:.1} GB", used_gb));
            self.quota_label.set_text(&format!("Quota: {:.1} GB", quota_gb));
            self.percent_label.set_text(&format!("Usage: {:.1}%", pct));
            self.progress.set_fraction((pct / 100.0).clamp(0.0, 1.0));

            self.progress.remove_css_class("warning");
            self.progress.remove_css_class("error");
            if quota.is_warning() {
                self.progress.add_css_class("error");
            } else if pct > 70.0 {
                self.progress.add_css_class("warning");
            }

            if let Some(ref date) = quota.last_update {
                self.date_label.set_text(&format!("last update: {}", date));
            }
            crate::log("[StorageQuota] refresh: data updated");
        } else {
            self.used_label.set_text("Failed to load storage info");
            crate::log("[StorageQuota] refresh: no data");
        }

        self.loading_spinner.stop();
        self.loading_spinner.set_visible(false);
        crate::log("[StorageQuota] refresh: spinner stopped");
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.container
    }
}
