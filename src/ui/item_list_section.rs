//! One list of things, with a filter and per-row actions.
//!
//! Three sidebar sections were growing the same list independently: marks,
//! saved coordinates, and the FITS header. Each had its own row layout, its own
//! idea of how tall a list should be, and — because they were written at
//! different times — its own answer to whether a list gets a filter. Marks got
//! roomy rows and no filter; the header got a filter and rows half the height;
//! bookmarks got neither.
//!
//! This is the shape they share: a count, a filter, rows of title + subtitle,
//! and buttons a section chooses. What differs is DATA and which actions apply,
//! so those are arguments rather than three implementations.

use gtk4::prelude::*;
use gtk4::{self as gtk};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

type Callback = Rc<RefCell<Option<Rc<dyn Fn(&str)>>>>;

/// How many rows a section shows before it scrolls.
///
/// Enough to see a few at once and compare them; few enough that three sections
/// still fit in a sidebar.
const VISIBLE_ROWS: i32 = 4;

/// One row's height in pixels: two lines of text plus its margins.
const ROW_HEIGHT: i32 = 68;

/// What a rebuild should do about the chosen row.
///
/// Two callers, two intents: the marks list is told by the canvas which mark is
/// chosen — including that none is — while the bookmarks list has no outside
/// owner and simply keeps its own. A bare `Option` cannot say both, and reading
/// `None` as the wrong one either wipes a selection or brings one back.
#[derive(Debug, Clone, Copy)]
pub enum Selection<'a> {
    Set(Option<&'a str>),
    Keep,
}

/// One row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListItem {
    pub id: String,
    pub title: String,
    pub subtitle: String,
}

/// An extra button on every row, beyond edit and delete.
#[derive(Debug, Clone, Copy)]
pub struct RowAction {
    pub icon: &'static str,
    pub tooltip: &'static str,
}

/// Which buttons a section's rows carry.
///
/// Three sections, three answers: a header keyword has nothing to do to it, a
/// bookmark can be gone to or removed, a mark can be renamed or removed. So the
/// buttons are described rather than assumed.
#[derive(Debug, Clone, Copy, Default)]
pub struct RowActions {
    /// A section-specific action, shown first.
    pub primary: Option<RowAction>,
    pub edit: bool,
    pub delete: bool,
}

impl RowActions {
    pub const NONE: Self = Self {
        primary: None,
        edit: false,
        delete: false,
    };
    pub const DELETE: Self = Self {
        primary: None,
        edit: false,
        delete: true,
    };
    pub const EDIT_AND_DELETE: Self = Self {
        primary: None,
        edit: true,
        delete: true,
    };

    /// With a leading action of its own.
    pub const fn with_primary(mut self, icon: &'static str, tooltip: &'static str) -> Self {
        self.primary = Some(RowAction { icon, tooltip });
        self
    }
}

/// How a section is set up.
pub struct SectionSpec<'a> {
    pub actions: RowActions,
    /// Placeholder for the filter box; `None` for no filter at all.
    pub filter_placeholder: Option<&'a str>,
    /// Shown when there is nothing to list.
    pub empty_message: &'a str,
    /// Whether picking a row means anything to this section.
    pub selectable: bool,
    /// Monospace titles. A FITS header is columns of keyword and value, and
    /// proportional text takes the columns away.
    pub monospace: bool,
}

pub struct ItemListSection {
    widget: gtk::Box,
    list: gtk::ListBox,
    empty: gtk::Label,
    count_label: gtk::Label,
    filter: Option<gtk::SearchEntry>,
    actions: RowActions,
    monospace: bool,
    items: RefCell<Vec<ListItem>>,
    selected_id: RefCell<Option<String>>,
    selectable: bool,
    /// What was selected when the current press began.
    ///
    /// `row-activated` fires on the click that SELECTS a row as well as on a
    /// click on one already selected, and `row-selected` runs first — so
    /// comparing against the live selection made the first click select and
    /// then immediately deselect, and a row appeared to need two clicks. This
    /// records the selection before the press, which is the thing the question
    /// is actually about.
    selected_before_press: RefCell<Option<String>>,
    rebuilding: Cell<bool>,
    on_select: Callback,
    on_primary: Callback,
    on_edit: Callback,
    on_delete: Callback,
}

impl ItemListSection {
    pub fn new(spec: SectionSpec<'_>) -> Rc<Self> {
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 10);
        widget.set_margin_top(10);
        widget.set_margin_bottom(12);

        let count_label = gtk::Label::new(None);
        count_label.add_css_class("dim-label");
        count_label.add_css_class("caption");
        count_label.set_halign(gtk::Align::Start);
        widget.append(&count_label);

        let filter = spec.filter_placeholder.map(|placeholder| {
            let entry = gtk::SearchEntry::new();
            entry.set_placeholder_text(Some(placeholder));
            widget.append(&entry);
            entry
        });

        let empty = gtk::Label::new(Some(spec.empty_message));
        empty.add_css_class("dim-label");
        empty.set_wrap(true);
        empty.set_xalign(0.0);
        empty.set_valign(gtk::Align::Start);
        // Sized like the list it stands in for, so a section that empties does
        // not collapse and then jump back when something is added.
        empty.set_size_request(-1, VISIBLE_ROWS * ROW_HEIGHT);
        widget.append(&empty);

        let list = gtk::ListBox::new();
        list.add_css_class("boxed-list");
        // The list never selects anything itself. Its selection moves on its
        // own when rows are removed, is already set by the time
        // `row-activated` can ask what WAS selected, and needs a capture-phase
        // gesture to inspect — three behaviours that each had to be suppressed,
        // and each suppression broke something else: a rebuild loop, a
        // deselect that jumped to the first row, and a select that took two
        // clicks. A CSS class the section adds and removes has none of them.
        list.set_selection_mode(gtk::SelectionMode::None);

        // A fixed height: four rows, and it scrolls past that.
        //
        // The list used to be as tall as its contents, so filtering shrank the
        // section and everything below it jumped up the sidebar — you type one
        // character and the thing you were about to click has moved. It holds
        // its height now whether it is showing one row or forty.
        //
        // `propagate_natural_height` is what gives a nested ScrolledWindow a
        // height at all inside an Expander; a min/max pair without it is what
        // left this list blank earlier.
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroll.set_propagate_natural_height(true);
        scroll.set_min_content_height(VISIBLE_ROWS * ROW_HEIGHT);
        scroll.set_max_content_height(VISIBLE_ROWS * ROW_HEIGHT);
        scroll.set_child(Some(&list));
        widget.append(&scroll);

        let section = Rc::new(Self {
            widget,
            list,
            empty,
            count_label,
            filter,
            actions: spec.actions,
            monospace: spec.monospace,
            items: RefCell::new(Vec::new()),
            selected_id: RefCell::new(None),
            selectable: spec.selectable,
            selected_before_press: RefCell::new(None),
            rebuilding: Cell::new(false),
            on_select: Rc::new(RefCell::new(None)),
            on_primary: Rc::new(RefCell::new(None)),
            on_edit: Rc::new(RefCell::new(None)),
            on_delete: Rc::new(RefCell::new(None)),
        });

        if let Some(entry) = section.filter.as_ref() {
            let this = Rc::downgrade(&section);
            entry.connect_search_changed(move |_| {
                if let Some(s) = this.upgrade() {
                    s.apply_filter();
                }
            });
        }

        section
    }

    /// Choose `id`, or clear the choice when it is already chosen.
    fn toggle(self: &Rc<Self>, id: &str) {
        let already = self.selected_id.borrow().as_deref() == Some(id);
        let now = if already { None } else { Some(id.to_string()) };
        *self.selected_id.borrow_mut() = now.clone();
        self.paint_selection();
        let handler = self.on_select.borrow().clone();
        if let Some(f) = handler.as_ref() {
            f(now.as_deref().unwrap_or(""));
        }
    }

    /// Which row is chosen, if any.
    pub fn selected(&self) -> Option<String> {
        self.selected_id.borrow().clone()
    }

    /// Choose a row as a click would, for tests and probes.
    pub fn click_row(self: &Rc<Self>, id: &str) {
        self.toggle(id);
    }

    /// Put the highlight on the chosen row and nowhere else.
    fn paint_selection(&self) {
        let chosen = self.selected_id.borrow().clone();
        let mut child = self.list.first_child();
        while let Some(row) = child {
            let next = row.next_sibling();
            if let Ok(row) = row.clone().downcast::<gtk::ListBoxRow>() {
                let is_chosen = row_id(&row).as_deref() == chosen.as_deref();
                if is_chosen {
                    row.add_css_class("item-selected");
                } else {
                    row.remove_css_class("item-selected");
                }
            }
            child = next;
        }
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.widget
    }

    /// Add a widget above the list — a section's own controls.
    pub fn prepend_control(&self, child: &impl IsA<gtk::Widget>) {
        self.widget.prepend(child.as_ref());
    }

    /// Add a widget below the list.
    pub fn append_control(&self, child: &impl IsA<gtk::Widget>) {
        self.widget.append(child.as_ref());
    }

    pub fn set_on_select(&self, f: impl Fn(&str) + 'static) {
        *self.on_select.borrow_mut() = Some(Rc::new(f));
    }

    /// The section's own row action — "go to this one", and the like.
    pub fn set_on_primary(&self, f: impl Fn(&str) + 'static) {
        *self.on_primary.borrow_mut() = Some(Rc::new(f));
    }

    pub fn set_on_edit(&self, f: impl Fn(&str) + 'static) {
        *self.on_edit.borrow_mut() = Some(Rc::new(f));
    }

    pub fn set_on_delete(&self, f: impl Fn(&str) + 'static) {
        *self.on_delete.borrow_mut() = Some(Rc::new(f));
    }

    /// Replace the rows.
    pub fn set_items(
        self: &Rc<Self>,
        items: &[ListItem],
        selected: Selection<'_>,
        count_text: Option<String>,
    ) {
        self.rebuilding.set(true);
        *self.items.borrow_mut() = items.to_vec();
        let chosen = match selected {
            // The caller owns the selection and is stating it — including
            // stating that there is none.
            Selection::Set(id) => id.map(str::to_string),
            // The caller does not track it, so the section keeps what it had,
            // as long as that row still exists. Without the distinction, a
            // refresh either wipes a selection the caller meant to keep or
            // resurrects one the caller meant to clear.
            Selection::Keep => self
                .selected_id
                .borrow()
                .clone()
                .filter(|id| items.iter().any(|i| &i.id == id)),
        };
        *self.selected_id.borrow_mut() = chosen.clone();
        // The press-time memory follows the real selection, or the next click
        // on the row that used to be chosen would read as a second click.
        *self.selected_before_press.borrow_mut() = chosen;
        while let Some(row) = self.list.first_child() {
            self.list.remove(&row);
        }
        let has_any = !items.is_empty();
        self.empty.set_visible(!has_any);
        self.list.set_visible(has_any);
        self.count_label
            .set_text(count_text.as_deref().unwrap_or(""));
        self.count_label
            .set_visible(count_text.is_some() && has_any);

        for item in items {
            let row = self.build_row(item);
            if self.selectable {
                self.make_selectable(&row, &item.id);
            }
            self.list.append(&row);
        }
        self.paint_selection();
        self.rebuilding.set(false);
        self.apply_filter();
    }

    fn build_row(&self, item: &ListItem) -> gtk::ListBoxRow {
        let row = gtk::ListBoxRow::new();
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        line.set_margin_top(12);
        line.set_margin_bottom(12);
        line.set_margin_start(10);
        line.set_margin_end(8);

        let text = gtk::Box::new(gtk::Orientation::Vertical, 4);
        let title = gtk::Label::new(Some(&item.title));
        if self.monospace {
            title.add_css_class("monospace");
        }
        title.set_xalign(0.0);
        title.set_ellipsize(gtk4::pango::EllipsizeMode::End);
        text.append(&title);
        if !item.subtitle.is_empty() {
            let sub = gtk::Label::new(Some(&item.subtitle));
            sub.add_css_class("dim-label");
            sub.add_css_class("caption");
            sub.set_xalign(0.0);
            sub.set_wrap(true);
            text.append(&sub);
        }
        text.set_hexpand(true);
        line.append(&text);

        if let Some(primary) = self.actions.primary {
            let button = icon_button(primary.icon, primary.tooltip);
            let id = item.id.clone();
            let cb = self.on_primary.clone();
            button.connect_clicked(move |_| {
                let handler = cb.borrow().clone();
                if let Some(f) = handler.as_ref() {
                    f(&id);
                }
            });
            line.append(&button);
        }
        if self.actions.edit {
            let edit = icon_button("document-edit-symbolic", crate::tr_en!("Rename this mark"));
            let id = item.id.clone();
            let cb = self.on_edit.clone();
            edit.connect_clicked(move |_| {
                let handler = cb.borrow().clone();
                if let Some(f) = handler.as_ref() {
                    f(&id);
                }
            });
            line.append(&edit);
        }
        if self.actions.delete {
            let delete = icon_button("user-trash-symbolic", crate::tr_en!("Delete this one"));
            let id = item.id.clone();
            let cb = self.on_delete.clone();
            delete.connect_clicked(move |_| {
                let handler = cb.borrow().clone();
                if let Some(f) = handler.as_ref() {
                    f(&id);
                }
            });
            line.append(&delete);
        }

        row.set_child(Some(&line));
        unsafe { row.set_data("list-item-id", item.id.clone()) };
        row
    }

    /// Make `row` respond to a click, when this section is selectable.
    fn make_selectable(self: &Rc<Self>, row: &gtk::ListBoxRow, id: &str) {
        let click = gtk::GestureClick::new();
        let this = Rc::downgrade(self);
        let id = id.to_string();
        click.connect_released(move |_, _, _, _| {
            if let Some(s) = this.upgrade() {
                s.toggle(&id);
            }
        });
        row.add_controller(click);
    }

    /// Hide rows that do not match the filter.
    fn apply_filter(&self) {
        let needle = self
            .filter
            .as_ref()
            .map(|e| e.text().to_string().to_lowercase())
            .unwrap_or_default();
        let items = self.items.borrow();
        let mut shown = 0usize;
        let mut child = self.list.first_child();
        let mut index = 0usize;
        while let Some(row) = child {
            let next = row.next_sibling();
            if let Some(item) = items.get(index) {
                let matches = needle.is_empty()
                    || item.title.to_lowercase().contains(&needle)
                    || item.subtitle.to_lowercase().contains(&needle);
                row.set_visible(matches);
                if matches {
                    shown += 1;
                }
            }
            index += 1;
            child = next;
        }
        // A filter that matches nothing should say so rather than looking like
        // an empty section.
        if !needle.is_empty() && !items.is_empty() {
            self.empty.set_visible(shown == 0);
            self.empty
                .set_text(crate::tr_en!("Nothing matches that filter."));
        }
    }
}

fn icon_button(icon: &str, tooltip: &str) -> gtk::Button {
    let button = gtk::Button::from_icon_name(icon);
    button.add_css_class("flat");
    button.set_valign(gtk::Align::Center);
    button.set_tooltip_text(Some(tooltip));
    button
}

fn row_id(row: &gtk::ListBoxRow) -> Option<String> {
    let id: Option<&String> = unsafe { row.data("list-item-id").map(|p| p.as_ref()) };
    id.cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each section gets the buttons its rows should carry, and no others.
    ///
    /// A header keyword has nothing to edit or delete; a bookmark can be
    /// removed but not renamed; a mark is both. Asserted through a function so
    /// the check is on behaviour rather than on three constants.
    fn buttons(actions: RowActions) -> (bool, bool) {
        (actions.edit, actions.delete)
    }

    #[test]
    fn each_section_gets_only_the_buttons_it_needs() {
        assert_eq!(buttons(RowActions::NONE), (false, false));
        assert_eq!(buttons(RowActions::DELETE), (false, true));
        assert_eq!(buttons(RowActions::EDIT_AND_DELETE), (true, true));
        assert!(RowActions::NONE.primary.is_none());
    }

    /// A section can add one action of its own without a new flag per section.
    #[test]
    fn a_primary_action_is_described_not_hard_coded() {
        let go = RowActions::DELETE.with_primary("go-next-symbolic", "Go to bookmark");
        let action = go.primary.expect("a primary action");
        assert_eq!(action.icon, "go-next-symbolic");
        assert!(go.delete, "the other buttons are unaffected");
        assert!(!go.edit);
    }
}
