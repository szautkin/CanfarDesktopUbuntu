use crate::helpers::adql_builder;
use crate::helpers::column_units;
use crate::helpers::data_train_manager::DataTrainManager;
use crate::helpers::range_parser;
use crate::helpers::unit_converter;
use crate::models::search_result::{
    build_columns_from_headers, default_columns, format_cell, format_cell_with_unit, RecentSearch,
    ResolverResult, SavedQuery, SearchFormState, SearchResultRow, SearchResults,
};

use crate::helpers::filter_to_adql;
use crate::state::AppServices;
use crate::ui::agent_badge::agent_badge;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

/// Convert the compact provenance stamp stored on a [`SavedQuery`] into the badge
/// model the shared [`agent_badge`] widget renders.  The stamp records the agent's
/// origin label + apply time; the tool dimension isn't stored, so it degrades to
/// the generic "mcp" agent label (matching the Research page's badge).
fn saved_query_badge(
    stamp: &crate::helpers::agent_attribution::AgentAttribution,
) -> crate::models::agent_attribution::AgentAttribution {
    crate::models::agent_attribution::AgentAttribution::new(
        stamp.origin.clone(),
        crate::tr_en!("mcp"),
        stamp.applied_at.clone(),
    )
}

/// Dropdown index tables. These map a `gtk::DropDown` position to the value
/// stored in [`SearchFormState`], and back again when a saved search is
/// restored — so both directions MUST read the same table. Module-level rather
/// than local for exactly that reason.
//
// The three unit tables come from `unit_converter`, which is what decides
// which units mean anything — the page only renders them. Keeping a second
// copy here is how the dropdown ended up offering 4 of the 14 units the
// converter has always handled.
use crate::helpers::store_events::{self, Store};
use crate::helpers::unit_converter::{PIXEL_SCALE_UNITS, SPECTRAL_UNITS, TIME_UNITS};

/// How often the page checks whether the saved-query or recent-search store
/// changed underneath it. One mutex read per tick; it rebuilds only when a
/// sequence actually moved.
const SIDEBAR_POLL_MS: u64 = 1000;
const DATE_PRESETS: [&str; 4] = ["", "Last 24 hours", "Last week", "Last month"];
const INTENTS: [&str; 3] = ["", "science", "calibration"];
/// Rows-per-page choices. The dropdown is decoded by index, so its LABELS are
/// derived from these numbers rather than written out a second time — the two
/// literals used to sit 350 lines apart, and adding a choice to one would have
/// silently given the wrong page size.
const ROWS_PER_PAGE: [usize; 5] = [25, 50, 100, 250, 500];
/// Index of the default page size (100) in [`ROWS_PER_PAGE`].
const DEFAULT_ROWS_PER_PAGE: usize = 2;
const RESOLVER_SERVICES: [&str; 5] = ["ALL", "SIMBAD", "NED", "VIZIER", "NONE"];

/// Split a comma-joined facet selection back into values, dropping blanks.
/// Inverse of `DataTrainManager::*_string()`.
fn split_facet(joined: &str) -> Vec<String> {
    joined
        .split(',')
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_string)
        .collect()
}

/// Render search results as delimited text — EVERY column, RAW values.
///
/// Deliberately not "what the grid shows". This file is opened in astropy,
/// TOPCAT or a spreadsheet, and display formatting destroys exactly what makes
/// it useful: sexagesimal in place of decimal degrees, rounded magnitudes,
/// "3.4 MB" instead of a byte count. It also used to export only the visible
/// columns, so hiding one to read the table more comfortably silently removed
/// that data from every later export.
///
/// Matches the reference's `ExportResultsCsv` / `ExportResultsTsv`, which write
/// `Results.Columns` and `row.Get(c)` untouched. Free-standing (not a method) so
/// the quoting rules can be tested without a window.
fn delimited_export(
    results: &crate::models::search_result::SearchResults,
    delimiter: &str,
) -> String {
    let quote = |value: &str| -> String {
        if delimiter == "\t" {
            // A TSV has no quoting convention; a literal tab inside a value
            // would silently add a column, so it becomes a space.
            return value.replace('\t', " ");
        }
        // A newline inside a value splits the record in two if left bare — it
        // must be quoted alongside commas and quotes.
        if value.contains(',') || value.contains('"') || value.contains('\n') {
            format!("\"{}\"", value.replace('"', "\"\""))
        } else {
            value.to_string()
        }
    };

    let mut out = String::new();
    // Header: the TAP column names, so the file's columns match the ADQL that
    // produced them and can be re-queried.
    let headers: Vec<String> = results.columns.iter().map(|c| quote(c)).collect();
    out.push_str(&headers.join(delimiter));
    out.push('\n');

    for row in &results.rows {
        let cells: Vec<String> = results
            .columns
            .iter()
            .map(|column| quote(row.get(column)))
            .collect();
        out.push_str(&cells.join(delimiter));
        out.push('\n');
    }
    out
}

/// The concrete `YYYY-MM-DD..YYYY-MM-DD` window a date preset stands for, or
/// `None` when `preset` is the blank entry.
///
/// Derived from `adql_builder::preset_days_back`, so the range written into the
/// visible field is exactly the one the query will use.
fn preset_date_range(preset: &str) -> Option<String> {
    let days = crate::helpers::adql_builder::preset_days_back(preset)?;
    let now = chrono::Utc::now();
    let start = now - chrono::Duration::days(days as i64);
    Some(format!(
        "{}..{}",
        start.format("%Y-%m-%d"),
        now.format("%Y-%m-%d")
    ))
}

/// Position of `value` in a dropdown table, or 0 (the first entry) when absent.
fn dropdown_index(table: &[&str], value: &str) -> u32 {
    table.iter().position(|v| *v == value).unwrap_or(0) as u32
}

/// Position of a spectral unit, tolerating every spelling the converter accepts.
///
/// Saved searches persist the unit as text, and older records hold `Angstrom`
/// and `um` where the list now carries `Å` and `µm`. Exact matching misses those
/// and falls back to entry 0 — which is `m`, so a restored 500 nm search would
/// silently become 500 metres.
fn spectral_unit_index(value: &str) -> u32 {
    match unit_converter::canonical_spectral_unit(value) {
        Some(canonical) => dropdown_index(&SPECTRAL_UNITS, canonical),
        // Unknown unit: keep the caller's first entry rather than inventing one.
        None => 0,
    }
}

mod mcp;

/// Store sequences this page has already rendered, one per sidebar.
///
/// Tracked separately so an agent saving a query does not also rebuild the
/// recent-search list and throw away its scroll position for nothing.
#[derive(Default)]
struct SeenStoreSeq {
    saved: u64,
    recent: u64,
}

pub struct SearchPage {
    widget: gtk::Box,
    services: Arc<AppServices>,
    /// The top-level application window, used as `transient_for` parent for
    /// modal dialogs and as the `save_future` parent for the file picker —
    /// critical to avoid XDG portal deadlocks when a child widget is already
    /// detached.
    main_window: adw::ApplicationWindow,
    /// Sequences from [`store_events`] the sidebars have already reflected.
    last_store_seq: RefCell<SeenStoreSeq>,
    // Tabs
    notebook: gtk::Notebook,
    // --- Form fields (Observation) ---
    observation_id: gtk::Entry,
    pi_name: gtk::Entry,
    proposal_id: gtk::Entry,
    proposal_title: gtk::Entry,
    keywords: gtk::Entry,
    data_release: gtk::Entry,
    public_only: gtk::CheckButton,
    intent: gtk::DropDown,
    // --- Form fields (Spatial) ---
    target: gtk::Entry,
    resolver: gtk::DropDown,
    radius: gtk::SpinButton,
    pixel_scale: gtk::Entry,
    pixel_scale_unit: gtk::DropDown,
    spatial_cutout: gtk::CheckButton,
    resolver_status: gtk::Label,
    // --- Form fields (Temporal) ---
    obs_date: gtk::Entry,
    date_preset: gtk::DropDown,
    integration_time: gtk::Entry,
    time_unit: gtk::DropDown,
    time_span: gtk::Entry,
    // --- Form fields (Spectral) ---
    spectral_coverage: gtk::Entry,
    spectral_unit: gtk::DropDown,
    spectral_sampling: gtk::Entry,
    resolving_power: gtk::Entry,
    bandpass_width: gtk::Entry,
    rest_frame_energy: gtk::Entry,
    spectral_cutout: gtk::CheckButton,
    // --- Options ---
    max_records: gtk::SpinButton,
    // --- ADQL editor ---
    adql_editor: gtk::TextView,
    // --- Results ---
    results_panel: gtk::Box,
    results_count_label: gtk::Label,
    page_label: gtk::Label,
    /// "Apply filters to ADQL" button — shown only while client-side column
    /// filters are active (ref `ApplyFiltersBtn` / `UpdateApplyFiltersButton`).
    apply_filters_btn: gtk::Button,
    // --- Sidebar ---
    recent_list: gtk::ListBox,
    saved_list: gtk::ListBox,
    save_name_entry: gtk::Entry,
    // --- Status ---
    status_label: gtk::Label,
    search_spinner: gtk::Spinner,
    // --- Data Train ---
    train_lists: [gtk::ListBox; 7],
    train_manager: Rc<RefCell<DataTrainManager>>,
    // --- State ---
    resolved_ra: Rc<RefCell<Option<f64>>>,
    resolved_dec: Rc<RefCell<Option<f64>>>,
    /// Resolver-provenance (SCI-9-3): the service that actually produced the
    /// current coordinates (`result.service` else the selected resolver) and the
    /// RFC-3339 epoch it was resolved at. Captured on a successful resolution,
    /// cleared whenever the coordinates are (target edit / reset / FITS crosshair),
    /// and frozen into the saved/recent search + export bundle. Mirrors Windows
    /// `SearchViewModel._resolverServiceUsed` / `_resolutionEpoch`.
    resolver_service_used: Rc<RefCell<Option<String>>>,
    resolution_epoch: Rc<RefCell<Option<String>>>,
    results_store: Rc<RefCell<Option<SearchResults>>>,
    current_page: Rc<RefCell<usize>>,
    page_size: Rc<RefCell<usize>>,
    sort_column: Rc<RefCell<Option<String>>>,
    sort_ascending: Rc<RefCell<bool>>,
    column_filters: Rc<RefCell<std::collections::HashMap<String, String>>>,
    /// Debounce token for typed column filters. Bumped on every keystroke; the
    /// pending re-render checks it and gives way to a newer one.
    filter_generation: Rc<RefCell<u64>>,
    /// The live filter entry for each column, refreshed on every header build.
    ///
    /// Re-rendering rebuilds the header, which DESTROYS the entry the user is
    /// typing in — so focus and the caret have to be put back afterwards, and
    /// this is how the replacement widget is found.
    filter_entries: Rc<RefCell<std::collections::HashMap<String, gtk::Entry>>>,
    /// The "Additional Constraints" (data-train) expander, kept so restoring a
    /// saved search — or an agent setting constraints — can reveal the panel it
    /// just populated rather than leaving the change hidden behind a collapsed
    /// header.
    train_expander: gtk::Expander,
    /// Explicit per-column visibility chosen by the user (cleaned key → shown).
    /// A key is present only once the user has actually toggled that column; an
    /// absent key means "use the column's default". Storing overrides rather than
    /// a hide-only set is what lets a NON-default column be switched ON — the
    /// reference writes `ResultColumns[i].Visible` in both directions.
    column_visibility: Rc<RefCell<std::collections::HashMap<String, bool>>>,
    /// Per-column chosen display unit (cleaned column key → unit id). Only holds
    /// explicit non-default choices; an absent key means "column default" (RA/Dec
    /// → sexagesimal, others → their legacy readable format). Loaded on build and
    /// saved on every change (search_store `column_units.json`), so choices survive
    /// restarts. Ref `ColumnUnitCatalog` / `LocalSettingsColumnUnitStore`.
    column_units: Rc<RefCell<std::collections::HashMap<String, String>>>,
    /// Monotonic token used to debounce + cancel live target resolution: each
    /// keystroke bumps it, and a pending timeout / in-flight resolve is discarded
    /// unless its captured token still matches (ref `ResolveTargetDebouncedAsync`).
    resolve_generation: Rc<RefCell<u64>>,
}

const DEFAULT_PAGE_SIZE: usize = 100;

impl SearchPage {
    pub fn new(services: Arc<AppServices>, main_window: adw::ApplicationWindow) -> Rc<Self> {
        let widget = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        widget.set_vexpand(true);
        widget.set_hexpand(true);

        // =====================================================================
        // MAIN CONTENT (left, expandable)
        // =====================================================================
        let main_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        main_box.set_hexpand(true);
        main_box.set_margin_start(12);
        main_box.set_margin_top(12);
        main_box.set_margin_bottom(12);
        main_box.set_margin_end(12);

        let title = gtk::Label::new(Some(crate::tr_en!("CADC Archive Search")));
        title.add_css_class("title-2");
        title.set_halign(gtk::Align::Start);
        title.set_margin_bottom(12);
        main_box.append(&title);

        // --- Notebook (3 tabs: Search Form | Results | ADQL Editor) ---
        let notebook = gtk::Notebook::new();
        notebook.set_vexpand(true);

        // ====== TAB 1: SEARCH FORM ======
        let form_tab = gtk::Box::new(gtk::Orientation::Vertical, 0);
        form_tab.set_vexpand(true);

        let form_scroll = gtk::ScrolledWindow::new();
        form_scroll.set_vexpand(true);

        let form_content = gtk::Box::new(gtk::Orientation::Vertical, 12);
        form_content.set_margin_start(12);
        form_content.set_margin_end(12);
        form_content.set_margin_top(12);
        form_content.set_margin_bottom(12);

        // --- 4 constraint columns (matching CADC web) ---
        let columns = gtk::Grid::new();
        columns.set_column_spacing(12);
        columns.set_column_homogeneous(true);

        // Col 1: Observation
        let (
            obs_col,
            observation_id,
            pi_name,
            proposal_id,
            proposal_title,
            keywords,
            data_release,
            public_only,
            intent,
        ) = build_observation_column();
        columns.attach(&obs_col, 0, 0, 1, 1);

        // Col 2: Spatial
        let (
            spatial_col,
            target,
            resolver,
            radius,
            pixel_scale,
            pixel_scale_unit,
            spatial_cutout,
            resolver_status,
        ) = build_spatial_column();
        columns.attach(&spatial_col, 1, 0, 1, 1);

        // Col 3: Temporal
        let (temporal_col, obs_date, date_preset, integration_time, time_unit, time_span) =
            build_temporal_column();
        columns.attach(&temporal_col, 2, 0, 1, 1);

        // Col 4: Spectral
        let (
            spectral_col,
            spectral_coverage,
            spectral_unit,
            spectral_sampling,
            resolving_power,
            bandpass_width,
            rest_frame_energy,
            spectral_cutout,
        ) = build_spectral_column();
        columns.attach(&spectral_col, 3, 0, 1, 1);

        form_content.append(&columns);

        // --- Additional Constraints (Data Train) - Expander ---
        let train_expander = gtk::Expander::new(Some(crate::tr_en!("Additional Constraints")));
        let (train_grid, train_lists) = build_data_train();
        train_expander.set_child(Some(&train_grid));
        // Vertical gap to the columns grid above is provided by form_content's
        // own 12px box spacing — no extra margin needed here.
        form_content.append(&train_expander);

        form_scroll.set_child(Some(&form_content));
        form_tab.append(&form_scroll);

        // --- Pinned action bar (bottom of form tab, outside scroll) ---
        let action_bar = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        action_bar.add_css_class("toolbar");

        let search_btn = gtk::Button::new();
        let search_content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        search_content.append(&gtk::Image::from_icon_name("system-search-symbolic"));
        search_content.append(&gtk::Label::new(Some(crate::tr_en!("Search"))));
        search_btn.set_child(Some(&search_content));
        search_btn.add_css_class("suggested-action");
        action_bar.append(&search_btn);

        let reset_btn = gtk::Button::with_label(crate::tr_en!("Reset"));
        action_bar.append(&reset_btn);

        let max_label = gtk::Label::new(Some(crate::tr_en!("Max Records")));
        max_label.add_css_class("caption");
        action_bar.append(&max_label);
        let max_records = gtk::SpinButton::with_range(10.0, 30000.0, 100.0);
        max_records.set_value(10000.0);
        max_records.set_width_chars(6);
        action_bar.append(&max_records);

        let search_spinner = gtk::Spinner::new();
        search_spinner.set_visible(false);
        action_bar.append(&search_spinner);

        let status_label = gtk::Label::new(None);
        status_label.add_css_class("caption");
        status_label.add_css_class("dim-label");
        status_label.set_hexpand(true);
        status_label.set_halign(gtk::Align::End);
        action_bar.append(&status_label);

        form_tab.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        form_tab.append(&action_bar);

        notebook.append_page(
            &form_tab,
            Some(&gtk::Label::new(Some(crate::tr_en!("Search Form")))),
        );

        // ====== TAB 2: RESULTS ======
        let results_tab = gtk::Box::new(gtk::Orientation::Vertical, 0);
        results_tab.set_vexpand(true);

        // Results toolbar
        let results_toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        results_toolbar.add_css_class("toolbar");

        let results_count_label = gtk::Label::new(Some(crate::tr_en!("No results")));
        results_count_label.add_css_class("caption");
        results_count_label.set_hexpand(true);
        results_count_label.set_halign(gtk::Align::Start);
        results_toolbar.append(&results_count_label);

        let columns_btn = gtk::Button::with_label(crate::tr_en!("Columns"));
        columns_btn.add_css_class("flat");
        columns_btn.set_tooltip_text(Some(crate::tr_en!("Select visible columns")));
        results_toolbar.append(&columns_btn);

        let csv_btn = gtk::Button::with_label(crate::tr_en!("CSV"));
        csv_btn.add_css_class("flat");
        csv_btn.set_tooltip_text(Some(crate::tr_en!("Export results as CSV file")));
        results_toolbar.append(&csv_btn);

        let tsv_btn = gtk::Button::with_label(crate::tr_en!("TSV"));
        tsv_btn.add_css_class("flat");
        tsv_btn.set_tooltip_text(Some(crate::tr_en!("Export results as TSV file")));
        results_toolbar.append(&tsv_btn);

        let refresh_results_btn = gtk::Button::from_icon_name("view-refresh-symbolic");
        refresh_results_btn.set_tooltip_text(Some(crate::tr_en!("Apply filters and re-render")));
        results_toolbar.append(&refresh_results_btn);

        // "Apply filters to ADQL" — only visible while column filters are active.
        let apply_filters_btn = gtk::Button::with_label(crate::tr_en!("Apply filters to ADQL"));
        apply_filters_btn.add_css_class("flat");
        apply_filters_btn.set_tooltip_text(Some(crate::tr_en!(
            "Append the active column filters as an ADQL WHERE clause"
        )));
        apply_filters_btn.set_visible(false);
        results_toolbar.append(&apply_filters_btn);

        let rows_label = gtk::Label::new(Some(crate::tr_en!("Rows/page:")));
        rows_label.add_css_class("caption");
        results_toolbar.append(&rows_label);
        let row_choices: Vec<String> = ROWS_PER_PAGE.iter().map(usize::to_string).collect();
        let row_choice_refs: Vec<&str> = row_choices.iter().map(String::as_str).collect();
        let rows_combo = gtk::DropDown::new(
            Some(gtk::StringList::new(&row_choice_refs)),
            gtk::Expression::NONE,
        );
        rows_combo.set_selected(DEFAULT_ROWS_PER_PAGE as u32);
        results_toolbar.append(&rows_combo);

        results_tab.append(&results_toolbar);

        // Results scroll area
        let results_scroll = gtk::ScrolledWindow::new();
        results_scroll.set_vexpand(true);
        let results_panel = gtk::Box::new(gtk::Orientation::Vertical, 0);
        results_panel.set_margin_start(12);
        results_panel.set_margin_end(12);
        results_panel.set_margin_top(12);
        results_panel.set_margin_bottom(12);
        results_scroll.set_child(Some(&results_panel));
        results_tab.append(&results_scroll);

        // Pagination
        let page_bar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        page_bar.add_css_class("toolbar");

        let first_btn = gtk::Button::from_icon_name("go-first-symbolic");
        first_btn.set_tooltip_text(Some(crate::tr_en!("First page")));
        page_bar.append(&first_btn);
        let prev_btn = gtk::Button::from_icon_name("go-previous-symbolic");
        prev_btn.set_tooltip_text(Some(crate::tr_en!("Previous page")));
        page_bar.append(&prev_btn);
        let page_label = gtk::Label::new(Some(crate::tr_en!("Page 1")));
        page_label.add_css_class("caption");
        page_bar.append(&page_label);
        let next_btn = gtk::Button::from_icon_name("go-next-symbolic");
        next_btn.set_tooltip_text(Some(crate::tr_en!("Next page")));
        page_bar.append(&next_btn);
        let last_btn = gtk::Button::from_icon_name("go-last-symbolic");
        last_btn.set_tooltip_text(Some(crate::tr_en!("Last page")));
        page_bar.append(&last_btn);

        results_tab.append(&page_bar);

        notebook.append_page(
            &results_tab,
            Some(&gtk::Label::new(Some(crate::tr_en!("Results")))),
        );

        // ====== TAB 3: ADQL EDITOR ======
        let adql_tab = gtk::Box::new(gtk::Orientation::Vertical, 12);
        adql_tab.set_margin_start(12);
        adql_tab.set_margin_end(12);
        adql_tab.set_margin_top(12);
        adql_tab.set_margin_bottom(12);

        let adql_scroll = gtk::ScrolledWindow::new();
        adql_scroll.set_vexpand(true);
        let adql_editor = gtk::TextView::new();
        adql_editor.set_monospace(true);
        adql_editor.set_wrap_mode(gtk::WrapMode::Word);
        adql_editor.set_editable(true);
        adql_editor.set_margin_start(12);
        adql_editor.set_margin_end(12);
        adql_editor.set_margin_top(12);
        adql_editor.set_margin_bottom(12);
        adql_scroll.set_child(Some(&adql_editor));
        adql_tab.append(&adql_scroll);

        let adql_action = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        adql_action.add_css_class("toolbar");
        let exec_btn = gtk::Button::new();
        let exec_content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        exec_content.append(&gtk::Image::from_icon_name("media-playback-start-symbolic"));
        exec_content.append(&gtk::Label::new(Some(crate::tr_en!("Execute"))));
        exec_btn.set_child(Some(&exec_content));
        exec_btn.add_css_class("suggested-action");
        adql_action.append(&exec_btn);
        adql_tab.append(&adql_action);

        notebook.append_page(
            &adql_tab,
            Some(&gtk::Label::new(Some(crate::tr_en!("ADQL Editor")))),
        );

        main_box.append(&notebook);
        widget.append(&main_box);

        // =====================================================================
        // SIDEBAR (right, 260px fixed)
        // =====================================================================
        widget.append(&gtk::Separator::new(gtk::Orientation::Vertical));

        let sidebar_scroll = gtk::ScrolledWindow::new();
        sidebar_scroll.set_size_request(260, -1);
        sidebar_scroll.set_vexpand(true);

        let sidebar = gtk::Box::new(gtk::Orientation::Vertical, 12);
        sidebar.set_margin_start(12);
        sidebar.set_margin_end(12);
        sidebar.set_margin_top(12);
        sidebar.set_margin_bottom(12);

        // Recent Searches card
        let recent_card = gtk::Box::new(gtk::Orientation::Vertical, 6);
        recent_card.add_css_class("card");

        let recent_header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        recent_header.set_margin_start(12);
        recent_header.set_margin_end(12);
        recent_header.set_margin_top(12);
        let recent_title = gtk::Label::new(Some(crate::tr_en!("Recent Searches")));
        recent_title.add_css_class("heading");
        recent_title.set_hexpand(true);
        recent_title.set_halign(gtk::Align::Start);
        recent_header.append(&recent_title);
        let clear_recent_btn = gtk::Button::with_label(crate::tr_en!("Clear All"));
        clear_recent_btn.add_css_class("flat");
        clear_recent_btn.add_css_class("caption");
        recent_header.append(&clear_recent_btn);
        recent_card.append(&recent_header);

        let recent_list = gtk::ListBox::new();
        recent_list.set_selection_mode(gtk::SelectionMode::None);
        recent_list.set_margin_start(12);
        recent_list.set_margin_end(12);
        recent_list.set_margin_bottom(12);
        recent_list.set_placeholder(Some(
            &gtk::Label::builder()
                .label(crate::tr_en!("No recent searches"))
                .css_classes(vec!["dim-label".to_string(), "caption".to_string()])
                .margin_top(8)
                .margin_bottom(8)
                .build(),
        ));
        recent_card.append(&recent_list);
        sidebar.append(&recent_card);

        // Saved Queries card
        let saved_card = gtk::Box::new(gtk::Orientation::Vertical, 6);
        saved_card.add_css_class("card");

        let saved_title = gtk::Label::new(Some(crate::tr_en!("Saved Queries")));
        saved_title.add_css_class("heading");
        saved_title.set_halign(gtk::Align::Start);
        saved_title.set_margin_start(12);
        saved_title.set_margin_top(12);
        saved_card.append(&saved_title);

        let save_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        save_row.set_margin_start(12);
        save_row.set_margin_end(12);
        let save_name_entry = gtk::Entry::new();
        save_name_entry.set_placeholder_text(Some(crate::tr_en!("Name (optional)")));
        save_name_entry.set_hexpand(true);
        save_row.append(&save_name_entry);
        let save_btn = gtk::Button::from_icon_name("document-save-symbolic");
        save_btn.set_tooltip_text(Some(crate::tr_en!("Save current ADQL")));
        save_row.append(&save_btn);
        saved_card.append(&save_row);

        let saved_list = gtk::ListBox::new();
        saved_list.set_selection_mode(gtk::SelectionMode::None);
        saved_list.set_margin_start(12);
        saved_list.set_margin_end(12);
        saved_list.set_margin_bottom(12);
        saved_card.append(&saved_list);
        sidebar.append(&saved_card);

        sidebar_scroll.set_child(Some(&sidebar));
        widget.append(&sidebar_scroll);

        // =====================================================================
        // Build the struct
        // =====================================================================
        // Restore persisted per-column display-unit choices before the struct
        // takes ownership of `services`.
        let loaded_column_units = services.search_store.load_column_units();
        let page = Rc::new(SearchPage {
            widget,
            services,
            main_window,
            // Seeded with what has already happened, so opening the page does
            // not replay an old change as if it were new.
            last_store_seq: RefCell::new(SeenStoreSeq {
                saved: store_events::current_seq(Store::SavedQueries),
                recent: store_events::current_seq(Store::RecentSearches),
            }),
            notebook,
            observation_id,
            pi_name,
            proposal_id,
            proposal_title,
            keywords,
            data_release,
            public_only,
            intent,
            target,
            resolver,
            radius,
            pixel_scale,
            pixel_scale_unit,
            spatial_cutout,
            resolver_status,
            obs_date,
            date_preset,
            integration_time,
            time_unit,
            time_span,
            spectral_coverage,
            spectral_unit,
            spectral_sampling,
            resolving_power,
            bandpass_width,
            rest_frame_energy,
            spectral_cutout,
            max_records,
            adql_editor,
            results_panel,
            results_count_label,
            page_label,
            apply_filters_btn,
            recent_list,
            saved_list,
            save_name_entry,
            train_lists,
            train_manager: Rc::new(RefCell::new(DataTrainManager::new())),
            status_label,
            search_spinner,
            resolved_ra: Rc::new(RefCell::new(None)),
            resolved_dec: Rc::new(RefCell::new(None)),
            resolver_service_used: Rc::new(RefCell::new(None)),
            resolution_epoch: Rc::new(RefCell::new(None)),
            results_store: Rc::new(RefCell::new(None)),
            current_page: Rc::new(RefCell::new(0)),
            page_size: Rc::new(RefCell::new(DEFAULT_PAGE_SIZE)),
            sort_column: Rc::new(RefCell::new(None)),
            sort_ascending: Rc::new(RefCell::new(true)),
            column_filters: Rc::new(RefCell::new(std::collections::HashMap::new())),
            filter_generation: Rc::new(RefCell::new(0)),
            filter_entries: Rc::new(RefCell::new(std::collections::HashMap::new())),
            train_expander,
            column_visibility: Rc::new(RefCell::new(std::collections::HashMap::new())),
            column_units: Rc::new(RefCell::new(loaded_column_units)),
            resolve_generation: Rc::new(RefCell::new(0)),
        });

        // =====================================================================
        // Wire events
        // =====================================================================

        // Search button
        let p = page.clone();
        search_btn.connect_clicked(move |_| {
            let p = p.clone();
            glib::spawn_future_local(async move { p.execute_search().await });
        });

        // Reset button
        let p = page.clone();
        reset_btn.connect_clicked(move |_| p.clear_form());

        // Execute ADQL button
        let p = page.clone();
        exec_btn.connect_clicked(move |_| {
            let p = p.clone();
            glib::spawn_future_local(async move { p.execute_raw_adql().await });
        });

        // Pagination
        let p = page.clone();
        first_btn.connect_clicked(move |_| {
            *p.current_page.borrow_mut() = 0;
            p.render_results_page();
        });
        let p = page.clone();
        prev_btn.connect_clicked(move |_| {
            let cur = *p.current_page.borrow();
            if cur > 0 {
                *p.current_page.borrow_mut() = cur - 1;
            }
            p.render_results_page();
        });
        let p = page.clone();
        next_btn.connect_clicked(move |_| {
            let cur = *p.current_page.borrow();
            let total = p.total_pages();
            if cur + 1 < total {
                *p.current_page.borrow_mut() = cur + 1;
            }
            p.render_results_page();
        });
        let p = page.clone();
        last_btn.connect_clicked(move |_| {
            let total = p.total_pages();
            if total > 0 {
                *p.current_page.borrow_mut() = total - 1;
            }
            p.render_results_page();
        });

        // Clear recent
        let p = page.clone();
        clear_recent_btn.connect_clicked(move |_| {
            let _ = p.services.search_store.clear_recent();
            p.refresh_recent();
        });

        // Save query
        let p = page.clone();
        save_btn.connect_clicked(move |_| p.save_current_query());

        // Column selector
        let p = page.clone();
        columns_btn.connect_clicked(move |btn| {
            let p = p.clone();
            let btn = btn.clone();
            glib::spawn_future_local(async move {
                p.show_column_selector(&btn).await;
            });
        });

        // CSV file export
        let p = page.clone();
        csv_btn.connect_clicked(move |btn| {
            let p = p.clone();
            let btn = btn.clone();
            glib::spawn_future_local(async move {
                p.export_to_file(&btn, ",", "csv", crate::tr_en!("CSV Files"))
                    .await;
            });
        });

        // TSV file export
        let p = page.clone();
        tsv_btn.connect_clicked(move |btn| {
            let p = p.clone();
            let btn = btn.clone();
            glib::spawn_future_local(async move {
                p.export_to_file(&btn, "\t", "tsv", crate::tr_en!("TSV Files"))
                    .await;
            });
        });

        // Refresh results (re-apply filters/sort)
        let p = page.clone();
        refresh_results_btn.connect_clicked(move |_| {
            *p.current_page.borrow_mut() = 0;
            p.render_results_page();
        });

        // Rows per page combo
        let p = page.clone();
        rows_combo.connect_selected_notify(move |combo| {
            let idx = combo.selected() as usize;
            let new_size = ROWS_PER_PAGE
                .get(idx)
                .copied()
                .unwrap_or(ROWS_PER_PAGE[DEFAULT_ROWS_PER_PAGE]);
            *p.page_size.borrow_mut() = new_size;
            *p.current_page.borrow_mut() = 0;
            p.render_results_page();
        });

        // Apply active client-side filters onto the current ADQL (→ editor tab).
        let p = page.clone();
        page.apply_filters_btn
            .connect_clicked(move |_| p.apply_filters_to_adql());

        // Live, debounced target resolution: a changed target name (or a changed
        // resolver service) invalidates any resolved coords and schedules a fresh
        // resolve 500 ms later (ref `OnTargetChanged` / `ResolveTargetDebouncedAsync`).
        let p = page.clone();
        page.target
            .connect_changed(move |_| p.schedule_target_resolve());
        let p = page.clone();
        page.resolver
            .connect_selected_notify(move |_| p.schedule_target_resolve());

        // Keyboard shortcut: Ctrl+Enter to search
        let p = page.clone();
        let key_controller = gtk::EventControllerKey::new();
        key_controller.connect_key_pressed(move |_, key, _code, modifier| {
            let ctrl = modifier.contains(gtk4::gdk::ModifierType::CONTROL_MASK);
            if ctrl && key == gtk4::gdk::Key::Return {
                let p = p.clone();
                glib::spawn_future_local(async move { p.execute_search().await });
                return gtk::glib::Propagation::Stop;
            }
            gtk::glib::Propagation::Proceed
        });
        page.widget.add_controller(key_controller);

        // Picking a date preset writes the concrete range into the visible date
        // field, so the user can SEE the window they just asked for — and edit
        // it, by clearing the preset back to blank. The dates come from the same
        // rule the query uses, so what is shown always matches what is searched.
        {
            let weak = Rc::downgrade(&page);
            page.date_preset.connect_selected_notify(move |combo| {
                let Some(page) = weak.upgrade() else { return };
                let Some(preset) = DATE_PRESETS.get(combo.selected() as usize) else {
                    return;
                };
                if let Some(range) = preset_date_range(preset) {
                    page.obs_date.set_text(&range);
                }
            });
        }

        // Load recent + saved
        page.refresh_recent();
        page.refresh_saved();

        // Follow saved-query and recent-search edits made elsewhere — an agent
        // applying save_query or remove_recent_search writes the store directly,
        // and until now the sidebar kept showing the previous list until the user
        // happened to trigger a refresh of its own. Weak, so the timer dies with
        // the page.
        {
            let weak = Rc::downgrade(&page);
            glib::timeout_add_local(
                std::time::Duration::from_millis(SIDEBAR_POLL_MS),
                move || match weak.upgrade() {
                    Some(page) => {
                        page.follow_store_changes();
                        glib::ControlFlow::Continue
                    }
                    None => glib::ControlFlow::Break,
                },
            );
        }

        // Load data train in background
        let p = page.clone();
        glib::spawn_future_local(async move { p.load_data_train().await });

        page
    }

    /// Rebuild a sidebar when its store changed underneath the page.
    ///
    /// Each list is tracked separately, so an agent saving a query does not
    /// redraw the recent-search list and lose its scroll position for nothing.
    fn follow_store_changes(self: &Rc<Self>) {
        let saved_seq = store_events::current_seq(Store::SavedQueries);
        let recent_seq = store_events::current_seq(Store::RecentSearches);

        // Decide inside a scoped borrow and release it BEFORE rebuilding: a
        // refresh runs arbitrary widget code, and holding a RefCell across it is
        // how a re-entrant borrow panic gets introduced later.
        let (rebuild_saved, rebuild_recent) = {
            let mut seen = self.last_store_seq.borrow_mut();
            let saved = saved_seq > seen.saved;
            let recent = recent_seq > seen.recent;
            seen.saved = saved_seq;
            seen.recent = recent_seq;
            (saved, recent)
        };

        if rebuild_saved {
            self.refresh_saved();
        }
        if rebuild_recent {
            self.refresh_recent();
        }
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.widget
    }

    /// Prefill the search form with a sky position (e.g. from a FITS crosshair)
    /// and land on the Search Form tab — not a stale results/ADQL tab. The
    /// coordinates are stored as already-resolved so no name resolution runs.
    pub fn show_search_form(&self, ra: f64, dec: f64) {
        self.notebook.set_current_page(Some(0));
        // Set the text FIRST: this synchronously fires the debounced resolver,
        // which clears the coords and schedules a resolve. We then bump the
        // generation token to invalidate that pending resolve, and finally stamp
        // the crosshair coords so they survive (no network round-trip needed).
        self.target
            .set_text(&crate::models::fits_image::WcsInfo::format_for_resolver(
                ra, dec,
            ));
        {
            let mut g = self.resolve_generation.borrow_mut();
            *g = g.wrapping_add(1);
        }
        *self.resolved_ra.borrow_mut() = Some(ra);
        *self.resolved_dec.borrow_mut() = Some(dec);
        self.resolver_status
            .set_text(crate::tr_en!("From FITS crosshair"));
    }

    fn build_form_state(&self) -> SearchFormState {
        let spectral_units = SPECTRAL_UNITS;
        let time_units = TIME_UNITS;
        let pixel_scale_units = PIXEL_SCALE_UNITS;
        let date_presets = DATE_PRESETS;
        let intents = INTENTS;
        let resolver_services = RESOLVER_SERVICES;

        let spectral_unit = spectral_units
            .get(self.spectral_unit.selected() as usize)
            .unwrap_or(&"nm")
            .to_string();
        let time_unit = time_units
            .get(self.time_unit.selected() as usize)
            .unwrap_or(&"s")
            .to_string();

        // Parse range fields using range_parser
        let parse_range_minmax = |entry: &gtk::Entry| -> (Option<f64>, Option<f64>) {
            let text = entry.text().to_string();
            match range_parser::parse_range(&text) {
                Some(r) => match r.op {
                    range_parser::RangeOp::Between => {
                        (r.value1.parse().ok(), r.value2.and_then(|v| v.parse().ok()))
                    }
                    range_parser::RangeOp::GreaterThan
                    | range_parser::RangeOp::GreaterThanOrEqual => (r.value1.parse().ok(), None),
                    range_parser::RangeOp::LessThan | range_parser::RangeOp::LessThanOrEqual => {
                        (None, r.value1.parse().ok())
                    }
                    range_parser::RangeOp::Equals => {
                        let v: Option<f64> = r.value1.parse().ok();
                        (v, v)
                    }
                },
                None => (None, None),
            }
        };

        // Spectral coverage
        let (wl_min, wl_max) = parse_range_minmax(&self.spectral_coverage);

        // Resolving power (dimensionless)
        let (rp_min, rp_max) = parse_range_minmax(&self.resolving_power);

        // Integration time — parse range and convert units to seconds
        let (it_min_raw, it_max_raw) = parse_range_minmax(&self.integration_time);
        let it_min = it_min_raw.and_then(|v| unit_converter::to_seconds(v, &time_unit));
        let it_max = it_max_raw.and_then(|v| unit_converter::to_seconds(v, &time_unit));

        // Time span — parse range and convert to days
        let (ts_min_raw, ts_max_raw) = parse_range_minmax(&self.time_span);
        let ts_min = ts_min_raw.and_then(|v| unit_converter::to_days(v, &time_unit));
        let ts_max = ts_max_raw.and_then(|v| unit_converter::to_days(v, &time_unit));

        // Pixel scale — keep the operator-aware raw text (so `>`, `<=`, `A..B`
        // reach the ADQL builder) alongside the legacy single-max numeric parse.
        // Without the raw text the builder's operator path was unreachable and a
        // range like the field's own `0.1..1.0` placeholder produced NO clause.
        let ps_text = self.pixel_scale.text().to_string();
        let ps_raw = {
            let trimmed = ps_text.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(ps_text.clone())
            }
        };
        let ps_max = ps_text.trim().parse::<f64>().ok();

        // Bandpass width
        let (bw_min, bw_max) = parse_range_minmax(&self.bandpass_width);

        // Spectral sampling — keep the operator-aware raw text (so `>`, `<=`,
        // `A..B` reach the ADQL builder) alongside the legacy numeric parse.
        let ss_text = self.spectral_sampling.text().to_string();
        let ss_raw = {
            let trimmed = ss_text.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(ss_text.clone())
            }
        };
        let ss_val = ss_text.trim().parse::<f64>().ok();

        // Rest frame energy
        let (rfe_min, rfe_max) = parse_range_minmax(&self.rest_frame_energy);

        // Observation date — parse range for start/end
        let obs_date_text = self.obs_date.text().to_string();
        let (obs_start, obs_end) = match range_parser::parse_range(&obs_date_text) {
            Some(r) if r.op == range_parser::RangeOp::Between => {
                (r.value1, r.value2.unwrap_or_default())
            }
            Some(r) => (r.value1, String::new()),
            None => (String::new(), String::new()),
        };

        // Data Train selections
        let mgr = self.train_manager.borrow();

        SearchFormState {
            target: self.target.text().to_string(),
            resolver_service: resolver_services
                .get(self.resolver.selected() as usize)
                .unwrap_or(&"ALL")
                .to_string(),
            // Resolver provenance captured on the last successful resolution
            // (mirrors Windows `_resolverServiceUsed` / `_resolutionEpoch`).
            resolver_service_used: self.resolver_service_used.borrow().clone(),
            resolution_epoch: self.resolution_epoch.borrow().clone(),
            resolved_ra: *self.resolved_ra.borrow(),
            resolved_dec: *self.resolved_dec.borrow(),
            search_radius: self.radius.value(),
            pixel_scale_max: ps_max,
            pixel_scale_raw: ps_raw,
            pixel_scale_unit: pixel_scale_units
                .get(self.pixel_scale_unit.selected() as usize)
                .unwrap_or(&"arcsec")
                .to_string(),
            spatial_cutout: self.spatial_cutout.is_active(),
            observation_id: self.observation_id.text().to_string(),
            proposal_pi: self.pi_name.text().to_string(),
            proposal_id: self.proposal_id.text().to_string(),
            proposal_title: self.proposal_title.text().to_string(),
            proposal_keywords: self.keywords.text().to_string(),
            data_release: self.data_release.text().to_string(),
            intent: intents
                .get(self.intent.selected() as usize)
                .unwrap_or(&"")
                .to_string(),
            public_only: self.public_only.is_active(),
            date_preset: date_presets
                .get(self.date_preset.selected() as usize)
                .unwrap_or(&"")
                .to_string(),
            obs_date_start: obs_start,
            obs_date_end: obs_end,
            obs_date_raw: obs_date_text,
            integration_time_min: it_min,
            integration_time_max: it_max,
            integration_time_unit: time_unit.clone(),
            time_span_min: ts_min,
            time_span_max: ts_max,
            time_span_unit: time_unit,
            wavelength_min: wl_min,
            wavelength_max: wl_max,
            wavelength_unit: spectral_unit.clone(),
            spectral_coverage: None, // covered by wavelength_min/max
            spectral_sampling: ss_val,
            spectral_sampling_raw: ss_raw,
            spectral_sampling_unit: spectral_unit.clone(),
            resolving_power_min: rp_min,
            resolving_power_max: rp_max,
            bandpass_width_min: bw_min,
            bandpass_width_max: bw_max,
            bandpass_width_unit: spectral_unit.clone(),
            rest_frame_energy_min: rfe_min,
            rest_frame_energy_max: rfe_max,
            rest_frame_energy_unit: spectral_unit,
            spectral_cutout: self.spectral_cutout.is_active(),
            // Data Train
            band: mgr.bands_string(),
            collection: mgr.collections_string(),
            instrument: mgr.instruments_string(),
            filter_name: mgr.filters_string(),
            calibration_level: mgr.cal_levels_string(),
            data_product_type: mgr.data_types_string(),
            obs_type: mgr.obs_types_string(),
            max_records: self.max_records.value() as u32,
            // Verbatim entry text, so a saved search restores exactly as typed —
            // the numeric min/max above cannot distinguish `>5` from `>=5`.
            integration_time_raw: self.integration_time.text().to_string(),
            time_span_raw: self.time_span.text().to_string(),
            spectral_coverage_raw: self.spectral_coverage.text().to_string(),
            resolving_power_raw: self.resolving_power.text().to_string(),
            bandpass_width_raw: self.bandpass_width.text().to_string(),
            rest_frame_energy_raw: self.rest_frame_energy.text().to_string(),
            // Every field is now listed explicitly, with no `..Default::default()`
            // tail — so adding a field to SearchFormState fails to compile HERE,
            // forcing a decision about whether the form populates it. A silent
            // default would mean the field never round-trips through a saved
            // search, which is exactly the class of bug this method just fixed.
        }
    }

    fn clear_form(self: &Rc<Self>) {
        self.observation_id.set_text("");
        self.pi_name.set_text("");
        self.proposal_id.set_text("");
        self.proposal_title.set_text("");
        self.keywords.set_text("");
        self.data_release.set_text("");
        self.public_only.set_active(false);
        self.intent.set_selected(0);
        self.target.set_text("");
        self.resolver.set_selected(0);
        self.radius.set_value(0.0167);
        self.pixel_scale.set_text("");
        self.spatial_cutout.set_active(false);
        self.resolver_status.set_text("");
        self.obs_date.set_text("");
        self.date_preset.set_selected(0);
        self.integration_time.set_text("");
        self.time_span.set_text("");
        self.spectral_coverage.set_text("");
        self.spectral_sampling.set_text("");
        self.resolving_power.set_text("");
        self.bandpass_width.set_text("");
        self.rest_frame_energy.set_text("");
        self.spectral_cutout.set_active(false);
        self.max_records.set_value(10000.0);
        *self.resolved_ra.borrow_mut() = None;
        *self.resolved_dec.borrow_mut() = None;
        self.clear_resolver_provenance();
        // Additional Constraints are part of the form: leaving the facet
        // selections behind silently applied them to the next search.
        self.train_manager.borrow_mut().clear_all();
        self.refresh_train_ui();
        self.status_label.set_text(crate::tr_en!("Form cleared"));
    }

    /// Restore a saved [`SearchFormState`] into the form widgets — the inverse of
    /// [`build_form_state`](Self::build_form_state).
    ///
    /// Two subtleties this has to respect:
    ///  * Setting the target text synchronously fires the debounced resolver,
    ///    which clears the stored coordinates. So the generation token is bumped
    ///    afterwards to discard that pending resolve, and the saved coordinates
    ///    are stamped back — the same dance `show_search_form` performs.
    ///  * The facet selections are restored with `set_all_selections`, NOT by
    ///    replaying toggles: a toggle clears everything downstream of it, which
    ///    would wipe the very selections being restored.
    ///
    /// Returns true when the restored state carried any facet selection, so the
    /// caller can expand the Additional Constraints panel (matching the reference).
    fn load_from_form_state(self: &Rc<Self>, s: &SearchFormState) -> bool {
        // ── Observation ──────────────────────────────────────────────────────
        self.observation_id.set_text(&s.observation_id);
        self.pi_name.set_text(&s.proposal_pi);
        self.proposal_id.set_text(&s.proposal_id);
        self.proposal_title.set_text(&s.proposal_title);
        self.keywords.set_text(&s.proposal_keywords);
        self.data_release.set_text(&s.data_release);
        self.public_only.set_active(s.public_only);
        self.intent
            .set_selected(dropdown_index(&INTENTS, &s.intent));

        // ── Spatial ──────────────────────────────────────────────────────────
        self.radius.set_value(s.search_radius);
        self.pixel_scale
            .set_text(s.pixel_scale_raw.as_deref().unwrap_or(""));
        self.pixel_scale_unit
            .set_selected(dropdown_index(&PIXEL_SCALE_UNITS, &s.pixel_scale_unit));
        self.spatial_cutout.set_active(s.spatial_cutout);

        // ── Temporal ─────────────────────────────────────────────────────────
        self.obs_date.set_text(&s.obs_date_raw);
        self.date_preset
            .set_selected(dropdown_index(&DATE_PRESETS, &s.date_preset));
        self.integration_time.set_text(&s.integration_time_raw);
        self.time_span.set_text(&s.time_span_raw);
        self.time_unit
            .set_selected(dropdown_index(&TIME_UNITS, &s.integration_time_unit));

        // ── Spectral ─────────────────────────────────────────────────────────
        self.spectral_coverage.set_text(&s.spectral_coverage_raw);
        self.spectral_sampling
            .set_text(s.spectral_sampling_raw.as_deref().unwrap_or(""));
        self.resolving_power.set_text(&s.resolving_power_raw);
        self.bandpass_width.set_text(&s.bandpass_width_raw);
        self.rest_frame_energy.set_text(&s.rest_frame_energy_raw);
        self.spectral_unit
            .set_selected(spectral_unit_index(&s.wavelength_unit));
        self.spectral_cutout.set_active(s.spectral_cutout);

        self.max_records.set_value(s.max_records as f64);

        // ── Target + resolved coordinates ────────────────────────────────────
        // Resolver first, so the resolve this target edit schedules would use the
        // saved service; then invalidate that pending resolve and stamp the saved
        // coordinates, which are already authoritative.
        self.resolver
            .set_selected(dropdown_index(&RESOLVER_SERVICES, &s.resolver_service));
        self.target.set_text(&s.target);
        {
            let mut g = self.resolve_generation.borrow_mut();
            *g = g.wrapping_add(1);
        }
        *self.resolved_ra.borrow_mut() = s.resolved_ra;
        *self.resolved_dec.borrow_mut() = s.resolved_dec;
        *self.resolver_service_used.borrow_mut() = s.resolver_service_used.clone();
        *self.resolution_epoch.borrow_mut() = s.resolution_epoch.clone();
        let coord_readout = match (s.resolved_ra, s.resolved_dec) {
            (Some(ra), Some(dec)) => format!("RA {:.5}  Dec {:.5}", ra, dec),
            _ => String::new(),
        };
        self.resolver_status.set_text(&coord_readout);

        // ── Additional Constraints (data train) ──────────────────────────────
        let facets = [
            split_facet(&s.band),
            split_facet(&s.collection),
            split_facet(&s.instrument),
            split_facet(&s.filter_name),
            split_facet(&s.calibration_level),
            split_facet(&s.data_product_type),
            split_facet(&s.obs_type),
        ];
        let had_facets = facets.iter().any(|v| !v.is_empty());
        self.train_manager.borrow_mut().set_all_selections(facets);
        self.refresh_train_ui();
        if had_facets {
            // Don't restore constraints into a collapsed panel — the user would
            // see an unexplained narrowing of their results.
            self.train_expander.set_expanded(true);
        }

        self.notebook.set_current_page(Some(0));
        had_facets
    }

    async fn execute_search(self: &Rc<Self>) {
        let mut state = self.build_form_state();

        // Auto-resolve target if needed (skipped when the resolver is "NONE",
        // i.e. name-only search with no coordinate constraint).
        if !state.target.is_empty()
            && state.resolved_ra.is_none()
            && state.resolver_service != "NONE"
        {
            self.status_label
                .set_text(crate::tr_en!("Resolving target..."));
            self.search_spinner.set_visible(true);
            self.search_spinner.start();

            let svc = self.services.clone();
            let t = state.target.clone();
            let rs = state.resolver_service.clone();
            match self
                .services
                .spawn(async move {
                    let token = svc.get_token().await;
                    svc.tap.resolve_target(&t, &rs, token.as_deref()).await
                })
                .await
            {
                Ok(r) => {
                    state.resolved_ra = Some(r.ra);
                    state.resolved_dec = Some(r.dec);
                    *self.resolved_ra.borrow_mut() = Some(r.ra);
                    *self.resolved_dec.borrow_mut() = Some(r.dec);
                    // Capture resolver provenance into both the persistent page
                    // state and this search's form state (feeds RecentSearch +
                    // the export provenance line).
                    self.capture_resolver_provenance(&r, &state.resolver_service);
                    state.resolver_service_used = self.resolver_service_used.borrow().clone();
                    state.resolution_epoch = self.resolution_epoch.borrow().clone();
                    self.resolver_status.set_text(&format!(
                        "RA: {:.5}  Dec: {:.5} ({})",
                        r.ra,
                        r.dec,
                        r.service.as_deref().unwrap_or("?")
                    ));
                }
                Err(e) => {
                    self.search_spinner.stop();
                    self.search_spinner.set_visible(false);
                    self.status_label
                        .set_text(&format!("Resolve failed: {}", e));
                    return;
                }
            }
        }

        let adql = adql_builder::build(&state);
        self.adql_editor.buffer().set_text(&adql);
        self.run_query(&adql, state.max_records, Some(&state)).await;
    }

    async fn execute_raw_adql(self: &Rc<Self>) {
        let buffer = self.adql_editor.buffer();
        let adql = buffer
            .text(&buffer.start_iter(), &buffer.end_iter(), false)
            .to_string();
        if adql.trim().is_empty() {
            self.status_label
                .set_text(crate::tr_en!("Enter an ADQL query"));
            return;
        }
        self.run_query(&adql, self.max_records.value() as u32, None)
            .await;
    }

    async fn run_query(
        self: &Rc<Self>,
        adql: &str,
        max_records: u32,
        form_state: Option<&SearchFormState>,
    ) {
        self.status_label.set_text(crate::tr_en!("Searching..."));
        self.search_spinner.set_visible(true);
        self.search_spinner.start();

        let svc = self.services.clone();
        let adql_owned = adql.to_string();

        let result = self
            .services
            .spawn(async move {
                let token = svc.get_token().await;
                svc.tap
                    .execute_query(&adql_owned, max_records, token.as_deref())
                    .await
            })
            .await;

        self.search_spinner.stop();
        self.search_spinner.set_visible(false);

        match result {
            Ok(results) => {
                let count = results.total_rows();
                self.results_count_label
                    .set_text(&format!("{} observations", count));
                self.status_label
                    .set_text(&format!("Found {} observations", count));

                // Save recent
                let summary = form_state
                    .map(|s| s.summary())
                    .unwrap_or_else(|| crate::tr_en!("ADQL query").to_string());
                let recent = RecentSearch {
                    summary,
                    adql: adql.to_string(),
                    // Denormalised resolver-provenance copies (the primary source
                    // is `form_state`; these are the exporter's fallback).
                    resolver_service_used: form_state.and_then(|s| s.resolver_service_used.clone()),
                    resolution_epoch: form_state.and_then(|s| s.resolution_epoch.clone()),
                    form_state: form_state.cloned().unwrap_or_default(),
                    result_count: count,
                    searched_at: chrono::Utc::now().to_rfc3339(),
                };
                let _ = self.services.search_store.save_recent(recent);
                self.refresh_recent();

                *self.results_store.borrow_mut() = Some(results);
                *self.current_page.borrow_mut() = 0;
                self.render_results_page();

                // Switch to Results tab
                self.notebook.set_current_page(Some(1));
            }
            Err(e) => {
                self.status_label.set_text(&format!("Search failed: {}", e));
            }
        }
    }

    /// Whether `col` is currently shown: the user's explicit choice if they have
    /// made one, otherwise the column's default. The ONLY place visibility is
    /// decided, so the grid, the export and the column dialog can never disagree.
    fn is_col_visible(&self, col: &crate::models::search_result::ResultColumnInfo) -> bool {
        crate::models::search_result::column_is_visible(
            &self.column_visibility.borrow(),
            &col.key,
            col.visible,
        )
    }

    fn get_processed_rows(&self) -> Vec<SearchResultRow> {
        let store = self.results_store.borrow();
        let Some(results) = &*store else {
            return Vec::new();
        };

        // Apply column filters
        let filters = self.column_filters.borrow();
        let mut rows = crate::helpers::result_filter::filter_rows(&results.rows, &filters);

        // Apply sort
        if let Some(ref col) = *self.sort_column.borrow() {
            let asc = *self.sort_ascending.borrow();
            crate::helpers::result_filter::sort_rows(&mut rows, col, asc);
        }

        rows
    }

    fn total_pages(&self) -> usize {
        let rows = self.get_processed_rows();
        let ps = *self.page_size.borrow();
        if ps == 0 {
            return 0;
        }
        rows.len().div_ceil(ps)
    }

    fn render_results_page(self: &Rc<Self>) {
        // Keep the "Apply filters to ADQL" button in sync with filter state.
        self.update_apply_filters_button();

        // Clear. The filter-entry lookup goes with the widgets it points at —
        // focusing a destroyed entry after a rebuild would do nothing visible
        // and silently swallow the user's next keystroke.
        self.filter_entries.borrow_mut().clear();
        while let Some(child) = self.results_panel.first_child() {
            self.results_panel.remove(&child);
        }

        let processed = self.get_processed_rows();
        let ps = *self.page_size.borrow();
        let page = *self.current_page.borrow();
        let total = processed.len();
        let start = page * ps;
        let end = (start + ps).min(total);
        let total_pages = if ps > 0 { total.div_ceil(ps) } else { 0 };

        // Update status
        let store = self.results_store.borrow();
        let raw_total = store.as_ref().map(|r| r.total_rows()).unwrap_or(0);
        drop(store);

        if total < raw_total {
            self.page_label.set_text(&format!(
                "Page {} of {} ({}-{} of {}, filtered from {})",
                page + 1,
                total_pages.max(1),
                if total > 0 { start + 1 } else { 0 },
                end,
                total,
                raw_total
            ));
        } else {
            self.page_label.set_text(&format!(
                "Page {} of {} ({}-{} of {})",
                page + 1,
                total_pages.max(1),
                if total > 0 { start + 1 } else { 0 },
                end,
                total
            ));
        }

        let columns = {
            let store = self.results_store.borrow();
            match &*store {
                Some(r) => build_columns_from_headers(&r.columns),
                None => default_columns(),
            }
        };
        let vis_columns: Vec<_> = columns
            .iter()
            .filter(|c| self.is_col_visible(c))
            .cloned()
            .collect();
        let sort_col = self.sort_column.borrow().clone();
        let sort_asc = *self.sort_ascending.borrow();

        // Header row with clickable sort + filter entries
        let header_row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        for col in vis_columns.iter() {
            let col_box = gtk::Box::new(gtk::Orientation::Vertical, 1);
            col_box.set_size_request(100, -1);
            col_box.set_margin_end(4);

            // Clickable header label for sorting
            let sort_indicator = if sort_col.as_deref() == Some(&col.key) {
                if sort_asc {
                    " \u{25b2}"
                } else {
                    " \u{25bc}"
                }
            } else {
                ""
            };
            let header_btn =
                gtk::Button::with_label(&format!("{}{}", col.display_name, sort_indicator));
            header_btn.add_css_class("flat");
            header_btn.add_css_class("caption");
            header_btn.set_halign(gtk::Align::Start);

            let page_rc = Rc::clone(self);
            let key = col.key.clone();
            header_btn.connect_clicked(move |_btn| {
                {
                    let mut sc = page_rc.sort_column.borrow_mut();
                    let mut sa = page_rc.sort_ascending.borrow_mut();
                    if sc.as_deref() == Some(&key) {
                        *sa = !*sa;
                    } else {
                        *sc = Some(key.clone());
                        *sa = true;
                    }
                    *page_rc.current_page.borrow_mut() = 0;
                }
                page_rc.render_results_page();
            });

            // For unit-bearing columns, pair the sort label with a small
            // unit-menu chevron (ref `BuildUnitHeader`); otherwise use the label
            // alone. Either way the whole header cell stays 100px wide.
            if column_units::has_menu(&col.key) {
                let header_line = gtk::Box::new(gtk::Orientation::Horizontal, 0);
                header_btn.set_hexpand(true);
                header_line.append(&header_btn);
                header_line.append(&self.build_unit_menu_button(&col.key));
                col_box.append(&header_line);
            } else {
                col_box.append(&header_btn);
            }

            // Per-column filter entry — restore existing filter text
            let filter_entry = gtk::Entry::new();
            filter_entry.set_placeholder_text(Some(crate::tr_en!("Filter...")));
            filter_entry.set_width_chars(10);
            filter_entry.add_css_class("caption");
            if let Some(existing) = self.column_filters.borrow().get(&col.key) {
                filter_entry.set_text(existing);
            }
            let filters_rc = self.column_filters.clone();
            let key2 = col.key.clone();
            let key3 = col.key.clone();
            let apply_btn = self.apply_filters_btn.clone();
            let page_rc = Rc::clone(self);
            filter_entry.connect_changed(move |entry| {
                let text = entry.text().to_string();
                let active = {
                    let mut f = filters_rc.borrow_mut();
                    if text.is_empty() {
                        f.remove(&key2);
                    } else {
                        f.insert(key2.clone(), text);
                    }
                    !f.is_empty()
                };
                apply_btn.set_visible(active);
                // Re-render, so typing a filter narrows the TABLE. Clicking a
                // cell's "narrow to this value" always did; typing the same
                // filter only revealed the Apply button, so the identical
                // constraint behaved two different ways.
                page_rc.schedule_filter_render(&key3);
            });
            self.filter_entries
                .borrow_mut()
                .insert(col.key.clone(), filter_entry.clone());
            col_box.append(&filter_entry);

            header_row.append(&col_box);
        }
        self.results_panel.append(&header_row);
        self.results_panel
            .append(&gtk::Separator::new(gtk::Orientation::Horizontal));

        // Data rows
        for row in processed.iter().skip(start).take(ps) {
            let row_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            row_box.set_margin_top(1);
            row_box.set_margin_bottom(1);

            for col in vis_columns.iter() {
                let raw = row.get(&col.header);
                // Unit-bearing columns render through the chosen-unit formatter
                // (RA/Dec sexagesimal by default); all others keep the fixed
                // per-column formatter to preserve existing behaviour.
                let formatted = if column_units::has_menu(&col.key) {
                    let chosen = self.column_units.borrow().get(&col.key).cloned();
                    format_cell_with_unit(&col.header, raw, chosen.as_deref())
                } else {
                    format_cell(raw, col.format)
                };

                // Identity columns become "narrow to this value" links: a click
                // sets a client-side column filter and re-renders (ref
                // `NarrowableKeys` / `IsNarrowable`).
                if is_narrowable(&col.key) && !raw.is_empty() {
                    let inner = gtk::Label::new(Some(&formatted));
                    inner.add_css_class("caption");
                    inner.set_halign(gtk::Align::Start);
                    inner.set_ellipsize(gtk::pango::EllipsizeMode::End);

                    let cell_btn = gtk::Button::new();
                    cell_btn.set_child(Some(&inner));
                    cell_btn.add_css_class("flat");
                    cell_btn.set_size_request(100, -1);
                    cell_btn.set_halign(gtk::Align::Start);
                    cell_btn.set_margin_end(4);
                    cell_btn.set_tooltip_text(Some(&format!("Narrow to: {}", raw)));

                    let filters_rc = self.column_filters.clone();
                    let apply_btn = self.apply_filters_btn.clone();
                    let page_rc = Rc::clone(self);
                    let ckey = col.key.clone();
                    let cval = raw.to_string();
                    cell_btn.connect_clicked(move |_| {
                        filters_rc.borrow_mut().insert(ckey.clone(), cval.clone());
                        apply_btn.set_visible(true);
                        *page_rc.current_page.borrow_mut() = 0;
                        page_rc.render_results_page();
                    });
                    row_box.append(&cell_btn);
                } else {
                    let label = gtk::Label::new(Some(&formatted));
                    label.add_css_class("caption");
                    label.set_size_request(100, -1);
                    label.set_halign(gtk::Align::Start);
                    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
                    label.set_margin_end(4);
                    label.set_selectable(true);
                    row_box.append(&label);
                }
            }

            // "Save to Research" button at the end of the row — routes
            // through the same flow as the detail dialog button.
            let publisher_id = row.get("publisherID").to_string();
            if !publisher_id.is_empty() {
                let save_btn = gtk::Button::from_icon_name("bookmark-new-symbolic");
                save_btn.add_css_class("flat");
                save_btn.set_tooltip_text(Some(crate::tr_en!(
                    "Save to Research (downloads preview + FITS file)"
                )));
                save_btn.set_valign(gtk::Align::Center);
                let services = self.services.clone();
                let pub_id = publisher_id.clone();
                let raw = row.clone();
                let main_window = self.main_window.clone();
                save_btn.connect_clicked(move |_| {
                    let services = services.clone();
                    let pub_id = pub_id.clone();
                    let raw = raw.clone();
                    let main_window = main_window.clone();
                    glib::spawn_future_local(async move {
                        save_to_research(&services, &pub_id, &raw, &main_window).await;
                    });
                });
                row_box.append(&save_btn);

                // "Details" → full CAOM2 observation detail page.
                let details_btn = gtk::Button::from_icon_name("view-more-symbolic");
                details_btn.add_css_class("flat");
                details_btn.set_tooltip_text(Some(crate::tr_en!(
                    "View the full CAOM2 observation metadata"
                )));
                details_btn.set_valign(gtk::Align::Center);
                let pub_id_detail = publisher_id.clone();
                details_btn.connect_clicked(move |btn| {
                    if let Some(root) = btn.root().and_then(|r| r.downcast::<gtk::Window>().ok()) {
                        if let Some(app) = root.application() {
                            let ag: &gtk::gio::ActionGroup = app.upcast_ref();
                            ag.activate_action(
                                "open-observation-detail",
                                Some(&glib::Variant::from(pub_id_detail.as_str())),
                            );
                        }
                    }
                });
                row_box.append(&details_btn);
            }

            // Wrap row in a clickable button for detail modal
            let row_btn = gtk::Button::new();
            row_btn.set_child(Some(&row_box));
            row_btn.add_css_class("flat");
            row_btn.set_margin_start(0);
            row_btn.set_margin_end(0);

            let row_data: Vec<(String, String)> = columns
                .iter()
                .filter(|c| c.visible)
                .map(|c| {
                    let raw = row.get(&c.header);
                    let formatted = format_cell(raw, c.format);
                    (c.display_name.clone(), formatted)
                })
                .collect();
            // Try multiple possible header names for target name
            let target_name = {
                let t = row.get("Target Name");
                if t.is_empty() {
                    row.get("\"Target Name\"").to_string()
                } else {
                    t.to_string()
                }
            };
            let pub_id_for_detail = row.get("publisherID").to_string();
            let raw_row_for_detail = row.clone();
            let services_for_detail = self.services.clone();
            let main_window_for_detail = self.main_window.clone();
            row_btn.connect_clicked(move |_| {
                let data = row_data.clone();
                let name = target_name.clone();
                let pub_id = pub_id_for_detail.clone();
                let services = services_for_detail.clone();
                let raw_row = raw_row_for_detail.clone();
                let main_window = main_window_for_detail.clone();
                glib::spawn_future_local(async move {
                    show_row_detail(&name, &data, &pub_id, &raw_row, &services, &main_window).await;
                });
            });

            self.results_panel.append(&row_btn);
        }
    }

    /// Show the "Apply filters to ADQL" button only while filters are active.
    fn update_apply_filters_button(&self) {
        let active = !self.column_filters.borrow().is_empty();
        self.apply_filters_btn.set_visible(active);
    }

    /// Append the active client-side column filters to the current ADQL as a
    /// WHERE fragment and switch to the ADQL editor tab (ref
    /// `OnApplyFiltersToAdql` / `BuildFilteredAdql`).
    fn apply_filters_to_adql(self: &Rc<Self>) {
        let where_frag = {
            let filters = self.column_filters.borrow();
            if filters.is_empty() {
                return;
            }
            let columns = self
                .results_store
                .borrow()
                .as_ref()
                .map(|r| r.columns.clone())
                .unwrap_or_default();
            filter_to_adql::filters_to_where(&filters, &columns)
        };
        if where_frag.trim().is_empty() {
            return;
        }

        let buffer = self.adql_editor.buffer();
        let base = buffer
            .text(&buffer.start_iter(), &buffer.end_iter(), false)
            .to_string();
        let base = base.trim_end();
        if base.is_empty() {
            // Nothing to attach the WHERE to — leave the editor untouched.
            self.status_label
                .set_text(crate::tr_en!("Run a search first, then apply filters"));
            return;
        }

        // Reference always AND-appends (its generated ADQL always has a WHERE);
        // fall back to a fresh WHERE when the editor holds a WHERE-less query.
        let combined = if base.to_uppercase().contains("WHERE") {
            format!("{}\nAND {}", base, where_frag)
        } else {
            format!("{}\nWHERE {}", base, where_frag)
        };
        self.adql_editor.buffer().set_text(&combined);
        self.notebook.set_current_page(Some(2));
    }

    /// The resolver service currently selected in the dropdown.
    fn selected_resolver_service(&self) -> String {
        const SERVICES: [&str; 5] = ["ALL", "SIMBAD", "NED", "VIZIER", "NONE"];
        SERVICES
            .get(self.resolver.selected() as usize)
            .copied()
            .unwrap_or("ALL")
            .to_string()
    }

    /// Freeze resolver provenance from a successful resolution. Mirrors Windows
    /// `_resolverServiceUsed = result.Service ?? ResolverService` and
    /// `_resolutionEpoch = result.ResolvedAt`: the actual resolver that produced
    /// the coordinates (falling back to the selected service) and the resolution
    /// epoch (falling back to now if the result carried none).
    fn capture_resolver_provenance(&self, r: &ResolverResult, selected_service: &str) {
        let service = r
            .service
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(selected_service)
            .to_string();
        *self.resolver_service_used.borrow_mut() = Some(service);
        *self.resolution_epoch.borrow_mut() = Some(
            r.resolved_at
                .clone()
                .unwrap_or_else(|| chrono::Utc::now().to_rfc3339()),
        );
    }

    /// Clear resolver provenance whenever the coordinates it describes are cleared
    /// (target edit, form reset, unresolved result, or FITS-crosshair coords which
    /// come from no resolver). Mirrors Windows clearing `_resolverServiceUsed` /
    /// `_resolutionEpoch` alongside `ResolvedRA` / `ResolvedDec`.
    fn clear_resolver_provenance(&self) {
        *self.resolver_service_used.borrow_mut() = None;
        *self.resolution_epoch.borrow_mut() = None;
    }

    /// Debounced live target-name resolution (ref `ResolveTargetDebouncedAsync`).
    /// Each call bumps the generation token, so a pending 500 ms timeout or an
    /// in-flight network resolve is discarded once superseded by a newer edit.
    /// Re-render the results after a short pause in typing.
    ///
    /// Debounced rather than immediate: the filter runs over the whole result
    /// set (up to 10,000 rows) and then rebuilds a page of widgets, which is
    /// visible jank on every keystroke at the larger page sizes. The generation
    /// token means a burst of typing produces one render, not one per character.
    fn schedule_filter_render(self: &Rc<Self>, column_key: &str) {
        let my_gen = {
            let mut g = self.filter_generation.borrow_mut();
            *g = g.wrapping_add(1);
            *g
        };
        let page = Rc::clone(self);
        let column_key = column_key.to_string();
        glib::timeout_add_local_once(std::time::Duration::from_millis(200), move || {
            // Superseded by a newer keystroke while this was pending.
            if *page.filter_generation.borrow() != my_gen {
                return;
            }
            // Back to the first page: the row that was at page 3 of the old
            // result set is meaningless once the set has changed under it.
            *page.current_page.borrow_mut() = 0;
            page.render_results_page();

            // The render rebuilt the header, destroying the entry being typed
            // in. Put the caret back at the end of its replacement, or the user
            // types one character and the next goes nowhere.
            if let Some(entry) = page.filter_entries.borrow().get(&column_key) {
                entry.grab_focus();
                entry.set_position(-1);
            }
        });
    }

    fn schedule_target_resolve(self: &Rc<Self>) {
        // A changed target invalidates any previously resolved coordinates and
        // their resolver provenance.
        *self.resolved_ra.borrow_mut() = None;
        *self.resolved_dec.borrow_mut() = None;
        self.clear_resolver_provenance();

        let my_gen = {
            let mut g = self.resolve_generation.borrow_mut();
            *g = g.wrapping_add(1);
            *g
        };

        let text = self.target.text().to_string();
        let service = self.selected_resolver_service();

        // Name-only ("NONE") or empty target → no resolution, just clear status.
        if text.trim().is_empty() || service == "NONE" {
            self.resolver_status.set_text("");
            return;
        }

        let page = Rc::clone(self);
        glib::timeout_add_local_once(std::time::Duration::from_millis(500), move || {
            // Superseded by a newer keystroke while the debounce was pending.
            if *page.resolve_generation.borrow() != my_gen {
                return;
            }
            page.resolver_status.set_text(crate::tr_en!("Resolving..."));

            let page = Rc::clone(&page);
            glib::spawn_future_local(async move {
                let svc = page.services.clone();
                let t = text.clone();
                let rs = service.clone();
                let result = page
                    .services
                    .spawn(async move {
                        let token = svc.get_token().await;
                        svc.tap.resolve_target(&t, &rs, token.as_deref()).await
                    })
                    .await;

                // Discard a stale in-flight resolve whose token no longer matches.
                if *page.resolve_generation.borrow() != my_gen {
                    return;
                }

                match result {
                    Ok(r) => {
                        *page.resolved_ra.borrow_mut() = Some(r.ra);
                        *page.resolved_dec.borrow_mut() = Some(r.dec);
                        page.capture_resolver_provenance(&r, &service);
                        let type_suffix = r
                            .object_type
                            .as_deref()
                            .filter(|s| !s.trim().is_empty())
                            .map(|s| format!(" ({})", s))
                            .unwrap_or_default();
                        page.resolver_status.set_text(&format!(
                            "RA: {:.4}  Dec: {:.4}{}",
                            r.ra, r.dec, type_suffix
                        ));
                    }
                    Err(_) => {
                        *page.resolved_ra.borrow_mut() = None;
                        *page.resolved_dec.borrow_mut() = None;
                        page.clear_resolver_provenance();
                        page.resolver_status.set_text(crate::tr_en!("Not found"));
                    }
                }
            });
        });
    }

    /// Persist the current per-column display-unit map so choices survive restarts
    /// (search_store `column_units.json`; mirrors the Windows column-unit store).
    fn persist_column_units(&self) {
        let _ = self
            .services
            .search_store
            .save_column_units(&self.column_units.borrow());
    }

    /// Build the small unit-menu chevron for a unit-bearing column header (ref
    /// `BuildUnitHeader`). Selecting a unit stores the choice (or clears it for
    /// "Default" / the coordinate sexagesimal default) and re-renders the grid.
    fn build_unit_menu_button(self: &Rc<Self>, key: &str) -> gtk::MenuButton {
        let menu_btn = gtk::MenuButton::new();
        menu_btn.set_icon_name("pan-down-symbolic");
        menu_btn.add_css_class("flat");
        menu_btn.set_valign(gtk::Align::Center);
        menu_btn.set_tooltip_text(Some(crate::tr_en!("Display unit")));

        let popover = gtk::Popover::new();
        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 2);
        vbox.set_margin_start(6);
        vbox.set_margin_end(6);
        vbox.set_margin_top(6);
        vbox.set_margin_bottom(6);

        let is_coord = key == "ra(j20000)" || key == "dec(j20000)";
        let default_id = column_units::default_unit_id(key);
        let active = self.column_units.borrow().get(key).cloned();

        // Radio-group leader: the CheckButtons act as a single-select toggle set.
        let mut group_leader: Option<gtk::CheckButton> = None;

        // Non-coordinate columns keep a readable legacy default → offer an
        // explicit "Default" choice that clears the stored unit.
        if !is_coord {
            let def = gtk::CheckButton::with_label(crate::tr_en!("Default"));
            def.set_active(active.is_none());
            group_leader = Some(def.clone());
            {
                let page = Rc::clone(self);
                let key_c = key.to_string();
                let pop = popover.clone();
                def.connect_toggled(move |b| {
                    if b.is_active() {
                        page.column_units.borrow_mut().remove(&key_c);
                        page.persist_column_units();
                        pop.popdown();
                        // Defer the rebuild: this handler's own popover/menu is a
                        // child of the grid we are about to tear down.
                        let page = page.clone();
                        glib::idle_add_local_once(move || page.render_results_page());
                    }
                });
            }
            vbox.append(&def);
            vbox.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        }

        for choice in column_units::available_units(key) {
            let item = gtk::CheckButton::with_label(choice.label);
            match &group_leader {
                Some(leader) => item.set_group(Some(leader)),
                None => group_leader = Some(item.clone()),
            }
            // Checked when explicitly chosen, or — for coords with no explicit
            // choice — the sexagesimal default id.
            let checked = active.as_deref() == Some(choice.id)
                || (active.is_none() && is_coord && Some(choice.id) == default_id);
            item.set_active(checked);
            {
                let page = Rc::clone(self);
                let key_c = key.to_string();
                let id = choice.id.to_string();
                let default_owned = default_id.map(|s| s.to_string());
                let pop = popover.clone();
                item.connect_toggled(move |b| {
                    if !b.is_active() {
                        return;
                    }
                    // The coord default choice (hms/dms) maps back to "no stored
                    // unit" → the sexagesimal default render.
                    if is_coord && Some(id.as_str()) == default_owned.as_deref() {
                        page.column_units.borrow_mut().remove(&key_c);
                    } else {
                        page.column_units
                            .borrow_mut()
                            .insert(key_c.clone(), id.clone());
                    }
                    page.persist_column_units();
                    pop.popdown();
                    // Defer the rebuild: this handler's own popover/menu is a
                    // child of the grid we are about to tear down.
                    let page = page.clone();
                    glib::idle_add_local_once(move || page.render_results_page());
                });
            }
            vbox.append(&item);
        }

        popover.set_child(Some(&vbox));
        menu_btn.set_popover(Some(&popover));
        menu_btn
    }

    fn save_current_query(self: &Rc<Self>) {
        let buffer = self.adql_editor.buffer();
        let adql = buffer
            .text(&buffer.start_iter(), &buffer.end_iter(), false)
            .to_string();
        if adql.trim().is_empty() {
            self.status_label.set_text(crate::tr_en!("No ADQL to save"));
            return;
        }
        let name = self.save_name_entry.text().to_string();
        let name = if name.trim().is_empty() {
            format!("Query {}", chrono::Utc::now().format("%H:%M"))
        } else {
            name
        };
        let query = SavedQuery {
            name,
            adql,
            created_at: chrono::Utc::now().to_rfc3339(),
            // User-authored save from the UI — no agent badge.
            agent_attribution: None,
        };
        let _ = self.services.search_store.save_query(query);
        self.save_name_entry.set_text("");
        self.refresh_saved();
        self.status_label.set_text(crate::tr_en!("Query saved"));
    }

    /// Export the result set as delimited text — EVERY column, RAW values.
    fn export_delimited(&self, delimiter: &str) -> String {
        match &*self.results_store.borrow() {
            Some(results) => delimited_export(results, delimiter),
            None => String::new(),
        }
    }

    async fn export_to_file(
        &self,
        parent: &impl IsA<gtk::Widget>,
        delimiter: &str,
        ext: &str,
        label: &str,
    ) {
        let content = self.export_delimited(delimiter);
        if content.is_empty() {
            self.status_label
                .set_text(crate::tr_en!("No results to export"));
            return;
        }

        let root = parent.root().and_downcast::<gtk::Window>();
        let filter = gtk::FileFilter::new();
        filter.add_pattern(&format!("*.{}", ext));
        filter.set_name(Some(label));
        let filters = gtk4::gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);

        let dialog = gtk::FileDialog::builder()
            .title(format!("Export as {}", ext.to_uppercase()))
            .initial_name(format!("search_results.{}", ext))
            .filters(&filters)
            .build();

        if let Ok(file) = dialog.save_future(root.as_ref()).await {
            if let Some(path) = file.path() {
                match std::fs::write(&path, &content) {
                    Ok(()) => {
                        self.status_label.set_text(&format!(
                            "Exported to {}",
                            path.file_name().unwrap_or_default().to_string_lossy()
                        ));
                    }
                    Err(e) => {
                        self.status_label.set_text(&format!("Export failed: {}", e));
                    }
                }
            }
        }
    }

    async fn show_column_selector(self: &Rc<Self>, parent: &impl IsA<gtk::Widget>) {
        let root = parent.root().and_downcast::<gtk::Window>();

        let dialog = adw::Window::builder()
            .title(crate::tr_en!("Select Columns"))
            .default_width(500)
            .default_height(400)
            .modal(true)
            .build();
        if let Some(ref w) = root {
            dialog.set_transient_for(Some(w));
        }

        let toolbar_view = adw::ToolbarView::new();
        let header = adw::HeaderBar::new();
        toolbar_view.add_top_bar(&header);

        let scroll = gtk::ScrolledWindow::new();
        scroll.set_vexpand(true);

        // Get current columns
        let columns = {
            let store = self.results_store.borrow();
            match &*store {
                Some(r) => build_columns_from_headers(&r.columns),
                None => default_columns(),
            }
        };

        // Build checkbox grid (3 columns like Windows)
        let grid = gtk::Grid::new();
        grid.set_column_spacing(12);
        grid.set_row_spacing(6);
        grid.set_margin_start(12);
        grid.set_margin_end(12);
        grid.set_margin_top(12);
        grid.set_margin_bottom(12);
        grid.set_column_homogeneous(true);

        let rows_per_col = columns.len().div_ceil(3);
        let checks: Rc<RefCell<Vec<(String, gtk::CheckButton)>>> =
            Rc::new(RefCell::new(Vec::new()));

        for (i, col) in columns.iter().enumerate() {
            let grid_col = (i / rows_per_col) as i32;
            let grid_row = (i % rows_per_col) as i32;

            let check = gtk::CheckButton::with_label(&col.display_name);
            check.set_active(self.is_col_visible(col));
            grid.attach(&check, grid_col, grid_row, 1, 1);
            checks.borrow_mut().push((col.key.clone(), check));
        }

        scroll.set_child(Some(&grid));

        // Apply button
        let btn_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        btn_box.set_margin_start(12);
        btn_box.set_margin_end(12);
        btn_box.set_margin_bottom(12);
        btn_box.set_halign(gtk::Align::End);

        let apply_btn = gtk::Button::with_label(crate::tr_en!("Apply"));
        apply_btn.add_css_class("suggested-action");
        let visibility_rc = self.column_visibility.clone();
        let checks_clone = checks.clone();
        let dialog_clone = dialog.clone();
        let current_page = self.current_page.clone();
        apply_btn.connect_clicked(move |_| {
            // Record every checkbox, not just the cleared ones: a ticked column
            // that is not visible by default has to be recorded as an explicit
            // `true`, or it can never be switched on.
            let mut chosen = std::collections::HashMap::new();
            for (key, check) in checks_clone.borrow().iter() {
                chosen.insert(key.clone(), check.is_active());
            }
            *visibility_rc.borrow_mut() = chosen;
            *current_page.borrow_mut() = 0;
            dialog_clone.close();
        });
        btn_box.append(&apply_btn);

        let cancel_btn = gtk::Button::with_label(crate::tr_en!("Cancel"));
        let dialog_clone2 = dialog.clone();
        cancel_btn.connect_clicked(move |_| {
            dialog_clone2.close();
        });
        btn_box.append(&cancel_btn);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(&scroll);
        content.append(&btn_box);
        toolbar_view.set_content(Some(&content));
        dialog.set_content(Some(&toolbar_view));

        // Re-render results when dialog closes
        let page_rc = Rc::clone(self);
        dialog.connect_close_request(move |_| {
            page_rc.render_results_page();
            gtk::glib::Propagation::Proceed
        });

        dialog.present();
    }

    fn refresh_recent(self: &Rc<Self>) {
        use crate::helpers::adql_summary;

        while let Some(child) = self.recent_list.first_child() {
            self.recent_list.remove(&child);
        }

        self.recent_list.add_css_class("boxed-list");

        for recent in self.services.search_store.load_recent() {
            // Use the user-provided summary as the title; fall back to the parsed
            // ADQL short summary if the stored one is empty.
            let title = if recent.summary.trim().is_empty() {
                adql_summary::short_summary(&recent.adql)
            } else {
                recent.summary.clone()
            };
            let when = adql_summary::format_saved_at(&recent.searched_at);
            let result_count_text = format!(
                "{} result{}",
                recent.result_count,
                if recent.result_count == 1 { "" } else { "s" }
            );
            let subtitle = if when.is_empty() {
                result_count_text
            } else {
                format!("{} · {}", result_count_text, when)
            };

            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(&title))
                .subtitle(glib::markup_escape_text(&subtitle))
                .activatable(true)
                .build();

            let icon = gtk::Image::from_icon_name("document-open-recent-symbolic");
            icon.add_css_class("dim-label");
            row.add_prefix(&icon);

            // Run button
            let run_btn = gtk::Button::from_icon_name("media-playback-start-symbolic");
            run_btn.add_css_class("flat");
            run_btn.set_valign(gtk::Align::Center);
            run_btn.set_tooltip_text(Some(crate::tr_en!("Re-run query")));
            {
                let page_rc = Rc::clone(self);
                let adql = recent.adql.clone();
                run_btn.connect_clicked(move |_| {
                    let adql = adql.clone();
                    let p = page_rc.clone();
                    glib::spawn_future_local(async move {
                        p.adql_editor.buffer().set_text(&adql);
                        p.run_query(&adql, p.max_records.value() as u32, None).await;
                        p.notebook.set_current_page(Some(1));
                        p.render_results_page();
                    });
                });
            }
            row.add_suffix(&run_btn);

            // Load into editor
            let load_btn = gtk::Button::from_icon_name("document-edit-symbolic");
            load_btn.add_css_class("flat");
            load_btn.set_valign(gtk::Align::Center);
            load_btn.set_tooltip_text(Some(crate::tr_en!("Load into ADQL editor")));
            {
                let adql = recent.adql.clone();
                let editor = self.adql_editor.clone();
                let notebook = self.notebook.clone();
                load_btn.connect_clicked(move |_| {
                    editor.buffer().set_text(&adql);
                    notebook.set_current_page(Some(2));
                });
            }
            row.add_suffix(&load_btn);

            // Remove button
            let remove_btn = gtk::Button::from_icon_name("user-trash-symbolic");
            remove_btn.add_css_class("flat");
            remove_btn.set_valign(gtk::Align::Center);
            remove_btn.set_tooltip_text(Some(crate::tr_en!("Remove")));
            {
                let page_rc = Rc::clone(self);
                let recent_adql = recent.adql.clone();
                remove_btn.connect_clicked(move |_| {
                    // One write, order preserved. Replaying the list through
                    // `save_recent` reversed it and merged entries sharing ADQL.
                    let mut all = page_rc.services.search_store.load_recent();
                    all.retain(|r| r.adql != recent_adql);
                    let _ = page_rc.services.search_store.save_all_recent(&all);
                    page_rc.refresh_recent();
                });
            }
            row.add_suffix(&remove_btn);

            // Row activation → restore the SEARCH FORM (ref `LoadRecentSearchCore`).
            // The pencil button beside it still loads the raw ADQL into the editor,
            // so both paths remain available.
            {
                let page_rc = Rc::clone(self);
                let state = recent.form_state.clone();
                let summary_text = title.clone();
                row.connect_activated(move |_| {
                    page_rc.load_from_form_state(&state);
                    page_rc
                        .status_label
                        .set_text(&format!("Loaded search: {}", summary_text));
                });
            }

            self.recent_list.append(&row);
        }
    }

    fn refresh_saved(self: &Rc<Self>) {
        use crate::helpers::adql_summary;

        // Clear existing rows
        while let Some(child) = self.saved_list.first_child() {
            self.saved_list.remove(&child);
        }

        // Apply boxed-list styling so AdwActionRows look like a proper card list
        self.saved_list.add_css_class("boxed-list");

        let saved_queries = self.services.search_store.load_saved();

        for saved in saved_queries {
            let summary = adql_summary::short_summary(&saved.adql);
            let when = adql_summary::format_saved_at(&saved.created_at);
            let subtitle = if when.is_empty() {
                summary
            } else {
                format!("{} · {}", summary, when)
            };

            let row = adw::ActionRow::builder()
                .title(glib::markup_escape_text(&saved.name))
                .subtitle(glib::markup_escape_text(&subtitle))
                .activatable(true)
                .build();

            // Prefix: query icon
            let icon = gtk::Image::from_icon_name("view-list-bullet-symbolic");
            icon.add_css_class("dim-label");
            row.add_prefix(&icon);

            // Agent provenance badge — shown only when an AI agent saved this
            // query over MCP (matches the Research list rows' inline AgentBadge).
            if let Some(stamp) = &saved.agent_attribution {
                row.add_suffix(&agent_badge(&saved_query_badge(stamp)));
            }

            // Suffix: Run + Details + Delete
            let run_btn = gtk::Button::from_icon_name("media-playback-start-symbolic");
            run_btn.add_css_class("flat");
            run_btn.set_valign(gtk::Align::Center);
            run_btn.set_tooltip_text(Some(crate::tr_en!("Run query")));
            {
                let page_rc = Rc::clone(self);
                let adql = saved.adql.clone();
                run_btn.connect_clicked(move |_| {
                    let adql = adql.clone();
                    let p = page_rc.clone();
                    glib::spawn_future_local(async move {
                        p.adql_editor.buffer().set_text(&adql);
                        p.run_query(&adql, p.max_records.value() as u32, None).await;
                        p.notebook.set_current_page(Some(1));
                        p.render_results_page();
                    });
                });
            }
            row.add_suffix(&run_btn);

            let view_btn = gtk::Button::from_icon_name("view-reveal-symbolic");
            view_btn.add_css_class("flat");
            view_btn.set_valign(gtk::Align::Center);
            view_btn.set_tooltip_text(Some(crate::tr_en!("View details")));
            {
                let page_rc = Rc::clone(self);
                let saved_c = saved.clone();
                view_btn.connect_clicked(move |_| {
                    let p = page_rc.clone();
                    let s = saved_c.clone();
                    glib::spawn_future_local(async move {
                        p.open_saved_query_details(s).await;
                    });
                });
            }
            row.add_suffix(&view_btn);

            let del_btn = gtk::Button::from_icon_name("user-trash-symbolic");
            del_btn.add_css_class("flat");
            del_btn.set_valign(gtk::Align::Center);
            del_btn.set_tooltip_text(Some(crate::tr_en!("Delete")));
            {
                let page_rc = Rc::clone(self);
                let name_for_del = saved.name.clone();
                del_btn.connect_clicked(move |_| {
                    let _ = page_rc.services.search_store.delete_saved(&name_for_del);
                    page_rc.refresh_saved();
                    page_rc
                        .status_label
                        .set_text(crate::tr_en!("Query deleted"));
                });
            }
            row.add_suffix(&del_btn);

            // Row activation (click / Enter) → open details dialog
            {
                let page_rc = Rc::clone(self);
                let saved_c = saved.clone();
                row.connect_activated(move |_| {
                    let p = page_rc.clone();
                    let s = saved_c.clone();
                    glib::spawn_future_local(async move {
                        p.open_saved_query_details(s).await;
                    });
                });
            }

            self.saved_list.append(&row);
        }
    }

    /// Open the saved-query detail dialog and handle the chosen action.
    async fn open_saved_query_details(
        self: &Rc<Self>,
        saved: crate::models::search_result::SavedQuery,
    ) {
        use crate::models::search_result::SavedQuery;
        use crate::ui::saved_query_dialog::{show_saved_query_dialog, SavedQueryAction};

        let action =
            show_saved_query_dialog(&self.widget, &saved.name, &saved.adql, &saved.created_at)
                .await;

        match action {
            SavedQueryAction::None => {}
            SavedQueryAction::Load => {
                self.adql_editor.buffer().set_text(&saved.adql);
                self.notebook.set_current_page(Some(2));
            }
            SavedQueryAction::Run => {
                self.adql_editor.buffer().set_text(&saved.adql);
                self.run_query(&saved.adql, self.max_records.value() as u32, None)
                    .await;
                self.notebook.set_current_page(Some(1));
                self.render_results_page();
            }
            SavedQueryAction::Rename(new_name) => {
                // Delete old entry then save with new name to preserve created_at
                let _ = self.services.search_store.delete_saved(&saved.name);
                let renamed = SavedQuery {
                    name: new_name,
                    adql: saved.adql,
                    created_at: saved.created_at,
                    // Renaming preserves the original provenance stamp.
                    agent_attribution: saved.agent_attribution,
                };
                let _ = self.services.search_store.save_query(renamed);
                self.refresh_saved();
                self.status_label.set_text(crate::tr_en!("Query renamed"));
            }
            SavedQueryAction::Delete => {
                let _ = self.services.search_store.delete_saved(&saved.name);
                self.refresh_saved();
                self.status_label.set_text(crate::tr_en!("Query deleted"));
            }
        }
    }

    async fn load_data_train(self: &Rc<Self>) {
        use crate::services::cache_service::{CacheKey, Freshness};
        use crate::services::health_tracker::{ServiceName, ServiceStatus};

        let cache_key = CacheKey::DataTrainRows;

        // Check cache first — serve fresh data without hitting the network
        if let Some(entry) = self
            .services
            .cache
            .read::<Vec<crate::helpers::data_train_manager::DataTrainRow>>(&cache_key)
        {
            let freshness = self.services.cache.entry_freshness(&cache_key, &entry);
            if freshness == Freshness::Fresh {
                let count = entry.data.len();
                self.train_manager.borrow_mut().load(entry.data);
                self.refresh_train_ui();
                self.status_label
                    .set_text(&format!("Data train loaded ({} entries)", count));
                self.services
                    .health
                    .set(ServiceName::Tap, ServiceStatus::Reachable);
                return;
            }
        }

        // Cache is stale or missing — try network
        self.status_label
            .set_text(crate::tr_en!("Loading data train..."));

        let svc = self.services.clone();
        let result = self
            .services
            .spawn(async move {
                let token = svc.get_token().await;
                svc.tap.fetch_data_train_rows(token.as_deref()).await
            })
            .await;

        match result {
            Ok(rows) => {
                let count = rows.len();
                self.services.cache.write(&cache_key, &rows);
                self.train_manager.borrow_mut().load(rows);
                self.refresh_train_ui();
                self.status_label
                    .set_text(&format!("Data train loaded ({} entries)", count));
                self.services
                    .health
                    .set(ServiceName::Tap, ServiceStatus::Reachable);
            }
            Err(e) => {
                // Network failed — try to serve any cached data regardless of TTL
                if let Some(entry) = self
                    .services
                    .cache
                    .read::<Vec<crate::helpers::data_train_manager::DataTrainRow>>(&cache_key)
                {
                    let count = entry.data.len();
                    let time_label = self
                        .services
                        .cache
                        .cached_time_label(&cache_key)
                        .unwrap_or_else(|| crate::tr_en!("unknown").into());
                    self.train_manager.borrow_mut().load(entry.data);
                    self.refresh_train_ui();
                    self.status_label.set_text(&format!(
                        "Data train loaded from cache ({} entries, last updated {})",
                        count, time_label
                    ));
                    self.services.toast.toast(format!(
                        "Archive unreachable — showing cached filters from {}",
                        time_label
                    ));
                } else {
                    self.status_label
                        .set_text(&format!("Data train failed: {}", e));
                    self.services.toast.toast_persistent(crate::tr_en!(
                        "Search filters unavailable — archive unreachable"
                    ));
                }
                self.services.health.set(
                    ServiceName::Tap,
                    ServiceStatus::Unreachable {
                        since: chrono::Utc::now(),
                        reason: e.to_string(),
                    },
                );
            }
        }
    }

    /// Rebuild all 7 data-train ListBox UIs from the manager's current state.
    ///
    /// Note the ordering inside the loop: each checkbox is restored with
    /// `set_active` BEFORE its `toggled` handler is connected. That is load-bearing
    /// — connecting first would fire the handler during the rebuild, and the
    /// handler takes `train_manager` mutably while this function holds it
    /// immutably, which is an immediate panic.
    fn refresh_train_ui(self: &Rc<Self>) {
        let mgr = self.train_manager.borrow();
        let all_lists: [(&gtk::ListBox, &[String], &std::collections::HashSet<String>); 7] = [
            (&self.train_lists[0], &mgr.all_bands, &mgr.available_bands),
            (
                &self.train_lists[1],
                &mgr.all_collections,
                &mgr.available_collections,
            ),
            (
                &self.train_lists[2],
                &mgr.all_instruments,
                &mgr.available_instruments,
            ),
            (
                &self.train_lists[3],
                &mgr.all_filters,
                &mgr.available_filters,
            ),
            (
                &self.train_lists[4],
                &mgr.all_cal_levels,
                &mgr.available_cal_levels,
            ),
            (
                &self.train_lists[5],
                &mgr.all_data_types,
                &mgr.available_data_types,
            ),
            (
                &self.train_lists[6],
                &mgr.all_obs_types,
                &mgr.available_obs_types,
            ),
        ];

        let selected_sets: [&std::collections::HashSet<String>; 7] = [
            &mgr.selected_bands,
            &mgr.selected_collections,
            &mgr.selected_instruments,
            &mgr.selected_filters,
            &mgr.selected_cal_levels,
            &mgr.selected_data_types,
            &mgr.selected_obs_types,
        ];

        for (idx, (list_box, all_values, available)) in all_lists.iter().enumerate() {
            while let Some(child) = list_box.first_child() {
                list_box.remove(&child);
            }
            for value in *all_values {
                let check = gtk::CheckButton::with_label(value);
                check.add_css_class("caption");

                // Gray out unavailable items
                if !available.contains(value) {
                    check.set_sensitive(false);
                }

                // Restore selection state
                if selected_sets[idx].contains(value) {
                    check.set_active(true);
                }

                // Wire toggle → cascade. Toggling one facet narrows the others,
                // so the whole panel has to be rebuilt: without it the grey-out
                // never updates, and values the cascade just cleared stay visibly
                // ticked while the model has dropped them.
                let value_owned = value.clone();
                let col_idx = idx;
                // Weak, so the closure a widget owns cannot keep the page alive.
                let weak = Rc::downgrade(self);
                check.connect_toggled(move |_btn| {
                    let Some(page) = weak.upgrade() else { return };
                    page.train_manager
                        .borrow_mut()
                        .toggle(col_idx, &value_owned);
                    page.status_label.set_text(crate::tr_en!("Filter updated"));
                    // Deferred: this handler's own checkbox is a child of the list
                    // we are about to tear down.
                    glib::idle_add_local_once(move || page.refresh_train_ui());
                });

                list_box.append(&check);
            }
        }
    }
}

// =============================================================================
// Narrow-to-value
// =============================================================================

/// Identity columns whose cell values can be clicked to "narrow to this value"
/// (a client-side column filter). Keys are cleaned column ids. Ref
/// `SearchPage.NarrowableKeys` / `IsNarrowable`.
fn is_narrowable(key: &str) -> bool {
    matches!(
        key,
        "collection" | "instrument" | "targetname" | "proposalid" | "piname"
    )
}

// =============================================================================
// Row detail dialog
// =============================================================================

async fn show_row_detail(
    target_name: &str,
    fields: &[(String, String)],
    publisher_id: &str,
    raw_row: &crate::models::search_result::SearchResultRow,
    services: &Arc<AppServices>,
    main_window: &adw::ApplicationWindow,
) {
    // ── Window ───────────────────────────────────────────────────────────
    let dialog = adw::Window::builder()
        .title(if target_name.is_empty() {
            crate::tr_en!("Observation Detail").to_string()
        } else {
            format!("Observation — {}", target_name)
        })
        .default_width(680)
        .default_height(580)
        .modal(true)
        .resizable(true)
        .transient_for(main_window)
        .build();
    dialog.set_width_request(400);
    dialog.set_height_request(360);

    let toolbar_view = adw::ToolbarView::new();

    // ── HeaderBar with title label + overflow menu ───────────────────────
    let header = adw::HeaderBar::new();

    let title_label = gtk::Label::new(Some(if target_name.is_empty() {
        crate::tr_en!("Observation Detail")
    } else {
        target_name
    }));
    title_label.add_css_class("heading");
    title_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    title_label.set_max_width_chars(40);
    header.set_title_widget(Some(&title_label));

    if !publisher_id.is_empty() {
        // Overflow menu with "Copy Publisher ID"
        let copy_btn = gtk::Button::from_icon_name("edit-copy-symbolic");
        copy_btn.add_css_class("flat");
        copy_btn.set_tooltip_text(Some(crate::tr_en!("Copy Publisher ID")));
        {
            let pub_id = publisher_id.to_string();
            let svc = services.clone();
            copy_btn.connect_clicked(move |btn| {
                let display = btn.display();
                display.clipboard().set_text(&pub_id);
                svc.toast.toast(crate::tr_en!("Publisher ID copied"));
            });
        }
        header.pack_end(&copy_btn);
    }

    toolbar_view.add_top_bar(&header);

    // ── Scrollable content: preview + metadata ──────────────────────────
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_vexpand(true);
    scroll.set_hexpand(true);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.set_margin_start(12);
    content.set_margin_end(12);
    content.set_margin_top(12);
    content.set_margin_bottom(12);

    // Preview Stack: loading / image / no-preview / error
    let preview_frame = gtk::Frame::new(None);
    preview_frame.add_css_class("card");
    preview_frame.set_margin_bottom(12);
    preview_frame.set_halign(gtk::Align::Center);

    let preview_stack = gtk::Stack::new();
    preview_stack.set_transition_type(gtk::StackTransitionType::Crossfade);
    preview_stack.set_transition_duration(200);
    preview_stack.set_size_request(360, 220);

    // "loading" child
    let loading_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    loading_box.set_halign(gtk::Align::Center);
    loading_box.set_valign(gtk::Align::Center);
    let spinner = gtk::Spinner::new();
    spinner.set_size_request(32, 32);
    spinner.start();
    loading_box.append(&spinner);
    preview_stack.add_named(&loading_box, Some("loading"));

    // "no-preview" child
    let no_preview = adw::StatusPage::new();
    no_preview.set_icon_name(Some("image-missing-symbolic"));
    no_preview.set_title(crate::tr_en!("No Preview"));
    no_preview.set_vexpand(false);
    preview_stack.add_named(&no_preview, Some("no-preview"));

    // "error" child
    let error_page = adw::StatusPage::new();
    error_page.set_icon_name(Some("network-error-symbolic"));
    error_page.set_title(crate::tr_en!("Preview Unavailable"));
    error_page.set_description(Some(crate::tr_en!("Check network connection")));
    error_page.set_vexpand(false);
    preview_stack.add_named(&error_page, Some("error"));

    preview_stack.set_visible_child_name("loading");
    preview_frame.set_child(Some(&preview_stack));

    if publisher_id.is_empty() {
        preview_frame.set_visible(false);
    } else {
        content.append(&preview_frame);

        // Async preview load — swap stack children on result
        let svc = services.clone();
        let pub_id = publisher_id.to_string();
        let stack_ref = preview_stack.clone();
        glib::spawn_future_local(async move {
            let dl_result = {
                let svc2 = svc.clone();
                let pid = pub_id.clone();
                svc.spawn(async move {
                    let token = svc2.get_token().await;
                    svc2.datalink.resolve(&pid, token.as_deref()).await
                })
                .await
            };

            let dl = match dl_result {
                Ok(d) => d,
                Err(_) => {
                    stack_ref.set_visible_child_name("error");
                    return;
                }
            };

            let preview_url = dl
                .files
                .iter()
                .find(|f| f.is_thumbnail())
                .or_else(|| dl.files.iter().find(|f| f.is_preview()))
                .map(|f| f.url.clone());

            let url = match preview_url {
                Some(u) => u,
                None => {
                    stack_ref.set_visible_child_name("no-preview");
                    return;
                }
            };

            let svc2 = svc.clone();
            let url_clone = url.clone();
            let bytes_result = svc
                .spawn(async move {
                    let token = svc2.get_token().await;
                    svc2.datalink
                        .download_image(&url_clone, token.as_deref())
                        .await
                })
                .await;

            let bytes = match bytes_result {
                Ok(b) => b,
                Err(_) => {
                    stack_ref.set_visible_child_name("error");
                    return;
                }
            };

            let gbytes = gtk::glib::Bytes::from(&bytes);
            let stream = gtk::gio::MemoryInputStream::from_bytes(&gbytes);
            let pixbuf = match gtk::gdk_pixbuf::Pixbuf::from_stream_future(&stream).await {
                Ok(p) => p,
                Err(_) => {
                    stack_ref.set_visible_child_name("error");
                    return;
                }
            };

            let texture = gtk::gdk::Texture::for_pixbuf(&pixbuf);
            let image = gtk::Picture::for_paintable(&texture);
            image.set_content_fit(gtk::ContentFit::Contain);
            image.set_size_request(360, 220);
            stack_ref.add_named(&image, Some("image"));
            stack_ref.set_visible_child_name("image");
        });
    }

    // Metadata group
    let metadata_group = adw::PreferencesGroup::new();
    metadata_group.set_title(crate::tr_en!("Observation Metadata"));

    for (label, value) in fields {
        if !value.is_empty() {
            let row = adw::ActionRow::builder()
                .title(label.as_str())
                .subtitle(value.as_str())
                .subtitle_selectable(true)
                .build();
            metadata_group.add(&row);
        }
    }
    content.append(&metadata_group);

    scroll.set_child(Some(&content));
    toolbar_view.set_content(Some(&scroll));

    // ── Fixed footer: gtk::ActionBar with Save + Download ───────────────
    let action_bar = gtk::ActionBar::new();

    // Save to Research button
    let save_btn = {
        let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        hbox.append(&gtk::Image::from_icon_name("bookmark-new-symbolic"));
        hbox.append(&gtk::Label::new(Some(crate::tr_en!("Save to Research"))));
        let btn = gtk::Button::new();
        btn.set_child(Some(&hbox));
        btn
    };
    // Save to Research is now the single primary action. It downloads both
    // the preview image and the FITS/FZ file to a managed directory; the
    // Research page then reads everything from disk with no further network
    // calls.  Mark it as the suggested action.
    save_btn.add_css_class("suggested-action");
    if publisher_id.is_empty() {
        save_btn.set_sensitive(false);
        save_btn.set_tooltip_text(Some(crate::tr_en!(
            "No publisher ID — observation cannot be saved"
        )));
    } else {
        save_btn.set_tooltip_text(Some(crate::tr_en!(
            "Download the preview and FITS file to the Research library"
        )));
    }
    {
        let svc = services.clone();
        let pub_id = publisher_id.to_string();
        let raw = raw_row.clone();
        let dialog_ref = dialog.clone();
        let main_window_ref = main_window.clone();
        save_btn.connect_clicked(move |_| {
            if pub_id.is_empty() {
                return;
            }
            let svc = svc.clone();
            let pub_id = pub_id.clone();
            let raw = raw.clone();
            let dialog_ref = dialog_ref.clone();
            let main_window_ref = main_window_ref.clone();
            glib::spawn_future_local(async move {
                // Close the dialog immediately — the download runs in the
                // background with toast feedback. The main window is passed
                // so any modal dialogs (multi-file picker) attach correctly.
                dialog_ref.close();
                save_to_research(&svc, &pub_id, &raw, &main_window_ref).await;
            });
        });
    }
    action_bar.pack_end(&save_btn);

    toolbar_view.add_bottom_bar(&action_bar);

    // ── Keyboard shortcut: Ctrl+S → Save to Research ────────────────────
    {
        let shortcuts = gtk::ShortcutController::new();
        shortcuts.set_scope(gtk::ShortcutScope::Local);

        let save_trigger = gtk::ShortcutTrigger::parse_string("<Control>s").unwrap();
        let save_action = {
            let save_btn = save_btn.clone();
            gtk::CallbackAction::new(move |_, _| {
                if save_btn.is_sensitive() {
                    save_btn.emit_clicked();
                }
                glib::Propagation::Stop
            })
        };
        shortcuts.add_shortcut(gtk::Shortcut::new(Some(save_trigger), Some(save_action)));

        dialog.add_controller(shortcuts);
    }

    dialog.set_content(Some(&toolbar_view));
    dialog.present();

    // Focus the save button (primary non-destructive action)
    if !publisher_id.is_empty() {
        save_btn.grab_focus();
    }
}

// =============================================================================
// Download flow
// =============================================================================

/// Save an observation to the Research library.  This is a single-action,
/// committing flow: it resolves DataLink, downloads the preview image AND
/// the FITS/FZ data file to a managed directory under
/// `~/.local/share/verbinal/observations/{obs_id}/`, and writes a store
/// record pointing at those local paths.  The Research page then reads
/// everything from disk with no further network calls.
async fn save_to_research(
    services: &Arc<AppServices>,
    publisher_id: &str,
    raw_row: &crate::models::search_result::SearchResultRow,
    main_window: &adw::ApplicationWindow,
) {
    use crate::services::managed_dir_for;

    // ── Duplicate check ───────────────────────────────────────────────
    let svc = services.clone();
    let pid = publisher_id.to_string();
    let already_saved = services
        .spawn(async move {
            let existing = svc.observation_store.load_async().await;
            existing.iter().any(|o| o.publisher_id == pid)
        })
        .await;
    if already_saved {
        services.toast.toast(crate::tr_en!("Already in Research"));
        return;
    }

    // ── Resolve DataLink ──────────────────────────────────────────────
    services.toast.toast(format!(
        "Resolving DataLink for {}…",
        short_pub_id(publisher_id)
    ));

    let svc = services.clone();
    let pid = publisher_id.to_string();
    let dl_result = services
        .spawn(async move {
            let token = svc.get_token().await;
            svc.datalink.resolve(&pid, token.as_deref()).await
        })
        .await;

    // Collect preview URL + science file selection BEFORE any disk writes
    let (science_url, science_filename, preview_url, dl_for_obs) = match dl_result {
        Ok(dl) => {
            let science_files: Vec<crate::models::search_result::DataLinkFile> = dl
                .files
                .iter()
                .filter(|f| f.is_science_data())
                .cloned()
                .collect();

            let picked_science = match science_files.len() {
                0 => None,
                1 => Some(science_files[0].clone()),
                _ => {
                    // Multi-file observation — let the user pick which artifact
                    crate::ui::datalink_file_dialog::show_datalink_file_dialog(
                        main_window,
                        science_files,
                    )
                    .await
                }
            };

            // If the user cancelled the multi-file picker, abort the whole save
            let (url, name) = match picked_science {
                Some(f) => (Some(f.url.clone()), Some(f.filename())),
                None if dl.files.iter().any(|f| f.is_science_data()) => {
                    // User cancelled the picker
                    return;
                }
                None => {
                    // No science file in DataLink — fall back to the synthesised
                    // download URL
                    (
                        dl.download_url
                            .clone()
                            .or_else(|| Some(services.datalink.download_url(publisher_id))),
                        None,
                    )
                }
            };

            let preview = dl
                .files
                .iter()
                .find(|f| f.is_thumbnail())
                .or_else(|| dl.files.iter().find(|f| f.is_preview()))
                .map(|f| (f.url.clone(), f.content_type.clone()));

            (url, name, preview, Some(dl))
        }
        Err(_) => {
            // DataLink failed — use the synthesised URL, no preview
            (
                Some(services.datalink.download_url(publisher_id)),
                None,
                None,
                None,
            )
        }
    };

    let science_url = match science_url {
        Some(u) => u,
        None => {
            services
                .toast
                .toast(crate::tr_en!("No science file found for this observation"));
            return;
        }
    };

    // ── Prepare the managed directory ─────────────────────────────────
    let obs_id = crate::helpers::caom2_uri::uuid_from_publisher_id(publisher_id);
    let managed_dir = managed_dir_for(&obs_id);
    if let Err(e) = std::fs::create_dir_all(&managed_dir) {
        services
            .toast
            .toast(format!("Cannot create storage directory: {}", e));
        return;
    }

    // ── Download the preview image (best effort, non-fatal) ──────────
    let mut local_preview_path = String::new();
    if let Some((url, content_type)) = &preview_url {
        services
            .toast
            .toast(crate::tr_en!("Downloading preview image…"));
        let svc = services.clone();
        let url_clone = url.clone();
        let preview_bytes = services
            .spawn(async move {
                let token = svc.get_token().await;
                svc.datalink
                    .download_image(&url_clone, token.as_deref())
                    .await
            })
            .await;
        if let Ok(bytes) = preview_bytes {
            let ext = preview_extension_from_content_type(content_type);
            let preview_path = managed_dir.join(format!("preview.{}", ext));
            if std::fs::write(&preview_path, &bytes).is_ok() {
                local_preview_path = preview_path.to_string_lossy().to_string();
            }
        }
    }

    // ── Download the FITS/FZ file (streamed to disk) ──────────────────
    let target_for_msg = raw_row.get("Target Name").to_string();
    let display_name = if !target_for_msg.is_empty() {
        target_for_msg
    } else {
        short_pub_id(publisher_id)
    };
    services
        .toast
        .toast(format!("Downloading {}…", display_name));

    // Choose a filename: prefer DataLink's name, fall back to URL extraction,
    // finally to "{obs_id}.fits". Computed up front because the stream writes
    // directly to the destination path (via a sibling ".tmp").
    let filename = science_filename
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| extract_filename(publisher_id, &science_url));
    let fits_path = managed_dir.join(&filename);

    // Stream the response body chunk-by-chunk on the tokio runtime, writing to a
    // sibling ".tmp" and renaming into place on success. Peak memory stays
    // bounded to a single chunk even for multi-GB cubes — the previous
    // implementation buffered the whole artifact into a Vec<u8> (an OOM risk).
    let svc = services.clone();
    let url_clone = science_url.clone();
    let dest = fits_path.clone();
    let toast_handle = services.toast.clone();
    let progress_label = display_name.clone();
    let dl_result = services
        .spawn(async move {
            let token = svc.get_token().await;
            stream_download_to_file(
                &url_clone,
                token.as_deref(),
                &dest,
                &toast_handle,
                &progress_label,
            )
            .await
        })
        .await;

    let file_size = match dl_result {
        Ok(n) => n,
        Err(e) => {
            // Clean up partial managed dir (the helper already removed its .tmp).
            crate::services::delete_managed_dir(&obs_id);
            services.toast.toast(format!("Download failed: {}", e));
            return;
        }
    };

    let local_path = fits_path.to_string_lossy().to_string();

    // ── Write the store record ────────────────────────────────────────
    let obs = build_downloaded_observation(
        publisher_id,
        raw_row,
        local_path,
        local_preview_path,
        file_size,
        dl_for_obs.as_ref(),
    );
    let svc = services.clone();
    let save_result = services
        .spawn(async move { svc.observation_store.save_async(obs).await })
        .await;

    match save_result {
        Ok(()) => {
            services.toast.toast_with_action(
                format!("Saved {}", display_name),
                crate::tr_en!("Go to Research"),
                "app.navigate-research",
            );
        }
        Err(e) => {
            // Leave the downloaded files on disk — the user can try again
            services
                .toast
                .toast(format!("Saved files, but store write failed: {}", e));
        }
    }
}

/// Stream an HTTP GET response body chunk-by-chunk into `dest`.
///
/// Writes to a sibling `<dest>.tmp` file first and renames it into place once
/// the transfer completes, so a partially-downloaded artifact never appears
/// under its final name. Peak memory stays bounded to a single chunk regardless
/// of file size — the whole point of streaming rather than buffering the body
/// into a `Vec<u8>`. Emits throttled progress toasts (percent + byte counts)
/// via `toast`. On any error the partial `.tmp` is removed and an `Err` is
/// returned. Returns the number of bytes written on success.
///
/// Runs on the tokio runtime (reqwest futures require it) — call it inside
/// `AppServices::spawn`.
///
/// `pub(crate)` so the Research page can reuse the exact same streaming idiom
/// when re-downloading a record whose local file went missing.
pub(crate) async fn stream_download_to_file(
    url: &str,
    token: Option<&str>,
    dest: &std::path::Path,
    toast: &crate::services::notification_service::ToastNotifier,
    label: &str,
) -> Result<u64, String> {
    use std::io::Write;

    // Sibling temp path: keep the real filename intact and just append ".tmp"
    // (so e.g. "foo.fits" -> "foo.fits.tmp"), avoiding any extension clash.
    let mut tmp_os = dest.as_os_str().to_owned();
    tmp_os.push(".tmp");
    let tmp_path = std::path::PathBuf::from(tmp_os);

    // A fresh client with a connect timeout but no overall request timeout —
    // multi-GB transfers legitimately run for minutes, so a whole-request
    // deadline would be wrong here.
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;

    let mut req = client.get(url);
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    let mut resp = req.send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status().as_u16()));
    }
    let total = resp.content_length();

    let file = std::fs::File::create(&tmp_path).map_err(|e| e.to_string())?;
    let mut writer = std::io::BufWriter::new(file);

    let mut downloaded: u64 = 0;
    let mut last_report = std::time::Instant::now();
    let mut last_pct: i64 = -1;
    let mut last_report_bytes: u64 = 0;

    loop {
        match resp.chunk().await {
            Ok(Some(chunk)) => {
                if let Err(e) = writer.write_all(&chunk) {
                    let _ = writer.flush();
                    let _ = std::fs::remove_file(&tmp_path);
                    return Err(e.to_string());
                }
                downloaded += chunk.len() as u64;

                // Throttle progress toasts so the overlay queue never floods:
                // report at most ~once/700ms, and only on a real advance
                // (>=1% when the total is known, else every >=64 MiB).
                let advanced = match total {
                    Some(t) if t > 0 => {
                        let pct = (downloaded.min(t) as i64 * 100) / t as i64;
                        if pct > last_pct {
                            last_pct = pct;
                            true
                        } else {
                            false
                        }
                    }
                    _ => downloaded.saturating_sub(last_report_bytes) >= 64 * 1024 * 1024,
                };
                if advanced && last_report.elapsed() >= std::time::Duration::from_millis(700) {
                    toast.toast(format_download_progress(label, downloaded, total));
                    last_report = std::time::Instant::now();
                    last_report_bytes = downloaded;
                }
            }
            Ok(None) => break,
            Err(e) => {
                let _ = writer.flush();
                let _ = std::fs::remove_file(&tmp_path);
                return Err(e.to_string());
            }
        }
    }

    if let Err(e) = writer.flush() {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(e.to_string());
    }
    drop(writer);

    // Replace any stale destination, then rename the completed temp into place.
    if dest.exists() {
        let _ = std::fs::remove_file(dest);
    }
    if let Err(e) = std::fs::rename(&tmp_path, dest) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(e.to_string());
    }

    Ok(downloaded)
}

/// Human-readable byte size (IEC units) for progress display.
fn format_byte_size(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    let b = bytes as f64;
    if b >= GIB {
        format!("{:.2} GiB", b / GIB)
    } else if b >= MIB {
        format!("{:.1} MiB", b / MIB)
    } else if b >= KIB {
        format!("{:.1} KiB", b / KIB)
    } else {
        format!("{} B", bytes)
    }
}

/// Build a progress-toast string, e.g.
/// `"Downloading M81… 128.0 MiB / 1.20 GiB (10%)"`, or — when the server sends
/// no Content-Length — `"Downloading M81… 128.0 MiB"`.
fn format_download_progress(label: &str, downloaded: u64, total: Option<u64>) -> String {
    match total {
        Some(t) if t > 0 => {
            let pct = (downloaded.min(t) as u128 * 100 / t as u128) as u64;
            format!(
                "Downloading {}… {} / {} ({}%)",
                label,
                format_byte_size(downloaded),
                format_byte_size(t),
                pct
            )
        }
        _ => format!("Downloading {}… {}", label, format_byte_size(downloaded)),
    }
}

/// Guess a file extension from an HTTP Content-Type header.
fn preview_extension_from_content_type(content_type: &str) -> &'static str {
    let lower = content_type.to_lowercase();
    if lower.contains("jpeg") || lower.contains("jpg") {
        "jpg"
    } else if lower.contains("png") {
        "png"
    } else if lower.contains("gif") {
        "gif"
    } else if lower.contains("webp") {
        "webp"
    } else {
        "bin"
    }
}

/// Construct a `DownloadedObservation` from a raw search result row, picking
/// up the CAOM2 column names used by the project's ADQL query (see
/// `helpers/adql_builder.rs`).
///
/// `local_path` and `local_preview_path` hold the final on-disk locations
/// of the FITS file and preview image respectively (or empty when absent).
/// `thumbnail_url` / `preview_url` are also persisted as provenance but the
/// Research page always prefers the local paths for display.
fn build_downloaded_observation(
    publisher_id: &str,
    raw_row: &crate::models::search_result::SearchResultRow,
    local_path: String,
    local_preview_path: String,
    file_size: u64,
    datalink: Option<&crate::models::search_result::DataLinkResult>,
) -> crate::services::DownloadedObservation {
    // Row lookup — `SearchResultRow::get` already returns an empty string on miss.
    let pick = |keys: &[&str]| -> String {
        for k in keys {
            let v = raw_row.get(k);
            if !v.is_empty() {
                return v.to_string();
            }
        }
        String::new()
    };

    // Preview URLs from DataLink (first match wins). Persisted for provenance
    // and as a fallback for records saved before the managed-storage redesign.
    let (thumbnail_url, preview_url) = match datalink {
        Some(dl) => {
            let thumb = dl
                .files
                .iter()
                .find(|f| f.is_thumbnail())
                .map(|f| f.url.clone())
                .unwrap_or_default();
            let prev = dl
                .files
                .iter()
                .find(|f| f.is_preview())
                .map(|f| f.url.clone())
                .unwrap_or_default();
            (thumb, prev)
        }
        None => (String::new(), String::new()),
    };

    crate::services::DownloadedObservation {
        id: crate::helpers::caom2_uri::uuid_from_publisher_id(publisher_id),
        publisher_id: publisher_id.to_string(),
        // Headers below MUST match the ADQL column aliases in
        // `helpers/adql_builder.rs::SELECT_COLUMNS`. If that file changes,
        // this list must follow.
        collection: pick(&["collection"]),
        observation_id: pick(&["Obs. ID", "observationID"]),
        target_name: pick(&["Target Name"]),
        instrument: pick(&["Instrument"]),
        filter: pick(&["Filter"]),
        ra: pick(&["RA (J2000.0)"]),
        dec: pick(&["Dec. (J2000.0)"]),
        start_date: pick(&["Start Date"]),
        cal_level: pick(&["Cal. Lev."]),
        local_path,
        file_size,
        downloaded_at: chrono::Utc::now().to_rfc3339(),
        thumbnail_url,
        preview_url,
        local_preview_path,
        // Citation handle. These four come straight off the result row, so a
        // search-page save carries everything `notes.md` needs to cite it.
        proposal_id: pick(&["Proposal ID"]),
        proposal_pi: pick(&["PI Name"]),
        proposal_title: pick(&["Proposal Title"]),
        data_release: pick(&["Data Release"]),
        // UI-initiated saves from the Search page have no agent provenance.
        agent_attribution: None,
    }
}

/// Stable, deterministic ID for a publisher DID — avoids duplicate entries
/// when the same observation is saved multiple times.
fn short_pub_id(pub_id: &str) -> String {
    pub_id
        .rsplit('/')
        .next()
        .unwrap_or(pub_id)
        .chars()
        .take(32)
        .collect()
}

fn extract_filename(publisher_id: &str, url: &str) -> String {
    // Try URL path first
    if let Some(name) = url.rsplit('/').next() {
        if !name.is_empty() && name.contains('.') {
            return name.to_string();
        }
    }
    // Try publisherID: "ivo://cadc.nrc.ca/CFHT?1100689/1100689o" → "1100689o"
    if let Some(after_slash) = publisher_id.rsplit('/').next() {
        if !after_slash.is_empty() {
            return format!("{}.fits", after_slash);
        }
    }
    if let Some(after_q) = publisher_id.rsplit('?').next() {
        if !after_q.is_empty() {
            return format!("{}.fits", after_q);
        }
    }
    "observation.fits".to_string()
}

// =============================================================================
// Column builder helpers
// =============================================================================

fn labeled_entry(label_text: &str, placeholder: &str) -> (gtk::Box, gtk::Entry) {
    let container = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let label = gtk::Label::new(Some(label_text));
    label.add_css_class("caption");
    label.set_halign(gtk::Align::Start);
    container.append(&label);
    let entry = gtk::Entry::new();
    entry.set_placeholder_text(Some(placeholder));
    container.append(&entry);
    (container, entry)
}

fn labeled_entry_with_combo(
    label_text: &str,
    placeholder: &str,
    items: &[&str],
) -> (gtk::Box, gtk::Entry, gtk::DropDown) {
    let container = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let label = gtk::Label::new(Some(label_text));
    label.add_css_class("caption");
    label.set_halign(gtk::Align::Start);
    container.append(&label);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let entry = gtk::Entry::new();
    entry.set_placeholder_text(Some(placeholder));
    entry.set_hexpand(true);
    row.append(&entry);
    let list = gtk::StringList::new(items);
    let combo = gtk::DropDown::new(Some(list), gtk::Expression::NONE);
    combo.set_size_request(80, -1);
    row.append(&combo);
    container.append(&row);
    (container, entry, combo)
}

fn build_observation_column() -> (
    gtk::Box,
    gtk::Entry,
    gtk::Entry,
    gtk::Entry,
    gtk::Entry,
    gtk::Entry,
    gtk::Entry,
    gtk::CheckButton,
    gtk::DropDown,
) {
    let col = gtk::Box::new(gtk::Orientation::Vertical, 6);

    let heading = gtk::Label::new(Some(crate::tr_en!("Observation")));
    heading.add_css_class("heading");
    heading.set_halign(gtk::Align::Start);
    col.append(&heading);

    let (w, observation_id) = labeled_entry(
        crate::tr_en!("Observation ID"),
        crate::tr_en!("e.g. jw01345*"),
    );
    // An id is an identifier, not a phrase: a bare value matches exactly, so say
    // so — the `*` in the placeholder is otherwise the only hint that partial
    // ids need one.
    observation_id.set_tooltip_text(Some(crate::tr_en!(
        "Matches the observation ID exactly (case-insensitive). Use * as a wildcard, e.g. jw01345*"
    )));
    col.append(&w);
    let (w, pi_name) = labeled_entry(crate::tr_en!("PI Name"), crate::tr_en!("e.g. Smith"));
    col.append(&w);
    let (w, proposal_id) = labeled_entry(crate::tr_en!("Proposal ID"), "");
    col.append(&w);
    let (w, proposal_title) = labeled_entry(crate::tr_en!("Proposal Title"), "");
    col.append(&w);
    let (w, keywords) = labeled_entry(crate::tr_en!("Keywords"), "");
    col.append(&w);
    let (w, data_release) = labeled_entry(
        crate::tr_en!("Data Release"),
        crate::tr_en!("e.g. > 2023-01-01"),
    );
    col.append(&w);

    let public_only = gtk::CheckButton::with_label(crate::tr_en!("Public only"));
    col.append(&public_only);

    let intent_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let intent_label = gtk::Label::new(Some(crate::tr_en!("Intent")));
    intent_label.add_css_class("caption");
    intent_label.set_halign(gtk::Align::Start);
    intent_box.append(&intent_label);
    let intent_list = gtk::StringList::new(&INTENTS);
    let intent = gtk::DropDown::new(Some(intent_list), gtk::Expression::NONE);
    intent_box.append(&intent);
    col.append(&intent_box);

    (
        col,
        observation_id,
        pi_name,
        proposal_id,
        proposal_title,
        keywords,
        data_release,
        public_only,
        intent,
    )
}

fn build_spatial_column() -> (
    gtk::Box,
    gtk::Entry,
    gtk::DropDown,
    gtk::SpinButton,
    gtk::Entry,
    gtk::DropDown,
    gtk::CheckButton,
    gtk::Label,
) {
    let col = gtk::Box::new(gtk::Orientation::Vertical, 6);

    let heading = gtk::Label::new(Some(crate::tr_en!("Spatial")));
    heading.add_css_class("heading");
    heading.set_halign(gtk::Align::Start);
    col.append(&heading);

    let (w, target) = labeled_entry(
        crate::tr_en!("Target or Coordinates"),
        crate::tr_en!("e.g. M31, NGC 1234"),
    );
    col.append(&w);

    let resolver_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let resolver_label = gtk::Label::new(Some(crate::tr_en!("Resolver")));
    resolver_label.add_css_class("caption");
    resolver_label.set_halign(gtk::Align::Start);
    resolver_box.append(&resolver_label);
    let resolver_list = gtk::StringList::new(&RESOLVER_SERVICES);
    let resolver = gtk::DropDown::new(Some(resolver_list), gtk::Expression::NONE);
    resolver_box.append(&resolver);
    col.append(&resolver_box);

    let resolver_status = gtk::Label::new(None);
    resolver_status.add_css_class("caption");
    resolver_status.add_css_class("dim-label");
    resolver_status.set_halign(gtk::Align::Start);
    resolver_status.set_wrap(true);
    col.append(&resolver_status);

    let radius_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let radius_label = gtk::Label::new(Some(crate::tr_en!("Radius (deg)")));
    radius_label.add_css_class("caption");
    radius_label.set_halign(gtk::Align::Start);
    radius_box.append(&radius_label);
    let radius = gtk::SpinButton::with_range(0.0, 10.0, 0.01);
    radius.set_digits(4);
    radius.set_value(0.0167);
    radius_box.append(&radius);
    col.append(&radius_box);

    let (w, pixel_scale, pixel_scale_unit) = labeled_entry_with_combo(
        crate::tr_en!("Pixel Scale"),
        crate::tr_en!("e.g. 0.1..1.0"),
        &PIXEL_SCALE_UNITS,
    );
    col.append(&w);

    let spatial_cutout = gtk::CheckButton::with_label(crate::tr_en!("Spatial cutout"));
    col.append(&spatial_cutout);

    (
        col,
        target,
        resolver,
        radius,
        pixel_scale,
        pixel_scale_unit,
        spatial_cutout,
        resolver_status,
    )
}

fn build_temporal_column() -> (
    gtk::Box,
    gtk::Entry,
    gtk::DropDown,
    gtk::Entry,
    gtk::DropDown,
    gtk::Entry,
) {
    let col = gtk::Box::new(gtk::Orientation::Vertical, 6);

    let heading = gtk::Label::new(Some(crate::tr_en!("Temporal")));
    heading.add_css_class("heading");
    heading.set_halign(gtk::Align::Start);
    col.append(&heading);

    let (w, obs_date, date_preset) = labeled_entry_with_combo(
        crate::tr_en!("Observation Date"),
        crate::tr_en!("e.g. 2020..2021"),
        &DATE_PRESETS,
    );
    col.append(&w);

    let (w, integration_time, time_unit) = labeled_entry_with_combo(
        crate::tr_en!("Integration Time"),
        crate::tr_en!("e.g. 100..3600"),
        &TIME_UNITS,
    );
    col.append(&w);

    let (w, time_span) = labeled_entry(crate::tr_en!("Time Span"), crate::tr_en!("e.g. 1..10 d"));
    col.append(&w);

    (
        col,
        obs_date,
        date_preset,
        integration_time,
        time_unit,
        time_span,
    )
}

fn build_spectral_column() -> (
    gtk::Box,
    gtk::Entry,
    gtk::DropDown,
    gtk::Entry,
    gtk::Entry,
    gtk::Entry,
    gtk::Entry,
    gtk::CheckButton,
) {
    let col = gtk::Box::new(gtk::Orientation::Vertical, 6);

    let heading = gtk::Label::new(Some(crate::tr_en!("Spectral")));
    heading.add_css_class("heading");
    heading.set_halign(gtk::Align::Start);
    col.append(&heading);

    let (w, spectral_coverage, spectral_unit) = labeled_entry_with_combo(
        crate::tr_en!("Spectral Coverage"),
        crate::tr_en!("e.g. 400..700"),
        &SPECTRAL_UNITS,
    );
    col.append(&w);

    let (w, spectral_sampling) = labeled_entry(crate::tr_en!("Spectral Sampling"), "");
    col.append(&w);
    let (w, resolving_power) = labeled_entry(
        crate::tr_en!("Resolving Power"),
        crate::tr_en!("e.g. 1000..5000"),
    );
    col.append(&w);
    let (w, bandpass_width) = labeled_entry(crate::tr_en!("Bandpass Width"), "");
    col.append(&w);
    let (w, rest_frame_energy) = labeled_entry(crate::tr_en!("Rest Frame Energy"), "");
    col.append(&w);

    let spectral_cutout = gtk::CheckButton::with_label(crate::tr_en!("Spectral cutout"));
    col.append(&spectral_cutout);

    (
        col,
        spectral_coverage,
        spectral_unit,
        spectral_sampling,
        resolving_power,
        bandpass_width,
        rest_frame_energy,
        spectral_cutout,
    )
}

fn build_data_train() -> (gtk::Grid, [gtk::ListBox; 7]) {
    let grid = gtk::Grid::new();
    grid.set_column_spacing(12);
    grid.set_column_homogeneous(true);
    grid.set_margin_start(12);
    grid.set_margin_end(12);
    grid.set_margin_top(12);
    grid.set_margin_bottom(12);

    let train_labels = [
        crate::tr_en!("Band"),
        crate::tr_en!("Collection"),
        crate::tr_en!("Instrument"),
        crate::tr_en!("Filter"),
        crate::tr_en!("Cal. Level"),
        crate::tr_en!("Data Type"),
        crate::tr_en!("Obs. Type"),
    ];

    let mut lists: Vec<gtk::ListBox> = Vec::new();

    for (i, label_text) in train_labels.iter().enumerate() {
        let col_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let label = gtk::Label::new(Some(label_text));
        label.add_css_class("caption");
        label.set_halign(gtk::Align::Start);
        col_box.append(&label);

        let scroll = gtk::ScrolledWindow::new();
        scroll.set_min_content_height(120);
        scroll.set_max_content_height(180);
        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::Multiple);
        let placeholder = gtk::Label::new(Some(crate::tr_en!("Loading...")));
        placeholder.add_css_class("dim-label");
        placeholder.add_css_class("caption");
        list.set_placeholder(Some(&placeholder));
        scroll.set_child(Some(&list));
        col_box.append(&scroll);

        grid.attach(&col_box, i as i32, 0, 1, 1);
        lists.push(list);
    }

    let arr: [gtk::ListBox; 7] = [
        lists[0].clone(),
        lists[1].clone(),
        lists[2].clone(),
        lists[3].clone(),
        lists[4].clone(),
        lists[5].clone(),
        lists[6].clone(),
    ];

    (grid, arr)
}

#[cfg(test)]
mod stream_download_tests {
    use super::{format_byte_size, format_download_progress};

    #[test]
    fn byte_size_scales_units() {
        assert_eq!(format_byte_size(0), "0 B");
        assert_eq!(format_byte_size(512), "512 B");
        assert_eq!(format_byte_size(1024), "1.0 KiB");
        assert_eq!(format_byte_size(1024 * 1024), "1.0 MiB");
        assert_eq!(format_byte_size(3 * 1024 * 1024 * 1024), "3.00 GiB");
    }

    #[test]
    fn progress_with_known_total_shows_percent() {
        let s = format_download_progress("M81", 512 * 1024 * 1024, Some(1024 * 1024 * 1024));
        assert!(s.contains("M81"), "{s}");
        assert!(s.contains("50%"), "{s}");
        assert!(s.contains('/'), "{s}");
    }

    #[test]
    fn progress_without_total_omits_percent() {
        let s = format_download_progress("M81", 10 * 1024 * 1024, None);
        assert!(s.contains("10.0 MiB"), "{s}");
        assert!(!s.contains('%'), "{s}");
        assert!(!s.contains('/'), "{s}");
    }

    #[test]
    fn progress_clamps_percent_when_downloaded_exceeds_total() {
        // Guards against >100% if Content-Length under-reports the body.
        let s = format_download_progress("X", 2048, Some(1024));
        assert!(s.contains("100%"), "{s}");
    }
}

#[cfg(test)]
mod date_preset_tests {
    use super::{preset_date_range, DATE_PRESETS};

    #[test]
    fn every_offered_preset_writes_a_range_except_the_blank_one() {
        // The blank first entry means "no preset" and must leave the date field
        // alone rather than writing today..today.
        assert_eq!(DATE_PRESETS[0], "");
        assert_eq!(preset_date_range(DATE_PRESETS[0]), None);

        for preset in DATE_PRESETS.iter().skip(1) {
            let range = preset_date_range(preset)
                .unwrap_or_else(|| panic!("`{preset}` is offered but writes nothing"));
            let (start, end) = range
                .split_once("..")
                .unwrap_or_else(|| panic!("`{preset}` produced `{range}`, not a range"));
            assert_eq!(start.len(), 10, "ISO date, not a timestamp: {range}");
            assert_eq!(end.len(), 10, "ISO date, not a timestamp: {range}");
            assert!(start < end, "the window must run forwards: {range}");
        }
    }

    #[test]
    fn the_window_shown_is_the_window_queried() {
        // The field is filled from the same rule the ADQL builder uses, so the
        // dates on screen cannot drift away from the results. A second copy of
        // these numbers is exactly how that drift would start.
        for preset in DATE_PRESETS.iter().skip(1) {
            let days = crate::helpers::adql_builder::preset_days_back(preset)
                .unwrap_or_else(|| panic!("`{preset}` has no window"));
            let range = preset_date_range(preset).unwrap();
            let (start, end) = range.split_once("..").unwrap();

            let start = chrono::NaiveDate::parse_from_str(start, "%Y-%m-%d").unwrap();
            let end = chrono::NaiveDate::parse_from_str(end, "%Y-%m-%d").unwrap();
            assert_eq!(
                (end - start).num_days(),
                days as i64,
                "`{preset}` shows a {}-day window but queries {days}",
                (end - start).num_days()
            );
        }
    }
}

#[cfg(test)]
mod combo_tests {
    //! The form's dropdowns are read back by INDEX — `form_state` does
    //! `SPECTRAL_UNITS.get(dropdown.selected())` — so the visible list and the
    //! value list must be the same array. They were separate literals, and when
    //! `SPECTRAL_UNITS` grew from 4 units to 14 the combo kept showing the old
    //! four: picking "Angstrom" at index 1 then meant `SPECTRAL_UNITS[1]`, which
    //! is "cm". A search in Ångström silently ran in centimetres.
    //!
    //! Every combo now renders from its own constant. This guard reads the
    //! source to prove it, because the alternative — a literal — compiles
    //! perfectly and fails only at runtime, in a unit no test would notice.

    /// Constants that both populate a dropdown and decode its selection.
    const INDEXED_BY_DROPDOWN: &[&str] = &[
        "PIXEL_SCALE_UNITS",
        "DATE_PRESETS",
        "TIME_UNITS",
        "SPECTRAL_UNITS",
        "INTENTS",
        "RESOLVER_SERVICES",
    ];

    /// The full argument list of a call, from `(` to its BALANCED `)`.
    ///
    /// Stopping at the first `)` would end inside `tr_en!("…")` — the first
    /// argument — and never reach the item list. That is exactly how the first
    /// version of this guard passed while the bug it was written for was still
    /// present, which is worth more than the guard itself as a lesson: a guard
    /// that cannot fail is worse than none, because it is trusted.
    fn call_arguments(source: &str, open_paren: usize) -> &str {
        let bytes = source.as_bytes();
        let mut depth = 0usize;
        for (offset, byte) in bytes[open_paren..].iter().enumerate() {
            match byte {
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        return &source[open_paren..open_paren + offset];
                    }
                }
                _ => {}
            }
        }
        &source[open_paren..]
    }

    #[test]
    fn every_combo_is_populated_from_the_constant_that_decodes_it() {
        let source = include_str!("mod.rs");
        let needle = "labeled_entry_with_combo(";

        let mut call_sites = 0;
        for (index, _) in source.match_indices(needle) {
            let args = call_arguments(source, index + needle.len() - 1);
            if args.contains("items: &[&str]") {
                continue; // the function definition itself
            }
            call_sites += 1;
            assert!(
                !args.contains("&["),
                "a combo is populated from an inline list rather than the constant \
                 that decodes its selection — the two drift, and the index then \
                 means different things on each side:{args}"
            );
        }
        assert!(
            call_sites >= 4,
            "expected to inspect every combo; found only {call_sites}"
        );

        // The plain `gtk::StringList` dropdowns are decoded by index the same
        // way and must come from their constants too.
        for (index, _) in source.match_indices("gtk::StringList::new(") {
            let args = call_arguments(source, index + "gtk::StringList::new(".len() - 1);
            assert!(
                !args.contains("&[\""),
                "a dropdown is populated from an inline list rather than the \
                 constant that decodes its selection:{args}"
            );
        }

        // And each constant is actually used to build a dropdown.
        for name in INDEXED_BY_DROPDOWN {
            assert!(
                source.contains(&format!("&{name}")),
                "`{name}` decodes a dropdown selection but does not populate one"
            );
        }
    }
}

#[cfg(test)]
mod export_tests {
    use super::delimited_export;
    use crate::models::search_result::{SearchResultRow, SearchResults};
    use std::collections::HashMap;

    fn results(columns: &[&str], rows: &[&[&str]]) -> SearchResults {
        SearchResults {
            columns: columns.iter().map(|c| c.to_string()).collect(),
            rows: rows
                .iter()
                .map(|cells| {
                    let mut values = HashMap::new();
                    for (col, cell) in columns.iter().zip(cells.iter()) {
                        values.insert(col.to_string(), cell.to_string());
                    }
                    SearchResultRow { values }
                })
                .collect(),
            query: None,
        }
    }

    #[test]
    fn values_are_exported_raw_not_as_the_grid_displays_them() {
        // The grid renders RA as sexagesimal; the file must carry the decimal
        // degrees TAP returned, or the export cannot be used for analysis.
        let r = results(
            &["RA (J2000.0)", "Int. Time"],
            &[&["10.684708333", "1200.0"]],
        );
        let csv = delimited_export(&r, ",");
        assert!(csv.contains("10.684708333"), "{csv}");
        assert!(!csv.contains("00:42:"), "no sexagesimal in the file: {csv}");
        assert!(csv.contains("1200.0"), "{csv}");
    }

    #[test]
    fn every_column_is_exported_not_just_the_visible_ones() {
        // Hiding a column to read the table more comfortably used to remove it
        // from every later export — a silent data loss the user never asked for.
        let r = results(
            &["Obs. ID", "Quality", "Provenance Name"],
            &[&["1234567p", "good", "MegaPipe"]],
        );
        let csv = delimited_export(&r, ",");
        let header = csv.lines().next().unwrap();
        assert_eq!(header, "Obs. ID,Quality,Provenance Name");
        assert!(csv.contains("MegaPipe"), "{csv}");
    }

    #[test]
    fn a_value_containing_the_delimiter_is_quoted() {
        let r = results(&["Proposal Title"], &[&["Dust, gas, and stars"]]);
        let csv = delimited_export(&r, ",");
        assert!(csv.contains("\"Dust, gas, and stars\""), "{csv}");
    }

    #[test]
    fn quotes_and_newlines_inside_a_value_do_not_break_the_record() {
        // A bare newline splits one row into two, which shifts every column
        // after it — the reader sees corrupt data rather than an error.
        let r = results(&["Note"], &[&["He said \"hi\"\nthen left"]]);
        let csv = delimited_export(&r, ",");
        let body = csv.lines().skip(1).collect::<Vec<_>>().join("\n");
        assert!(body.starts_with('"'), "the value must be quoted: {csv}");
        assert!(body.contains("\"\"hi\"\""), "quotes are doubled: {csv}");
    }

    #[test]
    fn a_tab_inside_a_tsv_value_becomes_a_space() {
        // TSV has no quoting convention, so an embedded tab would silently add
        // a column.
        let r = results(&["Target Name"], &[&["M31\tandromeda"]]);
        let tsv = delimited_export(&r, "\t");
        let row = tsv.lines().nth(1).unwrap();
        assert_eq!(row, "M31 andromeda");
        assert_eq!(row.matches('\t').count(), 0, "{tsv}");
    }

    #[test]
    fn a_missing_cell_exports_as_empty_and_keeps_the_columns_aligned() {
        let mut r = results(&["A", "B"], &[&["1", "2"]]);
        r.rows[0].values.remove("B");
        let csv = delimited_export(&r, ",");
        assert_eq!(csv.lines().nth(1).unwrap(), "1,");
    }
}
