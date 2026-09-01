//! How wide a docked side panel is, and what it does when there is no room.
//!
//! There were five answers to that: `viewer_shell::COLUMN_WIDTH` said 280, the
//! navigation split view said 220 to 280, the file panel's `Paned` said 280,
//! the Search page said 260, and the Research and Workflows lists said whatever
//! `hexpand` left over. Four numbers that are nearly the same, and one that is
//! not a number.
//!
//! The rule the numbers serve, in three clauses:
//!
//! 1. **A panel states its width and holds it.** The content beside it is the
//!    only thing that expands.
//! 2. **The content takes the squeeze**, down to its own honest minimum.
//! 3. **Below a breakpoint the panel stops taking space** and overlays instead.
//!
//! Clause 3 is what makes clause 1 safe. Without it a panel that refuses to
//! shrink is simply clipped when the window is small, which is what the Search
//! page did: at the window the app opens at, its right panel was 47 px past the
//! edge, so every row's edit and delete buttons were outside the window with
//! nothing reporting a problem.

use gtk4::prelude::*;
use gtk4::{self as gtk};
use libadwaita as adw;
use libadwaita::prelude::*;

/// The smallest a picture area asks to be, in logical pixels.
///
/// Clause 2 of the rule above, as a number. A `set_content_width` is a
/// MINIMUM, not a preferred size, so a viewer that asked for its image's width
/// capped at 800 gave itself an 800 px floor — and the panel beside it could
/// then only dock in a window nobody opens by default. Both viewers did that,
/// and the cube did it in a homogeneous `Stack`, so its 2D slice set the floor
/// for the 3D volume as well.
///
/// It has to stay below [`COLLAPSE_FLEXIBLE_SP`] minus [`WIDTH`], or the
/// picture's own minimum decides when a panel docks and the threshold means
/// nothing.
pub const CONTENT_FLOOR: i32 = 360;

/// A docked side panel, in logical pixels.
///
/// One number, or the app has several ideas of how wide "a panel" is.
pub const WIDTH: i32 = 280;

/// Where a panel beside FLEXIBLE content stops docking.
///
/// An image, a 3D volume, a list: content with no width it insists on, which
/// can give the panel its 280 and still be worth looking at. The floor is what
/// is left over — 660 leaves 380 for the content, which is about the narrowest
/// a picture is worth showing at.
///
/// This is the number the viewers had wrong. Their old 900 meant the FITS and
/// cube control columns — colormap, stretch, cut levels, channel scrubber,
/// marks — were not on screen at all at the window the app opens at, only
/// reachable through a toggle and then only as an overlay over the picture.
pub const COLLAPSE_FLEXIBLE_SP: f64 = 660.0;

/// Where a panel beside RIGID content stops docking.
///
/// A form is not an image: its fields have widths they cannot go below, so the
/// content cannot absorb the squeeze and the panel is what has to go.
///
/// Derived, not chosen. `panel_width_probe` measures the Search page's form at
/// a 583 px minimum and its panel at [`RECENT_WIDTH`]; 583 + 340 is 923, so a
/// page that docked below about that is a page drawing past its own edge —
/// which is exactly what it did, by 47 px, at the window the app opens at.
/// Rounded up for the margin the measurement does not include.
///
/// Both are in `sp`, which scales with the user's text size: a threshold in raw
/// pixels is one that moves out from under the text it was measured against.
pub const COLLAPSE_RIGID_SP: f64 = 940.0;

/// A list of names is not a column of controls, and is not this wide.
///
/// [`WIDTH`] fits a stack of dropdowns and sliders, which are as wide as they
/// are told to be. A list is as wide as the longest name someone gave a thing,
/// and `panel_width_probe` measures those with truncation switched off:
///
/// | list | rows | median | widest |
/// | --- | --- | --- | --- |
/// | Recent Searches | 13 | 334 | 413 |
/// | Workflows | 17 | 429 | 641 |
///
/// The median, not the maximum: a panel sized for its longest row is sized for
/// its rarest one, and one 641 px workflow title is not a reason to give every
/// list 641 px.
pub const RECENT_WIDTH: i32 = 340;
/// See [`RECENT_WIDTH`] — the same measurement, on a longer kind of name.
pub const LIST_WIDTH: i32 = 430;

/// Make `w` a panel: it states its width and does not expand.
///
/// The `set_hexpand(false)` is the load-bearing half, and it is not redundant
/// with never calling `set_hexpand(true)`. GTK propagates expansion UPWARD from
/// any descendant, so the Search panel — which never asked to expand — grew
/// without limit because one label inside it did: 175 px at a 1200 window and
/// 618 at 2000, taking half of every pixel a wider window brought. Setting the
/// flag explicitly to false is what stops the propagation.
pub fn pin(w: &impl IsA<gtk::Widget>, width: i32) {
    let w = w.as_ref();
    w.set_size_request(width, -1);
    w.set_hexpand(false);
    // A marker, so a panel can be FOUND. `panel_width_probe` has to tell a
    // panel from a shrink floor or a thumbnail, and every guess at that from
    // the outside — "a width request between 200 and 400" — was wrong about
    // something. This is the code saying which it is.
    w.add_css_class(MARKER);
}

/// The CSS class [`pin`] leaves on a panel. Not styled; it is a label.
pub const MARKER: &str = "verbinal-panel";

/// Make `w` a panel at the standard [`WIDTH`].
pub fn pin_standard(w: &impl IsA<gtk::Widget>) {
    pin(w, WIDTH);
}

/// A content pane with a panel docked beside it, that overlays when narrow.
///
/// The shape both viewers already had, lifted out of `viewer_shell` so the
/// pages can have it too. An `OverlaySplitView` rather than a `Paned`: on a
/// wide window the panel is docked beside the content, and on a narrow one it
/// floats over it instead of squeezing it — or, before this existed, instead of
/// being clipped by the window edge.
///
/// The breakpoint lives in an `adw::BreakpointBin` so the rule belongs to the
/// thing it governs: a page is inside a window it does not own, and asking the
/// window to know about a page's panel is the coupling this avoids.
pub struct Docked {
    /// The whole thing — put this in the page.
    pub widget: adw::BreakpointBin,
    /// Shows and hides the panel. Useful at any width, and the only way back to
    /// the panel once the window is narrow enough to have hidden it.
    pub toggle: gtk::ToggleButton,
}

/// Dock `panel` at the end of `content`, collapsing below `collapse_sp`.
///
/// The threshold is the caller's because it depends on what the content is:
/// [`COLLAPSE_FLEXIBLE_SP`] for something that shrinks, [`COLLAPSE_RIGID_SP`]
/// for something that cannot. One number for both was tried and cannot work —
/// it either takes the viewers' controls away while there is room for them, or
/// leaves the Search form to be drawn past the window edge.
pub fn docked(
    content: &impl IsA<gtk::Widget>,
    panel: &impl IsA<gtk::Widget>,
    tooltip: &str,
    collapse_sp: f64,
) -> Docked {
    use gtk4::glib::prelude::ToValue;

    let split = adw::OverlaySplitView::new();
    split.set_content(Some(content.as_ref()));
    split.set_sidebar(Some(panel.as_ref()));
    split.set_sidebar_position(gtk::PackType::End);
    split.set_show_sidebar(true);
    split.set_hexpand(true);
    split.set_vexpand(true);

    let bin = adw::BreakpointBin::new();
    bin.set_hexpand(true);
    bin.set_vexpand(true);
    // A BreakpointBin refuses to allocate smaller than its child's minimum, so
    // the child must be allowed to shrink to the width the breakpoint watches
    // for — otherwise the condition can never be met.
    bin.set_size_request(360, 200);
    bin.set_child(Some(&split));

    let breakpoint = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
        adw::BreakpointConditionLengthType::MaxWidth,
        collapse_sp,
        adw::LengthUnit::Sp,
    ));
    breakpoint.add_setter(&split, "collapsed", Some(&true.to_value()));
    // Hidden as well as collapsed: a narrow window should give the whole width
    // to the content, and the toggle brings the panel back over it. Overlaying
    // it the moment the window narrows would cover the work uninvited.
    breakpoint.add_setter(&split, "show-sidebar", Some(&false.to_value()));
    bin.add_breakpoint(breakpoint);

    let toggle = gtk::ToggleButton::new();
    toggle.set_icon_name("sidebar-show-right-symbolic");
    toggle.add_css_class("flat");
    toggle.set_valign(gtk::Align::Center);
    toggle.set_tooltip_text(Some(tooltip));
    // Bound both ways: the button reflects a collapse the breakpoint caused,
    // and pressing it moves the same property. A separate bool would be a
    // second opinion about whether the panel is on screen.
    split
        .bind_property("show-sidebar", &toggle, "active")
        .bidirectional()
        .sync_create()
        .build();

    Docked {
        widget: bin,
        toggle,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The threshold has to leave room for the panel it collapses.
    ///
    /// A collapse width below the panel's own width would mean a panel that
    /// docks only when it has less space than it needs, which is the bug with
    /// the sign flipped.
    #[test]
    fn a_panel_only_docks_where_it_fits() {
        for (name, sp) in [
            ("flexible", COLLAPSE_FLEXIBLE_SP),
            ("rigid", COLLAPSE_RIGID_SP),
        ] {
            assert!(
                sp > f64::from(WIDTH) * 2.0,
                "a {WIDTH} px panel docking below {sp} sp ({name}) would take \
                 more than half the width it is docked in"
            );
        }
    }

    /// The picture's floor leaves room for the panel to dock above it.
    ///
    /// If a picture insists on more than the collapse threshold minus a panel,
    /// then the picture decides when the panel docks — and [`COLLAPSE_FLEXIBLE_SP`]
    /// becomes a number that describes nothing, which is exactly what happened.
    #[test]
    fn a_picture_leaves_room_for_the_panel_beside_it() {
        let room = COLLAPSE_FLEXIBLE_SP - f64::from(WIDTH);
        assert!(
            f64::from(CONTENT_FLOOR) <= room,
            "a picture insisting on {CONTENT_FLOOR} px cannot have a {WIDTH} px \
             panel docked beside it below {COLLAPSE_FLEXIBLE_SP} sp — the \
             picture would decide the threshold, not the threshold"
        );
    }

    /// No picture area sets a floor of its own.
    ///
    /// `set_content_width` reads like "how big I would like to be" and means
    /// "how small I may ever be". Two viewers spelled a number there and both
    /// were the reason their controls would not dock.
    #[test]
    fn no_picture_area_states_its_own_floor() {
        let mut strays: Vec<String> = Vec::new();
        for (path, source) in crate::testing::rust_sources() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            // A dialog sizes itself to its content, so a floor there is a
            // widget saying how big the dialog should be — nothing is docked
            // beside it and nothing gets pushed off a screen edge.
            if name == "panel.rs" || name.ends_with("_dialog.rs") {
                continue;
            }
            for (i, line) in crate::testing::code(&source).lines().enumerate() {
                let t = line.trim();
                if !t.contains("set_content_width(") {
                    continue;
                }
                // The hazard is a number LARGER than the shared floor. A
                // smaller one — a preview, a thumbnail — is a widget saying it
                // is small, which is not what takes a panel off the screen.
                let biggest = t
                    .split(|c: char| !c.is_ascii_digit())
                    .filter_map(|n| n.parse::<i32>().ok())
                    .max()
                    .unwrap_or(0);
                if biggest > CONTENT_FLOOR {
                    strays.push(format!("{name}:{}: {t}", i + 1));
                }
            }
        }
        assert!(
            strays.is_empty(),
            "these give a picture area a minimum width of their own instead of \
             `panel::CONTENT_FLOOR`, which is how a docked panel stops \
             docking: {strays:#?}"
        );
    }

    /// Rigid content lets its panel go sooner than flexible content does.
    ///
    /// The two numbers exist because they must differ. Collapsed into one, the
    /// app is back to one of the two bugs this fixed: the viewers' controls
    /// vanish while there is room for them, or the Search form is drawn past
    /// the window edge.
    ///
    /// Read through a function so the compiler cannot fold it away — as a bare
    /// comparison of two constants it is a lint, not a test.
    #[test]
    fn a_form_gives_up_its_panel_before_a_picture_does() {
        fn sp(v: f64) -> f64 {
            std::hint::black_box(v)
        }
        assert!(
            sp(COLLAPSE_RIGID_SP) > sp(COLLAPSE_FLEXIBLE_SP),
            "a form ({COLLAPSE_RIGID_SP}) must hold on to its width longer than \
             a picture ({COLLAPSE_FLEXIBLE_SP})"
        );
    }

    /// Every panel width in the UI comes from here.
    ///
    /// The failure this catches is not a wrong number; it is a second number.
    /// Two panels that are nearly the same width look like a mistake, and the
    /// one that drifts is always the one nobody knew about.
    #[test]
    fn nothing_states_its_own_panel_width() {
        // Widths that mean something other than "a side panel" — a dialog, a
        // thumbnail, a tile — are not this module's business.
        const ALLOWED: &[&str] = &[
            "src/ui/panel.rs",
            // The navigation split view takes its own min/max, in f64, from
            // libadwaita's API rather than a size request.
            "src/ui/main_window.rs",
        ];
        let mut strays: Vec<String> = Vec::new();
        for (path, source) in crate::testing::rust_sources() {
            let rel = path
                .to_string_lossy()
                .rsplit_once("canfar-ubuntu/")
                .map(|(_, r)| r.to_string())
                .unwrap_or_default();
            if ALLOWED.contains(&rel.as_str()) || !rel.starts_with("src/ui/") {
                continue;
            }
            for (i, line) in crate::testing::code(&source).lines().enumerate() {
                let t = line.trim();
                // A side-panel-sized width request, spelled out. Only the
                // `(w, -1)` form: a request with a real height is a tile, a
                // preview or a placeholder, and those are not panels.
                if (t.contains("set_size_request(260, -1)")
                    || t.contains("set_size_request(280, -1)"))
                    && !t.contains("panel::")
                {
                    strays.push(format!("{rel}:{}: {t}", i + 1));
                }
            }
        }
        assert!(
            strays.is_empty(),
            "these state a panel width of their own instead of using \
             `panel::WIDTH`: {strays:#?}"
        );
    }
}
