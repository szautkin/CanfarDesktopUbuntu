use gtk4::prelude::*;
use gtk4::{self as gtk};
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

pub async fn show_delete_dialog(parent: &impl IsA<gtk::Widget>, session_name: &str) -> bool {
    let dialog = adw::MessageDialog::new(
        parent
            .root()
            .and_then(|r| r.downcast::<gtk::Window>().ok())
            .as_ref(),
        Some("Delete Session"),
        Some(&format!(
            "Are you sure you want to delete session '{}'?\n\nThis action cannot be undone.",
            session_name
        )),
    );

    dialog.add_response("cancel", "Cancel");
    dialog.add_response("delete", "Delete");
    dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");

    let result = Rc::new(RefCell::new(false));
    let (sender, receiver) = tokio::sync::oneshot::channel::<()>();
    let sender = Rc::new(RefCell::new(Some(sender)));

    {
        let result = result.clone();
        let sender = sender.clone();
        dialog.connect_response(None, move |_, response| {
            *result.borrow_mut() = response == "delete";
            if let Some(s) = sender.borrow_mut().take() {
                let _ = s.send(());
            }
        });
    }

    dialog.present();
    let _ = receiver.await;
    let val = *result.borrow();
    val
}
