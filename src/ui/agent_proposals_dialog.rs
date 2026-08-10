//! Agent-proposals review dialog.
//!
//! Write tools an AI agent invokes over MCP enqueue a *proposal* rather than
//! mutating the app: reversible ones auto-apply, but destructive ones wait here
//! for the user to Apply or Reject. Port of `Views/Dialogs/AgentProposalsDialog`.

use crate::state::AppServices;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use libadwaita as adw;
use libadwaita::prelude::*;
use std::rc::Rc;
use std::sync::Arc;

/// Number of proposals currently awaiting user review (for the header badge).
pub fn pending_count(services: &AppServices) -> usize {
    services
        .mcp_host
        .proposals()
        .map(|s| s.pending_count())
        .unwrap_or(0)
}

/// Show the modal proposals-review dialog.
pub fn show_agent_proposals(parent: &impl IsA<gtk::Widget>, services: Arc<AppServices>) {
    let dialog = adw::Window::builder()
        .title("Agent proposals")
        .default_width(540)
        .default_height(460)
        .modal(true)
        .build();
    if let Some(root) = parent.root().and_downcast::<gtk::Window>() {
        dialog.set_transient_for(Some(&root));
    }

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());

    let outer = gtk::Box::new(gtk::Orientation::Vertical, 8);
    outer.set_margin_start(12);
    outer.set_margin_end(12);
    outer.set_margin_top(12);
    outer.set_margin_bottom(12);

    let caption = gtk::Label::new(Some(
        "Destructive changes requested by an AI agent are held here until you approve them. \
Reversible writes are applied automatically.",
    ));
    caption.add_css_class("dim-label");
    caption.set_wrap(true);
    caption.set_xalign(0.0);
    outer.append(&caption);

    let scroll = gtk::ScrolledWindow::new();
    scroll.set_vexpand(true);
    scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::None);
    list.add_css_class("boxed-list");
    scroll.set_child(Some(&list));
    outer.append(&scroll);

    toolbar.set_content(Some(&outer));
    dialog.set_content(Some(&toolbar));

    let content = Rc::new(ProposalsContent { services, list });
    content.refresh();

    dialog.present();
}

struct ProposalsContent {
    services: Arc<AppServices>,
    list: gtk::ListBox,
}

impl ProposalsContent {
    /// Rebuild the list from the current pending proposals.
    fn refresh(self: &Rc<Self>) {
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }

        let pending = self
            .services
            .mcp_host
            .proposals()
            .map(|s| s.pending())
            .unwrap_or_default();

        if pending.is_empty() {
            let empty = adw::ActionRow::new();
            empty.set_title("No pending proposals");
            self.list.append(&empty);
            return;
        }

        for p in pending {
            let row = adw::ActionRow::new();
            row.set_title(&p.kind);
            row.set_subtitle(&p.summary);
            if p.destructive {
                let badge = gtk::Label::new(Some("destructive"));
                badge.add_css_class("error");
                badge.add_css_class("caption");
                row.add_prefix(&badge);
            }

            let reject_btn = gtk::Button::with_label("Reject");
            reject_btn.add_css_class("destructive-action");
            reject_btn.set_valign(gtk::Align::Center);
            {
                let this = self.clone();
                let id = p.id.clone();
                reject_btn.connect_clicked(move |_| {
                    let _ = this.services.mcp_host.reject_proposal(&id);
                    this.refresh();
                });
            }

            let apply_btn = gtk::Button::with_label("Apply");
            apply_btn.add_css_class("suggested-action");
            apply_btn.set_valign(gtk::Align::Center);
            {
                let this = self.clone();
                let id = p.id.clone();
                apply_btn.connect_clicked(move |btn| {
                    btn.set_sensitive(false);
                    let this = this.clone();
                    let id = id.clone();
                    glib::spawn_future_local(async move {
                        let services = this.services.clone();
                        let id2 = id.clone();
                        let result = this
                            .services
                            .spawn(async move {
                                services
                                    .mcp_host
                                    .apply_proposal(services.as_ref(), &id2)
                                    .await
                            })
                            .await;
                        match result {
                            Ok(msg) => this.services.toast.toast(format!("Applied: {msg}")),
                            Err(e) => this.services.toast.toast(format!("Apply failed: {e}")),
                        }
                        this.refresh();
                    });
                });
            }

            row.add_suffix(&reject_btn);
            row.add_suffix(&apply_btn);
            self.list.append(&row);
        }
    }
}
