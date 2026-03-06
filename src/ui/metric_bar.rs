use gtk4::prelude::*;
use gtk4::{self as gtk};

#[derive(Clone)]
pub struct MetricBar {
    pub container: gtk::Box,
    heading_label: gtk::Label,
    progress: gtk::ProgressBar,
}

impl MetricBar {
    pub fn new(heading: &str) -> Self {
        let container = gtk::Box::new(gtk::Orientation::Vertical, 4);

        let heading_label = gtk::Label::new(Some(heading));
        heading_label.set_halign(gtk::Align::Start);
        heading_label.add_css_class("caption-heading");
        container.append(&heading_label);

        let progress = gtk::ProgressBar::new();
        progress.set_fraction(0.0);
        container.append(&progress);

        MetricBar {
            container,
            heading_label,
            progress,
        }
    }

    /// Update with "available / total" display.
    /// `used` = amount in use, `total` = total capacity.
    /// Shows "Available {label}: {available} / {total} {unit}"
    pub fn update_available(&self, label: &str, used: f64, total: f64, unit: &str) {
        let available = total - used;
        let used_fraction = if total > 0.0 { used / total } else { 0.0 };
        self.progress.set_fraction(used_fraction.clamp(0.0, 1.0));
        self.heading_label.set_text(&format!(
            "Available {}: {:.1} / {:.1}{}",
            label,
            available,
            total,
            if unit.is_empty() { String::new() } else { format!(" {}", unit) },
        ));

        self.progress.remove_css_class("warning");
        self.progress.remove_css_class("error");
        if used_fraction > 0.9 {
            self.progress.add_css_class("error");
        } else if used_fraction > 0.7 {
            self.progress.add_css_class("warning");
        }
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.container
    }
}
