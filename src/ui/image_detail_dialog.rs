//! What is actually inside one container image.
//!
//! The find-by-package search answers "which images have numpy". This answers
//! the other direction — "what is in THIS one" — which until now could only be
//! read as the comma-joined blob the discovery dialog puts in a subtitle: forty
//! names of six hundred, in one paragraph, sorted alphabetically and truncated
//! mid-thought.
//!
//! The measurements that shaped it, over the 219 manifests cached here:
//!
//! ```text
//!   packages per image   median 624   max 1400
//!   dpkg                 median 477   max  960   (176 of 219 images)
//!   python               median 105   max  658   (185 of 219 images)
//!   conda envs                                   ( 25 of 219 images)
//!   rpm                                          ( 43 of 219 images)
//!   apk, R               empty in every one
//! ```
//!
//! Three things follow from those numbers, and they are the whole design:
//!
//! 1. **Six hundred names is not a paragraph, it is a list to search.** So the
//!    dialog leads with a filter that narrows every section at once and says
//!    how many matched, rather than making the reader scroll for a name they
//!    already know they are looking for.
//! 2. **Nothing renders until it is asked for.** A section builds its chips the
//!    first time it is opened, and is capped at [`CHIPS_PER_SECTION`]. Nine
//!    hundred live widgets is a measurable stall in this app — the images card
//!    has the numbers — and it would be paid to draw names nobody has scrolled
//!    to.
//! 3. **An empty section is not a section.** `apk` and R are empty in all 219;
//!    rendering "R (0)" on every image is a row that only ever says no.
//!
//! Motion is GTK's own expander and nothing else. A dialog that reveals facts
//! does not need anything to move to make its point.

use crate::helpers::discovery_formatting::{failure_summary, package_count, time_ago};
use crate::models::image_manifest::{DiscoveryOutcome, ImageManifest};
use crate::state::AppServices;
use crate::ui::dialog::Dialog;
use crate::ui::{fit, space};
use gtk4::prelude::*;
use gtk4::{self as gtk};
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

/// How tall the dialog may get before its content scrolls.
const DIALOG_HEIGHT: i32 = 720;

/// How many package chips one section will build.
///
/// The largest section measured here is 960 names. Building them all costs a
/// layout pass over 960 live widgets every time the dialog resizes, to show
/// names past the two hundredth that nobody scrolls to — the search is the way
/// to reach those, and it says so when it truncates.
const CHIPS_PER_SECTION: usize = 200;

/// A package name has to be quite long before truncating it helps.
const CHIP_MAX_CHARS: i32 = 28;

/// One ecosystem's packages.
struct Section {
    title: &'static str,
    packages: Vec<String>,
    /// Each name lowercased once, parallel to `packages`.
    ///
    /// The filter runs on every keystroke across every section — up to 1400
    /// names here — and `to_lowercase()` allocates a fresh String for each one
    /// it touches. Typing is the highest-frequency interaction this dialog has,
    /// and the one place latency is felt as the UI being slow rather than the
    /// work being big. Same trick the discovery dialog's row haystack uses.
    lowercased: Vec<String>,
}

struct DetailUi {
    /// Every section with something in it, in reading order.
    sections: Vec<Section>,
    /// The expander and chip container for each section, parallel to
    /// `sections`.
    rows: Vec<(adw::ExpanderRow, gtk::FlowBox)>,
    /// Which sections have had their chips built, so opening one twice does not
    /// rebuild it.
    built: RefCell<Vec<bool>>,
    filter: RefCell<String>,
    summary: gtk::Label,
}

/// Open the detail view for `image_id` over `parent`.
///
/// Reads the cached manifest only — it never probes. An image that has not been
/// inspected says so and points at the button that would do it, rather than
/// quietly starting a job the user did not ask for.
pub fn show_image_detail_dialog(
    parent: &impl IsA<gtk::Widget>,
    services: Arc<AppServices>,
    image_id: &str,
) {
    let dialog = Dialog::new(image_id, fit::DETAIL, DIALOG_HEIGHT);
    let outcome = services.image_manifests.get(image_id);

    match outcome.as_ref().map(|o| &o.outcome) {
        Some(DiscoveryOutcome::Manifest(manifest)) => {
            let recorded = outcome.as_ref().map(|o| o.discovered_at.as_str());
            build_manifest_view(&dialog, image_id, manifest, recorded);
        }
        Some(DiscoveryOutcome::Failure {
            category,
            message,
            job_id,
        }) => {
            build_failure_view(&dialog, category, message, job_id.as_deref());
        }
        None => build_uninspected_view(&dialog),
    }

    let close = gtk::Button::with_label(crate::tr_en!("Close"));
    {
        let window = dialog.window.clone();
        close.connect_clicked(move |_| window.close());
    }
    dialog.add_action(&close);
    dialog.present(parent);
}

// ── The three states ────────────────────────────────────────────────────────

fn build_manifest_view(
    dialog: &Dialog,
    image_id: &str,
    manifest: &ImageManifest,
    recorded_at: Option<&str>,
) {
    let now = chrono::Utc::now().to_rfc3339();

    // ── Identity, and when this was true ──
    //
    // The captured time comes from the probe, not from when this machine
    // happened to write the file: a manifest recovered from CANFAR storage was
    // captured weeks ago, and presenting it as "inspected just now" is the one
    // thing that would make the rest of this dialog untrustworthy.
    let captured = manifest
        .captured_at
        .as_deref()
        .or(recorded_at)
        .filter(|t| !t.is_empty());
    let when = captured
        .map(|t| crate::tr_fmt!("Inspected {}", time_ago(t, &now)))
        .unwrap_or_else(|| crate::tr_en!("Inspected — date not recorded").to_string());

    let header = gtk::Box::new(gtk::Orientation::Vertical, space::ROW);

    // Selectable: the whole point of reading an image's identity is usually to
    // paste it somewhere.
    let id_label = gtk::Label::new(Some(image_id));
    id_label.add_css_class("title-4");
    id_label.set_halign(gtk::Align::Start);
    id_label.set_selectable(true);
    id_label.set_wrap(true);
    id_label.set_xalign(0.0);
    header.append(&id_label);

    let total = package_count(manifest);
    let subtitle = gtk::Label::new(Some(&format!(
        "{}  ·  {}",
        crate::tr_plural!(total, "{} package", "{} packages"),
        when
    )));
    subtitle.add_css_class("dim-label");
    subtitle.set_halign(gtk::Align::Start);
    subtitle.set_wrap(true);
    subtitle.set_xalign(0.0);
    header.append(&subtitle);
    dialog.content().append(&header);

    // ── At a glance ──
    let facts = adw::PreferencesGroup::builder()
        .title(crate::tr_en!("Environment"))
        .build();
    for (title, value) in environment_facts(manifest) {
        facts.add(&fact_row(&title, &value));
    }
    dialog.content().append(&facts);

    // ── The packages ──
    let sections = sections_of(manifest);
    if sections.is_empty() {
        // A manifest with no packages at all is a real outcome, not an error:
        // the probe ran and found a distroless or scratch-based image.
        let empty = gtk::Label::new(Some(crate::tr_en!(
            "The probe found no package manager in this image."
        )));
        empty.add_css_class("dim-label");
        empty.set_halign(gtk::Align::Start);
        dialog.content().append(&empty);
        return;
    }

    let search = gtk::SearchEntry::new();
    search.set_placeholder_text(Some(crate::tr_en!("Filter packages…")));
    dialog.content().append(&search);

    let summary = gtk::Label::new(None);
    summary.add_css_class("dim-label");
    summary.add_css_class("caption");
    summary.set_halign(gtk::Align::Start);
    dialog.content().append(&summary);

    let group = adw::PreferencesGroup::builder()
        .title(crate::tr_en!("Installed packages"))
        .build();

    let mut rows = Vec::new();
    for section in &sections {
        let row = adw::ExpanderRow::new();
        // Counts today, but the rule is blanket for a reason: deciding
        // case-by-case which row is safe is how the one showing a package name
        // gets missed.
        row.set_use_markup(false);
        row.set_title(section.title);
        row.set_subtitle(&crate::tr_plural!(
            section.packages.len(),
            "{} package",
            "{} packages"
        ));

        let flow = chip_container();
        let holder = adw::ActionRow::new();
        holder.set_activatable(false);
        holder.set_child(Some(&flow));
        row.add_row(&holder);

        group.add(&row);
        rows.push((row, flow));
    }
    dialog.content().append(&group);

    let ui = Rc::new(DetailUi {
        built: RefCell::new(vec![false; sections.len()]),
        sections,
        rows,
        filter: RefCell::new(String::new()),
        summary,
    });
    ui.render();

    // Chips are built when a section is first opened, not up front: the largest
    // here is 960 names and most are never looked at.
    for (index, (row, _)) in ui.rows.iter().enumerate() {
        let ui = ui.clone();
        row.connect_expanded_notify(move |r| {
            if r.is_expanded() {
                ui.fill(index);
            }
        });
    }

    {
        let ui = ui.clone();
        search.connect_search_changed(move |entry| {
            *ui.filter.borrow_mut() = entry.text().to_string();
            ui.render();
        });
    }
}

fn build_failure_view(dialog: &Dialog, category: &str, message: &str, job_id: Option<&str>) {
    let title = gtk::Label::new(Some(&failure_summary(category, message)));
    title.add_css_class("title-4");
    title.set_halign(gtk::Align::Start);
    title.set_wrap(true);
    title.set_xalign(0.0);
    dialog.content().append(&title);

    let explain = gtk::Label::new(Some(crate::tr_en!(
        "This image could not be inspected, so there is nothing to search inside it. \
         Inspect it again from the CANFAR Images list — a rebuilt image often succeeds \
         where an earlier attempt did not."
    )));
    explain.add_css_class("dim-label");
    explain.set_halign(gtk::Align::Start);
    explain.set_wrap(true);
    explain.set_xalign(0.0);
    dialog.content().append(&explain);

    // The probe job that produced this, so it can be looked up while CANFAR
    // still has it. Above the output, because it is the one line someone
    // reporting the failure needs to quote.
    if let Some(job_id) = job_id.filter(|j| !j.is_empty()) {
        let group = adw::PreferencesGroup::new();
        group.add(&fact_row(crate::tr_en!("Probe job"), job_id));
        dialog.content().append(&group);
    }

    // The job's own words, collapsed — the same treatment the Batch Jobs
    // history and the status bar give a failure, from the same helper.
    if !message.trim().is_empty() {
        dialog.content().append(&crate::ui::failure_detail::reason_row(
            crate::tr_en!("What the probe reported"),
            crate::tr_en!("Open for the full output"),
            message,
            None,
        ));
    }
}

fn build_uninspected_view(dialog: &Dialog) {
    let title = gtk::Label::new(Some(crate::tr_en!("Not inspected yet")));
    title.add_css_class("title-4");
    title.set_halign(gtk::Align::Start);
    title.set_xalign(0.0);
    dialog.content().append(&title);

    let explain = gtk::Label::new(Some(crate::tr_en!(
        "Nothing is known about what is installed in this image. Press Inspect in the \
         CANFAR Images list to run a probe job; it takes a few minutes, and the result \
         is cached and searchable afterwards."
    )));
    explain.add_css_class("dim-label");
    explain.set_halign(gtk::Align::Start);
    explain.set_wrap(true);
    explain.set_xalign(0.0);
    dialog.content().append(&explain);
}

// ── Rendering ───────────────────────────────────────────────────────────────

impl DetailUi {
    /// Apply the current filter: section counts, visibility, and the summary
    /// line. Chips are only rebuilt for sections that are open.
    fn render(self: &Rc<Self>) {
        let filter = self.filter.borrow().clone();
        let mut matched = 0usize;
        let mut sections_with_hits = 0usize;

        for (index, section) in self.sections.iter().enumerate() {
            let hits = section.matching(&filter).count();
            matched += hits;
            let (row, _) = &self.rows[index];

            // A section with no match under the current filter is hidden
            // outright rather than shown reading zero. With a filter typed, the
            // useful thing on screen is the short list of sections that DO have
            // it.
            row.set_visible(hits > 0);
            row.set_use_markup(false);
            row.set_subtitle(&crate::tr_plural!(hits, "{} package", "{} packages"));
            if hits > 0 {
                sections_with_hits += 1;
            }

            // Only what is on screen: a closed section rebuilds when opened.
            self.built.borrow_mut()[index] = false;
            if row.is_expanded() && hits > 0 {
                self.fill(index);
            }
        }

        if filter.is_empty() {
            self.summary.set_text("");
            return;
        }
        self.summary.set_text(&if matched == 0 {
            crate::tr_en!("No package matches that.").to_string()
        } else {
            crate::tr_fmt!(
                "{} matching, across {}",
                matched,
                crate::tr_plural!(sections_with_hits, "{} section", "{} sections")
            )
        });
    }

    /// Build one section's chips, once.
    fn fill(self: &Rc<Self>, index: usize) {
        if self.built.borrow()[index] {
            return;
        }
        self.built.borrow_mut()[index] = true;

        let (_, flow) = &self.rows[index];
        while let Some(child) = flow.first_child() {
            flow.remove(&child);
        }

        let filter = self.filter.borrow().clone();
        let section = &self.sections[index];
        let mut shown = 0usize;
        for name in section.matching(&filter).take(CHIPS_PER_SECTION) {
            flow.append(&chip(name));
            shown += 1;
        }

        let total = section.matching(&filter).count();
        if total > shown {
            // Says what is missing and how to reach it, rather than trailing
            // off in an ellipsis.
            let more = gtk::Label::new(Some(&crate::tr_fmt!(
                "+{} more — search to narrow",
                total - shown
            )));
            more.add_css_class("dim-label");
            more.add_css_class("caption");
            flow.append(&more);
        }
    }
}

impl Section {
    /// The packages whose name contains `filter`, case-insensitively.
    ///
    /// The filter is folded here rather than by the caller. It read better as a
    /// contract — "pass it lowercased" — right up until a caller passed `NUM`
    /// and got nothing, with no error and no empty-state to explain it. One
    /// allocation per call against a scan of up to 960 names is not the cost
    /// worth optimising; a silent wrong answer is not worth having at all.
    fn matching<'a>(&'a self, filter: &str) -> impl Iterator<Item = &'a String> + 'a {
        let needle = filter.trim().to_lowercase();
        self.packages
            .iter()
            .zip(&self.lowercased)
            .filter(move |(_, lower)| needle.is_empty() || lower.contains(&needle))
            .map(|(name, _)| name)
    }
}

// ── Content ─────────────────────────────────────────────────────────────────

/// The OS and interpreter facts, skipping the ones this image has nothing to
/// say about.
///
/// Kernel is deliberately dropped when the probe could not read one: it records
/// `"unknown (static layer scan)"` for every image scanned without running it,
/// which is most of them, and a row saying that on every image is a row that
/// teaches the reader to skip the section.
fn environment_facts(m: &ImageManifest) -> Vec<(String, String)> {
    let mut facts = Vec::new();

    // The long release string when there is one — "Debian GNU/Linux 13
    // (trixie)" tells a reader more than "debian 13", and both are recorded.
    let os = m
        .os_release
        .clone()
        .filter(|r| !r.is_empty())
        .unwrap_or_else(|| {
            format!(
                "{} {}",
                m.os_family.clone().unwrap_or_default(),
                m.os_version.clone().unwrap_or_default()
            )
            .trim()
            .to_string()
        });
    if !os.is_empty() {
        facts.push((crate::tr_en!("Operating system").to_string(), os));
    }

    if let Some(kernel) = m.kernel.as_deref() {
        if !kernel.is_empty() && !kernel.starts_with("unknown") {
            facts.push((crate::tr_en!("Kernel").to_string(), kernel.to_string()));
        }
    }
    if !m.capabilities.is_empty() {
        facts.push((
            crate::tr_en!("Capabilities").to_string(),
            m.capabilities.join(", "),
        ));
    }
    if !m.conda_envs.is_empty() {
        facts.push((
            crate::tr_en!("Conda environments").to_string(),
            m.conda_envs.join(", "),
        ));
    }
    if !m.shells.is_empty() {
        facts.push((crate::tr_en!("Shells").to_string(), m.shells.join(", ")));
    }

    if facts.is_empty() {
        facts.push((
            crate::tr_en!("Operating system").to_string(),
            crate::tr_en!("Not recorded").to_string(),
        ));
    }
    facts
}

/// The non-empty package sections, in the order a reader wants them.
///
/// Python first: it is what an astronomer is looking for, and the system
/// packages are the long tail underneath. A section with nothing in it is left
/// out entirely — `apk` and R are empty in all 219 manifests cached here, and a
/// row that only ever reads zero is a row that trains people to skip the group.
fn sections_of(m: &ImageManifest) -> Vec<Section> {
    let mut out = Vec::new();
    let mut push = |title: &'static str, packages: &[String]| {
        if !packages.is_empty() {
            let mut packages = packages.to_vec();
            packages.sort();
            let lowercased = packages.iter().map(|p| p.to_lowercase()).collect();
            out.push(Section {
                title,
                packages,
                lowercased,
            });
        }
    };

    push("Python", &m.python);
    push("R", &m.r_packages);
    push("System · apt", &m.dpkg);
    push("System · rpm", &m.rpm);
    push("System · apk", &m.apk);
    out
}

// ── Widgets ─────────────────────────────────────────────────────────────────

fn fact_row(title: &str, value: &str) -> adw::ActionRow {
    let row = adw::ActionRow::new();
    row.set_use_markup(false);
    row.set_title(title);
    row.set_subtitle(value);
    row.set_subtitle_lines(0);
    // Worth copying: an OS release string or a capability list is the sort of
    // thing that ends up in an issue report.
    row.set_subtitle_selectable(true);
    row
}

fn chip_container() -> gtk::FlowBox {
    let flow = gtk::FlowBox::new();
    flow.set_selection_mode(gtk::SelectionMode::None);
    flow.set_row_spacing(space::ROW as u32);
    flow.set_column_spacing(space::ROW as u32);
    flow.set_max_children_per_line(12);
    flow.set_homogeneous(false);
    space::inset(&flow, space::ROW);
    flow
}

/// One package name.
///
/// A chip rather than a row: six hundred names as rows is six hundred screens
/// of scrolling, and the reader is scanning for a word, not reading records.
fn chip(name: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(name));
    label.add_css_class("caption");
    label.add_css_class("dim-label");
    label.set_selectable(true);
    label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    label.set_max_width_chars(CHIP_MAX_CHARS);
    label.set_tooltip_text(Some(name));
    label
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> ImageManifest {
        ImageManifest {
            python: vec!["numpy".into(), "astropy".into()],
            dpkg: vec!["bash".into()],
            ..Default::default()
        }
    }

    #[test]
    fn nothing_inside_a_modal_opens_this_modal() {
        // A dialog raised from a dialog is the bug that froze this app once:
        // the parent closes, the child is left behind the main window still
        // holding the input grab, and the cause is invisible. The image row in
        // the CANFAR Images card is the only door in — the find-by-package
        // dialog shows the same manifest inline, in an expander, on purpose.
        let mut offenders: Vec<String> = Vec::new();
        for (path, source) in crate::testing::rust_sources() {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            if name == "image_detail_dialog.rs" || name == "canfar_images.rs" {
                continue; // where it lives, and the one row that opens it
            }
            let code = crate::testing::without_comments(crate::testing::code(&source));
            if code.contains("show_image_detail_dialog") {
                offenders.push(name);
            }
        }
        assert!(
            offenders.is_empty(),
            "the image detail modal is opened from {offenders:?} — it may only be \
             opened from an image row in the CANFAR Images card, never from inside \
             another modal"
        );
    }

    #[test]
    fn an_empty_ecosystem_is_not_shown_at_all() {
        // `apk` and R are empty in all 219 manifests cached on this machine. A
        // row reading "R (0)" on every image trains the reader to skip the
        // group that also holds the section they want.
        let titles: Vec<&str> = sections_of(&manifest()).iter().map(|s| s.title).collect();
        assert_eq!(titles, vec!["Python", "System · apt"]);
    }

    #[test]
    fn python_comes_before_the_system_packages() {
        // What someone opening this is looking for. The 477-name apt list is
        // the long tail underneath, not the headline.
        let sections = sections_of(&manifest());
        assert_eq!(sections[0].title, "Python");
    }

    #[test]
    fn packages_are_sorted_so_a_name_can_be_found_by_eye() {
        let sections = sections_of(&manifest());
        assert_eq!(sections[0].packages, vec!["astropy", "numpy"]);
    }

    #[test]
    fn the_filter_is_case_insensitive_and_matches_inside_a_name() {
        // Nobody types the exact package name — they type "astro" or "CFITSIO".
        let sections = sections_of(&manifest());
        let hits: Vec<&String> = sections[0].matching("astro").collect();
        assert_eq!(hits, vec!["astropy"]);
        // Case is folded inside `matching`, so a filter typed in any case
        // works. Passing "NUM" here used to silently match nothing.
        assert_eq!(sections[0].matching("NUM").count(), 1);
        assert_eq!(sections[0].matching("NumPy").count(), 1);
        // A trailing space from a paste should not empty the list.
        assert_eq!(sections[0].matching("  numpy  ").count(), 1);
    }

    #[test]
    fn an_empty_filter_matches_everything() {
        let sections = sections_of(&manifest());
        assert_eq!(sections[0].matching("").count(), 2);
    }

    #[test]
    fn a_kernel_the_probe_could_not_read_is_left_out() {
        // The static layer scan records "unknown (static layer scan)" for most
        // images. Showing it is a row that says nothing, on nearly every image.
        let m = ImageManifest {
            kernel: Some("unknown (static layer scan)".into()),
            os_family: Some("debian".into()),
            ..Default::default()
        };
        let titles: Vec<String> = environment_facts(&m).into_iter().map(|(t, _)| t).collect();
        assert!(
            !titles.iter().any(|t| t == "Kernel"),
            "an unreadable kernel is being shown: {titles:?}"
        );
    }

    #[test]
    fn a_real_kernel_is_shown() {
        let m = ImageManifest {
            kernel: Some("6.8.0-38-generic".into()),
            ..Default::default()
        };
        assert!(environment_facts(&m).iter().any(|(t, _)| t == "Kernel"));
    }

    #[test]
    fn the_long_os_release_wins_over_the_family_and_version() {
        // "Debian GNU/Linux 13 (trixie)" tells a reader more than "debian 13",
        // and the probe records both.
        let m = ImageManifest {
            os_family: Some("debian".into()),
            os_version: Some("13".into()),
            os_release: Some("Debian GNU/Linux 13 (trixie)".into()),
            ..Default::default()
        };
        let (_, value) = environment_facts(&m).into_iter().next().unwrap();
        assert_eq!(value, "Debian GNU/Linux 13 (trixie)");
    }

    #[test]
    fn an_image_with_nothing_recorded_still_says_something() {
        // A blank Environment group reads as a broken dialog.
        assert!(!environment_facts(&ImageManifest::default()).is_empty());
    }

    #[test]
    fn a_section_never_builds_more_chips_than_the_cap() {
        // 960 live widgets is a layout cost paid on every resize, to draw names
        // past the two hundredth that nobody scrolls to.
        let many: Vec<String> = (0..900).map(|n| format!("pkg{n:03}")).collect();
        let section = Section {
            lowercased: many.clone(),
            title: "Python",
            packages: many,
        };
        assert_eq!(section.matching("").take(CHIPS_PER_SECTION).count(), 200);
    }
}
