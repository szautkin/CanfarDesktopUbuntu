//! The Search page's MCP steering surface — the Rust analogue of the reference's
//! `Views/SearchPage.Mcp.cs` partial class.
//!
//! Every operation here is reached over the viewer-command bridge
//! (`mcp::view_state::viewer_command("search", op, args)`), so it runs on the
//! GTK main thread and drives the SAME widgets a user drives. That is the whole
//! point: an agent and a person must not have two different paths into the page,
//! or the two will drift.
//!
//! A child module of `search_page` so it can reach the page's private widgets
//! without widening their visibility for everyone else.

use super::{
    dropdown_index, SearchPage, INTENTS, PIXEL_SCALE_UNITS, RESOLVER_SERVICES, SPECTRAL_UNITS,
    TIME_UNITS,
};
use crate::helpers::date_presets;
use crate::models::search_result::{build_columns_from_headers, default_columns};
use gtk4::prelude::*;
use serde_json::{json, Value};
use std::rc::Rc;

/// How a field's value is resolved to a dropdown position.
enum Choices {
    /// Exactly the listed values, case-insensitively.
    List(&'static [&'static str]),
    /// Through the unit converter, which also understands `Angstrom` and `um`
    /// alongside `Å` and `µm`.
    Spectral,
    /// Through the preset table, which also understands the spelling we used to
    /// persist and the one the macOS app uses.
    DatePreset,
}

/// Every enum-valued field of `set_search_form`.
///
/// One table, walked by both the validator and a test that reads the published
/// schema: an enum added to the schema without an entry here fails that test,
/// which is the only thing standing between an advertised choice and a value
/// silently swapped for another.
const ENUM_FIELDS: &[(&str, Choices)] = &[
    ("intent", Choices::List(&INTENTS)),
    ("resolver", Choices::List(&RESOLVER_SERVICES)),
    ("datePreset", Choices::DatePreset),
    ("pixelScaleUnit", Choices::List(&PIXEL_SCALE_UNITS)),
    ("timeUnit", Choices::List(&TIME_UNITS)),
    ("spectralUnit", Choices::Spectral),
    ("spectralSamplingUnit", Choices::Spectral),
    ("bandpassWidthUnit", Choices::Spectral),
    ("restFrameEnergyUnit", Choices::Spectral),
];

/// The dropdown position for `value` in `field`'s list, or an error naming what
/// the field accepts.
///
/// This replaces [`dropdown_index`] on every MCP path, and the difference is the
/// whole point: `dropdown_index` falls back to entry 0, which for a UNIT list is
/// a DIFFERENT unit. `timeUnit: "weeks"` selected seconds, so a search for
/// exposures over 5 weeks quietly ran as 5 seconds; `spectralCoverageUnit:
/// "furlong"` selected metres. Both reported success. The reference refuses the
/// value instead (`SearchUiTools.Choice`), and so do we now.
///
/// Case-insensitive, matching the reference's `OrdinalIgnoreCase`.
fn choice_index(field: &str, value: &str) -> Result<u32, String> {
    let allowed = ENUM_FIELDS
        .iter()
        .find(|(name, _)| *name == field)
        .map(|(_, choices)| choices)
        .ok_or_else(|| format!("{field} is not a choice field"))?;

    match allowed {
        Choices::List(list) => list
            .iter()
            .position(|candidate| candidate.eq_ignore_ascii_case(value))
            .map(|i| i as u32)
            .ok_or_else(|| choice_error(field, list)),
        // Spectral units accept every spelling the converter does, then resolve
        // to the canonical entry — so nothing reaches the dropdown that the
        // query builder would later fail to convert.
        Choices::Spectral => crate::helpers::unit_converter::canonical_spectral_unit(value)
            .map(|canonical| dropdown_index(&SPECTRAL_UNITS, canonical))
            .ok_or_else(|| choice_error(field, &SPECTRAL_UNITS)),
        Choices::DatePreset => {
            date_presets::position(value).ok_or_else(|| choice_error(field, &date_presets::VALUES))
        }
    }
}

/// "field must be one of: a, b, c" — the empty entry, which means "no
/// constraint", is described rather than shown as a blank.
fn choice_error(field: &str, allowed: &[&str]) -> String {
    let named: Vec<&str> = allowed.iter().copied().filter(|v| !v.is_empty()).collect();
    let empty_note = if allowed.iter().any(|v| v.is_empty()) {
        ", or \"\" for no constraint"
    } else {
        ""
    };
    format!("{field} must be one of: {}{}", named.join(", "), empty_note)
}

/// Check a whole form patch before any of it is applied.
///
/// Up front, deliberately: the checks used to sit inline beside the widget they
/// guarded, so a patch with a bad `maxRecords` had already written six other
/// fields into the form by the time it was refused. A rejected patch must leave
/// the page exactly as it was.
///
/// Pure, so it can be tested — the page itself needs a GTK main loop.
pub(super) fn validate_form_patch(args: &Value) -> Result<(), String> {
    for (field, _) in ENUM_FIELDS {
        if let Some(value) = crate::mcp::tools::arg(args, field).and_then(Value::as_str) {
            choice_index(field, value)?;
        }
    }
    if let Some(v) = crate::mcp::tools::num_arg(args, "radius") {
        // The SAME bounds the widget and the schema use. A literal here would be
        // a third copy of the number, and the spinner would silently clamp
        // anything this let through.
        let (lo, hi) = super::RADIUS_RANGE_DEG;
        if !(lo..=hi).contains(&v) {
            return Err(format!(
                "radius must be between {lo} and {hi} degrees, got {v}"
            ));
        }
    }
    if let Some(v) = crate::mcp::tools::opt_u64(args, "maxRecords") {
        let (lo, hi) = super::MAX_RECORDS_RANGE;
        if !(lo as u64..=hi as u64).contains(&v) {
            return Err(format!("maxRecords must be between {lo} and {hi}, got {v}"));
        }
    }
    Ok(())
}

/// Tab indices, matching the reference's pivot order.
const TAB_FORM: u32 = 0;
const TAB_RESULTS: u32 = 1;
const TAB_ADQL: u32 = 2;

/// Hard cap on rows returned inline by `get_search_results`, mirroring the
/// reference. A full result set can be tens of thousands of rows, which would
/// blow the MCP response budget.
const MAX_INLINE_ROWS: usize = 500;

/// The seven Additional-Constraints facets, in the order
/// `DataTrainManager` indexes them. One table so the snapshot and the setter
/// cannot disagree about which index means what.
const FACETS: [&str; 7] = [
    "band",
    "collection",
    "instrument",
    "filter",
    "calLevel",
    "dataType",
    "obsType",
];

impl SearchPage {
    /// Run one MCP operation against the live page.
    ///
    /// Returns `Err` with a human-readable reason the agent can act on; the
    /// router turns that into a tool error.
    pub async fn handle_viewer_command(
        self: &Rc<Self>,
        op: &str,
        args: &Value,
    ) -> Result<Value, String> {
        match op {
            // ── Form ────────────────────────────────────────────────────────
            "get_search_form" => Ok(self.form_snapshot()),
            "set_search_form" => {
                self.apply_form_patch(args)?;
                self.notebook.set_current_page(Some(TAB_FORM));
                Ok(self.form_snapshot())
            }
            "reset_search_form" => {
                self.clear_form();
                self.notebook.set_current_page(Some(TAB_FORM));
                Ok(self.form_snapshot())
            }

            // ── Additional Constraints (data train) ─────────────────────────
            "get_search_constraints" => {
                self.ensure_data_train().await;
                Ok(self.constraints_snapshot(args))
            }
            "set_search_constraints" => {
                self.ensure_data_train().await;
                let dropped = self.apply_constraints(args)?;
                self.refresh_train_ui();
                self.train_expander.set_expanded(true);
                self.notebook.set_current_page(Some(TAB_FORM));
                let mut out = self.constraints_snapshot(args);
                out["dropped"] = json!(dropped);
                Ok(out)
            }

            // ── Running a search ────────────────────────────────────────────
            "run_search" => {
                self.guard_not_searching()?;
                self.execute_search().await;
                self.run_result_or_error()
            }
            "set_adql_query" => {
                let adql = crate::mcp::tools::str_arg(args, "adql");
                if adql.is_empty() {
                    return Err("adql is required".to_string());
                }
                self.adql_editor.buffer().set_text(&adql);
                self.notebook.set_current_page(Some(TAB_ADQL));
                Ok(json!({ "staged": true, "adql": adql }))
            }
            "execute_adql_query" => {
                self.guard_not_searching()?;
                let adql = crate::mcp::tools::str_arg(args, "adql");
                if !adql.is_empty() {
                    self.adql_editor.buffer().set_text(&adql);
                }
                if self.adql_text().trim().is_empty() {
                    return Err("the ADQL editor is empty — pass `adql`, or stage one with \
                         set_adql_query first"
                        .to_string());
                }
                self.execute_raw_adql().await;
                self.run_result_or_error()
            }

            // ── Results table ───────────────────────────────────────────────
            "get_search_results" => Ok(self.results_snapshot(args)),
            "set_search_results_view" => {
                self.apply_results_view(args)?;
                Ok(self.results_snapshot(&json!({ "includeRows": false })))
            }
            "export_search_results" => self.export_results(args).await,

            // ── Side panel ──────────────────────────────────────────────────
            "load_recent_search" => self.load_recent(args),
            "run_saved_query" => self.run_saved(args).await,

            other => Err(format!("unknown search operation: {other}")),
        }
    }

    // ── Snapshots ───────────────────────────────────────────────────────────

    fn adql_text(&self) -> String {
        let buf = self.adql_editor.buffer();
        buf.text(&buf.start_iter(), &buf.end_iter(), false)
            .to_string()
    }

    /// Every field of the four constraint columns, plus resolver status and the
    /// staged ADQL — what the reference's `BuildFormSnapshot` returns.
    fn form_snapshot(&self) -> Value {
        json!({
            // Observation
            "observationId": self.observation_id.text().to_string(),
            "piName": self.pi_name.text().to_string(),
            "proposalId": self.proposal_id.text().to_string(),
            "proposalTitle": self.proposal_title.text().to_string(),
            "keywords": self.keywords.text().to_string(),
            "dataRelease": self.data_release.text().to_string(),
            "publicOnly": self.public_only.is_active(),
            "intent": INTENTS.get(self.intent.selected() as usize).unwrap_or(&""),
            // Spatial
            "target": self.target.text().to_string(),
            "resolver": RESOLVER_SERVICES
                .get(self.resolver.selected() as usize)
                .unwrap_or(&"ALL"),
            "radius": self.radius.value(),
            "pixelScale": self.pixel_scale.text().to_string(),
            "pixelScaleUnit": PIXEL_SCALE_UNITS
                .get(self.pixel_scale_unit.selected() as usize)
                .unwrap_or(&"arcsec"),
            "spatialCutout": self.spatial_cutout.is_active(),
            "resolverStatus": self.resolver_status.text().to_string(),
            "resolvedRa": *self.resolved_ra.borrow(),
            "resolvedDec": *self.resolved_dec.borrow(),
            // Temporal
            "observationDate": self.obs_date.text().to_string(),
            "datePreset": date_presets::VALUES
                .get(self.date_preset.selected() as usize)
                .unwrap_or(&""),
            "integrationTime": self.integration_time.text().to_string(),
            "timeSpan": self.time_span.text().to_string(),
            "timeUnit": TIME_UNITS.get(self.time_unit.selected() as usize).unwrap_or(&"s"),
            // Spectral
            "spectralCoverage": self.spectral_coverage.text().to_string(),
            "spectralSampling": self.spectral_sampling.text().to_string(),
            "resolvingPower": self.resolving_power.text().to_string(),
            "bandpassWidth": self.bandpass_width.text().to_string(),
            "restFrameEnergy": self.rest_frame_energy.text().to_string(),
            "spectralUnit": SPECTRAL_UNITS
                .get(self.spectral_unit.selected() as usize)
                .unwrap_or(&"nm"),
            "spectralSamplingUnit": SPECTRAL_UNITS
                .get(self.spectral_sampling_unit.selected() as usize)
                .unwrap_or(&"nm"),
            "bandpassWidthUnit": SPECTRAL_UNITS
                .get(self.bandpass_width_unit.selected() as usize)
                .unwrap_or(&"nm"),
            "restFrameEnergyUnit": SPECTRAL_UNITS
                .get(self.rest_frame_energy_unit.selected() as usize)
                .unwrap_or(&"nm"),
            "spectralCutout": self.spectral_cutout.is_active(),
            // Options + live state
            "maxRecords": self.max_records.value() as u32,
            "adql": self.adql_text(),
            "isSearching": self.search_spinner.is_visible(),
        })
    }

    /// Per-facet available + selected values, plus whether the data train has
    /// loaded at all (an empty facet list means "not loaded", not "no values").
    /// How many values of one facet are listed before the list is summarised.
    ///
    /// CADC has thousands of instruments and collections, and listing every
    /// value of all seven facets measured 99 KB in QA — spooled to a file the
    /// agent then had to grep. A caller choosing a filter needs to know what is
    /// on offer, not to receive the archive's whole vocabulary; when it needs
    /// one facet in full it can ask for that facet by name.
    const FACET_VALUES_INLINE: usize = 40;

    fn constraints_snapshot(&self, args: &Value) -> Value {
        let mgr = self.train_manager.borrow();
        let all: [&[String]; 7] = [
            &mgr.all_bands,
            &mgr.all_collections,
            &mgr.all_instruments,
            &mgr.all_filters,
            &mgr.all_cal_levels,
            &mgr.all_data_types,
            &mgr.all_obs_types,
        ];
        let available = [
            &mgr.available_bands,
            &mgr.available_collections,
            &mgr.available_instruments,
            &mgr.available_filters,
            &mgr.available_cal_levels,
            &mgr.available_data_types,
            &mgr.available_obs_types,
        ];

        let mut facets = serde_json::Map::new();
        for (idx, name) in FACETS.iter().enumerate() {
            // Keep the column's declared order rather than the HashSet's.
            let avail: Vec<&String> = all[idx]
                .iter()
                .filter(|v| available[idx].contains(*v))
                .collect();
            let mut selected: Vec<String> = mgr
                .selection(idx)
                .map(|s| s.iter().cloned().collect())
                .unwrap_or_default();
            selected.sort();

            // One facet asked for by name comes back whole; otherwise each is
            // capped so the reply stays readable.
            let wanted = crate::mcp::tools::arg(args, "facet")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|f| !f.is_empty());
            let full = wanted.is_some_and(|f| f.eq_ignore_ascii_case(name));
            let total = avail.len();
            let shown: Vec<&String> = if full {
                avail
            } else {
                avail.into_iter().take(Self::FACET_VALUES_INLINE).collect()
            };

            let mut entry = serde_json::Map::new();
            entry.insert("available".into(), json!(shown));
            entry.insert("availableCount".into(), json!(total));
            entry.insert("selected".into(), json!(selected));
            if total > shown.len() {
                entry.insert(
                    "note".into(),
                    json!(format!(
                        "{} of {total} values shown — call get_search_constraints \
                         {{\"facet\": \"{name}\"}} for all of them.",
                        shown.len()
                    )),
                );
            }
            facets.insert((*name).to_string(), Value::Object(entry));
        }

        json!({
            "loaded": !mgr.all_bands.is_empty(),
            "facets": facets,
        })
    }

    /// The results table's full view state, and (by default) the current page's
    /// RAW cell values — raw so an agent can compute on them, matching the
    /// reference.
    fn results_snapshot(&self, args: &Value) -> Value {
        let include_rows = crate::mcp::tools::opt_bool(args, "includeRows").unwrap_or(true);
        let max_rows = crate::mcp::tools::opt_u64(args, "maxRows")
            .map(|n| (n as usize).min(MAX_INLINE_ROWS))
            .unwrap_or(MAX_INLINE_ROWS);

        let columns = {
            let store = self.results_store.borrow();
            match &*store {
                Some(r) => build_columns_from_headers(&r.columns),
                None => default_columns(),
            }
        };
        let column_json: Vec<Value> = columns
            .iter()
            .map(|c| {
                json!({
                    "key": c.key,
                    "label": c.display_name,
                    "visible": self.is_col_visible(c),
                })
            })
            .collect();

        let total_rows = self
            .results_store
            .borrow()
            .as_ref()
            .map(|r| r.total_rows())
            .unwrap_or(0);
        let processed = self.get_processed_rows();
        let page = *self.current_page.borrow();
        let page_size = *self.page_size.borrow();

        let mut out = json!({
            "status": self.status_label.text().to_string(),
            "adql": self.adql_text(),
            "totalRows": total_rows,
            "filteredRows": processed.len(),
            "currentPage": page,
            "totalPages": self.total_pages(),
            "rowsPerPage": page_size,
            "pageStatus": self.page_label.text().to_string(),
            "sortColumn": self.sort_column.borrow().clone(),
            "sortAscending": *self.sort_ascending.borrow(),
            "filters": self.column_filters.borrow().clone(),
            "columnUnits": self.column_units.borrow().clone(),
            "columns": column_json,
        });

        if include_rows {
            let start = page.saturating_mul(page_size);
            // Column visibility is a preference about the GRID, tuned for the
            // default search view. An arbitrary ADQL `SELECT` — a JOIN, an
            // ObsCore query — returns columns that preference has never heard
            // of, so nothing matched, `rowColumns` came back empty, and every
            // row serialized as `[]`. QA saw `totalRows: 100` beside a hundred
            // empty arrays: the query worked, the data was there, and the tool
            // reported success while handing back nothing.
            //
            // A preference that would hide EVERY column is not a preference the
            // caller expressed. When it selects nothing, show the result as it
            // came back.
            // `allColumns` is what the row-detail modal shows: every column the
            // query returned, not the dozen the grid is set to. Without it an
            // agent could see a row's cells only by changing the grid's column
            // visibility, which is a change the person watching would see.
            let all = crate::mcp::tools::opt_bool(args, "allColumns").unwrap_or(false);
            let mut visible: Vec<&crate::models::search_result::ResultColumnInfo> = if all {
                columns.iter().collect()
            } else {
                columns.iter().filter(|c| self.is_col_visible(c)).collect()
            };
            if visible.is_empty() {
                visible = columns.iter().collect();
            }
            let headers: Vec<&str> = visible.iter().map(|c| c.key.as_str()).collect();
            let rows: Vec<Value> = processed
                .iter()
                .skip(start)
                .take(page_size.min(max_rows))
                .map(|row| {
                    Value::Array(visible.iter().map(|c| json!(row.get(&c.header))).collect())
                })
                .collect();
            out["rowColumns"] = json!(headers);
            out["rows"] = json!(rows);
        }
        out
    }

    /// `{ran, adql, totalRows, status}` — the shared tail of `run_search` and
    /// `execute_adql_query`, so both report a run the same way.
    /// What a search tool returns.
    ///
    /// `Err` when the search failed, carrying the service's own words. It used
    /// to return `Ok` with `"status": "Search failed"` — a tool reporting
    /// success for a failure, with the reason readable only on screen, so an
    /// agent could see that something went wrong and never what.
    fn run_result_or_error(&self) -> Result<Value, String> {
        if let Some(why) = self.last_search_error.borrow().clone() {
            // TAP names the column it could not find and stops there, so a
            // caller learns what is wrong and nothing about what is right —
            // and guesses again. This adds the next step: which table to ask
            // about, or that the "column" is a string literal in the wrong
            // quotes.
            return Err(crate::helpers::adql_error::explain(&why, &self.adql_text()));
        }
        Ok(self.run_result())
    }

    fn run_result(&self) -> Value {
        json!({
            "ran": true,
            "adql": self.adql_text(),
            "totalRows": self
                .results_store
                .borrow()
                .as_ref()
                .map(|r| r.total_rows())
                .unwrap_or(0),
            "status": self.status_label.text().to_string(),
        })
    }

    /// Refuse to start a second search while one is in flight, rather than
    /// interleaving two result sets into the same table.
    fn guard_not_searching(&self) -> Result<(), String> {
        if self.search_spinner.is_visible() {
            return Err("a search is already running".to_string());
        }
        Ok(())
    }

    /// Load the data train if it has not arrived yet.
    ///
    /// The page kicks this off in the background at construction, so on a cold
    /// start the facet lists are briefly empty. The reference hit exactly this
    /// (facets empty on first run) and fixed it by loading synchronously for the
    /// constraints tools — an agent must never be told "no values" when the
    /// answer is "not fetched yet".
    async fn ensure_data_train(self: &Rc<Self>) {
        if self.train_manager.borrow().all_bands.is_empty() {
            self.load_data_train().await;
        }
    }
}

// ── Mutators ────────────────────────────────────────────────────────────────

impl SearchPage {
    /// Apply a sparse form patch: only the fields present in `args` change.
    ///
    /// Ordering matters twice, exactly as in the reference:
    ///  * the resolver is set BEFORE the target, so the resolve that setting the
    ///    target triggers uses the service the caller asked for;
    ///  * `datePreset` is applied BEFORE `observationDate`, so an explicit date
    ///    wins over a preset given in the same call.
    fn apply_form_patch(self: &Rc<Self>, args: &Value) -> Result<(), String> {
        // Everything is checked before anything is written, so a refusal leaves
        // the form untouched rather than half-patched.
        validate_form_patch(args)?;

        let arg = |k: &str| crate::mcp::tools::arg(args, k);
        let text = |k: &str| arg(k).and_then(Value::as_str).map(str::to_string);

        if let Some(v) = text("resolver") {
            self.resolver.set_selected(choice_index("resolver", &v)?);
        }
        if let Some(v) = text("target") {
            self.target.set_text(&v);
        }

        // Observation
        if let Some(v) = text("observationId") {
            self.observation_id.set_text(&v);
        }
        if let Some(v) = text("piName") {
            self.pi_name.set_text(&v);
        }
        if let Some(v) = text("proposalId") {
            self.proposal_id.set_text(&v);
        }
        if let Some(v) = text("proposalTitle") {
            self.proposal_title.set_text(&v);
        }
        if let Some(v) = text("keywords") {
            self.keywords.set_text(&v);
        }
        if let Some(v) = text("dataRelease") {
            self.data_release.set_text(&v);
        }
        if let Some(v) = arg("publicOnly").and_then(Value::as_bool) {
            self.public_only.set_active(v);
        }
        if let Some(v) = text("intent") {
            self.intent.set_selected(choice_index("intent", &v)?);
        }

        // Spatial
        if let Some(v) = crate::mcp::tools::num_arg(args, "radius") {
            self.radius.set_value(v);
        }
        if let Some(v) = text("pixelScale") {
            self.pixel_scale.set_text(&v);
        }
        if let Some(v) = text("pixelScaleUnit") {
            self.pixel_scale_unit
                .set_selected(choice_index("pixelScaleUnit", &v)?);
        }
        if let Some(v) = arg("spatialCutout").and_then(Value::as_bool) {
            self.spatial_cutout.set_active(v);
        }

        // Temporal — preset first, so an explicit date in the same call wins.
        if let Some(v) = text("datePreset") {
            self.date_preset
                .set_selected(choice_index("datePreset", &v)?);
        }
        if let Some(v) = text("observationDate") {
            self.obs_date.set_text(&v);
        }
        if let Some(v) = text("integrationTime") {
            self.integration_time.set_text(&v);
        }
        if let Some(v) = text("timeSpan") {
            self.time_span.set_text(&v);
        }
        if let Some(v) = text("timeUnit") {
            self.time_unit.set_selected(choice_index("timeUnit", &v)?);
        }

        // Spectral
        if let Some(v) = text("spectralCoverage") {
            self.spectral_coverage.set_text(&v);
        }
        if let Some(v) = text("spectralSampling") {
            self.spectral_sampling.set_text(&v);
        }
        if let Some(v) = text("resolvingPower") {
            self.resolving_power.set_text(&v);
        }
        if let Some(v) = text("bandpassWidth") {
            self.bandpass_width.set_text(&v);
        }
        if let Some(v) = text("restFrameEnergy") {
            self.rest_frame_energy.set_text(&v);
        }
        // `spectralUnit` sets the coverage field's unit and, unless overridden
        // below, every other spectral unit too — so an agent that names one unit
        // for the whole block still gets what it meant, while one that wants a
        // coverage in nm and a sampling in GHz can say so.
        if let Some(v) = text("spectralUnit") {
            let index = choice_index("spectralUnit", &v)?;
            self.spectral_unit.set_selected(index);
            self.spectral_sampling_unit.set_selected(index);
            self.bandpass_width_unit.set_selected(index);
            self.rest_frame_energy_unit.set_selected(index);
        }
        for (key, combo) in [
            ("spectralSamplingUnit", &self.spectral_sampling_unit),
            ("bandpassWidthUnit", &self.bandpass_width_unit),
            ("restFrameEnergyUnit", &self.rest_frame_energy_unit),
        ] {
            if let Some(v) = text(key) {
                combo.set_selected(choice_index(key, &v)?);
            }
        }
        if let Some(v) = arg("spectralCutout").and_then(Value::as_bool) {
            self.spectral_cutout.set_active(v);
        }

        if let Some(v) = crate::mcp::tools::opt_u64(args, "maxRecords") {
            self.max_records.set_value(v as f64);
        }
        Ok(())
    }

    /// Replace the named facets' selections, then report what the cascade pruned.
    ///
    /// Each facet given REPLACES that facet's whole selection (an omitted facet is
    /// left alone), matching the reference. Values that the cascade makes
    /// unavailable are dropped rather than silently kept, and named back to the
    /// caller so an agent learns its combination was impossible.
    fn apply_constraints(&self, args: &Value) -> Result<Vec<String>, String> {
        let clear_all = crate::mcp::tools::bool_arg(args, "clearAll");

        let mut requested: Vec<Vec<String>> = Vec::with_capacity(7);
        {
            let mgr = self.train_manager.borrow();
            for (idx, name) in FACETS.iter().enumerate() {
                let current: Vec<String> = if clear_all {
                    Vec::new()
                } else {
                    mgr.selection(idx)
                        .map(|s| s.iter().cloned().collect())
                        .unwrap_or_default()
                };
                match crate::mcp::tools::arg(args, name) {
                    None | Some(Value::Null) => requested.push(current),
                    Some(Value::Array(items)) => requested.push(
                        items
                            .iter()
                            .filter_map(|v| v.as_str().map(str::to_string))
                            .collect(),
                    ),
                    Some(_) => {
                        return Err(format!("{name} must be an array of strings"));
                    }
                }
            }
        }

        let asked: Vec<String> = requested.iter().flatten().cloned().collect();
        let per_column: [Vec<String>; 7] = requested
            .try_into()
            .map_err(|_| "internal: facet count mismatch".to_string())?;
        self.train_manager
            .borrow_mut()
            .set_all_selections(per_column);

        // Anything asked for that the cascade did not keep.
        let mgr = self.train_manager.borrow();
        let kept: std::collections::HashSet<String> = (0..7)
            .filter_map(|i| mgr.selection(i))
            .flat_map(|s| s.iter().cloned())
            .collect();
        let mut dropped: Vec<String> = asked.into_iter().filter(|v| !kept.contains(v)).collect();
        dropped.sort();
        dropped.dedup();
        Ok(dropped)
    }

    /// Apply a results-table view command: filters, sort, column visibility,
    /// display units, page size and pagination.
    ///
    /// Every column key is validated UP FRONT, so a command naming one bad column
    /// changes nothing at all rather than applying half of itself.
    fn apply_results_view(self: &Rc<Self>, args: &Value) -> Result<(), String> {
        let columns = {
            let store = self.results_store.borrow();
            match &*store {
                Some(r) => build_columns_from_headers(&r.columns),
                None => return Err("no search results — run a search first".to_string()),
            }
        };
        let known: std::collections::HashSet<&str> =
            columns.iter().map(|c| c.key.as_str()).collect();
        // Resolve a caller's spelling to the one the grid uses.
        //
        // The keys are lower case because they are cleaned column names, and an
        // agent reading "Filter" off the heading strip wrote "Filter". Refusing
        // that taught it nothing it could not have guessed, so the case is
        // ignored — and the CANONICAL key is what comes back, because it is
        // what the filter map, the visibility map and the unit map are keyed
        // by. Matching case-insensitively and then storing the caller's
        // spelling would be a filter that never matches a row.
        let resolve = |key: &str| -> Result<String, String> {
            if known.contains(key) {
                return Ok(key.to_string());
            }
            let lowered = key.to_ascii_lowercase();
            if let Some(found) = known.iter().find(|k| **k == lowered) {
                return Ok((*found).to_string());
            }
            let mut names: Vec<&str> = known.iter().copied().collect();
            names.sort();
            Err(format!("unknown column {key:?}; known columns: {names:?}"))
        };

        // ── Validate everything before mutating anything ────────────────────
        let set_filters = crate::mcp::tools::arg(args, "setFilters").and_then(Value::as_object);
        let mut filters_to_set: Vec<(String, String)> = Vec::new();
        if let Some(map) = set_filters {
            for (key, value) in map {
                let key = resolve(key)?;
                let text = value.as_str().map(str::trim).unwrap_or_default();
                filters_to_set.push((key, text.to_string()));
            }
        }
        let sort_column = match crate::mcp::tools::opt_str_arg(args, "sortColumn") {
            Some(key) => Some(resolve(&key)?),
            None => None,
        };
        let show: Vec<String> = string_list(args, "showColumns")
            .iter()
            .map(|k| resolve(k))
            .collect::<Result<_, _>>()?;
        let hide: Vec<String> = string_list(args, "hideColumns")
            .iter()
            .map(|k| resolve(k))
            .collect::<Result<_, _>>()?;
        let units = crate::mcp::tools::arg(args, "columnUnits").and_then(Value::as_object);
        let mut units_to_set: Vec<(String, String)> = Vec::new();
        if let Some(map) = units {
            for (key, value) in map {
                let key = resolve(key)?;
                let unit = value.as_str().unwrap_or_default();
                if !crate::helpers::column_units::is_valid_unit(&key, unit) {
                    // Say what WOULD work. The unknown-column error above lists
                    // every column, and an agent given "not a display unit" and
                    // nothing else has to guess its way through "deg",
                    // "degrees", "sexagesimal" one call at a time.
                    let choices: Vec<&str> = crate::helpers::column_units::available_units(&key)
                        .iter()
                        .map(|c| c.id)
                        .collect();
                    return Err(if choices.is_empty() {
                        format!("column {key:?} has no display units to choose from")
                    } else {
                        format!(
                            "{unit:?} is not a display unit for column {key:?}; \
                             it takes {choices:?} (or \"\" to reset)"
                        )
                    });
                }
                units_to_set.push((key, unit.to_string()));
            }
        }

        // ── Apply ───────────────────────────────────────────────────────────
        if crate::mcp::tools::bool_arg(args, "clearFilters") {
            self.column_filters.borrow_mut().clear();
        }
        if !filters_to_set.is_empty() {
            let mut filters = self.column_filters.borrow_mut();
            for (key, text) in filters_to_set {
                if text.is_empty() {
                    filters.remove(&key);
                } else {
                    filters.insert(key, text);
                }
            }
        }
        if let Some(key) = sort_column {
            *self.sort_column.borrow_mut() = Some(key);
            *self.sort_ascending.borrow_mut() =
                crate::mcp::tools::opt_bool(args, "sortAscending").unwrap_or(true);
        }
        if !show.is_empty() || !hide.is_empty() {
            let mut visibility = self.column_visibility.borrow_mut();
            for key in show {
                visibility.insert(key, true);
            }
            for key in hide {
                visibility.insert(key, false);
            }
        }
        if !units_to_set.is_empty() {
            let mut chosen = self.column_units.borrow_mut();
            for (key, unit) in units_to_set {
                if unit.is_empty() {
                    chosen.remove(&key);
                } else {
                    chosen.insert(key, unit);
                }
            }
            drop(chosen);
            self.persist_column_units();
        }
        if let Some(n) = crate::mcp::tools::opt_u64(args, "rowsPerPage") {
            if !(1..=1000).contains(&n) {
                return Err(format!("rowsPerPage must be between 1 and 1000, got {n}"));
            }
            *self.page_size.borrow_mut() = n as usize;
            *self.current_page.borrow_mut() = 0;
        }

        // Pagination last, so it clamps against the page count the filters and
        // page size above just produced.
        let last_page = self.total_pages().saturating_sub(1);
        if let Some(action) = crate::mcp::tools::opt_str_arg(args, "pageAction") {
            let mut page = self.current_page.borrow_mut();
            *page = match action.as_str() {
                "first" => 0,
                "previous" | "prev" => page.saturating_sub(1),
                "next" => (*page + 1).min(last_page),
                "last" => last_page,
                other => {
                    return Err(format!(
                        "pageAction must be first/previous/next/last, got {other:?}"
                    ))
                }
            };
        }
        if let Some(n) = crate::mcp::tools::opt_u64(args, "page") {
            *self.current_page.borrow_mut() = (n as usize).min(last_page);
        }

        self.render_results_page();
        if crate::mcp::tools::bool_arg(args, "applyFiltersToAdql") {
            self.apply_filters_to_adql();
        } else {
            self.notebook.set_current_page(Some(TAB_RESULTS));
        }
        Ok(())
    }

    /// Write the full result set to a file as CSV or TSV — no picker, since an
    /// agent cannot answer one.
    async fn export_results(self: &Rc<Self>, args: &Value) -> Result<Value, String> {
        let rows = self.get_processed_rows();
        if rows.is_empty() {
            return Err("no search results to export".to_string());
        }
        let format = match crate::mcp::tools::str_arg(args, "format")
            .to_lowercase()
            .as_str()
        {
            "" | "csv" => "csv",
            "tsv" => "tsv",
            other => return Err(format!("format must be csv or tsv, got {other:?}")),
        };
        let delimiter = if format == "csv" { "," } else { "\t" };
        let body = self.export_delimited(delimiter);

        let path = match crate::mcp::tools::opt_str_arg(args, "path") {
            Some(p) => {
                // Without this, `vos:/home/alice/out.csv` is created as a LOCAL
                // directory named `vos:`, written into, and reported back as
                // `"exported": true` with the path the caller recognises.
                crate::helpers::local_path::reject_remote(
                    &p,
                    crate::helpers::local_path::SAVE_THEN_UPLOAD,
                )?;
                std::path::PathBuf::from(p)
            }
            None => default_export_path(format),
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("could not create {}: {e}", parent.display()))?;
        }
        std::fs::write(&path, body)
            .map_err(|e| format!("could not write {}: {e}", path.display()))?;

        Ok(json!({
            "exported": true,
            "path": path.to_string_lossy(),
            "rows": rows.len(),
            "format": format,
        }))
    }

    /// Restore a recent search into the form by its index in `list_recent_searches`.
    fn load_recent(self: &Rc<Self>, args: &Value) -> Result<Value, String> {
        // Reload rather than trusting a cached list, so indices match what
        // `list_recent_searches` most recently reported.
        let all = self.services.search_store.load_recent();
        let index = crate::mcp::tools::opt_u64(args, "index").unwrap_or(0) as usize;
        let entry = all.get(index).ok_or_else(|| {
            format!(
                "no recent search at index {index} ({} available)",
                all.len()
            )
        })?;

        let had_facets = self.load_from_form_state(&entry.form_state);
        self.status_label
            .set_text(&crate::tr_fmt!("Loaded search: {}", entry.summary));
        Ok(json!({
            "loaded": true,
            "summary": entry.summary,
            "restoredConstraints": had_facets,
            "form": self.form_snapshot(),
        }))
    }

    /// Run a saved query by its exact name.
    async fn run_saved(self: &Rc<Self>, args: &Value) -> Result<Value, String> {
        let name = crate::mcp::tools::str_arg(args, "name");
        if name.is_empty() {
            return Err("name is required".to_string());
        }
        self.guard_not_searching()?;

        // Reload first: a saved query an agent added via `save_query` moments ago
        // is on disk but not yet in the sidebar.
        self.refresh_saved();
        let saved = self.services.search_store.load_saved();
        let query = saved
            .iter()
            .find(|q| q.name == name)
            .ok_or_else(|| format!("no saved query named {name:?}"))?;

        let adql = query.adql.clone();
        self.adql_editor.buffer().set_text(&adql);
        self.run_query(&adql, self.max_records.value() as u32, None)
            .await;
        self.notebook.set_current_page(Some(TAB_RESULTS));
        self.render_results_page();
        Ok(self.run_result())
    }
}

/// Read an optional array-of-strings argument as a plain `Vec`.
fn string_list(args: &Value, key: &str) -> Vec<String> {
    crate::mcp::tools::opt_str_array(args, key).unwrap_or_default()
}

/// `~/Downloads/Verbinal/search_results_<timestamp>.<ext>` — the reference's
/// default destination, so an agent need not invent a path.
fn default_export_path(format: &str) -> std::path::PathBuf {
    let stamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
    let dir = directories::UserDirs::new()
        .and_then(|d| d.download_dir().map(std::path::Path::to_path_buf))
        .unwrap_or_else(std::env::temp_dir)
        .join("Verbinal");
    dir.join(format!("search_results_{stamp}.{format}"))
}

#[cfg(test)]
mod column_key_tests {
    /// A rejected unit says what the column WOULD take.
    ///
    /// The unknown-column error lists every column, which is what makes it
    /// useful. The unit error said only that the unit was wrong, so an agent
    /// asked for "deg", then "sexagesimal", then gave up — three round trips to
    /// learn a fact the app already had.
    #[test]
    fn a_rejected_unit_names_the_ones_that_would_work() {
        let choices: Vec<&str> = crate::helpers::column_units::available_units("ra(j20000)")
            .iter()
            .map(|c| c.id)
            .collect();
        assert!(
            !choices.is_empty(),
            "ra(j20000) has display units, so a refusal can name them"
        );
        assert!(
            choices.contains(&"degrees"),
            "the units for ra(j20000) are {choices:?}, which no longer includes \
             the one the error message was written against"
        );
        let source = include_str!("mcp.rs");
        assert!(
            crate::testing::code(source).contains("it takes {choices:?}"),
            "the unit refusal no longer lists the choices, so it is back to \
             telling an agent only that it was wrong"
        );
    }

    /// The case a caller writes does not decide whether a column exists.
    ///
    /// The keys are cleaned column names and therefore lower case, but an agent
    /// reads "Filter" and "Instrument" off the heading strip — and a refusal
    /// there teaches nothing that could not have been guessed.
    #[test]
    fn a_column_key_is_matched_whatever_case_it_is_written_in() {
        let source = crate::testing::code(include_str!("mcp.rs"));
        assert!(
            source.contains("to_ascii_lowercase"),
            "column keys are matched exactly again, so `Filter` is an error and \
             `filter` is not"
        );
        // And the canonical form is what gets stored — a filter keyed by the
        // caller's spelling matches no row at all.
        assert!(
            source.contains("filters_to_set") && source.contains("units_to_set"),
            "the resolved key is no longer what is applied, so a filter can be \
             stored under a key the grid does not use"
        );
    }
}

#[cfg(test)]
mod choice_tests {
    use super::{choice_index, validate_form_patch, ENUM_FIELDS};
    use serde_json::json;

    /// The properties `set_search_form` publishes.
    fn schema() -> serde_json::Map<String, serde_json::Value> {
        crate::mcp::tools::search_ui::form_properties()
            .as_object()
            .expect("form_properties is an object")
            .clone()
    }

    #[test]
    fn every_advertised_enum_is_enforced() {
        // The structural half of this fix. Enforcement is a list of field names,
        // and a list drifts from the schema it mirrors — so the test walks the
        // SCHEMA: a new enum field that nothing validates fails here, rather
        // than silently accepting anything and substituting entry 0.
        for (field, spec) in schema() {
            if spec.get("enum").is_none() {
                continue;
            }
            assert!(
                ENUM_FIELDS.iter().any(|(name, _)| *name == field),
                "`{field}` advertises an enum but nothing validates it"
            );
            let refused = validate_form_patch(&json!({ &field: "definitely-not-a-choice" }));
            assert!(
                refused.is_err(),
                "`{field}` accepted a value outside its own enum"
            );
        }
    }

    #[test]
    fn every_enforced_field_is_advertised() {
        // The other direction: validating against a list the schema never
        // published would refuse values a compliant client had no way to know
        // about.
        let schema = schema();
        for (field, _) in ENUM_FIELDS {
            let spec = schema
                .get(*field)
                .unwrap_or_else(|| panic!("`{field}` is validated but not in the schema"));
            assert!(
                spec.get("enum").is_some(),
                "`{field}` is validated as a choice but advertises no enum"
            );
        }
    }

    #[test]
    fn the_advertised_choices_are_the_ones_accepted() {
        // Not just "an enum exists" — the same values. A schema listing a choice
        // the app refuses is as bad as the reverse: a validating client would
        // send it in good faith and be rejected.
        for (field, spec) in schema() {
            let Some(values) = spec.get("enum").and_then(|e| e.as_array()) else {
                continue;
            };
            for value in values.iter().filter_map(|v| v.as_str()) {
                assert!(
                    choice_index(&field, value).is_ok(),
                    "`{field}` advertises {value:?} but refuses it"
                );
            }
        }
    }

    #[test]
    fn a_unit_outside_the_list_is_refused_not_substituted() {
        // The reason this exists. Falling back to entry 0 meant `weeks` selected
        // SECONDS: a search for exposures longer than 5 weeks ran as 5 seconds,
        // returned rows, and reported success.
        let err = validate_form_patch(&json!({ "timeUnit": "weeks" })).unwrap_err();
        assert!(err.contains("timeUnit must be one of"), "{err}");
        assert!(err.contains('d'), "the error should name the units: {err}");

        let err = validate_form_patch(&json!({ "spectralUnit": "furlong" })).unwrap_err();
        assert!(err.contains("spectralUnit must be one of"), "{err}");
    }

    #[test]
    fn a_choice_is_matched_regardless_of_case() {
        // Matching the reference's OrdinalIgnoreCase comparison.
        assert_eq!(
            choice_index("intent", "SCIENCE"),
            choice_index("intent", "science")
        );
        assert_eq!(
            choice_index("resolver", "simbad"),
            choice_index("resolver", "SIMBAD")
        );
    }

    #[test]
    fn a_spectral_unit_accepts_the_spellings_the_converter_does() {
        // `Angstrom` and `um` appear in saved searches and in agent prompts; the
        // converter has always understood them, so refusing them here would be a
        // gratuitous divergence. They resolve to the canonical entry, never to
        // entry 0 — which is metres, and 500 metres is not 500 Ångström.
        let angstrom = choice_index("spectralUnit", "Å").unwrap();
        assert_eq!(choice_index("spectralUnit", "Angstrom"), Ok(angstrom));
        assert_ne!(angstrom, 0);

        let micron = choice_index("spectralUnit", "µm").unwrap();
        assert_eq!(choice_index("spectralUnit", "um"), Ok(micron));
    }

    #[test]
    fn an_empty_choice_means_no_constraint() {
        // "" is a real entry in intent and datePreset — clearing the field, not
        // an invalid value.
        assert_eq!(choice_index("intent", ""), Ok(0));
        assert_eq!(choice_index("datePreset", ""), Ok(0));
    }

    #[test]
    fn a_rejected_patch_is_checked_before_anything_is_written() {
        // Validation runs over the whole patch up front, so a bad field late in
        // the object refuses the call without the earlier fields having been
        // applied. Proving the ORDER here needs the live page; what this pins is
        // that the validator sees every field regardless of position.
        let err = validate_form_patch(&json!({
            "target": "M31",
            "radius": 0.5,
            "maxRecords": 999_999
        }))
        .unwrap_err();
        assert!(err.contains("maxRecords"), "{err}");
    }
}
