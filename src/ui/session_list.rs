use crate::models::Session;
use crate::state::AppServices;
use crate::ui::card_header::card_header;
use crate::ui::session_card::{ActionCallback, SessionAction, SessionCard};
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

type OptionalCallback<T> = Rc<RefCell<Option<Box<dyn Fn(T)>>>>;

const AUTO_REFRESH_SECS: u32 = 15;

pub struct SessionListView {
    pub container: gtk::Box,
    cards_box: gtk::Box,
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
        let container = gtk::Box::new(gtk::Orientation::Vertical, 12);
        container.add_css_class("card");
        container.set_margin_start(12);
        container.set_margin_end(12);
        container.set_margin_top(12);
        container.set_margin_bottom(12);
        container.set_vexpand(true);

        let (header, loading_spinner, refresh_btn) = card_header(crate::tr_en!("Active Sessions"));

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

        let filter_types =
            gtk::StringList::new(&["All", "notebook", "desktop", "carta", "contributed", "firefly"]);
        let filter_dropdown = gtk::DropDown::new(Some(filter_types), gtk::Expression::NONE);
        filter_dropdown.set_valign(gtk::Align::Center);
        header.insert_child_after(&filter_dropdown, Some(&count_label));

        container.append(&header);

        let empty_label = gtk::Label::new(Some(crate::tr_en!("No active sessions")));
        empty_label.add_css_class("dim-label");
        empty_label.set_margin_top(32);
        empty_label.set_margin_bottom(32);
        empty_label.set_visible(false);
        container.append(&empty_label);

        let scrolled = gtk::ScrolledWindow::new();
        scrolled.set_hscrollbar_policy(gtk::PolicyType::Automatic);
        scrolled.set_vscrollbar_policy(gtk::PolicyType::Automatic);
        scrolled.set_vexpand(true);
        scrolled.set_min_content_height(200);

        let cards_box = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        cards_box.set_margin_start(12);
        cards_box.set_margin_end(12);
        cards_box.set_margin_bottom(12);
        scrolled.set_child(Some(&cards_box));
        container.append(&scrolled);

        let on_action: ActionCallback = Rc::new(RefCell::new(Box::new(|_| {})));

        let view = Rc::new(SessionListView {
            container,
            cards_box,
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
                    self.services.toast.toast(&crate::tr_fmt!(
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
        let types = ["All", "notebook", "desktop", "carta", "contributed", "firefly"];
        let idx = self.filter_dropdown.selected() as usize;
        match types.get(idx) {
            Some(&"All") | None => None,
            Some(t) => Some(t.to_string()),
        }
    }

    fn update_sessions(&self, sessions: Vec<Session>) {
        while let Some(child) = self.cards_box.first_child() {
            self.cards_box.remove(&child);
        }

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
        let count_tmpl = if count == 1 { "{} session" } else { "{} sessions" };
        self.count_label
            .set_text(&crate::tr_fmt!(count_tmpl, count));
        self.empty_label.set_visible(visible.is_empty());

        for session in &visible {
            let card = SessionCard::new(session, self.on_action.clone());
            self.cards_box.append(card.widget());
        }

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
                let filter_types =
                    ["All", "notebook", "desktop", "carta", "contributed", "firefly"];

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

                    let active_filter = match filter_types.get(filter_dropdown.selected() as usize) {
                        Some(&"All") | None => None,
                        Some(t) => Some(*t),
                    };

                    while let Some(child) = cards_box.first_child() {
                        cards_box.remove(&child);
                    }
                    let count = new_sessions.len();
                    let count_tmpl = if count == 1 { "{} session" } else { "{} sessions" };
                    count_label.set_text(&crate::tr_fmt!(count_tmpl, count));
                    let visible: Vec<&Session> = new_sessions
                        .iter()
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
        self.sessions.borrow().len()
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
