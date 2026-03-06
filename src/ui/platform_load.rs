use crate::state::AppServices;
use crate::ui::metric_bar::MetricBar;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use std::rc::Rc;
use std::sync::Arc;

pub struct PlatformLoadView {
    pub container: gtk::Box,
    cpu_bar: MetricBar,
    ram_bar: MetricBar,
    instances_label: gtk::Label,
    updated_label: gtk::Label,
    loading_spinner: gtk::Spinner,
    services: Arc<AppServices>,
}

impl PlatformLoadView {
    pub fn new(services: Arc<AppServices>) -> Rc<Self> {
        // === Container — same as SessionListView ===
        let container = gtk::Box::new(gtk::Orientation::Vertical, 8);
        container.add_css_class("card");
        container.set_margin_start(8);
        container.set_margin_end(8);
        container.set_margin_bottom(8);

        // === Header — same as SessionListView ===
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        header.set_margin_start(16);
        header.set_margin_end(16);
        header.set_margin_top(12);

        let title = gtk::Label::new(Some("Platform Load"));
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
        let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
        content.set_margin_start(16);
        content.set_margin_end(16);
        content.set_margin_bottom(12);

        let cpu_bar = MetricBar::new("Available CPUs");
        content.append(cpu_bar.widget());

        let ram_bar = MetricBar::new("Available RAM");
        content.append(ram_bar.widget());

        let instances_label = gtk::Label::new(Some("Instances: \u{2014}"));
        instances_label.set_halign(gtk::Align::Start);
        instances_label.add_css_class("caption");
        content.append(&instances_label);

        let updated_label = gtk::Label::new(None);
        updated_label.set_halign(gtk::Align::Start);
        updated_label.add_css_class("dim-label");
        updated_label.add_css_class("caption");
        content.append(&updated_label);

        container.append(&content);

        // === Rc<Self> — same as SessionListView ===
        let view = Rc::new(PlatformLoadView {
            container,
            cpu_bar,
            ram_bar,
            instances_label,
            updated_label,
            loading_spinner,
            services,
        });

        // === Refresh button — COPIED from SessionListView line 96-104 ===
        {
            let view = view.clone();
            refresh_btn.connect_clicked(move |_| {
                crate::log("[PlatformLoad] refresh button CLICKED");
                let view = view.clone();
                glib::spawn_future_local(async move {
                    crate::log("[PlatformLoad] spawn_future_local ENTERED");
                    view.refresh().await;
                    crate::log("[PlatformLoad] spawn_future_local DONE");
                });
            });
        }

        view
    }

    // === refresh — same structure as SessionListView.refresh() ===
    pub async fn refresh(&self) {
        self.loading_spinner.set_visible(true);
        self.loading_spinner.start();
        crate::log("[PlatformLoad] refresh: spinner started");

        let svc = self.services.clone();
        let result = self.services.spawn(async move {
            let token = svc.get_token().await;
            crate::log(&format!("[PlatformLoad] token={}", token.is_some()));
            if let Some(token) = token {
                svc.platform.get_stats(&token).await.ok()
            } else {
                None
            }
        }).await;

        if let Some(stats) = result {
            if let Some(ref cores) = stats.cores {
                self.cpu_bar.update_available(
                    "CPUs", cores.requested(), cores.total(), "",
                );
            }
            if let Some(ref ram) = stats.ram {
                self.ram_bar.update_available(
                    "RAM", ram.requested_gb(), ram.total_gb(), "GB",
                );
            }
            if let Some(ref instances) = stats.instances {
                self.instances_label.set_text(&format!(
                    "Instances: {} total ({} sessions, {} desktop apps, {} headless)",
                    instances.total.unwrap_or(0),
                    instances.session.unwrap_or(0),
                    instances.desktop_app.unwrap_or(0),
                    instances.headless.unwrap_or(0),
                ));
            }
            self.updated_label.set_text(&format!(
                "last update: {} UTC",
                chrono::Utc::now().format("%Y-%m-%d %H:%M")
            ));
            crate::log("[PlatformLoad] refresh: data updated");
        } else {
            crate::log("[PlatformLoad] refresh: no data");
        }

        self.loading_spinner.stop();
        self.loading_spinner.set_visible(false);
        crate::log("[PlatformLoad] refresh: spinner stopped");
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.container
    }
}
