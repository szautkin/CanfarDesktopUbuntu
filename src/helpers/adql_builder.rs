use crate::helpers::range_parser::{self, ParsedRange, RangeOp};
use crate::helpers::sexagesimal;
use crate::helpers::unit_converter;
use crate::models::search_result::SearchFormState;

// Exact 41-column SELECT matching the Windows implementation
const SELECT_COLUMNS: &str = r#"
    Observation.observationID,
    Observation.collection,
    Observation.sequenceNumber,
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
    Observation.observationID AS "Obs. ID",
    Plane.energy_bounds_lower AS "Min. Wavelength",
    Plane.energy_bounds_upper AS "Max. Wavelength",
    AREA(Plane.position_bounds) AS "Field of View",
    Plane.position_sampleSize AS "Pixel Scale",
    Plane.energy_resolvingPower AS "Resolving Power",
    Plane.time_bounds_upper AS "End Date",
    Plane.dataProductType AS "Data Type",
    Observation.target_moving AS "Moving Target",
    Plane.provenance_name AS "Provenance Name",
    Observation.intent AS "Intent",
    Observation.target_type AS "Target Type",
    Observation.algorithm_name AS "Algorithm",
    Observation.proposal_title AS "Proposal Title",
    Observation.proposal_keywords AS "Proposal Keywords",
    Plane.position_resolution AS "Spatial Resolution",
    Plane.energy_transition_species AS "Molecule",
    Plane.energy_transition_transition AS "Transition",
    Plane.energy_emBand AS "Band",
    Plane.energy_bounds_width AS "Bandpass Width",
    Plane.energy_sampleSize AS "Energy Sample Size",
    Plane.energy_restwav AS "Rest Frame Energy",
    Plane.time_bounds_width AS "Time Span",
    Observation.requirements_flag AS "Quality",
    Plane.publisherID
"#;

const FROM_CLAUSE: &str =
    "caom2.Plane AS Plane JOIN caom2.Observation AS Observation ON Plane.obsID = Observation.obsID";

const QUALITY_FILTER: &str = "( Plane.quality_flag IS NULL OR Plane.quality_flag != 'junk' )";

/// Build an ADQL query from the search form state.
pub fn build(state: &SearchFormState) -> String {
    let mut clauses = vec![QUALITY_FILTER.to_string()];

    add_observation_clauses(state, &mut clauses);
    add_spatial_clauses(state, &mut clauses);
    add_temporal_clauses(state, &mut clauses);
    add_spectral_clauses(state, &mut clauses);
    add_data_train_clauses(state, &mut clauses);

    let where_str = clauses.join("\nAND ");

    format!(
        "SELECT TOP {}\n{}\nFROM {}\nWHERE {}",
        state.max_records, SELECT_COLUMNS, FROM_CLAUSE, where_str
    )
}

// ---------------------------------------------------------------------------
// Observation constraints
// ---------------------------------------------------------------------------

fn add_observation_clauses(state: &SearchFormState, clauses: &mut Vec<String>) {
    add_text_like(clauses, "Observation.observationID", &state.observation_id);
    add_text_like(clauses, "Observation.proposal_pi", &state.proposal_pi);
    add_text_like(clauses, "Observation.proposal_id", &state.proposal_id);
    add_text_like(clauses, "Observation.proposal_title", &state.proposal_title);
    add_text_like(
        clauses,
        "Observation.proposal_keywords",
        &state.proposal_keywords,
    );

    if !state.intent.is_empty() {
        clauses.push(format!(
            "Observation.intent = '{}'",
            escape_sql(&state.intent)
        ));
    }

    if state.public_only {
        clauses.push("Plane.dataRelease <= GETDATE()".to_string());
    }
}

// ---------------------------------------------------------------------------
// Spatial constraints
// ---------------------------------------------------------------------------

fn add_spatial_clauses(state: &SearchFormState, clauses: &mut Vec<String>) {
    // Precedence mirrors the Windows/macOS SpatialBuilder: a coordinate-range box,
    // then a direct coordinate pair (decimal OR colon-sexagesimal), then resolved
    // name coordinates, then a plain target-name substring match.
    let target = state.target.trim();
    if !target.is_empty() {
        if let Some((ra_lo, ra_hi, dec_lo, dec_hi)) = try_parse_coord_range(target) {
            clauses.push(format!(
                "INTERSECTS( RANGE_S2D({}, {}, {}, {}), Plane.position_bounds ) = 1",
                ra_lo, ra_hi, dec_lo, dec_hi
            ));
        } else if let Some((ra, dec, radius)) =
            try_parse_coordinate_pair(target, state.search_radius)
        {
            clauses.push(circle_clause(ra, dec, radius));
        } else if let (Some(ra), Some(dec)) = (state.resolved_ra, state.resolved_dec) {
            clauses.push(circle_clause(ra, dec, state.search_radius));
        } else {
            // Unresolved target name — fall back to name search.
            add_text_like(clauses, "Observation.target_name", target);
        }
    } else if let (Some(ra), Some(dec)) = (state.resolved_ra, state.resolved_dec) {
        clauses.push(circle_clause(ra, dec, state.search_radius));
    }

    // Pixel scale: an operator-aware raw text field (RANGE syntax) takes precedence
    // over the legacy numeric max.
    if let Some(raw) = state
        .pixel_scale_raw
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if let Some(range) = range_parser::parse_range(raw) {
            add_converted_range_clause(
                "Plane.position_sampleSize",
                &range,
                &state.pixel_scale_unit,
                clauses,
                |v, u| v.trim().parse::<f64>().ok().and_then(|n| unit_converter::to_degrees(n, u)),
            );
        }
    } else if let Some(ps) = state.pixel_scale_max {
        let deg = unit_converter::to_degrees(ps, &state.pixel_scale_unit).unwrap_or(ps / 3600.0);
        clauses.push(format!("Plane.position_sampleSize <= {}", deg));
    }
}

/// `INTERSECTS( CIRCLE('ICRS', ra, dec, radius), Plane.position_bounds ) = 1`.
fn circle_clause(ra: f64, dec: f64, radius: f64) -> String {
    format!(
        "INTERSECTS( CIRCLE('ICRS', {}, {}, {}), Plane.position_bounds ) = 1",
        ra, dec, radius
    )
}

/// Parse a coordinate-range box `"raLo..raHi decLo..decHi"` (decimal degrees) for
/// a `RANGE_S2D` search. Returns `None` unless the input is exactly two `..`-ranges.
fn try_parse_coord_range(input: &str) -> Option<(f64, f64, f64, f64)> {
    let parts: Vec<&str> = input.split_whitespace().collect();
    if parts.len() != 2 || !parts[0].contains("..") || !parts[1].contains("..") {
        return None;
    }
    let ra: Vec<&str> = parts[0].split("..").collect();
    let dec: Vec<&str> = parts[1].split("..").collect();
    if ra.len() != 2 || dec.len() != 2 {
        return None;
    }
    Some((
        ra[0].trim().parse().ok()?,
        ra[1].trim().parse().ok()?,
        dec[0].trim().parse().ok()?,
        dec[1].trim().parse().ok()?,
    ))
}

/// Parse a direct coordinate pair `"RA DEC [radius]"` from the target field. RA/Dec
/// may be decimal degrees (`"10.68 41.27"`) or colon-delimited sexagesimal
/// (`"10:42:44 +41:16:09"`). Radius is optional with an optional unit
/// (deg/arcmin/arcsec or `'`). Returns `None` for a plain name like `"M31"`.
fn try_parse_coordinate_pair(input: &str, default_radius: f64) -> Option<(f64, f64, f64)> {
    let parts: Vec<&str> = input.split_whitespace().collect();
    if parts.len() < 2 {
        return None;
    }
    let ra = try_parse_angle(parts[0], true)?;
    let dec = try_parse_angle(parts[1], false)?;
    if !(0.0..=360.0).contains(&ra) || !(-90.0..=90.0).contains(&dec) {
        return None;
    }
    let mut radius = default_radius;
    if parts.len() >= 3 {
        let parsed = parse_radius(parts[2]);
        if parsed > 0.0 {
            radius = parsed;
        }
    }
    Some((ra, dec, radius))
}

/// Decimal degrees, or colon-delimited sexagesimal (RA in hours, Dec in degrees).
fn try_parse_angle(token: &str, is_ra: bool) -> Option<f64> {
    if let Ok(v) = token.parse::<f64>() {
        return Some(v); // decimal degrees
    }
    if token.contains(':') {
        return if is_ra {
            sexagesimal::parse_ra(token)
        } else {
            sexagesimal::parse_dec(token)
        };
    }
    None
}

/// Parse a radius token with optional unit to degrees. Returns 0.0 if unparseable.
/// `'` is treated as arcmin (mirrors the reference `ParseRadius`).
fn parse_radius(input: &str) -> f64 {
    let trimmed = input.trim().replace('\'', "arcmin");
    let lower = trimmed.to_ascii_lowercase();
    let (num, factor) = if let Some(p) = lower.strip_suffix("arcmin") {
        (p, 1.0 / 60.0)
    } else if let Some(p) = lower.strip_suffix("arcsec") {
        (p, 1.0 / 3600.0)
    } else if let Some(p) = lower.strip_suffix("deg") {
        (p, 1.0)
    } else {
        (lower.as_str(), 1.0)
    };
    match num.trim().parse::<f64>() {
        Ok(v) => v * factor,
        Err(_) => 0.0,
    }
}

// ---------------------------------------------------------------------------
// Temporal constraints
// ---------------------------------------------------------------------------

fn add_temporal_clauses(state: &SearchFormState, clauses: &mut Vec<String>) {
    // Date presets — use INTERSECTS(INTERVAL) with MJD (matching Windows)
    let now_mjd = current_mjd();
    match state.date_preset.as_str() {
        "Last 24 hours" => {
            clauses.push(format!(
                "INTERSECTS( INTERVAL({}, {}), Plane.time_bounds_samples ) = 1",
                now_mjd - 1.0,
                now_mjd
            ));
        }
        "Last week" => {
            clauses.push(format!(
                "INTERSECTS( INTERVAL({}, {}), Plane.time_bounds_samples ) = 1",
                now_mjd - 7.0,
                now_mjd
            ));
        }
        "Last month" => {
            clauses.push(format!(
                "INTERSECTS( INTERVAL({}, {}), Plane.time_bounds_samples ) = 1",
                now_mjd - 30.0,
                now_mjd
            ));
        }
        _ => {
            let raw = state.obs_date_raw.trim();
            // Explicit range operators (>, >=, <, <=) on the observation-date field
            // map onto time_bounds comparisons (ported AddDateRangeClause). Plain
            // values and "A..B" ranges keep the existing partial-date expansion.
            let op_range = if raw.is_empty() {
                None
            } else {
                range_parser::parse_range(raw).filter(|r| {
                    matches!(
                        r.op,
                        RangeOp::GreaterThan
                            | RangeOp::GreaterThanOrEqual
                            | RangeOp::LessThan
                            | RangeOp::LessThanOrEqual
                    )
                })
            };
            if let Some(range) = op_range {
                add_date_range_clause(&range, clauses, None);
            } else {
                // Custom date range — try expanding partial dates first
                let (start_str, end_str) = if !raw.is_empty() {
                    expand_date_to_range(raw)
                } else {
                    (state.obs_date_start.clone(), state.obs_date_end.clone())
                };

                let mjd_start = if !start_str.is_empty() {
                    date_to_mjd(&start_str)
                } else {
                    None
                };
                let mjd_end = if !end_str.is_empty() {
                    date_to_mjd(&end_str)
                } else {
                    None
                };

                match (mjd_start, mjd_end) {
                    (Some(lo), Some(hi)) => {
                        clauses.push(format!(
                            "INTERSECTS( INTERVAL({}, {}), Plane.time_bounds_samples ) = 1",
                            lo, hi
                        ));
                    }
                    (Some(lo), None) => {
                        clauses.push(format!("Plane.time_bounds_lower >= {}", lo));
                    }
                    (None, Some(hi)) => {
                        clauses.push(format!("Plane.time_bounds_upper <= {}", hi));
                    }
                    _ => {}
                }
            }
        }
    }

    // Integration time (already converted to seconds in build_form_state)
    if let Some(min) = state.integration_time_min {
        clauses.push(format!("Plane.time_exposure >= {}", min));
    }
    if let Some(max) = state.integration_time_max {
        clauses.push(format!("Plane.time_exposure <= {}", max));
    }

    // Time span (already converted to days in build_form_state)
    if let Some(min) = state.time_span_min {
        clauses.push(format!("Plane.time_bounds_width >= {}", min));
    }
    if let Some(max) = state.time_span_max {
        clauses.push(format!("Plane.time_bounds_width <= {}", max));
    }

    // Data release — full range-operator support (ported AddDateRangeClause with a
    // dedicated column). ">2020", ">=2020-06", "<2021", "2019..2021", or "2020".
    if !state.data_release.trim().is_empty() {
        if let Some(range) = range_parser::parse_range(&state.data_release) {
            add_date_range_clause(&range, clauses, Some("Plane.dataRelease"));
        }
    }
}

/// Apply a parsed date range to the query (port of `ADQLBuilder.AddDateRangeClause`).
///
/// With `column = Some(col)` the bounds compare directly against `col` (MJD);
/// with `column = None` they map onto `Plane.time_bounds_*` / an
/// `INTERSECTS(INTERVAL(...))` overlap. Values are parsed leniently (a bare year
/// or year-month expands to its start/end date).
fn add_date_range_clause(range: &ParsedRange, clauses: &mut Vec<String>, column: Option<&str>) {
    match range.op {
        RangeOp::Between => {
            if let (Some(lo), Some(hi)) = (
                date_to_mjd_flexible(&range.value1),
                range.value2.as_deref().and_then(date_to_mjd_flexible),
            ) {
                push_interval(clauses, column, lo, hi);
            }
        }
        RangeOp::GreaterThan => {
            if let Some(v) = date_to_mjd_flexible(&range.value1) {
                clauses.push(format!(
                    "{} > {}",
                    column.unwrap_or("Plane.time_bounds_lower"),
                    v
                ));
            }
        }
        RangeOp::GreaterThanOrEqual => {
            if let Some(v) = date_to_mjd_flexible(&range.value1) {
                clauses.push(format!(
                    "{} >= {}",
                    column.unwrap_or("Plane.time_bounds_lower"),
                    v
                ));
            }
        }
        RangeOp::LessThan => {
            if let Some(v) = date_to_mjd_flexible(&range.value1) {
                clauses.push(format!(
                    "{} < {}",
                    column.unwrap_or("Plane.time_bounds_upper"),
                    v
                ));
            }
        }
        RangeOp::LessThanOrEqual => {
            if let Some(v) = date_to_mjd_flexible(&range.value1) {
                clauses.push(format!(
                    "{} <= {}",
                    column.unwrap_or("Plane.time_bounds_upper"),
                    v
                ));
            }
        }
        RangeOp::Equals => {
            if let Some((lo, hi)) = expand_date_to_mjd_range(&range.value1) {
                push_interval(clauses, column, lo, hi);
            }
        }
    }
}

/// Between/Equals bounds: a direct `col >= lo AND col <= hi` when a column is
/// given, else an `INTERSECTS(INTERVAL(...))` overlap against the time samples.
fn push_interval(clauses: &mut Vec<String>, column: Option<&str>, lo: f64, hi: f64) {
    match column {
        Some(col) => clauses.push(format!("{} >= {} AND {} <= {}", col, lo, col, hi)),
        None => clauses.push(format!(
            "INTERSECTS( INTERVAL( {}, {} ), Plane.time_bounds_samples ) = 1",
            lo, hi
        )),
    }
}

/// Parse a date value to MJD, expanding a bare year/year-month to its start date.
fn date_to_mjd_flexible(s: &str) -> Option<f64> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    date_to_mjd(t).or_else(|| {
        let (start, _) = expand_single_date(t);
        if start.is_empty() {
            None
        } else {
            date_to_mjd(&start)
        }
    })
}

/// Expand a date value to an inclusive MJD range (`"2020"` → whole year, etc.).
fn expand_date_to_mjd_range(s: &str) -> Option<(f64, f64)> {
    let (start, end) = expand_single_date(s.trim());
    let lo = date_to_mjd(&start)?;
    let hi = if end.is_empty() { lo } else { date_to_mjd(&end)? };
    Some((lo, hi))
}

/// Get the current Modified Julian Date.
fn current_mjd() -> f64 {
    let now = chrono::Utc::now();
    let unix_days = now.timestamp() as f64 / 86400.0;
    unix_days + 40587.0 // MJD epoch offset from Unix epoch
}

/// Expand a partial date string to a full range.
/// "2020" → ("2020-01-01", "2020-12-31")
/// "2020-06" → ("2020-06-01", "2020-06-30")
/// "2020-01-15" → ("2020-01-15", "")
/// "2020..2021" → ("2020-01-01", "2021-12-31")
fn expand_date_to_range(input: &str) -> (String, String) {
    let input = input.trim();

    // Handle range syntax first
    if let Some(dot_idx) = input.find("..") {
        let left = input[..dot_idx].trim();
        let right = input[dot_idx + 2..].trim();
        let (left_start, _) = expand_single_date(left);
        let (right_start, right_end) = expand_single_date(right);
        let start = left_start;
        let end = if right_end.is_empty() {
            right_start
        } else {
            right_end
        };
        return (start, end);
    }

    expand_single_date(input)
}

fn expand_single_date(input: &str) -> (String, String) {
    let input = input.trim();
    if input.is_empty() {
        return (String::new(), String::new());
    }

    // Year only: "2020"
    if input.len() == 4 && input.chars().all(|c| c.is_ascii_digit()) {
        return (
            format!("{}-01-01", input),
            format!("{}-12-31", input),
        );
    }

    // Year-month: "2020-06"
    if input.len() == 7 && input.chars().nth(4) == Some('-') {
        let year: i32 = input[..4].parse().unwrap_or(2000);
        let month: u32 = input[5..7].parse().unwrap_or(1);
        let last_day = last_day_of_month(year, month);
        return (
            format!("{}-01", input),
            format!("{}-{:02}", input, last_day),
        );
    }

    // Full date: return as-is
    (input.to_string(), String::new())
}

fn last_day_of_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 {
                29
            } else {
                28
            }
        }
        _ => 30,
    }
}

// ---------------------------------------------------------------------------
// Spectral constraints
// ---------------------------------------------------------------------------

fn add_spectral_clauses(state: &SearchFormState, clauses: &mut Vec<String>) {
    // Spectral coverage — overlap semantics (matching Windows):
    // Find observations whose energy range overlaps the query range
    if let (Some(wmin), Some(wmax)) = (state.wavelength_min, state.wavelength_max) {
        let lo = convert_spectral_to_metres(wmin, &state.wavelength_unit);
        let hi = convert_spectral_to_metres(wmax, &state.wavelength_unit);
        if let (Some(lo_m), Some(hi_m)) = (lo, hi) {
            // Swap if frequency/energy units cause inversion
            let (lo_m, hi_m) = if lo_m > hi_m { (hi_m, lo_m) } else { (lo_m, hi_m) };
            // Overlap: obs_lower <= query_hi AND query_lo <= obs_upper
            clauses.push(format!(
                "Plane.energy_bounds_lower <= {} AND {} <= Plane.energy_bounds_upper",
                hi_m, lo_m
            ));
        }
    } else {
        // Single-bound spectral constraints
        if let Some(wmin) = state.wavelength_min {
            if let Some(m) = convert_spectral_to_metres(wmin, &state.wavelength_unit) {
                clauses.push(format!("Plane.energy_bounds_lower >= {}", m));
            }
        }
        if let Some(wmax) = state.wavelength_max {
            if let Some(m) = convert_spectral_to_metres(wmax, &state.wavelength_unit) {
                clauses.push(format!("Plane.energy_bounds_upper <= {}", m));
            }
        }
    }

    // Resolving power (dimensionless)
    if let Some(rmin) = state.resolving_power_min {
        clauses.push(format!("Plane.energy_resolvingPower >= {}", rmin));
    }
    if let Some(rmax) = state.resolving_power_max {
        clauses.push(format!("Plane.energy_resolvingPower <= {}", rmax));
    }

    // Bandpass width (convert to meters)
    if let Some(bmin) = state.bandpass_width_min {
        if let Some(m) = convert_spectral_to_metres(bmin, &state.bandpass_width_unit) {
            clauses.push(format!("Plane.energy_bounds_width >= {}", m));
        }
    }
    if let Some(bmax) = state.bandpass_width_max {
        if let Some(m) = convert_spectral_to_metres(bmax, &state.bandpass_width_unit) {
            clauses.push(format!("Plane.energy_bounds_width <= {}", m));
        }
    }

    // Spectral sampling (convert to meters)
    if let Some(ss) = state.spectral_sampling {
        if let Some(m) = convert_spectral_to_metres(ss, &state.spectral_sampling_unit) {
            clauses.push(format!("Plane.energy_sampleSize <= {}", m));
        }
    }

    // Rest frame energy (convert to meters — stored as rest wavelength in CAOM2)
    if let Some(rmin) = state.rest_frame_energy_min {
        if let Some(m) = convert_spectral_to_metres(rmin, &state.rest_frame_energy_unit) {
            clauses.push(format!("Plane.energy_restwav >= {}", m));
        }
    }
    if let Some(rmax) = state.rest_frame_energy_max {
        if let Some(m) = convert_spectral_to_metres(rmax, &state.rest_frame_energy_unit) {
            clauses.push(format!("Plane.energy_restwav <= {}", m));
        }
    }
}

/// Convert a spectral value to metres using the comprehensive unit_converter.
/// Falls back to the old wavelength_to_meters for basic units.
fn convert_spectral_to_metres(value: f64, unit: &str) -> Option<f64> {
    unit_converter::to_metres(value, unit).or_else(|| Some(wavelength_to_meters(value, unit)))
}

// ---------------------------------------------------------------------------
// Data Train constraints
// ---------------------------------------------------------------------------

fn add_data_train_clauses(state: &SearchFormState, clauses: &mut Vec<String>) {
    add_in_clause(clauses, "Observation.collection", &state.collection);
    add_in_clause(clauses, "Observation.instrument_name", &state.instrument);
    add_in_clause(clauses, "Plane.energy_emBand", &state.band);
    add_in_clause(clauses, "Plane.energy_bandpassName", &state.filter_name);
    add_in_clause(clauses, "Plane.calibrationLevel", &state.calibration_level);
    add_in_clause(clauses, "Plane.dataProductType", &state.data_product_type);
    add_in_clause(clauses, "Observation.type", &state.obs_type);
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Add a LIKE clause for text search. Supports wildcards: * → %
fn add_text_like(clauses: &mut Vec<String>, column: &str, value: &str) {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return;
    }

    if trimmed.contains('*') {
        // Wildcard search
        let pattern = escape_like(trimmed).replace('*', "%");
        clauses.push(format!("lower({}) LIKE lower('{}')", column, pattern));
    } else {
        // Substring search
        clauses.push(format!(
            "lower({}) LIKE lower('%{}%')",
            column,
            escape_like(trimmed)
        ));
    }
}

/// Add an IN clause for comma-separated multi-select values.
fn add_in_clause(clauses: &mut Vec<String>, column: &str, value: &str) {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return;
    }

    let values: Vec<&str> = trimmed
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    if values.is_empty() {
        return;
    }

    if values.len() == 1 {
        clauses.push(format!("{} = '{}'", column, escape_sql(values[0])));
    } else {
        let quoted: Vec<String> = values
            .iter()
            .map(|v| format!("'{}'", escape_sql(v)))
            .collect();
        clauses.push(format!("{} IN ({})", column, quoted.join(", ")));
    }
}

/// Apply a parsed numeric range to a column, converting each side via `convert`
/// (port of `ADQLBuilder.AddConvertedRangeClause`). Supports `=`, `<`, `<=`, `>`,
/// `>=` and `A..B`. `Between` sides are normalised low→high.
fn add_converted_range_clause<F>(
    column: &str,
    range: &ParsedRange,
    unit: &str,
    clauses: &mut Vec<String>,
    convert: F,
) where
    F: Fn(&str, &str) -> Option<f64>,
{
    match range.op {
        RangeOp::Between => {
            if let (Some(v1), Some(v2)) = (
                convert(&range.value1, unit),
                range.value2.as_deref().and_then(|s| convert(s, unit)),
            ) {
                let (lo, hi) = if v1 <= v2 { (v1, v2) } else { (v2, v1) };
                clauses.push(format!("{} >= {} AND {} <= {}", column, lo, column, hi));
            }
        }
        RangeOp::GreaterThan => {
            if let Some(v) = convert(&range.value1, unit) {
                clauses.push(format!("{} > {}", column, v));
            }
        }
        RangeOp::GreaterThanOrEqual => {
            if let Some(v) = convert(&range.value1, unit) {
                clauses.push(format!("{} >= {}", column, v));
            }
        }
        RangeOp::LessThan => {
            if let Some(v) = convert(&range.value1, unit) {
                clauses.push(format!("{} < {}", column, v));
            }
        }
        RangeOp::LessThanOrEqual => {
            if let Some(v) = convert(&range.value1, unit) {
                clauses.push(format!("{} <= {}", column, v));
            }
        }
        RangeOp::Equals => {
            if let Some(v) = convert(&range.value1, unit) {
                clauses.push(format!("{} = {}", column, v));
            }
        }
    }
}

/// Convert a wavelength value to meters based on the unit.
fn wavelength_to_meters(value: f64, unit: &str) -> f64 {
    match unit {
        "nm" => value * 1e-9,
        "um" | "μm" | "micron" => value * 1e-6,
        "Angstrom" | "angstrom" | "A" | "Å" => value * 1e-10,
        "mm" => value * 1e-3,
        "cm" => value * 1e-2,
        "m" => value,
        _ => value * 1e-9, // default nm
    }
}

/// Convert a date string (YYYY-MM-DD) to Modified Julian Date.
fn date_to_mjd(date_str: &str) -> Option<f64> {
    let date = chrono::NaiveDate::parse_from_str(date_str.trim(), "%Y-%m-%d").ok()?;
    let y = date.year() as f64;
    let m = date.month() as f64;
    let d = date.day() as f64;

    // Julian Date calculation
    let (y2, m2) = if m <= 2.0 {
        (y - 1.0, m + 12.0)
    } else {
        (y, m)
    };

    let a = (y2 / 100.0).floor();
    let b = 2.0 - a + (a / 4.0).floor();
    let jd = (365.25 * (y2 + 4716.0)).floor() + (30.6001 * (m2 + 1.0)).floor() + d + b - 1524.5;

    Some(jd - 2_400_000.5) // MJD = JD - 2400000.5
}

use chrono::Datelike;

fn escape_sql(s: &str) -> String {
    s.replace('\'', "''")
}

fn escape_like(s: &str) -> String {
    s.replace('\'', "''")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_basic_query() {
        let state = SearchFormState::new();
        let adql = build(&state);
        assert!(adql.contains("SELECT TOP 1000"));
        assert!(adql.contains("FROM caom2.Plane"));
        assert!(adql.contains("quality_flag"));
    }

    #[test]
    fn build_with_coordinates() {
        let mut state = SearchFormState::new();
        state.resolved_ra = Some(83.633);
        state.resolved_dec = Some(22.014);
        state.search_radius = 0.1;
        let adql = build(&state);
        assert!(adql.contains("INTERSECTS"));
        assert!(adql.contains("83.633"));
    }

    #[test]
    fn build_with_collection() {
        let mut state = SearchFormState::new();
        state.collection = "JWST".to_string();
        let adql = build(&state);
        assert!(adql.contains("collection = 'JWST'"));
    }

    #[test]
    fn build_with_multi_collection() {
        let mut state = SearchFormState::new();
        state.collection = "JWST, HST".to_string();
        let adql = build(&state);
        assert!(adql.contains("IN ('JWST', 'HST')"));
    }

    #[test]
    fn build_with_observation_id_wildcard() {
        let mut state = SearchFormState::new();
        state.observation_id = "jw01345*".to_string();
        let adql = build(&state);
        assert!(adql.contains("LIKE"));
        assert!(adql.contains("jw01345%"));
    }

    #[test]
    fn build_with_proposal_pi() {
        let mut state = SearchFormState::new();
        state.proposal_pi = "Smith".to_string();
        let adql = build(&state);
        assert!(adql.contains("proposal_pi"));
        assert!(adql.contains("%Smith%"));
    }

    #[test]
    fn build_with_intent() {
        let mut state = SearchFormState::new();
        state.intent = "science".to_string();
        let adql = build(&state);
        assert!(adql.contains("intent = 'science'"));
    }

    #[test]
    fn build_with_public_only() {
        let mut state = SearchFormState::new();
        state.public_only = true;
        let adql = build(&state);
        assert!(adql.contains("dataRelease <= GETDATE()"));
    }

    #[test]
    fn build_with_date_preset() {
        let mut state = SearchFormState::new();
        state.date_preset = "Last 24 hours".to_string();
        let adql = build(&state);
        assert!(adql.contains("INTERSECTS( INTERVAL("));
        assert!(adql.contains("Plane.time_bounds_samples"));
    }

    #[test]
    fn build_with_wavelength() {
        let mut state = SearchFormState::new();
        state.wavelength_min = Some(400.0);
        state.wavelength_max = Some(700.0);
        state.wavelength_unit = "nm".to_string();
        let adql = build(&state);
        assert!(adql.contains("energy_bounds_lower >= 4e-7") || adql.contains("0.0000004"));
    }

    #[test]
    fn build_with_resolving_power() {
        let mut state = SearchFormState::new();
        state.resolving_power_min = Some(1000.0);
        let adql = build(&state);
        assert!(adql.contains("resolvingPower >= 1000"));
    }

    #[test]
    fn build_unresolved_target_fallback() {
        let mut state = SearchFormState::new();
        state.target = "M31".to_string();
        // resolved_ra/dec are None
        let adql = build(&state);
        assert!(adql.contains("target_name"));
        assert!(adql.contains("%M31%"));
    }

    #[test]
    fn sql_injection_escaped() {
        let mut state = SearchFormState::new();
        state.collection = "test'; DROP TABLE--".to_string();
        let adql = build(&state);
        assert!(adql.contains("test''; DROP TABLE--"));
        assert!(!adql.contains("test'; DROP"));
    }

    #[test]
    fn wavelength_conversion() {
        assert!((wavelength_to_meters(500.0, "nm") - 5e-7).abs() < 1e-15);
        assert!((wavelength_to_meters(5000.0, "Angstrom") - 5e-7).abs() < 1e-15);
        assert!((wavelength_to_meters(0.5, "um") - 5e-7).abs() < 1e-15);
    }

    #[test]
    fn date_to_mjd_known_value() {
        // 2000-01-01 12:00 UT = MJD 51544.5, but date_to_mjd uses midnight
        // 2000-01-01 00:00 UT = MJD 51544.0
        let mjd = date_to_mjd("2000-01-01").unwrap();
        assert!((mjd - 51544.0).abs() < 0.5);
    }

    #[test]
    fn date_to_mjd_invalid() {
        assert!(date_to_mjd("not-a-date").is_none());
    }

    #[test]
    fn spectral_overlap_semantics() {
        let mut state = SearchFormState::new();
        state.wavelength_min = Some(400.0);
        state.wavelength_max = Some(700.0);
        state.wavelength_unit = "nm".to_string();
        let adql = build(&state);
        // Overlap: obs_lower <= query_hi AND query_lo <= obs_upper
        assert!(adql.contains("energy_bounds_lower <="));
        assert!(adql.contains("<= Plane.energy_bounds_upper"));
    }

    #[test]
    fn date_expansion_year() {
        let (start, end) = expand_date_to_range("2020");
        assert_eq!(start, "2020-01-01");
        assert_eq!(end, "2020-12-31");
    }

    #[test]
    fn date_expansion_month() {
        let (start, end) = expand_date_to_range("2020-02");
        assert_eq!(start, "2020-02-01");
        assert_eq!(end, "2020-02-29"); // 2020 is a leap year
    }

    #[test]
    fn date_expansion_range() {
        let (start, end) = expand_date_to_range("2019..2021");
        assert_eq!(start, "2019-01-01");
        assert_eq!(end, "2021-12-31");
    }

    #[test]
    fn build_with_integration_time() {
        let mut state = SearchFormState::new();
        state.integration_time_min = Some(60.0); // 60 seconds
        state.integration_time_max = Some(3600.0);
        let adql = build(&state);
        assert!(adql.contains("time_exposure >= 60"));
        assert!(adql.contains("time_exposure <= 3600"));
    }

    #[test]
    fn build_with_data_train() {
        let mut state = SearchFormState::new();
        state.band = "Optical".to_string();
        state.instrument = "ACS,WFC3".to_string();
        let adql = build(&state);
        assert!(adql.contains("energy_emBand = 'Optical'"));
        assert!(adql.contains("instrument_name IN ('ACS', 'WFC3')"));
    }

    // ── Target coordinate parsing ───────────────────────────────────────────

    #[test]
    fn target_decimal_pair_makes_circle() {
        let mut state = SearchFormState::new();
        state.target = "10.68 41.27".to_string();
        state.search_radius = 0.1;
        let adql = build(&state);
        assert!(adql.contains("CIRCLE('ICRS', 10.68, 41.27, 0.1)"), "{}", adql);
        assert!(adql.contains(", Plane.position_bounds ) = 1"));
        // Not treated as a target-name LIKE (the SELECT still lists target_name).
        assert!(!adql.contains("lower(Observation.target_name)"));
    }

    #[test]
    fn target_sexagesimal_pair_makes_circle() {
        let mut state = SearchFormState::new();
        state.target = "10:00:00 +41:16:00".to_string();
        let adql = build(&state);
        // 10h == 150 deg.
        assert!(adql.contains("CIRCLE('ICRS', 150"), "{}", adql);
    }

    #[test]
    fn target_pair_with_radius_unit() {
        let mut state = SearchFormState::new();
        state.target = "10.0 20.0 5arcmin".to_string();
        let adql = build(&state);
        // 5 arcmin == 5/60 deg.
        let deg = 5.0 / 60.0;
        assert!(adql.contains(&format!("CIRCLE('ICRS', 10, 20, {})", deg)), "{}", adql);
    }

    #[test]
    fn target_coord_range_makes_range_s2d() {
        let mut state = SearchFormState::new();
        state.target = "10..20 30..40".to_string();
        let adql = build(&state);
        assert!(adql.contains("RANGE_S2D(10, 20, 30, 40)"), "{}", adql);
        assert!(adql.contains("INTERSECTS( RANGE_S2D"));
    }

    #[test]
    fn target_name_falls_back_to_like_then_resolved() {
        // Plain name, unresolved → LIKE.
        let mut state = SearchFormState::new();
        state.target = "M31".to_string();
        let adql = build(&state);
        assert!(adql.contains("lower(Observation.target_name) LIKE"), "{}", adql);
        assert!(adql.contains("%m31%") || adql.contains("%M31%"));

        // Plain name, resolved coords present → CIRCLE from resolved coords.
        let mut state2 = SearchFormState::new();
        state2.target = "M31".to_string();
        state2.resolved_ra = Some(10.68);
        state2.resolved_dec = Some(41.27);
        let adql2 = build(&state2);
        assert!(adql2.contains("CIRCLE('ICRS', 10.68, 41.27"), "{}", adql2);
        assert!(!adql2.contains("lower(Observation.target_name)"));
    }

    #[test]
    fn parse_radius_units() {
        assert!((parse_radius("0.5deg") - 0.5).abs() < 1e-12);
        assert!((parse_radius("30arcsec") - 30.0 / 3600.0).abs() < 1e-12);
        assert!((parse_radius("2'") - 2.0 / 60.0).abs() < 1e-12);
        assert!((parse_radius("1.5") - 1.5).abs() < 1e-12); // bare → degrees
        assert_eq!(parse_radius("abc"), 0.0);
    }

    // ── Range operators ─────────────────────────────────────────────────────

    #[test]
    fn data_release_range_operators() {
        let mut gt = SearchFormState::new();
        gt.data_release = ">2020".to_string();
        assert!(build(&gt).contains("Plane.dataRelease >"), "greater-than");

        let mut le = SearchFormState::new();
        le.data_release = "<=2021-06".to_string();
        assert!(build(&le).contains("Plane.dataRelease <="), "less-equal");

        let mut eq = SearchFormState::new();
        eq.data_release = "2020".to_string();
        let a = build(&eq);
        // Equals on a bare year expands to a >= .. AND <= .. range.
        assert!(a.contains("Plane.dataRelease >=") && a.contains("Plane.dataRelease <="), "{}", a);

        let mut bt = SearchFormState::new();
        bt.data_release = "2019-01-01..2021-01-01".to_string();
        let b = build(&bt);
        assert!(b.contains("Plane.dataRelease >=") && b.contains("Plane.dataRelease <="), "{}", b);
    }

    #[test]
    fn obs_date_operator_maps_to_time_bounds() {
        let mut gt = SearchFormState::new();
        gt.obs_date_raw = ">2020-01-01".to_string();
        let a = build(&gt);
        assert!(a.contains("Plane.time_bounds_lower >"), "{}", a);

        let mut lt = SearchFormState::new();
        lt.obs_date_raw = "<2021-01-01".to_string();
        let b = build(&lt);
        assert!(b.contains("Plane.time_bounds_upper <"), "{}", b);

        // A plain year keeps the existing INTERVAL-overlap expansion.
        let mut plain = SearchFormState::new();
        plain.obs_date_raw = "2020".to_string();
        assert!(build(&plain).contains("INTERSECTS( INTERVAL("));
    }

    #[test]
    fn obs_date_operator_keeps_integration_time_clause() {
        // Regression: the operator branch must not short-circuit later clauses.
        let mut state = SearchFormState::new();
        state.obs_date_raw = ">2020-01-01".to_string();
        state.integration_time_min = Some(60.0);
        let adql = build(&state);
        assert!(adql.contains("Plane.time_bounds_lower >"));
        assert!(adql.contains("time_exposure >= 60"));
    }

    #[test]
    fn pixel_scale_raw_operator_overrides_max() {
        let mut state = SearchFormState::new();
        state.pixel_scale_raw = Some("> 0.2".to_string());
        state.pixel_scale_unit = "arcsec".to_string();
        let adql = build(&state);
        // 0.2 arcsec == 0.2/3600 deg.
        let deg = 0.2 / 3600.0;
        assert!(adql.contains(&format!("Plane.position_sampleSize > {}", deg)), "{}", adql);
    }

    #[test]
    fn add_converted_range_clause_all_ops() {
        let ident = |v: &str, _u: &str| v.trim().parse::<f64>().ok();

        let mut c = Vec::new();
        add_converted_range_clause("X", &range_parser::parse_range("5..1").unwrap(), "", &mut c, ident);
        assert_eq!(c[0], "X >= 1 AND X <= 5"); // normalised low→high

        let mut c = Vec::new();
        add_converted_range_clause("X", &range_parser::parse_range(">= 3").unwrap(), "", &mut c, ident);
        assert_eq!(c[0], "X >= 3");

        let mut c = Vec::new();
        add_converted_range_clause("X", &range_parser::parse_range("< 7").unwrap(), "", &mut c, ident);
        assert_eq!(c[0], "X < 7");

        let mut c = Vec::new();
        add_converted_range_clause("X", &range_parser::parse_range("42").unwrap(), "", &mut c, ident);
        assert_eq!(c[0], "X = 42");
    }
}
