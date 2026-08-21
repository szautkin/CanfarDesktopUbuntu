//! Generic rename dialog — prompts for a new name, prefilled with the current one.

use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

/// Show a modal rename dialog and return the user-entered name, or None if cancelled.
pub async fn show_rename_dialog(
    parent: &impl IsA<gtk::Widget>,
    title: &str,
    current_name: &str,
) -> Option<String> {
    let dialog = adw::Window::builder()
        .title(title)
        .default_width(crate::ui::fit::PROMPT)
        .modal(true)
        .build();

    if let Some(root) = parent.root().and_downcast::<gtk::Window>() {
        dialog.set_transient_for(Some(&root));
    }

    let toolbar_view = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    toolbar_view.add_top_bar(&header);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    content.set_margin_start(24);
    content.set_margin_end(24);
    content.set_margin_top(12);
    content.set_margin_bottom(24);

    let entry = gtk::Entry::new();
    entry.set_text(current_name);
    entry.set_activates_default(true);
    // Select the file stem (before the last dot) so extension is preserved by default
    let select_end = current_name
        .rfind('.')
        .map(|i| i as i32)
        .unwrap_or(current_name.len() as i32);
    entry.select_region(0, select_end);
    content.append(&entry);

    let btn_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    btn_row.set_halign(gtk::Align::End);
    let cancel_btn = gtk::Button::with_label(crate::tr_en!("Cancel"));
    let rename_btn = gtk::Button::with_label(crate::tr_en!("Rename"));
    rename_btn.add_css_class("suggested-action");
    rename_btn.set_receives_default(true);
    btn_row.append(&cancel_btn);
    btn_row.append(&rename_btn);
    content.append(&btn_row);

    toolbar_view.set_content(Some(&content));
    dialog.set_content(Some(&toolbar_view));

    let (tx, rx) = tokio::sync::oneshot::channel::<Option<String>>();
    let tx = Rc::new(RefCell::new(Some(tx)));

    {
        let dialog = dialog.clone();
        let tx = tx.clone();
        cancel_btn.connect_clicked(move |_| {
            if let Some(tx) = tx.borrow_mut().take() {
                let _ = tx.send(None);
            }
            dialog.close();
        });
    }
    {
        let dialog = dialog.clone();
        let entry = entry.clone();
        let tx = tx.clone();
        let original = current_name.to_string();
        rename_btn.connect_clicked(move |_| {
            let name = entry.text().to_string();
            let name_trimmed = name.trim();
            if !name_trimmed.is_empty() && name_trimmed != original {
                if let Some(tx) = tx.borrow_mut().take() {
                    let _ = tx.send(Some(name_trimmed.to_string()));
                }
                dialog.close();
            } else {
                // No change — just cancel
                if let Some(tx) = tx.borrow_mut().take() {
                    let _ = tx.send(None);
                }
                dialog.close();
            }
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
    rx.await.ok().flatten()
}
