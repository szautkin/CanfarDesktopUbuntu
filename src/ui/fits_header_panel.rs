//! Collapsible FITS header panel.
//!
//! Displays the ordered list of FITS header keywords (keyword | value | comment)
//! with a search entry for filtering. Wrapped in a `gtk::Revealer` so the
//! parent can toggle visibility with an animated slide.

use gtk4::prelude::*;
use gtk4::{self as gtk};
use std::cell::RefCell;
use std::rc::Rc;

pub struct FitsHeaderPanel {
    widget: gtk::Box,
    /// The at-a-glance summary, rebuilt when the panel is pointed at another
    /// image.
    info_grid: gtk::Grid,
    section: Rc<crate::ui::item_list_section::ItemListSection>,
    entries: Rc<RefCell<Vec<(String, String, String)>>>,
}

impl FitsHeaderPanel {
    /// An empty panel. Content arrives through [`Self::set_content`] when a tab
    /// becomes the visible one — there is one panel for the viewer, not one per
    /// image.
    pub fn new() -> Rc<Self> {
        // Lives in the control column now, so it sizes to the column rather
        // than demanding 320 px of its own beside the image.
        let container = gtk::Box::new(gtk::Orientation::Vertical, 0);

        // Image Info summary (dimensions, WCS, pixel scale, FoV, sky centre, …),
        // rebuilt whenever the panel is pointed at another image.
        let info_grid = gtk::Grid::new();
        info_grid.set_column_spacing(10);
        info_grid.set_row_spacing(2);
        info_grid.set_margin_bottom(8);
        container.append(&info_grid);

        let title = gtk::Label::new(Some(crate::tr_en!("FITS Header")));
        title.add_css_class("caption-heading");
        title.add_css_class("dim-label");
        title.set_halign(gtk::Align::Start);
        title.set_margin_bottom(4);
        container.append(&title);

        // Search
        // The same list component the marks and bookmarks sections use: its
        // filter, its fixed height, and no row buttons — a header keyword has
        // nothing to do to it.
        use crate::ui::item_list_section::{ItemListSection, RowActions, SectionSpec};
        let section = ItemListSection::new(SectionSpec {
            actions: RowActions::NONE,
            filter_placeholder: Some(crate::tr_en!("Filter keywords…")),
            empty_message: crate::tr_en!("This extension carries no header keywords."),
            selectable: false,
            monospace: true,
        });
        container.append(section.widget());

        let widget = container;

        let panel = Rc::new(FitsHeaderPanel {
            widget,
            info_grid,
            section,
            entries: Rc::new(RefCell::new(Vec::new())),
        });
        // Filtering is the section's own; this panel only supplies rows.
        panel.rebuild();
        panel
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.widget
    }

    /// Point the panel at another image: its summary rows and its keywords.
    ///
    /// One panel for the viewer rather than one per tab. The per-tab panels each
    /// carried their own 320 px of layout beside the image, whether or not
    /// anyone had opened them.
    pub fn set_content(
        self: &Rc<Self>,
        entries: Vec<(String, String, String)>,
        info: Vec<(String, String)>,
    ) {
        while let Some(child) = self.info_grid.first_child() {
            self.info_grid.remove(&child);
        }
        for (i, (label, value)) in info.iter().enumerate() {
            let l = gtk::Label::new(Some(label));
            l.add_css_class("caption");
            l.add_css_class("dim-label");
            l.set_halign(gtk::Align::Start);
            l.set_yalign(0.0);
            self.info_grid.attach(&l, 0, i as i32, 1, 1);
            let v = gtk::Label::new(Some(value));
            v.add_css_class("caption");
            v.set_halign(gtk::Align::Start);
            v.set_xalign(0.0);
            v.set_hexpand(true);
            v.set_selectable(true);
            v.set_wrap(true);
            self.info_grid.attach(&v, 1, i as i32, 1, 1);
        }
        *self.entries.borrow_mut() = entries;
        self.rebuild();
    }

    /// Hand the section this extension's keywords.
    ///
    /// Was a hundred lines of row-building and its own filter; the rows, the
    /// filter, the empty message and the height all belong to the component
    /// now, and what is left is the part only a FITS header knows: a keyword
    /// and its value read as one line, and the comment beneath it.
    fn rebuild(self: &Rc<Self>) {
        use crate::ui::item_list_section::ListItem;
        let entries = self.entries.borrow();
        let items: Vec<ListItem> = entries
            .iter()
            .map(|(key, value, comment)| ListItem {
                id: key.clone(),
                title: format!("{key:<8} {value}"),
                subtitle: comment.clone(),
            })
            .collect();
        let count = (!items.is_empty())
            .then(|| crate::tr_plural!(items.len(), "{} keyword", "{} keywords"));
        drop(entries);
        self.section
            .set_items(&items, crate::ui::item_list_section::Selection::Keep, count);
    }
}
