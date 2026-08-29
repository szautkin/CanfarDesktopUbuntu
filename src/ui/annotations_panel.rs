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
use std::cell::RefCell;
use std::rc::Rc;

type Callback = Rc<RefCell<Option<Box<dyn Fn(&str)>>>>;

pub struct AnnotationsPanel {
    widget: gtk::Box,
    list: gtk::ListBox,
    empty: gtk::Label,
    count_label: gtk::Label,
    draw_slot: gtk::Box,
    clear_button: gtk::Button,
    /// Called with an id when a row is chosen.
    on_select: Callback,
    /// Called with an id when a row's delete is pressed.
    on_delete: Callback,
    /// Called with an id when a row's edit is pressed.
    on_edit: Callback,
    /// Called (with an empty id) when Clear all is pressed.
    on_clear: Callback,
    /// Which row is chosen, so a second click on it can mean "never mind".
    selected_id: RefCell<Option<String>>,
    /// True while the list is being repopulated.
    ///
    /// Rebuilding selects a row, and selecting a row tells the viewer, which
    /// refreshes the panel, which rebuilds... The first version also connected
    /// the handler inside the rebuild, so each pass added another one and every
    /// one of them fired. It ended as a stack overflow while placing a mark.
    rebuilding: std::cell::Cell<bool>,
}

impl AnnotationsPanel {
    pub fn new() -> Rc<Self> {
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 10);
        // No vexpand. The sidebar is a Box inside a ScrolledWindow: a child
        // that asks to expand makes the column try to fit the viewport rather
        // than its contents, and a ScrolledWindow's minimum height is zero — so
        // the header list next door was crushed to a couple of rows while this
        // panel took the slack.
        widget.set_margin_top(10);
        widget.set_margin_bottom(12);

        let count_label = gtk::Label::new(None);
        count_label.add_css_class("dim-label");
        count_label.add_css_class("caption");
        count_label.set_halign(gtk::Align::Start);
        widget.append(&count_label);

        let hint = gtk::Label::new(Some(crate::tr_en!(
            "Click a mark to edit it: drag a grip to resize, the shape to move it, then confirm."
        )));
        hint.add_css_class("dim-label");
        hint.add_css_class("caption");
        hint.set_wrap(true);
        hint.set_xalign(0.0);
        widget.append(&hint);

        let empty = gtk::Label::new(Some(crate::tr_en!(
            "Nothing marked yet. Turn on Draw in the toolbar, then click the image."
        )));
        empty.add_css_class("dim-label");
        empty.set_wrap(true);
        empty.set_xalign(0.0);
        widget.append(&empty);

        let list = gtk::ListBox::new();
        list.add_css_class("boxed-list");
        list.set_selection_mode(gtk::SelectionMode::Single);
        // No inner ScrolledWindow. The sidebar column is already one, and a
        // scroller nested in it reports a natural height of nearly nothing —
        // so inside an Expander, which sizes to its child's natural height, the
        // list collapsed and the rows were present and invisible.
        // `propagate_natural_height` had been papering over that; removing it
        // alongside a bad `min_content_height` took the list's height with it.
        //
        // Without the nesting the ListBox reports the height of its rows, all
        // of them are visible, and the sidebar scrolls when there are many —
        // one scroller doing the work instead of two negotiating.
        widget.append(&list);

        // Where the drawing controls go. They used to be a section of their own
        // further up the sidebar — a toggle and a shape picker under their own
        // heading —
        // which put making a mark and looking at your marks in two different
        // places. The viewer still owns the widgets; this panel decides where
        // in the section they sit.
        let draw_title = gtk::Label::new(Some(crate::tr_en!("Add Mark")));
        draw_title.add_css_class("heading");
        draw_title.set_halign(gtk::Align::Start);
        widget.append(&draw_title);

        let draw_slot = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        draw_slot.set_halign(gtk::Align::Start);
        widget.append(&draw_slot);

        let clear_button = gtk::Button::with_label(crate::tr_en!("Clear all marks"));
        clear_button.add_css_class("destructive-action");
        clear_button.set_halign(gtk::Align::Start);
        widget.append(&clear_button);

        let panel = Rc::new(Self {
            widget,
            list,
            empty,
            count_label,
            draw_slot,
            clear_button,
            on_select: Rc::new(RefCell::new(None)),
            on_delete: Rc::new(RefCell::new(None)),
            on_edit: Rc::new(RefCell::new(None)),
            on_clear: Rc::new(RefCell::new(None)),
            selected_id: RefCell::new(None),
            rebuilding: std::cell::Cell::new(false),
        });

        {
            // Clearing takes the user's own marks as well as an agent's, and
            // nothing brings them back — so it asks. The other destructive
            // action, deleting one mark, does not: one mark is easy to redraw
            // and the button is right beside it.
            let cb = panel.on_clear.clone();
            let widget = panel.widget.clone();
            panel.clear_button.connect_clicked(move |_| {
                // `AdwMessageDialog`, as the research page's own delete
                // confirmation uses — one dialog style across the app.
                let root = widget.root().and_then(|r| r.downcast::<gtk::Window>().ok());
                let dialog = adw::MessageDialog::new(
                    root.as_ref(),
                    Some(crate::tr_en!("Delete every mark?")),
                    Some(crate::tr_en!(
                        "This removes all marks on this image, including any an agent \
                         made. It cannot be undone."
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
                        if let Some(f) = cb.borrow().as_ref() {
                            f("");
                        }
                    }
                });
                dialog.present();
            });
        }
        {
            // Once, here — not inside the rebuild.
            let cb = panel.on_select.clone();
            let panel_ref = Rc::downgrade(&panel);
            // `row-activated` fires on every click, including one on the row
            // that is already selected — which `row-selected` does not. That is
            // how a second click can mean "never mind".
            let cb_activate = panel.on_select.clone();
            let panel_activate = Rc::downgrade(&panel);
            panel.list.connect_row_activated(move |list, row| {
                let Some(p) = panel_activate.upgrade() else {
                    return;
                };
                if p.rebuilding.get() {
                    return;
                }
                let id: Option<&String> = unsafe { row.data("annotation-id").map(|p| p.as_ref()) };
                let Some(id) = id else { return };
                if p.selected_id.borrow().as_deref() == Some(id.as_str()) {
                    // Already the chosen one: unchoose it.
                    list.select_row(None::<&gtk::ListBoxRow>);
                    *p.selected_id.borrow_mut() = None;
                    if let Some(f) = cb_activate.borrow().as_ref() {
                        f("");
                    }
                }
            });
            panel.list.connect_row_selected(move |_, row| {
                let Some(p) = panel_ref.upgrade() else { return };
                // A selection the rebuild made is not a selection the user
                // made, and telling anyone about it starts the loop again.
                if p.rebuilding.get() {
                    return;
                }
                let Some(row) = row else { return };
                let id: Option<&String> = unsafe { row.data("annotation-id").map(|p| p.as_ref()) };
                if let Some(id) = id {
                    *p.selected_id.borrow_mut() = Some(id.clone());
                    if let Some(f) = cb.borrow().as_ref() {
                        f(id);
                    }
                }
            });
        }
        panel
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.widget
    }

    pub fn set_on_select(&self, f: impl Fn(&str) + 'static) {
        *self.on_select.borrow_mut() = Some(Box::new(f));
    }

    pub fn set_on_delete(&self, f: impl Fn(&str) + 'static) {
        *self.on_delete.borrow_mut() = Some(Box::new(f));
    }

    /// Put the viewer's drawing controls at the top of this section.
    pub fn set_draw_controls(&self, controls: &impl IsA<gtk::Widget>) {
        self.draw_slot.append(controls.as_ref());
    }

    pub fn set_on_edit(&self, f: impl Fn(&str) + 'static) {
        *self.on_edit.borrow_mut() = Some(Box::new(f));
    }

    pub fn set_on_clear(&self, f: impl Fn(&str) + 'static) {
        *self.on_clear.borrow_mut() = Some(Box::new(f));
    }

    /// Redraw the list from `annotations`.
    pub fn set_annotations(&self, annotations: &[Annotation], selected: Option<&str>) {
        self.rebuilding.set(true);
        *self.selected_id.borrow_mut() = selected.map(str::to_string);
        while let Some(row) = self.list.first_child() {
            self.list.remove(&row);
        }
        let has_any = !annotations.is_empty();
        self.empty.set_visible(!has_any);
        self.list.set_visible(has_any);
        self.clear_button.set_sensitive(has_any);
        self.count_label.set_text(&if has_any {
            crate::tr_fmt!("{} marks", annotations.len())
        } else {
            String::new()
        });

        for a in annotations {
            let row = gtk::ListBoxRow::new();
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            // Room to read. The rows were tight enough that the label and its
            // position ran together, and these are two different things: what
            // the mark says, and where it is.
            line.set_margin_top(12);
            line.set_margin_bottom(12);
            line.set_margin_start(10);
            line.set_margin_end(8);

            let text = gtk::Box::new(gtk::Orientation::Vertical, 4);
            let title = gtk::Label::new(Some(&display_title(a)));
            title.set_xalign(0.0);
            title.set_ellipsize(gtk4::pango::EllipsizeMode::End);
            text.append(&title);

            let sub = gtk::Label::new(Some(&describe(a)));
            sub.add_css_class("dim-label");
            sub.add_css_class("caption");
            sub.set_xalign(0.0);
            text.append(&sub);
            text.set_hexpand(true);
            line.append(&text);

            let edit = gtk::Button::from_icon_name("document-edit-symbolic");
            edit.add_css_class("flat");
            edit.set_valign(gtk::Align::Center);
            edit.set_tooltip_text(Some(crate::tr_en!("Rename this mark")));
            {
                let id = a.id.clone();
                let cb = self.on_edit.clone();
                edit.connect_clicked(move |_| {
                    if let Some(f) = cb.borrow().as_ref() {
                        f(&id);
                    }
                });
            }
            line.append(&edit);

            let delete = gtk::Button::from_icon_name("user-trash-symbolic");
            delete.add_css_class("flat");
            delete.set_valign(gtk::Align::Center);
            delete.set_tooltip_text(Some(crate::tr_en!("Delete this mark")));
            {
                let id = a.id.clone();
                let cb = self.on_delete.clone();
                delete.connect_clicked(move |_| {
                    if let Some(f) = cb.borrow().as_ref() {
                        f(&id);
                    }
                });
            }
            line.append(&delete);

            row.set_child(Some(&line));
            // The id travels with the row, so selection does not depend on the
            // list's order matching the model's.
            unsafe { row.set_data("annotation-id", a.id.clone()) };
            self.list.append(&row);
            if selected == Some(a.id.as_str()) {
                self.list.select_row(Some(&row));
            }
        }

        self.rebuilding.set(false);
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
