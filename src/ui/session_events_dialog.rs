use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use std::cell::RefCell;
use std::rc::Rc;

pub async fn show_events_dialog(
    parent: &impl IsA<gtk::Widget>,
    session_name: &str,
    events: &str,
    logs: &str,
) {
    let window = gtk::Window::builder()
        .title(format!("Events/Logs: {}", session_name))
        .default_width(600)
        .default_height(500)
        .modal(true)
        .build();

    if let Some(root) = parent.root().and_then(|r| r.downcast::<gtk::Window>().ok()) {
        window.set_transient_for(Some(&root));
    }

    let notebook = gtk::Notebook::new();

    // Events tab
    let events_scroll = gtk::ScrolledWindow::new();
    events_scroll.set_vexpand(true);
    let events_view = gtk::TextView::new();
    events_view.set_editable(false);
    events_view.set_monospace(true);
    events_view.set_margin_start(8);
    events_view.set_margin_end(8);
    events_view.set_margin_top(8);
    events_view.set_margin_bottom(8);
    events_view.set_wrap_mode(gtk::WrapMode::Word);
    events_view.buffer().set_text(if events.is_empty() {
        "No events available"
    } else {
        events
    });
    events_scroll.set_child(Some(&events_view));
    notebook.append_page(&events_scroll, Some(&gtk::Label::new(Some("Events"))));

    // Logs tab
    let logs_scroll = gtk::ScrolledWindow::new();
    logs_scroll.set_vexpand(true);
    let logs_view = gtk::TextView::new();
    logs_view.set_editable(false);
    logs_view.set_monospace(true);
    logs_view.set_margin_start(8);
    logs_view.set_margin_end(8);
    logs_view.set_margin_top(8);
    logs_view.set_margin_bottom(8);
    logs_view.set_wrap_mode(gtk::WrapMode::Word);
    logs_view.buffer().set_text(if logs.is_empty() {
        "No logs available"
    } else {
        logs
    });
    logs_scroll.set_child(Some(&logs_view));
    notebook.append_page(&logs_scroll, Some(&gtk::Label::new(Some("Logs"))));

    window.set_child(Some(&notebook));

    let (sender, receiver) = tokio::sync::oneshot::channel::<()>();
    let sender = Rc::new(RefCell::new(Some(sender)));

    window.connect_close_request(move |_| {
        if let Some(s) = sender.borrow_mut().take() {
            let _ = s.send(());
        }
        glib::Propagation::Proceed
    });

    window.present();
    let _ = receiver.await;
}
