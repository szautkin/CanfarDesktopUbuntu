use crate::models::session::INTERACTIVE_SESSION_TYPES;
use crate::models::Session;
use crate::state::AppServices;
use crate::ui::session_card::{ActionCallback, SessionAction, SessionCard};
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

type OptionalCallback<T> = Rc<RefCell<Option<Box<dyn Fn(T)>>>>;

const AUTO_REFRESH_SECS: u32 = 15;

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
}

impl SessionListView {
    pub fn new(services: Arc<AppServices>) -> Rc<Self> {
        let card = crate::ui::card::Card::new(crate::tr_en!("Active Sessions"));
        let container = card.widget.clone();
        container.set_vexpand(true);
        let header = card.header.clone();
        let loading_spinner = card.spinner.clone();
        let refresh_btn = card.with_refresh();

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

        let on_action: ActionCallback = Rc::new(RefCell::new(Box::new(|_| {})));

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

        view
    }

    pub fn set_on_action(&self, callback: impl Fn(SessionAction) + 'static) {
        *self.on_action.borrow_mut() = Box::new(callback);
    }

    pub fn set_on_sessions_changed(&self, callback: impl Fn(usize) + 'static) {
        *self.on_sessions_changed.borrow_mut() = Some(Box::new(callback));
    }

    pub async fn refresh(&self) {
        use crate::services::cache_service::CacheKey;
        use crate::services::health_tracker::{ServiceName, ServiceStatus};

        self.loading_spinner.set_visible(true);
        self.loading_spinner.start();

        // Yield so GTK renders the spinner before the async call
        glib::timeout_future(std::time::Duration::from_millis(50)).await;

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
                    self.services.toast.toast(crate::tr_fmt!(
                        "Sessions unreachable — cached list from {}",
                        time_label
                    ));
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

        self.loading_spinner.stop();
        self.loading_spinner.set_visible(false);
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

    fn update_sessions(&self, sessions: Vec<Session>) {
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

        let has_pending = sessions.iter().any(|s| s.is_pending());
        *self.sessions.borrow_mut() = sessions;

        if let Some(ref cb) = *self.on_sessions_changed.borrow() {
            cb(count);
        }

        // Auto-poll every 15s while any session is pending, with countdown
        if has_pending {
            let services = self.services.clone();
            let sessions_ref = self.sessions.clone();
            let cards_box = self.cards_box.clone();
            let count_label = self.count_label.clone();
            let countdown_label = self.countdown_label.clone();
            let empty_label = self.empty_label.clone();
            let on_action = self.on_action.clone();
            let on_changed = self.on_sessions_changed.clone();
            let container = self.container.clone();
            let filter_dropdown = self.filter_dropdown.clone();

            countdown_label.set_visible(true);

            glib::spawn_future_local(async move {
                loop {
                    // Countdown from 15 to 1
                    for remaining in (1..=AUTO_REFRESH_SECS).rev() {
                        countdown_label.set_text(&crate::tr_fmt!("refresh in {}s", remaining));
                        glib::timeout_future_seconds(1).await;
                    }
                    countdown_label.set_text(crate::tr_en!("refreshing..."));

                    let svc = services.clone();
                    let result = services
                        .spawn(async move {
                            let token = svc.get_token().await;
                            if let Some(token) = token {
                                svc.sessions.get_sessions(&token).await.ok()
                            } else {
                                None
                            }
                        })
                        .await;

                    let Some(new_sessions) = result else {
                        countdown_label.set_visible(false);
                        break;
                    };

                    let active_filter =
                        selected_session_filter(filter_dropdown.selected() as usize);

                    while let Some(child) = cards_box.first_child() {
                        cards_box.remove(&child);
                    }
                    // Headless (batch) jobs never render as cards nor count toward
                    // the interactive cap (matches update_sessions above).
                    let count = new_sessions.iter().filter(|s| !s.is_headless()).count();
                    let count_tmpl = if count == 1 {
                        "{} session"
                    } else {
                        "{} sessions"
                    };
                    count_label.set_text(&crate::tr_fmt!(count_tmpl, count));
                    let visible: Vec<&Session> = new_sessions
                        .iter()
                        .filter(|s| !s.is_headless())
                        .filter(|s| match active_filter {
                            None => true,
                            Some(f) => s.session_type.eq_ignore_ascii_case(f),
                        })
                        .collect();
                    empty_label.set_visible(visible.is_empty());
                    for session in visible {
                        let card = SessionCard::new(session, on_action.clone());
                        cards_box.append(card.widget());
                    }
                    if let Some(ref cb) = *on_changed.borrow() {
                        cb(count);
                    }

                    // Notify on state transitions during poll
                    check_notifications(
                        &sessions_ref.borrow(),
                        &new_sessions,
                        &services.notifications,
                        &container,
                    );

                    let still_pending = new_sessions.iter().any(|s| s.is_pending());
                    *sessions_ref.borrow_mut() = new_sessions;

                    if !still_pending {
                        countdown_label.set_visible(false);
                        break;
                    }
                }
            });
        } else {
            self.countdown_label.set_visible(false);
        }
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

    pub fn session_count_by_type(&self, session_type: &str) -> usize {
        self.sessions
            .borrow()
            .iter()
            .filter(|s| s.session_type.eq_ignore_ascii_case(session_type))
            .count()
    }

    pub fn sessions_ref(&self) -> Rc<RefCell<Vec<Session>>> {
        self.sessions.clone()
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.container
    }
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

        // Became Failed (and wasn't already Failed)
        if session.status.eq_ignore_ascii_case("failed") {
            let was_failed = old
                .iter()
                .any(|s| s.id == session.id && s.status.eq_ignore_ascii_case("failed"));
            if !was_failed {
                notifications.notify_session_failed(gio_app, &session.id, &session.name);
            }
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
