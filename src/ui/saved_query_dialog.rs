//! Detail dialog for a saved ADQL query.
//!
//! Shows the full ADQL in a monospace, selectable, read-only view with a
//! header containing the query name + creation time, and a button row with
//! Copy / Rename / Load into Editor / Run Query actions.
//!
//! Returns an [`SavedQueryAction`] describing what the user chose.

use crate::helpers::adql_summary;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

/// What the user decided in the saved query dialog.
#[derive(Debug, Clone)]
pub enum SavedQueryAction {
    /// No action — just close the dialog.
    None,
    /// Load the ADQL into the editor.
    Load,
    /// Run the query immediately.
    Run,
    /// Rename the query to the given new name.
    Rename(String),
    /// Delete the query.
    Delete,
}

/// Show the saved query detail dialog modally.
///
/// Arguments:
/// - `parent`: the widget whose root window becomes the transient parent
/// - `name`: the query name
/// - `adql`: the raw ADQL text
/// - `created_at`: RFC3339 timestamp or empty
pub async fn show_saved_query_dialog(
    parent: &impl IsA<gtk::Widget>,
    name: &str,
    adql: &str,
    created_at: &str,
) -> SavedQueryAction {
    let dialog = adw::Window::builder()
        .title(crate::tr_fmt!("Query: {}", name))
        .default_width(crate::ui::fit::DETAIL)
        .default_height(520)
        .modal(true)
        .build();

    if let Some(root) = parent.root().and_downcast::<gtk::Window>() {
        dialog.set_transient_for(Some(&root));
    }

    let toolbar_view = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    toolbar_view.add_top_bar(&header);

    // Content
    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    content.set_margin_start(18);
    content.set_margin_end(18);
    content.set_margin_top(12);
    content.set_margin_bottom(18);

    // Metadata header: name + subtitle
    let name_label = gtk::Label::new(Some(name));
    name_label.add_css_class("title-3");
    name_label.set_halign(gtk::Align::Start);
    name_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    content.append(&name_label);

    let summary_text = {
        let summary = adql_summary::short_summary(adql);
        let time = adql_summary::format_saved_at(created_at);
        if time.is_empty() {
            summary
        } else {
            format!("{} · Saved {}", summary, time)
        }
    };
    let summary_label = gtk::Label::new(Some(&summary_text));
    summary_label.add_css_class("dim-label");
    summary_label.add_css_class("caption");
    summary_label.set_halign(gtk::Align::Start);
    content.append(&summary_label);

    content.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

    // Full ADQL in a monospace text view
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_vexpand(true);
    scroll.set_hexpand(true);
    scroll.add_css_class("card");

    let text_view = gtk::TextView::new();
    text_view.set_editable(false);
    text_view.set_monospace(true);
    text_view.set_cursor_visible(false);
    text_view.set_wrap_mode(gtk::WrapMode::Word);
    text_view.set_left_margin(12);
    text_view.set_right_margin(12);
    text_view.set_top_margin(8);
    text_view.set_bottom_margin(8);
    text_view.buffer().set_text(adql);

    scroll.set_child(Some(&text_view));
    content.append(&scroll);

    // Action button row
    let btn_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    btn_row.set_halign(gtk::Align::End);

    let copy_btn = gtk::Button::with_label(crate::tr_en!("Copy"));
    copy_btn.set_icon_name("edit-copy-symbolic");
    btn_row.append(&copy_btn);

    let rename_btn = gtk::Button::with_label(crate::tr_en!("Rename"));
    rename_btn.set_icon_name("document-edit-symbolic");
    btn_row.append(&rename_btn);

    let delete_btn = gtk::Button::with_label(crate::tr_en!("Delete"));
    delete_btn.add_css_class("destructive-action");
    btn_row.append(&delete_btn);

    // Spacer pushes the primary actions to the right
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    btn_row.append(&spacer);

    let load_btn = gtk::Button::with_label(crate::tr_en!("Load into Editor"));
    btn_row.append(&load_btn);

    let run_btn = gtk::Button::with_label(crate::tr_en!("Run Query"));
    run_btn.add_css_class("suggested-action");
    btn_row.append(&run_btn);

    content.append(&btn_row);

    toolbar_view.set_content(Some(&content));
    dialog.set_content(Some(&toolbar_view));

    // ── Wire actions ────────────────────────────────────────────────────────
    let action: Rc<RefCell<SavedQueryAction>> = Rc::new(RefCell::new(SavedQueryAction::None));
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let tx = Rc::new(RefCell::new(Some(tx)));

    let finish = {
        let dialog = dialog.clone();
        let tx = tx.clone();
        Rc::new(move || {
            if let Some(tx) = tx.borrow_mut().take() {
                let _ = tx.send(());
            }
            dialog.close();
        })
    };

    // Copy → copy ADQL to clipboard, keep dialog open
    {
        let adql_for_copy = adql.to_string();
        let dialog_for_toast = dialog.clone();
        copy_btn.connect_clicked(move |btn| {
            if let Some(display) = gtk::gdk::Display::default() {
                let clipboard = display.clipboard();
                clipboard.set_text(&adql_for_copy);
                btn.set_label(crate::tr_en!("Copied!"));
                // Reset the label after a short delay
                let btn = btn.clone();
                glib::timeout_add_seconds_local_once(1, move || {
                    btn.set_label(crate::tr_en!("Copy"));
                });
            }
            let _ = &dialog_for_toast;
        });
    }

    // Rename → show inline rename dialog
    {
        let current_name = name.to_string();
        let dialog_ref = dialog.clone();
        let action = action.clone();
        let finish = finish.clone();
        rename_btn.connect_clicked(move |_| {
            let current_name = current_name.clone();
            let dialog_ref = dialog_ref.clone();
            let action = action.clone();
            let finish = finish.clone();
            glib::spawn_future_local(async move {
                if let Some(new_name) = crate::ui::rename_dialog::show_rename_dialog(
                    &dialog_ref,
                    crate::tr_en!("Rename Query"),
                    &current_name,
                )
                .await
                {
                    *action.borrow_mut() = SavedQueryAction::Rename(new_name);
                    finish();
                }
            });
        });
    }

    // Delete
    {
        let action = action.clone();
        let finish = finish.clone();
        delete_btn.connect_clicked(move |_| {
            *action.borrow_mut() = SavedQueryAction::Delete;
            finish();
        });
    }

    // Load
    {
        let action = action.clone();
        let finish = finish.clone();
        load_btn.connect_clicked(move |_| {
            *action.borrow_mut() = SavedQueryAction::Load;
            finish();
        });
    }

    // Run
    {
        let action = action.clone();
        let finish = finish.clone();
        run_btn.connect_clicked(move |_| {
            *action.borrow_mut() = SavedQueryAction::Run;
            finish();
        });
    }

    // Close via window button
    {
        let tx = tx.clone();
        dialog.connect_close_request(move |_| {
            if let Some(tx) = tx.borrow_mut().take() {
                let _ = tx.send(());
            }
            glib::Propagation::Proceed
        });
    }

    dialog.present();
    let _ = rx.await;
    let result = action.borrow().clone();
    result
}
