pub mod agent_badge;
pub mod agent_proposals_dialog;
pub mod ai_connect_wizard;
pub mod ai_guide_page;
pub mod annotations_panel;
pub mod batch_jobs_dialog;
pub mod batch_jobs_view;
pub mod busy;
pub mod canfar_images;
pub mod card;
pub mod coord_chip;
pub mod cube_export;
pub mod cube_slice_view;
pub mod cube_tab_host;
pub mod cube_viewer;
pub mod cube_volume_gl;
pub mod dashboard;
pub mod datalink_file_dialog;
pub mod delete_dialog;
pub mod dialog;
pub mod export_dialog;
pub mod failure_detail;
pub mod figure_plate;
pub mod file_panel;
pub mod fit;
pub mod fits_canvas;
pub mod fits_coords_panel;
pub mod fits_export;
pub mod fits_header_panel;
pub mod fits_tab;
pub mod fits_viewer;
pub mod image_discovery_dialog;
pub mod item_list_section;
pub mod launch_dialog;
pub mod launch_fab;
pub mod launch_form;
pub mod login_dialog;
pub mod main_window;
pub mod mark_label_editor;
pub mod metric_bar;
pub mod notebook_cell;
pub mod notebook_host;
pub mod notebook_page;
pub mod observation_detail_page;
pub mod panel;
pub mod platform_load;
pub mod poll;
pub mod recent_launches;
pub mod recents_section;
pub mod rename_dialog;
pub mod research_page;
pub mod resource_selector;
pub mod saved_query_dialog;
pub mod search_page;
pub mod session_card;
pub mod session_events_dialog;
pub mod session_icon;
pub mod session_list;
pub mod settings_page;
pub mod share_dialog;
pub mod sound;
pub mod space;
pub mod status_bar;
pub mod storage_quota;
pub mod text_viewer_dialog;
pub mod tiles;
pub mod toasts;
pub mod toast;
pub mod vospace_browser;
pub mod workflows_page;

pub use main_window::build_main_window;

use std::cell::RefCell;
use std::rc::Rc;

/// A late-bound, optional UI callback owned by one widget.
///
/// Widgets are constructed before their host knows what to do with their events,
/// so the host installs the handler afterwards — hence `RefCell<Option<_>>`.
/// `Rc` (not `Box`) so a handler can be cloned out and invoked without holding
/// the borrow across the call, which would panic if the handler re-entered the
/// widget.
pub type CallbackSlot<F> = RefCell<Option<Rc<F>>>;

/// A [`CallbackSlot`] shared across clones of a widget handle — the same slot
/// seen by every closure that captured the widget.
pub type SharedCallbackSlot<F> = Rc<CallbackSlot<F>>;
pub mod viewer_shell;
pub mod working_dots;

#[cfg(test)]
mod markup_tests {
    /// A row shown text from outside the app must say it is not markup.
    ///
    /// `AdwPreferencesRow` treats its title and subtitle as Pango markup by
    /// default. A tool description reading `vos:<user>/workflows/` is then an
    /// unclosed `<user>` element: GTK refuses to render it, logs
    ///
    /// ```text
    /// Failed to set text '…' from markup due to error parsing markup:
    /// Element "markup" was closed, but the currently open element is "user"
    /// ```
    ///
    /// once per row per rebuild, and shows nothing where the text should be.
    /// Anything a user typed, a service returned, or a tool author wrote can
    /// contain an angle bracket, and none of it is markup.
    ///
    /// A literal is fine — that text is ours and it is not going to change
    /// underneath us — so only the `&variable` form is checked.
    #[test]
    fn a_row_given_text_from_outside_says_it_is_not_markup() {
        let mut unguarded: Vec<String> = Vec::new();
        for (path, source) in crate::testing::rust_sources() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let code = crate::testing::code(&source);
            // `set_subtitle`, and not `set_title`, on purpose: a subtitle
            // belongs only to an `AdwPreferencesRow` and its descendants, which
            // are exactly the markup-rendering widgets. A `title` is also on
            // `AdwNavigationPage` and `AdwTabPage`, whose titles are plain text
            // with no markup property to set, and telling those apart from the
            // outside took a guess that was wrong about something either way.
            // In practice a row that shows a variable title shows a variable
            // subtitle beside it, and one `set_use_markup(false)` covers both.
            for setter in ["set_subtitle(&"] {
                for (at, _) in code.match_indices(setter) {
                    // The receiver, and how far back to look for its
                    // `use_markup`: the same window the popover guard in
                    // `viewer_shell` uses, for the same reason — a widget is
                    // configured within a few lines of being built.
                    // Floored to a char boundary: the window is a byte count,
                    // and landing inside a multi-byte character panics the
                    // guard instead of reporting on it. A file with a box-
                    // drawing comment near a `set_subtitle` is enough.
                    let mut start = at.saturating_sub(900);
                    while start < at && !code.is_char_boundary(start) {
                        start += 1;
                    }
                    if code[start..at].contains("set_use_markup(false)") {
                        continue;
                    }
                    let line = code[..at].lines().count();
                    let snippet: String = code[at..].lines().next().unwrap_or("").into();
                    unguarded.push(format!("{name}:{line}: {}", snippet.trim()));
                }
            }
        }
        assert!(
            unguarded.is_empty(),
            "these rows render text from outside the app as Pango markup, so an \
             angle bracket in it means the text does not appear at all: \
             {unguarded:#?}"
        );
    }
}
