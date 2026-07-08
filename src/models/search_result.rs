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
    pub query: Option<String>,
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
    #[serde(default)]
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
    header
        .replace(['"', ' ', '.'], "")
        .trim()
        .to_lowercase()
}

/// Default visible keys (cleaned form) matching the Windows app.
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
    "datatype",
    "band",
];

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
pub fn parse_csv(csv: &str, query: Option<&str>) -> SearchResults {
    let mut result = SearchResults {
        query: query.map(|s| s.to_string()),
        ..Default::default()
    };

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
            if mag < 0.001 || mag > 1e6 {
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
            if mag < 0.001 || mag > 1e6 {
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
    use super::*;

    #[test]
    fn parse_csv_basic() {
        let csv = "col1,col2,col3\nval1,val2,val3\na,b,c";
        let result = parse_csv(csv, None);
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
        let result = parse_csv(csv, None);
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
        let result = parse_csv(csv, None);
        assert_eq!(result.rows[0].get("name"), "hello, world");
    }

    #[test]
    fn parse_csv_escaped_double_quotes() {
        // RFC 4180: doubled quotes inside quoted fields represent a literal quote
        let csv = "name,desc\n\"value with \"\"quotes\"\"\",test";
        let result = parse_csv(csv, None);
        assert_eq!(result.rows[0].get("name"), "value with \"quotes\"");
    }

    #[test]
    fn parse_csv_empty() {
        let result = parse_csv("", None);
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
        let mut r = RecentSearch::default();
        r.resolver_service_used = Some("SIMBAD".to_string());
        r.resolution_epoch = Some("2026-07-08T00:00:00Z".to_string());
        let json = serde_json::to_string(&r).unwrap();
        let back: RecentSearch = serde_json::from_str(&json).unwrap();
        assert_eq!(back.resolver_service_used.as_deref(), Some("SIMBAD"));
        assert_eq!(back.resolution_epoch.as_deref(), Some("2026-07-08T00:00:00Z"));

        // Legacy payloads that predate the provenance fields still deserialize
        // (serde default → None). Start from a full serialization and drop the
        // two new keys so the rest of the required fields remain valid.
        let mut value: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&RecentSearch::default()).unwrap()).unwrap();
        let obj = value.as_object_mut().unwrap();
        obj.remove("resolver_service_used");
        obj.remove("resolution_epoch");
        let legacy: RecentSearch = serde_json::from_value(value).unwrap();
        assert!(legacy.resolver_service_used.is_none());
        assert!(legacy.resolution_epoch.is_none());
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
    fn default_columns_has_entries() {
        let cols = default_columns();
        assert!(cols.len() > 10);
        assert!(cols[0].visible);
        assert_eq!(cols[0].key, "collection");
    }
}
