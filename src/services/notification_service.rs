use gtk4::gio;
use gtk4::prelude::*;
use std::collections::HashSet;
use std::sync::Mutex;

pub struct NotificationService {
    notified_sessions: Mutex<HashSet<String>>,
}

impl NotificationService {
    pub fn new() -> Self {
        NotificationService {
            notified_sessions: Mutex::new(HashSet::new()),
        }
    }

    /// Send a desktop notification when a session becomes ready.
    /// Returns true if the notification was sent (not a duplicate).
    pub fn notify_session_ready(
        &self,
        app: &gio::Application,
        session_id: &str,
        name: &str,
        session_type: &str,
    ) -> bool {
        let key = format!("ready:{}", session_id);
        if !self.mark_notified(&key) {
            return false;
        }

        let notification =
            gio::Notification::new(&format!("{} Session Ready", capitalize(session_type)));
        notification.set_body(Some(&format!(
            "Your {} session '{}' is now running. Click to open.",
            session_type, name
        )));
        notification.set_priority(gio::NotificationPriority::Normal);
        app.send_notification(Some(&key), &notification);
        true
    }

    /// Send a desktop notification when a session fails.
    pub fn notify_session_failed(
        &self,
        app: &gio::Application,
        session_id: &str,
        name: &str,
    ) -> bool {
        let key = format!("failed:{}", session_id);
        if !self.mark_notified(&key) {
            return false;
        }

        let notification = gio::Notification::new("Session Failed");
        notification.set_body(Some(&format!(
            "Your session '{}' failed to start. Check events for details.",
            name
        )));
        notification.set_priority(gio::NotificationPriority::Urgent);
        app.send_notification(Some(&key), &notification);
        true
    }

    /// Send a desktop notification when a session is expiring soon.
    pub fn notify_session_expiring(
        &self,
        app: &gio::Application,
        session_id: &str,
        name: &str,
    ) -> bool {
        let key = format!("expiring:{}", session_id);
        if !self.mark_notified(&key) {
            return false;
        }

        let notification = gio::Notification::new("Session Expiring Soon");
        notification.set_body(Some(&format!(
            "Your session '{}' will expire within 1 hour. Consider renewing.",
            name
        )));
        notification.set_priority(gio::NotificationPriority::High);
        app.send_notification(Some(&key), &notification);
        true
    }

    /// Mark a notification key as sent. Returns true if it was not previously sent.
    fn mark_notified(&self, key: &str) -> bool {
        let mut set = self.notified_sessions.lock().unwrap();
        set.insert(key.to_string())
    }

    /// Clear all notification tracking (e.g., on logout).
    pub fn clear(&self) {
        let mut set = self.notified_sessions.lock().unwrap();
        set.clear();
    }
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => c.to_uppercase().to_string() + chars.as_str(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capitalize_works() {
        assert_eq!(capitalize("notebook"), "Notebook");
        assert_eq!(capitalize(""), "");
        assert_eq!(capitalize("a"), "A");
    }

    #[test]
    fn dedup_notifications() {
        let svc = NotificationService::new();
        assert!(svc.mark_notified("test:1"));
        assert!(!svc.mark_notified("test:1"));
        svc.clear();
        assert!(svc.mark_notified("test:1"));
    }
}
