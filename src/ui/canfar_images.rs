//! Full-width dashboard widget listing every CANFAR session container image.
//!
//! Port of `Views/Controls/CanfarImagesControl.xaml(.cs)` +
//! `DashboardPage.OnUseImageRequested`. Images come from
//! [`ImageService::get_images`]; each row shows its cached image-discovery state
//! (never inspected / discovered + package count / failed) read straight from the
//! shared [`JsonManifestStore`], with three per-row / per-widget actions:
//!
//!   * **Inspect** — runs the discovery coordinator probe for that image
//!     (`services.image_discovery.discover_image`, cached successes short-circuit)
//!     and refreshes the row.
//!   * **Use this image** — fires the `on_use_image` callback with the image id;
//!     the dashboard wires it to the `use-launch-image` app action so the launch
//!     form pre-selects the image (mirrors the Windows `UseImageRequested` event).
//!   * **Find images by package…** — opens the faceted
//!     [`show_image_discovery_dialog`]; picking an image there also fires
//!     `on_use_image` and refreshes the list (the dialog may have probed images).
//!
//! A per-type filter bar (the SelectorBar analog, single-select linked toggle
//! buttons) plus a total-count badge sit in the header. Rows are ordered
//! discovered → failed → never, then by image id (mirrors the reference sort).

use crate::helpers::discovery_formatting::{category_label, package_count};
use crate::helpers::image_parser::ImageParser;
use crate::models::image_manifest::{DiscoveryOutcome, LastOutcome};
use crate::models::ParsedImage;
use crate::state::AppServices;
use crate::ui::image_discovery_dialog::show_image_discovery_dialog;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;

/// 3-state discovery status for one image row (mirrors `ImageDiscoveryStatus`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiscoveryStatus {
    Discovered,
    Failed,
    Unknown,
}

pub struct CanfarImagesView {
    container: gtk::Box,
    services: Arc<AppServices>,
    filter_bar: gtk::Box,
    count_badge: gtk::Label,
    subtitle: gtk::Label,
    list_box: gtk::ListBox,
    spinner: gtk::Spinner,
    /// Every parsed image last fetched from the images endpoint.
    images: RefCell<Vec<ParsedImage>>,
    /// The distinct session types available, in `available_types` order.
    types: RefCell<Vec<String>>,
    /// The type currently selected in the filter bar (empty ⇒ show all).
    selected_type: RefCell<String>,
    /// Image ids with a probe currently in flight (shown as "Inspecting…").
    running: RefCell<HashSet<String>>,
    #[allow(clippy::type_complexity)]
    on_use_image: Rc<RefCell<Option<Box<dyn Fn(String)>>>>,
}

impl CanfarImagesView {
    pub fn new(services: Arc<AppServices>) -> Rc<Self> {
        let container = gtk::Box::new(gtk::Orientation::Vertical, 12);
        container.add_css_class("card");
        container.set_margin_start(12);
        container.set_margin_end(12);
        container.set_margin_top(12);
        container.set_margin_bottom(12);

        // ── Header: title · (count) · Find-by-package · spinner · refresh ──
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        header.set_margin_start(12);
        header.set_margin_end(12);
        header.set_margin_top(12);

        let title = gtk::Label::new(Some(crate::tr_en!("CANFAR Images")));
        title.add_css_class("title-4");
        title.set_halign(gtk::Align::Start);
        header.append(&title);

        let count_badge = gtk::Label::new(None);
        count_badge.add_css_class("dim-label");
        count_badge.add_css_class("caption");
        header.append(&count_badge);

        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        header.append(&spacer);

        let find_btn = gtk::Button::with_label(crate::tr_en!("Find images by package…"));
        find_btn.add_css_class("flat");
        find_btn.set_valign(gtk::Align::Center);
        header.append(&find_btn);

        let spinner = gtk::Spinner::new();
        spinner.set_visible(false);
        header.append(&spinner);

        let refresh_btn = gtk::Button::from_icon_name("view-refresh-symbolic");
        refresh_btn.set_tooltip_text(Some(crate::tr_en!("Refresh")));
        refresh_btn.set_valign(gtk::Align::Center);
        header.append(&refresh_btn);

        container.append(&header);

        // ── Per-type filter bar (linked toggle buttons) ──
        let filter_bar = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        filter_bar.add_css_class("linked");
        filter_bar.set_margin_start(12);
        filter_bar.set_margin_end(12);
        filter_bar.set_halign(gtk::Align::Start);
        container.append(&filter_bar);

        // ── Discovered X of Y subtitle + row list, grouped with a tight 6px
        // gap so the caption reads as directly describing the list below it
        // (the outer container's 12px spacing still separates this group
        // from the filter bar above). ──
        let list_section = gtk::Box::new(gtk::Orientation::Vertical, 6);

        let subtitle = gtk::Label::new(None);
        subtitle.add_css_class("dim-label");
        subtitle.set_halign(gtk::Align::Start);
        subtitle.set_margin_start(12);
        subtitle.set_margin_end(12);
        list_section.append(&subtitle);

        // ── Scrollable row list ──
        let scrolled = gtk::ScrolledWindow::new();
        scrolled.set_vscrollbar_policy(gtk::PolicyType::Automatic);
        scrolled.set_hscrollbar_policy(gtk::PolicyType::Never);
        scrolled.set_min_content_height(180);
        scrolled.set_max_content_height(360);
        scrolled.set_propagate_natural_height(true);

        let list_box = gtk::ListBox::new();
        list_box.add_css_class("boxed-list");
        list_box.set_selection_mode(gtk::SelectionMode::None);
        list_box.set_margin_start(12);
        list_box.set_margin_end(12);
        list_box.set_margin_bottom(12);
        scrolled.set_child(Some(&list_box));
        list_section.append(&scrolled);
        container.append(&list_section);

        let view = Rc::new(CanfarImagesView {
            container,
            services,
            filter_bar,
            count_badge,
            subtitle,
            list_box,
            spinner,
            images: RefCell::new(Vec::new()),
            types: RefCell::new(Vec::new()),
            selected_type: RefCell::new(String::new()),
            running: RefCell::new(HashSet::new()),
            on_use_image: Rc::new(RefCell::new(None)),
        });

        // Refresh button re-fetches the catalogue.
        {
            let view = view.clone();
            refresh_btn.connect_clicked(move |_| {
                let view = view.clone();
                glib::spawn_future_local(async move {
                    view.refresh().await;
                });
            });
        }

        // Find-by-package opens the faceted discovery dialog.
        {
            let view = view.clone();
            find_btn.connect_clicked(move |_| {
                view.clone().open_find_dialog();
            });
        }

        view
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.container
    }

    /// Register the "Use this image" callback (the dashboard activates the
    /// `use-launch-image` app action with the image id).
    pub fn set_on_use_image(&self, callback: impl Fn(String) + 'static) {
        *self.on_use_image.borrow_mut() = Some(Box::new(callback));
    }

    /// Fetch the image catalogue, rebuild the type filter bar and the row list.
    pub async fn refresh(self: &Rc<Self>) {
        self.spinner.set_visible(true);
        self.spinner.start();

        let svc = self.services.clone();
        let result = self
            .services
            .spawn(async move {
                let token = svc.get_token().await.unwrap_or_default();
                svc.images.get_images(&token).await
            })
            .await;

        self.spinner.stop();
        self.spinner.set_visible(false);

        match result {
            Ok(raw) => {
                let parsed = ImageParser::parse_all(&raw);
                self.count_badge.set_text(&format!("({})", parsed.len()));
                let types = ImageParser::available_types(&parsed);

                // Keep the current selection if it still exists, else pick the first.
                let want = self.selected_type.borrow().clone();
                let selected = if types.iter().any(|t| t == &want) {
                    want
                } else {
                    types.first().cloned().unwrap_or_default()
                };
                *self.images.borrow_mut() = parsed;
                *self.types.borrow_mut() = types;
                *self.selected_type.borrow_mut() = selected;

                self.rebuild_filter_bar();
                self.rebuild_rows();
            }
            Err(e) => {
                self.subtitle.set_text(&format!("Failed to load images: {e}"));
            }
        }
    }

    // ── Filter bar ─────────────────────────────────────────────────────────

    fn rebuild_filter_bar(self: &Rc<Self>) {
        while let Some(child) = self.filter_bar.first_child() {
            self.filter_bar.remove(&child);
        }

        let types = self.types.borrow().clone();
        let selected = self.selected_type.borrow().clone();
        let mut group: Option<gtk::ToggleButton> = None;

        for ty in &types {
            let btn = gtk::ToggleButton::with_label(&capitalize(ty));
            match &group {
                Some(first) => btn.set_group(Some(first)),
                None => group = Some(btn.clone()),
            }
            btn.set_active(ty == &selected);

            let view = self.clone();
            let ty = ty.clone();
            btn.connect_toggled(move |b| {
                // Only react to the newly-activated button; the paired
                // deactivation of the previous button is ignored.
                if b.is_active() {
                    *view.selected_type.borrow_mut() = ty.clone();
                    view.rebuild_rows();
                }
            });
            self.filter_bar.append(&btn);
        }
        self.filter_bar.set_visible(!types.is_empty());
    }

    // ── Row list ───────────────────────────────────────────────────────────

    fn rebuild_rows(self: &Rc<Self>) {
        while let Some(child) = self.list_box.first_child() {
            self.list_box.remove(&child);
        }

        let selected = self.selected_type.borrow().clone();
        let running = self.running.borrow().clone();

        // Filter to the selected type, resolve each image's status, and order
        // discovered → failed → never, then by image id.
        let mut rows: Vec<(DiscoveryStatus, ParsedImage)> = self
            .images
            .borrow()
            .iter()
            .filter(|img| selected.is_empty() || img.types.iter().any(|t| t == &selected))
            .map(|img| {
                let outcome = self.services.image_manifests.get(&img.id);
                (status_of(outcome.as_ref()), img.clone())
            })
            .collect();
        rows.sort_by(|a, b| {
            status_order(a.0)
                .cmp(&status_order(b.0))
                .then_with(|| a.1.id.cmp(&b.1.id))
        });

        for (_, img) in &rows {
            let is_running = running.contains(&img.id);
            self.list_box.append(&self.build_row(img, is_running));
        }

        if rows.is_empty() {
            let empty = gtk::Label::new(Some(crate::tr_en!("No images available")));
            empty.add_css_class("dim-label");
            empty.set_margin_top(12);
            empty.set_margin_bottom(12);
            self.list_box.append(&empty);
        }

        self.update_subtitle();
    }

    fn build_row(self: &Rc<Self>, img: &ParsedImage, is_running: bool) -> adw::ActionRow {
        let outcome = self.services.image_manifests.get(&img.id);
        let status = status_of(outcome.as_ref());

        let row = adw::ActionRow::new();
        row.set_title(&img.display_name);
        row.set_subtitle(&if is_running {
            crate::tr_en!("Inspecting…").to_string()
        } else {
            meta_line(outcome.as_ref(), &img.id)
        });
        row.set_subtitle_lines(0);

        // Status glyph prefix.
        let icon = gtk::Image::from_icon_name(status_icon(status, is_running));
        if let Some(css) = status_icon_css(status, is_running) {
            icon.add_css_class(css);
        }
        icon.set_tooltip_text(Some(status_tooltip(status)));
        row.add_prefix(&icon);

        // Inspect button.
        let inspect_btn = gtk::Button::with_label(crate::tr_en!("Inspect"));
        inspect_btn.add_css_class("flat");
        inspect_btn.set_valign(gtk::Align::Center);
        inspect_btn.set_sensitive(!is_running);
        {
            let view = self.clone();
            let id = img.id.clone();
            inspect_btn.connect_clicked(move |_| {
                view.clone().start_inspect(id.clone());
            });
        }
        row.add_suffix(&inspect_btn);

        // Use-this-image button. Deliberately a regular (non-suggested) button:
        // "suggested-action" is reserved for a single primary call-to-action
        // per view (the Launch button in the launch form), not repeated on
        // every row here.
        let use_btn = gtk::Button::with_label(crate::tr_en!("Use this image"));
        use_btn.set_valign(gtk::Align::Center);
        {
            let on_use = self.on_use_image.clone();
            let id = img.id.clone();
            use_btn.connect_clicked(move |_| {
                if let Some(cb) = on_use.borrow().as_ref() {
                    cb(id.clone());
                }
            });
        }
        row.add_suffix(&use_btn);

        row
    }

    /// Kick off (or refresh) a probe for `image_id`, marking the row running,
    /// then refresh the row when the coordinator returns.
    fn start_inspect(self: &Rc<Self>, image_id: String) {
        if !self.running.borrow_mut().insert(image_id.clone()) {
            return; // already running
        }
        self.rebuild_rows();

        let view = self.clone();
        let services = self.services.clone();
        let coordinator = self.services.image_discovery.clone();
        glib::spawn_future_local(async move {
            let svc = services.clone();
            let id = image_id.clone();
            let _ = services
                .spawn(async move { coordinator.discover_image(&svc, &id, false).await })
                .await;
            view.running.borrow_mut().remove(&image_id);
            view.rebuild_rows();
        });
    }

    /// Open the faceted find-by-package dialog. A committed pick fires
    /// `on_use_image` and rebuilds the list (the dialog may have probed images).
    fn open_find_dialog(self: Rc<Self>) {
        let view = self.clone();
        let on_pick: Rc<dyn Fn(String)> = Rc::new(move |image_id: String| {
            if let Some(cb) = view.on_use_image.borrow().as_ref() {
                cb(image_id);
            }
            view.rebuild_rows();
        });
        show_image_discovery_dialog(self.widget(), self.services.clone(), on_pick);
    }

    fn update_subtitle(self: &Rc<Self>) {
        let total = self.images.borrow().len();
        if total == 0 {
            self.subtitle.set_text("");
            return;
        }
        let discovered = self
            .images
            .borrow()
            .iter()
            .filter(|i| {
                self.services
                    .image_manifests
                    .get(&i.id)
                    .map(|o| o.is_success())
                    .unwrap_or(false)
            })
            .count();
        self.subtitle
            .set_text(&format!("Discovered {discovered} of {total} images"));
    }
}

// ---------------------------------------------------------------------------
// Pure presentation helpers (mirror CanfarImageRow / ApplyStatus / StatusOrder)
// ---------------------------------------------------------------------------

/// Map a cached outcome to the 3-state row status.
fn status_of(outcome: Option<&LastOutcome>) -> DiscoveryStatus {
    match outcome {
        Some(o) if o.is_success() => DiscoveryStatus::Discovered,
        Some(_) => DiscoveryStatus::Failed,
        None => DiscoveryStatus::Unknown,
    }
}

/// Sort key: discovered first, then failed, then never inspected.
fn status_order(s: DiscoveryStatus) -> u8 {
    match s {
        DiscoveryStatus::Discovered => 0,
        DiscoveryStatus::Failed => 1,
        DiscoveryStatus::Unknown => 2,
    }
}

/// Row subtitle: "os_family os_version · N packages" for a discovered image, the
/// failure message (or category label) for a failed probe, else the image id.
fn meta_line(outcome: Option<&LastOutcome>, image_id: &str) -> String {
    match outcome {
        Some(o) if o.is_success() => {
            let m = match o.manifest() {
                Some(m) => m,
                None => return image_id.to_string(),
            };
            let os = match &m.os_family {
                Some(f) if !f.is_empty() && f != "unknown" => {
                    format!("{} {} · ", f, m.os_version.clone().unwrap_or_default())
                }
                _ => String::new(),
            };
            format!("{os}{} packages", package_count(m))
        }
        Some(o) => match &o.outcome {
            DiscoveryOutcome::Failure {
                category, message, ..
            } => {
                if message.trim().is_empty() {
                    category_label(category).to_string()
                } else {
                    message.clone()
                }
            }
            DiscoveryOutcome::Manifest(_) => image_id.to_string(),
        },
        None => image_id.to_string(),
    }
}

fn status_icon(status: DiscoveryStatus, running: bool) -> &'static str {
    if running {
        return "content-loading-symbolic";
    }
    match status {
        DiscoveryStatus::Discovered => "emblem-ok-symbolic",
        DiscoveryStatus::Failed => "dialog-warning-symbolic",
        DiscoveryStatus::Unknown => "content-loading-symbolic",
    }
}

fn status_icon_css(status: DiscoveryStatus, running: bool) -> Option<&'static str> {
    if running {
        return None;
    }
    match status {
        DiscoveryStatus::Discovered => Some("success"),
        DiscoveryStatus::Failed => Some("warning"),
        DiscoveryStatus::Unknown => Some("dim-label"),
    }
}

fn status_tooltip(status: DiscoveryStatus) -> &'static str {
    match status {
        DiscoveryStatus::Discovered => crate::tr_en!("Discovered"),
        DiscoveryStatus::Failed => crate::tr_en!("Inspection failed"),
        DiscoveryStatus::Unknown => crate::tr_en!("Not inspected yet"),
    }
}

/// Capitalize the first character (ASCII), leaving the rest untouched.
fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::image_manifest::ImageManifest;

    const AT: &str = "2026-07-07T00:00:00Z";

    fn discovered_outcome(os: Option<&str>, ver: Option<&str>, python: &[&str]) -> LastOutcome {
        let m = ImageManifest {
            image_id: "img:1".to_string(),
            os_family: os.map(|s| s.to_string()),
            os_version: ver.map(|s| s.to_string()),
            python: python.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        };
        LastOutcome::success(m, AT)
    }

    #[test]
    fn status_of_maps_three_states() {
        assert_eq!(status_of(None), DiscoveryStatus::Unknown);
        assert_eq!(
            status_of(Some(&discovered_outcome(Some("ubuntu"), Some("22.04"), &["numpy"]))),
            DiscoveryStatus::Discovered
        );
        let failed = LastOutcome::failure("img:1", "JobTimedOut", "timed out", None, AT);
        assert_eq!(status_of(Some(&failed)), DiscoveryStatus::Failed);
    }

    #[test]
    fn status_order_ranks_discovered_first() {
        assert!(status_order(DiscoveryStatus::Discovered) < status_order(DiscoveryStatus::Failed));
        assert!(status_order(DiscoveryStatus::Failed) < status_order(DiscoveryStatus::Unknown));
    }

    #[test]
    fn rows_sort_discovered_then_failed_then_unknown_then_by_id() {
        let mut rows = vec![
            (DiscoveryStatus::Unknown, "z"),
            (DiscoveryStatus::Discovered, "b"),
            (DiscoveryStatus::Failed, "a"),
            (DiscoveryStatus::Discovered, "a"),
        ];
        rows.sort_by(|x, y| {
            status_order(x.0)
                .cmp(&status_order(y.0))
                .then_with(|| x.1.cmp(y.1))
        });
        assert_eq!(
            rows,
            vec![
                (DiscoveryStatus::Discovered, "a"),
                (DiscoveryStatus::Discovered, "b"),
                (DiscoveryStatus::Failed, "a"),
                (DiscoveryStatus::Unknown, "z"),
            ]
        );
    }

    #[test]
    fn meta_line_discovered_includes_os_and_count() {
        let o = discovered_outcome(Some("ubuntu"), Some("22.04"), &["numpy", "scipy"]);
        assert_eq!(meta_line(Some(&o), "img:1"), "ubuntu 22.04 · 2 packages");
    }

    #[test]
    fn meta_line_discovered_hides_unknown_os() {
        let o = discovered_outcome(Some("unknown"), None, &["numpy"]);
        assert_eq!(meta_line(Some(&o), "img:1"), "1 packages");
        let none_os = discovered_outcome(None, None, &[]);
        assert_eq!(meta_line(Some(&none_os), "img:1"), "0 packages");
    }

    #[test]
    fn meta_line_failed_uses_message_then_category() {
        let with_msg = LastOutcome::failure("img:1", "JobTimedOut", "the probe timed out", None, AT);
        assert_eq!(meta_line(Some(&with_msg), "img:1"), "the probe timed out");

        let blank_msg = LastOutcome::failure("img:1", "JobTimedOut", "  ", None, AT);
        assert_eq!(meta_line(Some(&blank_msg), "img:1"), "Timed out");
    }

    #[test]
    fn meta_line_never_shows_image_id() {
        assert_eq!(meta_line(None, "images.canfar.net/skaha/base:1.0"), "images.canfar.net/skaha/base:1.0");
    }

    #[test]
    fn capitalize_first_char_only() {
        assert_eq!(capitalize("notebook"), "Notebook");
        assert_eq!(capitalize("carta"), "Carta");
        assert_eq!(capitalize(""), "");
        assert_eq!(capitalize("R"), "R");
    }

    #[test]
    fn status_icon_and_css_track_state() {
        assert_eq!(status_icon(DiscoveryStatus::Discovered, false), "emblem-ok-symbolic");
        assert_eq!(status_icon(DiscoveryStatus::Failed, false), "dialog-warning-symbolic");
        assert_eq!(status_icon(DiscoveryStatus::Discovered, true), "content-loading-symbolic");
        assert_eq!(status_icon_css(DiscoveryStatus::Discovered, false), Some("success"));
        assert_eq!(status_icon_css(DiscoveryStatus::Failed, false), Some("warning"));
        assert_eq!(status_icon_css(DiscoveryStatus::Discovered, true), None);
    }
}
