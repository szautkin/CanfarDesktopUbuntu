use crate::models::session::INTERACTIVE_SESSION_TYPES;
use crate::models::Session;
use crate::state::AppServices;
use crate::ui::poll;
use crate::ui::session_card::{ActionCallback, SessionAction, SessionCard};
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use libadwaita as adw;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

type OptionalCallback<T> = Rc<RefCell<Option<Box<dyn Fn(T)>>>>;

/// Session-type choices in the strip's filter dropdown, `All` first.
///
/// One list: it builds the dropdown AND decodes the selection in both places
/// that read it. It used to be written out three times, and a type added to the
/// visible list alone would have filtered by whatever sat at that index in the
/// stale copies.
const SESSION_FILTER_TYPES: [&str; 6] = [
    SESSION_FILTER_ALL,
    INTERACTIVE_SESSION_TYPES[0],
    INTERACTIVE_SESSION_TYPES[1],
    INTERACTIVE_SESSION_TYPES[2],
    INTERACTIVE_SESSION_TYPES[3],
    INTERACTIVE_SESSION_TYPES[4],
];

/// The `All` entry — no filter, rather than a type named "All".
const SESSION_FILTER_ALL: &str = "All";

/// Which session type the filter dropdown's `index` selects, or `None` for
/// "All" — and for an index the list does not have, which is the safe reading:
/// showing everything beats hiding sessions the user has running.
fn selected_session_filter(index: usize) -> Option<&'static str> {
    match SESSION_FILTER_TYPES.get(index) {
        Some(&SESSION_FILTER_ALL) | None => None,
        Some(session_type) => Some(session_type),
    }
}

pub struct SessionListView {
    pub container: gtk::Box,
    cards_box: gtk::Box,
    /// The sessions the currently-rendered cards were built from, in display
    /// order. Compared against each poll so unchanged cards are left alone —
    /// see `reconcile_cards`.
    rendered: RefCell<Vec<Session>>,
    empty_label: gtk::Label,
    loading_spinner: gtk::Spinner,
    count_label: gtk::Label,
    countdown_label: gtk::Label,
    filter_dropdown: gtk::DropDown,
    sessions: Rc<RefCell<Vec<Session>>>,
    services: Arc<AppServices>,
    on_action: ActionCallback,
    on_sessions_changed: OptionalCallback<usize>,
    /// What the last update saw, which is what the poller's next interval is
    /// chosen from: `pending` — a session is still coming up, so a "ready"
    /// notification is still to come; `changed` — something actually moved.
    pending: std::cell::Cell<bool>,
    changed: std::cell::Cell<bool>,
    /// Asked to open the launch form. Set by the dashboard, which owns the
    /// form and the modal it lives in.
    on_launch_requested: OptionalCallback<()>,
    /// Whether the poll loop is already running.
    ///
    /// The loop no longer stops when nothing is pending, so it can no longer
    /// rely on ending to keep itself unique. Without this, every call to
    /// `update_sessions` would start another one and they would accumulate for
    /// the life of the window.
    polling: std::cell::Cell<bool>,
}

impl SessionListView {
    pub fn new(services: Arc<AppServices>) -> Rc<Self> {
        let card = crate::ui::card::Card::new(crate::tr_en!("Active Sessions"));
        let container = card.widget.clone();
        container.set_vexpand(true);
        let header = card.header.clone();
        let loading_spinner = card.spinner.clone();
        let refresh_btn = card.with_refresh();

        // The way to start a session, next to the list of the ones you have.
        //
        // It used to share the job with a floating button over the Portal,
        // which is why it was deliberately understated. The floating button is
        // gone — it carried no label, sat away from anything it related to, and
        // a user who had not noticed it had no way in — so this is now the one
        // primary action on the page and is styled as one.
        let launch_btn = gtk::Button::builder()
            .child(
                &adw::ButtonContent::builder()
                    .icon_name("list-add-symbolic")
                    .label(crate::tr_en!("Launch session"))
                    .build(),
            )
            .valign(gtk::Align::Center)
            .build();
        // The accent, which is the one thing on this page asking to be pressed.
        // `suggested-action` rather than a colour of its own: it takes the
        // theme's accent, so it stays right when the accent changes and it is
        // the same red as the rest of the app's primaries.
        launch_btn.add_css_class("suggested-action");
        launch_btn.set_tooltip_text(Some(crate::tr_en!("Start a new interactive session")));
        header.append(&launch_btn);

        let countdown_label = gtk::Label::new(None);
        countdown_label.add_css_class("dim-label");
        countdown_label.add_css_class("caption");
        countdown_label.set_visible(false);
        // Insert after title
        header.insert_child_after(&countdown_label, Some(&header.first_child().unwrap()));

        let count_label = gtk::Label::new(Some(crate::tr_en!("0 sessions")));
        count_label.add_css_class("dim-label");
        count_label.add_css_class("caption");
        // Insert after countdown
        header.insert_child_after(&count_label, Some(&countdown_label));

        let filter_types = gtk::StringList::new(&SESSION_FILTER_TYPES);
        let filter_dropdown = gtk::DropDown::new(Some(filter_types), gtk::Expression::NONE);
        filter_dropdown.set_valign(gtk::Align::Center);
        header.insert_child_after(&filter_dropdown, Some(&count_label));

        let empty_label = gtk::Label::new(Some(crate::tr_en!("No active sessions")));
        empty_label.add_css_class("dim-label");
        empty_label.set_margin_top(32);
        empty_label.set_margin_bottom(32);
        empty_label.set_visible(false);
        card.content.append(&empty_label);

        let scrolled = gtk::ScrolledWindow::new();
        scrolled.set_hscrollbar_policy(gtk::PolicyType::Automatic);
        // Never scroll vertically: size the strip to the cards' natural height so
        // a card's action row is never clipped mid-button.
        scrolled.set_vscrollbar_policy(gtk::PolicyType::Never);
        scrolled.set_propagate_natural_height(true);

        let cards_box = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        cards_box.set_margin_bottom(12);
        scrolled.set_child(Some(&cards_box));
        card.content.append(&scrolled);

        let on_action: ActionCallback = Rc::new(RefCell::new(Box::new(|_, _| {})));

        let view = Rc::new(SessionListView {
            container,
            cards_box,
            rendered: RefCell::new(Vec::new()),
            empty_label,
            loading_spinner,
            count_label,
            countdown_label,
            filter_dropdown,
            sessions: Rc::new(RefCell::new(Vec::new())),
            services,
            on_action,
            on_sessions_changed: Rc::new(RefCell::new(None)),
            on_launch_requested: Rc::new(RefCell::new(None)),
            // Assume a session may be coming up until the first update says
            // otherwise: the alternative is starting slow on exactly the case
            // that needs to be fast.
            pending: std::cell::Cell::new(true),
            changed: std::cell::Cell::new(true),
            polling: std::cell::Cell::new(false),
        });

        // Filter dropdown
        {
            let view_weak = Rc::downgrade(&view);
            let filter_dropdown = view.filter_dropdown.clone();
            filter_dropdown.connect_selected_notify(move |_| {
                if let Some(view) = view_weak.upgrade() {
                    let sessions = view.sessions.borrow().clone();
                    view.update_sessions(sessions);
                }
            });
        }

        // Refresh button
        {
            let view = view.clone();
            refresh_btn.connect_clicked(move |_| {
                let view = view.clone();
                glib::spawn_future_local(async move {
                    view.refresh().await;
                });
            });
        }

        {
            let cb = view.on_launch_requested.clone();
            launch_btn.connect_clicked(move |_| {
                if let Some(cb) = cb.borrow().as_ref() {
                    cb(());
                }
            });
        }

        // Started here rather than only from `update_sessions`, which runs only
        // after a load succeeds: a first load that fails with nothing cached
        // would otherwise leave the strip with no periodic refresh at all, and
        // the poller is now the only one there is.
        view.start_polling();

        view
    }

    /// The handler is given a `Working` guard with the action and owns it for
    /// the length of the work — see [`crate::ui::busy`].
    pub fn set_on_action(
        &self,
        callback: impl Fn(SessionAction, crate::ui::busy::Working) + 'static,
    ) {
        *self.on_action.borrow_mut() = Box::new(callback);
    }

    /// Register what happens when the header's Launch button is pressed.
    ///
    /// The card does not own the launch form — the dashboard does, along with
    /// the modal it opens in — so this asks rather than acts.
    pub fn set_on_launch_requested(&self, callback: impl Fn() + 'static) {
        *self.on_launch_requested.borrow_mut() = Some(Box::new(move |()| callback()));
    }

    pub fn set_on_sessions_changed(&self, callback: impl Fn(usize) + 'static) {
        *self.on_sessions_changed.borrow_mut() = Some(Box::new(callback));
    }

    /// Fetch the session list and render it, showing the loading spinner.
    ///
    /// For anything the user asked for: the spinner is the acknowledgement that
    /// the click landed.
    pub async fn refresh(self: &Rc<Self>) {
        self.load(true).await;
    }

    /// Fetch and render.
    ///
    /// `announce` draws the loading spinner. The background poller passes
    /// `false`: it runs for the life of the window, and a spinner flashing on
    /// its own every few seconds reads as the app doing something the user did
    /// not ask for.
    async fn load(self: &Rc<Self>, announce: bool) {
        use crate::services::cache_service::CacheKey;
        use crate::services::health_tracker::{ServiceName, ServiceStatus};

        if announce {
            self.loading_spinner.set_visible(true);
            self.loading_spinner.start();

            // Yield so GTK renders the spinner before the async call
            glib::timeout_future(std::time::Duration::from_millis(50)).await;
        }

        let svc = self.services.clone();
        let result = self
            .services
            .spawn(async move {
                let token = svc.get_token().await;
                if let Some(token) = token {
                    svc.sessions.get_sessions(&token).await
                } else {
                    Err(crate::services::ApiError::Unauthorized)
                }
            })
            .await;

        match result {
            Ok(sessions) => {
                // Cache successful refresh only (never cache during/after mutations)
                self.services.cache.write(&CacheKey::Sessions, &sessions);
                self.services
                    .health
                    .set(ServiceName::Sessions, ServiceStatus::Reachable);
                self.update_sessions(sessions);
            }
            Err(e) => {
                // Network failure — serve stale cache if available
                if let Some(entry) = self
                    .services
                    .cache
                    .read::<Vec<Session>>(&CacheKey::Sessions)
                {
                    let time_label = self
                        .services
                        .cache
                        .cached_time_label(&CacheKey::Sessions)
                        .unwrap_or_else(|| "unknown".into());
                    self.update_sessions(entry.data);
                    // Only for a refresh the user asked for. The poller runs
                    // for the life of the window, so an outage would otherwise
                    // raise this toast every interval until the network came
                    // back — the health tracker below is the right place for a
                    // condition that persists, and it is already told.
                    if announce {
                        self.services.toast.toast(crate::tr_fmt!(
                            "Sessions unreachable — cached list from {}",
                            time_label
                        ));
                    }
                }
                self.services.health.set(
                    ServiceName::Sessions,
                    ServiceStatus::Unreachable {
                        since: chrono::Utc::now(),
                        reason: e.to_string(),
                    },
                );
            }
        }

        if announce {
            self.loading_spinner.stop();
            self.loading_spinner.set_visible(false);
        }
    }

    fn active_filter(&self) -> Option<String> {
        selected_session_filter(self.filter_dropdown.selected() as usize).map(str::to_string)
    }

    /// Bring the card strip in line with `visible`, rebuilding only what changed.
    ///
    /// The strip polls every 15 seconds. Clearing and re-appending every card on
    /// each poll reset the horizontal scroll position and dropped hover/focus
    /// four times a minute — while in the common case nothing about the sessions
    /// had changed at all. Mirrors the reference's `ReconcileCards`.
    ///
    /// A card is reused only when its session compares EQUAL, so any visible
    /// field (status, in-use CPU/RAM, expiry) still refreshes; the card is built
    /// in one pass from a `Session`, so equality is the honest test for "does
    /// this need redrawing".
    fn reconcile_cards(&self, visible: &[&Session]) {
        let mut rendered = self.rendered.borrow_mut();

        // Fast path: nothing changed, so touch no widgets at all.
        if rendered.len() == visible.len() && rendered.iter().zip(visible).all(|(a, b)| a == *b) {
            return;
        }

        // Any structural or content change rebuilds from scratch. Reusing widgets
        // across a reorder would need per-field setters on SessionCard; the win
        // that matters is the fast path above, which covers the steady state.
        while let Some(child) = self.cards_box.first_child() {
            self.cards_box.remove(&child);
        }
        for session in visible {
            let card = SessionCard::new(session, self.on_action.clone());
            self.cards_box.append(card.widget());
        }

        rendered.clear();
        rendered.extend(visible.iter().map(|s| (*s).clone()));
    }

    fn update_sessions(self: &Rc<Self>, sessions: Vec<Session>) {
        let filter = self.active_filter();
        // Headless (batch) jobs are shown in the Batch Jobs panel, never here, and
        // never count toward the interactive-session cap (matches the reference).
        let visible: Vec<&Session> = sessions
            .iter()
            .filter(|s| !s.is_headless())
            .filter(|s| match &filter {
                None => true,
                Some(f) => s.session_type.eq_ignore_ascii_case(f),
            })
            .collect();

        let count = sessions.iter().filter(|s| !s.is_headless()).count();
        let count_tmpl = if count == 1 {
            "{} session"
        } else {
            "{} sessions"
        };
        self.count_label
            .set_text(&crate::tr_fmt!(count_tmpl, count));
        self.empty_label.set_visible(visible.is_empty());

        self.reconcile_cards(&visible);

        // Fire desktop notifications for state transitions
        check_notifications(
            &self.sessions.borrow(),
            &sessions,
            &self.services.notifications,
            &self.container,
        );
        // The same question the notification check asks, kept for the poller:
        // movement now means movement is likely again soon.
        self.changed
            .set(differs(&self.sessions.borrow(), &sessions));

        let has_pending = sessions.iter().any(|s| s.is_pending());
        *self.sessions.borrow_mut() = sessions;

        if let Some(ref cb) = *self.on_sessions_changed.borrow() {
            cb(count);
        }

        // Record what the poller chooses its next interval from, and make sure
        // exactly one poller is running. See `start_polling`.
        self.pending.set(has_pending);
        self.start_polling();
    }

    /// Keep the strip in step with the platform, for as long as the window is
    /// open.
    ///
    /// This used to run only while a session was pending, and stop the moment
    /// none was. That left the strip blind whenever the app was merely sitting
    /// there: a session started from another machine never appeared, one that
    /// died was never noticed, and `notify_session_expiring` — which has no
    /// other trigger — could only fire in the accident of some *other* session
    /// being pending at the time.
    ///
    /// So it runs continuously now, and the cadence carries the cost: quick
    /// while a session is coming up, [`poll::IDLE_SECS`] apart when nothing is
    /// in flight and the poll is only there to notice the unexpected.
    fn start_polling(self: &Rc<Self>) {
        if self.polling.replace(true) {
            return;
        }

        let weak = Rc::downgrade(self);
        let countdown_label = self.countdown_label.clone();
        glib::spawn_future_local(async move {
            let mut cadence = poll::Cadence::new(poll::SESSION_WATCH_SECS);
            loop {
                // The countdown is for someone waiting on a session to come up.
                // Ticking away for the rest of the day, next to nothing, is
                // just a moving thing on screen with no meaning.
                let waiting = weak.upgrade().is_some_and(|v| v.pending.get());
                countdown_label.set_visible(waiting);
                if waiting {
                    poll::countdown(&countdown_label, cadence.secs()).await;
                } else {
                    glib::timeout_future_seconds(cadence.secs()).await;
                }

                let Some(view) = weak.upgrade() else { break };
                // `load` calls `update_sessions`, which calls `start_polling`
                // again; the `polling` flag makes that call a no-op rather than
                // a second poller.
                view.load(false).await;
                cadence.observe(view.pending.get(), view.changed.get());
            }
        });
    }

    pub fn session_count(&self) -> usize {
        // Only interactive sessions count toward the 3-session cap; headless
        // (batch) jobs are excluded (matches SessionListViewModel.IsHeadless).
        self.sessions
            .borrow()
            .iter()
            .filter(|s| !s.is_headless())
            .count()
    }
    pub fn sessions_ref(&self) -> Rc<RefCell<Vec<Session>>> {
        self.sessions.clone()
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.container
    }
}

/// Did anything move between two polls — a session appearing, going, or
/// changing status?
///
/// Only the identity and status matter: the rest of a `Session` carries fields
/// that shift on their own (remaining time, most obviously), and treating those
/// as movement would hold the poller on the fast lane forever, which is the
/// thing the backoff exists to prevent.
fn differs(old: &[Session], new: &[Session]) -> bool {
    old.len() != new.len()
        || new
            .iter()
            .any(|n| !old.iter().any(|o| o.id == n.id && o.status == n.status))
}

/// Did this session fail since the last time we looked?
///
/// The question has to be asked that way round. Asking "is it Failed, and was
/// it not Failed before?" answers YES for a session the app has never seen —
/// and on start-up it has seen nothing, so every session sitting in Failed is
/// announced as though it had just happened. A session that died yesterday
/// raised "Session Failed" on every launch of the app, because the dedup set
/// that would have suppressed the repeat lives in memory and starts empty.
///
/// So a session absent from `old` is skipped, exactly as the Batch Jobs card
/// skips a job it has no previous state for. The cost is that a session which
/// appears already-Failed — launched elsewhere, failed between two polls — is
/// never announced. That is the right side to err on: the alternative is
/// announcing history as news, and there is no way to tell from the listing
/// which it is.
fn newly_failed(old: &[Session], session: &Session) -> bool {
    if !session.status.eq_ignore_ascii_case("failed") {
        return false;
    }
    let Some(previous) = old.iter().find(|s| s.id == session.id) else {
        return false;
    };
    !previous.status.eq_ignore_ascii_case("failed")
}

/// Check for session state transitions and fire desktop notifications.
fn check_notifications(
    old: &[Session],
    new: &[Session],
    notifications: &crate::services::NotificationService,
    widget: &gtk::Box,
) {
    // Get the GIO Application from the widget tree
    let Some(app) = widget
        .root()
        .and_then(|r| r.downcast::<gtk::Window>().ok())
        .and_then(|w| w.application())
    else {
        return;
    };
    let gio_app: &gtk4::gio::Application = app.upcast_ref();

    for session in new {
        let was_pending = old.iter().any(|s| s.id == session.id && s.is_pending());

        // Pending → Running
        if session.is_running() && was_pending {
            notifications.notify_session_ready(
                gio_app,
                &session.id,
                &session.name,
                &session.session_type,
            );
        }

        // Became Failed — a transition, not a state.
        if newly_failed(old, session) {
            notifications.notify_session_failed(gio_app, &session.id, &session.name);
        }

        // Expiring within 1 hour
        if session.is_running() {
            if let Ok(expiry) = chrono::DateTime::parse_from_rfc3339(&session.expiry_time) {
                let remaining = expiry.signed_duration_since(chrono::Utc::now());
                if remaining.num_hours() < 1 && remaining.num_seconds() > 0 {
                    notifications.notify_session_expiring(gio_app, &session.id, &session.name);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::newly_failed;
    use crate::models::Session;

    fn session(id: &str, status: &str) -> Session {
        Session {
            id: id.into(),
            userid: String::new(),
            image: String::new(),
            session_type: "notebook".into(),
            status: status.into(),
            name: id.into(),
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

    #[test]
    fn a_session_already_failed_on_start_up_is_not_news() {
        // The bug this function exists for. On start-up nothing has been seen,
        // so "is Failed and was not Failed before" is true for every failed
        // session in the listing — and a session that died yesterday raised
        // "Session Failed" on every launch of the app.
        let now = [session("vi-astroml-22de8ea2", "Failed")];
        assert!(!newly_failed(&[], &now[0]));
    }

    #[test]
    fn a_session_that_fails_while_watching_is_announced() {
        let before = [session("s1", "Running")];
        assert!(newly_failed(&before, &session("s1", "Failed")));
    }

    #[test]
    fn a_failure_is_announced_once() {
        // Every poll re-reads the same listing; without this the notification
        // repeats for as long as the failed session is listed.
        let before = [session("s1", "Failed")];
        assert!(!newly_failed(&before, &session("s1", "Failed")));
    }

    #[test]
    fn a_session_that_is_not_failed_is_not_a_failure() {
        let before = [session("s1", "Pending")];
        assert!(!newly_failed(&before, &session("s1", "Running")));
    }

    #[test]
    fn the_platforms_casing_is_not_load_bearing() {
        // Skaha has used both "Failed" and "failed".
        let before = [session("s1", "Running")];
        assert!(newly_failed(&before, &session("s1", "failed")));
        let before = [session("s1", "failed")];
        assert!(!newly_failed(&before, &session("s1", "FAILED")));
    }

    #[test]
    fn the_card_carries_the_pages_one_primary_action() {
        // There is exactly one way into the launch form now, and this is it.
        // The floating button that used to be the other one carried no label,
        // sat away from anything it related to, and was the kind of control a
        // user can simply not notice — at which point there was no way in at
        // all. This card is where someone looking at their sessions goes to
        // start another.
        let code =
            crate::testing::without_comments(crate::testing::code(include_str!("session_list.rs")));
        assert!(
            code.contains("on_launch_requested"),
            "the Active Sessions card no longer offers a way to launch"
        );
        // And it is THE primary action: the floating button that used to hold
        // that role is gone, so an understated launch button would leave the
        // page with nothing asking to be pressed.
        let at = code.find("launch_btn").expect("launch button is gone");
        let window = &code[at..(at + 700).min(code.len())];
        assert!(
            window.contains("suggested-action"),
            "the launch button is not styled as the page's primary action"
        );
    }

    use super::{selected_session_filter, SESSION_FILTER_ALL, SESSION_FILTER_TYPES};

    #[test]
    fn the_all_entry_means_no_filter() {
        // Not a session type called "All" — the platform has no such type, and
        // filtering by it would show an empty strip.
        assert_eq!(SESSION_FILTER_ALL, "All");
        assert_eq!(selected_session_filter(0), None);
    }

    #[test]
    fn each_entry_decodes_to_the_type_beside_it() {
        // The dropdown is built from this list and decoded against it, so index
        // and value cannot disagree — which is the whole point of one list.
        for (index, session_type) in SESSION_FILTER_TYPES.iter().enumerate().skip(1) {
            assert_eq!(selected_session_filter(index), Some(*session_type));
        }
    }

    #[test]
    fn an_index_past_the_end_shows_everything() {
        // Safer than hiding sessions the user has running.
        assert_eq!(selected_session_filter(SESSION_FILTER_TYPES.len()), None);
        assert_eq!(selected_session_filter(999), None);
    }

    #[test]
    fn the_filter_offers_exactly_the_interactive_types() {
        use crate::models::session::INTERACTIVE_SESSION_TYPES;

        // Built from the shared list, so a type added to the launcher shows up
        // here automatically rather than being unfilterable.
        assert_eq!(
            SESSION_FILTER_TYPES.len(),
            INTERACTIVE_SESSION_TYPES.len() + 1,
            "the filter is `All` plus every interactive type"
        );
        for session_type in INTERACTIVE_SESSION_TYPES {
            assert!(
                SESSION_FILTER_TYPES.contains(&session_type),
                "{session_type}"
            );
        }
    }

    #[test]
    fn headless_is_not_offered_because_batch_jobs_are_not_cards() {
        // The strip renders interactive sessions only; a headless filter would
        // select for something the strip never shows.
        assert!(!SESSION_FILTER_TYPES.contains(&"headless"));
    }
}
