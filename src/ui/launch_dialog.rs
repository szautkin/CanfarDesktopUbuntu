use gtk4::prelude::*;
use gtk4::{self as gtk};
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

/// Show a launch result dialog. Auto-closes after 2s on success.
pub async fn show_launch_dialog(
    parent: &impl IsA<gtk::Widget>,
    name: &str,
    image_label: &str,
    session_type: &str,
    cores: u32,
    ram: u32,
    gpus: u32,
    result: Result<String, String>,
) {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 8);

    // Resource summary
    let mut res_parts = vec![format!("CPU: {}", cores), format!("RAM: {}G", ram)];
    if gpus > 0 {
        res_parts.push(format!("GPU: {}", gpus));
    }
    let resource_summary = res_parts.join("  \u{00B7}  ");

    let detail_label = gtk::Label::new(Some(&format!(
        "{}  \u{00B7}  {}",
        image_label, resource_summary
    )));
    detail_label.add_css_class("caption");
    detail_label.add_css_class("dim-label");
    detail_label.set_wrap(true);
    detail_label.set_halign(gtk::Align::Start);
    content.append(&detail_label);

    let (heading, body) = match &result {
        Ok(_) => (
            format!("{} launched!", name),
            "Session is starting. It will appear in Active Sessions shortly.".to_string(),
        ),
        Err(e) => ("Launch failed".to_string(), e.clone()),
    };

    let dialog = adw::MessageDialog::new(
        parent
            .root()
            .and_then(|r| r.downcast::<gtk::Window>().ok())
            .as_ref(),
        Some(&format!("{} {}", session_type, heading)),
        Some(&body),
    );
    dialog.set_extra_child(Some(&content));
    dialog.add_response("close", "Close");
    dialog.set_close_response("close");

    let (sender, receiver) = tokio::sync::oneshot::channel::<()>();
    let sender = Rc::new(RefCell::new(Some(sender)));

    {
        let sender = sender.clone();
        dialog.connect_response(None, move |_, _| {
            if let Some(s) = sender.borrow_mut().take() {
                let _ = s.send(());
            }
        });
    }

    if result.is_ok() {
        let dialog_ref = dialog.downgrade();
        gtk::glib::spawn_future_local(async move {
            gtk::glib::timeout_future(std::time::Duration::from_secs(2)).await;
            if let Some(d) = dialog_ref.upgrade() {
                d.response("close");
            }
        });
    }

    dialog.present();
    let _ = receiver.await;
}
