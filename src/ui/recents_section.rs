//! The "recently viewed" list both viewers show when nothing is open.
//!
//! The cube viewer had one, hand-rolled: a `ListBox` it emptied and refilled,
//! with its own row boxes, its own icon, its own ellipsizing label. The FITS
//! viewer wanted the same thing, and a second copy of that is how the two
//! quietly end up behaving differently — one ellipsizing paths and the other
//! wrapping them, one dropping dead entries and the other not.
//!
//! So there is one of them, over [`ItemListSection`] like the sidebar sections,
//! and one store behind it keyed by [`RecentKind`].
//!
//! **Rows carry buttons rather than being selectable.** Opening a recent is an
//! action, not a choice you can un-make, and a list where the selected row
//! stays lit after the file has opened describes a state that no longer exists.
//! Forgetting one is the other thing you want to do to a list like this, and
//! nothing else is.

use crate::services::recent_files_service::{RecentFilesService, RecentKind};
use crate::ui::item_list_section::{ItemListSection, ListItem, RowActions, SectionSpec, Selection};
use gtk4::{self as gtk};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

type PathCallback = RefCell<Option<Rc<dyn Fn(PathBuf)>>>;

pub struct RecentsSection {
    section: Rc<ItemListSection>,
    service: RecentFilesService,
    /// Row id (the path) is what a row is addressed by, because a file name
    /// alone is ambiguous across directories and two `i2d.fits` from different
    /// observations is exactly what an astronomer has.
    on_open: PathCallback,
}

impl RecentsSection {
    pub fn new(kind: RecentKind, empty_message: &'static str) -> Rc<Self> {
        let section = ItemListSection::new(SectionSpec {
            actions: RowActions::DELETE
                .with_primary("document-open-symbolic", crate::tr_en!("Open this file")),
            // A filter earns its place here: eight entries is the cap, but they
            // are long paths from observation directories that differ late in
            // the string.
            filter_placeholder: Some(crate::tr_en!("Filter recent files")),
            empty_message,
            selectable: false,
            monospace: false,
        });

        let this = Rc::new(Self {
            section,
            service: RecentFilesService::new(kind),
            on_open: RefCell::new(None),
        });

        {
            let weak = Rc::downgrade(&this);
            this.section.set_on_primary(move |id| {
                let Some(this) = weak.upgrade() else {
                    return;
                };
                let cb = this.on_open.borrow().clone();
                if let Some(cb) = cb {
                    cb(PathBuf::from(id));
                }
            });
        }
        {
            let weak = Rc::downgrade(&this);
            this.section.set_on_delete(move |id| {
                let Some(this) = weak.upgrade() else {
                    return;
                };
                // Fully qualified: `gtk4::prelude` brings in several traits
                // with their own `remove`, and the inherent one loses.
                RecentFilesService::remove(&this.service, Path::new(id));
                this.refresh();
            });
        }

        this.refresh();
        this
    }

    pub fn widget(&self) -> &gtk::Box {
        self.section.widget()
    }

    /// Called with the path when a row's open button is pressed.
    pub fn set_on_open(&self, f: impl Fn(PathBuf) + 'static) {
        *self.on_open.borrow_mut() = Some(Rc::new(f));
    }

    /// Note that a file was opened, and put it at the top.
    pub fn record(&self, path: &Path) {
        self.service.add(path);
        self.refresh();
    }

    /// Whether there is anything to show. The empty state hides the whole
    /// section rather than showing a heading over nothing.
    pub fn is_empty(&self) -> bool {
        self.service.list().is_empty()
    }

    /// Rebuild from the store.
    ///
    /// Entries whose file has moved or been deleted are already dropped by the
    /// service on load, so a row that is here can be opened.
    pub fn refresh(&self) {
        let items: Vec<ListItem> = self
            .service
            .list()
            .into_iter()
            .map(|p| ListItem {
                id: p.display().to_string(),
                title: p
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| p.display().to_string()),
                // The directory, not the whole path again: the file name is
                // already the title, and repeating it doubles the length of
                // every row for nothing.
                subtitle: p
                    .parent()
                    .map(|d| d.display().to_string())
                    .unwrap_or_default(),
            })
            .collect();
        self.section.set_items(&items, Selection::Set(None), None);
    }
}
