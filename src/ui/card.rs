//! The card every Portal component sits in.
//!
//! Seven widgets each built this by hand: a vertical box, the `card` style, four
//! margin calls, then a heading row with its own four. Five used
//! `card_header`, two wrote their own — so two titles were a different size
//! from the other five, and a heading sat at a different distance from its
//! frame depending on which file you were in.
//!
//! One type owns the frame, the heading and where content goes. A component
//! says what it is and appends its content; it does not decide what a card
//! looks like, because then there are seven answers to that.

use crate::ui::space;
use gtk4::prelude::*;
use gtk4::{self as gtk};

/// Turn a card's frame off, or back on.
///
/// A card's frame says "separate object on a page". Inside a dialog there is no
/// page and nothing to be separate from, so the frame draws a second box just
/// inside the first — visible in the launch modal as a white card sitting on
/// the dialog's own ground, one inset in from every edge.
///
/// Lives here because the class name does: a caller toggling it would have to
/// name `"card"` itself, which is the thing the guard below forbids.
pub fn set_framed(widget: &gtk::Box, framed: bool) {
    if framed {
        widget.add_css_class("card");
    } else {
        widget.remove_css_class("card");
    }
}

/// A titled card: frame, heading row, and a content area to fill.
pub struct Card {
    /// The whole card — put this in the layout.
    pub widget: gtk::Box,
    /// Append content here. Already inset from the frame.
    pub content: gtk::Box,
    /// Shown while the card is loading. Hidden until asked for.
    pub spinner: gtk::Spinner,
    /// The heading row, for a card that needs its own action beside the title.
    pub header: gtk::Box,
}

impl Card {
    /// A card titled `title`, with a spinner beside the heading.
    pub fn new(title: &str) -> Self {
        let widget = gtk::Box::new(gtk::Orientation::Vertical, space::CARD);
        widget.add_css_class("card");
        space::edge_all(&widget);

        let header = gtk::Box::new(gtk::Orientation::Horizontal, space::ROW);
        header.set_margin_start(space::CARD);
        header.set_margin_end(space::CARD);
        header.set_margin_top(space::CARD);

        let label = gtk::Label::new(Some(title));
        label.add_css_class("title-4");
        label.set_halign(gtk::Align::Start);
        label.set_hexpand(true);
        header.append(&label);

        let spinner = gtk::Spinner::new();
        spinner.set_visible(false);
        header.append(&spinner);

        widget.append(&header);

        // Content is inset on the sides and the bottom; the heading above
        // already provides the top gap.
        let content = gtk::Box::new(gtk::Orientation::Vertical, space::CARD);
        content.set_margin_start(space::CARD);
        content.set_margin_end(space::CARD);
        content.set_margin_bottom(space::CARD);
        content.set_vexpand(true);
        widget.append(&content);

        Card {
            widget,
            content,
            spinner,
            header,
        }
    }

    /// Add a refresh button to the heading and hand it back.
    ///
    /// Not every card can be refreshed, so it is asked for rather than always
    /// present — a button that does nothing is worse than no button.
    pub fn with_refresh(&self) -> gtk::Button {
        let btn = gtk::Button::from_icon_name("view-refresh-symbolic");
        btn.add_css_class("flat");
        btn.set_valign(gtk::Align::Center);
        btn.set_tooltip_text(Some(crate::tr_en!("Refresh")));
        self.header.append(&btn);
        btn
    }

    /// Add an action button to the heading, left of any refresh.
    pub fn with_action(&self, icon: &str, tooltip: &str) -> gtk::Button {
        let btn = gtk::Button::from_icon_name(icon);
        btn.add_css_class("flat");
        btn.set_valign(gtk::Align::Center);
        btn.set_tooltip_text(Some(tooltip));
        self.header.append(&btn);
        btn
    }
}

#[cfg(test)]
mod tests {
    //! Every Portal component must get its card from here, or the Portal is
    //! seven components that each look slightly different.

    const PORTAL_CARDS: &[(&str, &str)] = &[
        ("session_list", include_str!("session_list.rs")),
        ("storage_quota", include_str!("storage_quota.rs")),
        ("batch_jobs_view", include_str!("batch_jobs_view.rs")),
        ("platform_load", include_str!("platform_load.rs")),
        ("recent_launches", include_str!("recent_launches.rs")),
        ("canfar_images", include_str!("canfar_images.rs")),
        // The eighth, and the one this list originally missed — so it kept a
        // hand-rolled header with no margins while the guard reported the
        // Portal consistent. A list of files to check is only as good as the
        // list.
        ("launch_form", include_str!("launch_form.rs")),
    ];

    /// Every component the Portal grid holds. Checked against the source so a
    /// ninth card cannot be added without joining the list above.
    #[test]
    fn the_list_holds_every_component_the_portal_shows() {
        let dashboard = crate::testing::code(include_str!("dashboard.rs"));
        for (name, _) in PORTAL_CARDS {
            let ty: String = name
                .split('_')
                .map(|w| {
                    let mut c = w.chars();
                    c.next()
                        .map(|f| f.to_uppercase().to_string() + c.as_str())
                        .unwrap_or_default()
                })
                .collect();
            // `session_list` builds `SessionListView`, `canfar_images` builds
            // `CanfarImagesView`, and so on.
            assert!(
                dashboard.contains(&ty) || dashboard.contains(name),
                "{name} is guarded but the Portal does not show it"
            );
        }
        // And nothing the Portal shows is missing from the list — counted out
        // of `dashboard.rs` rather than written down.
        //
        // This used to assert against a hand-kept list plus the literal `8`, so
        // adding a card meant remembering to edit three places and removing one
        // meant editing three again. A count derived from the Portal itself
        // cannot be forgotten: every card is built as `X::new(services.clone())`
        // there, and the launch form takes the session list too, so it is
        // counted separately.
        let built = dashboard.matches("::new(services.clone())").count()
            + dashboard.matches("LaunchFormView::new(").count();
        assert_eq!(
            PORTAL_CARDS.len(),
            built,
            "the Portal builds {built} components; the guard checks {}",
            PORTAL_CARDS.len()
        );
    }

    #[test]
    fn no_portal_component_builds_its_own_card() {
        for (name, source) in PORTAL_CARDS {
            let code = crate::testing::code(source);
            assert!(
                !code.contains(r#"add_css_class("card")"#),
                "{name} styles its own card; the frame, the heading size and the \
                 inset are then its own opinion. Use ui::card::Card."
            );
        }
    }

    #[test]
    fn a_portal_component_does_not_set_its_own_edges() {
        // The four-margin block is where the raggedness came from: a component
        // that insets itself is a component that can inset itself differently.
        for (name, source) in PORTAL_CARDS {
            let code = crate::testing::code(source);
            let hand_written = code.matches("set_margin_start(12)").count();
            assert_eq!(
                hand_written, 0,
                "{name} still writes its own card inset {hand_written} time(s)"
            );
        }
    }
}
