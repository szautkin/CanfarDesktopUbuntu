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
use std::cell::{Cell, RefCell};
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

/// The whole Marks section: the collapsible, the list inside it, and the two
/// controls that produce marks.
///
/// [`AnnotationsPanel`] was already shared between the two viewers, but each
/// one assembled the section around it — the same expander, the same pencil
/// toggle, the same Circle/Box picker, built twice. That is the copy that
/// drifts: the section is where a person looks for marks, and two viewers
/// whose Marks section is subtly different is a worse bug than either being
/// wrong, because it teaches that the thing behaves differently depending on
/// what you have open.
///
/// One argument, because one thing genuinely differs: the FITS viewer can pan
/// with Shift-drag while drawing and the cube cannot, so their tooltips say
/// different things about it.
/// Told the new style whenever a style control moves.
type StyleCallback = Rc<dyn Fn(crate::models::annotation::MarkStyle)>;

pub struct MarksSection {
    expander: gtk::Expander,
    panel: Rc<AnnotationsPanel>,
    draw_mode: gtk::ToggleButton,
    draw_kind: gtk::DropDown,
    colour: gtk::ColorDialogButton,
    font_size: gtk::SpinButton,
    bold: gtk::ToggleButton,
    stroke: gtk::SpinButton,
    /// Told the style whenever a control moves.
    on_style: RefCell<Option<StyleCallback>>,
    /// Suppresses `on_style` while the controls are being set FROM a style,
    /// so showing a mark's style does not immediately write it back.
    settling: Cell<bool>,
}

impl MarksSection {
    pub fn new(draw_tooltip: &str) -> Rc<Self> {
        // The drawing controls belong INSIDE the section, beside the list of
        // what they produce, rather than in a toolbar elsewhere.
        let draw_mode = gtk::ToggleButton::new();
        draw_mode.set_icon_name("document-edit-symbolic");
        draw_mode.set_tooltip_text(Some(draw_tooltip));

        // Two shapes. A "callout" was a small circle with a leader, and every
        // shape has a leader now; a "text" was a label with nothing to point
        // at. Both kinds still exist in the model and over MCP — stored marks
        // and an agent's calls keep working — they are simply not choices a
        // person has to make here.
        let kind_items = gtk::StringList::new(&[crate::tr_en!("Circle"), crate::tr_en!("Box")]);
        let draw_kind = gtk::DropDown::new(Some(kind_items), gtk::Expression::NONE);
        draw_kind.set_selected(0);

        let draw_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        draw_box.append(&draw_mode);
        draw_box.append(&draw_kind);

        // Style, directly under the pencil that uses it. Four controls,
        // acting on the selected mark when there is one and on what the next
        // mark will look like otherwise — which is how every drawing
        // application behaves, and avoids a separate "preferences for new
        // marks" screen nobody would find.
        let colour = gtk::ColorDialogButton::new(Some(gtk::ColorDialog::new()));
        colour.set_tooltip_text(Some(crate::tr_en!("Mark colour")));

        let font_size = gtk::SpinButton::with_range(6.0, 72.0, 1.0);
        font_size.set_value(crate::models::annotation::DEFAULT_FONT_SIZE);
        font_size.set_tooltip_text(Some(crate::tr_en!("Label size in pixels")));

        let bold = gtk::ToggleButton::new();
        bold.set_icon_name("format-text-bold-symbolic");
        bold.set_tooltip_text(Some(crate::tr_en!("Bold label")));

        let stroke = gtk::SpinButton::with_range(0.5, 20.0, 0.5);
        stroke.set_digits(1);
        stroke.set_value(crate::models::annotation::DEFAULT_STROKE);
        stroke.set_tooltip_text(Some(crate::tr_en!("Outline thickness in pixels")));

        let style_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        style_box.append(&colour);
        style_box.append(&font_size);
        style_box.append(&bold);
        style_box.append(&stroke);

        // Both rows go into the panel's one drawing slot rather than the panel
        // growing a second slot: the slot means "the controls that make a
        // mark", and what a mark looks like is part of making it.
        let rows = gtk::Box::new(gtk::Orientation::Vertical, 6);
        rows.append(&draw_box);
        rows.append(&style_box);

        let panel = AnnotationsPanel::new();
        panel.set_draw_controls(&rows);
        let expander = gtk::Expander::new(Some(crate::tr_en!("Marks")));
        expander.set_child(Some(panel.widget()));

        let this = Rc::new(Self {
            expander,
            panel,
            draw_mode,
            draw_kind,
            colour,
            font_size,
            bold,
            stroke,
            on_style: RefCell::new(None),
            settling: Cell::new(false),
        });

        // One handler for four controls: they all mean the same thing, and
        // four copies of "read all four, clamp, notify" would be four places
        // to forget the guard.
        let announce = {
            let weak = Rc::downgrade(&this);
            move || {
                let Some(this) = weak.upgrade() else { return };
                if this.settling.get() {
                    return;
                }
                let cb = this.on_style.borrow().clone();
                if let Some(cb) = cb {
                    cb(this.style());
                }
            }
        };
        {
            let a = announce.clone();
            this.colour.connect_rgba_notify(move |_| a());
        }
        {
            let a = announce.clone();
            this.font_size.connect_value_changed(move |_| a());
        }
        {
            let a = announce.clone();
            this.bold.connect_toggled(move |_| a());
        }
        {
            let a = announce;
            this.stroke.connect_value_changed(move |_| a());
        }
        this.show_style_for(None);
        this
    }

    /// What the next mark will be, shape and look together.
    ///
    /// Asked at draw time and again when the mark lands, so the preview and
    /// the mark cannot disagree about either half.
    pub fn pending(&self) -> crate::models::annotation::PendingMark {
        crate::models::annotation::PendingMark {
            kind: self.kind(),
            style: self.style(),
        }
    }

    /// What the controls currently say.
    pub fn style(&self) -> crate::models::annotation::MarkStyle {
        let c = self.colour.rgba();
        crate::models::annotation::MarkStyle {
            colour: (
                f64::from(c.red()),
                f64::from(c.green()),
                f64::from(c.blue()),
            ),
            font_size: self.font_size.value(),
            bold: self.bold.is_active(),
            stroke: self.stroke.value(),
        }
        .sane()
    }

    /// Point the style row at the selected mark, or — when nothing is
    /// selected — at the stored default, which is what the next mark will get.
    ///
    /// One place decides that, so no path through either viewer can leave the
    /// row describing a mark that is no longer selected.
    pub fn show_style_for(&self, selected: Option<&Annotation>) {
        self.show_style(match selected {
            Some(mark) => mark.effective_style(),
            None => crate::services::settings_service::default_mark_style(
                crate::models::annotation::Author::User,
            ),
        });
    }

    /// Show `style` without announcing it back.
    ///
    /// Setting a widget emits its change signal, so without the guard,
    /// displaying the selected mark's style would immediately write that same
    /// style back to it — harmless here, but it would also overwrite the
    /// stored default every time a mark was clicked.
    pub fn show_style(&self, style: crate::models::annotation::MarkStyle) {
        let style = style.sane();
        self.settling.set(true);
        self.colour.set_rgba(&gtk::gdk::RGBA::new(
            style.colour.0 as f32,
            style.colour.1 as f32,
            style.colour.2 as f32,
            1.0,
        ));
        self.font_size.set_value(style.font_size);
        self.bold.set_active(style.bold);
        self.stroke.set_value(style.stroke);
        self.settling.set(false);
    }

    /// Called with the style whenever a control moves.
    pub fn set_on_style_changed(&self, f: impl Fn(crate::models::annotation::MarkStyle) + 'static) {
        *self.on_style.borrow_mut() = Some(Rc::new(f));
    }

    /// The collapsible, to append to a control column.
    pub fn widget(&self) -> &gtk::Expander {
        &self.expander
    }

    /// The list, for its selection and row-action callbacks.
    pub fn panel(&self) -> &Rc<AnnotationsPanel> {
        &self.panel
    }

    /// The pencil. Owned by the section so both viewers drive drawing through
    /// the same widget rather than each holding their own.
    pub fn draw_mode(&self) -> &gtk::ToggleButton {
        &self.draw_mode
    }

    /// The shape the picker is on, read at click time rather than remembered —
    /// what is drawn is whatever the picker says when you click, not what it
    /// said when drawing was armed.
    pub fn kind(&self) -> crate::models::annotation::AnnotationKind {
        use crate::models::annotation::AnnotationKind;
        if self.draw_kind.selected() == 1 {
            AnnotationKind::Rect
        } else {
            AnnotationKind::Circle
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::annotation::{Anchor, AnnotationKind, Author};

    fn mark(kind: AnnotationKind, text: &str, author: Author) -> Annotation {
        Annotation::new(kind, Anchor::ImagePixel { x: 12.0, y: 34.0 }, text, author)
    }

    /// Both viewers mount this section, so both must connect its style row.
    ///
    /// The controls exist either way; unconnected, they move and nothing
    /// happens — no error, no log line, and a person concluding the feature is
    /// broken. That is exactly the failure a compiler cannot see, because a
    /// callback nobody registers is not a type error.
    #[test]
    fn both_viewers_connect_the_style_row() {
        for (path, source) in crate::testing::rust_sources() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name != "fits_viewer.rs" && name != "cube_viewer.rs" {
                continue;
            }
            let code = crate::testing::code(&source);
            assert!(
                code.contains("set_on_style_changed"),
                "{name} mounts the Marks section but never connects its style \
                 row — the controls would move and do nothing"
            );
            assert!(
                code.contains("show_style_for"),
                "{name} never points the style row at the selected mark, so it \
                 would describe whatever was last touched"
            );
        }
    }

    /// The stored default is read when a mark is CREATED, nowhere else.
    ///
    /// The whole reason a mark carries its own style is that changing the
    /// setting must leave marks already drawn alone. A draw path that consulted
    /// the setting would restyle everyone's work the moment the colour button
    /// moved — silently, and to marks they had already exported.
    ///
    /// `show_style_for` is the one caller: it fills the CONTROLS, which is what
    /// the next mark is made from. Anything else asking is the bug.
    #[test]
    fn nothing_but_the_style_row_reads_the_stored_default() {
        let mut callers: Vec<String> = Vec::new();
        for (path, source) in crate::testing::rust_sources() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name == "settings_service.rs" || name == "annotations_panel.rs" {
                continue;
            }
            for (i, line) in crate::testing::code(&source).lines().enumerate() {
                if line.contains("default_mark_style(") {
                    callers.push(format!("{name}:{}: {}", i + 1, line.trim()));
                }
            }
        }
        assert!(
            callers.is_empty(),
            "the stored default is meant to be read only where a new mark is \
             made from it; these read it elsewhere, and would restyle marks \
             already drawn: {callers:#?}"
        );
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
