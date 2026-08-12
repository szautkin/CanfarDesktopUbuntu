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
    widget: gtk::Revealer,
    list_box: gtk::ListBox,
    entries: Rc<RefCell<Vec<(String, String, String)>>>,
    search_entry: gtk::SearchEntry,
}

impl FitsHeaderPanel {
    pub fn new(entries: Vec<(String, String, String)>) -> Rc<Self> {
        Self::new_with_info(entries, Vec::new())
    }

    /// Construct with an at-a-glance Image Info summary block rendered above the
    /// searchable raw header.
    pub fn new_with_info(
        entries: Vec<(String, String, String)>,
        info: Vec<(String, String)>,
    ) -> Rc<Self> {
        let container = gtk::Box::new(gtk::Orientation::Vertical, 0);
        container.set_width_request(320);
        container.set_vexpand(true);

        // Image Info summary (dimensions, WCS, pixel scale, FoV, sky centre, …)
        if !info.is_empty() {
            let info_title = gtk::Label::new(Some(crate::tr_en!("Image Info")));
            info_title.add_css_class("heading");
            info_title.set_halign(gtk::Align::Start);
            info_title.set_margin_start(12);
            info_title.set_margin_end(12);
            info_title.set_margin_top(12);
            info_title.set_margin_bottom(6);
            container.append(&info_title);

            let info_grid = gtk::Grid::new();
            info_grid.set_column_spacing(10);
            info_grid.set_row_spacing(2);
            info_grid.set_margin_start(12);
            info_grid.set_margin_end(12);
            info_grid.set_margin_bottom(8);
            for (i, (label, value)) in info.iter().enumerate() {
                let l = gtk::Label::new(Some(label));
                l.add_css_class("caption-heading");
                l.set_halign(gtk::Align::Start);
                info_grid.attach(&l, 0, i as i32, 1, 1);
                let v = gtk::Label::new(Some(value));
                v.add_css_class("caption");
                v.set_halign(gtk::Align::Start);
                v.set_hexpand(true);
                v.set_selectable(true);
                v.set_wrap(true);
                info_grid.attach(&v, 1, i as i32, 1, 1);
            }
            container.append(&info_grid);
            container.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        }

        // Header
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        header.set_margin_start(8);
        header.set_margin_end(8);
        header.set_margin_top(8);
        header.set_margin_bottom(4);
        let title = gtk::Label::new(Some(crate::tr_en!("FITS Header")));
        title.add_css_class("heading");
        title.set_halign(gtk::Align::Start);
        title.set_hexpand(true);
        header.append(&title);
        container.append(&header);

        // Search
        let search_entry = gtk::SearchEntry::new();
        search_entry.set_placeholder_text(Some(crate::tr_en!("Filter keywords…")));
        search_entry.set_margin_start(8);
        search_entry.set_margin_end(8);
        search_entry.set_margin_bottom(4);
        container.append(&search_entry);

        // List
        let list_box = gtk::ListBox::new();
        list_box.set_selection_mode(gtk::SelectionMode::None);
        list_box.add_css_class("boxed-list");
        list_box.set_margin_start(8);
        list_box.set_margin_end(8);
        list_box.set_margin_bottom(8);

        let scroll = gtk::ScrolledWindow::new();
        scroll.set_hexpand(false);
        scroll.set_vexpand(true);
        scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroll.set_child(Some(&list_box));
        container.append(&scroll);

        // Wrap in a revealer for collapsible animation
        let widget = gtk::Revealer::new();
        // Slides in from the trailing edge, where the panel now lives — a
        // SlideRight on a right-hand panel animates away from its own edge.
        widget.set_transition_type(gtk::RevealerTransitionType::SlideLeft);
        widget.set_transition_duration(200);
        widget.set_reveal_child(false);
        widget.set_child(Some(&container));

        let panel = Rc::new(FitsHeaderPanel {
            widget,
            list_box,
            entries: Rc::new(RefCell::new(entries)),
            search_entry,
        });

        // Initial population
        panel.rebuild("");

        // Wire search
        let p = panel.clone();
        panel.search_entry.connect_search_changed(move |entry| {
            let q = entry.text().to_string().to_ascii_lowercase();
            p.rebuild(&q);
        });

        panel
    }

    pub fn widget(&self) -> &gtk::Revealer {
        &self.widget
    }

    /// Toggle visibility with animation.
    pub fn toggle(&self) {
        let now = self.widget.reveals_child();
        self.widget.set_reveal_child(!now);
    }

    /// Is the panel currently revealed?
    pub fn is_revealed(&self) -> bool {
        self.widget.reveals_child()
    }

    fn rebuild(&self, filter: &str) {
        while let Some(child) = self.list_box.first_child() {
            self.list_box.remove(&child);
        }

        let entries = self.entries.borrow();
        let filter_lower = filter.trim().to_ascii_lowercase();

        // Say WHY the list is empty. A panel that opens onto nothing reads as a
        // broken panel; an extension with no keywords, or a filter that matched
        // none, is information the reader can act on.
        let mut shown = 0usize;

        for (key, value, comment) in entries.iter() {
            if !filter_lower.is_empty()
                && !key.to_ascii_lowercase().contains(&filter_lower)
                && !value.to_ascii_lowercase().contains(&filter_lower)
            {
                continue;
            }

            let row = gtk::ListBoxRow::new();
            let grid = gtk::Grid::new();
            grid.set_column_spacing(8);
            grid.set_margin_start(6);
            grid.set_margin_end(6);
            grid.set_margin_top(4);
            grid.set_margin_bottom(4);

            let k_label = gtk::Label::new(Some(key));
            k_label.add_css_class("monospace");
            k_label.add_css_class("caption-heading");
            k_label.set_halign(gtk::Align::Start);
            k_label.set_width_chars(10);
            grid.attach(&k_label, 0, 0, 1, 1);

            let v_label = gtk::Label::new(Some(value));
            v_label.add_css_class("monospace");
            v_label.add_css_class("caption");
            v_label.set_halign(gtk::Align::Start);
            v_label.set_hexpand(true);
            v_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
            v_label.set_selectable(true);
            grid.attach(&v_label, 1, 0, 1, 1);

            if !comment.is_empty() {
                let c_label = gtk::Label::new(Some(comment));
                c_label.add_css_class("dim-label");
                c_label.add_css_class("caption");
                c_label.set_halign(gtk::Align::Start);
                c_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
                grid.attach(&c_label, 0, 1, 2, 1);
            }

            row.set_child(Some(&grid));
            self.list_box.append(&row);
            shown += 1;
        }

        if shown == 0 {
            let message = if entries.is_empty() {
                crate::tr_en!("This extension carries no header keywords.")
            } else {
                crate::tr_en!("No keyword matches that search.")
            };
            let empty = gtk::Label::new(Some(message));
            empty.add_css_class("dim-label");
            empty.add_css_class("caption");
            empty.set_wrap(true);
            empty.set_margin_start(12);
            empty.set_margin_end(12);
            empty.set_margin_top(12);
            empty.set_margin_bottom(12);
            let row = gtk::ListBoxRow::new();
            row.set_selectable(false);
            row.set_activatable(false);
            row.set_child(Some(&empty));
            self.list_box.append(&row);
        }
    }
}
