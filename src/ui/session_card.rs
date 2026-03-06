use crate::models::Session;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use std::cell::RefCell;
use std::rc::Rc;

pub struct SessionCard {
    pub container: gtk::Box,
}

#[derive(Clone)]
pub enum SessionAction {
    Open(String),
    Delete(String, String),
    Renew(String, String),
    Events(String, String),
}

impl SessionCard {
    pub fn new(session: &Session, on_action: Rc<RefCell<Box<dyn Fn(SessionAction)>>>) -> Self {
        let container = gtk::Box::new(gtk::Orientation::Vertical, 0);
        container.set_width_request(320);
        container.add_css_class("card");
        container.set_margin_start(4);
        container.set_margin_end(4);
        container.set_margin_top(4);
        container.set_margin_bottom(4);

        let inner = gtk::Box::new(gtk::Orientation::Vertical, 8);
        inner.set_margin_start(16);
        inner.set_margin_end(16);
        inner.set_margin_top(12);
        inner.set_margin_bottom(12);

        // Header: type badge + name + status
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        header.set_valign(gtk::Align::Center);

        let type_badge = gtk::Label::new(Some(session.type_display()));
        type_badge.add_css_class("caption");
        type_badge.add_css_class(&format!("session-type-{}", session.session_type.to_lowercase()));
        header.append(&type_badge);

        let name_label = gtk::Label::new(Some(&session.name));
        name_label.add_css_class("heading");
        name_label.set_hexpand(true);
        name_label.set_halign(gtk::Align::Start);
        name_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        header.append(&name_label);

        let status_badge = gtk::Label::new(Some(&session.status));
        status_badge.add_css_class("caption");
        status_badge.add_css_class(&format!("status-{}", session.status.to_lowercase()));
        header.append(&status_badge);

        inner.append(&header);

        // Image name
        let image_display = match session.image.rsplit_once('/') {
            Some((_, name)) => name.to_string(),
            None => session.image.clone(),
        };
        let image_label = gtk::Label::new(Some(&image_display));
        image_label.add_css_class("caption");
        image_label.add_css_class("dim-label");
        image_label.set_halign(gtk::Align::Start);
        image_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        inner.append(&image_label);

        // Times
        if !session.start_time.is_empty() {
            let times_box = gtk::Box::new(gtk::Orientation::Horizontal, 16);

            let start_box = gtk::Box::new(gtk::Orientation::Horizontal, 4);
            let start_icon = gtk::Image::from_icon_name("media-playback-start-symbolic");
            start_icon.set_pixel_size(12);
            start_box.append(&start_icon);
            let start_text = gtk::Label::new(Some(&format_time(&session.start_time)));
            start_text.add_css_class("caption");
            start_box.append(&start_text);
            times_box.append(&start_box);

            if !session.expiry_time.is_empty() {
                let expiry_box = gtk::Box::new(gtk::Orientation::Horizontal, 4);
                let expiry_icon = gtk::Image::from_icon_name("alarm-symbolic");
                expiry_icon.set_pixel_size(12);
                expiry_box.append(&expiry_icon);
                let expiry_text = gtk::Label::new(Some(&format_time(&session.expiry_time)));
                expiry_text.add_css_class("caption");
                expiry_box.append(&expiry_text);
                times_box.append(&expiry_box);
            }

            inner.append(&times_box);
        }

        // Resources
        let res_box = gtk::Box::new(gtk::Orientation::Horizontal, 12);

        let cpu_label = gtk::Label::new(Some(&format!("CPU: {}", session.requested_cpu_cores)));
        cpu_label.add_css_class("caption");
        res_box.append(&cpu_label);

        let ram_label = gtk::Label::new(Some(&format!("RAM: {}", session.requested_ram)));
        ram_label.add_css_class("caption");
        res_box.append(&ram_label);

        if session.requested_gpu_cores != "0" {
            let gpu_label =
                gtk::Label::new(Some(&format!("GPU: {}", session.requested_gpu_cores)));
            gpu_label.add_css_class("caption");
            res_box.append(&gpu_label);
        }

        if !session.is_fixed_resources {
            let flex_badge = gtk::Label::new(Some("FLEX"));
            flex_badge.add_css_class("caption");
            flex_badge.add_css_class("flex-badge");
            res_box.append(&flex_badge);
        }

        inner.append(&res_box);

        // Action buttons
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        actions.set_halign(gtk::Align::End);
        actions.set_margin_top(4);

        let open_btn = gtk::Button::from_icon_name("web-browser-symbolic");
        open_btn.set_tooltip_text(Some("Open in browser"));
        open_btn.set_sensitive(session.is_running());
        {
            let url = session.connect_url.clone();
            let on_action = on_action.clone();
            open_btn.connect_clicked(move |_| {
                (on_action.borrow())(SessionAction::Open(url.clone()));
            });
        }
        actions.append(&open_btn);

        let renew_btn = gtk::Button::from_icon_name("view-refresh-symbolic");
        renew_btn.set_tooltip_text(Some("Renew session"));
        {
            let id = session.id.clone();
            let name = session.name.clone();
            let on_action = on_action.clone();
            renew_btn.connect_clicked(move |_| {
                (on_action.borrow())(SessionAction::Renew(id.clone(), name.clone()));
            });
        }
        actions.append(&renew_btn);

        let events_btn = gtk::Button::from_icon_name("dialog-information-symbolic");
        events_btn.set_tooltip_text(Some("View events/logs"));
        {
            let id = session.id.clone();
            let name = session.name.clone();
            let on_action = on_action.clone();
            events_btn.connect_clicked(move |_| {
                (on_action.borrow())(SessionAction::Events(id.clone(), name.clone()));
            });
        }
        actions.append(&events_btn);

        let delete_btn = gtk::Button::from_icon_name("user-trash-symbolic");
        delete_btn.set_tooltip_text(Some("Delete session"));
        delete_btn.add_css_class("destructive-action");
        {
            let id = session.id.clone();
            let name = session.name.clone();
            let on_action = on_action.clone();
            delete_btn.connect_clicked(move |_| {
                (on_action.borrow())(SessionAction::Delete(id.clone(), name.clone()));
            });
        }
        actions.append(&delete_btn);

        inner.append(&actions);
        container.append(&inner);

        SessionCard {
            container,
        }
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.container
    }

}

fn format_time(iso: &str) -> String {
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(iso) {
        dt.format("%b %d %H:%M").to_string()
    } else if iso.len() > 16 {
        iso[..16].to_string()
    } else {
        iso.to_string()
    }
}
