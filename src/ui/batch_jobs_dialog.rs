//! Modal dialog showing batch jobs grouped by status.
//!
//! Presented as a tabbed window (Pending/Running/Completed/Failed). Each job
//! row exposes Events, Logs, and Delete actions.

use crate::helpers::batch_jobs_helper::{self, BatchJobState, JobEntry};
use crate::models::job_record::JobRecord;
use crate::models::session::Session;
use crate::state::AppServices;
use crate::ui::delete_dialog::show_delete_dialog;
use crate::ui::session_events_dialog::show_events_dialog;
use crate::ui::space;
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
    entries: Vec<JobEntry>,
    initial_state: BatchJobState,
) {
    let window = gtk::Window::builder()
        .title(crate::tr_en!("Batch Jobs"))
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
        let tab = build_state_tab(&window, services.clone(), &entries, *state);
        notebook.append_page(&tab, Some(&gtk::Label::new(Some(state.label()))));
    }

    // History last: what the four live tabs cannot show, because CANFAR has
    // reaped the jobs and the image-discovery coordinator deletes its own the
    // moment they finish.
    let history_tab = build_history_tab(services.clone());
    notebook.append_page(
        &history_tab,
        Some(&gtk::Label::new(Some(crate::tr_en!("History")))),
    );

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
    all_entries: &[JobEntry],
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

    let filtered = batch_jobs_helper::of_state(all_entries, state);

    if filtered.is_empty() {
        let empty = gtk::Label::new(Some(&crate::tr_fmt!(
            "No {} jobs",
            state.label().to_lowercase()
        )));
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

/// The persistent history: the last finished jobs, with the reason each failure
/// failed, kept after the job itself is gone.
fn build_history_tab(services: Arc<AppServices>) -> gtk::ScrolledWindow {
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_vexpand(true);
    scroll.set_hexpand(true);

    let column = gtk::Box::new(gtk::Orientation::Vertical, space::ROW);
    space::inset(&column, space::CARD);

    let records = services.job_history.load();

    let header = gtk::Box::new(gtk::Orientation::Horizontal, space::CONTROL);
    let caption = gtk::Label::new(Some(&crate::tr_plural!(
        records.len(),
        "{} finished job, kept after CANFAR reaped it",
        "{} finished jobs, kept after CANFAR reaped them"
    )));
    caption.add_css_class("caption");
    caption.add_css_class("dim-label");
    caption.set_halign(gtk::Align::Start);
    caption.set_hexpand(true);
    header.append(&caption);

    let list_box = gtk::ListBox::new();

    let clear_btn = gtk::Button::with_label(crate::tr_en!("Clear history"));
    clear_btn.add_css_class("flat");
    clear_btn.set_tooltip_text(Some(crate::tr_en!(
        "Forget every recorded job, including the reasons they failed"
    )));
    clear_btn.set_sensitive(!records.is_empty());
    {
        let services = services.clone();
        let list_box = list_box.clone();
        let caption = caption.clone();
        clear_btn.connect_clicked(move |btn| {
            if services.job_history.clear().is_err() {
                services
                    .toast
                    .toast(crate::tr_en!("Could not clear the job history"));
                return;
            }
            while let Some(child) = list_box.first_child() {
                list_box.remove(&child);
            }
            list_box.append(&empty_history_row());
            caption.set_text(&crate::tr_plural!(
                0,
                "{} finished job, kept after CANFAR reaped it",
                "{} finished jobs, kept after CANFAR reaped them"
            ));
            btn.set_sensitive(false);
        });
    }
    header.append(&clear_btn);
    column.append(&header);
    list_box.set_selection_mode(gtk::SelectionMode::None);
    list_box.add_css_class("boxed-list");

    if records.is_empty() {
        list_box.append(&empty_history_row());
    } else {
        for record in &records {
            list_box.append(&build_history_row(record));
        }
    }
    column.append(&list_box);

    scroll.set_child(Some(&column));
    scroll
}

/// The placeholder shown when nothing has been recorded — on first open, and
/// again after the history is cleared.
fn empty_history_row() -> gtk::ListBoxRow {
    let empty = gtk::Label::new(Some(crate::tr_en!(
        "No finished jobs recorded yet. Jobs appear here once they succeed \
         or fail, along with the logs and events explaining why."
    )));
    empty.add_css_class("dim-label");
    empty.set_wrap(true);
    empty.set_max_width_chars(60);
    empty.set_margin_top(24);
    empty.set_margin_bottom(24);
    let row = gtk::ListBoxRow::new();
    row.set_child(Some(&empty));
    row
}

/// One remembered job. Failures expand to their reason; successes have none to
/// show, so they stay a single line.
fn build_history_row(record: &JobRecord) -> gtk::Widget {
    let title = format!("{}  ·  {}", record.name, record.outcome.label());
    let dot: gtk::Widget = outcome_dot(record).upcast();
    crate::ui::failure_detail::reason_row(
        &title,
        &history_subtitle(record),
        record.failure_reason.as_deref().unwrap_or(""),
        Some(&dot),
    )
}

/// What kind of job it was, and when it finished.
fn history_subtitle(record: &JobRecord) -> String {
    let when = crate::helpers::discovery_formatting::time_ago(
        &record.finished_at,
        &chrono::Utc::now().to_rfc3339(),
    );
    format!(
        "{}  ·  {}  ·  {when}",
        record.summary(),
        parse_image_label(&record.image)
    )
}

fn outcome_dot(record: &JobRecord) -> gtk::Label {
    let dot = gtk::Label::new(Some("\u{25cf}"));
    dot.add_css_class(record.outcome.css_class());
    dot.set_valign(gtk::Align::Center);
    dot.set_tooltip_text(Some(&record.status));
    dot
}

/// One job row, live or remembered.
///
/// A remembered job has no Events, Logs or Delete: Skaha no longer has it, and
/// a button that fetches nothing is worse than no button. What it has instead
/// is the reason it failed, captured while the job still existed — which is the
/// thing those buttons were for.
fn build_job_row(window: &gtk::Window, services: Arc<AppServices>, entry: JobEntry) -> gtk::Widget {
    match entry {
        JobEntry::Live(job) => build_live_job_row(window, services, job).upcast(),
        JobEntry::Remembered(record) => build_history_row(&record),
    }
}

fn build_live_job_row(
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

    let id_label = gtk::Label::new(Some(&crate::tr_fmt!("ID: {}", job.id)));
    id_label.add_css_class("caption");
    id_label.add_css_class("dim-label");
    id_label.set_halign(gtk::Align::Start);
    vbox.append(&id_label);

    hbox.append(&vbox);

    // Action buttons
    let events_btn = gtk::Button::from_icon_name("text-x-generic-symbolic");
    events_btn.add_css_class("flat");
    events_btn.set_tooltip_text(Some(crate::tr_en!("View events & logs")));
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
    delete_btn.set_tooltip_text(Some(crate::tr_en!("Delete job")));
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
                services.toast.toast(crate::tr_fmt!("Deleted {}", job_name));
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

#[cfg(test)]
mod tests {
    const SOURCE: &str = include_str!("batch_jobs_dialog.rs");

    #[test]
    fn the_dialog_is_what_filters_by_state() {
        // The counterpart to the guard in batch_jobs_view: the tile hands over
        // everything, so the per-tab filtering has to happen here or every tab
        // shows every job.
        let code = crate::testing::code(SOURCE);
        let at = code
            .find("fn build_state_tab")
            .expect("build_state_tab is gone");
        let end = code[at..]
            .find("\n}\n")
            .map(|e| at + e)
            .unwrap_or(code.len());
        assert!(
            code[at..end].contains("of_state(all_entries, state)"),
            "a tab no longer selects the jobs in its own state"
        );
    }

    #[test]
    fn every_state_gets_a_tab_and_so_does_the_history() {
        let code = crate::testing::code(SOURCE);
        for state in ["Pending", "Running", "Completed", "Failed"] {
            assert!(
                code.contains(&format!("BatchJobState::{state}")),
                "no tab for {state}"
            );
        }
        assert!(
            code.contains("build_history_tab("),
            "the History tab is gone"
        );
    }
}
