use crate::ui::text_viewer_dialog::show_tabbed_text_dialog;
use gtk4::prelude::*;
use gtk4::{self as gtk};

pub async fn show_events_dialog(
    parent: &impl IsA<gtk::Widget>,
    session_name: &str,
    events: &str,
    logs: &str,
) {
    let title = format!("Events/Logs: {}", session_name);
    let events_display = if events.is_empty() {
        "No events available"
    } else {
        events
    };
    let logs_display = if logs.is_empty() {
        "No logs available"
    } else {
        logs
    };
    show_tabbed_text_dialog(
        parent,
        &title,
        &[("Events", events_display), ("Logs", logs_display)],
    )
    .await;
}
