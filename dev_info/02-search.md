# Search Module Specification

> Module status: **Partial stub** -- basic form + TAP query + resolver exist; full spec below describes the complete Windows-parity implementation to build.
> Covers: CADC archive search, ADQL query building, target resolution, data train filtering, DataLink file resolution, result display, recent/saved queries

---

## 1. Page Layout

The Search page is a three-panel horizontal layout:

```
+------------------+------+-------------------------------------------+
| Sidebar (220px)  | Sep  | Main Area                                 |
|                  |      |                                           |
| Recent Searches  |      | +-------------------+---+--------------+ |
| (max 20)         |      | | Search Form       |Sep| Results      | |
|                  |      | | (380px fixed)      |   | (hexpand)   | |
| Saved Queries    |      | |                   |   |              | |
| (named)          |      | |                   |   |              | |
|                  |      | +-------------------+---+--------------+ |
+------------------+------+-------------------------------------------+
```

- **Sidebar**: `gtk::Box` vertical, 220px width, scrollable. Contains Recent Searches list (max 20) on top, Saved Queries list below.
- **Main Area**: `gtk::Box` horizontal. Contains the search form on the left (380px fixed, scrollable), a vertical separator, and the results panel (hexpand) on the right.
- The entire page is `vexpand = true, hexpand = true`.

### Tab System

The search form area has a `gtk::Notebook` with two tabs:
1. **Form** -- the visual constraint builder
2. **ADQL** -- raw ADQL editor (gtk::TextView, monospace, editable)

When switching from Form to ADQL, the current form state is serialized to ADQL and pre-filled. When switching from ADQL to Form, the raw ADQL is preserved but the form is not reverse-parsed (ADQL tab takes priority if edited).

---

## 2. Search Form

The form tab is a vertical scrollable area containing four constraint groups as `adw::PreferencesGroup` sections, followed by the Data Train, then action buttons.

### 2.1 Observation Constraints

Group title: **"Observation"**

| Field | Widget | ADQL Column | Notes |
|-------|--------|-------------|-------|
| Observation ID | `adw::EntryRow` | `Observation.observationID` | Wildcards: `*` converted to `%` for SQL LIKE |
| Proposal PI | `adw::EntryRow` | `Observation.proposal_pi` | Wildcards supported |
| Proposal ID | `adw::EntryRow` | `Observation.proposal_id` | Wildcards supported |
| Proposal Title | `adw::EntryRow` | `Observation.proposal_title` | Wildcards supported |
| Proposal Keywords | `adw::EntryRow` | `Observation.proposal_keywords` | Wildcards supported |
| Intent | `adw::ComboRow` | `Observation.intent` | Options: `["Any", "science", "calibration"]` |
| Public Only | `gtk::Switch` | `Plane.dataRelease` | When ON: `Plane.dataRelease <= '{now_utc}'` |
| Data Release | `adw::EntryRow` | `Plane.dataRelease` | Optional date constraint, format YYYY-MM-DD |

**Wildcard handling**: If the user enters `*`, replace with `%` and use `LIKE` instead of `=`. If no wildcard, use `=` for exact match. Example: `NGC*` becomes `Observation.observationID LIKE 'NGC%'`.

### 2.2 Spatial Constraints

Group title: **"Spatial"**

| Field | Widget | Notes |
|-------|--------|-------|
| Target Name | `adw::EntryRow` | Free text, resolved via name resolver service |
| Resolver Service | `adw::ComboRow` | Options: `["ALL", "SIMBAD", "NED", "VIZIER"]` |
| Resolved RA | `gtk::Label` (read-only) | Populated after resolve, degrees, 5 decimal places |
| Resolved Dec | `gtk::Label` (read-only) | Populated after resolve, degrees, 5 decimal places |
| Search Radius | `adw::SpinRow` | Range: 0.001 to 10.0 degrees, step 0.01, default 0.05, 3 decimal places |
| Pixel Scale | `adw::SpinRow` | Range: 0.0 to 100.0 arcsec, step 0.01, optional |
| Resolve Button | `gtk::Button` | "Resolve Target", suggested-action CSS class |

**Spatial ADQL clause**:
```sql
INTERSECTS(Plane.position_bounds, CIRCLE('ICRS', {ra}, {dec}, {radius})) = 1
```

**Pixel Scale clause** (if set):
```sql
Plane.position_sampleSize <= {value}
```

### 2.3 Temporal Constraints

Group title: **"Temporal"**

| Field | Widget | Notes |
|-------|--------|-------|
| Date Preset | `adw::ComboRow` | Options: `["Custom", "Last 24 hours", "Last Week", "Last Month", "Last Year"]` |
| Observation Start | `adw::EntryRow` | Date string YYYY-MM-DD or MJD number |
| Observation End | `adw::EntryRow` | Date string YYYY-MM-DD or MJD number |
| Integration Time Min | `adw::SpinRow` | Seconds, range 0 to 1e7 |
| Integration Time Max | `adw::SpinRow` | Seconds, range 0 to 1e7 |
| Time Span Min | `adw::SpinRow` | Days, range 0 to 1e5 |
| Time Span Max | `adw::SpinRow` | Days, range 0 to 1e5 |

**Date presets**: When a preset is selected, auto-fill start/end dates:
- Last 24 hours: start = now - 1 day
- Last Week: start = now - 7 days
- Last Month: start = now - 30 days
- Last Year: start = now - 365 days
- End is always "now"

**MJD conversion**:
- Gregorian to MJD: `MJD = JD - 2400000.5` where `JD = 367*Y - INT(7*(Y+INT((M+9)/12))/4) + INT(275*M/9) + D + 1721013.5`
- If input looks like a date string (contains `-`), convert to MJD for the ADQL clause
- If input is already a number, use directly as MJD

**Temporal ADQL clauses**:
```sql
Plane.time_bounds_lower >= {start_mjd}
Plane.time_bounds_upper <= {end_mjd}
Plane.time_exposure >= {min_seconds}
Plane.time_exposure <= {max_seconds}
```

**Range syntax in entry fields**: Support `"2020..2021"` for ranges (generates `BETWEEN`), `"> 2019"` for open-ended (generates `>=`), `"< 2023"` (generates `<=`).

### 2.4 Spectral Constraints

Group title: **"Spectral"**

| Field | Widget | Notes |
|-------|--------|-------|
| Wavelength Min | `adw::SpinRow` | Default unit: meters, but UI shows converted value |
| Wavelength Max | `adw::SpinRow` | Default unit: meters |
| Wavelength Unit | `adw::ComboRow` | Options: `["nm", "um", "Angstrom", "m"]` |
| Spectral Coverage | `adw::SpinRow` | Dimensionless ratio |
| Spectral Sampling | `adw::SpinRow` | Dimensionless ratio |
| Resolving Power | `adw::SpinRow` | Dimensionless |
| Bandpass Width Min | `adw::SpinRow` | Same unit as wavelength |
| Bandpass Width Max | `adw::SpinRow` | Same unit as wavelength |
| Rest-frame Energy Min | `adw::SpinRow` | eV |
| Rest-frame Energy Max | `adw::SpinRow` | eV |

**Unit conversion** (all stored internally as meters in ADQL):
- nm to m: multiply by 1e-9
- um (micron) to m: multiply by 1e-6
- Angstrom to m: multiply by 1e-10
- m: as-is

**Spectral ADQL clauses**:
```sql
Plane.energy_bounds_lower >= {min_m}
Plane.energy_bounds_upper <= {max_m}
Plane.energy_resolvingPower >= {min_rp}
Plane.energy_resolvingPower <= {max_rp}
```

---

## 3. Data Train

The data train is a row of 7 cascading filter lists displayed below the constraint groups. Each list is a `gtk::ListBox` inside a `gtk::ScrolledWindow` (max-height 200px), arranged horizontally.

### Filter Lists (left to right)

| # | Label | ADQL Column | Source |
|---|-------|-------------|--------|
| 1 | Band | `Plane.energy_emBand` | Known values: `Radio, Millimeter, Infrared, Optical, UV, EUV, X-ray, Gamma-ray` |
| 2 | Collection | `Observation.collection` | Dynamic: `SELECT DISTINCT Observation.collection FROM caom2.Observation ORDER BY Observation.collection` |
| 3 | Instrument | `Observation.instrument_name` | Dynamic: filtered by selected collection |
| 4 | Filter | `Plane.energy_bandpassName` | Dynamic: filtered by selected collection + instrument |
| 5 | Cal. Level | `Plane.calibrationLevel` | Known values: `0` (Raw), `1` (Calibrated), `2` (Product), `3` (Analysis) |
| 6 | Data Type | `Plane.dataProductType` | Known values: `image, spectrum, timeseries, visibility, measurements, catalog, cube` |
| 7 | Obs. Type | `Observation.type` | Known values: `OBJECT, FLAT, BIAS, DARK, CAL, FOCUS` |

### Cascading Behavior

1. **Band** is always fully populated with known values
2. **Collection** is always loaded from the archive on page init (one-time query, cacheable)
3. When **Collection** selection changes: query instruments for that collection, populate Instrument list
4. When **Instrument** selection changes: query filters for that collection+instrument, populate Filter list
5. Cal. Level, Data Type, and Obs. Type are always fully populated with known values

### Collection Query

```sql
SELECT DISTINCT Observation.collection
FROM caom2.Observation AS Observation
ORDER BY Observation.collection
```

### Instrument Query (cascaded from collection)

```sql
SELECT DISTINCT Observation.instrument_name
FROM caom2.Observation AS Observation
WHERE Observation.collection = '{collection}'
ORDER BY Observation.instrument_name
```

### Filter/Bandpass Query (cascaded from collection + instrument)

```sql
SELECT DISTINCT Plane.energy_bandpassName
FROM caom2.Plane AS Plane
JOIN caom2.Observation AS Observation ON Plane.obsID = Observation.obsID
WHERE Observation.collection = '{collection}'
AND Observation.instrument_name = '{instrument}'
ORDER BY Plane.energy_bandpassName
```

### Multi-Select

Each data train list supports multi-selection. Multiple selected values in a single list produce an `IN (...)` clause:
```sql
Observation.collection IN ('JWST', 'HST', 'CFHT')
```

Single selection produces `= '{value}'`.

---

## 4. ADQL Builder

### SELECT Columns

The builder produces a SELECT clause with the following columns:

```sql
SELECT TOP {max_records}
    Observation.observationID,
    Observation.collection,
    Plane.productID,
    COORD1(CENTROID(Plane.position_bounds)) AS "RA (J2000.0)",
    COORD2(CENTROID(Plane.position_bounds)) AS "Dec. (J2000.0)",
    Observation.target_name AS "Target Name",
    Plane.time_bounds_lower AS "Start Date",
    Plane.time_exposure AS "Int. Time",
    Observation.instrument_name AS "Instrument",
    Plane.energy_bandpassName AS "Filter",
    Plane.calibrationLevel AS "Cal. Lev.",
    Observation.type AS "Obs. Type",
    Observation.proposal_id AS "Proposal ID",
    Observation.proposal_pi AS "PI Name",
    Plane.dataRelease AS "Data Release",
    Plane.energy_bounds_lower AS "Min. Wavelength",
    Plane.energy_bounds_upper AS "Max. Wavelength",
    AREA(Plane.position_bounds) AS "Field of View",
    Plane.position_sampleSize AS "Pixel Scale",
    Plane.energy_resolvingPower AS "Resolving Power",
    Plane.dataProductType AS "Data Type",
    Observation.intent AS "Intent",
    Plane.energy_emBand AS "Band",
    Plane.publisherID
```

### FROM Clause

```sql
FROM caom2.Plane AS Plane
JOIN caom2.Observation AS Observation ON Plane.obsID = Observation.obsID
```

### WHERE Clause Construction

Always includes the quality filter:
```sql
( Plane.quality_flag IS NULL OR Plane.quality_flag != 'junk' )
```

Additional clauses are ANDed together based on non-empty form fields.

### SQL Injection Prevention

All string values are escaped by replacing `'` with `''` (single-quote doubling). The `escape_sql()` function handles this.

### Range Syntax Parsing

For numeric entry fields, the builder supports:
- `"2020..2021"` -> `column BETWEEN 2020 AND 2021`
- `"> 100"` -> `column > 100`
- `">= 100"` -> `column >= 100`
- `"< 50"` -> `column < 50`
- `"<= 50"` -> `column <= 50`
- `"100"` -> `column = 100`

### Max Records

Applied as `SELECT TOP {max_records}` in the ADQL. Default: 1000. Range: 10 to 50000.

---

## 5. TAP Service

### Execute Query

- **Endpoint**: `POST https://ws.cadc-ccda.hia-iha.nrc-cnrc.gc.ca/argus/sync`
- **Auth**: None required (public TAP service)
- **Content-Type**: `application/x-www-form-urlencoded`
- **Timeout**: 60 seconds
- **Form parameters**:

| Parameter | Value |
|-----------|-------|
| `LANG` | `ADQL` |
| `FORMAT` | `csv` |
| `MAXREC` | `{max_records}` (string) |
| `QUERY` | `{adql_query}` (string) |

- **Response**: CSV text (first line = column headers, subsequent lines = data rows)
- **Error**: Non-200 status -> `ApiError::Server { status, body }`

### CSV Parsing

The `parse_csv()` function handles:
1. Split into lines, skip empty lines
2. First line = column names (split by comma)
3. Subsequent lines = data rows
4. Quoted fields: `"hello, world"` -> `hello, world` (handles commas inside double quotes)
5. Rows with mismatched column count are skipped
6. Returns `SearchResults { columns: Vec<String>, rows: Vec<SearchResultRow>, query: Option<String> }`

Each `SearchResultRow` is a `HashMap<String, String>` mapping column name to value.

---

## 6. Target Resolver

### Endpoint

```
GET https://ws.cadc-ccda.hia-iha.nrc-cnrc.gc.ca/cadc-target-resolver/find?target={name}&service={service}&format=ascii&detail=max&cached=true
```

- **Auth**: None
- **Timeout**: 15 seconds
- **URL encoding**: Target name is URL-encoded via `urlencoding::encode()`
- **Service values**: `ALL`, `SIMBAD`, `NED`, `VIZIER` (case-sensitive)

### Response Format (ASCII)

```
ra=10.684708
dec=41.26875
coordsys=ICRS
oType=AGN
service=Simbad(simbad.cds.unistra.fr)
```

### Parsing

The `parse_resolver_response()` function:
1. Iterates lines, splits on `=`
2. Extracts `ra`, `dec` (f64), `coordsys`, `oType`/`otype`, `service` (all String)
3. Returns `None` if either `ra` or `dec` is missing

### Resolve Flow

1. User enters target name and clicks "Resolve Target"
2. Show "Resolving '{name}'..." in status
3. Call resolver endpoint
4. On success: display "RA: {ra:.5}  Dec: {dec:.5}  ({service})" in coord label
5. On failure: show error in status label
6. Resolved coordinates are stored in form state for the subsequent search

### Auto-Resolve on Search

If the user clicks Search with a target name but no resolved coordinates:
1. Auto-resolve the target first
2. If resolve fails, abort the search with an error
3. If resolve succeeds, continue with the resolved coordinates

---

## 7. DataLink Service

### Endpoint

```
GET https://ws.cadc-ccda.hia-iha.nrc-cnrc.gc.ca/caom2ops/datalink?id={publisher_id}
```

- **Auth**: None (public files) or `Authorization: Bearer {token}` (proprietary files)
- **Response**: VOTable XML document

### VOTable Response Structure

The DataLink response is a VOTable XML containing a `<TABLE>` with rows. Each row has fields:

| Field | Type | Description |
|-------|------|-------------|
| `ID` | String | The publisher ID that was queried |
| `access_url` | String | Direct URL to the file |
| `service_def` | String | Service definition (usually empty for direct links) |
| `error_message` | String | Error text if link resolution failed |
| `semantics` | String | Link type: `#this`, `#preview`, `#thumbnail`, `#auxiliary` |
| `description` | String | Human-readable description |
| `content_type` | String | MIME type: `application/fits`, `image/jpeg`, `image/png`, `text/plain` |
| `content_length` | Long | File size in bytes |

### Semantic Types

| Semantics | Meaning | Use |
|-----------|---------|-----|
| `#this` | Science data file | FITS downloads (the actual observation data) |
| `#preview` | Preview image | JPG/PNG for visual display in results |
| `#thumbnail` | Thumbnail image | Small JPG for result list rows (typically ~4KB) |
| `#auxiliary` | Auxiliary/support file | Guide star data, association files, etc. |

### Real-World DataLink Response Example

For a single JWST observation, a typical response contains:
- 5 `#this` links (various FITS products: cal, i2d, crf, rate, rateints) -- each 80-170 MB
- 5 `#preview` links (corresponding JPG previews) -- each ~1 MB
- 5 `#thumbnail` links (corresponding small JPGs) -- each ~4 KB
- Multiple `#auxiliary` links (guide star FITS, association JSON/CSV)

### Implementation Requirements

1. **Caching**: Cache DataLink results per `publisherID` in an in-memory `HashMap`. Results do not change frequently.
2. **Concurrency**: Max 3 concurrent DataLink resolution requests (use a `tokio::sync::Semaphore`).
3. **Thumbnail downloads**: For result rows with thumbnails, download the thumbnail JPGs for inline display. Also limit to max 3 concurrent image downloads.
4. **Lazy loading**: Only resolve DataLink when the user expands/selects a result row, not for all results at once.

### VOTable XML Parsing

Parse using `roxmltree`:
1. Find `<TABLE>` element
2. Read `<FIELD>` elements to get column names and order
3. Iterate `<TR>` elements inside `<DATA><TABLEDATA>`
4. For each `<TR>`, read `<TD>` elements in field order
5. Build `DataLinkFile` structs from the parsed values

---

## 8. Results Panel

### Layout

```
+-------------------------------------------+
| Results Header                            |
| "Results"  (title-4)    "N observations"  |
+-------------------------------------------+
| [Col Picker] [Sort] [Page Size] [Export]  |
+-------------------------------------------+
| Results List (scrollable, vexpand)        |
|                                           |
| Row 1: [thumb] Target | Collection | ...  |
| Row 2: [thumb] Target | Collection | ...  |
| ...                                       |
+-------------------------------------------+
| Pagination: [<] Page 1 of N [>]          |
+-------------------------------------------+
```

### Dynamic Columns

Not all columns need to be shown by default. The column picker (button in toolbar) opens a popover with checkboxes for each available column. Default visible columns:

1. Target Name
2. Collection
3. Instrument
4. Filter
5. RA (J2000.0)
6. Dec. (J2000.0)
7. Start Date
8. Int. Time
9. Cal. Lev.
10. Data Type

Columns hidden by default: Observation ID, Product ID, Proposal ID, PI Name, Data Release, Min/Max Wavelength, Field of View, Pixel Scale, Resolving Power, Obs. Type, Intent, Band, publisherID.

### In-Memory Filtering and Sorting

All search results are kept in memory (`Vec<SearchResultRow>`). The UI provides:

**Client-side filter**: A `gtk::SearchEntry` at the top of results that filters rows by checking if any visible column value contains the filter text (case-insensitive).

**Sorting**: Click a column header to sort. First click = ascending, second = descending, third = no sort. Sorting is done in-memory on the full result set. Numeric columns (RA, Dec, Int. Time, wavelengths, etc.) sort numerically; string columns sort lexicographically.

### Pagination

| Page Size Options | Default |
|-------------------|---------|
| 25, 50, 100, 250, 500 | 100 |

- Page size selector: `adw::ComboRow` or `gtk::DropDown`
- Navigation: Previous/Next buttons + "Page {current} of {total}" label
- Total pages = ceil(filtered_row_count / page_size)

### Result Row Display

Each row in the `gtk::ListBox` is an `adw::ActionRow` with:
- **Prefix**: Thumbnail image (if DataLink has been resolved and a `#thumbnail` URL exists), otherwise a star icon (`starred-symbolic`)
- **Title**: Target Name (or Observation ID if target is empty)
- **Subtitle**: `"{collection} | {instrument} | {band} | {data_type}"` (non-empty values joined by `|`)
- **Suffix**: Detail label `"RA: {ra}  Dec: {dec}  Cal: {cal_level}"`

### Cell Formatting

| Column | Format |
|--------|--------|
| RA (J2000.0) | 5 decimal places (e.g., `10.68471`) |
| Dec. (J2000.0) | 5 decimal places (e.g., `41.26875`) |
| Start Date (MJD) | Convert to ISO date: `YYYY-MM-DD HH:MM` (MJD to Gregorian) |
| Int. Time | Auto-units: `< 60s` show seconds with 1 decimal; `60-3600s` show minutes; `> 3600s` show hours |
| Cal. Lev. | Map: `0` = "Raw", `1` = "Cal", `2` = "Product", `3` = "Analysis" |
| Wavelength values | Scientific notation if very small (< 1e-6), otherwise 6 significant digits |
| Field of View | Degrees with 4 decimal places, append " deg^2" |
| Pixel Scale | Arcseconds with 3 decimal places, append `"` |
| Data Release | ISO date format |

### MJD to Gregorian Conversion

```
JD = MJD + 2400000.5
Then standard JD-to-calendar algorithm:
  L = JD + 68569
  N = 4 * L / 146097
  L = L - (146097 * N + 3) / 4
  I = 4000 * (L + 1) / 1461001
  L = L - 1461 * I / 4 + 31
  J = 80 * L / 2447
  D = L - 2447 * J / 80
  L = J / 11
  M = J + 2 - 12 * L
  Y = 100 * (N - 49) + I + L
```

Use `chrono` crate for actual implementation: construct a `NaiveDate` from MJD by adding MJD days to the MJD epoch (1858-11-17).

### CSV/TSV Export

Export button in the toolbar opens a file save dialog (`gtk::FileDialog`). Options:
- **CSV**: comma-separated, with column headers, quoted strings
- **TSV**: tab-separated, with column headers

Export includes all rows matching the current filter (not just the current page).

---

## 9. Recent Searches

### Storage

- **File**: `{XDG_DATA_HOME}/Verbinal/recent_searches.json`
- **Format**: JSON array of `RecentSearch` objects
- **Max entries**: 20

### Model

```rust
pub struct RecentSearch {
    pub summary: String,        // Human-readable summary e.g. "M31 | JWST | Infrared"
    pub adql: String,           // The full ADQL query that was executed
    pub result_count: usize,    // Number of rows returned
    pub searched_at: String,    // RFC 3339 timestamp
}
```

### Behavior

- After each successful search, save a `RecentSearch` to the list
- `summary` is built from non-empty form fields: target, collection, instrument, band joined by ` | `. If no fields set, use "All observations".
- Dedup: if an entry with the same `adql` already exists, remove the old one before inserting at front
- Display in the sidebar as `adw::ActionRow` items with:
  - Title: summary text
  - Subtitle: `"{result_count} results - {relative_time}"` (e.g., "2 hours ago")
  - Click to re-execute the saved ADQL query

### Sidebar Display

```
+----------------------------+
| Recent Searches  [Clear]   |
+----------------------------+
| M31 | JWST                 |
| 1,234 results - 2h ago    |
+----------------------------+
| NGC 1234 | HST | Optical   |
| 56 results - yesterday     |
+----------------------------+
| ...                        |
+----------------------------+
| Saved Queries    [+]       |
+----------------------------+
| "My JWST Survey"           |
| 5,000 results              |
+----------------------------+
```

---

## 10. Saved Queries

### Storage

- **File**: `{XDG_DATA_HOME}/Verbinal/saved_queries.json`
- **Format**: JSON array of `SavedQuery` objects

### Model

```rust
pub struct SavedQuery {
    pub name: String,             // User-chosen name
    pub form_state: SearchFormState,  // Complete form state snapshot
    pub adql: String,             // The ADQL query
    pub created_at: String,       // RFC 3339 timestamp
    pub last_used: Option<String>, // RFC 3339 timestamp of last execution
}
```

### Behavior

- User clicks a "Save Query" button (available after a search is executed)
- Dialog asks for a name (adw::EntryRow in a MessageDialog)
- Saves the entire `SearchFormState` + generated ADQL
- Clicking a saved query in the sidebar restores the form state and optionally re-executes
- Right-click or long-press on a saved query shows options: Rename, Delete, Execute

---

## 11. ADQL Editor Tab

The second tab in the search form notebook provides a raw ADQL editor.

### Layout

```
+-----------------------------------+
| [ADQL Editor]                     |
| gtk::TextView (monospace)         |
| Full ADQL query text              |
|                                   |
|                                   |
+-----------------------------------+
| [Execute] [Copy] [Clear]         |
+-----------------------------------+
```

### Behavior

- When switching FROM the Form tab, the ADQL editor is populated with `adql_builder::build(&form_state)`
- The user can freely edit the ADQL text
- "Execute" sends the ADQL directly to the TAP service (bypassing the form builder)
- "Copy" copies the ADQL to the system clipboard
- "Clear" empties the editor
- If the user edits the ADQL and switches back to the Form tab, the form is NOT updated (ADQL takes priority)

---

## 12. Download Flow

### Trigger

When the user selects a result row and clicks a "Download" button (or double-clicks):

### Step 1: Resolve DataLink

If not already cached, call the DataLink service to get available files:
```
GET https://ws.cadc-ccda.hia-iha.nrc-cnrc.gc.ca/caom2ops/datalink?id={publisherID}
```

### Step 2: File Picker Dialog

Show a dialog listing all available files from the DataLink response:

```
+-------------------------------------------+
| Download Files for: {target_name}         |
+-------------------------------------------+
| [x] jw05791_nrcb2_cal.fits    (112 MB)   |
| [x] jw05791_nrcb2_i2d.fits    (113 MB)   |
| [ ] jw05791_nrcb2_rate.fits   (80 MB)    |
| [ ] jw05791_nrcb2_rateints.fits (160 MB) |
+-------------------------------------------+
| [ ] Include previews (JPG)                |
| [ ] Include auxiliary files               |
+-------------------------------------------+
| Download to: [~/Downloads/verbinal/]  [..]|
+-------------------------------------------+
|                     [Cancel] [Download]   |
+-------------------------------------------+
```

- Files grouped by semantics: `#this` files shown first with checkboxes
- `#preview` and `#auxiliary` toggleable as groups
- File sizes shown in human-readable format (KB/MB/GB)
- Default download directory: `~/Downloads/verbinal/`
- Directory chooser via `gtk::FileDialog`

### Step 3: Download with Progress

- Show a progress dialog with per-file progress bars
- Download files using `reqwest` with streaming response
- Track bytes received vs. content-length for progress percentage
- Max 3 concurrent downloads (Semaphore)
- On completion, optionally save to an ObservationStore (for the Research module)

### Step 4: Save to ObservationStore

After download completes, save metadata about the downloaded observation:
- publisherID
- target name, collection, instrument
- local file paths
- download timestamp

This data feeds the (future) Research module.

---

## 13. Data Models

### SearchFormState (full spec)

```rust
pub struct SearchFormState {
    // Spatial
    pub target: String,
    pub resolver_service: String,       // "ALL", "SIMBAD", "NED", "VIZIER"
    pub resolved_ra: Option<f64>,       // Degrees
    pub resolved_dec: Option<f64>,      // Degrees
    pub search_radius: f64,             // Degrees, default 0.05
    pub pixel_scale_max: Option<f64>,   // Arcseconds

    // Observation
    pub observation_id: String,         // Supports wildcards
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
    pub integration_time_min: Option<f64>,  // Seconds
    pub integration_time_max: Option<f64>,
    pub time_span_min: Option<f64>,         // Days
    pub time_span_max: Option<f64>,

    // Spectral
    pub wavelength_min: Option<f64>,    // Internal: meters
    pub wavelength_max: Option<f64>,    // Internal: meters
    pub wavelength_unit: String,        // "nm", "um", "Angstrom", "m"
    pub spectral_coverage: Option<f64>,
    pub spectral_sampling: Option<f64>,
    pub resolving_power_min: Option<f64>,
    pub resolving_power_max: Option<f64>,
    pub bandpass_width_min: Option<f64>,
    pub bandpass_width_max: Option<f64>,
    pub rest_frame_energy_min: Option<f64>, // eV
    pub rest_frame_energy_max: Option<f64>,

    // Data Train selections
    pub collection: String,             // Single or comma-separated
    pub instrument: String,
    pub band: String,                   // energy_emBand value
    pub filter_name: String,            // energy_bandpassName
    pub calibration_level: String,      // "0", "1", "2", "3", or ""
    pub data_product_type: String,      // "image", "spectrum", etc.
    pub obs_type: String,               // "OBJECT", "FLAT", etc.
    pub data_release: String,           // YYYY-MM-DD

    // Options
    pub max_records: u32,               // Default: 1000, range 10-50000
}
```

### SearchResults

```rust
pub struct SearchResults {
    pub columns: Vec<String>,
    pub rows: Vec<SearchResultRow>,
    pub query: Option<String>,          // The ADQL that produced these results
}
```

`total_rows()` returns `rows.len()`.

### SearchResultRow

```rust
pub struct SearchResultRow {
    pub values: HashMap<String, String>,  // Column name -> value
}
```

`get(key)` returns the value or `""` if not found.

### ResolverResult

```rust
pub struct ResolverResult {
    pub target: String,              // The input target name
    pub ra: f64,                     // Degrees
    pub dec: f64,                    // Degrees
    pub coord_sys: Option<String>,   // e.g., "ICRS"
    pub object_type: Option<String>, // e.g., "AGN", "SNR"
    pub service: Option<String>,     // e.g., "Simbad(simbad.cds.unistra.fr)"
}
```

### DataTrainRow

```rust
pub struct DataTrainRow {
    pub value: String,       // The enum value (e.g., "JWST", "Infrared")
    pub selected: bool,      // Whether this item is selected in the filter
}
```

### RecentSearch

```rust
pub struct RecentSearch {
    pub summary: String,
    pub adql: String,
    pub result_count: usize,
    pub searched_at: String,    // RFC 3339
}
```

### SavedQuery

```rust
pub struct SavedQuery {
    pub name: String,
    pub form_state: SearchFormState,
    pub adql: String,
    pub created_at: String,     // RFC 3339
    pub last_used: Option<String>,
}
```

### DataLinkResult

```rust
pub struct DataLinkResult {
    pub publisher_id: String,
    pub files: Vec<DataLinkFile>,
    pub resolved_at: std::time::Instant,
}
```

### DataLinkFile

```rust
pub struct DataLinkFile {
    pub url: String,
    pub semantics: String,       // "#this", "#preview", "#thumbnail", "#auxiliary"
    pub content_type: String,    // MIME type
    pub size: Option<u64>,       // Bytes
    pub description: String,
}

impl DataLinkFile {
    pub fn is_science_data(&self) -> bool {
        self.semantics == "#this"
    }

    pub fn is_preview(&self) -> bool {
        self.semantics == "#preview"
    }

    pub fn is_thumbnail(&self) -> bool {
        self.semantics == "#thumbnail"
    }

    pub fn is_auxiliary(&self) -> bool {
        self.semantics == "#auxiliary"
    }

    pub fn filename(&self) -> String {
        // Extract filename from URL path
        self.url.rsplit('/').next().unwrap_or("unknown").to_string()
    }

    pub fn size_display(&self) -> String {
        match self.size {
            Some(bytes) if bytes < 1024 => format!("{} B", bytes),
            Some(bytes) if bytes < 1024 * 1024 => format!("{:.1} KB", bytes as f64 / 1024.0),
            Some(bytes) if bytes < 1024 * 1024 * 1024 => format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0)),
            Some(bytes) => format!("{:.2} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0)),
            None => "unknown".to_string(),
        }
    }
}
```

### ResultColumnInfo

```rust
pub struct ResultColumnInfo {
    pub name: String,           // Column name as returned by TAP
    pub display_name: String,   // Human-readable label (the AS alias)
    pub visible: bool,          // Whether to show in the results table
    pub sortable: bool,         // Whether this column can be sorted
    pub is_numeric: bool,       // If true, sort numerically; if false, lexicographic
    pub format: ColumnFormat,   // How to format cell values
}

pub enum ColumnFormat {
    Plain,
    Degrees5,           // 5 decimal places
    MjdToDate,          // MJD -> ISO date
    IntegrationTime,    // Auto-units (s/min/h)
    CalibrationLevel,   // 0/1/2/3 -> name mapping
    WavelengthMeters,   // Scientific notation or significant digits
    AreaDegrees,        // 4 decimal places + " deg^2"
    ArcsecPixelScale,   // 3 decimal places + '"'
    IsoDate,            // ISO date string
}
```

---

## 14. External Service URLs

| Service | URL | Auth |
|---------|-----|------|
| TAP Sync | `https://ws.cadc-ccda.hia-iha.nrc-cnrc.gc.ca/argus/sync` | None |
| Target Resolver | `https://ws.cadc-ccda.hia-iha.nrc-cnrc.gc.ca/cadc-target-resolver/find` | None |
| DataLink | `https://ws.cadc-ccda.hia-iha.nrc-cnrc.gc.ca/caom2ops/datalink` | None/Bearer |
| File Download (raven) | `https://ws.cadc-ccda.hia-iha.nrc-cnrc.gc.ca/raven/files/{path}` | None/Bearer |

All CADC services are HTTPS. No API keys required for public data. Bearer token needed for proprietary data access.

---

## 15. Error Handling

### TAP Service Errors

- HTTP 400: Usually malformed ADQL. Display the error body to the user (it often contains a helpful parse error message from the TAP service).
- HTTP 5xx: "TAP service unavailable. Try again later."
- Timeout (60s): "Query timed out. Try narrowing your search or reducing max records."
- CSV parse failure: "Failed to parse results. The query may have returned an error document instead of CSV."

### Resolver Errors

- HTTP 4xx: "Could not resolve target '{name}'. Check the spelling or try a different resolver service."
- Empty coordinates: "No coordinates found for '{name}'."
- Timeout (15s): "Resolver timed out. Try again."

### DataLink Errors

- HTTP 4xx/5xx: "Failed to resolve download links for this observation."
- Empty VOTable: "No files available for this observation."
- Individual file download failure: Show per-file error, do not abort other downloads.

---

## 16. Concurrency Notes

### Threading Model

The search module shares the same concurrency architecture as the portal:
- TAP queries and resolver calls run on the Tokio runtime via `AppServices::spawn()` or `tokio::spawn()`
- Results are sent back to the GLib main thread via `tokio::sync::oneshot` channels
- UI updates happen exclusively on the GLib main thread via `glib::spawn_future_local()`

### Semaphore for DataLink

```rust
static DATALINK_SEMAPHORE: Lazy<tokio::sync::Semaphore> = Lazy::new(|| tokio::sync::Semaphore::new(3));
static IMAGE_DL_SEMAPHORE: Lazy<tokio::sync::Semaphore> = Lazy::new(|| tokio::sync::Semaphore::new(3));
```

Before each DataLink resolution or thumbnail download, acquire a permit. This prevents flooding the CADC servers.

### Caching

- **DataLink cache**: `HashMap<String, DataLinkResult>` keyed by publisherID, stored in `Rc<RefCell<>>` on the UI thread. Entries never expire during a session.
- **Collection list cache**: Fetched once on page init, stored for the lifetime of the page.
- **Instrument/filter cascades**: Not cached; re-fetched when the parent selection changes (these are fast, small queries).

---

## 17. Keyboard Shortcuts

| Shortcut | Action |
|----------|--------|
| `Enter` (in target field) | Resolve target |
| `Ctrl+Enter` | Execute search |
| `Ctrl+S` | Save current query |
| `Ctrl+E` | Toggle ADQL editor tab |
| `Escape` | Clear current filter in results |

---

## 18. Integration with Other Modules

### FITS Viewer

When a `#this` file with content type `application/fits` is downloaded, the user should be offered the option to open it directly in the FITS Viewer tab. This is done by:
1. Saving the file to a temp or user-chosen location
2. Loading it into the `FitsViewer` component
3. Switching the ViewStack to the "fits" page

### VOSpace Storage

Downloaded files can optionally be uploaded to the user's VOSpace storage for access from CANFAR sessions. This integration is deferred to a future iteration.

### Research Module

The Research module (placeholder) will consume the observation metadata saved during downloads. The Search module should persist download records to `{XDG_DATA_HOME}/Verbinal/observations.json` for the Research module to read.
