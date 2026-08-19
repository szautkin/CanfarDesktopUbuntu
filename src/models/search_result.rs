use crate::helpers::agent_attribution::AgentAttribution;
use crate::helpers::{column_units, sexagesimal};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Search Results (from TAP CSV)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct SearchResults {
    pub columns: Vec<String>,
    pub rows: Vec<SearchResultRow>,
}

impl SearchResults {
    pub fn total_rows(&self) -> usize {
        self.rows.len()
    }
}

#[derive(Debug, Clone, Default)]
pub struct SearchResultRow {
    pub values: HashMap<String, String>,
}

impl SearchResultRow {
    pub fn get(&self, key: &str) -> &str {
        self.values.get(key).map(|s| s.as_str()).unwrap_or("")
    }
}

// ---------------------------------------------------------------------------
// Search Form State (all constraint fields)
// ---------------------------------------------------------------------------

/// The complete state of the search form.
///
/// `#[serde(default)]` at the container level is deliberate and load-bearing:
/// this struct is PERSISTED inside every recent search, so a record written by
/// an older build (or a newer one, missing a field this build expects) must
/// still load. Without it, adding a single field silently made every previously
/// saved search unreadable — they would vanish from the sidebar with no error.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchFormState {
    // Spatial
    pub target: String,
    pub resolver_service: String,
    /// The resolver service that actually produced `resolved_ra`/`resolved_dec`
    /// (provenance for a resolved name, e.g. "SIMBAD"/"NED"/"VizieR").
    #[serde(default)]
    pub resolver_service_used: Option<String>,
    /// The epoch/time at which the target name was resolved (RFC-3339 string).
    #[serde(default)]
    pub resolution_epoch: Option<String>,
    pub resolved_ra: Option<f64>,
    pub resolved_dec: Option<f64>,
    pub search_radius: f64,
    pub pixel_scale_max: Option<f64>,
    pub pixel_scale_unit: String,
    /// Optional operator-aware pixel-scale text (RANGE syntax: `> 0.2`, `0.1..0.3`).
    /// When present it takes precedence over `pixel_scale_max`.
    pub pixel_scale_raw: Option<String>,
    pub spatial_cutout: bool,

    // Observation
    pub observation_id: String,
    pub proposal_pi: String,
    pub proposal_id: String,
    pub proposal_title: String,
    pub proposal_keywords: String,
    pub intent: String,
    pub public_only: bool,

    // Temporal
    pub date_preset: String,
    pub obs_date_start: String,
    pub obs_date_end: String,
    pub obs_date_raw: String,
    pub integration_time_min: Option<f64>,
    pub integration_time_max: Option<f64>,
    pub integration_time_unit: String,
    pub time_span_min: Option<f64>,
    pub time_span_max: Option<f64>,
    pub time_span_unit: String,

    // Spectral
    pub wavelength_min: Option<f64>,
    pub wavelength_max: Option<f64>,
    pub wavelength_unit: String,
    pub spectral_coverage: Option<f64>,
    pub spectral_sampling: Option<f64>,
    /// Optional operator-aware spectral-sampling text (RANGE syntax: `> 0.2`,
    /// `0.1..0.3`, `<= 5`). When present it takes precedence over the legacy
    /// numeric `spectral_sampling` (mirrors `pixel_scale_raw`).
    pub spectral_sampling_raw: Option<String>,
    pub spectral_sampling_unit: String,
    pub resolving_power_min: Option<f64>,
    pub resolving_power_max: Option<f64>,
    pub bandpass_width_min: Option<f64>,
    pub bandpass_width_max: Option<f64>,
    pub bandpass_width_unit: String,
    pub rest_frame_energy_min: Option<f64>,
    pub rest_frame_energy_max: Option<f64>,
    pub rest_frame_energy_unit: String,
    pub spectral_cutout: bool,

    // Data Train
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

    // ── Verbatim entry text, for restoring a saved search into the form ──────
    //
    // The numeric min/max fields above are LOSSY: `parse_range_minmax` maps both
    // `>5` and `>=5` to `(Some(5), None)`, so the operator cannot be recovered
    // from them. These keep what the user actually typed, mirroring the existing
    // `pixel_scale_raw` / `spectral_sampling_raw` precedent. The container-level
    // `#[serde(default)]` is what lets searches saved before these existed load.
    pub integration_time_raw: String,
    pub time_span_raw: String,
    pub spectral_coverage_raw: String,
    pub resolving_power_raw: String,
    pub bandpass_width_raw: String,
    pub rest_frame_energy_raw: String,
}

impl SearchFormState {
    pub fn new() -> Self {
        SearchFormState {
            resolver_service: "ALL".to_string(),
            search_radius: 0.0167,
            max_records: 10000,
            wavelength_unit: "nm".to_string(),
            pixel_scale_unit: "arcsec".to_string(),
            integration_time_unit: "s".to_string(),
            time_span_unit: "d".to_string(),
            spectral_sampling_unit: "nm".to_string(),
            bandpass_width_unit: "nm".to_string(),
            rest_frame_energy_unit: "nm".to_string(),
            ..Default::default()
        }
    }

    /// Build a human-readable summary for recent search display.
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if !self.target.is_empty() {
            parts.push(self.target.clone());
        }
        if !self.collection.is_empty() {
            parts.push(self.collection.clone());
        }
        if !self.instrument.is_empty() {
            parts.push(self.instrument.clone());
        }
        if !self.band.is_empty() {
            parts.push(format!("band={}", self.band));
        }
        if parts.is_empty() {
            "All observations".to_string()
        } else {
            parts.join(", ")
        }
    }
}

// ---------------------------------------------------------------------------
// Resolver
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ResolverResult {
    pub target: String,
    pub ra: f64,
    pub dec: f64,
    pub coord_sys: Option<String>,
    pub object_type: Option<String>,
    pub service: Option<String>,
    /// UTC instant (RFC-3339) at which this resolution was produced — the
    /// resolver-provenance epoch (mirrors Windows `ResolverResult.ResolvedAt`,
    /// stamped `DateTime.UtcNow` when the result is materialized). Optional +
    /// `#[serde(default)]` so pre-provenance cached JSON still deserializes.
    #[serde(default)]
    pub resolved_at: Option<String>,
}

// ---------------------------------------------------------------------------
// DataLink
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct DataLinkResult {
    pub publisher_id: String,
    pub files: Vec<DataLinkFile>,
    pub download_url: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DataLinkFile {
    pub url: String,
    pub semantics: String,
    pub content_type: String,
    pub size: Option<u64>,
    pub description: String,
}

impl DataLinkResult {
    /// The science products — DataLink rows with `#this` semantics.
    ///
    /// This is the list `artifactIndex` addresses, in both `get_data_links` and
    /// `download_observation`. Keeping it in one place is load-bearing: if the
    /// tool that reports the index and the tool that consumes it disagreed
    /// about which rows count, an agent asking for "the second science file"
    /// would silently download a thumbnail.
    pub fn direct_files(&self) -> Vec<&DataLinkFile> {
        self.files.iter().filter(|f| f.is_science_data()).collect()
    }

    /// Preview-image URLs (`#preview` rows with an image content type).
    pub fn preview_urls(&self) -> Vec<String> {
        self.files
            .iter()
            .filter(|f| f.is_preview())
            .map(|f| f.url.clone())
            .collect()
    }

    /// Thumbnail URLs (`#thumbnail` rows).
    pub fn thumbnail_urls(&self) -> Vec<String> {
        self.files
            .iter()
            .filter(|f| f.is_thumbnail())
            .map(|f| f.url.clone())
            .collect()
    }

    /// Rows in none of the above buckets — `#auxiliary`, `#derivation`, and
    /// anything else a collection publishes.
    ///
    /// The reference discards these while parsing. We keep them: they are real
    /// artifacts, and dropping a row because it is unfamiliar loses data the
    /// user may want. They are deliberately NOT part of `direct_files`, so
    /// including them cannot shift an `artifactIndex`.
    pub fn other_files(&self) -> Vec<&DataLinkFile> {
        self.files
            .iter()
            .filter(|f| !f.is_science_data() && !f.is_preview() && !f.is_thumbnail())
            .collect()
    }
}

impl DataLinkFile {
    pub fn is_science_data(&self) -> bool {
        self.semantics == "#this"
    }
    pub fn is_preview(&self) -> bool {
        // Require an image content-type so a mislabelled #preview row (e.g. a data
        // product) is never rendered as an image (matches the reference guard).
        self.semantics == "#preview" && self.content_type.to_ascii_lowercase().contains("image")
    }
    pub fn is_thumbnail(&self) -> bool {
        self.semantics == "#thumbnail"
    }

    pub fn filename(&self) -> String {
        self.url.rsplit('/').next().unwrap_or("unknown").to_string()
    }

    #[cfg(test)]
    pub fn size_display(&self) -> String {
        match self.size {
            Some(b) if b < 1024 => format!("{} B", b),
            Some(b) if b < 1024 * 1024 => format!("{:.1} KB", b as f64 / 1024.0),
            Some(b) if b < 1024 * 1024 * 1024 => {
                format!("{:.1} MB", b as f64 / (1024.0 * 1024.0))
            }
            Some(b) => format!("{:.2} GB", b as f64 / (1024.0 * 1024.0 * 1024.0)),
            None => "unknown".to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// Saved Queries & Recent Searches
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SavedQuery {
    pub name: String,
    pub adql: String,
    pub created_at: String,
    /// Provenance stamp when this query was saved via an applied agent proposal.
    /// `None` for user-authored saves; a `Some(..)` value drives the wand badge.
    /// `#[serde(default)]` keeps pre-attribution JSON readable.
    #[serde(default)]
    pub agent_attribution: Option<AgentAttribution>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RecentSearch {
    pub summary: String,
    pub adql: String,
    pub form_state: SearchFormState,
    pub result_count: usize,
    pub searched_at: String,
    /// Resolver service that produced the coordinates for this search, if any.
    #[serde(default)]
    pub resolver_service_used: Option<String>,
    /// Epoch at which the target name was resolved (RFC-3339 string), if any.
    #[serde(default)]
    pub resolution_epoch: Option<String>,
}

// ---------------------------------------------------------------------------
// Column Formatting
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ColumnFormat {
    Plain,
    Degrees5,
    MjdToDate,
    IntegrationTime,
    CalibrationLevel,
    WavelengthMeters,
    IsoDate,
}

#[derive(Debug, Clone)]
pub struct ResultColumnInfo {
    pub key: String, // cleaned key for formatting dispatch (lowercase, no quotes/spaces/dots)
    pub header: String, // raw CSV header for row.get() lookup
    pub display_name: String, // human-readable label
    pub visible: bool,
    pub format: ColumnFormat,
}

/// Clean a CSV header into a normalized key (matches Windows CellFormatter.CleanKey).
/// Removes quotes, spaces, dots, lowercases.
pub fn clean_key(header: &str) -> String {
    header.replace(['"', ' ', '.'], "").trim().to_lowercase()
}

/// Default visible keys (cleaned form), matching the reference's
/// `CellFormatter.DefaultVisibleKeys`.
///
/// `obsid` belongs here and was missing: it is the handle every other tool
/// wants — the thing you paste into `get_observation_details`, or search for
/// again later — and a reader had to open the Columns dialog to discover the
/// results even had one. `datatype` and `band` are not in the reference's set
/// and are no longer in ours; both remain one click away in the dialog.
static DEFAULT_VISIBLE: &[&str] = &[
    "collection",
    "targetname",
    "ra(j20000)",
    "dec(j20000)",
    "startdate",
    "instrument",
    "filter",
    "callev",
    "obstype",
    "proposalid",
    "piname",
    "obsid",
];

/// Display width in pixels for a results column, by cleaned key.
///
/// Ported from the reference's `CellFormatter.ColumnWidth`. One fixed width for
/// all 41 columns wasted half the row on `filter` and `callev` while still
/// truncating a target name — and horizontal space is the scarce resource in a
/// grid this wide.
///
/// The header strip and the data rows live in separate scroll areas kept in step
/// by a shared adjustment, so BOTH sides must size a column through this
/// function or the labels drift off the values they name.
pub fn column_width(key: &str) -> i32 {
    match key {
        "collection" | "proposalid" | "obsid" => 100,
        "targetname" | "piname" => 110,
        "ra(j20000)" | "dec(j20000)" => 95,
        "startdate" | "enddate" | "datarelease" => 90,
        "instrument" => 90,
        "inttime" => 65,
        "filter" | "callev" | "band" => 60,
        "obstype" | "datatype" => 75,
        _ => 80,
    }
}

/// Cleaned keys whose TAP values are numeric.
///
/// Filters compare against the RAW cell, not the formatted one, so this follows
/// the CAOM2 column type rather than how the column is displayed: `startdate`
/// reads as a date but arrives as an MJD, and `datarelease` reads as a date and
/// arrives as an ISO timestamp string.
///
/// It decides which of CADC's two filter tooltips a column gets, and nothing
/// else — the filter parser itself switches on whether the values in front of
/// it parse as numbers, so a wrong entry here misleads the reader without
/// changing a single row.
static NUMERIC_KEYS: &[&str] = &[
    "sequencenumber",
    "ra(j20000)",
    "dec(j20000)",
    "startdate",
    "enddate",
    "inttime",
    "callev",
    "minwavelength",
    "maxwavelength",
    "fieldofview",
    "pixelscale",
    "resolvingpower",
    "spatialresolution",
    "bandpasswidth",
    "energysamplesize",
    "restframeenergy",
    "timespan",
];

/// Whether a results column holds numbers.
pub fn column_is_numeric(key: &str) -> bool {
    NUMERIC_KEYS.contains(&key)
}

/// Format dispatch by cleaned key, matching Windows CellFormatter.
fn format_for_key(key: &str) -> ColumnFormat {
    match key {
        "startdate" | "enddate" => ColumnFormat::MjdToDate,
        "ra(j20000)" | "dec(j20000)" => ColumnFormat::Degrees5,
        "inttime" => ColumnFormat::IntegrationTime,
        "callev" => ColumnFormat::CalibrationLevel,
        "minwavelength" | "maxwavelength" | "restframeenergy" => ColumnFormat::WavelengthMeters,
        "datarelease" => ColumnFormat::IsoDate,
        _ => ColumnFormat::Plain,
    }
}

/// Build column info list from actual CSV headers returned by TAP.
/// This ensures keys match the real data.
/// Resolve one column's visibility: an explicit user choice wins, otherwise the
/// column's own default from [`DEFAULT_VISIBLE`].
///
/// Overrides are stored per column rather than as a hide-only set, because a
/// hide-only set can never REVEAL a column that is not visible by default —
/// which silently made most of the 41 TAP columns unreachable.
pub fn column_is_visible(
    overrides: &std::collections::HashMap<String, bool>,
    key: &str,
    default_visible: bool,
) -> bool {
    overrides.get(key).copied().unwrap_or(default_visible)
}

pub fn build_columns_from_headers(headers: &[String]) -> Vec<ResultColumnInfo> {
    headers
        .iter()
        .map(|header| {
            let key = clean_key(header);
            let display = header.replace('"', "").trim().to_string();
            let visible = DEFAULT_VISIBLE.contains(&key.as_str());
            let format = format_for_key(&key);
            ResultColumnInfo {
                key,
                header: header.clone(),
                display_name: display,
                visible,
                format,
            }
        })
        .collect()
}

/// Fallback static columns (used when no results yet).
pub fn default_columns() -> Vec<ResultColumnInfo> {
    let headers = vec![
        "collection",
        "Target Name",
        "RA (J2000.0)",
        "Dec. (J2000.0)",
        "Start Date",
        "Int. Time",
        "Instrument",
        "Filter",
        "Cal. Lev.",
        "Obs. Type",
        "Proposal ID",
        "PI Name",
        "Data Type",
        "Band",
        // The ADQL selects this as `Obs. ID`; it belongs in the placeholder set
        // too, or the pre-search grid advertises a different set of columns than
        // the one a search returns.
        "Obs. ID",
        "publisherID",
    ];
    headers
        .iter()
        .map(|h| {
            let key = clean_key(h);
            let visible = DEFAULT_VISIBLE.contains(&key.as_str());
            let format = format_for_key(&key);
            ResultColumnInfo {
                key,
                header: h.to_string(),
                display_name: h.replace('"', "").trim().to_string(),
                visible,
                format,
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// CSV Parsing
// ---------------------------------------------------------------------------

/// Parse a CSV response from the TAP service into SearchResults.
///
/// The ADQL that produced it used to ride along in a `query` field nothing ever
/// read — the search page keeps the text it sent, in the editor the user typed
/// it into.
pub fn parse_csv(csv: &str) -> SearchResults {
    let mut result = SearchResults::default();

    let lines: Vec<&str> = csv.lines().filter(|l| !l.is_empty()).collect();
    if lines.is_empty() {
        return result;
    }

    result.columns = parse_csv_line(lines[0]);

    for line in &lines[1..] {
        let values = parse_csv_line(line);
        if values.len() != result.columns.len() {
            continue;
        }
        let mut row = SearchResultRow::default();
        for (j, col) in result.columns.iter().enumerate() {
            // Store each value under BOTH the raw CSV header (used by cell rendering)
            // and its cleaned key (used by column filter/sort). Without the cleaned
            // alias, filtering/sorting any AS-aliased column (RA, Dec, Target, …)
            // silently missed — `row.get(cleaned_key)` returned "".
            row.values.insert(col.clone(), values[j].clone());
            let cleaned = clean_key(col);
            if cleaned != *col {
                row.values
                    .entry(cleaned)
                    .or_insert_with(|| values[j].clone());
            }
        }
        result.rows.push(row);
    }

    result
}

fn parse_csv_line(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut in_quotes = false;
    let mut field = String::new();
    let chars: Vec<char> = line.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        let ch = chars[i];
        match ch {
            '"' if in_quotes && i + 1 < len && chars[i + 1] == '"' => {
                // RFC 4180: escaped double-quote inside quoted field
                field.push('"');
                i += 2;
                continue;
            }
            '"' => in_quotes = !in_quotes,
            ',' if !in_quotes => {
                fields.push(field.trim().to_string());
                field = String::new();
            }
            _ => field.push(ch),
        }
        i += 1;
    }
    fields.push(field.trim().to_string());
    fields
}

// ---------------------------------------------------------------------------
// Resolver Response Parsing
// ---------------------------------------------------------------------------

/// Parse the CADC resolver ASCII response into a ResolverResult.
pub fn parse_resolver_response(text: &str, target: &str) -> Option<ResolverResult> {
    let mut ra = None;
    let mut dec = None;
    let mut coord_sys = None;
    let mut object_type = None;
    let mut service = None;

    for line in text.lines() {
        let line = line.trim();
        if let Some((key, val)) = line.split_once('=') {
            let key = key.trim();
            let val = val.trim();
            match key {
                "ra" => ra = val.parse().ok(),
                "dec" => dec = val.parse().ok(),
                "coordsys" => coord_sys = Some(val.to_string()),
                "oType" | "otype" => object_type = Some(val.to_string()),
                "service" => service = Some(val.to_string()),
                _ => {}
            }
        }
    }

    Some(ResolverResult {
        target: target.to_string(),
        ra: ra?,
        dec: dec?,
        coord_sys,
        object_type,
        service,
        // Stamp the resolution epoch where the resolver result is materialized
        // (tap_service forwards this value unchanged). Mirrors the Windows
        // TAPService setting `ResolvedAt = DateTime.UtcNow`.
        resolved_at: Some(chrono::Utc::now().to_rfc3339()),
    })
}

// ---------------------------------------------------------------------------
// Cell Formatting
// ---------------------------------------------------------------------------

/// Format a cell value for display based on its column format.
pub fn format_cell(value: &str, format: ColumnFormat) -> String {
    if value.is_empty() {
        return String::new();
    }
    match format {
        ColumnFormat::Plain => value.to_string(),
        ColumnFormat::Degrees5 => match value.parse::<f64>() {
            Ok(v) => format!("{:.5}", v),
            Err(_) => value.to_string(),
        },
        ColumnFormat::MjdToDate => mjd_to_date(value),
        ColumnFormat::IntegrationTime => format_integration_time(value),
        ColumnFormat::CalibrationLevel => match value.trim() {
            "0" => "Raw".to_string(),
            "1" => "Calibrated".to_string(),
            "2" => "Product".to_string(),
            "3" => "Composite".to_string(),
            other => other.to_string(),
        },
        ColumnFormat::WavelengthMeters => match value.parse::<f64>() {
            Ok(v) if v < 1e-6 => format!("{:.3e} m", v),
            Ok(v) => format!("{:.6} m", v),
            Err(_) => value.to_string(),
        },
        ColumnFormat::IsoDate => {
            // Strip fractional seconds and Z suffix for cleaner display
            if value.len() > 10 {
                value[..10].to_string()
            } else {
                value.to_string()
            }
        }
    }
}

/// Format a cell honoring the chosen display unit for unit-menu columns
/// (RA/Dec sexagesimal vs degrees, spectral, duration, angle, area, dates).
///
/// `unit == None` selects the column's default rendering (RA/Dec default to
/// sexagesimal — the new behaviour; every other column keeps its readable legacy
/// default). Non-menu columns fall through to the per-key formatters. Port of
/// CanfarDesktop `CellFormatter.Format(columnKey, raw, unitId)`.
pub fn format_cell_with_unit(header: &str, raw: &str, unit: Option<&str>) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let key = clean_key(header);

    // An explicit unit chosen from the column's menu overrides default rendering.
    if let Some(u) = unit {
        if column_units::has_menu(&key) {
            return format_with_unit(&key, trimmed, u);
        }
    }

    match key.as_str() {
        "startdate" | "enddate" | "provelastexecuted" => mjd_to_date(trimmed),
        "ra(j20000)" => sexagesimal::format_ra_str(trimmed),
        "dec(j20000)" => sexagesimal::format_dec_str(trimmed),
        "inttime" => format_integration_time(trimmed),
        "callev" => format_calibration_c(trimmed),
        "download" | "movingtarget" => format_boolean(trimmed),
        "minwavelength" | "maxwavelength" | "restframeenergy" => format_wavelength(trimmed),
        "pixelscale" => format_scientific(trimmed, 4),
        "fieldofview" => format_scientific(trimmed, 6),
        "datarelease" => format_timestamp(trimmed),
        _ => trimmed.to_string(),
    }
}

/// Render a unit-menu column's cell in the chosen unit.
fn format_with_unit(key: &str, raw: &str, unit: &str) -> String {
    match key {
        "ra(j20000)" => {
            if unit == "degrees" {
                format_ra_degrees(raw)
            } else {
                sexagesimal::format_ra_str(raw)
            }
        }
        "dec(j20000)" => {
            if unit == "degrees" {
                format_dec_degrees(raw)
            } else {
                sexagesimal::format_dec_str(raw)
            }
        }
        "minwavelength" | "maxwavelength" | "restframeenergy" => {
            column_units::format_spectral(raw, unit)
        }
        "inttime" => column_units::format_duration(raw, unit),
        "pixelscale" | "positionresolution" => column_units::format_angle(raw, unit),
        "fieldofview" => column_units::format_area(raw, unit),
        "startdate" | "enddate" => {
            if unit == "mjd" {
                raw.to_string()
            } else {
                mjd_to_date(raw)
            }
        }
        _ => raw.to_string(),
    }
}

// RA degrees: fixed 6 decimals, sign only when negative.
fn format_ra_degrees(raw: &str) -> String {
    match raw.parse::<f64>() {
        Ok(v) if v.is_finite() => format!("{:.6}", v),
        _ => raw.to_string(),
    }
}

// Dec degrees: fixed 6 decimals, always signed.
fn format_dec_degrees(raw: &str) -> String {
    match raw.parse::<f64>() {
        Ok(v) if v.is_finite() => format!("{:+.6}", v),
        _ => raw.to_string(),
    }
}

// Calibration level matching the reference CellFormatter ("Cal", not "Calibrated").
fn format_calibration_c(raw: &str) -> String {
    match raw {
        "0" => "Raw",
        "1" => "Cal",
        "2" => "Product",
        "3" => "Composite",
        other => other,
    }
    .to_string()
}

fn format_boolean(raw: &str) -> String {
    if raw.eq_ignore_ascii_case("true") || raw == "1" {
        "\u{2713}".to_string()
    } else {
        String::new()
    }
}

fn format_wavelength(raw: &str) -> String {
    match raw.parse::<f64>() {
        Ok(v) => {
            let mag = v.abs();
            if !(0.001..=1e6).contains(&mag) {
                format!("{:.3e}", v)
            } else {
                format!("{}", v)
            }
        }
        Err(_) => raw.to_string(),
    }
}

fn format_scientific(raw: &str, decimals: usize) -> String {
    match raw.parse::<f64>() {
        Ok(v) => {
            let mag = v.abs();
            if !(0.001..=1e6).contains(&mag) {
                format!("{:.*e}", decimals, v)
            } else {
                format!("{}", v)
            }
        }
        Err(_) => raw.to_string(),
    }
}

fn format_timestamp(raw: &str) -> String {
    if !raw.contains('T') && !raw.contains(' ') {
        return raw.to_string();
    }
    let cleaned = raw.replace('T', " ").replace('Z', "");
    // Strip fractional seconds: find '.' at or after index 10.
    if cleaned.len() > 10 {
        if let Some(rel) = cleaned[10..].find('.') {
            return cleaned[..10 + rel].to_string();
        }
    }
    cleaned
}

/// Convert Modified Julian Date to ISO date string.
/// Uses the standard formula: Unix seconds = (MJD - 40587.0) * 86400.0
fn mjd_to_date(value: &str) -> String {
    match value.parse::<f64>() {
        Ok(mjd) => {
            let unix_seconds = (mjd - 40587.0) * 86400.0;
            let dt = chrono::DateTime::from_timestamp(unix_seconds as i64, 0);
            match dt {
                Some(dt) => dt.format("%Y-%m-%d").to_string(),
                None => value.to_string(),
            }
        }
        Err(_) => value.to_string(),
    }
}

/// Format integration time with automatic unit selection.
/// Matches Windows: use integer form if close to integer (abs(v - round(v)) < 0.01).
fn format_integration_time(value: &str) -> String {
    match value.parse::<f64>() {
        Ok(secs) => {
            let (val, unit) = if secs >= 3600.0 {
                (secs / 3600.0, "h")
            } else if secs >= 60.0 {
                (secs / 60.0, "m")
            } else {
                (secs, "s")
            };
            if (val - val.round()).abs() < 0.01 {
                format!("{}{}", val.round() as i64, unit)
            } else {
                format!("{:.1}{}", val, unit)
            }
        }
        Err(_) => value.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #[test]
    fn the_id_column_is_visible_without_opening_a_dialog() {
        // obsid is the handle every other tool wants — paste it into
        // get_observation_details, or search for it again next week. It was
        // absent from the default set, so the reader had to discover through the
        // Columns dialog that the results even had one.
        assert!(super::DEFAULT_VISIBLE.contains(&"obsid"));
    }

    #[test]
    fn the_default_visible_set_matches_the_reference() {
        // CellFormatter.DefaultVisibleKeys, minus the two virtual columns.
        let mut ours = super::DEFAULT_VISIBLE.to_vec();
        ours.sort_unstable();
        let mut theirs = vec![
            "collection",
            "targetname",
            "ra(j20000)",
            "dec(j20000)",
            "startdate",
            "instrument",
            "filter",
            "callev",
            "obstype",
            "proposalid",
            "piname",
            "obsid",
        ];
        theirs.sort_unstable();
        assert_eq!(ours, theirs);
    }

    #[test]
    fn a_column_is_as_wide_as_what_it_holds() {
        // A sexagesimal coordinate needs more room than a filter name; one width
        // for all 41 columns spent it in the wrong places.
        assert!(super::column_width("targetname") > super::column_width("filter"));
        assert!(super::column_width("ra(j20000)") > super::column_width("callev"));
        // Anything unlisted still gets the reference's default rather than 0.
        assert_eq!(super::column_width("some_new_tap_column"), 80);
    }

    #[test]
    fn every_column_is_wide_enough_to_read() {
        // Below this the ellipsis eats every value, which defeats a grid whose
        // point is comparing cells at a glance.
        for key in ["filter", "callev", "band", "inttime", "unknown"] {
            assert!(super::column_width(key) >= 60, "{key}");
        }
    }

    use super::*;

    #[test]
    fn parse_csv_basic() {
        let csv = "col1,col2,col3\nval1,val2,val3\na,b,c";
        let result = parse_csv(csv);
        assert_eq!(result.columns, vec!["col1", "col2", "col3"]);
        assert_eq!(result.total_rows(), 2);
        assert_eq!(result.rows[0].get("col1"), "val1");
        assert_eq!(result.rows[1].get("col3"), "c");
    }

    #[test]
    fn aliased_column_is_reachable_by_cleaned_key_for_filter_and_sort() {
        // Regression: filter/sort key by the CLEANED key ("targetname"), but values
        // were only stored under the raw header ("Target Name") → always missed.
        let csv = "Target Name,RA (deg)\nM31,10.68\nNGC 224,10.68";
        let result = parse_csv(csv);
        // Raw header still works (cell rendering path).
        assert_eq!(result.rows[0].get("Target Name"), "M31");
        // Cleaned key now works (filter/sort path).
        assert_eq!(result.rows[0].get(&clean_key("Target Name")), "M31");
        assert_eq!(result.rows[0].get(&clean_key("RA (deg)")), "10.68");
        assert!(!clean_key("Target Name").is_empty());
    }

    #[test]
    fn parse_csv_quoted_fields() {
        let csv = "name,desc\n\"hello, world\",test";
        let result = parse_csv(csv);
        assert_eq!(result.rows[0].get("name"), "hello, world");
    }

    #[test]
    fn parse_csv_escaped_double_quotes() {
        // RFC 4180: doubled quotes inside quoted fields represent a literal quote
        let csv = "name,desc\n\"value with \"\"quotes\"\"\",test";
        let result = parse_csv(csv);
        assert_eq!(result.rows[0].get("name"), "value with \"quotes\"");
    }

    #[test]
    fn parse_csv_empty() {
        let result = parse_csv("");
        assert_eq!(result.total_rows(), 0);
    }

    #[test]
    fn parse_resolver() {
        let text = "ra=83.633\ndec=22.014\ncoordsys=ICRS\noType=SNR\nservice=SIMBAD";
        let result = parse_resolver_response(text, "Crab Nebula").unwrap();
        assert!((result.ra - 83.633).abs() < 0.001);
        assert!((result.dec - 22.014).abs() < 0.001);
        assert_eq!(result.service.as_deref(), Some("SIMBAD"));
    }

    #[test]
    fn parse_resolver_missing_coords() {
        let text = "service=SIMBAD\noType=unknown";
        assert!(parse_resolver_response(text, "xxx").is_none());
    }

    #[test]
    fn parse_resolver_stamps_resolved_at_epoch() {
        // Provenance: a successful resolution carries a non-empty RFC-3339 epoch so
        // the export provenance line can show a real timestamp instead of "unknown".
        let text = "ra=83.633\ndec=22.014\nservice=NED";
        let r = parse_resolver_response(text, "Crab").unwrap();
        assert_eq!(r.service.as_deref(), Some("NED"));
        let epoch = r.resolved_at.expect("resolved_at should be stamped");
        assert!(!epoch.trim().is_empty());
        // Round-trips through chrono as a valid RFC-3339 instant.
        assert!(chrono::DateTime::parse_from_rfc3339(&epoch).is_ok());
    }

    #[test]
    fn form_state_spectral_sampling_raw_defaults_absent() {
        // New optional operator-aware field defaults to None and survives JSON.
        let s = SearchFormState::new();
        assert!(s.spectral_sampling_raw.is_none());
        let json = serde_json::to_string(&s).unwrap();
        let back: SearchFormState = serde_json::from_str(&json).unwrap();
        assert!(back.spectral_sampling_raw.is_none());
    }

    #[test]
    fn form_state_summary() {
        let mut state = SearchFormState::new();
        state.target = "M31".to_string();
        state.collection = "JWST".to_string();
        assert_eq!(state.summary(), "M31, JWST");
    }

    #[test]
    fn form_state_summary_empty() {
        let state = SearchFormState::new();
        assert_eq!(state.summary(), "All observations");
    }

    #[test]
    fn datalink_file_methods() {
        let f = DataLinkFile {
            url: "https://example.com/path/file.fits".to_string(),
            semantics: "#this".to_string(),
            content_type: "application/fits".to_string(),
            size: Some(1_048_576),
            description: "Science data".to_string(),
        };
        assert!(f.is_science_data());
        assert!(!f.is_preview());
        assert_eq!(f.filename(), "file.fits");
        assert_eq!(f.size_display(), "1.0 MB");
    }

    #[test]
    fn format_cell_degrees() {
        assert_eq!(
            format_cell("83.633212345", ColumnFormat::Degrees5),
            "83.63321"
        );
    }

    #[test]
    fn format_cell_calibration() {
        assert_eq!(
            format_cell("1", ColumnFormat::CalibrationLevel),
            "Calibrated"
        );
        assert_eq!(format_cell("0", ColumnFormat::CalibrationLevel), "Raw");
    }

    #[test]
    fn format_cell_integration_time() {
        assert_eq!(format_cell("30.0", ColumnFormat::IntegrationTime), "30s");
        assert_eq!(format_cell("3600.0", ColumnFormat::IntegrationTime), "1h");
        assert_eq!(format_cell("90.5", ColumnFormat::IntegrationTime), "1.5m");
    }

    #[test]
    fn mjd_to_date_epoch() {
        // MJD 51544.0 = 2000-01-01 00:00 UTC (using Unix epoch: (51544-40587)*86400)
        let result = mjd_to_date("51544.0");
        assert_eq!(result, "2000-01-01");
        // MJD 40587.0 = Unix epoch = 1970-01-01
        let result2 = mjd_to_date("40587.0");
        assert_eq!(result2, "1970-01-01");
    }

    #[test]
    fn format_cell_with_unit_ra_defaults_to_sexagesimal() {
        // 150 deg == 10h; default (None) renders sexagesimal.
        assert_eq!(
            format_cell_with_unit("RA (J2000.0)", "150.0", None),
            "10:00:00.00"
        );
        // Explicit degrees unit renders fixed decimals.
        assert_eq!(
            format_cell_with_unit("RA (J2000.0)", "150.0", Some("degrees")),
            "150.000000"
        );
    }

    #[test]
    fn format_cell_with_unit_dec_sign_and_degrees() {
        assert_eq!(
            format_cell_with_unit("Dec. (J2000.0)", "22.014", None),
            sexagesimal::format_dec(22.014)
        );
        assert_eq!(
            format_cell_with_unit("Dec. (J2000.0)", "22.014", Some("degrees")),
            "+22.014000"
        );
    }

    #[test]
    fn format_cell_with_unit_dates_mjd_vs_calendar() {
        // Calendar (default) converts MJD → ISO date.
        assert_eq!(
            format_cell_with_unit("Start Date", "51544.0", None),
            "2000-01-01"
        );
        // MJD unit passes the raw number through.
        assert_eq!(
            format_cell_with_unit("Start Date", "51544.0", Some("mjd")),
            "51544.0"
        );
    }

    #[test]
    fn format_cell_with_unit_spectral_and_passthrough() {
        assert_eq!(
            format_cell_with_unit("Min. Wavelength", "5e-7", Some("nm")),
            "500.0 nm"
        );
        // A non-menu column ignores any unit and returns the trimmed raw value.
        assert_eq!(
            format_cell_with_unit("Instrument", "  NIRCam ", Some("nm")),
            "NIRCam"
        );
        // Empty → empty.
        assert_eq!(format_cell_with_unit("Instrument", "   ", None), "");
    }

    #[test]
    fn recent_search_resolver_provenance_roundtrips() {
        let r = RecentSearch {
            resolver_service_used: Some("SIMBAD".to_string()),
            resolution_epoch: Some("2026-07-08T00:00:00Z".to_string()),
            ..Default::default()
        };
        let json = serde_json::to_string(&r).unwrap();
        let back: RecentSearch = serde_json::from_str(&json).unwrap();
        assert_eq!(back.resolver_service_used.as_deref(), Some("SIMBAD"));
        assert_eq!(
            back.resolution_epoch.as_deref(),
            Some("2026-07-08T00:00:00Z")
        );

        // Legacy payloads that predate the provenance fields still deserialize
        // (serde default → None). Start from a full serialization and drop the
        // two new keys so the rest of the required fields remain valid.
        let mut value: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&RecentSearch::default()).unwrap())
                .unwrap();
        let obj = value.as_object_mut().unwrap();
        obj.remove("resolver_service_used");
        obj.remove("resolution_epoch");
        let legacy: RecentSearch = serde_json::from_value(value).unwrap();
        assert!(legacy.resolver_service_used.is_none());
        assert!(legacy.resolution_epoch.is_none());
    }

    #[test]
    fn verbatim_range_text_survives_a_save_load_round_trip() {
        // Restoring a saved search must reproduce what the user TYPED. The numeric
        // min/max fields cannot: `parse_range_minmax` maps `>5` and `>=5` to the
        // same `(Some(5), None)`, so the operator is only recoverable from the
        // verbatim text.
        let state = SearchFormState {
            integration_time_raw: "> 300".to_string(),
            time_span_raw: "1..7".to_string(),
            spectral_coverage_raw: "<= 500".to_string(),
            resolving_power_raw: ">= 1000".to_string(),
            bandpass_width_raw: "10".to_string(),
            rest_frame_energy_raw: "0.5..2.5".to_string(),
            pixel_scale_raw: Some("0.1..1.0".to_string()),
            ..Default::default()
        };

        let json = serde_json::to_string(&state).unwrap();
        let back: SearchFormState = serde_json::from_str(&json).unwrap();

        assert_eq!(back.integration_time_raw, "> 300");
        assert_eq!(back.time_span_raw, "1..7");
        assert_eq!(back.spectral_coverage_raw, "<= 500");
        assert_eq!(back.resolving_power_raw, ">= 1000");
        assert_eq!(back.bandpass_width_raw, "10");
        assert_eq!(back.rest_frame_energy_raw, "0.5..2.5");
        assert_eq!(back.pixel_scale_raw.as_deref(), Some("0.1..1.0"));
    }

    #[test]
    fn a_search_saved_by_an_older_build_still_loads() {
        // A recent search persists this whole struct, so a record written before a
        // field existed must still deserialize — otherwise adding one field makes
        // every previously saved search vanish from the sidebar with no error.
        // The container-level `#[serde(default)]` is what guarantees it.
        let legacy =
            r#"{"target":"M31","resolver_service":"ALL","search_radius":0.5,"max_records":100}"#;
        let back: SearchFormState = serde_json::from_str(legacy).unwrap();
        assert_eq!(back.target, "M31");
        assert_eq!(back.max_records, 100);
        // Absent fields fall back to their defaults rather than failing the parse.
        assert_eq!(back.integration_time_raw, "");
        assert!(back.pixel_scale_raw.is_none());
        assert_eq!(back.wavelength_unit, "");
    }

    #[test]
    fn form_state_pixel_scale_raw_defaults_absent() {
        // New optional fields default to None and survive a JSON round-trip.
        let s = SearchFormState::new();
        assert!(s.pixel_scale_raw.is_none());
        assert!(s.resolver_service_used.is_none());
        let json = serde_json::to_string(&s).unwrap();
        let back: SearchFormState = serde_json::from_str(&json).unwrap();
        assert!(back.pixel_scale_raw.is_none());
    }

    #[test]
    fn column_visibility_default_applies_without_an_override() {
        let overrides = std::collections::HashMap::new();
        assert!(column_is_visible(&overrides, "collection", true));
        assert!(!column_is_visible(&overrides, "obsid", false));
    }

    #[test]
    fn an_override_can_reveal_a_non_default_column() {
        // The whole point: a hide-only model could never turn `obsid` on, so most
        // of the 41 TAP columns were unreachable from the column dialog.
        let mut overrides = std::collections::HashMap::new();
        overrides.insert("obsid".to_string(), true);
        assert!(column_is_visible(&overrides, "obsid", false));
    }

    #[test]
    fn an_override_can_hide_a_default_column() {
        let mut overrides = std::collections::HashMap::new();
        overrides.insert("collection".to_string(), false);
        assert!(!column_is_visible(&overrides, "collection", true));
    }

    #[test]
    fn every_default_column_is_a_real_column_key() {
        // A typo in DEFAULT_VISIBLE would silently hide a column forever.
        let all = default_columns();
        for key in DEFAULT_VISIBLE {
            assert!(
                all.iter().any(|c| c.key == *key),
                "DEFAULT_VISIBLE names `{key}`, which is not a known column"
            );
        }
    }

    #[test]
    fn default_columns_has_entries() {
        let cols = default_columns();
        assert!(cols.len() > 10);
        assert!(cols[0].visible);
        assert_eq!(cols[0].key, "collection");
    }

    /// A DataLink row set in the order CADC really returns them: the thumbnail
    /// and preview rows come FIRST, ahead of the science data.
    fn mixed_datalink() -> DataLinkResult {
        let row = |sem: &str, url: &str, ct: &str| DataLinkFile {
            url: url.to_string(),
            semantics: sem.to_string(),
            content_type: ct.to_string(),
            size: Some(1024),
            description: String::new(),
        };
        DataLinkResult {
            publisher_id: "ivo://cadc.nrc.ca/CFHT?1".to_string(),
            download_url: None,
            files: vec![
                row("#thumbnail", "https://x/thumb.png", "image/png"),
                row("#preview", "https://x/prev.png", "image/png"),
                row("#this", "https://x/science.fits", "application/fits"),
                row("#this", "https://x/science_mom0.fits", "application/fits"),
                row("#auxiliary", "https://x/weight.fits", "application/fits"),
            ],
        }
    }

    #[test]
    fn artifact_index_zero_is_the_first_science_file_not_the_first_row() {
        // The bug this guards: previews and thumbnails sharing the indexed list.
        // With the raw rows, index 0 is a THUMBNAIL — so an agent asking to
        // download "the first artifact" got a PNG instead of the science frame.
        let dl = mixed_datalink();
        let direct = dl.direct_files();
        assert_eq!(direct.len(), 2, "only the #this rows are science files");
        assert_eq!(direct[0].url, "https://x/science.fits");
        assert_eq!(direct[1].url, "https://x/science_mom0.fits");
    }

    #[test]
    fn previews_and_thumbnails_are_reported_separately() {
        let dl = mixed_datalink();
        assert_eq!(dl.preview_urls(), vec!["https://x/prev.png".to_string()]);
        assert_eq!(dl.thumbnail_urls(), vec!["https://x/thumb.png".to_string()]);
    }

    #[test]
    fn an_unfamiliar_semantics_row_is_kept_out_of_the_way() {
        // The reference discards these while parsing. Keeping them loses no
        // data, and because they are not in direct_files they cannot shift an
        // artifactIndex.
        let dl = mixed_datalink();
        let other = dl.other_files();
        assert_eq!(other.len(), 1);
        assert_eq!(other[0].semantics, "#auxiliary");
        assert!(
            !dl.direct_files()
                .iter()
                .any(|f| f.semantics == "#auxiliary"),
            "an auxiliary row must never occupy an artifactIndex slot"
        );
    }

    #[test]
    fn a_mislabelled_preview_is_not_treated_as_an_image() {
        // A #preview row whose content type is not an image is a data product
        // wearing the wrong label; rendering it as a picture would fail. It
        // falls through to the other bucket rather than the preview list.
        let dl = DataLinkResult {
            publisher_id: "ivo://x?1".to_string(),
            download_url: None,
            files: vec![DataLinkFile {
                url: "https://x/not-an-image.fits".to_string(),
                semantics: "#preview".to_string(),
                content_type: "application/fits".to_string(),
                size: None,
                description: String::new(),
            }],
        };
        assert!(dl.preview_urls().is_empty());
        assert_eq!(dl.other_files().len(), 1);
    }
}

#[cfg(test)]
mod real_tap_response {
    //! The pipeline, end to end, against a response CADC actually sent.
    //!
    //! Every synthetic CSV in these tests writes headers the way a person would
    //! (`RA (J2000.0)`). TAP does not: an `AS "RA (J2000.0)"` alias comes back
    //! CSV-quoted around a value that already contains quotes —
    //! `"""RA (J2000.0)"""` — so the header the app parses is `"RA (J2000.0)"`,
    //! quotes and all. A column whose key or lookup disagreed by those two
    //! characters would render blank in every row, which is what a "corrupted"
    //! table looks like.
    use super::*;

    const SAMPLE: &str = include_str!("../../tests/fixtures/tap_caom2_sample.csv");

    #[test]
    fn every_column_the_service_sent_resolves_to_its_own_value() {
        let results = parse_csv(SAMPLE);
        let columns = build_columns_from_headers(&results.columns);
        assert_eq!(columns.len(), 41, "the reference SELECT has 41 columns");
        assert!(!results.rows.is_empty(), "the fixture has data rows");

        // Position of each column in the raw CSV, so we can check the value the
        // app looks up is the value in that column — not merely non-empty.
        let row = &results.rows[0];
        let raw_first_row: Vec<&str> = SAMPLE.lines().nth(1).unwrap().split(',').collect();
        let mut unresolved = Vec::new();
        for (i, col) in columns.iter().enumerate() {
            match row.values.get(&col.header) {
                // Only compare where the naive split is trustworthy (no quoted
                // commas before it); the point is that the lookup HITS.
                Some(v) => {
                    if i < raw_first_row.len() && !raw_first_row[i].starts_with('"') {
                        assert_eq!(v, raw_first_row[i], "column {} is off by one", col.key);
                    }
                }
                None => unresolved.push(col.key.clone()),
            }
        }
        assert!(
            unresolved.is_empty(),
            "columns whose header does not resolve to a value — they render blank \
             in every row: {unresolved:?}"
        );
    }

    #[test]
    fn the_aliased_columns_keep_their_human_labels() {
        let results = parse_csv(SAMPLE);
        let columns = build_columns_from_headers(&results.columns);
        let labels: Vec<&str> = columns.iter().map(|c| c.display_name.as_str()).collect();
        // The quotes TAP wraps an alias in must not reach the column header.
        for label in &labels {
            assert!(!label.contains('"'), "a column is labelled {label:?}");
        }
        assert!(labels.contains(&"RA (J2000.0)"), "{labels:?}");
        assert!(labels.contains(&"Target Name"), "{labels:?}");
    }

    #[test]
    fn a_formatted_column_is_recognised_through_the_services_quoting() {
        // `format_for_key` dispatches on the cleaned key. If TAP's extra quotes
        // survived cleaning, RA/Dec would render as raw degrees and dates as
        // bare MJD numbers.
        let results = parse_csv(SAMPLE);
        let columns = build_columns_from_headers(&results.columns);
        let by_key = |k: &str| columns.iter().find(|c| c.key == k).cloned();
        assert!(
            matches!(
                by_key("ra(j20000)").map(|c| c.format),
                Some(ColumnFormat::Degrees5)
            ),
            "RA is not recognised as a coordinate column"
        );
        assert!(
            matches!(
                by_key("startdate").map(|c| c.format),
                Some(ColumnFormat::MjdToDate)
            ),
            "Start Date is not recognised as a date column"
        );
    }
}
