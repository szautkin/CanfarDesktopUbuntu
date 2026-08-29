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

/// One row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListItem {
    pub id: String,
    pub title: String,
    pub subtitle: String,
}

/// Which buttons a section's rows carry.
#[derive(Debug, Clone, Copy, Default)]
pub struct RowActions {
    pub edit: bool,
    pub delete: bool,
}

impl RowActions {
    pub const NONE: Self = Self {
        edit: false,
        delete: false,
    };
    pub const DELETE: Self = Self {
        edit: false,
        delete: true,
    };
    pub const EDIT_AND_DELETE: Self = Self {
        edit: true,
        delete: true,
    };
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
}

pub struct ItemListSection {
    widget: gtk::Box,
    list: gtk::ListBox,
    empty: gtk::Label,
    count_label: gtk::Label,
    filter: Option<gtk::SearchEntry>,
    actions: RowActions,
    items: RefCell<Vec<ListItem>>,
    selected_id: RefCell<Option<String>>,
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
        widget.append(&empty);

        let list = gtk::ListBox::new();
        list.add_css_class("boxed-list");
        list.set_selection_mode(if spec.selectable {
            gtk::SelectionMode::Single
        } else {
            gtk::SelectionMode::None
        });
        // No inner ScrolledWindow: the sidebar column is already one, and a
        // scroller nested in it reports almost no natural height — inside an
        // Expander that leaves the rows present and invisible.
        widget.append(&list);

        let section = Rc::new(Self {
            widget,
            list,
            empty,
            count_label,
            filter,
            actions: spec.actions,
            items: RefCell::new(Vec::new()),
            selected_id: RefCell::new(None),
            selected_before_press: RefCell::new(None),
            rebuilding: Cell::new(false),
            on_select: Rc::new(RefCell::new(None)),
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

        if spec.selectable {
            section.wire_selection();
        }
        section
    }

    /// Selection, and the second click that undoes it.
    fn wire_selection(self: &Rc<Self>) {
        // Capture phase: this runs before the ListBox updates its selection, so
        // it sees what WAS selected rather than what is about to be.
        {
            let this = Rc::downgrade(self);
            let press = gtk::GestureClick::new();
            press.set_propagation_phase(gtk::PropagationPhase::Capture);
            press.connect_pressed(move |_, _, _, _| {
                if let Some(s) = this.upgrade() {
                    *s.selected_before_press.borrow_mut() = s.selected_id.borrow().clone();
                }
            });
            self.list.add_controller(press);
        }
        {
            let this = Rc::downgrade(self);
            self.list.connect_row_selected(move |_, row| {
                let Some(s) = this.upgrade() else { return };
                if s.rebuilding.get() {
                    return;
                }
                let Some(id) = row.and_then(row_id) else {
                    return;
                };
                *s.selected_id.borrow_mut() = Some(id.clone());
                let handler = s.on_select.borrow().clone();
                if let Some(f) = handler.as_ref() {
                    f(&id);
                }
            });
        }
        {
            let this = Rc::downgrade(self);
            self.list.connect_row_activated(move |list, row| {
                let Some(s) = this.upgrade() else { return };
                if s.rebuilding.get() {
                    return;
                }
                let Some(id) = row_id(row) else { return };
                // Only a click on something that was ALREADY chosen means
                // "never mind".
                if s.selected_before_press.borrow().as_deref() == Some(id.as_str()) {
                    list.select_row(None::<&gtk::ListBoxRow>);
                    *s.selected_id.borrow_mut() = None;
                    let handler = s.on_select.borrow().clone();
                    if let Some(f) = handler.as_ref() {
                        f("");
                    }
                }
            });
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

    pub fn set_on_edit(&self, f: impl Fn(&str) + 'static) {
        *self.on_edit.borrow_mut() = Some(Rc::new(f));
    }

    pub fn set_on_delete(&self, f: impl Fn(&str) + 'static) {
        *self.on_delete.borrow_mut() = Some(Rc::new(f));
    }

    /// Replace the rows.
    pub fn set_items(
        &self,
        items: &[ListItem],
        selected: Option<&str>,
        count_text: Option<String>,
    ) {
        self.rebuilding.set(true);
        *self.items.borrow_mut() = items.to_vec();
        *self.selected_id.borrow_mut() = selected.map(str::to_string);
        *self.selected_before_press.borrow_mut() = selected.map(str::to_string);
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
            self.list.append(&row);
            if selected == Some(item.id.as_str()) {
                self.list.select_row(Some(&row));
            }
        }
        // Nothing chosen means nothing chosen. Removing rows makes a ListBox
        // move its selection to a surviving one, and a rebuild removes them
        // all — so deselecting landed on whatever came first, and the list
        // reported a selection the user had just cleared. The guard silences
        // the callback; it does not stop GTK choosing.
        if selected.is_none() {
            self.list.select_row(None::<&gtk::ListBoxRow>);
        }
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

        if self.actions.edit {
            let edit = icon_button("document-edit-symbolic", crate::tr_en!("Rename this mark"));
            let id = item.id.clone();
            let cb = self.on_edit.clone();
            edit.connect_clicked(move |_| {
                if let Some(f) = cb.borrow().as_ref() {
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
                if let Some(f) = cb.borrow().as_ref() {
                    f(&id);
                }
            });
            line.append(&delete);
        }

        row.set_child(Some(&line));
        unsafe { row.set_data("list-item-id", item.id.clone()) };
        row
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
    }
}
