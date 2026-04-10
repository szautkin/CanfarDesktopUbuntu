//! Modal dialog that lets the user pick one file from a multi-file DataLink
//! response.  Shown when an observation resolves to more than one
//! `#this`-semantic file (e.g. per-CCD MegaCam products).
//!
//! Returns the selected `DataLinkFile` or `None` if the user cancelled.

use crate::models::search_result::DataLinkFile;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

pub async fn show_datalink_file_dialog(
    parent: &adw::ApplicationWindow,
    files: Vec<DataLinkFile>,
) -> Option<DataLinkFile> {
    if files.is_empty() {
        return None;
    }

    let dialog = adw::Window::builder()
        .title(format!("Select File ({} available)", files.len()))
        .default_width(520)
        .default_height(420)
        .modal(true)
        .transient_for(parent)
        .build();

    let toolbar_view = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    toolbar_view.add_top_bar(&header);

    // Scrollable list of files
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_vexpand(true);
    scroll.set_hexpand(true);
    scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);

    let list_box = gtk::ListBox::new();
    list_box.set_selection_mode(gtk::SelectionMode::None);
    list_box.add_css_class("boxed-list");
    list_box.set_margin_start(12);
    list_box.set_margin_end(12);
    list_box.set_margin_top(12);
    list_box.set_margin_bottom(12);

    // Radio group — chain check buttons together so only one is selected at a time
    let selected_index: Rc<RefCell<usize>> = Rc::new(RefCell::new(0));
    let first_check: Rc<RefCell<Option<gtk::CheckButton>>> = Rc::new(RefCell::new(None));

    for (idx, file) in files.iter().enumerate() {
        let row = adw::ActionRow::builder()
            .title(glib::markup_escape_text(&file.filename()))
            .subtitle(glib::markup_escape_text(&format!(
                "{}  ·  {}",
                if file.semantics.is_empty() {
                    "(no semantics)"
                } else {
                    &file.semantics
                },
                if file.content_type.is_empty() {
                    "unknown type".to_string()
                } else {
                    file.content_type.clone()
                }
            )))
            .activatable(true)
            .build();

        let check = gtk::CheckButton::new();
        check.set_valign(gtk::Align::Center);

        // Chain radio group: first check is the leader, subsequent ones join it
        if let Some(leader) = first_check.borrow().as_ref() {
            check.set_group(Some(leader));
        }

        if idx == 0 {
            check.set_active(true);
            *first_check.borrow_mut() = Some(check.clone());
        }

        row.add_prefix(&check);
        row.set_activatable_widget(Some(&check));

        // When toggled, update the selected index
        {
            let selected_index = selected_index.clone();
            check.connect_toggled(move |btn| {
                if btn.is_active() {
                    *selected_index.borrow_mut() = idx;
                }
            });
        }

        list_box.append(&row);
    }

    scroll.set_child(Some(&list_box));
    toolbar_view.set_content(Some(&scroll));

    // Bottom action bar: Cancel + Download
    let action_bar = gtk::ActionBar::new();

    let cancel_btn = gtk::Button::with_label("Cancel");
    action_bar.pack_start(&cancel_btn);

    let download_btn = gtk::Button::with_label("Download");
    download_btn.add_css_class("suggested-action");
    action_bar.pack_end(&download_btn);

    toolbar_view.add_bottom_bar(&action_bar);

    dialog.set_content(Some(&toolbar_view));

    // Coordination channel
    let (tx, rx) = tokio::sync::oneshot::channel::<Option<usize>>();
    let tx = Rc::new(RefCell::new(Some(tx)));

    {
        let dialog_ref = dialog.clone();
        let tx = tx.clone();
        cancel_btn.connect_clicked(move |_| {
            if let Some(tx) = tx.borrow_mut().take() {
                let _ = tx.send(None);
            }
            dialog_ref.close();
        });
    }
    {
        let dialog_ref = dialog.clone();
        let tx = tx.clone();
        let selected_index = selected_index.clone();
        download_btn.connect_clicked(move |_| {
            let idx = *selected_index.borrow();
            if let Some(tx) = tx.borrow_mut().take() {
                let _ = tx.send(Some(idx));
            }
            dialog_ref.close();
        });
    }
    {
        let tx = tx.clone();
        dialog.connect_close_request(move |_| {
            if let Some(tx) = tx.borrow_mut().take() {
                let _ = tx.send(None);
            }
            glib::Propagation::Proceed
        });
    }

    dialog.present();
    match rx.await {
        Ok(Some(idx)) => files.into_iter().nth(idx),
        _ => None,
    }
}
