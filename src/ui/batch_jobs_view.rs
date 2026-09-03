//! Dashboard card showing a 2×2 grid of batch job counts.
//!
//! Batch jobs are CANFAR headless sessions grouped by status. Clicking any
//! count tile fires the `on_state_click` callback with the selected state.

use crate::helpers::batch_jobs_helper::{self, BatchJobCounts, BatchJobState, JobEntry};
use crate::models::job_record::{JobOrigin, JobOutcome, JobRecord};
use crate::models::session::Session;
use crate::state::AppServices;
use crate::ui::poll;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

type OnStateClickCb = Rc<RefCell<Option<Box<dyn Fn(BatchJobState, Vec<JobEntry>)>>>>;

/// Kind of terminal transition detected between two polls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JobTransition {
    Completed,
    Failed,
}

/// A single detected job transition, carrying what a notification needs and
/// what the history needs.
#[derive(Debug, Clone, PartialEq, Eq)]
struct JobTransitionEvent {
    id: String,
    name: String,
    /// Shortened for the notification body.
    image: String,
    /// The full reference, for the record — a history row that says "terminal"
    /// cannot tell you which registry or tag it came from.
    full_image: String,
    /// Skaha's own status word, kept verbatim.
    status: String,
    started_at: String,
    kind: JobTransition,
}

pub struct BatchJobsView {
    container: gtk::Box,
    pending_label: gtk::Label,
    running_label: gtk::Label,
    completed_label: gtk::Label,
    failed_label: gtk::Label,
    countdown_label: gtk::Label,
    services: Arc<AppServices>,
    /// Live jobs plus the finished ones we remember — what the tiles count and
    /// what the dialog lists.
    entries: Rc<RefCell<Vec<JobEntry>>>,
    /// What the last poll saw, which is what the next interval is chosen from.
    /// Set in `refresh`, where the jobs are in hand with their names —
    /// `prev_states` keeps only statuses, and the probe filter needs the name.
    ///
    /// `awaiting`: a job of the user's can still change state, so a
    /// notification is still to come. `changed`: something actually moved since
    /// the previous poll.
    awaiting: std::cell::Cell<bool>,
    changed: std::cell::Cell<bool>,
    /// Previous status keyed by job id, used to diff transitions between polls.
    prev_states: Rc<RefCell<HashMap<String, String>>>,
    on_state_click: OnStateClickCb,
    spinner: gtk::Spinner,
}

impl BatchJobsView {
    pub fn new(services: Arc<AppServices>) -> Rc<Self> {
        let card = crate::ui::card::Card::new("Batch Jobs");
        let container = card.widget.clone();
        let header = card.header.clone();
        let spinner = card.spinner.clone();
        let refresh_btn = card.with_refresh();

        // Small countdown hint inserted right after the title, before the
        // spinner/refresh button (mirrors SessionListView's countdown label).
        let countdown_label = gtk::Label::new(None);
        countdown_label.add_css_class("dim-label");
        countdown_label.add_css_class("caption");
        if let Some(first) = header.first_child() {
            header.insert_child_after(&countdown_label, Some(&first));
        }

        let grid = gtk::Grid::new();
        grid.set_row_spacing(6);
        grid.set_column_spacing(6);
        grid.set_row_homogeneous(true);
        grid.set_column_homogeneous(true);
        grid.set_margin_bottom(12);

        // Name and colour both come from the state itself. Spelling the four
        // CSS classes out here duplicated a mapping `BatchJobState` already
        // owns, and the four names reached the user untranslated.
        let (pending_btn, pending_label) = make_stat_tile(BatchJobState::Pending);
        let (running_btn, running_label) = make_stat_tile(BatchJobState::Running);
        let (completed_btn, completed_label) = make_stat_tile(BatchJobState::Completed);
        let (failed_btn, failed_label) = make_stat_tile(BatchJobState::Failed);

        grid.attach(&pending_btn, 0, 0, 1, 1);
        grid.attach(&running_btn, 1, 0, 1, 1);
        grid.attach(&completed_btn, 0, 1, 1, 1);
        grid.attach(&failed_btn, 1, 1, 1, 1);

        card.content.append(&grid);

        let view = Rc::new(BatchJobsView {
            container,
            pending_label,
            running_label,
            completed_label,
            failed_label,
            countdown_label,
            services,
            entries: Rc::new(RefCell::new(Vec::new())),
            // Assume so until the first poll says otherwise: the alternative is
            // starting slow on exactly the case that needs to be fast.
            awaiting: std::cell::Cell::new(true),
            changed: std::cell::Cell::new(true),
            prev_states: Rc::new(RefCell::new(HashMap::new())),
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
                // EVERY job, and which tab to open on — not just this tile's.
                //
                // This passed `of_state(...)`, so the dialog built its four
                // tabs from a list already filtered to one of them: whichever
                // tile you clicked had rows and the other three were empty,
                // whatever the counts beside them said.
                let jobs = v.entries.borrow().clone();
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

        // Auto-poll: a single long-lived loop that counts down and then
        // refreshes, forever, while the view is alive. A weak ref lets the loop
        // stop cleanly once the view is dropped.
        {
            let weak = Rc::downgrade(&view);
            let countdown_label = view.countdown_label.clone();
            glib::spawn_future_local(async move {
                // Start on the busy cadence: the first poll is what populates
                // the state map, and until it has run we do not know whether
                // anything is in flight.
                let mut cadence = poll::Cadence::new(poll::JOBS_WATCH_SECS);
                loop {
                    poll::countdown(&countdown_label, cadence.secs()).await;
                    match weak.upgrade() {
                        Some(v) => {
                            v.load(false).await;
                            // The interval is the upper bound on how late the
                            // next notification can be, so it follows the
                            // evidence rather than being a constant. See
                            // ui::poll.
                            cadence.observe(v.awaiting.get(), v.changed.get());
                        }
                        None => break,
                    }
                }
            });
        }

        view
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.container
    }

    pub fn set_on_state_click(&self, cb: impl Fn(BatchJobState, Vec<JobEntry>) + 'static) {
        *self.on_state_click.borrow_mut() = Some(Box::new(cb));
    }

    /// Fetch sessions, filter to headless, update the 4 count labels, and fire
    /// desktop notifications for any Pending/Running → Succeeded/Failed
    /// transition detected since the previous poll.
    /// Fetch the job list and render it, showing the spinner.
    ///
    /// For anything the user asked for: the spinner is the acknowledgement that
    /// the click landed.
    pub async fn refresh(&self) {
        self.load(true).await;
    }

    /// Fetch and render.
    ///
    /// `announce` draws the spinner. The background poller passes `false`: it
    /// can now come round as often as every few seconds, and a spinner blinking
    /// on its own a dozen times a minute reads as the app doing something the
    /// user did not ask for.
    async fn load(&self, announce: bool) {
        if announce {
            self.spinner.set_visible(true);
            self.spinner.start();
        }

        // Snapshot previous states before the async fetch (never hold the
        // RefCell borrow across an await point).
        let old_states = self.prev_states.borrow().clone();

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

        if announce {
            self.spinner.stop();
            self.spinner.set_visible(false);
        }

        match result {
            Ok(sessions) => {
                // Diff transitions against the previous poll (headless jobs only)
                // and record the new state map for the next comparison.
                let jobs: Vec<Session> = sessions
                    .iter()
                    .filter(|s| s.is_headless())
                    .cloned()
                    .collect();
                let events = detect_transitions(&old_states, &jobs);
                let new_states: HashMap<String, String> = jobs
                    .iter()
                    .map(|s| (s.id.clone(), s.status.clone()))
                    .collect();
                // Not just the transitions: a job appearing or being reaped is
                // movement too, and means the next one is probably close.
                self.awaiting.set(awaits_transition(&jobs));
                self.changed.set(new_states != old_states);
                *self.prev_states.borrow_mut() = new_states;

                self.fire_notifications(&events);
                // Before CANFAR reaps them. A job that has finished is on
                // borrowed time in the listing, and its logs and events go with
                // it — so the moment we notice it ended is the only moment we
                // can still find out why. Recorded BEFORE the counts are taken,
                // so a job that just finished is counted this poll rather than
                // the next.
                self.remember(&events).await;

                // Live jobs and remembered ones together. Counting only the
                // listing left Completed and Failed reading zero permanently:
                // CANFAR reaps finished headless jobs, and the image-discovery
                // coordinator deletes its own probes within seconds.
                // Read off the GTK thread. `load` parses the whole history
                // file — fifty records, each carrying up to 4,000 characters of
                // a failed job's logs — and this runs on a 45-second timer, so
                // it was a periodic parse of a couple of hundred kilobytes
                // between frames.
                let history = Arc::clone(&self.services.job_history);
                let history = self.services.spawn(async move { history.load() }).await;
                let entries =
                    batch_jobs_helper::merge(&sessions, &history, &chrono::Utc::now().to_rfc3339());
                self.update_counts(batch_jobs_helper::count_by_state(&entries));
                *self.entries.borrow_mut() = entries;
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

    /// Write finished jobs into the persistent history, fetching the reason for
    /// any that failed.
    ///
    /// The reason has to be fetched HERE. Skaha reaps finished headless jobs,
    /// and once a job is gone so are its logs and its events — the Batch Jobs
    /// dialog would offer a Logs button that returned nothing.
    async fn remember(&self, events: &[JobTransitionEvent]) {
        for ev in events {
            let outcome = match ev.kind {
                JobTransition::Completed => JobOutcome::Succeeded,
                JobTransition::Failed => JobOutcome::Failed,
            };

            let failure_reason = match ev.kind {
                JobTransition::Completed => None,
                JobTransition::Failed => Some(self.failure_reason(&ev.id).await),
            };

            let record = JobRecord {
                id: ev.id.clone(),
                name: ev.name.clone(),
                image: ev.full_image.clone(),
                origin: JobOrigin::User,
                outcome,
                status: ev.status.clone(),
                started_at: ev.started_at.clone(),
                finished_at: chrono::Utc::now().to_rfc3339(),
                failure_reason,
                target_image: None,
            };
            // Off the GTK thread. `record` is a read-modify-write of the whole
            // history file, and this loop runs once per finished job on a
            // 45-second poll — a burst of them was doing synchronous disk I/O
            // on the main loop, between frames.
            let history = Arc::clone(&self.services.job_history);
            let _ = self
                .services
                .spawn(async move {
                    let _ = history.record(record);
                })
                .await;
        }
    }

    /// Why a job failed, in its own words — fetched now, while the job still
    /// exists to be asked.
    async fn failure_reason(&self, job_id: &str) -> String {
        let svc = self.services.clone();
        let id = job_id.to_string();
        self.services
            .spawn(async move {
                match svc.get_token().await {
                    Some(t) => svc.sessions.get_diagnostics(&t, &id).await,
                    None => String::new(),
                }
            })
            .await
    }

    /// Route detected transitions to desktop notifications via the shared
    /// NotificationService. Needs the GIO application from the widget tree; if
    /// the view is not yet rooted in a window we simply skip (no panic).
    fn fire_notifications(&self, events: &[JobTransitionEvent]) {
        if events.is_empty() {
            return;
        }
        let Some(app) = self
            .container
            .root()
            .and_then(|r| r.downcast::<gtk::Window>().ok())
            .and_then(|w| w.application())
        else {
            return;
        };
        let gio_app: &gtk4::gio::Application = app.upcast_ref();

        for ev in events {
            match ev.kind {
                JobTransition::Completed => {
                    self.services
                        .notifications
                        .notify_job_completed(gio_app, &ev.id, &ev.name, &ev.image);
                }
                JobTransition::Failed => {
                    self.services
                        .notifications
                        .notify_job_failed(gio_app, &ev.id, &ev.name, &ev.image);
                }
            }
        }
    }
}

/// Can any of these jobs still change state — that is, is a notification still
/// to come?
///
/// Deliberately the same two filters `detect_transitions` applies, because the
/// question is precisely "will that function have anything to report next
/// time": the user's own jobs only (a coordinator probe never notifies, and a
/// catalogue sweep would otherwise hold the card at the busy interval for half
/// an hour), and only those not already finished.
fn awaits_transition(jobs: &[Session]) -> bool {
    jobs.iter().filter(|j| !is_app_probe(&j.name)).any(|j| {
        !matches!(
            BatchJobState::from_status(&j.status),
            BatchJobState::Completed | BatchJobState::Failed
        )
    })
}

/// Diff the previous status map against the current headless jobs and return
/// the terminal transitions (Pending/Running → Succeeded/Failed) that just
/// happened. A job whose *previous* status was already terminal is skipped so
/// we never re-notify (mirrors `BatchJobsControl.DetectTransitions`).
fn detect_transitions(
    old_states: &HashMap<String, String>,
    jobs: &[Session],
) -> Vec<JobTransitionEvent> {
    let mut out = Vec::new();
    for job in jobs {
        // Not the user's job — skip it entirely.
        //
        // The image-discovery coordinator launches its own headless jobs, and
        // they land in the same listing as anything the user submitted. Every
        // probe that failed raised a desktop notification about "a batch job"
        // the user never launched, and inspecting a catalogue means dozens of
        // them. It also filled the remembered history with probe rows, pushing
        // out the user's own.
        if is_app_probe(&job.name) {
            continue;
        }
        let Some(old_status) = old_states.get(&job.id) else {
            // Unknown job (first time seen) — nothing to compare against.
            continue;
        };
        // Skip if it was already in a terminal state last poll.
        if matches!(
            BatchJobState::from_status(old_status),
            BatchJobState::Completed | BatchJobState::Failed
        ) {
            continue;
        }
        let kind = match BatchJobState::from_status(&job.status) {
            BatchJobState::Completed => JobTransition::Completed,
            BatchJobState::Failed => JobTransition::Failed,
            _ => continue,
        };
        out.push(JobTransitionEvent {
            id: job.id.clone(),
            name: job.name.clone(),
            image: short_image(&job.image),
            full_image: job.image.clone(),
            status: job.status.clone(),
            started_at: job.start_time.clone(),
            kind,
        });
    }
    out
}

/// Whether this job is one the app launched for itself.
///
/// The coordinator names its probes with these prefixes
/// (`ImageDiscoveryCoordinator::run_discovery`): `vp-` for the in-target probe,
/// `vi-` for the syft inspector. Matching on the name is what there is — Skaha
/// has no notion of who asked, and the local `JobOrigin` is only known once a
/// record has already been written.
fn is_app_probe(name: &str) -> bool {
    name.starts_with("vp-") || name.starts_with("vi-")
}

/// Reduce a fully-qualified image reference to its last path segment.
fn short_image(image: &str) -> String {
    image.rsplit('/').next().unwrap_or(image).to_string()
}

fn make_stat_tile(state: BatchJobState) -> (gtk::Button, gtk::Label) {
    let btn = gtk::Button::new();
    btn.add_css_class("flat");

    let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    hbox.set_margin_start(6);
    hbox.set_margin_end(6);
    hbox.set_margin_top(6);
    hbox.set_margin_bottom(6);

    // A compact centered chip — ● 3 Pending — with no hexpand, so the dot,
    // count and label stay together instead of stretching across the grid cell.
    hbox.set_halign(gtk::Align::Center);

    let dot = gtk::Label::new(Some("●"));
    dot.add_css_class(state.css_class());
    hbox.append(&dot);

    let count_label = gtk::Label::new(Some("0"));
    count_label.add_css_class("title-3");
    hbox.append(&count_label);

    let name_label = gtk::Label::new(Some(crate::tr_en!(state.label())));
    name_label.add_css_class("caption");
    name_label.add_css_class("dim-label");
    hbox.append(&name_label);

    btn.set_child(Some(&hbox));
    (btn, count_label)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE: &str = include_str!("batch_jobs_view.rs");

    #[test]
    fn a_tile_hands_the_dialog_every_job_not_just_its_own() {
        // The click handler passed `of_state(...)`, so the dialog built its
        // four tabs from a list already filtered to one of them: whichever tile
        // you clicked had rows and the other three were empty, whatever the
        // counts beside them said. Filtering is the DIALOG's job — it has a tab
        // per state and does it there.
        let code = crate::testing::code(SOURCE);
        let at = code
            .find("btn.connect_clicked")
            .expect("the tiles no longer open the dialog");
        // Code only: the comment above the handler explains the bug by naming
        // it, and prose about a defect is not the defect.
        let handler = crate::testing::without_comments(&code[at..(at + 700).min(code.len())]);
        assert!(
            !handler.contains("of_state("),
            "the tile filters before the dialog does, so three of its four tabs \
             are always empty"
        );
        assert!(
            handler.contains("cb(state, jobs)"),
            "the dialog is no longer told which tab to open on"
        );
    }

    #[test]
    fn a_transition_carries_what_a_history_record_needs() {
        // The record needs the full image reference and Skaha's own status
        // word; the notification only ever needed a short name, and building
        // the record from that would lose the registry and the tag.
        let old: HashMap<String, String> = [("j1".to_string(), "Running".to_string())].into();
        let jobs = vec![job("j1", "Failed")];
        let events = detect_transitions(&old, &jobs);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].full_image, "images.canfar.net/skaha/base:1.0");
        assert_eq!(events[0].status, "Failed");
        assert_eq!(events[0].image, "base:1.0", "the short form is still there");
    }

    fn job(id: &str, status: &str) -> Session {
        Session {
            id: id.into(),
            userid: String::new(),
            image: "images.canfar.net/skaha/base:1.0".into(),
            session_type: "headless".into(),
            status: status.into(),
            name: format!("batch-{id}"),
            start_time: String::new(),
            expiry_time: String::new(),
            connect_url: String::new(),
            requested_ram: String::new(),
            requested_cpu_cores: String::new(),
            requested_gpu_cores: String::new(),
            ram_in_use: String::new(),
            cpu_cores_in_use: String::new(),
            is_fixed_resources: true,
        }
    }

    fn states(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(id, s)| ((*id).to_string(), (*s).to_string()))
            .collect()
    }

    #[test]
    fn the_app_does_not_notify_about_its_own_probes() {
        // Inspecting a catalogue launches hundreds of headless jobs. Every one
        // that failed raised a desktop notification about a "batch job" the
        // user never submitted, and wrote a history row that pushed out one
        // they did.
        let old = states(&[("p1", "Running"), ("p2", "Running"), ("mine", "Running")]);
        let mut probe_in_target = job("p1", "Failed");
        probe_in_target.name = "vp-images-canfar-net-skaha-base-1-0-abcd1234".into();
        let mut probe_inspector = job("p2", "Failed");
        probe_inspector.name = "vi-images-canfar-net-canucs-canucs-1-2-4-beef".into();
        let mine = job("mine", "Failed");

        let events = detect_transitions(&old, &[probe_in_target, probe_inspector, mine]);
        assert_eq!(events.len(), 1, "a probe was reported as the user's job");
        assert_eq!(events[0].id, "mine");
    }

    #[test]
    fn a_users_job_named_like_nothing_in_particular_still_reports() {
        // The prefixes are specific; an ordinary name must not be swallowed.
        assert!(!is_app_probe("batch-1"));
        assert!(!is_app_probe("visualisation"));
        assert!(!is_app_probe("vprobe-mine"));
        assert!(is_app_probe("vp-x"));
        assert!(is_app_probe("vi-x"));
    }

    #[test]
    fn short_image_takes_last_segment() {
        assert_eq!(short_image("images.canfar.net/skaha/base:1.0"), "base:1.0");
        assert_eq!(short_image("plainimage"), "plainimage");
    }

    #[test]
    fn first_poll_has_no_transitions() {
        // No previous state → nothing to compare against.
        let jobs = vec![job("a", "Succeeded"), job("b", "Failed")];
        assert!(detect_transitions(&HashMap::new(), &jobs).is_empty());
    }

    #[test]
    fn running_to_succeeded_and_pending_to_failed() {
        let old = states(&[("a", "Running"), ("b", "Pending")]);
        let jobs = vec![job("a", "Succeeded"), job("b", "Error")];
        let events = detect_transitions(&old, &jobs);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].kind, JobTransition::Completed);
        assert_eq!(events[0].id, "a");
        assert_eq!(events[1].kind, JobTransition::Failed);
        assert_eq!(events[1].id, "b");
    }

    #[test]
    fn already_terminal_is_not_renotified() {
        // Both were terminal last poll and remain terminal → no new events.
        let old = states(&[("a", "Succeeded"), ("b", "Failed")]);
        let jobs = vec![job("a", "Succeeded"), job("b", "Failed")];
        assert!(detect_transitions(&old, &jobs).is_empty());
    }

    #[test]
    fn still_running_produces_no_transition() {
        let old = states(&[("a", "Pending")]);
        let jobs = vec![job("a", "Running")];
        assert!(detect_transitions(&old, &jobs).is_empty());
    }
}
