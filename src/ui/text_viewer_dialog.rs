//! Shared read-only text viewer dialog.
//!
//! Used to display logs, events, or any other multi-line text content.
//! Supports single-panel (`show_text_dialog`) and tabbed (`show_tabbed_text_dialog`)
//! variants. Both await until the dialog is closed.

use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use std::cell::RefCell;
use std::rc::Rc;

fn make_text_panel(content: &str) -> gtk::ScrolledWindow {
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_vexpand(true);
    scroll.set_hexpand(true);

    let view = gtk::TextView::new();
    view.set_editable(false);
    view.set_monospace(true);
    view.set_margin_start(8);
    view.set_margin_end(8);
    view.set_margin_top(8);
    view.set_margin_bottom(8);
    view.set_wrap_mode(gtk::WrapMode::WordChar);

    let display = if content.is_empty() {
        crate::tr_en!("(empty)")
    } else {
        content
    };
    view.buffer().set_text(display);

    scroll.set_child(Some(&view));
    scroll
}

/// Show a modal dialog with a single read-only text panel.
pub async fn show_text_dialog(parent: &impl IsA<gtk::Widget>, title: &str, content: &str) {
    let window = gtk::Window::builder()
        .title(title)
        .default_width(600)
        .default_height(500)
        .modal(true)
        .build();

    if let Some(root) = parent.root().and_then(|r| r.downcast::<gtk::Window>().ok()) {
        window.set_transient_for(Some(&root));
    }

    window.set_child(Some(&make_text_panel(content)));

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

/// Show a modal dialog with multiple read-only text tabs.
///
/// `tabs` is a slice of `(tab_label, content)` pairs.
pub async fn show_tabbed_text_dialog(
    parent: &impl IsA<gtk::Widget>,
    title: &str,
    tabs: &[(&str, &str)],
) {
    let window = gtk::Window::builder()
        .title(title)
        .default_width(600)
        .default_height(500)
        .modal(true)
        .build();

    if let Some(root) = parent.root().and_then(|r| r.downcast::<gtk::Window>().ok()) {
        window.set_transient_for(Some(&root));
    }

    let notebook = gtk::Notebook::new();
    for (label, content) in tabs {
        notebook.append_page(
            &make_text_panel(content),
            Some(&gtk::Label::new(Some(label))),
        );
    }

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
