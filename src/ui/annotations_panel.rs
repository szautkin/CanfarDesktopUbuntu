//! The list of marks drawn on a viewer, with what to do about each.
//!
//! One panel for the FITS viewer and the cube: the two differ only in where
//! their marks come from, which is a pair of callbacks, so a second copy of
//! this would be a second place to fix a bug in the list.
//!
//! It is also the only way to reach a mark whose subject is off screen —
//! selecting one is how you find it again after panning away.

use crate::models::annotation::Annotation;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use libadwaita as adw;
use libadwaita::prelude::*;
use std::rc::Rc;

pub struct AnnotationsPanel {
    section: Rc<crate::ui::item_list_section::ItemListSection>,
    clear_button: gtk::Button,
    draw_slot: gtk::Box,
}

impl AnnotationsPanel {
    pub fn new() -> Rc<Self> {
        use crate::ui::item_list_section::{ItemListSection, RowActions, SectionSpec};

        let section = ItemListSection::new(SectionSpec {
            actions: RowActions::EDIT_AND_DELETE,
            filter_placeholder: Some(crate::tr_en!("Filter marks…")),
            empty_message: crate::tr_en!(
                "Nothing marked yet. Turn on Draw in the toolbar, then click the image."
            ),
            selectable: true,
            monospace: false,
        });

        let hint = gtk::Label::new(Some(crate::tr_en!(
            "Click a mark to edit it: drag a grip to resize, the shape to move it, then confirm."
        )));
        hint.add_css_class("dim-label");
        hint.add_css_class("caption");
        hint.set_wrap(true);
        hint.set_xalign(0.0);
        section.prepend_control(&hint);

        // Where the drawing controls go. They used to be a section of their own
        // further up the sidebar — a toggle and a shape picker under their own
        // heading — which put making a mark and looking at your marks in two
        // different places.
        let draw_slot = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        draw_slot.set_halign(gtk::Align::Start);
        let draw_title = gtk::Label::new(Some(crate::tr_en!("Add Mark")));
        draw_title.add_css_class("heading");
        draw_title.set_halign(gtk::Align::Start);
        section.append_control(&draw_title);
        section.append_control(&draw_slot);

        let clear_button = gtk::Button::with_label(crate::tr_en!("Clear all marks"));
        clear_button.add_css_class("destructive-action");
        clear_button.set_halign(gtk::Align::Start);
        section.append_control(&clear_button);

        Rc::new(Self {
            section,
            clear_button,
            draw_slot,
        })
    }

    pub fn widget(&self) -> &gtk::Box {
        self.section.widget()
    }

    /// Put the viewer's drawing controls at the top of this section.
    pub fn set_draw_controls(&self, controls: &impl IsA<gtk::Widget>) {
        self.draw_slot.append(controls.as_ref());
    }

    pub fn set_on_select(&self, f: impl Fn(&str) + 'static) {
        self.section.set_on_select(f);
    }

    pub fn set_on_edit(&self, f: impl Fn(&str) + 'static) {
        self.section.set_on_edit(f);
    }

    pub fn set_on_delete(&self, f: impl Fn(&str) + 'static) {
        self.section.set_on_delete(f);
    }

    /// Ask before clearing: it takes the user's own marks along with an agent's
    /// and nothing brings them back. Deleting ONE mark does not ask — one mark
    /// is easy to redraw and its button is right beside it.
    pub fn set_on_clear(&self, f: impl Fn(&str) + 'static) {
        let cb: Rc<dyn Fn(&str)> = Rc::new(f);
        let widget = self.section.widget().clone();
        self.clear_button.connect_clicked(move |_| {
            let root = widget.root().and_then(|r| r.downcast::<gtk::Window>().ok());
            let dialog = adw::MessageDialog::new(
                root.as_ref(),
                Some(crate::tr_en!("Delete every mark?")),
                Some(crate::tr_en!(
                    "This removes all marks on this image, including any an agent made. \
                     It cannot be undone."
                )),
            );
            dialog.add_response("cancel", crate::tr_en!("Cancel"));
            dialog.add_response("clear", crate::tr_en!("Delete all"));
            dialog.set_response_appearance("clear", adw::ResponseAppearance::Destructive);
            dialog.set_default_response(Some("cancel"));
            dialog.set_close_response("cancel");
            let cb = cb.clone();
            dialog.connect_response(None, move |_, response| {
                if response == "clear" {
                    cb("");
                }
            });
            dialog.present();
        });
    }

    /// Redraw the list from `annotations`.
    pub fn set_annotations(&self, annotations: &[Annotation], selected: Option<&str>) {
        let items: Vec<crate::ui::item_list_section::ListItem> = annotations
            .iter()
            .map(|a| crate::ui::item_list_section::ListItem {
                id: a.id.clone(),
                title: display_title(a),
                subtitle: describe(a),
            })
            .collect();
        let count = (!items.is_empty()).then(|| crate::tr_fmt!("{} marks", items.len()));
        // The canvas owns which mark is chosen, so this states it.
        self.section.set_items(
            &items,
            crate::ui::item_list_section::Selection::Set(selected),
            count,
        );
        self.clear_button.set_sensitive(!annotations.is_empty());
    }
}

/// The row's headline: its text, or its kind when it has none.
fn display_title(a: &Annotation) -> String {
    if a.text.trim().is_empty() {
        crate::tr_fmt!("({})", kind_label(a))
    } else {
        a.text.clone()
    }
}

fn kind_label(a: &Annotation) -> String {
    use crate::models::annotation::AnnotationKind::*;
    match a.kind {
        Rect => crate::tr_en!("box").to_string(),
        Circle => crate::tr_en!("circle").to_string(),
        Callout => crate::tr_en!("callout").to_string(),
        Text => crate::tr_en!("text").to_string(),
    }
}

/// Where it is, and who drew it.
///
/// The author is shown because an agent's marks and a person's sit in the same
/// list, and deleting someone else's work by mistake is the thing to prevent.
fn describe(a: &Annotation) -> String {
    use crate::models::annotation::{Anchor, Author};
    // `tr_fmt!` fills `{}` with Display values and does not read format specs,
    // so the rounding happens here and the template stays translatable.
    let place = match a.anchor {
        Anchor::ImagePixel { x, y } => {
            crate::tr_fmt!("pixel {}, {}", format!("{x:.0}"), format!("{y:.0}"))
        }
        Anchor::Sky { ra_deg, dec_deg } => {
            crate::tr_fmt!("{}°, {}°", format!("{ra_deg:.4}"), format!("{dec_deg:.4}"))
        }
        Anchor::Data { x, y, z } => crate::tr_fmt!(
            "voxel {}, {}, ch {}",
            format!("{x:.0}"),
            format!("{y:.0}"),
            format!("{z:.0}")
        ),
    };
    match a.author {
        Author::Agent => crate::tr_fmt!("{} — {} — by the agent", kind_label(a), place),
        Author::User => crate::tr_fmt!("{} — {}", kind_label(a), place),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::annotation::{Anchor, AnnotationKind, Author};

    fn mark(kind: AnnotationKind, text: &str, author: Author) -> Annotation {
        Annotation::new(kind, Anchor::ImagePixel { x: 12.0, y: 34.0 }, text, author)
    }

    #[test]
    fn a_labelled_mark_shows_its_label() {
        let a = mark(AnnotationKind::Circle, "NGC 5194", Author::User);
        assert_eq!(display_title(&a), "NGC 5194");
    }

    /// A bare shape still says what it is.
    #[test]
    fn an_unlabelled_mark_shows_its_kind() {
        let a = mark(AnnotationKind::Rect, "", Author::User);
        let title = display_title(&a);
        assert!(!title.is_empty(), "an unlabelled row would be blank");
        assert!(title.contains("box"), "{title}");
    }

    /// An agent's mark says so.
    ///
    /// The two authors share one list, and the row is where someone decides
    /// whether a mark is theirs to delete.
    #[test]
    fn an_agents_mark_is_named_as_one() {
        let theirs = describe(&mark(AnnotationKind::Circle, "x", Author::Agent));
        let mine = describe(&mark(AnnotationKind::Circle, "x", Author::User));
        assert!(theirs.contains("agent"), "{theirs}");
        assert!(!mine.contains("agent"), "{mine}");
    }

    /// Each anchor space reads in its own units.
    #[test]
    fn the_position_is_shown_in_the_units_it_was_stored_in() {
        let pixel = describe(&mark(AnnotationKind::Circle, "", Author::User));
        assert!(pixel.contains("pixel"), "{pixel}");

        let sky = Annotation::new(
            AnnotationKind::Circle,
            Anchor::Sky {
                ra_deg: 202.4696,
                dec_deg: 47.1953,
            },
            "",
            Author::User,
        );
        let sky = describe(&sky);
        assert!(sky.contains("202.4696"), "{sky}");
        assert!(sky.contains('°'), "{sky}");

        let voxel = Annotation::new(
            AnnotationKind::Circle,
            Anchor::Data {
                x: 32.0,
                y: 40.0,
                z: 12.0,
            },
            "",
            Author::User,
        );
        let voxel = describe(&voxel);
        assert!(voxel.contains("voxel"), "{voxel}");
        assert!(voxel.contains("ch 12"), "{voxel}");
    }
}
