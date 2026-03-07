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
    sessions: Rc<RefCell<Vec<Session>>>,
    services: Arc<AppServices>,
    on_action: ActionCallback,
    on_sessions_changed: OptionalCallback<usize>,
}

impl SessionListView {
    pub fn new(services: Arc<AppServices>) -> Rc<Self> {
        let container = gtk::Box::new(gtk::Orientation::Vertical, 8);
        container.add_css_class("card");
        container.set_margin_start(8);
        container.set_margin_end(8);
        container.set_margin_top(8);
        container.set_vexpand(true);

        let (header, loading_spinner, refresh_btn) = card_header("Active Sessions");

        let countdown_label = gtk::Label::new(None);
        countdown_label.add_css_class("dim-label");
        countdown_label.add_css_class("caption");
        countdown_label.set_visible(false);
        // Insert after title
        header.insert_child_after(&countdown_label, Some(&header.first_child().unwrap()));

        let count_label = gtk::Label::new(Some("0 sessions"));
        count_label.add_css_class("dim-label");
        count_label.add_css_class("caption");
        // Insert after countdown
        header.insert_child_after(&count_label, Some(&countdown_label));

        container.append(&header);

        let empty_label = gtk::Label::new(Some("No active sessions"));
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

        let cards_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
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
            sessions: Rc::new(RefCell::new(Vec::new())),
            services,
            on_action,
            on_sessions_changed: Rc::new(RefCell::new(None)),
        });

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
                    svc.sessions.get_sessions(&token).await.ok()
                } else {
                    None
                }
            })
            .await;

        if let Some(sessions) = result {
            self.update_sessions(sessions);
        }

        self.loading_spinner.stop();
        self.loading_spinner.set_visible(false);
    }

    fn update_sessions(&self, sessions: Vec<Session>) {
        while let Some(child) = self.cards_box.first_child() {
            self.cards_box.remove(&child);
        }

        let count = sessions.len();
        self.count_label.set_text(&format!(
            "{} session{}",
            count,
            if count == 1 { "" } else { "s" }
        ));
        self.empty_label.set_visible(count == 0);

        for session in &sessions {
            let card = SessionCard::new(session, self.on_action.clone());
            self.cards_box.append(card.widget());
        }

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

            countdown_label.set_visible(true);

            glib::spawn_future_local(async move {
                loop {
                    // Countdown from 15 to 1
                    for remaining in (1..=AUTO_REFRESH_SECS).rev() {
                        countdown_label.set_text(&format!("refresh in {}s", remaining));
                        glib::timeout_future_seconds(1).await;
                    }
                    countdown_label.set_text("refreshing...");

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

                    while let Some(child) = cards_box.first_child() {
                        cards_box.remove(&child);
                    }
                    let count = new_sessions.len();
                    count_label.set_text(&format!(
                        "{} session{}",
                        count,
                        if count == 1 { "" } else { "s" }
                    ));
                    empty_label.set_visible(count == 0);
                    for session in &new_sessions {
                        let card = SessionCard::new(session, on_action.clone());
                        cards_box.append(card.widget());
                    }
                    if let Some(ref cb) = *on_changed.borrow() {
                        cb(count);
                    }

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
