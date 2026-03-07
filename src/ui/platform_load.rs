use crate::models::SkahaStatsResponse;
use crate::state::AppServices;
use crate::ui::card_header::card_header;
use crate::ui::metric_bar::MetricBar;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

pub struct PlatformLoadView {
    pub container: gtk::Box,
    content_box: gtk::Box,
    status_label: gtk::Label,
    loading_spinner: gtk::Spinner,
    refresh_btn: gtk::Button,
    data: Rc<RefCell<Option<SkahaStatsResponse>>>,
    services: Arc<AppServices>,
}

impl PlatformLoadView {
    pub fn new(services: Arc<AppServices>) -> Rc<Self> {
        let container = gtk::Box::new(gtk::Orientation::Vertical, 8);
        container.add_css_class("card");
        container.set_margin_start(8);
        container.set_margin_end(8);
        container.set_margin_bottom(8);

        let (header, loading_spinner, refresh_btn) = card_header("Platform Load");
        container.append(&header);

        let status_label = gtk::Label::new(None);
        status_label.set_halign(gtk::Align::Start);
        status_label.add_css_class("dim-label");
        status_label.add_css_class("caption");
        status_label.set_margin_start(16);
        status_label.set_visible(false);
        container.append(&status_label);

        let content_box = gtk::Box::new(gtk::Orientation::Vertical, 12);
        content_box.set_margin_start(16);
        content_box.set_margin_end(16);
        content_box.set_margin_bottom(12);
        container.append(&content_box);

        let view = Rc::new(PlatformLoadView {
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
                if let Some(token) = token {
                    svc.platform.get_stats(&token).await.ok()
                } else {
                    None
                }
            })
            .await;

        if let Some(stats) = result {
            self.update_data(stats);
            self.status_label.set_visible(false);
        } else {
            self.status_label.set_text("Failed to load platform data");
            self.status_label.set_visible(true);
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

    fn update_data(&self, stats: SkahaStatsResponse) {
        while let Some(child) = self.content_box.first_child() {
            self.content_box.remove(&child);
        }

        if let Some(ref cores) = stats.cores {
            let cpu_bar = MetricBar::new("Available CPUs");
            cpu_bar.update_available("CPUs", cores.requested(), cores.total(), "");
            self.content_box.append(cpu_bar.widget());
        }

        if let Some(ref ram) = stats.ram {
            let ram_bar = MetricBar::new("Available RAM");
            ram_bar.update_available("RAM", ram.requested_gb(), ram.total_gb(), "GB");
            self.content_box.append(ram_bar.widget());
        }

        if let Some(ref instances) = stats.instances {
            let label = gtk::Label::new(Some(&format!(
                "Instances: {} total ({} sessions, {} desktop apps, {} headless)",
                instances.total.unwrap_or(0),
                instances.session.unwrap_or(0),
                instances.desktop_app.unwrap_or(0),
                instances.headless.unwrap_or(0),
            )));
            label.set_halign(gtk::Align::Start);
            label.add_css_class("caption");
            self.content_box.append(&label);
        }

        let updated = gtk::Label::new(Some(&format!(
            "last update: {} UTC",
            chrono::Utc::now().format("%Y-%m-%d %H:%M")
        )));
        updated.set_halign(gtk::Align::Start);
        updated.add_css_class("dim-label");
        updated.add_css_class("caption");
        self.content_box.append(&updated);

        *self.data.borrow_mut() = Some(stats);
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.container
    }
}
