# Research Module -- Implementation Plan

## Current State

The Research module is **entirely new**. No source files exist for it yet. The main window currently has a placeholder page for the "Research" tab in the `ViewStack`. The following infrastructure exists that this module depends on:

- `ObservationStore` -- does not exist, needs to be created
- `DownloadedObservation` model -- does not exist
- `SearchPage` download flow (Step 10 in 02-search-plan.md) -- not yet implemented; the Research module needs this to receive downloaded observations
- `FitsViewer::load_from_path()` at `src/ui/fits_viewer.rs` line 219 -- already works, can be called from Research
- `directories::ProjectDirs` pattern -- used throughout (e.g., `src/services/template_service.rs`)
- `adw::ToastOverlay` -- not yet used in the app but available via libadwaita

**Files to create**:
- `src/models/downloaded_observation.rs`
- `src/services/observation_store.rs`
- `src/ui/research_page.rs`

**Files to modify**:
- `src/models/mod.rs` -- add `pub mod downloaded_observation;`
- `src/services/mod.rs` -- add `pub mod observation_store;`
- `src/ui/mod.rs` -- add `pub mod research_page;`
- `src/state.rs` -- add `ObservationStore` to `AppServices`
- `src/ui/main_window.rs` -- replace placeholder with `ResearchPage`

---

## Step 1: DownloadedObservation Model

**File**: `src/models/downloaded_observation.rs` (new)

```rust
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadedObservation {
    pub id: String,                     // UUID v4
    pub publisher_id: String,           // CAOM2 Plane.publisherID (dedup key)
    pub collection: String,             // e.g., "JWST", "HST", "CFHT"
    pub observation_id: String,         // CAOM2 Observation.observationID
    pub target_name: String,            // Observation.target_name
    pub instrument: String,             // Observation.instrument_name
    pub filter: String,                 // Plane.energy_bandpassName
    pub ra: f64,                        // Right ascension (J2000, degrees)
    pub dec: f64,                       // Declination (J2000, degrees)
    pub start_date: Option<String>,     // ISO 8601
    pub cal_level: i32,                 // Plane.calibrationLevel (0-4)
    pub local_path: String,             // Absolute path to downloaded file
    pub file_size: u64,                 // Bytes
    pub downloaded_at: DateTime<Utc>,   // Timestamp
    pub thumbnail_url: Option<String>,  // DataLink #thumbnail URL
    pub preview_url: Option<String>,    // DataLink #preview URL
}

impl DownloadedObservation {
    /// Check if the local file still exists on disk.
    pub fn file_exists(&self) -> bool {
        Path::new(&self.local_path).exists()
    }

    /// Human-readable file size.
    pub fn size_display(&self) -> String {
        let b = self.file_size as f64;
        if b < 1024.0 { format!("{} B", self.file_size) }
        else if b < 1024.0 * 1024.0 { format!("{:.1} KB", b / 1024.0) }
        else if b < 1024.0 * 1024.0 * 1024.0 { format!("{:.1} MB", b / (1024.0 * 1024.0)) }
        else { format!("{:.2} GB", b / (1024.0 * 1024.0 * 1024.0)) }
    }

    /// Display name: target_name or observation_id.
    pub fn display_name(&self) -> &str {
        if self.target_name.is_empty() { &self.observation_id } else { &self.target_name }
    }

    /// Icon name by collection.
    pub fn icon_name(&self) -> &'static str {
        match self.collection.to_uppercase().as_str() {
            "JWST" | "HST" => "starred-symbolic",
            _ => "image-x-generic-symbolic",
        }
    }

    /// Check if the file is a FITS file by extension.
    pub fn is_fits(&self) -> bool {
        let ext = Path::new(&self.local_path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        matches!(ext.as_str(), "fits" | "fit" | "fts" | "fz")
    }
}
```

### Conversion from SearchResultRow

Add a helper function (or `From` impl) for constructing a `DownloadedObservation` from search result data + download metadata:

```rust
use crate::models::search_result::SearchResultRow;
use uuid::Uuid;

impl DownloadedObservation {
    pub fn from_search_result(
        row: &SearchResultRow,
        local_path: String,
        file_size: u64,
        thumbnail_url: Option<String>,
        preview_url: Option<String>,
    ) -> Self {
        DownloadedObservation {
            id: Uuid::new_v4().to_string(),
            publisher_id: row.get("Plane.publisherID").to_string(),
            collection: row.get("Observation.collection").to_string(),
            observation_id: row.get("Observation.observationID").to_string(),
            target_name: row.get("Target Name").to_string(),
            instrument: row.get("Instrument").to_string(),
            filter: row.get("Filter").to_string(),
            ra: row.get("RA (J2000.0)").parse().unwrap_or(0.0),
            dec: row.get("Dec. (J2000.0)").parse().unwrap_or(0.0),
            start_date: {
                let s = row.get("Start Date");
                if s.is_empty() { None } else { Some(s.to_string()) }
            },
            cal_level: row.get("Cal. Lev.").parse().unwrap_or(0),
            local_path,
            file_size,
            downloaded_at: Utc::now(),
            thumbnail_url,
            preview_url,
        }
    }
}
```

**Cargo.toml**: Add `uuid = { version = "1", features = ["v4"] }` if not already present.

**File**: `src/models/mod.rs` -- Add `pub mod downloaded_observation;` and `pub use downloaded_observation::DownloadedObservation;`

**Dependencies**: None.

---

## Step 2: ObservationStore Service

**File**: `src/services/observation_store.rs` (new)

```rust
use crate::models::DownloadedObservation;
use directories::ProjectDirs;
use std::path::{Path, PathBuf};

pub struct ObservationStore {
    file_path: PathBuf,
}

impl ObservationStore {
    pub fn new() -> Self {
        let file_path = ProjectDirs::from("net", "canfar", "Verbinal")
            .map(|dirs| dirs.data_dir().join("downloaded_observations.json"))
            .unwrap_or_else(|| PathBuf::from("downloaded_observations.json"));
        ObservationStore { file_path }
    }

    /// Load all observations from disk. Returns empty vec on missing/corrupt file.
    pub fn load(&self) -> Vec<DownloadedObservation> {
        if !self.file_path.exists() {
            return Vec::new();
        }
        match std::fs::read_to_string(&self.file_path) {
            Ok(contents) => serde_json::from_str(&contents).unwrap_or_default(),
            Err(e) => {
                eprintln!("Warning: could not read observation store: {}", e);
                Vec::new()
            }
        }
    }

    /// Save a new observation. Deduplicates by publisher_id.
    /// Inserts at position 0 for MRU ordering.
    pub fn save(&self, obs: DownloadedObservation) -> Result<(), String> {
        let mut all = self.load();
        // Dedup: remove existing entry with same publisher_id
        all.retain(|o| o.publisher_id != obs.publisher_id);
        // Insert at front
        all.insert(0, obs);
        self.write_all(&all)
    }

    /// Remove observation by id. Also deletes the local file if it exists.
    pub fn remove(&self, id: &str) -> Result<(), String> {
        let mut all = self.load();
        if let Some(obs) = all.iter().find(|o| o.id == id) {
            let path = Path::new(&obs.local_path);
            if path.exists() {
                let _ = std::fs::remove_file(path); // Ignore error if already gone
            }
        }
        all.retain(|o| o.id != id);
        self.write_all(&all)
    }

    /// Remove entries whose local_path no longer exists on disk.
    /// Returns count of removed entries.
    pub fn prune_stale(&self) -> Result<usize, String> {
        let all = self.load();
        let before = all.len();
        let remaining: Vec<DownloadedObservation> = all.into_iter()
            .filter(|o| Path::new(&o.local_path).exists())
            .collect();
        let removed = before - remaining.len();
        if removed > 0 {
            self.write_all(&remaining)?;
        }
        Ok(removed)
    }

    /// Find by publisher_id. Used to check if already downloaded.
    pub fn find_by_publisher_id(&self, publisher_id: &str) -> Option<DownloadedObservation> {
        self.load().into_iter().find(|o| o.publisher_id == publisher_id)
    }

    fn write_all(&self, obs: &[DownloadedObservation]) -> Result<(), String> {
        if let Some(parent) = self.file_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let json = serde_json::to_string_pretty(obs).map_err(|e| e.to_string())?;
        std::fs::write(&self.file_path, json).map_err(|e| e.to_string())
    }
}
```

**File**: `src/services/mod.rs` -- Add `pub mod observation_store;` and `pub use observation_store::ObservationStore;`

**File**: `src/state.rs` -- Add `pub observation_store: ObservationStore` to `AppServices` struct (after line 19). Initialize as `observation_store: ObservationStore::new()` in `new()`.

**Dependencies**: Step 1 (DownloadedObservation model).

---

## Step 3: UI -- Observation List (Left Panel)

**File**: `src/ui/research_page.rs` (new)

### 3a: Struct definition

```rust
use crate::models::DownloadedObservation;
use crate::services::ObservationStore;
use crate::state::AppServices;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

pub struct ResearchPage {
    widget: gtk::Box,
    services: Arc<AppServices>,
    observations: Rc<RefCell<Vec<DownloadedObservation>>>,
    list_box: gtk::ListBox,
    filter_entry: gtk::SearchEntry,
    status_label: gtk::Label,
    detail_view: Rc<DetailView>,
    toast_overlay: adw::ToastOverlay,
    on_open_fits: Rc<RefCell<Option<Box<dyn Fn(PathBuf)>>>>,
}
```

### 3b: Constructor layout

```rust
impl ResearchPage {
    pub fn new(services: Arc<AppServices>) -> Rc<Self> {
        let toast_overlay = adw::ToastOverlay::new();
        let outer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        outer.set_vexpand(true);
        outer.set_hexpand(true);

        // --- Left Panel: Observation List (300px) ---
        let left = gtk::Box::new(gtk::Orientation::Vertical, 0);
        left.set_size_request(300, -1);

        let filter_entry = gtk::SearchEntry::new();
        filter_entry.set_placeholder_text(Some("Filter observations..."));
        filter_entry.set_margin_start(8);
        filter_entry.set_margin_end(8);
        filter_entry.set_margin_top(8);
        left.append(&filter_entry);

        let scrolled = gtk::ScrolledWindow::new();
        scrolled.set_vexpand(true);
        let list_box = gtk::ListBox::new();
        list_box.set_selection_mode(gtk::SelectionMode::Single);
        list_box.add_css_class("boxed-list");
        list_box.set_margin_start(8);
        list_box.set_margin_end(8);
        list_box.set_margin_top(4);
        list_box.set_margin_bottom(8);
        scrolled.set_child(Some(&list_box));
        left.append(&scrolled);

        let status_label = gtk::Label::new(Some("0 observations"));
        status_label.add_css_class("dim-label");
        status_label.add_css_class("caption");
        status_label.set_margin_start(8);
        status_label.set_margin_bottom(4);
        status_label.set_halign(gtk::Align::Start);
        left.append(&status_label);

        outer.append(&left);
        outer.append(&gtk::Separator::new(gtk::Orientation::Vertical));

        // --- Right Panel: Detail View ---
        let detail_view = DetailView::new();
        outer.append(detail_view.widget());

        toast_overlay.set_child(Some(&outer));

        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        widget.set_vexpand(true);
        widget.set_hexpand(true);
        widget.append(&toast_overlay);

        let page = Rc::new(ResearchPage {
            widget,
            services,
            observations: Rc::new(RefCell::new(Vec::new())),
            list_box,
            filter_entry,
            status_label,
            detail_view,
            toast_overlay,
            on_open_fits: Rc::new(RefCell::new(None)),
        });

        // Wire filter entry
        let p = page.clone();
        page.filter_entry.connect_search_changed(move |_| { p.refresh_list(); });

        // Wire list selection
        let p = page.clone();
        page.list_box.connect_row_selected(move |_, row| {
            if let Some(row) = row {
                let idx = row.index() as usize;
                p.on_observation_selected(idx);
            }
        });

        page
    }
}
```

### 3c: List population

```rust
impl ResearchPage {
    pub fn refresh_list(&self) {
        let all = self.services.observation_store.load();
        let filter_text = self.filter_entry.text().to_string().to_lowercase();

        // Clear list
        while let Some(child) = self.list_box.first_child() {
            self.list_box.remove(&child);
        }

        let filtered: Vec<DownloadedObservation> = all.into_iter().filter(|obs| {
            if filter_text.is_empty() { return true; }
            obs.target_name.to_lowercase().contains(&filter_text)
                || obs.collection.to_lowercase().contains(&filter_text)
                || obs.instrument.to_lowercase().contains(&filter_text)
                || obs.observation_id.to_lowercase().contains(&filter_text)
        }).collect();

        // Status bar
        let total_size: u64 = filtered.iter().map(|o| o.file_size).sum();
        let size_display = DownloadedObservation { file_size: total_size, ..Default::default() }.size_display();
        self.status_label.set_text(&format!("{} observations | {}", filtered.len(), size_display));

        // Build rows
        for obs in &filtered {
            let row = adw::ActionRow::builder()
                .title(obs.display_name())
                .subtitle(format!("{}/{} | {} | Cal {}", obs.collection, obs.instrument, obs.filter, obs.cal_level))
                .build();
            row.add_prefix(&gtk::Image::from_icon_name(obs.icon_name()));
            self.list_box.append(&row);
        }

        *self.observations.borrow_mut() = filtered;
    }
}
```

Note: `DownloadedObservation` needs a `Default` impl or the size_display can be a free function. Simpler: use the existing `size_display` as a standalone helper.

### 3d: Load on activation

```rust
impl ResearchPage {
    /// Called when the Research tab is activated in the ViewStack.
    pub fn activate(&self) {
        // Prune stale entries
        match self.services.observation_store.prune_stale() {
            Ok(n) if n > 0 => {
                let toast = adw::Toast::new(&format!("Removed {} stale entries (files no longer on disk)", n));
                self.toast_overlay.add_toast(toast);
            }
            _ => {}
        }
        self.refresh_list();
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.widget
    }

    pub fn set_on_open_fits(&self, cb: impl Fn(PathBuf) + 'static) {
        *self.on_open_fits.borrow_mut() = Some(Box::new(cb));
    }
}
```

**Dependencies**: Steps 1, 2.

---

## Step 4: UI -- Detail View (Right Panel)

**File**: `src/ui/research_page.rs` (inner struct)

### 4a: DetailView struct

```rust
struct DetailView {
    widget: gtk::Box,
    preview_picture: gtk::Picture,
    preview_spinner: gtk::Spinner,
    metadata_grid: gtk::Grid,
    file_info_group: adw::PreferencesGroup,
    file_status_icon: gtk::Image,
    open_btn: gtk::Button,
    show_btn: gtk::Button,
    delete_btn: gtk::Button,
    action_box: gtk::Box,
}

impl DetailView {
    fn new() -> Rc<Self> {
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 8);
        widget.set_hexpand(true);
        widget.set_vexpand(true);
        widget.set_margin_start(16);
        widget.set_margin_end(16);
        widget.set_margin_top(16);
        widget.set_margin_bottom(16);

        // Preview image container (400x300)
        let preview_frame = gtk::Frame::new(None);
        preview_frame.set_size_request(400, 300);
        let preview_overlay = gtk::Overlay::new();
        let preview_picture = gtk::Picture::new();
        preview_picture.set_content_fit(gtk::ContentFit::Contain);
        preview_picture.set_can_shrink(true);
        preview_overlay.set_child(Some(&preview_picture));
        let preview_spinner = gtk::Spinner::new();
        preview_spinner.set_halign(gtk::Align::Center);
        preview_spinner.set_valign(gtk::Align::Center);
        preview_spinner.set_visible(false);
        preview_overlay.add_overlay(&preview_spinner);
        preview_frame.set_child(Some(&preview_overlay));
        widget.append(&preview_frame);

        // Metadata grid (2 columns x 5 rows)
        let metadata_grid = gtk::Grid::new();
        metadata_grid.set_column_spacing(12);
        metadata_grid.set_row_spacing(4);
        widget.append(&metadata_grid);

        // File info
        let file_info_group = adw::PreferencesGroup::builder().title("File Info").build();
        widget.append(&file_info_group);

        let file_status_icon = gtk::Image::new();

        // Action buttons
        let action_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        action_box.set_margin_top(8);
        let open_btn = gtk::Button::with_label("Open File");
        open_btn.add_css_class("suggested-action");
        let show_btn = gtk::Button::with_label("Show in Files");
        show_btn.add_css_class("flat");
        let delete_btn = gtk::Button::with_label("Delete");
        delete_btn.add_css_class("destructive-action");
        action_box.append(&open_btn);
        action_box.append(&show_btn);
        action_box.append(&delete_btn);
        widget.append(&action_box);

        Rc::new(DetailView {
            widget, preview_picture, preview_spinner, metadata_grid,
            file_info_group, file_status_icon, open_btn, show_btn, delete_btn, action_box,
        })
    }

    fn widget(&self) -> &gtk::Box { &self.widget }
}
```

### 4b: Update detail on selection

```rust
impl DetailView {
    fn update(&self, obs: &DownloadedObservation) {
        // Clear metadata grid
        while let Some(child) = self.metadata_grid.first_child() {
            self.metadata_grid.remove(&child);
        }

        // Populate metadata grid
        let fields = [
            ("Collection:", &obs.collection),
            ("Obs ID:", &obs.observation_id),
            ("Target:", &obs.target_name),
            ("Instrument:", &obs.instrument),
            ("Filter:", &obs.filter),
            ("Cal Level:", &obs.cal_level.to_string()),
            ("RA:", &format!("{:.5}", obs.ra)),
            ("Dec:", &format!("{:.5}", obs.dec)),
            ("Start Date:", obs.start_date.as_deref().unwrap_or("N/A")),
        ];

        for (i, (label_text, value_text)) in fields.iter().enumerate() {
            let col = if i < 5 { 0 } else { 2 };
            let row = if i < 5 { i } else { i - 5 };

            let label = gtk::Label::new(Some(label_text));
            label.add_css_class("dim-label");
            label.set_halign(gtk::Align::End);
            self.metadata_grid.attach(&label, col as i32, row as i32, 1, 1);

            let value = gtk::Label::new(Some(value_text));
            value.set_halign(gtk::Align::Start);
            value.set_selectable(true);
            self.metadata_grid.attach(&value, col as i32 + 1, row as i32, 1, 1);
        }

        // File info section
        // Clear and re-add rows for path, size, downloaded_at, existence
        while let Some(child) = self.file_info_group.first_child() {
            // PreferencesGroup children removal
        }

        // File existence check
        let exists = obs.file_exists();
        self.file_status_icon.set_from_icon_name(Some(
            if exists { "emblem-ok-symbolic" } else { "dialog-warning-symbolic" }
        ));
    }
}
```

**Dependencies**: Step 3.

---

## Step 5: Preview Loading

**File**: `src/ui/research_page.rs`

### 5a: Async preview image download

```rust
impl ResearchPage {
    fn on_observation_selected(&self, idx: usize) {
        let obs_list = self.observations.borrow();
        let Some(obs) = obs_list.get(idx) else { return; };

        self.detail_view.update(obs);

        // Load preview image
        let preview_url = obs.preview_url.clone().or_else(|| obs.thumbnail_url.clone());
        if let Some(url) = preview_url {
            let picture = self.detail_view.preview_picture.clone();
            let spinner = self.detail_view.preview_spinner.clone();
            let services = self.services.clone();

            spinner.set_visible(true);
            spinner.start();

            glib::spawn_future_local(async move {
                let result = services.spawn(async move {
                    let client = reqwest::Client::new();
                    client.get(&url)
                        .timeout(std::time::Duration::from_secs(30))
                        .send().await
                        .map_err(|e| e.to_string())?
                        .bytes().await
                        .map_err(|e| e.to_string())
                }).await;

                spinner.stop();
                spinner.set_visible(false);

                match result {
                    Ok(bytes) => {
                        let gbytes = glib::Bytes::from_owned(bytes.to_vec());
                        let stream = gtk4::gio::MemoryInputStream::from_bytes(&gbytes);
                        match gdk_pixbuf::Pixbuf::from_stream(
                            &stream,
                            gtk4::gio::Cancellable::NONE,
                        ) {
                            Ok(pixbuf) => {
                                let texture = gdk4::Texture::for_pixbuf(&pixbuf);
                                picture.set_paintable(Some(&texture));
                            }
                            Err(_) => {
                                picture.set_icon_name(Some("image-x-generic-symbolic"));
                            }
                        }
                    }
                    Err(_) => {
                        picture.set_icon_name(Some("image-x-generic-symbolic"));
                    }
                }
            });
        } else {
            self.detail_view.preview_picture.set_icon_name(Some("image-x-generic-symbolic"));
        }
    }
}
```

### 5b: Preview cache (optional optimization)

Add a simple LRU cache for loaded textures:

```rust
preview_cache: Rc<RefCell<HashMap<String, gdk4::Texture>>>,
// Max 20 entries; evict oldest on overflow
```

Check cache before starting async download. Insert into cache after successful load.

**Dependencies**: Steps 3, 4.

---

## Step 6: File Actions

**File**: `src/ui/research_page.rs`

### 6a: Wire Open File button

```rust
// In constructor, after detail_view is created:
let p = page.clone();
page.detail_view.open_btn.connect_clicked(move |_| {
    let obs_list = p.observations.borrow();
    let selected_idx = p.list_box.selected_row().map(|r| r.index() as usize);
    let Some(idx) = selected_idx else { return; };
    let Some(obs) = obs_list.get(idx) else { return; };

    let path = std::path::Path::new(&obs.local_path);
    if !path.exists() {
        let toast = adw::Toast::new(&format!("File not found: {}", obs.local_path));
        p.toast_overlay.add_toast(toast);
        return;
    }

    if obs.is_fits() {
        // Fire OpenInFitsViewer callback
        if let Some(ref cb) = *p.on_open_fits.borrow() {
            cb(path.to_path_buf());
        }
    } else {
        let _ = open::that(path);
    }
});
```

### 6b: Wire Show in Files button

```rust
let p = page.clone();
page.detail_view.show_btn.connect_clicked(move |_| {
    let obs_list = p.observations.borrow();
    let selected_idx = p.list_box.selected_row().map(|r| r.index() as usize);
    let Some(idx) = selected_idx else { return; };
    let Some(obs) = obs_list.get(idx) else { return; };

    let path = std::path::Path::new(&obs.local_path);
    if let Some(parent) = path.parent() {
        let _ = open::that(parent);
    }
});
```

### 6c: Wire Delete button with confirmation dialog

```rust
let p = page.clone();
page.detail_view.delete_btn.connect_clicked(move |_| {
    let p = p.clone();
    glib::spawn_future_local(async move {
        let obs_list = p.observations.borrow();
        let selected_idx = p.list_box.selected_row().map(|r| r.index() as usize);
        let Some(idx) = selected_idx else { return; };
        let Some(obs) = obs_list.get(idx).cloned() else { return; };
        drop(obs_list);

        // Show confirmation dialog
        let root = p.widget.root().and_downcast::<gtk::Window>();
        let dialog = adw::MessageDialog::builder()
            .heading("Delete Observation?")
            .body("This will remove the local file and the observation record.")
            .modal(true)
            .build();
        if let Some(ref win) = root {
            dialog.set_transient_for(Some(win));
        }
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("delete", "Delete");
        dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));

        let response = dialog.choose_future().await;
        if response == "delete" {
            let _ = p.services.observation_store.remove(&obs.id);
            p.refresh_list();
            let toast = adw::Toast::new(&format!("Deleted {}", obs.display_name()));
            p.toast_overlay.add_toast(toast);
        }
    });
});
```

**Dependencies**: Steps 3, 4.

---

## Step 7: Integration

### 7a: Register in mod files

**File**: `src/models/mod.rs` -- Add:
```rust
pub mod downloaded_observation;
pub use downloaded_observation::DownloadedObservation;
```

**File**: `src/services/mod.rs` -- Add:
```rust
pub mod observation_store;
pub use observation_store::ObservationStore;
```

**File**: `src/ui/mod.rs` -- Add:
```rust
pub mod research_page;
```

### 7b: Add ObservationStore to AppServices

**File**: `src/state.rs`

Add field to struct (line ~19):
```rust
pub observation_store: ObservationStore,
```

Initialize in `new()` (around line 43):
```rust
observation_store: ObservationStore::new(),
```

### 7c: Replace placeholder in main_window.rs

**File**: `src/ui/main_window.rs`

Find the placeholder page for "Research" in the `ViewStack`. Replace with:

```rust
let research_page = ResearchPage::new(services.clone());
view_stack.add_titled(research_page.widget(), Some("research"), "Research");
```

### 7d: Wire Open in FITS Viewer callback

In main_window.rs, after creating both `research_page` and `fits_viewer`:

```rust
let fits_viewer_ref = fits_viewer.clone();
let view_stack_ref = view_stack.clone();
research_page.set_on_open_fits(move |path| {
    fits_viewer_ref.load_from_path(&path);
    view_stack_ref.set_visible_child_name("fits");
});
```

### 7e: Wire Search -> Research download flow

In the Search module's download completion handler (Step 10 of 02-search-plan.md), after a file is downloaded:

```rust
// Construct DownloadedObservation from search result + download metadata
let obs = DownloadedObservation::from_search_result(
    &row,
    local_path.to_string_lossy().to_string(),
    file_size,
    thumbnail_url,
    preview_url,
);
services.observation_store.save(obs).ok();
// Show toast
let toast = adw::Toast::new(&format!("Downloaded {} - view in Research", filename));
toast_overlay.add_toast(toast);
```

### 7f: Wire ViewStack page activation

In main_window.rs, when the visible child changes to "research":

```rust
view_stack.connect_visible_child_name_notify(move |stack| {
    if stack.visible_child_name().as_deref() == Some("research") {
        research_page.activate();
    }
});
```

**Dependencies**: Steps 1-6 all complete. Search module Step 10 for the download flow wire-up.

---

## Implementation Order

| Step | Description | File(s) | Effort | Dependencies |
|------|-------------|---------|--------|-------------|
| 1 | DownloadedObservation model | `src/models/downloaded_observation.rs` (new) | 30 min | None |
| 2 | ObservationStore service | `src/services/observation_store.rs` (new), `src/state.rs` | 45 min | Step 1 |
| 3 | UI - Observation list | `src/ui/research_page.rs` (new) | 1.5 hr | Steps 1, 2 |
| 4 | UI - Detail view | `src/ui/research_page.rs` | 1.5 hr | Step 3 |
| 5 | Preview loading | `src/ui/research_page.rs` | 1 hr | Steps 3, 4 |
| 6 | File actions | `src/ui/research_page.rs` | 1 hr | Steps 3, 4 |
| 7 | Integration | `src/state.rs`, `src/ui/main_window.rs`, mod files | 1 hr | Steps 1-6 |

**Total estimate**: ~7.5 hours.

Steps 1 and 2 are sequential. Steps 3-6 are sequential (building up the UI). Step 7 is final wiring. The Search module's download flow (02-search-plan Step 10) is a cross-module dependency for the full end-to-end flow but is not blocking for implementing the Research module itself.
