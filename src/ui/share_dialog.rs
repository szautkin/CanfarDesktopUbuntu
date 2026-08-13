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
    let dialog = adw::Window::builder()
        .title(crate::tr_fmt!("Share {}", node_name))
        .default_width(480)
        .modal(true)
        .build();

    if let Some(root) = parent.root().and_downcast::<gtk::Window>() {
        dialog.set_transient_for(Some(&root));
    }

    let toolbar_view = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    toolbar_view.add_top_bar(&header);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    content.set_margin_start(18);
    content.set_margin_end(18);
    content.set_margin_top(12);
    content.set_margin_bottom(18);

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

    let btn_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    btn_row.set_halign(gtk::Align::End);
    let cancel_btn = gtk::Button::with_label(crate::tr_en!("Cancel"));
    let save_btn = gtk::Button::with_label(crate::tr_en!("Save"));
    save_btn.add_css_class("suggested-action");
    btn_row.append(&cancel_btn);
    btn_row.append(&save_btn);
    content.append(&btn_row);

    toolbar_view.set_content(Some(&content));
    dialog.set_content(Some(&toolbar_view));

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

    dialog.present();
    rx.await.ok().flatten()
}
