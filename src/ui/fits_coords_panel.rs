//! Collapsible saved-coordinates panel for the FITS viewer.
//!
//! Three sections:
//!   1. Current Crosshair — read-only readout + Save Bookmark button
//!   2. Go To Coordinate — RA/Dec entry fields + Go button
//!   3. Bookmarks — list of saved sky positions with Go/Delete actions

use crate::helpers::fits_bookmarks::{self, FitsBookmark};
use crate::models::fits_image::WcsInfo;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use std::cell::RefCell;
use std::rc::Rc;

type GoToCallback = Rc<RefCell<Option<Box<dyn Fn(f64, f64)>>>>;
type SaveBookmarkCallback = Rc<RefCell<Option<Box<dyn Fn() -> Option<(f64, f64, String)>>>>>;
/// `Rc` rather than `Box` so the handler can be cloned out before it runs — a
/// handler that reaches back into this panel would otherwise panic on the
/// borrow.
type SearchHereCallback = Rc<RefCell<Option<Rc<dyn Fn(f64, f64)>>>>;

pub struct FitsCoordsPanel {
    /// The panel's content. Collapsing is the column expander's job now — a
    /// revealer inside an expander would be two things that both believe they
    /// decide whether the section is open.
    widget: gtk::Box,
    crosshair_label: gtk::Label,
    bookmark_label_entry: gtk::Entry,
    ra_entry: gtk::Entry,
    dec_entry: gtk::Entry,
    bookmarks_list: gtk::ListBox,
    bookmarks: Rc<RefCell<Vec<FitsBookmark>>>,
    /// The current crosshair's sky position (when a WCS is available).
    current_radec: Rc<RefCell<Option<(f64, f64)>>>,
    on_go_to: GoToCallback,
    on_save_bookmark: SaveBookmarkCallback,
    on_search_here: SearchHereCallback,
    search_here_btn: gtk::Button,
}

impl FitsCoordsPanel {
    pub fn new() -> Rc<Self> {
        let container = gtk::Box::new(gtk::Orientation::Vertical, 12);
        container.set_width_request(280);
        container.set_margin_start(12);
        container.set_margin_end(12);
        container.set_margin_top(12);
        container.set_margin_bottom(12);
        container.set_vexpand(true);

        // ── Current Crosshair section ────────────────────────────────────────
        let section1 = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let sec1_title = gtk::Label::new(Some(crate::tr_en!("Current Crosshair")));
        sec1_title.add_css_class("heading");
        sec1_title.set_halign(gtk::Align::Start);
        section1.append(&sec1_title);

        let crosshair_label = gtk::Label::new(Some(crate::tr_en!("(right-click on image)")));
        crosshair_label.add_css_class("monospace");
        crosshair_label.add_css_class("caption");
        crosshair_label.add_css_class("dim-label");
        crosshair_label.set_halign(gtk::Align::Start);
        crosshair_label.set_wrap(true);
        section1.append(&crosshair_label);

        let bookmark_label_entry = gtk::Entry::new();
        bookmark_label_entry.set_placeholder_text(Some(crate::tr_en!("Bookmark label…")));
        section1.append(&bookmark_label_entry);

        let save_bookmark_btn = gtk::Button::with_label(crate::tr_en!("Save Bookmark"));
        save_bookmark_btn.set_icon_name("starred-symbolic");
        save_bookmark_btn.add_css_class("suggested-action");
        section1.append(&save_bookmark_btn);

        // "Search Here" — take the crosshair sky position to the Search form.
        let search_here_btn = gtk::Button::with_label(crate::tr_en!("Search Here"));
        search_here_btn.set_icon_name("system-search-symbolic");
        search_here_btn
            .set_tooltip_text(Some(crate::tr_en!("Search the archive at this position")));
        search_here_btn.set_sensitive(false);
        section1.append(&search_here_btn);

        container.append(&section1);
        container.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

        // ── Go To Coordinate section ─────────────────────────────────────────
        let section2 = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let sec2_title = gtk::Label::new(Some(crate::tr_en!("Go To Coordinate")));
        sec2_title.add_css_class("heading");
        sec2_title.set_halign(gtk::Align::Start);
        section2.append(&sec2_title);

        let ra_entry = gtk::Entry::new();
        ra_entry.set_placeholder_text(Some(crate::tr_en!("RA (degrees)")));
        section2.append(&ra_entry);

        let dec_entry = gtk::Entry::new();
        dec_entry.set_placeholder_text(Some(crate::tr_en!("Dec (degrees)")));
        section2.append(&dec_entry);

        let go_btn = gtk::Button::with_label(crate::tr_en!("Go To"));
        go_btn.set_icon_name("go-next-symbolic");
        section2.append(&go_btn);

        container.append(&section2);
        container.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

        // ── Bookmarks list section ───────────────────────────────────────────
        let section3 = gtk::Box::new(gtk::Orientation::Vertical, 6);
        section3.set_vexpand(true);
        let sec3_title = gtk::Label::new(Some(crate::tr_en!("Bookmarks")));
        sec3_title.add_css_class("heading");
        sec3_title.set_halign(gtk::Align::Start);
        section3.append(&sec3_title);

        let bookmarks_list = gtk::ListBox::new();
        bookmarks_list.set_selection_mode(gtk::SelectionMode::None);
        bookmarks_list.add_css_class("boxed-list");

        let scroll = gtk::ScrolledWindow::new();
        scroll.set_vexpand(true);
        scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroll.set_child(Some(&bookmarks_list));
        section3.append(&scroll);

        container.append(&section3);

        let widget = container;

        let panel = Rc::new(FitsCoordsPanel {
            widget,
            crosshair_label,
            bookmark_label_entry,
            ra_entry,
            dec_entry,
            bookmarks_list,
            bookmarks: Rc::new(RefCell::new(fits_bookmarks::load_bookmarks())),
            current_radec: Rc::new(RefCell::new(None)),
            on_go_to: Rc::new(RefCell::new(None)),
            on_save_bookmark: Rc::new(RefCell::new(None)),
            on_search_here: Rc::new(RefCell::new(None)),
            search_here_btn,
        });

        // Wire Search Here
        {
            let p = panel.clone();
            panel.search_here_btn.connect_clicked(move |_| {
                p.search_here();
            });
        }

        // Wire Go To
        {
            let p = panel.clone();
            go_btn.connect_clicked(move |_| {
                let ra_txt = p.ra_entry.text().to_string();
                let dec_txt = p.dec_entry.text().to_string();
                if let (Ok(ra), Ok(dec)) =
                    (ra_txt.trim().parse::<f64>(), dec_txt.trim().parse::<f64>())
                {
                    if let Some(cb) = p.on_go_to.borrow().as_ref() {
                        cb(ra, dec);
                    }
                }
            });
        }

        // Wire Save Bookmark
        {
            let p = panel.clone();
            save_bookmark_btn.connect_clicked(move |_| {
                let label = p.bookmark_label_entry.text().to_string();
                if label.trim().is_empty() {
                    return;
                }
                let (ra, dec, source) = match p.on_save_bookmark.borrow().as_ref() {
                    Some(cb) => match cb() {
                        Some(v) => v,
                        None => return,
                    },
                    None => return,
                };
                if let Ok(new_list) = fits_bookmarks::add_bookmark(label, ra, dec, source) {
                    *p.bookmarks.borrow_mut() = new_list;
                    p.rebuild_bookmarks_list();
                    p.bookmark_label_entry.set_text("");
                }
            });
        }

        panel.rebuild_bookmarks_list();
        panel
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.widget
    }

    /// Update the current crosshair readout. Pass `None` to clear.
    pub fn set_current_crosshair(&self, pos_pixel: Option<(f64, f64)>, wcs: Option<&WcsInfo>) {
        match pos_pixel {
            Some((px, py)) => {
                if let Some(w) = wcs {
                    let (ra, dec) = w.pixel_to_sky(px, py);
                    let (ra_str, dec_str) = WcsInfo::format_coords(ra, dec);
                    self.crosshair_label.set_text(&format!(
                        "Pixel ({:.1}, {:.1})\nRA  {}\nDec {}",
                        px, py, ra_str, dec_str
                    ));
                    *self.current_radec.borrow_mut() = Some((ra, dec));
                    self.search_here_btn.set_sensitive(true);
                } else {
                    self.crosshair_label
                        .set_text(&format!("Pixel ({:.1}, {:.1})\nNo WCS", px, py));
                    *self.current_radec.borrow_mut() = None;
                    self.search_here_btn.set_sensitive(false);
                }
            }
            None => {
                self.crosshair_label
                    .set_text(crate::tr_en!("(right-click on image)"));
                *self.current_radec.borrow_mut() = None;
                self.search_here_btn.set_sensitive(false);
            }
        }
    }

    /// Register a callback invoked when "Search Here" is pressed, with the
    /// crosshair's `(ra, dec)` in degrees.
    /// Search the archive at the crosshair, if one is placed on an image with a
    /// WCS.
    ///
    /// Public because the control column offers the same action: the crosshair
    /// section is where a reader looks for it, and this panel is where the
    /// position lives. One path, two entry points — a second copy would be a
    /// second answer to "which crosshair?".
    pub fn search_here(&self) {
        let Some((ra, dec)) = *self.current_radec.borrow() else {
            return;
        };
        let handler = self.on_search_here.borrow().clone();
        if let Some(cb) = handler {
            cb(ra, dec);
        }
    }

    /// Whether a crosshair with sky coordinates is currently placed.
    pub fn has_sky_position(&self) -> bool {
        self.current_radec.borrow().is_some()
    }

    pub fn set_on_search_here(&self, cb: impl Fn(f64, f64) + 'static) {
        *self.on_search_here.borrow_mut() = Some(Rc::new(cb));
    }

    pub fn set_on_go_to(&self, cb: impl Fn(f64, f64) + 'static) {
        *self.on_go_to.borrow_mut() = Some(Box::new(cb));
    }

    /// Register a callback that returns `(ra, dec, source_filename)` for the
    /// currently-active crosshair position, or `None` if no crosshair is placed.
    pub fn set_on_save_bookmark(&self, cb: impl Fn() -> Option<(f64, f64, String)> + 'static) {
        *self.on_save_bookmark.borrow_mut() = Some(Box::new(cb));
    }

    fn rebuild_bookmarks_list(self: &Rc<Self>) {
        while let Some(child) = self.bookmarks_list.first_child() {
            self.bookmarks_list.remove(&child);
        }

        let bookmarks = self.bookmarks.borrow().clone();
        if bookmarks.is_empty() {
            let empty = gtk::Label::new(Some(crate::tr_en!("No bookmarks yet")));
            empty.add_css_class("dim-label");
            empty.add_css_class("caption");
            empty.set_margin_top(8);
            empty.set_margin_bottom(8);
            let row = gtk::ListBoxRow::new();
            row.set_child(Some(&empty));
            self.bookmarks_list.append(&row);
            return;
        }

        for bm in bookmarks {
            let row = gtk::ListBoxRow::new();
            let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            hbox.set_margin_start(6);
            hbox.set_margin_end(6);
            hbox.set_margin_top(4);
            hbox.set_margin_bottom(4);

            let vbox = gtk::Box::new(gtk::Orientation::Vertical, 2);
            vbox.set_hexpand(true);
            let name = gtk::Label::new(Some(&bm.label));
            name.add_css_class("heading");
            name.set_halign(gtk::Align::Start);
            vbox.append(&name);

            let (ra_str, dec_str) = WcsInfo::format_coords(bm.ra_deg, bm.dec_deg);
            let coords = gtk::Label::new(Some(&format!("{}  {}", ra_str, dec_str)));
            coords.add_css_class("monospace");
            coords.add_css_class("caption");
            coords.add_css_class("dim-label");
            coords.set_halign(gtk::Align::Start);
            vbox.append(&coords);
            hbox.append(&vbox);

            let go_bm_btn = gtk::Button::from_icon_name("go-next-symbolic");
            go_bm_btn.add_css_class("flat");
            go_bm_btn.set_tooltip_text(Some(crate::tr_en!("Go to bookmark")));
            {
                let p = self.clone();
                let ra = bm.ra_deg;
                let dec = bm.dec_deg;
                go_bm_btn.connect_clicked(move |_| {
                    if let Some(cb) = p.on_go_to.borrow().as_ref() {
                        cb(ra, dec);
                    }
                });
            }
            hbox.append(&go_bm_btn);

            let del_bm_btn = gtk::Button::from_icon_name("edit-delete-symbolic");
            del_bm_btn.add_css_class("flat");
            del_bm_btn.set_tooltip_text(Some(crate::tr_en!("Delete bookmark")));
            {
                let p = self.clone();
                let id = bm.id;
                del_bm_btn.connect_clicked(move |_| {
                    if let Ok(new_list) = fits_bookmarks::remove_bookmark(id) {
                        *p.bookmarks.borrow_mut() = new_list;
                        p.rebuild_bookmarks_list();
                    }
                });
            }
            hbox.append(&del_bm_btn);

            row.set_child(Some(&hbox));
            self.bookmarks_list.append(&row);
        }
    }
}
