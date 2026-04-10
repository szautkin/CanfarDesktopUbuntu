//! Dashboard card showing a 2×2 grid of batch job counts.
//!
//! Batch jobs are CANFAR headless sessions grouped by status. Clicking any
//! count tile fires the `on_state_click` callback with the selected state.

use crate::helpers::batch_jobs_helper::{self, BatchJobCounts, BatchJobState};
use crate::models::session::Session;
use crate::state::AppServices;
use crate::ui::card_header::card_header;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

type OnStateClickCb = Rc<RefCell<Option<Box<dyn Fn(BatchJobState, Vec<Session>)>>>>;

pub struct BatchJobsView {
    container: gtk::Box,
    pending_label: gtk::Label,
    running_label: gtk::Label,
    completed_label: gtk::Label,
    failed_label: gtk::Label,
    services: Arc<AppServices>,
    sessions: Rc<RefCell<Vec<Session>>>,
    on_state_click: OnStateClickCb,
    spinner: gtk::Spinner,
}

impl BatchJobsView {
    pub fn new(services: Arc<AppServices>) -> Rc<Self> {
        let container = gtk::Box::new(gtk::Orientation::Vertical, 0);
        container.add_css_class("card");
        container.set_margin_bottom(8);

        let (header, spinner, refresh_btn) = card_header("Batch Jobs");
        container.append(&header);

        let grid = gtk::Grid::new();
        grid.set_row_spacing(8);
        grid.set_column_spacing(8);
        grid.set_row_homogeneous(true);
        grid.set_column_homogeneous(true);
        grid.set_margin_start(16);
        grid.set_margin_end(16);
        grid.set_margin_top(8);
        grid.set_margin_bottom(12);

        let (pending_btn, pending_label) = make_stat_tile("Pending", "batch-dot-pending");
        let (running_btn, running_label) = make_stat_tile("Running", "batch-dot-running");
        let (completed_btn, completed_label) = make_stat_tile("Completed", "batch-dot-completed");
        let (failed_btn, failed_label) = make_stat_tile("Failed", "batch-dot-failed");

        grid.attach(&pending_btn, 0, 0, 1, 1);
        grid.attach(&running_btn, 1, 0, 1, 1);
        grid.attach(&completed_btn, 0, 1, 1, 1);
        grid.attach(&failed_btn, 1, 1, 1, 1);

        container.append(&grid);

        let view = Rc::new(BatchJobsView {
            container,
            pending_label,
            running_label,
            completed_label,
            failed_label,
            services,
            sessions: Rc::new(RefCell::new(Vec::new())),
            on_state_click: Rc::new(RefCell::new(None)),
            spinner,
        });

        // Wire tile clicks
        let states = [
            (pending_btn, BatchJobState::Pending),
            (running_btn, BatchJobState::Running),
            (completed_btn, BatchJobState::Completed),
            (failed_btn, BatchJobState::Failed),
        ];
        for (btn, state) in states {
            let v = view.clone();
            btn.connect_clicked(move |_| {
                let jobs = batch_jobs_helper::filter_by_state(&v.sessions.borrow(), state);
                if let Some(cb) = v.on_state_click.borrow().as_ref() {
                    cb(state, jobs);
                }
            });
        }

        // Refresh button
        {
            let v = view.clone();
            refresh_btn.connect_clicked(move |_| {
                let v = v.clone();
                glib::spawn_future_local(async move {
                    v.refresh().await;
                });
            });
        }

        view
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.container
    }

    pub fn set_on_state_click(&self, cb: impl Fn(BatchJobState, Vec<Session>) + 'static) {
        *self.on_state_click.borrow_mut() = Some(Box::new(cb));
    }

    /// Fetch sessions, filter to headless, update the 4 count labels.
    pub async fn refresh(&self) {
        self.spinner.set_visible(true);
        self.spinner.start();

        let svc = self.services.clone();
        let result = self
            .services
            .spawn(async move {
                let token = svc.get_token().await;
                match token {
                    Some(t) => svc.sessions.get_sessions(&t).await,
                    None => Ok(Vec::new()),
                }
            })
            .await;

        self.spinner.stop();
        self.spinner.set_visible(false);

        match result {
            Ok(sessions) => {
                let counts = batch_jobs_helper::group_by_state(&sessions);
                *self.sessions.borrow_mut() = sessions;
                self.update_counts(counts);
            }
            Err(_) => {
                // Silently keep previous counts on failure
            }
        }
    }

    fn update_counts(&self, counts: BatchJobCounts) {
        self.pending_label.set_text(&counts.pending.to_string());
        self.running_label.set_text(&counts.running.to_string());
        self.completed_label.set_text(&counts.completed.to_string());
        self.failed_label.set_text(&counts.failed.to_string());
    }
}

fn make_stat_tile(name: &str, dot_class: &str) -> (gtk::Button, gtk::Label) {
    let btn = gtk::Button::new();
    btn.add_css_class("flat");

    let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    hbox.set_margin_start(8);
    hbox.set_margin_end(8);
    hbox.set_margin_top(6);
    hbox.set_margin_bottom(6);

    let dot = gtk::Label::new(Some("●"));
    dot.add_css_class(dot_class);
    hbox.append(&dot);

    let count_label = gtk::Label::new(Some("0"));
    count_label.add_css_class("title-2");
    count_label.set_hexpand(true);
    count_label.set_halign(gtk::Align::End);
    hbox.append(&count_label);

    let name_label = gtk::Label::new(Some(name));
    name_label.add_css_class("caption");
    name_label.add_css_class("dim-label");
    hbox.append(&name_label);

    btn.set_child(Some(&hbox));
    (btn, count_label)
}
