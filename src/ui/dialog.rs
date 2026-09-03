//! The shell every modal shares: a header, content that scrolls, actions that
//! do not.
//!
//! Seventeen dialogs each built this by hand. Three pinned their buttons in a
//! bottom bar; five put their content in no scroller at all. A dialog in that
//! second group has nothing holding its buttons on screen: content taller than
//! the window pushes them past the bottom edge, and the user reaches for a
//! Done button that is not there.
//!
//! Two rules, and they are the whole point of the type:
//!
//! 1. **Content scrolls.** It lives in a `GtkScrolledWindow`, so no amount of
//!    it can make the window taller than the space available. Horizontal
//!    scrolling is off — sideways overflow is a layout fault to fix, not to
//!    scroll past (see [`crate::ui::fit`]).
//! 2. **Actions do not.** They sit in the toolbar view's bottom bar, outside
//!    the scroller, so they are visible whatever the content does.
//!
//! Width comes from [`crate::ui::fit`]'s vocabulary; the inset is
//! [`crate::ui::space`]'s. This type owns the arrangement, not the numbers.

use gtk4::prelude::*;
use gtk4::{self as gtk};
use libadwaita as adw;
use libadwaita::prelude::*;

use crate::ui::space;

/// A modal built to the shared shape.
///
/// Kept as a struct rather than a builder function because callers need the
/// pieces back — the window to close it, the content box to fill, the action
/// row to put buttons in — and returning a tuple of three would leave the
/// caller to remember which is which.
pub struct Dialog {
    /// The window itself: present it, close it, make it transient.
    pub window: adw::Window,
    /// Where the caller's widgets go. Already inset and already inside the
    /// scroller.
    content: gtk::Box,
    /// The bottom bar. Empty until something is added, and hidden while empty
    /// so a dialog with no actions has no stray strip along its bottom.
    actions: gtk::Box,
}

impl Dialog {
    /// A modal `width` wide, titled `title`, no taller than `height`.
    ///
    /// `width` is a role from [`crate::ui::fit`] — `PROMPT`, `FORM`, `DETAIL`,
    /// `BROWSE` — not a number.
    ///
    /// `height` is a CAP, not a size. A dialog with little in it stays short;
    /// one with more than `height` of content stops there and scrolls the rest.
    /// Without the cap a window grows to whatever its content asks for — a
    /// 560px dialog measured 2034px in `examples/wizard_height_probe.rs` — and
    /// a window taller than the display puts its own action row below the
    /// bottom edge of the screen, which is where the buttons went.
    pub fn new(title: &str, width: i32, height: i32) -> Self {
        // No `default_height`. The scroller below already says "as tall as the
        // content, and no taller than `height`", and a default height fights
        // it: the window opens at that size whatever the content asks for, so a
        // short form gets a band of dead space under it and the buttons sit far
        // from the thing they act on. The launch form showed 250px of nothing.
        let window = adw::Window::builder()
            .title(title)
            .default_width(width)
            .modal(true)
            .resizable(true)
            .build();

        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&adw::HeaderBar::new());

        let content = gtk::Box::new(gtk::Orientation::Vertical, space::CARD);
        space::edge_all(&content);

        // Never horizontally: a dialog that scrolls sideways is one whose
        // content sets a floor wider than the dialog, and that is a bug in the
        // content. Vertically always, so the actions below can never be pushed
        // off the bottom.
        // `propagate_natural_height` keeps a short dialog short — a rename
        // prompt should not open at the height of a wizard. `max_content_height`
        // is what stops the other end: past it the window stays put and the
        // content scrolls. Together they are the whole height policy, which is
        // why the window sets no default height of its own.
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .propagate_natural_height(true)
            .max_content_height(height)
            .vexpand(true)
            .child(&content)
            .build();
        toolbar.set_content(Some(&scroller));

        let actions = space::action_row(space::CONTROL);
        actions.set_visible(false);
        // A hexpanding spacer so secondary actions sit left and primary ones
        // right, without either caller doing the algebra.
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        actions.append(&spacer);
        toolbar.add_bottom_bar(&actions);

        window.set_content(Some(&toolbar));

        Dialog {
            window,
            content,
            actions,
        }
    }

    /// The box the caller fills. Inset, vertical, already scrolling.
    pub fn content(&self) -> &gtk::Box {
        &self.content
    }

    /// An action on the right — the one that completes the dialog.
    pub fn add_action(&self, button: &impl IsA<gtk::Widget>) {
        self.actions.append(button);
        self.actions.set_visible(true);
    }

    /// An action on the left — Cancel, Back, anything that steps away.
    ///
    /// Placed before the spacer so it stays left however many actions the
    /// right-hand side collects.
    pub fn add_secondary_action(&self, button: &impl IsA<gtk::Widget>) {
        self.actions
            .insert_child_after(button, None::<&gtk::Widget>);
        self.actions.set_visible(true);
    }

    /// Show it, transient for the window `parent` belongs to.
    pub fn present(&self, parent: &impl IsA<gtk::Widget>) {
        self.window
            .set_transient_for(anchor_window(parent).as_ref());
        self.window.present();
    }
}

/// The window a new dialog should hang from.
///
/// Not simply `widget.root()`. A widget can live INSIDE a dialog — the launch
/// form does, now that it is presented in one — and a modal parented to another
/// modal is at the window manager's mercy: when the inner one closed, the new
/// dialog was left with no visible parent, dropped behind the main window, and
/// went on holding the input grab. The app looked frozen, and the only thing
/// wrong was a dialog nobody could see.
///
/// So: the widget's toplevel, and if that toplevel is itself transient for
/// something, the window at the end of that chain — the real application
/// window.
/// A height cap of `fraction` of the window this dialog opens over.
///
/// For a dialog whose content varies — the launch form has three tabs of very
/// different lengths — a fixed cap is either too short for the longest or too
/// tall for the shortest. Tying it to the window means the cap is "most of what
/// you can see" on any display, rather than a number chosen against one.
///
/// Falls back to `default_cap` before the parent is mapped, which is when its
/// height reads as zero.
pub fn viewport_share(parent: &impl IsA<gtk::Widget>, fraction: f64, default_cap: i32) -> i32 {
    let height = anchor_window(parent).map(|w| w.height()).unwrap_or(0);
    if height <= 0 {
        return default_cap;
    }
    ((height as f64) * fraction).round() as i32
}

pub fn anchor_window(widget: &impl IsA<gtk::Widget>) -> Option<gtk::Window> {
    let mut window = widget.root().and_downcast::<gtk::Window>()?;
    // Bounded: a transient chain is two or three deep in this app, and a cycle
    // would otherwise hang the UI it is meant to protect.
    for _ in 0..8 {
        match window.transient_for() {
            Some(parent) => window = parent,
            None => break,
        }
    }
    Some(window)
}

#[cfg(test)]
mod tests {
    /// Every modal is built through this module.
    ///
    /// The list is the debt, and it is here rather than absent so it is
    /// countable: a file on it builds its own shell and may have no scroller
    /// under its content, which is how a Done button ends up past the bottom
    /// edge. A file NOT on it may not start.
    #[test]
    fn a_dialog_is_as_tall_as_its_content_and_no_taller() {
        // `height` is a CAP, applied to the scroller. Setting it as the
        // window's default height as well makes every dialog open at the cap
        // whatever it holds, so a short form gets a band of dead space and its
        // buttons sit far from the thing they act on — 250px of nothing under
        // the launch form's Standard tab.
        let code = crate::testing::without_comments(crate::testing::code(include_str!(
            "dialog.rs"
        )));
        let at = code.find("fn new(").expect("Dialog::new is gone");
        let body = &code[at..(at + 1200).min(code.len())];
        assert!(
            !body.contains(".default_height("),
            "Dialog sets a default height, which overrides the hug-to-content policy"
        );
        assert!(
            body.contains("propagate_natural_height(true)") && body.contains("max_content_height"),
            "the height policy is no longer 'as tall as the content, capped'"
        );
    }

    #[test]
    fn every_dialog_anchors_to_a_real_window_not_to_another_dialog() {
        // A modal parented to a modal that then closes is left behind the main
        // window still holding the input grab — the app appears frozen and the
        // cause is invisible. That is what `.root()` gives you: the toplevel the
        // widget happens to be in, which may itself be a dialog.
        //
        // Flags the risky idiom specifically. A dialog handed an explicit window
        // it already holds is fine; one that reaches for `.root()` is not.
        let mut bare: Vec<String> = Vec::new();
        for (path, source) in crate::testing::rust_sources() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name == "dialog.rs" {
                continue; // where anchor_window lives
            }
            let code = crate::testing::without_comments(crate::testing::code(&source));
            for needle in ["set_transient_for(", "MessageDialog::new("] {
                for (at, _) in code.match_indices(needle) {
                    let arg = &code[at..(at + 160).min(code.len())];
                    if arg.contains(".root()") {
                        bare.push(format!("{name}:{}", code[..at].lines().count()));
                    }
                }
            }
        }
        bare.sort();
        assert!(
            bare.is_empty(),
            "dialog(s) taking their parent from `.root()`, which is the window \
             they were raised FROM and may itself be a dialog that is closing — \
             use `ui::dialog::anchor_window`: {bare:#?}"
        );
    }

    #[test]
    fn no_new_dialog_builds_its_own_shell() {
        // Not yet migrated. Shrink this; never extend it.
        const HAND_ROLLED: &[&str] = &[
            "agent_proposals_dialog.rs",
            "ai_guide_page.rs",
            "batch_jobs_dialog.rs",
            "datalink_file_dialog.rs",
            "image_discovery_dialog.rs",
            "main_window.rs",
            "notebook_host.rs",
            "saved_query_dialog.rs",
            "search_page/mod.rs",
            "settings_page.rs",
            "text_viewer_dialog.rs",
            "vospace_browser.rs",
        ];

        let mut rogue = Vec::new();
        for (path, text) in crate::testing::rust_sources() {
            let p = path.to_string_lossy().to_string();
            if !p.contains("/ui/") || p.ends_with("/ui/dialog.rs") {
                continue;
            }
            let code = crate::testing::without_comments(crate::testing::code(&text));
            if !code.contains("adw::Window::builder()") && !code.contains("gtk::Window::builder()")
            {
                continue;
            }
            if HAND_ROLLED.iter().any(|f| p.ends_with(f)) {
                continue;
            }
            rogue.push(p);
        }

        assert!(
            rogue.is_empty(),
            "modal(s) building their own shell. Use `ui::dialog::Dialog`, which \
             scrolls its content and pins its actions, so the buttons cannot be \
             pushed off the bottom edge: {rogue:#?}"
        );
    }
}
