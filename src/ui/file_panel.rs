use directories::UserDirs;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

// ---------------------------------------------------------------------------
// FileType
// ---------------------------------------------------------------------------

/// Classifies a file by its extension so callers can decide how to open it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileType {
    Fits,
    Notebook,
    Other,
}

impl FileType {
    fn from_path(path: &std::path::Path) -> Self {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_lowercase());
        match ext.as_deref() {
            Some("fits") | Some("fit") | Some("fts") => FileType::Fits,
            Some("ipynb") | Some("py") | Some("md") => FileType::Notebook,
            _ => FileType::Other,
        }
    }
}

// ---------------------------------------------------------------------------
// DirEntry — a single row in the file list
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct DirEntry {
    path: PathBuf,
    name: String,
    is_dir: bool,
    size: Option<u64>,
}

impl DirEntry {
    fn icon_name(&self) -> &'static str {
        if self.is_dir {
            return "folder-symbolic";
        }
        let lower = self.name.to_lowercase();
        if lower.ends_with(".fits") || lower.ends_with(".fit") || lower.ends_with(".fts") {
            "image-x-generic-symbolic"
        } else {
            "text-x-generic-symbolic"
        }
    }

    fn size_label(&self) -> String {
        match self.size {
            None => String::new(),
            Some(b) if b < 1_024 => format!("{} B", b),
            Some(b) if b < 1_048_576 => format!("{:.1} KB", b as f64 / 1_024.0),
            Some(b) if b < 1_073_741_824 => format!("{:.1} MB", b as f64 / 1_048_576.0),
            Some(b) => format!("{:.1} GB", b as f64 / 1_073_741_824.0),
        }
    }
}

// ---------------------------------------------------------------------------
// FilePanel
// ---------------------------------------------------------------------------

type OpenFileCb = Box<dyn Fn(PathBuf, FileType)>;

/// A collapsible left-sidebar panel that browses the local filesystem.
pub struct FilePanel {
    widget: gtk::Box,
    list_box: gtk::ListBox,
    /// Narrows the current folder's listing. Held so `populate` can read it —
    /// the filter is part of what "the current listing" means.
    filter_entry: gtk::SearchEntry,
    path_label: gtk::Label,
    current_path: Rc<RefCell<PathBuf>>,
    entries: Rc<RefCell<Vec<DirEntry>>>,
    on_open_file: Rc<RefCell<Option<OpenFileCb>>>,
}

impl FilePanel {
    /// Create a new FilePanel starting at the user's home directory.
    pub fn new() -> Rc<Self> {
        let start_path = UserDirs::new()
            .map(|u| u.home_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from("/"));

        // ----------------------------------------------------------------
        // Root container — fixed width, not expanding
        // ----------------------------------------------------------------
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        widget.set_width_request(280);
        widget.set_hexpand(false);

        // ----------------------------------------------------------------
        // Mini header bar inside the panel
        // ----------------------------------------------------------------
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        header.set_margin_start(12);
        header.set_margin_end(12);
        header.set_margin_top(6);
        header.set_margin_bottom(6);

        let path_label = gtk::Label::new(Some(&format_path(&start_path)));
        path_label.set_hexpand(true);
        path_label.set_halign(gtk::Align::Start);
        path_label.set_ellipsize(gtk::pango::EllipsizeMode::Start);
        path_label.add_css_class("caption");
        path_label.set_tooltip_text(Some(start_path.to_str().unwrap_or("")));

        let home_btn = gtk::Button::from_icon_name("go-home-symbolic");
        home_btn.set_tooltip_text(Some(crate::tr_en!("Go to Home")));
        home_btn.add_css_class("flat");
        home_btn.add_css_class("circular");

        let up_btn = gtk::Button::from_icon_name("go-up-symbolic");
        up_btn.set_tooltip_text(Some(crate::tr_en!("Go Up")));
        up_btn.add_css_class("flat");
        up_btn.add_css_class("circular");

        let refresh_btn = gtk::Button::from_icon_name("view-refresh-symbolic");
        refresh_btn.set_tooltip_text(Some(crate::tr_en!("Refresh")));
        refresh_btn.add_css_class("flat");
        refresh_btn.add_css_class("circular");

        // Browse somewhere other than home. Without it this panel could only
        // walk down from the home directory — a cube on an external disk was
        // unreachable from here however many times you pressed Up.
        let open_folder_btn = gtk::Button::from_icon_name("folder-open-symbolic");
        open_folder_btn.set_tooltip_text(Some(crate::tr_en!("Browse another folder…")));
        open_folder_btn.add_css_class("flat");
        open_folder_btn.add_css_class("circular");

        header.append(&path_label);
        header.append(&open_folder_btn);
        header.append(&home_btn);
        header.append(&up_btn);
        header.append(&refresh_btn);

        // A directory of a few hundred frames is the normal case for this panel,
        // and scrolling one to find `1234567p` is not finding it.
        let filter_entry = gtk::SearchEntry::new();
        filter_entry.set_placeholder_text(Some(crate::tr_en!("Filter this folder…")));
        filter_entry.set_margin_start(12);
        filter_entry.set_margin_end(12);
        filter_entry.set_margin_bottom(6);

        let sep = gtk::Separator::new(gtk::Orientation::Horizontal);

        // ----------------------------------------------------------------
        // Scrollable file list
        // ----------------------------------------------------------------
        let scrolled = gtk::ScrolledWindow::new();
        scrolled.set_vexpand(true);
        scrolled.set_hexpand(false);
        scrolled.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);

        let list_box = gtk::ListBox::new();
        list_box.set_selection_mode(gtk::SelectionMode::Single);
        list_box.add_css_class("navigation-sidebar");
        list_box.set_margin_start(12);
        list_box.set_margin_end(12);
        list_box.set_margin_top(6);
        list_box.set_margin_bottom(12);
        scrolled.set_child(Some(&list_box));

        widget.append(&header);
        widget.append(&filter_entry);
        widget.append(&sep);
        widget.append(&scrolled);

        let current_path = Rc::new(RefCell::new(start_path));
        let entries: Rc<RefCell<Vec<DirEntry>>> = Rc::new(RefCell::new(Vec::new()));
        let on_open_file: Rc<RefCell<Option<OpenFileCb>>> = Rc::new(RefCell::new(None));

        let panel = Rc::new(FilePanel {
            widget,
            list_box,
            filter_entry,
            path_label,
            current_path,
            entries,
            on_open_file,
        });

        // ----------------------------------------------------------------
        // Wire up navigation buttons
        // ----------------------------------------------------------------
        {
            let p = Rc::clone(&panel);
            home_btn.connect_clicked(move |_| {
                let home = UserDirs::new()
                    .map(|u| u.home_dir().to_path_buf())
                    .unwrap_or_else(|| PathBuf::from("/"));
                *p.current_path.borrow_mut() = home;
                p.populate();
            });
        }

        {
            let p = Rc::clone(&panel);
            up_btn.connect_clicked(move |_| {
                let new_path = {
                    let current = p.current_path.borrow();
                    current
                        .parent()
                        .map(|par| par.to_path_buf())
                        .unwrap_or_else(|| current.clone())
                };
                *p.current_path.borrow_mut() = new_path;
                p.populate();
            });
        }

        {
            let p = Rc::clone(&panel);
            refresh_btn.connect_clicked(move |_| {
                p.populate();
            });
        }
        {
            let p = panel.clone();
            panel.filter_entry.connect_search_changed(move |_| {
                p.populate();
            });
        }
        // Browse elsewhere: a portal folder chooser, so the panel can reach a
        // mounted disk or a scratch directory rather than only what lies under
        // home.
        {
            let p = panel.clone();
            open_folder_btn.connect_clicked(move |btn| {
                let root = btn.root().and_downcast::<gtk::Window>();
                let dialog = gtk::FileDialog::builder()
                    .title(crate::tr_en!("Browse another folder"))
                    .build();
                let p = p.clone();
                dialog.select_folder(root.as_ref(), gtk::gio::Cancellable::NONE, move |result| {
                    if let Some(folder) = result.ok().and_then(|f| f.path()) {
                        *p.current_path.borrow_mut() = folder;
                        p.filter_entry.set_text("");
                        p.populate();
                    }
                });
            });
        }

        // ----------------------------------------------------------------
        // Double-click to navigate or open
        // ----------------------------------------------------------------
        {
            let p = Rc::clone(&panel);
            panel.list_box.connect_row_activated(move |_, row| {
                let idx = row.index() as usize;
                let entry = p.entries.borrow().get(idx).cloned();
                if let Some(entry) = entry {
                    if entry.is_dir {
                        *p.current_path.borrow_mut() = entry.path.clone();
                        p.populate();
                    } else {
                        let file_type = FileType::from_path(&entry.path);
                        if let FileType::Other = file_type {
                            // No action for unknown types
                            return;
                        }
                        if let Some(cb) = p.on_open_file.borrow().as_ref() {
                            cb(entry.path.clone(), file_type);
                        }
                    }
                }
            });
        }

        // Initial population
        panel.populate();

        panel
    }

    /// The root widget to embed in the sidebar.
    pub fn widget(&self) -> &gtk::Box {
        &self.widget
    }

    /// Register a callback that fires when the user double-clicks an
    /// actionable file (FITS or Notebook).
    pub fn set_on_open_file(&self, cb: impl Fn(PathBuf, FileType) + 'static) {
        *self.on_open_file.borrow_mut() = Some(Box::new(cb));
    }

    /// Re-read the current directory from disk and repopulate the list.
    pub fn refresh(&self) {
        self.populate();
    }

    // ----------------------------------------------------------------
    // Private helpers
    // ----------------------------------------------------------------

    fn populate(&self) {
        let path = self.current_path.borrow().clone();

        // Update path label
        self.path_label.set_text(&format_path(&path));
        self.path_label
            .set_tooltip_text(Some(path.to_str().unwrap_or("")));

        // Read directory contents
        let mut dirs: Vec<DirEntry> = Vec::new();
        let mut files: Vec<DirEntry> = Vec::new();

        match std::fs::read_dir(&path) {
            Ok(rd) => {
                // Case-insensitive substring, the same rule the other filter
                // boxes in this app use — a reader who learned it once should
                // not have to learn a second one here.
                let needle = self.filter_entry.text().to_string().to_lowercase();
                for entry_result in rd.flatten() {
                    let entry_path = entry_result.path();
                    let name = entry_result.file_name().to_string_lossy().to_string();

                    if !needle.is_empty() && !name.to_lowercase().contains(&needle) {
                        continue;
                    }

                    // Skip hidden entries (dot-files)
                    if name.starts_with('.') {
                        continue;
                    }

                    let meta = entry_result.metadata().ok();
                    let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);
                    let size = if is_dir { None } else { meta.map(|m| m.len()) };

                    let de = DirEntry {
                        path: entry_path,
                        name,
                        is_dir,
                        size,
                    };

                    if is_dir {
                        dirs.push(de);
                    } else {
                        files.push(de);
                    }
                }
            }
            Err(_) => {
                // Unreadable directory — show nothing
            }
        }

        // Sort alphabetically, case-insensitive
        dirs.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        files.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

        let all_entries: Vec<DirEntry> = dirs.into_iter().chain(files).collect();

        // Rebuild the list
        while let Some(child) = self.list_box.first_child() {
            self.list_box.remove(&child);
        }

        for entry in &all_entries {
            self.list_box.append(&build_row(entry));
        }

        *self.entries.borrow_mut() = all_entries;
    }
}

// ---------------------------------------------------------------------------
// Row builder
// ---------------------------------------------------------------------------

fn build_row(entry: &DirEntry) -> gtk::ListBoxRow {
    let row_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    row_box.set_margin_start(6);
    row_box.set_margin_end(6);
    row_box.set_margin_top(6);
    row_box.set_margin_bottom(6);

    let icon = gtk::Image::from_icon_name(entry.icon_name());
    icon.set_pixel_size(16);
    icon.set_valign(gtk::Align::Center);
    row_box.append(&icon);

    let name_label = gtk::Label::new(Some(&entry.name));
    name_label.set_hexpand(true);
    name_label.set_halign(gtk::Align::Start);
    name_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    name_label.set_max_width_chars(24);
    row_box.append(&name_label);

    if !entry.is_dir {
        let size_label = gtk::Label::new(Some(&entry.size_label()));
        size_label.add_css_class("dim-label");
        size_label.add_css_class("caption");
        size_label.set_halign(gtk::Align::End);
        row_box.append(&size_label);
    }

    // Right-click actions, as the reference's context menu offers: copy the
    // path, and hand the file to the desktop's own file manager. Creating,
    // renaming and deleting are deliberately NOT here — this panel exists to
    // open a file in the app, and the system file manager is a better file
    // manager than a 280 px sidebar will ever be. (VOSpace is different: the
    // Storage page does manage files, because for a remote store there is no
    // other tool.)
    let menu = gtk::gio::Menu::new();
    menu.append(Some(crate::tr_en!("Copy Path")), Some("row.copy-path"));
    menu.append(
        Some(crate::tr_en!("Show in Files")),
        Some("row.show-in-files"),
    );

    let actions = gtk::gio::SimpleActionGroup::new();
    {
        let path = entry.path.clone();
        let copy = gtk::gio::SimpleAction::new("copy-path", None);
        let row_box_for_copy = row_box.clone();
        copy.connect_activate(move |_, _| {
            row_box_for_copy
                .clipboard()
                .set_text(path.to_str().unwrap_or_default());
        });
        actions.add_action(&copy);
    }
    {
        let path = entry.path.clone();
        let show = gtk::gio::SimpleAction::new("show-in-files", None);
        show.connect_activate(move |_, _| {
            // The containing folder for a file, the folder itself for a folder —
            // "show me this" means the place you can see it.
            let target = if path.is_dir() {
                path.clone()
            } else {
                path.parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|| path.clone())
            };
            let _ = open::that(target);
        });
        actions.add_action(&show);
    }

    let popover = gtk::PopoverMenu::from_model(Some(&menu));
    popover.set_has_arrow(false);
    popover.set_halign(gtk::Align::Start);

    let row = gtk::ListBoxRow::new();
    row.set_child(Some(&row_box));
    row.set_tooltip_text(Some(entry.path.to_str().unwrap_or(&entry.name)));
    row.insert_action_group("row", Some(&actions));
    popover.set_parent(&row);

    let gesture = gtk::GestureClick::new();
    gesture.set_button(gtk::gdk::BUTTON_SECONDARY);
    {
        let popover = popover.clone();
        gesture.connect_pressed(move |_, _, x, y| {
            popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
            popover.popup();
        });
    }
    row.add_controller(gesture);
    // The popover is parented to the row, so it must go when the row does —
    // otherwise every repopulate leaks one, and this list repopulates on every
    // keystroke in the filter.
    row.connect_destroy(move |_| popover.unparent());
    row
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn format_path(path: &std::path::Path) -> String {
    // Show home as ~ to save space
    if let Some(user_dirs) = UserDirs::new() {
        let home = user_dirs.home_dir();
        if let Ok(rel) = path.strip_prefix(home) {
            let rel_str = rel.to_string_lossy();
            if rel_str.is_empty() {
                return "~/".to_string();
            }
            return format!("~/{}", rel_str);
        }
    }
    path.to_string_lossy().to_string()
}

#[cfg(test)]
mod affordance_tests {
    //! What this panel is FOR, and what it deliberately is not.
    //!
    //! It exists to open a local file in the app. The reference's equivalent
    //! (`LocalFileBrowserPanel`) offers twelve affordances; ours offered three —
    //! home, up, refresh — so a folder of three hundred frames had to be
    //! scrolled, and anything outside the home directory was unreachable however
    //! many times you pressed Up.

    fn source() -> &'static str {
        crate::testing::code(include_str!("file_panel.rs"))
    }

    #[test]
    fn a_crowded_folder_can_be_narrowed() {
        assert!(
            source().contains(r#"crate::tr_en!("Filter this folder…")"#),
            "a folder of hundreds of frames needs a filter, or finding one is scrolling"
        );
    }

    #[test]
    fn somewhere_other_than_home_can_be_reached() {
        assert!(
            source().contains("select_folder("),
            "without a folder chooser this panel can only walk down from home"
        );
    }

    #[test]
    fn a_row_offers_the_two_actions_that_are_ours_to_offer() {
        let code = source();
        for action in ["row.copy-path", "row.show-in-files"] {
            assert!(code.contains(action), "the row menu is missing {action}");
        }
    }

    #[test]
    fn file_management_is_left_to_the_file_manager() {
        // The reference's menu also creates, renames and deletes. Deliberately
        // not here: a 280 px sidebar whose job is "open this file" is a worse
        // file manager than the one the desktop already ships, and we hand the
        // file to it. VOSpace is the opposite case — the Storage page DOES
        // manage files, because for a remote store there is no other tool.
        let code = source();
        for absent in ["row.delete", "row.rename", "row.new-folder"] {
            assert!(
                !code.contains(absent),
                "{absent} appeared; if local file management is wanted, decide it \
                 deliberately rather than by accretion"
            );
        }
    }
}
