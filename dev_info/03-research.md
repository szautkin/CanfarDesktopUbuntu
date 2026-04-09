# 03 - Research Module Specification

## Purpose

The Research module manages downloaded observations. When the Search module downloads an observation file, it is registered in a persistent local store. The Research module provides browsing, preview, metadata inspection, and file management for these local files.

## Layout

Two-column horizontal split using `gtk::Box` with `Orientation::Horizontal`.

### Left Panel: Observation List (300px fixed width)

```
+-------------------------------+
| Filter entry (search bar)     |
+-------------------------------+
| [icon] M31 JWST/NIRCAM       |  <- adw::ActionRow
|         F444W | Cal 2         |
+-------------------------------+
| [icon] NGC 1234 CFHT/WIRCam  |
|         K | Cal 1             |
+-------------------------------+
| ...                           |
+-------------------------------+
| 12 observations | 3.2 GB      |  <- status bar
+-------------------------------+
```

- Container: `gtk::ScrolledWindow` with fixed `set_size_request(300, -1)`.
- Filter bar: `gtk::SearchEntry` at top, filters observation list by target name, collection, instrument, or observation ID. Case-insensitive substring match.
- List widget: `gtk::ListBox` with `SelectionMode::Single` and `boxed-list` CSS class.
- Each row: `adw::ActionRow` with:
  - Prefix: `gtk::Image` with icon mapped by collection (JWST = `starred-symbolic`, HST = `starred-symbolic`, default = `image-x-generic-symbolic`).
  - Title: `"{TargetName}"` (fallback to ObservationID if TargetName is empty).
  - Subtitle: `"{Collection}/{Instrument} | {Filter} | Cal {CalLevel}"`.
- Status bar: `gtk::Label` at bottom showing `"{count} observations | {total_size}"`.
- Selection emits a signal to update the right detail panel.

### Right Panel: Detail View

```
+----------------------------------------------+
| Preview Image (async loaded)                  |
| 400x300, centered, placeholder while loading  |
+----------------------------------------------+
| Metadata Grid                                 |
| Collection:  JWST         Obs ID: jw01234     |
| Target:      M31          Instrument: NIRCAM  |
| Filter:      F444W        Cal Level: 2        |
| RA:          10.684       Dec: 41.269         |
| Start Date:  2024-01-15                       |
+----------------------------------------------+
| File Info                                     |
| Path: /home/user/Downloads/obs_12345.fits     |
| Size: 145.3 MB                                |
| Downloaded: 2024-03-15 14:22:31               |
| Status: [green] File exists                   |
+----------------------------------------------+
| [Open File] [Show in Files] [Delete]          |
+----------------------------------------------+
```

- Preview image: `gtk::Picture` inside a fixed-height container (400x300). Loaded asynchronously from the DataLink preview URL. Shows a spinner (`gtk::Spinner`) while loading, falls back to `image-x-generic-symbolic` placeholder on error.
- Metadata grid: `gtk::Grid` with 2 columns (label + value), 5 rows. Labels use `dim-label` CSS class. Values are selectable `gtk::Label` widgets.
- File info section: `adw::PreferencesGroup` with `adw::ActionRow` entries for path, size, download timestamp, and existence check.
- Existence check: On selection, verify `LocalPath` exists on disk. Show green checkmark icon if present, red warning icon if file is missing.
- Action buttons: `gtk::Box` horizontal with three buttons:
  - **Open File**: `suggested-action` CSS class.
  - **Show in File Manager**: flat button.
  - **Delete**: `destructive-action` CSS class.

## Data Model

### DownloadedObservation

```rust
use serde::{Deserialize, Serialize};
use chrono::{DateTime, Utc};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadedObservation {
    pub id: String,                     // UUID v4 (generated at download time)
    pub publisher_id: String,           // CAOM2 Plane.publisherID (unique key for dedup)
    pub collection: String,             // e.g., "JWST", "HST", "CFHT"
    pub observation_id: String,         // CAOM2 Observation.observationID
    pub target_name: String,            // Observation.target_name
    pub instrument: String,             // Observation.instrument_name
    pub filter: String,                 // Plane.energy_bandpassName
    pub ra: f64,                        // Right ascension in degrees (J2000)
    pub dec: f64,                       // Declination in degrees (J2000)
    pub start_date: Option<String>,     // Plane.time_bounds_lower (ISO 8601)
    pub cal_level: i32,                 // Plane.calibrationLevel (0-4)
    pub local_path: String,             // Absolute path to downloaded file on disk
    pub file_size: u64,                 // File size in bytes
    pub downloaded_at: DateTime<Utc>,   // Timestamp of download completion
    pub thumbnail_url: Option<String>,  // DataLink thumbnail URL (small preview)
    pub preview_url: Option<String>,    // DataLink preview URL (larger image)
}
```

Fields are populated from the SearchResultRow at download time. The `publisher_id` is extracted from the `publisherID` column in CADC TAP results and serves as the deduplication key.

## ObservationStore

Persistent JSON store for downloaded observations. File location: `{ProjectDirs::data_dir()}/downloaded_observations.json`.

### Storage Format

```json
[
  {
    "id": "a1b2c3d4-...",
    "publisher_id": "ivo://cadc.nrc.ca/JWST?jw01234/jw01234001001_02101_00001_nrca1/jw01234001001_02101_00001_nrca1_cal",
    "collection": "JWST",
    "observation_id": "jw01234",
    "target_name": "M31",
    "instrument": "NIRCAM",
    "filter": "F444W",
    "ra": 10.684,
    "dec": 41.269,
    "start_date": "2024-01-15T00:00:00Z",
    "cal_level": 2,
    "local_path": "/home/user/Downloads/jw01234_nrca1_cal.fits",
    "file_size": 152371200,
    "downloaded_at": "2024-03-15T14:22:31Z",
    "thumbnail_url": "https://www.cadc-ccda.hia-iha.nrc-cnrc.gc.ca/caom2ops/...",
    "preview_url": "https://www.cadc-ccda.hia-iha.nrc-cnrc.gc.ca/caom2ops/..."
  }
]
```

### API

```rust
pub struct ObservationStore {
    file_path: PathBuf,
}

impl ObservationStore {
    pub fn new() -> Self;

    /// Load all observations from disk. Returns empty vec on missing/corrupt file.
    pub fn load(&self) -> Vec<DownloadedObservation>;

    /// Save a new observation. Deduplicates by publisher_id (replaces existing
    /// entry if same publisher_id found). Inserts at position 0 for MRU ordering.
    pub fn save(&self, obs: DownloadedObservation) -> Result<(), String>;

    /// Remove observation by id. Also deletes the local file if it exists.
    pub fn remove(&self, id: &str) -> Result<(), String>;

    /// Remove entries whose local_path no longer exists on disk.
    /// Called on module activation (when user navigates to Research tab).
    pub fn prune_stale(&self) -> Result<usize, String>;

    /// Find by publisher_id. Used to check if already downloaded.
    pub fn find_by_publisher_id(&self, publisher_id: &str) -> Option<DownloadedObservation>;
}
```

### Behavior

- **MRU order**: Most recently downloaded/accessed observations appear first. `save()` always inserts at index 0.
- **Deduplication**: Before inserting, scan for existing entry with same `publisher_id`. If found, remove the old entry and insert the new one at position 0.
- **Stale pruning**: On module load, iterate all entries. For each entry where `!Path::new(&obs.local_path).exists()`, remove from the list. Display a toast: `"Removed {n} stale entries (files no longer on disk)"`.
- **Thread safety**: All store operations happen on the main thread (GTK context). No Mutex needed.
- **File persistence**: Same pattern as `RecentLaunchService` -- atomic write of full JSON array. Create parent directories if needed.

## Preview Image Loading

### DataLink URL Resolution

When a search result is downloaded, the preview URL is obtained from the CADC DataLink service:

```
GET https://ws.cadc-ccda.hia-iha.nrc-cnrc.gc.ca/caom2ops/datalink?ID={publisherID}
```

Parse the VOTable response to find the `#preview` and `#thumbnail` access URLs. Store these URLs in the `DownloadedObservation`.

### Async Image Loading

When an observation is selected in the list:

1. Show spinner in preview area.
2. Spawn async task: `GET preview_url` (or thumbnail_url as fallback).
3. On success: decode image bytes (PNG/JPEG) into `gdk_pixbuf::Pixbuf`, scale to fit 400x300 container, create `gdk::Texture`, set on `gtk::Picture`.
4. On failure: show `image-x-generic-symbolic` placeholder with `dim-label` style.
5. Cache loaded pixbufs in a `HashMap<String, gdk::Texture>` keyed by observation id (max 20 entries, LRU eviction).

## Actions

### Open File

```rust
fn open_file(obs: &DownloadedObservation) {
    let path = Path::new(&obs.local_path);
    if !path.exists() {
        // Show toast: "File not found: {path}"
        return;
    }
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    if ext.eq_ignore_ascii_case("fits") || ext.eq_ignore_ascii_case("fit") || ext.eq_ignore_ascii_case("fts") {
        // Fire OpenInFitsViewer event (see Integration below)
    } else {
        // Use system default handler
        open::that(path);
    }
}
```

### Show in File Manager

```rust
fn show_in_file_manager(obs: &DownloadedObservation) {
    let path = Path::new(&obs.local_path);
    if let Some(parent) = path.parent() {
        open::that(parent);
    }
}
```

### Delete

1. Show confirmation dialog: `adw::MessageDialog` with heading "Delete Observation?" and body "This will remove the local file and the observation record."
2. On confirm:
   - Delete local file: `std::fs::remove_file(&obs.local_path)` (ignore error if already gone).
   - Remove from store: `observation_store.remove(&obs.id)`.
   - Refresh the observation list.
   - Show toast: `"Deleted {target_name}"`.

## Integration with Other Modules

### Search -> Research (Download Flow)

When the Search module downloads a file:

1. Search page resolves DataLink URLs for the selected observation.
2. Download file to user-chosen path (or default `~/Downloads/`).
3. Construct `DownloadedObservation` from the search result row data.
4. Call `ObservationStore::save(obs)`.
5. Show toast: `"Downloaded {filename} - view in Research"`.

The Search page needs access to a shared `Rc<ObservationStore>` instance, passed through `AppServices` or held at the main window level.

### Research -> FITS Viewer (Open in FITS Viewer)

When "Open File" is clicked for a FITS file:

1. Research module fires a callback/event that the main window listens for.
2. Main window calls `fits_viewer.load_from_path(&path)`.
3. Main window switches the `ViewStack` to the "fits" page.

Implementation: The `ResearchPage` struct holds an `on_open_fits: Rc<RefCell<Option<Box<dyn Fn(PathBuf)>>>>` callback. The main window sets this callback during construction to wire the navigation.

### Research -> Search (Search at Position)

Optional future feature: A "Search Near This Position" button in the detail view that:
1. Sets the Search module's target RA/Dec from the observation's coordinates.
2. Switches to the Search tab.
3. Auto-executes the search.

## Module Files to Create

| File | Purpose |
|------|---------|
| `src/models/downloaded_observation.rs` | `DownloadedObservation` struct |
| `src/services/observation_store.rs` | JSON persistence, dedup, prune |
| `src/ui/research_page.rs` | Full UI: list + detail + actions |

Update `src/models/mod.rs`, `src/services/mod.rs`, `src/ui/mod.rs` to include the new modules. Replace the placeholder page in `main_window.rs` with the real `ResearchPage`.

## GTK4/Adwaita Widget Mapping

| Concept | Widget |
|---------|--------|
| Observation list | `gtk::ListBox` with `adw::ActionRow` rows |
| Filter bar | `gtk::SearchEntry` |
| Preview image | `gtk::Picture` inside `gtk::Frame` |
| Metadata grid | `gtk::Grid` with label/value pairs |
| File info | `adw::PreferencesGroup` with `adw::ActionRow` entries |
| Action buttons | `gtk::Button` in horizontal `gtk::Box` |
| Confirmation dialog | `adw::MessageDialog` |
| Status messages | `adw::Toast` via `adw::ToastOverlay` |

## Error Handling

- File not found on open/show: Display `adw::Toast` with error message, update file info status to show warning icon.
- Store read/write failure: Log error, display toast. Never crash.
- Preview load failure: Show placeholder icon, no error dialog.
- Invalid JSON in store file: Log warning, return empty list. Do not delete the corrupted file (user may want to recover).
