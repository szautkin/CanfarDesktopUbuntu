//! The Portal's floating launch button.
//!
//! The launch form used to occupy two thirds of the Portal's second row
//! permanently, whether or not anyone was launching anything — the largest
//! single block of space on the page, spent on a form that is idle almost all
//! the time. It now lives in a modal, and this is the way in: a circular button
//! pinned to the bottom-right corner whose popover offers the three forms.
//!
//! The button reports WHICH form was asked for; opening it is the caller's job
//! (`dashboard.rs`), because the form belongs to the dashboard and outlives any
//! one modal.

use crate::ui::launch_form::LaunchTab;
use crate::ui::space;
use gtk4::prelude::*;
use gtk4::{self as gtk};

/// Width of the popover's option list.
///
/// Matches the notebook host's menus (`notebook_host.rs`), which are the same
/// shape: a short vertical stack of flat buttons that should not resize as the
/// labels are translated.
const MENU_WIDTH: i32 = 180;

/// Diameter of the button.
///
/// Roughly twice a toolbar button. It is the only floating control on the page
/// and the one thing someone comes to the Portal to do, so it is sized to be
/// found without looking — a flat icon button at the default 34px reads as
/// another piece of card chrome.
const FAB_SIZE: i32 = 64;

/// Icon size inside it, scaled to match.
const FAB_ICON: i32 = 24;

/// A floating action button offering the three launch forms.
pub struct LaunchFab {
    /// The overlay: the Portal's content with the button floating over it.
    widget: gtk::Overlay,
}

impl LaunchFab {
    /// Float a launch button over `content`.
    ///
    /// `on_pick` fires with the chosen tab once the popover has closed.
    pub fn new(content: &impl IsA<gtk::Widget>, on_pick: impl Fn(LaunchTab) + 'static) -> Self {
        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(content));

        let button = gtk::MenuButton::new();
        button.set_icon_name("list-add-symbolic");
        // `circular` + `suggested-action`: the only floating control on the
        // page, and the one thing someone comes to the Portal to do.
        button.add_css_class("circular");
        button.add_css_class("suggested-action");
        button.set_tooltip_text(Some(crate::tr_en!("Launch a session or batch job")));
        button.set_size_request(FAB_SIZE, FAB_SIZE);
        if let Some(icon) = button.child().and_downcast::<gtk::Image>() {
            icon.set_pixel_size(FAB_ICON);
        }
        button.set_halign(gtk::Align::End);
        button.set_valign(gtk::Align::End);
        space::inset(&button, space::EDGE);

        let on_pick = std::rc::Rc::new(on_pick);
        let menu = gtk::Box::new(gtk::Orientation::Vertical, space::ROW / 2);
        space::inset(&menu, space::ROW);
        menu.set_size_request(MENU_WIDTH, -1);

        let popover = gtk::Popover::new();
        popover.set_child(Some(&menu));

        for tab in LaunchTab::ORDER {
            let item = gtk::Button::with_label(tab.label());
            item.add_css_class("flat");
            let on_pick = on_pick.clone();
            let popover = popover.clone();
            item.connect_clicked(move |_| {
                // A popover does not close when something inside it is
                // activated — the notebook's menus learned this the hard way —
                // so it would still be sitting over the modal we are about to
                // open.
                popover.popdown();
                on_pick(tab);
            });
            menu.append(&item);
        }

        // Owned by the MenuButton rather than hand-parented, so the popover
        // goes when the button does and click / Enter / tap all work.
        button.set_popover(Some(&popover));

        // Hover opens it too. Deliberately IN ADDITION to the click the
        // MenuButton already handles: hover alone cannot be reached by a
        // keyboard or a touchscreen, so it is an accelerator, not the door.
        {
            let target = button.clone();
            let motion = gtk::EventControllerMotion::new();
            motion.connect_enter(move |_, _, _| {
                target.popup();
            });
            button.add_controller(motion);
        }

        overlay.add_overlay(&button);
        LaunchFab { widget: overlay }
    }

    /// The overlay to put where the content used to go.
    pub fn widget(&self) -> &gtk::Overlay {
        &self.widget
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE: &str = include_str!("launch_fab.rs");

    #[test]
    fn every_tab_is_offered_and_none_is_named_twice() {
        // The menu is built by iterating `LaunchTab::ORDER`, so a tab added to
        // the form appears here without anyone remembering to add it. This
        // guards the iteration rather than a hand-written list.
        let code = crate::testing::without_comments(crate::testing::code(SOURCE));
        assert!(
            code.contains("for tab in LaunchTab::ORDER"),
            "the launch menu no longer enumerates the tabs, so a new one would \
             be silently unreachable"
        );
        assert_eq!(LaunchTab::ORDER.len(), 3);
    }

    #[test]
    fn the_popover_closes_before_the_modal_opens() {
        // GTK does not close a popover when a button inside it is activated.
        // Left open it floats above the dialog that was just presented.
        let code = crate::testing::without_comments(crate::testing::code(SOURCE));
        let at = code
            .find("item.connect_clicked")
            .expect("the menu items no longer do anything");
        let handler = &code[at..(at + 320).min(code.len())];
        let popdown = handler.find("popdown()").expect("the popover is left open");
        let pick = handler.find("on_pick(").expect("the pick is not reported");
        assert!(
            popdown < pick,
            "the modal opens before the popover closes, so the popover ends up \
             over the dialog"
        );
    }

    #[test]
    fn hover_is_an_accelerator_and_not_the_only_way_in() {
        // A hover-only control is unreachable by keyboard and by touch. The
        // MenuButton keeps its own click handling; the motion controller is
        // additional.
        let code = crate::testing::without_comments(crate::testing::code(SOURCE));
        assert!(
            code.contains("EventControllerMotion"),
            "hover no longer opens the launch menu"
        );
        assert!(
            code.contains("button.set_popover(Some(&popover))"),
            "the popover is not owned by the MenuButton, so click and keyboard \
             activation are gone"
        );
    }
}
