use gtk4::gio;
use gtk4::prelude::*;
use std::collections::HashSet;
use std::sync::Mutex;
use tokio::sync::mpsc;

// ---------------------------------------------------------------------------
// In-app toast dispatch (cross-thread safe)
// ---------------------------------------------------------------------------

/// A toast message that can be sent from any thread (tokio or glib).
#[derive(Debug, Clone)]
pub struct ToastMessage {
    pub body: String,
    /// 0 = persistent until dismissed. Default = 5 seconds.
    pub timeout: u32,
    /// Optional action button: (label, app-action-name).  When set, the
    /// toast receiver in `main_window.rs` attaches the label and on click
    /// calls `app.activate_action(&action_name, None)`.
    pub action: Option<ToastAction>,
}

/// An action button attached to a toast.
#[derive(Debug, Clone)]
pub struct ToastAction {
    pub label: String,
    pub action_name: String,
}

impl ToastMessage {
    pub fn new(body: impl Into<String>) -> Self {
        Self {
            body: body.into(),
            timeout: 5,
            action: None,
        }
    }

    pub fn persistent(body: impl Into<String>) -> Self {
        Self {
            body: body.into(),
            timeout: 0,
            action: None,
        }
    }

    pub fn with_action(
        body: impl Into<String>,
        label: impl Into<String>,
        action_name: impl Into<String>,
    ) -> Self {
        Self {
            body: body.into(),
            timeout: 0,
            action: Some(ToastAction {
                label: label.into(),
                action_name: action_name.into(),
            }),
        }
    }
}

/// Send-safe handle for dispatching in-app toasts from any thread.
/// Uses `tokio::sync::mpsc::UnboundedSender` which is `Clone + Send`.
#[derive(Clone)]
pub struct ToastNotifier {
    sender: mpsc::UnboundedSender<ToastMessage>,
}

impl ToastNotifier {
    /// Create a notifier + receiver pair.
    /// The receiver must be consumed in a `glib::spawn_future_local` loop in main_window.
    pub fn new() -> (Self, mpsc::UnboundedReceiver<ToastMessage>) {
        let (sender, receiver) = mpsc::unbounded_channel();
        (Self { sender }, receiver)
    }

    /// Show a toast with default timeout (5s).
    pub fn toast(&self, body: impl Into<String>) {
        let _ = self.sender.send(ToastMessage::new(body));
    }

    /// Show a persistent toast (user must dismiss).
    pub fn toast_persistent(&self, body: impl Into<String>) {
        let _ = self.sender.send(ToastMessage::persistent(body));
    }

    /// Show a persistent toast with an action button that activates the
    /// named `gio::SimpleAction` registered on the application (e.g.
    /// `"app.navigate-research"`).
    pub fn toast_with_action(
        &self,
        body: impl Into<String>,
        label: impl Into<String>,
        action_name: impl Into<String>,
    ) {
        let _ = self
            .sender
            .send(ToastMessage::with_action(body, label, action_name));
    }
}

// ---------------------------------------------------------------------------
// Desktop notifications (existing)
// ---------------------------------------------------------------------------

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

        let notification = gio::Notification::new(&format!(
            "{} Session Ready",
            crate::models::session::type_label(session_type)
        ));
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

    /// Send a desktop notification when a headless/batch job completes
    /// (transitioned into Succeeded/Completed). Returns true if sent.
    pub fn notify_job_completed(
        &self,
        app: &gio::Application,
        job_id: &str,
        name: &str,
        image: &str,
    ) -> bool {
        let key = format!("job_completed:{}", job_id);
        if !self.mark_notified(&key) {
            return false;
        }

        let notification = gio::Notification::new("Batch Job Completed");
        notification.set_body(Some(&format!(
            "Your batch job '{}' ({}) finished successfully.",
            name, image
        )));
        notification.set_priority(gio::NotificationPriority::Normal);
        app.send_notification(Some(&key), &notification);
        true
    }

    /// Send a desktop notification when a headless/batch job fails
    /// (transitioned into Failed/Error). Returns true if sent.
    pub fn notify_job_failed(
        &self,
        app: &gio::Application,
        job_id: &str,
        name: &str,
        image: &str,
    ) -> bool {
        let key = format!("job_failed:{}", job_id);
        if !self.mark_notified(&key) {
            return false;
        }

        let notification = gio::Notification::new("Batch Job Failed");
        notification.set_body(Some(&format!(
            "Your batch job '{}' ({}) failed. Check events & logs for details.",
            name, image
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

impl Default for NotificationService {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capitalize_works() {
        use crate::models::session::type_label;
        assert_eq!(type_label("notebook"), "Notebook");
        assert_eq!(type_label(""), "");
        assert_eq!(type_label("a"), "A");
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
