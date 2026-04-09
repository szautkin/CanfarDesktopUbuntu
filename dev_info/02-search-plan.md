# Search Module -- Implementation Plan

## Current State

The Search module has a **partial stub** implementation:

- `src/ui/search_page.rs` -- A two-panel layout (form left, results right). Has basic form with target/radius/collection/instrument/band/max-records. Has resolve button and search button. Displays results as `adw::ActionRow` items in a `ListBox`. **Missing**: sidebar, ADQL editor tab, data train, 4 constraint groups, pagination, sorting, column picker, DataLink, download flow, saved/recent queries.
- `src/services/tap_service.rs` -- Complete: `execute_query()` and `resolve_target()` both work. **Missing**: data train fetch queries, DataLink service.
- `src/helpers/adql_builder.rs` -- Partial: builds SELECT with all columns, has spatial/collection/instrument/band clauses and `escape_sql()`. **Missing**: observation constraints (wildcard/LIKE), temporal constraints (MJD conversion, date presets, range syntax), spectral constraints (unit conversion), data train IN clauses, public-only filter.
- `src/models/search_result.rs` -- Has `SearchResults`, `SearchResultRow`, `SearchFormState` (only 9 fields), `ResolverResult`, `RecentSearch`, `parse_csv()`, `parse_resolver_response()`. **Missing**: full `SearchFormState` (30+ fields), `DataLinkResult`, `DataLinkFile`, `SavedQuery`, `ResultColumnInfo`, `ColumnFormat`.

---

## Step 1: Expand Models

**File**: `src/models/search_result.rs`

### 1a: Expand SearchFormState

Replace the current 9-field `SearchFormState` (lines 28-39) with the full spec version:

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SearchFormState {
    // Spatial
    pub target: String,
    pub resolver_service: String,       // "ALL", "SIMBAD", "NED", "VIZIER"
    pub resolved_ra: Option<f64>,
    pub resolved_dec: Option<f64>,
    pub search_radius: f64,             // Degrees, default 0.05
    pub pixel_scale_max: Option<f64>,   // Arcseconds

    // Observation
    pub observation_id: String,
    pub proposal_pi: String,
    pub proposal_id: String,
    pub proposal_title: String,
    pub proposal_keywords: String,
    pub intent: String,                 // "", "science", "calibration"
    pub public_only: bool,

    // Temporal
    pub date_preset: String,            // "Custom", "Last 24 hours", etc.
    pub obs_date_start: String,         // YYYY-MM-DD or MJD
    pub obs_date_end: String,
    pub integration_time_min: Option<f64>,
    pub integration_time_max: Option<f64>,
    pub time_span_min: Option<f64>,
    pub time_span_max: Option<f64>,

    // Spectral
    pub wavelength_min: Option<f64>,    // Internal: meters
    pub wavelength_max: Option<f64>,
    pub wavelength_unit: String,        // "nm", "um", "Angstrom", "m"
    pub spectral_coverage: Option<f64>,
    pub spectral_sampling: Option<f64>,
    pub resolving_power_min: Option<f64>,
    pub resolving_power_max: Option<f64>,
    pub bandpass_width_min: Option<f64>,
    pub bandpass_width_max: Option<f64>,
    pub rest_frame_energy_min: Option<f64>,
    pub rest_frame_energy_max: Option<f64>,

    // Data Train selections
    pub collection: String,
    pub instrument: String,
    pub band: String,
    pub filter_name: String,
    pub calibration_level: String,
    pub data_product_type: String,
    pub obs_type: String,
    pub data_release: String,

    // Options
    pub max_records: u32,
}

impl SearchFormState {
    pub fn new() -> Self {
        SearchFormState {
            resolver_service: "ALL".to_string(),
            search_radius: 0.05,
            max_records: 1000,
            wavelength_unit: "nm".to_string(),
            ..Default::default()
        }
    }
}
```

Update the existing `SearchFormState::new()` (line 42-50) to match.

### 1b: Add DataLinkResult and DataLinkFile

Append to `src/models/search_result.rs`:

```rust
#[derive(Debug, Clone)]
pub struct DataLinkResult {
    pub publisher_id: String,
    pub files: Vec<DataLinkFile>,
    pub resolved_at: std::time::Instant,
}

#[derive(Debug, Clone)]
pub struct DataLinkFile {
    pub url: String,
    pub semantics: String,       // "#this", "#preview", "#thumbnail", "#auxiliary"
    pub content_type: String,
    pub size: Option<u64>,
    pub description: String,
}

impl DataLinkFile {
    pub fn is_science_data(&self) -> bool { self.semantics == "#this" }
    pub fn is_preview(&self) -> bool { self.semantics == "#preview" }
    pub fn is_thumbnail(&self) -> bool { self.semantics == "#thumbnail" }
    pub fn is_auxiliary(&self) -> bool { self.semantics == "#auxiliary" }

    pub fn filename(&self) -> String {
        self.url.rsplit('/').next().unwrap_or("unknown").to_string()
    }

    pub fn size_display(&self) -> String {
        match self.size {
            Some(b) if b < 1024 => format!("{} B", b),
            Some(b) if b < 1024 * 1024 => format!("{:.1} KB", b as f64 / 1024.0),
            Some(b) if b < 1024 * 1024 * 1024 => format!("{:.1} MB", b as f64 / (1024.0 * 1024.0)),
            Some(b) => format!("{:.2} GB", b as f64 / (1024.0 * 1024.0 * 1024.0)),
            None => "unknown".to_string(),
        }
    }
}
```

### 1c: Add SavedQuery

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SavedQuery {
    pub name: String,
    pub form_state: SearchFormState,
    pub adql: String,
    pub created_at: String,
    pub last_used: Option<String>,
}
```

### 1d: Add ResultColumnInfo and ColumnFormat

```rust
#[derive(Debug, Clone)]
pub struct ResultColumnInfo {
    pub name: String,
    pub display_name: String,
    pub visible: bool,
    pub sortable: bool,
    pub is_numeric: bool,
    pub format: ColumnFormat,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ColumnFormat {
    Plain,
    Degrees5,
    MjdToDate,
    IntegrationTime,
    CalibrationLevel,
    WavelengthMeters,
    AreaDegrees,
    ArcsecPixelScale,
    IsoDate,
}
```

Add a function `fn default_columns() -> Vec<ResultColumnInfo>` that returns the full 24-column list with correct visibility/format defaults matching the spec (Section 8).

**Dependencies**: None. This is foundational for all other steps.

---

## Step 2: Complete TAP Service

**File**: `src/services/tap_service.rs`

### 2a: Add data train fetch methods

Add methods for the cascading data train queries:

```rust
/// Fetch distinct collections from CAOM2
pub async fn fetch_collections(&self) -> Result<Vec<String>, ApiError> {
    let adql = "SELECT DISTINCT Observation.collection FROM caom2.Observation AS Observation ORDER BY Observation.collection";
    let results = self.execute_query(adql, 10000).await?;
    Ok(results.rows.iter().map(|r| r.get("Observation.collection").to_string()).filter(|s| !s.is_empty()).collect())
}

/// Fetch instruments for a given collection
pub async fn fetch_instruments(&self, collection: &str) -> Result<Vec<String>, ApiError> {
    let adql = format!(
        "SELECT DISTINCT Observation.instrument_name FROM caom2.Observation AS Observation WHERE Observation.collection = '{}' ORDER BY Observation.instrument_name",
        escape_sql(collection)
    );
    let results = self.execute_query(&adql, 10000).await?;
    Ok(results.rows.iter().map(|r| r.get("Observation.instrument_name").to_string()).filter(|s| !s.is_empty()).collect())
}

/// Fetch bandpass names for a given collection + instrument
pub async fn fetch_filters(&self, collection: &str, instrument: &str) -> Result<Vec<String>, ApiError> {
    let adql = format!(
        "SELECT DISTINCT Plane.energy_bandpassName FROM caom2.Plane AS Plane JOIN caom2.Observation AS Observation ON Plane.obsID = Observation.obsID WHERE Observation.collection = '{}' AND Observation.instrument_name = '{}' ORDER BY Plane.energy_bandpassName",
        escape_sql(collection), escape_sql(instrument)
    );
    let results = self.execute_query(&adql, 10000).await?;
    Ok(results.rows.iter().map(|r| r.get("Plane.energy_bandpassName").to_string()).filter(|s| !s.is_empty()).collect())
}
```

Add a private `escape_sql` function (or import from `adql_builder`).

### 2b: Register TAPService in AppServices

**File**: `src/state.rs`

Add `pub tap: Arc<TAPService>` to `AppServices`. Initialize with `Arc::new(TAPService::new(client.clone()))` in `new()`. This gives the search page access through the shared services instead of creating its own.

**Dependencies**: Step 1 (for the `escape_sql` reuse).

---

## Step 3: NEW DataLink Service

**File**: `src/services/datalink_service.rs` (new)

### 3a: Struct definition

```rust
use crate::models::search_result::{DataLinkResult, DataLinkFile};
use crate::services::api_error::ApiError;
use reqwest::Client;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, Semaphore};

const DATALINK_URL: &str = "https://ws.cadc-ccda.hia-iha.nrc-cnrc.gc.ca/caom2ops/datalink";

pub struct DataLinkService {
    client: Client,
    cache: Mutex<HashMap<String, DataLinkResult>>,
    semaphore: Arc<Semaphore>,
}

impl DataLinkService {
    pub fn new(client: Client) -> Self {
        DataLinkService {
            client,
            cache: Mutex::new(HashMap::new()),
            semaphore: Arc::new(Semaphore::new(3)),
        }
    }

    pub async fn resolve(&self, publisher_id: &str, token: Option<&str>) -> Result<DataLinkResult, ApiError> {
        // Check cache
        {
            let cache = self.cache.lock().await;
            if let Some(cached) = cache.get(publisher_id) {
                return Ok(cached.clone());
            }
        }

        let _permit = self.semaphore.acquire().await.map_err(|e| ApiError::Network(e.to_string()))?;

        let url = format!("{}?id={}", DATALINK_URL, urlencoding::encode(publisher_id));
        let mut req = self.client.get(&url);
        if let Some(t) = token {
            req = req.bearer_auth(t);
        }
        let resp = req.timeout(std::time::Duration::from_secs(30)).send().await?;

        if !resp.status().is_success() {
            return Err(ApiError::Server { status: resp.status().as_u16(), body: resp.text().await.unwrap_or_default() });
        }

        let xml = resp.text().await.map_err(|e| ApiError::Parse(e.to_string()))?;
        let files = parse_datalink_votable(&xml)?;

        let result = DataLinkResult {
            publisher_id: publisher_id.to_string(),
            files,
            resolved_at: std::time::Instant::now(),
        };

        // Store in cache
        {
            let mut cache = self.cache.lock().await;
            cache.insert(publisher_id.to_string(), result.clone());
        }

        Ok(result)
    }
}
```

### 3b: VOTable XML parser

```rust
fn parse_datalink_votable(xml: &str) -> Result<Vec<DataLinkFile>, ApiError> {
    let doc = roxmltree::Document::parse(xml)
        .map_err(|e| ApiError::Parse(format!("VOTable parse error: {}", e)))?;

    // Find FIELD elements to determine column order
    let mut field_names: Vec<String> = Vec::new();
    for node in doc.descendants() {
        if node.tag_name().name() == "FIELD" {
            if let Some(name) = node.attribute("name") {
                field_names.push(name.to_lowercase());
            }
        }
    }

    let mut files = Vec::new();
    for tr in doc.descendants() {
        if tr.tag_name().name() != "TR" { continue; }
        let tds: Vec<String> = tr.children()
            .filter(|n| n.tag_name().name() == "TD")
            .map(|n| n.text().unwrap_or("").to_string())
            .collect();

        if tds.len() != field_names.len() { continue; }

        let get = |name: &str| -> String {
            field_names.iter().position(|f| f == name)
                .and_then(|i| tds.get(i))
                .cloned()
                .unwrap_or_default()
        };

        let url = get("access_url");
        if url.is_empty() { continue; }

        files.push(DataLinkFile {
            url,
            semantics: get("semantics"),
            content_type: get("content_type"),
            size: get("content_length").parse().ok(),
            description: get("description"),
        });
    }

    Ok(files)
}
```

### 3c: Register in mod.rs

**File**: `src/services/mod.rs` -- Add `pub mod datalink_service;` and re-export.

**Dependencies**: Step 1 (DataLinkResult/DataLinkFile models). Requires `roxmltree` (already in Cargo.toml for vospace_parser).

---

## Step 4: NEW Search Store Service

**File**: `src/services/search_store.rs` (new)

```rust
use crate::models::search_result::{RecentSearch, SavedQuery};
use directories::ProjectDirs;
use std::path::PathBuf;

const MAX_RECENT: usize = 20;

pub struct SearchStore {
    recent_path: PathBuf,
    saved_path: PathBuf,
}

impl SearchStore {
    pub fn new() -> Self {
        let base = ProjectDirs::from("net", "canfar", "Verbinal")
            .map(|d| d.data_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
        SearchStore {
            recent_path: base.join("recent_searches.json"),
            saved_path: base.join("saved_queries.json"),
        }
    }

    // --- Recent Searches ---
    pub fn load_recent(&self) -> Vec<RecentSearch> { /* read JSON, return vec */ }
    pub fn save_recent(&self, search: RecentSearch) -> Result<(), String> {
        // Dedup by ADQL, insert at front, truncate to MAX_RECENT, write JSON
    }
    pub fn clear_recent(&self) -> Result<(), String> { /* write empty array */ }

    // --- Saved Queries ---
    pub fn load_saved(&self) -> Vec<SavedQuery> { /* read JSON */ }
    pub fn save_query(&self, query: SavedQuery) -> Result<(), String> { /* append, write */ }
    pub fn remove_saved(&self, name: &str) -> Result<(), String> { /* filter out, write */ }
    pub fn rename_saved(&self, old_name: &str, new_name: &str) -> Result<(), String> { /* find + rename */ }
}
```

Follow the same patterns as `RecentLaunchService` for JSON persistence.

**File**: `src/services/mod.rs` -- Add `pub mod search_store;`

**File**: `src/state.rs` -- Add `pub search_store: SearchStore` to `AppServices` and initialize in `new()`.

**Dependencies**: Step 1 (RecentSearch, SavedQuery models).

---

## Step 5: Complete ADQL Builder

**File**: `src/helpers/adql_builder.rs`

### 5a: Add observation constraint functions

```rust
fn add_observation_clauses(state: &SearchFormState, clauses: &mut Vec<String>) {
    add_wildcard_clause(&state.observation_id, "Observation.observationID", clauses);
    add_wildcard_clause(&state.proposal_pi, "Observation.proposal_pi", clauses);
    add_wildcard_clause(&state.proposal_id, "Observation.proposal_id", clauses);
    add_wildcard_clause(&state.proposal_title, "Observation.proposal_title", clauses);
    add_wildcard_clause(&state.proposal_keywords, "Observation.proposal_keywords", clauses);

    if !state.intent.is_empty() {
        clauses.push(format!("Observation.intent = '{}'", escape_sql(&state.intent)));
    }

    if state.public_only {
        clauses.push(format!("Plane.dataRelease <= '{}'", chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S")));
    }
}

fn add_wildcard_clause(value: &str, column: &str, clauses: &mut Vec<String>) {
    if value.is_empty() { return; }
    let escaped = escape_sql(value);
    if escaped.contains('*') {
        let like_val = escaped.replace('*', "%");
        clauses.push(format!("{} LIKE '{}'", column, like_val));
    } else {
        clauses.push(format!("{} = '{}'", column, escaped));
    }
}
```

### 5b: Add temporal constraint functions

```rust
fn add_temporal_clauses(state: &SearchFormState, clauses: &mut Vec<String>) {
    if !state.obs_date_start.is_empty() {
        let mjd = date_to_mjd(&state.obs_date_start);
        clauses.push(format!("Plane.time_bounds_lower >= {}", mjd));
    }
    if !state.obs_date_end.is_empty() {
        let mjd = date_to_mjd(&state.obs_date_end);
        clauses.push(format!("Plane.time_bounds_upper <= {}", mjd));
    }
    if let Some(min) = state.integration_time_min {
        clauses.push(format!("Plane.time_exposure >= {}", min));
    }
    if let Some(max) = state.integration_time_max {
        clauses.push(format!("Plane.time_exposure <= {}", max));
    }
}

fn date_to_mjd(input: &str) -> f64 {
    if input.contains('-') {
        // Parse YYYY-MM-DD and convert to MJD
        if let Ok(date) = chrono::NaiveDate::parse_from_str(input, "%Y-%m-%d") {
            let epoch = chrono::NaiveDate::from_ymd_opt(1858, 11, 17).unwrap();
            return (date - epoch).num_days() as f64;
        }
    }
    // Already a number
    input.parse::<f64>().unwrap_or(0.0)
}
```

### 5c: Add spectral constraint functions

```rust
fn add_spectral_clauses(state: &SearchFormState, clauses: &mut Vec<String>) {
    if let Some(min) = state.wavelength_min {
        let meters = convert_to_meters(min, &state.wavelength_unit);
        clauses.push(format!("Plane.energy_bounds_lower >= {}", meters));
    }
    if let Some(max) = state.wavelength_max {
        let meters = convert_to_meters(max, &state.wavelength_unit);
        clauses.push(format!("Plane.energy_bounds_upper <= {}", meters));
    }
    // resolving power, pixel scale, etc.
}

fn convert_to_meters(value: f64, unit: &str) -> f64 {
    match unit {
        "nm" => value * 1e-9,
        "um" => value * 1e-6,
        "Angstrom" => value * 1e-10,
        _ => value, // meters
    }
}
```

### 5d: Add data train IN-clause generation

```rust
fn add_data_train_clauses(state: &SearchFormState, clauses: &mut Vec<String>) {
    add_multi_select_clause(&state.collection, "Observation.collection", clauses);
    add_multi_select_clause(&state.instrument, "Observation.instrument_name", clauses);
    add_multi_select_clause(&state.band, "Plane.energy_emBand", clauses);
    add_multi_select_clause(&state.filter_name, "Plane.energy_bandpassName", clauses);
    add_multi_select_clause(&state.calibration_level, "Plane.calibrationLevel", clauses);
    add_multi_select_clause(&state.data_product_type, "Plane.dataProductType", clauses);
    add_multi_select_clause(&state.obs_type, "Observation.type", clauses);
}

fn add_multi_select_clause(value: &str, column: &str, clauses: &mut Vec<String>) {
    if value.is_empty() { return; }
    let parts: Vec<&str> = value.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
    if parts.len() == 1 {
        clauses.push(format!("{} = '{}'", column, escape_sql(parts[0])));
    } else if parts.len() > 1 {
        let in_list = parts.iter().map(|p| format!("'{}'", escape_sql(p))).collect::<Vec<_>>().join(", ");
        clauses.push(format!("{} IN ({})", column, in_list));
    }
}
```

### 5e: Add range syntax parser

```rust
fn parse_range(input: &str, column: &str) -> Option<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() { return None; }
    if trimmed.contains("..") {
        let parts: Vec<&str> = trimmed.split("..").collect();
        if parts.len() == 2 {
            return Some(format!("{} BETWEEN {} AND {}", column, parts[0].trim(), parts[1].trim()));
        }
    }
    if trimmed.starts_with(">=") {
        return Some(format!("{} >= {}", column, trimmed[2..].trim()));
    }
    if trimmed.starts_with('>') {
        return Some(format!("{} > {}", column, trimmed[1..].trim()));
    }
    if trimmed.starts_with("<=") {
        return Some(format!("{} <= {}", column, trimmed[2..].trim()));
    }
    if trimmed.starts_with('<') {
        return Some(format!("{} < {}", column, trimmed[1..].trim()));
    }
    Some(format!("{} = {}", column, trimmed))
}
```

### 5f: Update build() function

Update the `build()` function (line 36) to call all the new clause functions:

```rust
pub fn build(state: &SearchFormState) -> String {
    let mut clauses = vec![QUALITY_FILTER.to_string()];
    add_spatial_clauses(state, &mut clauses);
    add_observation_clauses(state, &mut clauses);
    add_temporal_clauses(state, &mut clauses);
    add_spectral_clauses(state, &mut clauses);
    add_data_train_clauses(state, &mut clauses);
    let where_str = clauses.join("\nAND ");
    format!("SELECT TOP {}\n{}\nFROM {}\nWHERE {}", state.max_records, SELECT_COLUMNS, FROM_CLAUSE, where_str)
}
```

**Dependencies**: Step 1 (expanded SearchFormState).

---

## Step 6: NEW Cell Formatter

**File**: `src/helpers/cell_formatter.rs` (new)

```rust
use crate::models::search_result::ColumnFormat;

pub fn format_cell(value: &str, format: ColumnFormat) -> String {
    if value.is_empty() { return String::new(); }
    match format {
        ColumnFormat::Plain => value.to_string(),
        ColumnFormat::Degrees5 => {
            value.parse::<f64>().map(|v| format!("{:.5}", v)).unwrap_or_else(|_| value.to_string())
        }
        ColumnFormat::MjdToDate => mjd_to_date(value),
        ColumnFormat::IntegrationTime => format_integration_time(value),
        ColumnFormat::CalibrationLevel => match value.trim() {
            "0" => "Raw".to_string(),
            "1" => "Cal".to_string(),
            "2" => "Product".to_string(),
            "3" => "Analysis".to_string(),
            _ => value.to_string(),
        },
        ColumnFormat::WavelengthMeters => format_wavelength(value),
        ColumnFormat::AreaDegrees => {
            value.parse::<f64>().map(|v| format!("{:.4} deg^2", v)).unwrap_or_else(|_| value.to_string())
        }
        ColumnFormat::ArcsecPixelScale => {
            value.parse::<f64>().map(|v| format!("{:.3}\"", v * 3600.0)).unwrap_or_else(|_| value.to_string())
        }
        ColumnFormat::IsoDate => value.to_string(), // Already ISO
    }
}

fn mjd_to_date(value: &str) -> String {
    let mjd: f64 = match value.parse() {
        Ok(v) => v,
        Err(_) => return value.to_string(),
    };
    let epoch = chrono::NaiveDate::from_ymd_opt(1858, 11, 17).unwrap();
    let days = mjd.floor() as i64;
    let frac = mjd - days as f64;
    let date = epoch + chrono::Duration::days(days);
    let secs = (frac * 86400.0) as u32;
    let time = chrono::NaiveTime::from_num_seconds_from_midnight_opt(secs, 0).unwrap_or_default();
    let dt = chrono::NaiveDateTime::new(date, time);
    dt.format("%Y-%m-%d %H:%M").to_string()
}

fn format_integration_time(value: &str) -> String {
    let secs: f64 = match value.parse() {
        Ok(v) => v,
        Err(_) => return value.to_string(),
    };
    if secs < 60.0 { format!("{:.1}s", secs) }
    else if secs < 3600.0 { format!("{:.1}min", secs / 60.0) }
    else { format!("{:.1}h", secs / 3600.0) }
}

fn format_wavelength(value: &str) -> String {
    let v: f64 = match value.parse() {
        Ok(v) => v,
        Err(_) => return value.to_string(),
    };
    if v < 1e-6 { format!("{:.3e} m", v) }
    else { format!("{:.6} m", v) }
}
```

**File**: `src/helpers/mod.rs` -- Add `pub mod cell_formatter;`

**Dependencies**: None (uses chrono which is already a dependency).

---

## Step 7: UI -- Search Form Rewrite

**File**: `src/ui/search_page.rs` -- major rewrite

### 7a: Add Notebook tabs (Form / ADQL)

Replace the current single-form layout with a `gtk::Notebook`:

```rust
let form_notebook = gtk::Notebook::new();
// Tab 0: Form (visual constraint builder)
// Tab 1: ADQL (raw editor)
form_notebook.append_page(&form_scroll, Some(&gtk::Label::new(Some("Form"))));
form_notebook.append_page(&adql_editor_box, Some(&gtk::Label::new(Some("ADQL"))));
```

### 7b: Build 4 constraint groups

Create `adw::PreferencesGroup` for each:

1. **Observation** group: `observation_id`, `proposal_pi`, `proposal_id`, `proposal_title`, `proposal_keywords` (all `adw::EntryRow`), `intent` (`adw::ComboRow` with Any/science/calibration), `public_only` (`gtk::Switch` in `adw::ActionRow`).

2. **Spatial** group: `target_entry`, `resolver_combo` (`adw::ComboRow` ALL/SIMBAD/NED/VIZIER), `coord_label` (read-only), `radius_spin`, `pixel_scale_spin`, `resolve_btn`.

3. **Temporal** group: `date_preset` (`adw::ComboRow`), `obs_date_start`/`obs_date_end` (`adw::EntryRow`), `integration_time_min`/`max` (`adw::SpinRow`), `time_span_min`/`max` (`adw::SpinRow`).

4. **Spectral** group: `wavelength_min`/`max` (`adw::SpinRow`), `wavelength_unit` (`adw::ComboRow` nm/um/Angstrom/m), `resolving_power_min`/`max`, `bandpass_width_min`/`max`, `rest_frame_energy_min`/`max`.

### 7c: Build data train row

Below the constraint groups, add a horizontal `gtk::Box` containing 7 scrollable `gtk::ListBox` widgets (each max-height 200px):

```rust
let train_box = gtk::Box::new(gtk::Orientation::Horizontal, 4);
// Band | Collection | Instrument | Filter | Cal Level | Data Type | Obs Type
// Each: gtk::Frame with title label + ScrolledWindow + ListBox with multi-select
```

Band, Cal Level, Data Type, Obs Type get static values. Collection is fetched on init via `tap_service.fetch_collections()`. Instrument and Filter cascade from selection changes.

### 7d: Wire date presets

When `date_preset` ComboRow changes:
- "Last 24 hours": set start = now - 1 day, end = ""
- "Last Week": set start = now - 7 days
- etc.

### 7e: Build ADQL editor tab

```rust
let adql_view = gtk::TextView::new();
adql_view.set_monospace(true);
adql_view.set_wrap_mode(gtk::WrapMode::Word);
// Buttons: Execute, Copy, Clear
```

Wire tab switch: when switching to ADQL tab, call `adql_builder::build(&self.build_form_state())` and populate the text buffer.

### 7f: Update build_form_state()

Update to read all 30+ fields from the new form widgets.

**Dependencies**: Steps 1, 2, 5.

---

## Step 8: UI -- Results Panel

**File**: `src/ui/search_page.rs` (results section)

### 8a: Add results toolbar

Above the results list, add a toolbar with:
- Column picker button (opens a `gtk::Popover` with checkboxes for each column)
- Sort dropdown or clickable column headers
- Page size dropdown (25/50/100/250/500)
- Export button (CSV/TSV)
- Filter `gtk::SearchEntry`

### 8b: Implement pagination

```rust
struct PaginationState {
    page_size: usize,      // Default 100
    current_page: usize,   // 0-indexed
    total_rows: usize,
}

fn paginated_rows(&self) -> &[SearchResultRow] {
    let start = self.pagination.current_page * self.pagination.page_size;
    let end = (start + self.pagination.page_size).min(self.filtered_rows.len());
    &self.filtered_rows[start..end]
}
```

Navigation: Previous/Next buttons + "Page X of Y" label.

### 8c: Implement client-side filtering

`gtk::SearchEntry::connect_search_changed` -> filter `results.rows` by checking if any visible column value contains the filter text (case-insensitive). Store filtered rows in `filtered_rows: Rc<RefCell<Vec<SearchResultRow>>>`.

### 8d: Implement sorting

Track `sort_column: Option<String>` and `sort_ascending: bool`. On column header click: toggle direction or clear sort. Sort `filtered_rows` in place using `ResultColumnInfo.is_numeric` to decide numeric vs lexicographic.

### 8e: Implement CSV/TSV export

```rust
async fn export_results(&self, format: &str) {
    let dialog = gtk::FileDialog::builder().title("Export Results").initial_name(&format!("results.{}", format)).build();
    // On file chosen: write header line + data lines with appropriate separator
}
```

### 8f: Format cell values using cell_formatter

In `display_results()`, use `cell_formatter::format_cell(value, column_info.format)` for each cell.

**Dependencies**: Steps 1, 6.

---

## Step 9: UI -- Sidebar

**File**: `src/ui/search_page.rs` (add sidebar panel)

### 9a: Add sidebar to the left of the main area

```rust
// Outermost layout becomes: Sidebar | Separator | MainArea
let outer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
let sidebar = gtk::Box::new(gtk::Orientation::Vertical, 8);
sidebar.set_size_request(220, -1);
// ...
outer.append(&sidebar);
outer.append(&gtk::Separator::new(gtk::Orientation::Vertical));
outer.append(&main_area); // existing form + results
```

### 9b: Recent Searches section

```rust
let recent_title = gtk::Label::new(Some("Recent Searches"));
recent_title.add_css_class("title-4");
let recent_list = gtk::ListBox::new();
recent_list.add_css_class("boxed-list");
// Populate from search_store.load_recent()
// Each row: adw::ActionRow with summary as title, "N results - relative_time" as subtitle
// Click -> re-execute saved ADQL
```

### 9c: Saved Queries section

```rust
let saved_title = gtk::Label::new(Some("Saved Queries"));
let saved_list = gtk::ListBox::new();
// Populate from search_store.load_saved()
// Each row: adw::ActionRow with name as title, result info as subtitle
// Click -> restore form state + optionally re-execute
// Long-press / right-click -> Rename, Delete, Execute context menu
```

### 9d: "Save Query" button in results toolbar

After a search completes, enable a "Save Query" button. On click, show `adw::MessageDialog` with name entry. Save via `search_store.save_query()`.

**Dependencies**: Step 4.

---

## Step 10: UI -- Download Flow

**File**: `src/ui/search_page.rs` (add download logic)

### 10a: Add download button to result rows

In `display_results()`, add a download button as suffix on each `adw::ActionRow`:

```rust
let download_btn = gtk::Button::from_icon_name("folder-download-symbolic");
download_btn.set_tooltip_text(Some("Download"));
download_btn.set_valign(gtk::Align::Center);
download_btn.add_css_class("flat");
row.add_suffix(&download_btn);
```

### 10b: DataLink resolution on download click

```rust
// On download button click:
// 1. Get publisherID from the row data
// 2. Show spinner
// 3. Call datalink_service.resolve(publisher_id, token)
// 4. Show file picker dialog
```

### 10c: File picker dialog

Create a new `adw::Window` or `adw::MessageDialog` listing all `#this` files with checkboxes, sizes, and a directory chooser:

```rust
fn show_download_dialog(parent: &impl IsA<gtk::Widget>, result: &DataLinkResult, target_name: &str) {
    // List #this files with checkboxes
    // Toggles for #preview and #auxiliary
    // Directory picker defaulting to ~/Downloads/verbinal/
    // Download button
}
```

### 10d: Download execution with progress

```rust
async fn download_files(files: &[DataLinkFile], target_dir: &Path, token: Option<&str>) {
    let semaphore = Arc::new(Semaphore::new(3));
    for file in files {
        let permit = semaphore.acquire().await;
        // GET file.url, stream to target_dir/filename
        // Track progress via content-length header
    }
}
```

### 10e: Save to ObservationStore

After download completes, construct a `DownloadedObservation` and call `observation_store.save()`.

**Dependencies**: Steps 1, 3 (DataLink service). Step 7 in Research module (ObservationStore, which is a Step 3 cross-dependency).

---

## Step 11: Integration

**File**: `src/ui/main_window.rs`

### 11a: Wire SearchPage with AppServices

Change `SearchPage::new()` to accept `Arc<AppServices>` instead of just `reqwest::Client`:

```rust
pub fn new(services: Arc<AppServices>) -> Rc<Self> {
    let tap_service = services.tap.clone();
    // Access search_store, datalink_service via services
}
```

### 11b: Register new services in state.rs

**File**: `src/state.rs`

Add:
```rust
pub tap: Arc<TAPService>,
pub datalink: Arc<DataLinkService>,
pub search_store: SearchStore,
```

Initialize in `new()`:
```rust
tap: Arc::new(TAPService::new(client.clone())),
datalink: Arc::new(DataLinkService::new(client.clone())),
search_store: SearchStore::new(),
```

### 11c: Add keyboard shortcuts

**File**: `src/ui/search_page.rs`

Add `gtk::EventControllerKey` to the search page widget:
- `Ctrl+Enter` -> execute search
- `Ctrl+S` -> save query
- `Ctrl+E` -> toggle ADQL editor tab
- `Enter` in target field -> resolve target
- `Escape` -> clear results filter

### 11d: Wire FITS Viewer integration

When a `#this` file with `application/fits` content type is downloaded, offer to open in FITS Viewer. Add callback:

```rust
on_open_fits: Rc<RefCell<Option<Box<dyn Fn(PathBuf)>>>>
```

Main window sets this to call `fits_viewer.load_from_path()` and switch to the fits tab.

**Dependencies**: Steps 2-10 all complete.

---

## Implementation Order

| Step | Description | File(s) | Effort | Dependencies |
|------|-------------|---------|--------|-------------|
| 1 | Expand models | `src/models/search_result.rs` | 1 hr | None |
| 2 | Complete TAP Service | `src/services/tap_service.rs`, `src/state.rs` | 1 hr | Step 1 |
| 3 | DataLink Service | `src/services/datalink_service.rs` (new) | 2 hr | Step 1 |
| 4 | Search Store Service | `src/services/search_store.rs` (new) | 1 hr | Step 1 |
| 5 | Complete ADQL Builder | `src/helpers/adql_builder.rs` | 2 hr | Step 1 |
| 6 | Cell Formatter | `src/helpers/cell_formatter.rs` (new) | 1 hr | None |
| 7 | UI - Search Form rewrite | `src/ui/search_page.rs` | 4 hr | Steps 1, 2, 5 |
| 8 | UI - Results Panel | `src/ui/search_page.rs` | 3 hr | Steps 1, 6 |
| 9 | UI - Sidebar | `src/ui/search_page.rs` | 2 hr | Step 4 |
| 10 | UI - Download Flow | `src/ui/search_page.rs` | 3 hr | Steps 1, 3 |
| 11 | Integration | `src/state.rs`, `src/ui/main_window.rs` | 1 hr | Steps 2-10 |

**Total estimate**: ~21 hours of implementation work.

Steps 1, 5, 6 can be done in parallel. Steps 2, 3, 4 can be done in parallel after Step 1. Steps 7, 8, 9, 10 depend on the service/model layer being complete. Step 11 is final wiring.
