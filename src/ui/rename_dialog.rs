//! Generic rename dialog — prompts for a new name, prefilled with the current one.

use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use std::cell::RefCell;
use std::rc::Rc;

/// Show a modal rename dialog and return the user-entered name, or None if cancelled.
pub async fn show_rename_dialog(
    parent: &impl IsA<gtk::Widget>,
    title: &str,
    current_name: &str,
) -> Option<String> {
    let shell = crate::ui::dialog::Dialog::new(title, crate::ui::fit::PROMPT, 320);
    let dialog = shell.window.clone();
    let content = shell.content().clone();

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

    // In the shell's bottom bar rather than appended to the content: buttons
    // inside a scroller can be scrolled away from.
    let cancel_btn = gtk::Button::with_label(crate::tr_en!("Cancel"));
    let rename_btn = gtk::Button::with_label(crate::tr_en!("Rename"));
    rename_btn.add_css_class("suggested-action");
    rename_btn.set_receives_default(true);
    shell.add_secondary_action(&cancel_btn);
    shell.add_action(&rename_btn);

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

    shell.present(parent);
    rx.await.ok().flatten()
}
