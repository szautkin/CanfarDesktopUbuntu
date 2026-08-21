//! VOSpace node sharing dialog — set public flag and read/write group access.
//!
//! Prefilled from the node's current ACL (fetched via `get_node`) so that
//! pressing Save never silently revokes existing groups.

use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

/// The access-control choices returned by the share dialog.
pub struct ShareResult {
    pub is_public: bool,
    pub group_read: Vec<String>,
    pub group_write: Vec<String>,
}

fn parse_groups(text: &str) -> Vec<String> {
    text.split_whitespace().map(|s| s.to_string()).collect()
}

/// Show a modal Share dialog prefilled with the node's current ACL.
/// Returns the chosen access-control state, or `None` if cancelled.
pub async fn show_share_dialog(
    parent: &impl IsA<gtk::Widget>,
    node_name: &str,
    is_public: bool,
    group_read: &[String],
    group_write: &[String],
) -> Option<ShareResult> {
    let shell = crate::ui::dialog::Dialog::new(
        &crate::tr_fmt!("Share {}", node_name),
        crate::ui::fit::FORM,
        480,
    );
    let dialog = shell.window.clone();
    let content = shell.content().clone();

    let group = adw::PreferencesGroup::new();
    group.set_description(Some(crate::tr_en!(
        "Grant access by CADC group URI (e.g. ivo://cadc.nrc.ca/gms?MyGroup). \
Separate multiple groups with spaces."
    )));

    let public_row = adw::SwitchRow::new();
    public_row.set_title(crate::tr_en!("Public"));
    public_row.set_subtitle(crate::tr_en!("Anyone can read"));
    public_row.set_active(is_public);
    group.add(&public_row);

    let read_row = adw::EntryRow::new();
    read_row.set_title(crate::tr_en!("Read groups"));
    read_row.set_text(&group_read.join(" "));
    group.add(&read_row);

    let write_row = adw::EntryRow::new();
    write_row.set_title(crate::tr_en!("Write groups"));
    write_row.set_text(&group_write.join(" "));
    group.add(&write_row);

    content.append(&group);

    let cancel_btn = gtk::Button::with_label(crate::tr_en!("Cancel"));
    let save_btn = gtk::Button::with_label(crate::tr_en!("Save"));
    save_btn.add_css_class("suggested-action");
    shell.add_secondary_action(&cancel_btn);
    shell.add_action(&save_btn);

    let (tx, rx) = tokio::sync::oneshot::channel::<Option<ShareResult>>();
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
        let tx = tx.clone();
        let public_row = public_row.clone();
        let read_row = read_row.clone();
        let write_row = write_row.clone();
        save_btn.connect_clicked(move |_| {
            let result = ShareResult {
                is_public: public_row.is_active(),
                group_read: parse_groups(&read_row.text()),
                group_write: parse_groups(&write_row.text()),
            };
            if let Some(tx) = tx.borrow_mut().take() {
                let _ = tx.send(Some(result));
            }
            dialog.close();
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
