//! Modal dialog showing batch jobs grouped by status.
//!
//! Presented as a tabbed window (Pending/Running/Completed/Failed). Each job
//! row exposes Events, Logs, and Delete actions.

use crate::helpers::batch_jobs_helper::{self, BatchJobState};
use crate::models::session::Session;
use crate::state::AppServices;
use crate::ui::delete_dialog::show_delete_dialog;
use crate::ui::session_events_dialog::show_events_dialog;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

/// Show a modal batch jobs dialog with the given sessions (already filtered to headless).
/// `initial_state` selects which tab is visible on open.
pub async fn show_batch_jobs_dialog(
    parent: &impl IsA<gtk::Widget>,
    services: Arc<AppServices>,
    sessions: Vec<Session>,
    initial_state: BatchJobState,
) {
    let window = gtk::Window::builder()
        .title("Batch Jobs")
        .default_width(720)
        .default_height(520)
        .modal(true)
        .build();

    if let Some(root) = parent.root().and_then(|r| r.downcast::<gtk::Window>().ok()) {
        window.set_transient_for(Some(&root));
    }

    let notebook = gtk::Notebook::new();

    let states = [
        BatchJobState::Pending,
        BatchJobState::Running,
        BatchJobState::Completed,
        BatchJobState::Failed,
    ];

    for state in &states {
        let tab = build_state_tab(&window, services.clone(), &sessions, *state);
        notebook.append_page(&tab, Some(&gtk::Label::new(Some(state.label()))));
    }

    let initial_idx = states.iter().position(|s| *s == initial_state).unwrap_or(0);
    notebook.set_current_page(Some(initial_idx as u32));

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

fn build_state_tab(
    window: &gtk::Window,
    services: Arc<AppServices>,
    all_sessions: &[Session],
    state: BatchJobState,
) -> gtk::ScrolledWindow {
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_vexpand(true);
    scroll.set_hexpand(true);

    let list_box = gtk::ListBox::new();
    list_box.set_selection_mode(gtk::SelectionMode::None);
    list_box.add_css_class("boxed-list");
    list_box.set_margin_start(12);
    list_box.set_margin_end(12);
    list_box.set_margin_top(12);
    list_box.set_margin_bottom(12);

    let filtered = batch_jobs_helper::filter_by_state(all_sessions, state);

    if filtered.is_empty() {
        let empty = gtk::Label::new(Some(&format!("No {} jobs", state.label().to_lowercase())));
        empty.add_css_class("dim-label");
        empty.set_margin_top(24);
        empty.set_margin_bottom(24);
        let row = gtk::ListBoxRow::new();
        row.set_child(Some(&empty));
        list_box.append(&row);
    } else {
        for job in filtered {
            list_box.append(&build_job_row(window, services.clone(), job));
        }
    }

    scroll.set_child(Some(&list_box));
    scroll
}

fn build_job_row(
    window: &gtk::Window,
    services: Arc<AppServices>,
    job: Session,
) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    hbox.set_margin_start(8);
    hbox.set_margin_end(8);
    hbox.set_margin_top(8);
    hbox.set_margin_bottom(8);

    // Title + image column
    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 2);
    vbox.set_hexpand(true);

    let name_label = gtk::Label::new(Some(&job.name));
    name_label.add_css_class("heading");
    name_label.set_halign(gtk::Align::Start);
    name_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    vbox.append(&name_label);

    let short_image = parse_image_label(&job.image);
    let image_label = gtk::Label::new(Some(&short_image));
    image_label.add_css_class("caption");
    image_label.add_css_class("dim-label");
    image_label.set_halign(gtk::Align::Start);
    image_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    vbox.append(&image_label);

    let id_label = gtk::Label::new(Some(&format!("ID: {}", job.id)));
    id_label.add_css_class("caption");
    id_label.add_css_class("dim-label");
    id_label.set_halign(gtk::Align::Start);
    vbox.append(&id_label);

    hbox.append(&vbox);

    // Action buttons
    let events_btn = gtk::Button::from_icon_name("text-x-generic-symbolic");
    events_btn.add_css_class("flat");
    events_btn.set_tooltip_text(Some("View events & logs"));
    {
        let window = window.clone();
        let services = services.clone();
        let job_id = job.id.clone();
        let job_name = job.name.clone();
        events_btn.connect_clicked(move |_| {
            let window = window.clone();
            let services = services.clone();
            let job_id = job_id.clone();
            let job_name = job_name.clone();
            glib::spawn_future_local(async move {
                let svc = services.clone();
                let id = job_id.clone();
                let events_result = services
                    .spawn(async move {
                        let token = svc.get_token().await;
                        match token {
                            Some(t) => svc.sessions.get_events(&t, &id).await,
                            None => Ok(String::new()),
                        }
                    })
                    .await
                    .unwrap_or_default();

                let svc = services.clone();
                let id = job_id.clone();
                let logs_result = services
                    .spawn(async move {
                        let token = svc.get_token().await;
                        match token {
                            Some(t) => svc.sessions.get_logs(&t, &id).await,
                            None => Ok(String::new()),
                        }
                    })
                    .await
                    .unwrap_or_default();

                show_events_dialog(&window, &job_name, &events_result, &logs_result).await;
            });
        });
    }
    hbox.append(&events_btn);

    let delete_btn = gtk::Button::from_icon_name("user-trash-symbolic");
    delete_btn.add_css_class("flat");
    delete_btn.add_css_class("destructive-action");
    delete_btn.set_tooltip_text(Some("Delete job"));
    {
        let window = window.clone();
        let services = services.clone();
        let job_id = job.id.clone();
        let job_name = job.name.clone();
        delete_btn.connect_clicked(move |btn| {
            let window = window.clone();
            let services = services.clone();
            let job_id = job_id.clone();
            let job_name = job_name.clone();
            let btn = btn.clone();
            glib::spawn_future_local(async move {
                let confirmed = show_delete_dialog(&window, &job_name).await;
                if !confirmed {
                    return;
                }
                let svc = services.clone();
                let id = job_id.clone();
                let _ = services
                    .spawn(async move {
                        let token = svc.get_token().await;
                        if let Some(t) = token {
                            let _ = svc.sessions.delete_session(&t, &id).await;
                        }
                    })
                    .await;
                btn.set_sensitive(false);
                services.toast.toast(&format!("Deleted {}", job_name));
            });
        });
    }
    hbox.append(&delete_btn);

    row.set_child(Some(&hbox));
    row
}

fn parse_image_label(full_image: &str) -> String {
    full_image
        .rsplit('/')
        .next()
        .unwrap_or(full_image)
        .to_string()
}
