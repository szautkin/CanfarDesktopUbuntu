use crate::models::Session;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use std::cell::RefCell;
use std::rc::Rc;

pub type ActionCallback = Rc<RefCell<Box<dyn Fn(SessionAction)>>>;

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
    pub fn new(session: &Session, on_action: ActionCallback) -> Self {
        let container = gtk::Box::new(gtk::Orientation::Vertical, 0);
        container.set_size_request(220, -1);
        container.set_hexpand(false);
        container.set_halign(gtk::Align::Start);
        container.set_valign(gtk::Align::Start);
        container.add_css_class("card");
        container.set_margin_start(6);
        container.set_margin_end(6);
        container.set_margin_top(6);
        container.set_margin_bottom(6);

        let inner = gtk::Box::new(gtk::Orientation::Vertical, 6);
        inner.set_margin_start(12);
        inner.set_margin_end(12);
        inner.set_margin_top(12);
        inner.set_margin_bottom(12);

        // Header: session-type avatar (clips the opaque logo assets) beside the
        // name + status, with the image caption underneath.
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 12);

        let avatar = crate::ui::session_icon::session_type_avatar(&session.session_type, 40);
        avatar.set_valign(gtk::Align::Start);
        header.append(&avatar);

        let title_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
        title_box.set_hexpand(true);

        let name_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let name_label = gtk::Label::new(Some(&session.name));
        name_label.add_css_class("heading");
        name_label.set_hexpand(true);
        name_label.set_halign(gtk::Align::Start);
        name_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        name_row.append(&name_label);

        let status_badge = gtk::Label::new(Some(&session.status));
        status_badge.add_css_class("caption");
        status_badge.add_css_class(&format!("status-{}", session.status.to_lowercase()));
        status_badge.set_valign(gtk::Align::Center);
        name_row.append(&status_badge);
        title_box.append(&name_row);

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
        title_box.append(&image_label);

        header.append(&title_box);
        inner.append(&header);

        // Times
        if !session.start_time.is_empty() {
            let times_box = gtk::Box::new(gtk::Orientation::Horizontal, 12);

            let start_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            let start_icon = gtk::Image::from_icon_name("media-playback-start-symbolic");
            start_icon.set_pixel_size(12);
            start_box.append(&start_icon);
            let start_text = gtk::Label::new(Some(&format_time(&session.start_time)));
            start_text.add_css_class("caption");
            start_box.append(&start_text);
            times_box.append(&start_box);

            if !session.expiry_time.is_empty() {
                let expiry_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
                let expiry_icon = gtk::Image::from_icon_name("alarm-symbolic");
                expiry_icon.set_pixel_size(12);
                expiry_box.append(&expiry_icon);
                let expiry_text = gtk::Label::new(Some(&format_time(&session.expiry_time)));
                expiry_text.add_css_class("caption");

                // Highlight if expiring soon
                if let Ok(expiry_dt) = chrono::DateTime::parse_from_rfc3339(&session.expiry_time) {
                    let remaining = expiry_dt.signed_duration_since(chrono::Utc::now());
                    if remaining.num_hours() < 1 && remaining.num_seconds() > 0 {
                        expiry_text.add_css_class("error");
                    } else if remaining.num_hours() < 24 {
                        expiry_text.add_css_class("warning");
                    }
                }

                expiry_box.append(&expiry_text);
                times_box.append(&expiry_box);
            }

            inner.append(&times_box);
        }

        // Resources: fixed sessions show the requested CPU/RAM/GPU; flexible
        // ones have no requested values, so show only the FLEX badge with a
        // caption (never "CPU:  RAM:" with empty values).
        let res_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        if session.is_fixed_resources {
            if !session.requested_cpu_cores.is_empty() {
                let cpu_label = gtk::Label::new(Some(&crate::tr_fmt!(
                    "CPU: {}",
                    session.requested_cpu_cores
                )));
                cpu_label.add_css_class("caption");
                res_box.append(&cpu_label);
            }
            if !session.requested_ram.is_empty() {
                let ram_label =
                    gtk::Label::new(Some(&crate::tr_fmt!("RAM: {}", session.requested_ram)));
                ram_label.add_css_class("caption");
                res_box.append(&ram_label);
            }
            if session.requested_gpu_cores != "0" && !session.requested_gpu_cores.is_empty() {
                let gpu_label = gtk::Label::new(Some(&crate::tr_fmt!(
                    "GPU: {}",
                    session.requested_gpu_cores
                )));
                gpu_label.add_css_class("caption");
                res_box.append(&gpu_label);
            }
        } else {
            let res_caption = gtk::Label::new(Some(crate::tr_en!("Resources")));
            res_caption.add_css_class("caption");
            res_caption.add_css_class("dim-label");
            res_box.append(&res_caption);
            let flex_badge = gtk::Label::new(Some(crate::tr_en!("FLEX")));
            flex_badge.add_css_class("caption");
            flex_badge.add_css_class("flex-badge");
            flex_badge.set_tooltip_text(Some(crate::tr_en!(
                "Flexible resources — allocated by the platform"
            )));
            res_box.append(&flex_badge);
        }

        inner.append(&res_box);

        // In-use resources (only for running sessions with non-zero usage)
        if session.is_running()
            && (!session.cpu_cores_in_use.is_empty() || !session.ram_in_use.is_empty())
        {
            let has_cpu = !session.cpu_cores_in_use.is_empty() && session.cpu_cores_in_use != "0";
            let has_ram = !session.ram_in_use.is_empty() && session.ram_in_use != "0";
            if has_cpu || has_ram {
                let usage_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
                let prefix = gtk::Label::new(Some(crate::tr_en!("In use:")));
                prefix.add_css_class("caption");
                prefix.add_css_class("dim-label");
                usage_box.append(&prefix);
                if has_cpu {
                    let lbl =
                        gtk::Label::new(Some(&crate::tr_fmt!("CPU: {}", session.cpu_cores_in_use)));
                    lbl.add_css_class("caption");
                    usage_box.append(&lbl);
                }
                if has_ram {
                    let lbl = gtk::Label::new(Some(&crate::tr_fmt!("RAM: {}", session.ram_in_use)));
                    lbl.add_css_class("caption");
                    usage_box.append(&lbl);
                }
                inner.append(&usage_box);
            }
        }

        // Action buttons
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        actions.set_halign(gtk::Align::End);
        actions.set_margin_top(6);

        let open_btn = gtk::Button::from_icon_name("web-browser-symbolic");
        open_btn.add_css_class("flat");
        open_btn.add_css_class("circular");
        open_btn.set_tooltip_text(Some(crate::tr_en!("Open in browser")));
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
        renew_btn.add_css_class("flat");
        renew_btn.add_css_class("circular");
        renew_btn.set_tooltip_text(Some(crate::tr_en!("Renew session")));
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
        events_btn.add_css_class("flat");
        events_btn.add_css_class("circular");
        events_btn.set_tooltip_text(Some(crate::tr_en!("View events/logs")));
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
        delete_btn.add_css_class("circular");
        delete_btn.add_css_class("destructive-action");
        delete_btn.set_tooltip_text(Some(crate::tr_en!("Delete session")));
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

        SessionCard { container }
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
