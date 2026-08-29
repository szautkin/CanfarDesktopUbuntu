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
    bookmarks_section: Rc<crate::ui::item_list_section::ItemListSection>,
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

        // The same list component the marks section uses: a filter, a fixed
        // height, and the buttons this section needs — go, and delete.
        use crate::ui::item_list_section::{ItemListSection, RowActions, SectionSpec};
        let bookmarks_section = ItemListSection::new(SectionSpec {
            actions: RowActions::DELETE
                .with_primary("go-next-symbolic", crate::tr_en!("Go to bookmark")),
            filter_placeholder: Some(crate::tr_en!("Filter bookmarks…")),
            empty_message: crate::tr_en!("No bookmarks yet"),
            // Selectable, like the marks list. Picking a bookmark out and
            // clicking it again to change your mind should work the same way in
            // every list in this sidebar — that consistency is most of what the
            // shared component is for.
            selectable: true,
            monospace: false,
        });
        section3.append(bookmarks_section.widget());

        container.append(&section3);

        let widget = container;

        let panel = Rc::new(FitsCoordsPanel {
            widget,
            crosshair_label,
            bookmark_label_entry,
            ra_entry,
            dec_entry,
            bookmarks_section,
            bookmarks: Rc::new(RefCell::new(fits_bookmarks::load_bookmarks())),
            current_radec: Rc::new(RefCell::new(None)),
            on_go_to: Rc::new(RefCell::new(None)),
            on_save_bookmark: Rc::new(RefCell::new(None)),
            on_search_here: Rc::new(RefCell::new(None)),
            search_here_btn,
        });

        // The go and delete buttons the section draws, wired to what this panel
        // does with them. Deleting rebuilds from the store, which is the one
        // place bookmarks live.
        {
            let p = panel.clone();
            panel.bookmarks_section.set_on_primary(move |id| {
                let Ok(id) = id.parse::<u64>() else { return };
                let target = p.bookmarks.borrow().iter().find(|b| b.id == id).cloned();
                if let Some(bm) = target {
                    if let Some(cb) = p.on_go_to.borrow().as_ref() {
                        cb(bm.ra_deg, bm.dec_deg);
                    }
                }
            });
        }
        {
            let p = panel.clone();
            panel.bookmarks_section.set_on_delete(move |id| {
                let Ok(id) = id.parse::<u64>() else { return };
                if let Ok(remaining) = fits_bookmarks::remove_bookmark(id) {
                    *p.bookmarks.borrow_mut() = remaining;
                    p.rebuild_bookmarks_list();
                }
            });
        }

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
                    self.crosshair_label.set_text(&crate::tr_fmt!(
                        "Pixel ({}, {})\nRA  {}\nDec {}",
                        format!("{:.1}", px),
                        format!("{:.1}", py),
                        ra_str,
                        dec_str
                    ));
                    *self.current_radec.borrow_mut() = Some((ra, dec));
                    self.search_here_btn.set_sensitive(true);
                } else {
                    self.crosshair_label.set_text(&crate::tr_fmt!(
                        "Pixel ({}, {})\nNo WCS",
                        format!("{:.1}", px),
                        format!("{:.1}", py)
                    ));
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
        use crate::ui::item_list_section::ListItem;
        let bookmarks = self.bookmarks.borrow().clone();
        let items: Vec<ListItem> = bookmarks
            .iter()
            .map(|bm| {
                let (ra_str, dec_str) = WcsInfo::format_coords(bm.ra_deg, bm.dec_deg);
                ListItem {
                    id: bm.id.to_string(),
                    title: bm.label.clone(),
                    // Two coordinates and two spaces: nothing to translate,
                    // and a template that says only "{}  {}" tells a translator
                    // nothing either.
                    subtitle: format!("{ra_str}  {dec_str}"),
                }
            })
            .collect();
        let count = (!items.is_empty())
            .then(|| crate::tr_plural!(items.len(), "{} bookmark", "{} bookmarks"));
        // Nothing outside this list tracks which bookmark is chosen, so a
        // refresh keeps whatever was.
        self.bookmarks_section.set_items(
            &items,
            crate::ui::item_list_section::Selection::Keep,
            count,
        );
    }
}
