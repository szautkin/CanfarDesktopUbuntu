use crate::helpers::adql_builder;
use crate::helpers::data_train_manager::DataTrainManager;
use crate::helpers::range_parser;
use crate::helpers::unit_converter;
use crate::models::search_result::{
    build_columns_from_headers, default_columns, format_cell, RecentSearch, SavedQuery,
    SearchFormState, SearchResultRow, SearchResults,
};
use crate::state::AppServices;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

pub struct SearchPage {
    widget: gtk::Box,
    services: Arc<AppServices>,
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
    results_store: Rc<RefCell<Option<SearchResults>>>,
    current_page: Rc<RefCell<usize>>,
    page_size: Rc<RefCell<usize>>,
    sort_column: Rc<RefCell<Option<String>>>,
    sort_ascending: Rc<RefCell<bool>>,
    column_filters: Rc<RefCell<std::collections::HashMap<String, String>>>,
    hidden_columns: Rc<RefCell<std::collections::HashSet<String>>>,
}

const DEFAULT_PAGE_SIZE: usize = 100;

impl SearchPage {
    pub fn new(services: Arc<AppServices>) -> Rc<Self> {
        let widget = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        widget.set_vexpand(true);
        widget.set_hexpand(true);

        // =====================================================================
        // MAIN CONTENT (left, expandable)
        // =====================================================================
        let main_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        main_box.set_hexpand(true);
        main_box.set_margin_start(24);
        main_box.set_margin_top(24);
        main_box.set_margin_bottom(16);
        main_box.set_margin_end(16);

        let title = gtk::Label::new(Some("CADC Archive Search"));
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
        form_content.set_margin_start(4);
        form_content.set_margin_end(4);
        form_content.set_margin_top(12);
        form_content.set_margin_bottom(8);

        // --- 4 constraint columns (matching CADC web) ---
        let columns = gtk::Grid::new();
        columns.set_column_spacing(16);
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
        let train_expander = gtk::Expander::new(Some("Additional Constraints"));
        let (train_grid, train_lists) = build_data_train();
        train_expander.set_child(Some(&train_grid));
        train_expander.set_margin_top(8);
        form_content.append(&train_expander);

        form_scroll.set_child(Some(&form_content));
        form_tab.append(&form_scroll);

        // --- Pinned action bar (bottom of form tab, outside scroll) ---
        let action_bar = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        action_bar.set_margin_start(4);
        action_bar.set_margin_end(4);
        action_bar.set_margin_top(12);
        action_bar.set_margin_bottom(8);

        let search_btn = gtk::Button::new();
        let search_content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        search_content.append(&gtk::Image::from_icon_name("system-search-symbolic"));
        search_content.append(&gtk::Label::new(Some("Search")));
        search_btn.set_child(Some(&search_content));
        search_btn.add_css_class("suggested-action");
        action_bar.append(&search_btn);

        let reset_btn = gtk::Button::with_label("Reset");
        action_bar.append(&reset_btn);

        let max_label = gtk::Label::new(Some("Max Records"));
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

        notebook.append_page(&form_tab, Some(&gtk::Label::new(Some("Search Form"))));

        // ====== TAB 2: RESULTS ======
        let results_tab = gtk::Box::new(gtk::Orientation::Vertical, 0);
        results_tab.set_vexpand(true);

        // Results toolbar
        let results_toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        results_toolbar.set_margin_start(4);
        results_toolbar.set_margin_end(4);
        results_toolbar.set_margin_top(8);
        results_toolbar.set_margin_bottom(4);

        let results_count_label = gtk::Label::new(Some("No results"));
        results_count_label.add_css_class("caption");
        results_count_label.set_hexpand(true);
        results_count_label.set_halign(gtk::Align::Start);
        results_toolbar.append(&results_count_label);

        let columns_btn = gtk::Button::with_label("Columns");
        columns_btn.add_css_class("flat");
        columns_btn.set_tooltip_text(Some("Select visible columns"));
        results_toolbar.append(&columns_btn);

        let csv_btn = gtk::Button::with_label("CSV");
        csv_btn.add_css_class("flat");
        csv_btn.set_tooltip_text(Some("Export results as CSV file"));
        results_toolbar.append(&csv_btn);

        let tsv_btn = gtk::Button::with_label("TSV");
        tsv_btn.add_css_class("flat");
        tsv_btn.set_tooltip_text(Some("Export results as TSV file"));
        results_toolbar.append(&tsv_btn);

        let refresh_results_btn = gtk::Button::from_icon_name("view-refresh-symbolic");
        refresh_results_btn.set_tooltip_text(Some("Apply filters and re-render"));
        results_toolbar.append(&refresh_results_btn);

        let rows_label = gtk::Label::new(Some("Rows/page:"));
        rows_label.add_css_class("caption");
        results_toolbar.append(&rows_label);
        let rows_combo = gtk::DropDown::new(
            Some(gtk::StringList::new(&["25", "50", "100", "250", "500"])),
            gtk::Expression::NONE,
        );
        rows_combo.set_selected(2); // default 100
        results_toolbar.append(&rows_combo);

        results_tab.append(&results_toolbar);

        // Results scroll area
        let results_scroll = gtk::ScrolledWindow::new();
        results_scroll.set_vexpand(true);
        let results_panel = gtk::Box::new(gtk::Orientation::Vertical, 0);
        results_scroll.set_child(Some(&results_panel));
        results_tab.append(&results_scroll);

        // Pagination
        let page_bar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        page_bar.set_margin_start(4);
        page_bar.set_margin_end(4);
        page_bar.set_margin_top(8);
        page_bar.set_margin_bottom(8);

        let first_btn = gtk::Button::from_icon_name("go-first-symbolic");
        first_btn.set_tooltip_text(Some("First page"));
        page_bar.append(&first_btn);
        let prev_btn = gtk::Button::from_icon_name("go-previous-symbolic");
        prev_btn.set_tooltip_text(Some("Previous page"));
        page_bar.append(&prev_btn);
        let page_label = gtk::Label::new(Some("Page 1"));
        page_label.add_css_class("caption");
        page_bar.append(&page_label);
        let next_btn = gtk::Button::from_icon_name("go-next-symbolic");
        next_btn.set_tooltip_text(Some("Next page"));
        page_bar.append(&next_btn);
        let last_btn = gtk::Button::from_icon_name("go-last-symbolic");
        last_btn.set_tooltip_text(Some("Last page"));
        page_bar.append(&last_btn);

        results_tab.append(&page_bar);

        notebook.append_page(&results_tab, Some(&gtk::Label::new(Some("Results"))));

        // ====== TAB 3: ADQL EDITOR ======
        let adql_tab = gtk::Box::new(gtk::Orientation::Vertical, 8);
        adql_tab.set_margin_start(4);
        adql_tab.set_margin_end(4);
        adql_tab.set_margin_top(12);

        let adql_scroll = gtk::ScrolledWindow::new();
        adql_scroll.set_vexpand(true);
        let adql_editor = gtk::TextView::new();
        adql_editor.set_monospace(true);
        adql_editor.set_wrap_mode(gtk::WrapMode::Word);
        adql_editor.set_editable(true);
        adql_editor.set_margin_start(8);
        adql_editor.set_margin_end(8);
        adql_editor.set_margin_top(8);
        adql_editor.set_margin_bottom(8);
        adql_scroll.set_child(Some(&adql_editor));
        adql_tab.append(&adql_scroll);

        let adql_action = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let exec_btn = gtk::Button::new();
        let exec_content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        exec_content.append(&gtk::Image::from_icon_name("media-playback-start-symbolic"));
        exec_content.append(&gtk::Label::new(Some("Execute")));
        exec_btn.set_child(Some(&exec_content));
        exec_btn.add_css_class("suggested-action");
        adql_action.append(&exec_btn);
        adql_tab.append(&adql_action);

        notebook.append_page(&adql_tab, Some(&gtk::Label::new(Some("ADQL Editor"))));

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
        sidebar.set_margin_top(24);
        sidebar.set_margin_bottom(16);

        // Recent Searches card
        let recent_card = gtk::Box::new(gtk::Orientation::Vertical, 8);
        recent_card.add_css_class("card");
        recent_card.set_margin_start(4);
        recent_card.set_margin_end(4);

        let recent_header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        recent_header.set_margin_start(12);
        recent_header.set_margin_end(12);
        recent_header.set_margin_top(12);
        let recent_title = gtk::Label::new(Some("Recent Searches"));
        recent_title.add_css_class("heading");
        recent_title.set_hexpand(true);
        recent_title.set_halign(gtk::Align::Start);
        recent_header.append(&recent_title);
        let clear_recent_btn = gtk::Button::with_label("Clear All");
        clear_recent_btn.add_css_class("flat");
        clear_recent_btn.add_css_class("caption");
        recent_header.append(&clear_recent_btn);
        recent_card.append(&recent_header);

        let recent_list = gtk::ListBox::new();
        recent_list.set_selection_mode(gtk::SelectionMode::None);
        recent_list.set_margin_start(4);
        recent_list.set_margin_end(4);
        recent_list.set_margin_bottom(8);
        recent_list.set_placeholder(Some(
            &gtk::Label::builder()
                .label("No recent searches")
                .css_classes(vec!["dim-label".to_string(), "caption".to_string()])
                .margin_top(8)
                .margin_bottom(8)
                .build(),
        ));
        recent_card.append(&recent_list);
        sidebar.append(&recent_card);

        // Saved Queries card
        let saved_card = gtk::Box::new(gtk::Orientation::Vertical, 8);
        saved_card.add_css_class("card");
        saved_card.set_margin_start(4);
        saved_card.set_margin_end(4);

        let saved_title = gtk::Label::new(Some("Saved Queries"));
        saved_title.add_css_class("heading");
        saved_title.set_halign(gtk::Align::Start);
        saved_title.set_margin_start(12);
        saved_title.set_margin_top(12);
        saved_card.append(&saved_title);

        let save_row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        save_row.set_margin_start(12);
        save_row.set_margin_end(12);
        let save_name_entry = gtk::Entry::new();
        save_name_entry.set_placeholder_text(Some("Name (optional)"));
        save_name_entry.set_hexpand(true);
        save_row.append(&save_name_entry);
        let save_btn = gtk::Button::from_icon_name("document-save-symbolic");
        save_btn.set_tooltip_text(Some("Save current ADQL"));
        save_row.append(&save_btn);
        saved_card.append(&save_row);

        let saved_list = gtk::ListBox::new();
        saved_list.set_selection_mode(gtk::SelectionMode::None);
        saved_list.set_margin_start(4);
        saved_list.set_margin_end(4);
        saved_list.set_margin_bottom(8);
        saved_card.append(&saved_list);
        sidebar.append(&saved_card);

        sidebar_scroll.set_child(Some(&sidebar));
        widget.append(&sidebar_scroll);

        // =====================================================================
        // Build the struct
        // =====================================================================
        let page = Rc::new(SearchPage {
            widget,
            services,
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
            recent_list,
            saved_list,
            save_name_entry,
            train_lists,
            train_manager: Rc::new(RefCell::new(DataTrainManager::new())),
            status_label,
            search_spinner,
            resolved_ra: Rc::new(RefCell::new(None)),
            resolved_dec: Rc::new(RefCell::new(None)),
            results_store: Rc::new(RefCell::new(None)),
            current_page: Rc::new(RefCell::new(0)),
            page_size: Rc::new(RefCell::new(DEFAULT_PAGE_SIZE)),
            sort_column: Rc::new(RefCell::new(None)),
            sort_ascending: Rc::new(RefCell::new(true)),
            column_filters: Rc::new(RefCell::new(std::collections::HashMap::new())),
            hidden_columns: Rc::new(RefCell::new(std::collections::HashSet::new())),
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
                p.export_to_file(&btn, ",", "csv", "CSV Files").await;
            });
        });

        // TSV file export
        let p = page.clone();
        tsv_btn.connect_clicked(move |btn| {
            let p = p.clone();
            let btn = btn.clone();
            glib::spawn_future_local(async move {
                p.export_to_file(&btn, "\t", "tsv", "TSV Files").await;
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
            let sizes = [25, 50, 100, 250, 500];
            let idx = combo.selected() as usize;
            let new_size = sizes.get(idx).copied().unwrap_or(100);
            *p.page_size.borrow_mut() = new_size;
            *p.current_page.borrow_mut() = 0;
            p.render_results_page();
        });

        // Clear resolved coordinates when target name changes
        {
            let ra = page.resolved_ra.clone();
            let dec = page.resolved_dec.clone();
            let status = page.resolver_status.clone();
            page.target.connect_changed(move |_| {
                *ra.borrow_mut() = None;
                *dec.borrow_mut() = None;
                status.set_text("");
            });
        }

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

        // Load recent + saved
        page.refresh_recent();
        page.refresh_saved();

        // Load data train in background
        let p = page.clone();
        glib::spawn_future_local(async move { p.load_data_train().await });

        page
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.widget
    }

    fn build_form_state(&self) -> SearchFormState {
        let spectral_units = ["nm", "Angstrom", "um", "mm"];
        let time_units = ["s", "m", "h", "d"];
        let pixel_scale_units = ["arcsec", "arcmin", "deg"];
        let date_presets = ["", "Last 24 hours", "Last week", "Last month"];
        let intents = ["", "science", "calibration"];
        let resolver_services = ["ALL", "SIMBAD", "NED", "VIZIER"];

        let spectral_unit = spectral_units
            .get(self.spectral_unit.selected() as usize)
            .unwrap_or(&"nm")
            .to_string();
        let time_unit = time_units
            .get(self.time_unit.selected() as usize)
            .unwrap_or(&"s")
            .to_string();

        // Parse range fields using range_parser
        let parse_range_minmax =
            |entry: &gtk::Entry| -> (Option<f64>, Option<f64>) {
                let text = entry.text().to_string();
                match range_parser::parse_range(&text) {
                    Some(r) => match r.op {
                        range_parser::RangeOp::Between => (
                            r.value1.parse().ok(),
                            r.value2.and_then(|v| v.parse().ok()),
                        ),
                        range_parser::RangeOp::GreaterThan
                        | range_parser::RangeOp::GreaterThanOrEqual => {
                            (r.value1.parse().ok(), None)
                        }
                        range_parser::RangeOp::LessThan
                        | range_parser::RangeOp::LessThanOrEqual => {
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

        // Pixel scale — single max value, convert to degrees
        let ps_text = self.pixel_scale.text().to_string();
        let ps_max = ps_text.trim().parse::<f64>().ok();

        // Bandpass width
        let (bw_min, bw_max) = parse_range_minmax(&self.bandpass_width);

        // Spectral sampling
        let ss_val = self
            .spectral_sampling
            .text()
            .trim()
            .parse::<f64>()
            .ok();

        // Rest frame energy
        let (rfe_min, rfe_max) = parse_range_minmax(&self.rest_frame_energy);

        // Observation date — parse range for start/end
        let obs_date_text = self.obs_date.text().to_string();
        let (obs_start, obs_end) = match range_parser::parse_range(&obs_date_text) {
            Some(r) if r.op == range_parser::RangeOp::Between => (
                r.value1,
                r.value2.unwrap_or_default(),
            ),
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
            resolved_ra: *self.resolved_ra.borrow(),
            resolved_dec: *self.resolved_dec.borrow(),
            search_radius: self.radius.value(),
            pixel_scale_max: ps_max,
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
        }
    }

    fn clear_form(&self) {
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
        self.status_label.set_text("Form cleared");
    }

    async fn execute_search(self: &Rc<Self>) {
        let mut state = self.build_form_state();

        // Auto-resolve target if needed
        if !state.target.is_empty() && state.resolved_ra.is_none() {
            self.status_label.set_text("Resolving target...");
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
            self.status_label.set_text("Enter an ADQL query");
            return;
        }
        self.run_query(&adql, self.max_records.value() as u32, None)
            .await;
    }

    async fn run_query(self: &Rc<Self>, adql: &str, max_records: u32, form_state: Option<&SearchFormState>) {
        self.status_label.set_text("Searching...");
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
                    .unwrap_or_else(|| "ADQL query".to_string());
                let recent = RecentSearch {
                    summary,
                    adql: adql.to_string(),
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

    fn is_col_visible(&self, col: &crate::models::search_result::ResultColumnInfo) -> bool {
        col.visible && !self.hidden_columns.borrow().contains(&col.key)
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
        // Clear
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
        let hidden = self.hidden_columns.borrow().clone();
        let vis_columns: Vec<_> = columns
            .iter()
            .filter(|c| c.visible && !hidden.contains(&c.key))
            .cloned()
            .collect();
        let sort_col = self.sort_column.borrow().clone();
        let sort_asc = *self.sort_ascending.borrow();

        // Header row with clickable sort + filter entries
        let header_row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        header_row.set_margin_start(8);
        header_row.set_margin_end(8);
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
            col_box.append(&header_btn);

            // Per-column filter entry — restore existing filter text
            let filter_entry = gtk::Entry::new();
            filter_entry.set_placeholder_text(Some("Filter..."));
            filter_entry.set_width_chars(10);
            filter_entry.add_css_class("caption");
            if let Some(existing) = self.column_filters.borrow().get(&col.key) {
                filter_entry.set_text(existing);
            }
            let filters_rc = self.column_filters.clone();
            let key2 = col.key.clone();
            filter_entry.connect_changed(move |entry| {
                let text = entry.text().to_string();
                let mut f = filters_rc.borrow_mut();
                if text.is_empty() {
                    f.remove(&key2);
                } else {
                    f.insert(key2.clone(), text);
                }
            });
            col_box.append(&filter_entry);

            header_row.append(&col_box);
        }
        self.results_panel.append(&header_row);
        self.results_panel
            .append(&gtk::Separator::new(gtk::Orientation::Horizontal));

        // Data rows
        for row in processed.iter().skip(start).take(ps) {
            let row_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            row_box.set_margin_start(8);
            row_box.set_margin_end(8);
            row_box.set_margin_top(1);
            row_box.set_margin_bottom(1);

            for col in vis_columns.iter() {
                let raw = row.get(&col.header);
                let formatted = format_cell(raw, col.format);
                let label = gtk::Label::new(Some(&formatted));
                label.add_css_class("caption");
                label.set_size_request(100, -1);
                label.set_halign(gtk::Align::Start);
                label.set_ellipsize(gtk::pango::EllipsizeMode::End);
                label.set_margin_end(4);
                label.set_selectable(true);
                row_box.append(&label);
            }

            // Download button at end of row
            let publisher_id = row.get("publisherID").to_string();
            if !publisher_id.is_empty() {
                let dl_btn = gtk::Button::from_icon_name("folder-download-symbolic");
                dl_btn.add_css_class("flat");
                dl_btn.set_tooltip_text(Some("Download"));
                dl_btn.set_valign(gtk::Align::Center);
                let services = self.services.clone();
                let pub_id = publisher_id.clone();
                let status = self.status_label.clone();
                dl_btn.connect_clicked(move |btn| {
                    let services = services.clone();
                    let pub_id = pub_id.clone();
                    let status = status.clone();
                    let btn = btn.clone();
                    glib::spawn_future_local(async move {
                        download_observation(&services, &pub_id, &status, &btn).await;
                    });
                });
                row_box.append(&dl_btn);
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
            let services_for_detail = self.services.clone();
            row_btn.connect_clicked(move |_| {
                let data = row_data.clone();
                let name = target_name.clone();
                let pub_id = pub_id_for_detail.clone();
                let services = services_for_detail.clone();
                glib::spawn_future_local(async move {
                    show_row_detail(&name, &data, &pub_id, &services).await;
                });
            });

            self.results_panel.append(&row_btn);
        }
    }

    fn save_current_query(self: &Rc<Self>) {
        let buffer = self.adql_editor.buffer();
        let adql = buffer
            .text(&buffer.start_iter(), &buffer.end_iter(), false)
            .to_string();
        if adql.trim().is_empty() {
            self.status_label.set_text("No ADQL to save");
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
        };
        let _ = self.services.search_store.save_query(query);
        self.save_name_entry.set_text("");
        self.refresh_saved();
        self.status_label.set_text("Query saved");
    }

    fn export_csv(&self) -> String {
        let store = self.results_store.borrow();
        let Some(results) = &*store else {
            return String::new();
        };
        let columns = {
            let s2 = self.results_store.borrow();
            match &*s2 {
                Some(r) => build_columns_from_headers(&r.columns),
                None => default_columns(),
            }
        };
        let visible: Vec<_> = columns.iter().filter(|c| c.visible).collect();

        let mut csv = String::new();
        // Header
        let headers: Vec<&str> = visible.iter().map(|c| c.display_name.as_str()).collect();
        csv.push_str(&headers.join(","));
        csv.push('\n');
        // Rows
        for row in &results.rows {
            let cells: Vec<String> = visible
                .iter()
                .map(|col| {
                    let raw = row.get(&col.header);
                    let formatted = format_cell(raw, col.format);
                    if formatted.contains(',') || formatted.contains('"') {
                        format!("\"{}\"", formatted.replace('"', "\"\""))
                    } else {
                        formatted
                    }
                })
                .collect();
            csv.push_str(&cells.join(","));
            csv.push('\n');
        }
        csv
    }

    fn export_delimited(&self, delimiter: &str) -> String {
        let store = self.results_store.borrow();
        let Some(results) = &*store else {
            return String::new();
        };
        let columns = match &*store {
            Some(r) => build_columns_from_headers(&r.columns),
            None => default_columns(),
        };
        let hidden = self.hidden_columns.borrow();
        let visible: Vec<_> = columns
            .iter()
            .filter(|c| c.visible && !hidden.contains(&c.key))
            .collect();

        let mut out = String::new();
        // Header
        let headers: Vec<&str> = visible.iter().map(|c| c.display_name.as_str()).collect();
        out.push_str(&headers.join(delimiter));
        out.push('\n');
        // Rows
        for row in &results.rows {
            let cells: Vec<String> = visible
                .iter()
                .map(|col| {
                    let raw = row.get(&col.header);
                    let formatted = format_cell(raw, col.format);
                    if delimiter == "," && (formatted.contains(',') || formatted.contains('"')) {
                        format!("\"{}\"", formatted.replace('"', "\"\""))
                    } else if delimiter == "\t" {
                        formatted.replace('\t', " ")
                    } else {
                        formatted
                    }
                })
                .collect();
            out.push_str(&cells.join(delimiter));
            out.push('\n');
        }
        out
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
            self.status_label.set_text("No results to export");
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
            .title("Select Columns")
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

        let hidden = self.hidden_columns.borrow().clone();

        // Build checkbox grid (3 columns like Windows)
        let grid = gtk::Grid::new();
        grid.set_column_spacing(16);
        grid.set_row_spacing(4);
        grid.set_margin_start(16);
        grid.set_margin_end(16);
        grid.set_margin_top(16);
        grid.set_margin_bottom(16);
        grid.set_column_homogeneous(true);

        let rows_per_col = columns.len().div_ceil(3);
        let checks: Rc<RefCell<Vec<(String, gtk::CheckButton)>>> =
            Rc::new(RefCell::new(Vec::new()));

        for (i, col) in columns.iter().enumerate() {
            let grid_col = (i / rows_per_col) as i32;
            let grid_row = (i % rows_per_col) as i32;

            let check = gtk::CheckButton::with_label(&col.display_name);
            check.set_active(col.visible && !hidden.contains(&col.key));
            grid.attach(&check, grid_col, grid_row, 1, 1);
            checks.borrow_mut().push((col.key.clone(), check));
        }

        scroll.set_child(Some(&grid));

        // Apply button
        let btn_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        btn_box.set_margin_start(16);
        btn_box.set_margin_end(16);
        btn_box.set_margin_bottom(12);
        btn_box.set_halign(gtk::Align::End);

        let apply_btn = gtk::Button::with_label("Apply");
        apply_btn.add_css_class("suggested-action");
        let hidden_rc = self.hidden_columns.clone();
        let checks_clone = checks.clone();
        let dialog_clone = dialog.clone();
        let current_page = self.current_page.clone();
        apply_btn.connect_clicked(move |_| {
            let mut new_hidden = std::collections::HashSet::new();
            for (key, check) in checks_clone.borrow().iter() {
                if !check.is_active() {
                    new_hidden.insert(key.clone());
                }
            }
            *hidden_rc.borrow_mut() = new_hidden;
            *current_page.borrow_mut() = 0;
            dialog_clone.close();
        });
        btn_box.append(&apply_btn);

        let cancel_btn = gtk::Button::with_label("Cancel");
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
            run_btn.set_tooltip_text(Some("Re-run query"));
            {
                let page_rc = Rc::clone(self);
                let adql = recent.adql.clone();
                run_btn.connect_clicked(move |_| {
                    let adql = adql.clone();
                    let p = page_rc.clone();
                    glib::spawn_future_local(async move {
                        p.adql_editor.buffer().set_text(&adql);
                        p.run_query(&adql, p.max_records.value() as u32, None)
                            .await;
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
            load_btn.set_tooltip_text(Some("Load into ADQL editor"));
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
            remove_btn.set_tooltip_text(Some("Remove"));
            {
                let page_rc = Rc::clone(self);
                let recent_adql = recent.adql.clone();
                remove_btn.connect_clicked(move |_| {
                    let mut all = page_rc.services.search_store.load_recent();
                    all.retain(|r| r.adql != recent_adql);
                    let _ = page_rc.services.search_store.clear_recent();
                    for r in all.into_iter().rev() {
                        let _ = page_rc.services.search_store.save_recent(r);
                    }
                    page_rc.refresh_recent();
                });
            }
            row.add_suffix(&remove_btn);

            // Row activation → load into editor (lighter-weight than full dialog)
            {
                let adql = recent.adql.clone();
                let editor = self.adql_editor.clone();
                let notebook = self.notebook.clone();
                row.connect_activated(move |_| {
                    editor.buffer().set_text(&adql);
                    notebook.set_current_page(Some(2));
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

            // Suffix: Run + Details + Delete
            let run_btn = gtk::Button::from_icon_name("media-playback-start-symbolic");
            run_btn.add_css_class("flat");
            run_btn.set_valign(gtk::Align::Center);
            run_btn.set_tooltip_text(Some("Run query"));
            {
                let page_rc = Rc::clone(self);
                let adql = saved.adql.clone();
                run_btn.connect_clicked(move |_| {
                    let adql = adql.clone();
                    let p = page_rc.clone();
                    glib::spawn_future_local(async move {
                        p.adql_editor.buffer().set_text(&adql);
                        p.run_query(&adql, p.max_records.value() as u32, None)
                            .await;
                        p.notebook.set_current_page(Some(1));
                        p.render_results_page();
                    });
                });
            }
            row.add_suffix(&run_btn);

            let view_btn = gtk::Button::from_icon_name("view-reveal-symbolic");
            view_btn.add_css_class("flat");
            view_btn.set_valign(gtk::Align::Center);
            view_btn.set_tooltip_text(Some("View details"));
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
            del_btn.set_tooltip_text(Some("Delete"));
            {
                let page_rc = Rc::clone(self);
                let name_for_del = saved.name.clone();
                del_btn.connect_clicked(move |_| {
                    let _ = page_rc.services.search_store.delete_saved(&name_for_del);
                    page_rc.refresh_saved();
                    page_rc.status_label.set_text("Query deleted");
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

        let action = show_saved_query_dialog(
            &self.widget,
            &saved.name,
            &saved.adql,
            &saved.created_at,
        )
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
                };
                let _ = self.services.search_store.save_query(renamed);
                self.refresh_saved();
                self.status_label.set_text("Query renamed");
            }
            SavedQueryAction::Delete => {
                let _ = self.services.search_store.delete_saved(&saved.name);
                self.refresh_saved();
                self.status_label.set_text("Query deleted");
            }
        }
    }

    async fn load_data_train(&self) {
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
        self.status_label.set_text("Loading data train...");

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
                        .unwrap_or_else(|| "unknown".into());
                    self.train_manager.borrow_mut().load(entry.data);
                    self.refresh_train_ui();
                    self.status_label.set_text(&format!(
                        "Data train loaded from cache ({} entries, last updated {})",
                        count, time_label
                    ));
                    self.services.toast.toast(&format!(
                        "Archive unreachable — showing cached filters from {}",
                        time_label
                    ));
                } else {
                    self.status_label
                        .set_text(&format!("Data train failed: {}", e));
                    self.services
                        .toast
                        .toast_persistent("Search filters unavailable — archive unreachable");
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

    /// Rebuild all 7 data train ListBox UIs from the manager's available values.
    fn refresh_train_ui(&self) {
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

                // Wire toggle → cascade
                let mgr_ref = self.train_manager.clone();
                let value_owned = value.clone();
                let col_idx = idx;
                let this_status = self.status_label.clone();
                check.connect_toggled(move |_btn| {
                    mgr_ref.borrow_mut().toggle(col_idx, &value_owned);
                    // Refresh all lists downstream — for simplicity, defer to next idle
                    // We can't call self here, so just update status
                    this_status.set_text("Filter updated");
                });

                list_box.append(&check);
            }
        }
    }
}

// =============================================================================
// Row detail dialog
// =============================================================================

async fn show_row_detail(
    target_name: &str,
    fields: &[(String, String)],
    publisher_id: &str,
    services: &Arc<AppServices>,
) {
    let dialog = adw::Window::builder()
        .title(if target_name.is_empty() {
            "Observation Detail".to_string()
        } else {
            format!("Observation — {}", target_name)
        })
        .default_width(650)
        .default_height(500)
        .modal(true)
        .build();

    let toolbar_view = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    toolbar_view.add_top_bar(&header);

    let scroll = gtk::ScrolledWindow::new();
    scroll.set_vexpand(true);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
    content.set_margin_start(24);
    content.set_margin_end(24);
    content.set_margin_top(16);
    content.set_margin_bottom(24);

    // Preview image section
    if !publisher_id.is_empty() {
        let image_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        image_box.set_halign(gtk::Align::Center);
        image_box.set_margin_bottom(12);

        let spinner = gtk::Spinner::new();
        spinner.start();
        spinner.set_visible(true);
        image_box.append(&spinner);
        content.append(&image_box);

        // Load preview image in background
        let svc = services.clone();
        let pub_id = publisher_id.to_string();
        let image_box_clone = image_box.clone();
        let spinner_clone = spinner.clone();
        glib::spawn_future_local(async move {
            eprintln!("[preview] Resolving DataLink for: {}", pub_id);
            let result = {
                let svc2 = svc.clone();
                let pid = pub_id.clone();
                svc.spawn(async move {
                    let token = svc2.get_token().await;
                    svc2.datalink.resolve(&pid, token.as_deref()).await
                })
                    .await
            };

            spinner_clone.stop();
            spinner_clone.set_visible(false);

            match &result {
                Ok(dl_result) => {
                    eprintln!(
                        "[preview] DataLink resolved: {} files",
                        dl_result.files.len()
                    );
                    for f in &dl_result.files {
                        eprintln!(
                            "[preview]   {} | {} | {}",
                            f.semantics, f.content_type, f.url
                        );
                    }

                    // Prefer thumbnail (small, fast) then preview
                    let preview_url = dl_result
                        .files
                        .iter()
                        .find(|f| f.is_thumbnail())
                        .or_else(|| dl_result.files.iter().find(|f| f.is_preview()))
                        .map(|f| f.url.clone());

                    if let Some(url) = preview_url {
                        eprintln!("[preview] Downloading image: {}", url);
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

                        match bytes_result {
                            Ok(bytes) => {
                                eprintln!("[preview] Got {} bytes", bytes.len());
                                let gbytes = gtk::glib::Bytes::from(&bytes);
                                let stream = gtk::gio::MemoryInputStream::from_bytes(&gbytes);
                                match gtk::gdk_pixbuf::Pixbuf::from_stream(
                                    &stream,
                                    gtk::gio::Cancellable::NONE,
                                ) {
                                    Ok(pixbuf) => {
                                        eprintln!(
                                            "[preview] Pixbuf created: {}x{}",
                                            pixbuf.width(),
                                            pixbuf.height()
                                        );
                                        let texture = gtk::gdk::Texture::for_pixbuf(&pixbuf);
                                        let image = gtk::Picture::for_paintable(&texture);
                                        image.set_content_fit(gtk::ContentFit::Contain);
                                        image.set_size_request(300, 200);
                                        image_box_clone.append(&image);
                                        eprintln!("[preview] Image appended to dialog");
                                    }
                                    Err(e) => eprintln!("[preview] Pixbuf error: {}", e),
                                }
                            }
                            Err(e) => eprintln!("[preview] Download error: {}", e),
                        }
                    } else {
                        eprintln!("[preview] No preview/thumbnail URL found");
                        let no_preview = gtk::Label::new(Some("No preview available"));
                        no_preview.add_css_class("dim-label");
                        image_box_clone.append(&no_preview);
                    }
                }
                Err(e) => {
                    eprintln!("[preview] DataLink error: {}", e);
                    let err_label = gtk::Label::new(Some(&format!("Preview unavailable: {}", e)));
                    err_label.add_css_class("dim-label");
                    image_box_clone.append(&err_label);
                }
            }
        });
    }

    // Metadata fields
    let metadata_group = adw::PreferencesGroup::new();
    metadata_group.set_title("Observation Metadata");

    for (label, value) in fields {
        if !value.is_empty() {
            let row = adw::ActionRow::builder()
                .title(label.as_str())
                .subtitle(value.as_str())
                .build();
            metadata_group.add(&row);
        }
    }
    content.append(&metadata_group);

    // Download button
    if !publisher_id.is_empty() {
        let dl_btn = gtk::Button::new();
        let dl_content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        dl_content.append(&gtk::Image::from_icon_name("folder-download-symbolic"));
        dl_content.append(&gtk::Label::new(Some("Download FITS")));
        dl_btn.set_child(Some(&dl_content));
        dl_btn.add_css_class("suggested-action");
        dl_btn.set_halign(gtk::Align::Start);
        dl_btn.set_margin_top(12);

        let svc = services.clone();
        let pub_id = publisher_id.to_string();
        let dialog_ref = dialog.clone();
        dl_btn.connect_clicked(move |btn| {
            let svc = svc.clone();
            let pub_id = pub_id.clone();
            let btn = btn.clone();
            let dialog_ref = dialog_ref.clone();
            glib::spawn_future_local(async move {
                dialog_ref.close();
                download_observation_with_picker(&svc, &pub_id, &btn).await;
            });
        });
        content.append(&dl_btn);
    }

    scroll.set_child(Some(&content));
    toolbar_view.set_content(Some(&scroll));
    dialog.set_content(Some(&toolbar_view));
    dialog.present();
}

// =============================================================================
// Download flow
// =============================================================================

async fn download_observation(
    services: &Arc<AppServices>,
    publisher_id: &str,
    status: &gtk::Label,
    _parent: &impl IsA<gtk::Widget>,
) {
    status.set_text(&format!("Downloading {}...", publisher_id));

    // Resolve DataLink
    let svc = services.clone();
    let pid = publisher_id.to_string();
    let dl_result = services
        .spawn(async move {
            let token = svc.get_token().await;
            svc.datalink.resolve(&pid, token.as_deref()).await
        })
        .await;

    let url = match dl_result {
        Ok(ref dl) => dl
            .files
            .iter()
            .find(|f| f.is_science_data())
            .map(|f| f.url.clone())
            .or(dl.download_url.clone())
            .unwrap_or_else(|| crate::services::DataLinkService::download_url(publisher_id)),
        Err(_) => crate::services::DataLinkService::download_url(publisher_id),
    };

    // Download file
    let svc = services.clone();
    let url_clone = url.clone();
    let result = services
        .spawn(async move {
            let token = svc.get_token().await;
            svc.datalink
                .download_file(&url_clone, token.as_deref())
                .await
        })
        .await;

    match result {
        Ok((bytes, size)) => {
            let size_str = match size {
                Some(s) => format!("{:.1} MB", s as f64 / (1024.0 * 1024.0)),
                None => format!("{:.1} MB", bytes.len() as f64 / (1024.0 * 1024.0)),
            };

            // Extract filename from URL
            let filename = url
                .rsplit('/')
                .next()
                .unwrap_or("observation.fits")
                .to_string();

            // Save to ~/Downloads/verbinal/
            let download_dir = dirs_next().join(&filename);
            if let Err(e) = std::fs::write(&download_dir, &bytes) {
                status.set_text(&format!("Save failed: {}", e));
                return;
            }
            status.set_text(&format!("Downloaded {} ({})", filename, size_str));
        }
        Err(e) => {
            status.set_text(&format!("Download failed: {}", e));
        }
    }
}

async fn download_observation_with_picker(
    services: &Arc<AppServices>,
    publisher_id: &str,
    parent: &impl IsA<gtk::Widget>,
) {
    // Resolve DataLink
    let svc = services.clone();
    let pid = publisher_id.to_string();
    let dl_result = services
        .spawn(async move {
            let token = svc.get_token().await;
            svc.datalink.resolve(&pid, token.as_deref()).await
        })
        .await;

    let url = match dl_result {
        Ok(ref dl) => dl
            .files
            .iter()
            .find(|f| f.is_science_data())
            .map(|f| f.url.clone())
            .or(dl.download_url.clone())
            .unwrap_or_else(|| crate::services::DataLinkService::download_url(publisher_id)),
        Err(_) => crate::services::DataLinkService::download_url(publisher_id),
    };

    // Extract suggested filename
    let suggested = extract_filename(publisher_id, &url);

    // File save dialog
    let root = parent.root().and_downcast::<gtk::Window>();
    let dialog = gtk::FileDialog::builder()
        .title("Save Observation")
        .initial_name(&suggested)
        .build();

    let save_path = match dialog.save_future(root.as_ref()).await {
        Ok(file) => file.path(),
        Err(_) => return, // User cancelled
    };

    let Some(save_path) = save_path else { return };

    // Download
    let svc = services.clone();
    let url_clone = url.clone();
    let result = services
        .spawn(async move {
            let token = svc.get_token().await;
            svc.datalink
                .download_file(&url_clone, token.as_deref())
                .await
        })
        .await;

    match result {
        Ok((bytes, _)) => {
            // Atomic write: tmp + rename
            let tmp_path = save_path.with_extension("tmp");
            if let Err(e) = std::fs::write(&tmp_path, &bytes) {
                eprintln!("Write failed: {}", e);
                return;
            }
            if let Err(e) = std::fs::rename(&tmp_path, &save_path) {
                eprintln!("Rename failed: {}", e);
            }
        }
        Err(e) => {
            eprintln!("Download failed: {}", e);
        }
    }
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

fn dirs_next() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let dir = std::path::PathBuf::from(&home)
        .join("Downloads")
        .join("verbinal");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

// =============================================================================
// Column builder helpers
// =============================================================================

fn labeled_entry(label_text: &str, placeholder: &str) -> (gtk::Box, gtk::Entry) {
    let container = gtk::Box::new(gtk::Orientation::Vertical, 2);
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
    let container = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let label = gtk::Label::new(Some(label_text));
    label.add_css_class("caption");
    label.set_halign(gtk::Align::Start);
    container.append(&label);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
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
    let col = gtk::Box::new(gtk::Orientation::Vertical, 8);

    let heading = gtk::Label::new(Some("Observation"));
    heading.add_css_class("heading");
    heading.set_halign(gtk::Align::Start);
    col.append(&heading);

    let (w, observation_id) = labeled_entry("Observation ID", "e.g. jw01345*");
    col.append(&w);
    let (w, pi_name) = labeled_entry("PI Name", "e.g. Smith");
    col.append(&w);
    let (w, proposal_id) = labeled_entry("Proposal ID", "");
    col.append(&w);
    let (w, proposal_title) = labeled_entry("Proposal Title", "");
    col.append(&w);
    let (w, keywords) = labeled_entry("Keywords", "");
    col.append(&w);
    let (w, data_release) = labeled_entry("Data Release", "e.g. > 2023-01-01");
    col.append(&w);

    let public_only = gtk::CheckButton::with_label("Public only");
    col.append(&public_only);

    let intent_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let intent_label = gtk::Label::new(Some("Intent"));
    intent_label.add_css_class("caption");
    intent_label.set_halign(gtk::Align::Start);
    intent_box.append(&intent_label);
    let intent_list = gtk::StringList::new(&["", "science", "calibration"]);
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
    let col = gtk::Box::new(gtk::Orientation::Vertical, 8);

    let heading = gtk::Label::new(Some("Spatial"));
    heading.add_css_class("heading");
    heading.set_halign(gtk::Align::Start);
    col.append(&heading);

    let (w, target) = labeled_entry("Target or Coordinates", "e.g. M31, NGC 1234");
    col.append(&w);

    let resolver_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let resolver_label = gtk::Label::new(Some("Resolver"));
    resolver_label.add_css_class("caption");
    resolver_label.set_halign(gtk::Align::Start);
    resolver_box.append(&resolver_label);
    let resolver_list = gtk::StringList::new(&["ALL", "SIMBAD", "NED", "VIZIER"]);
    let resolver = gtk::DropDown::new(Some(resolver_list), gtk::Expression::NONE);
    resolver_box.append(&resolver);
    col.append(&resolver_box);

    let resolver_status = gtk::Label::new(None);
    resolver_status.add_css_class("caption");
    resolver_status.add_css_class("dim-label");
    resolver_status.set_halign(gtk::Align::Start);
    resolver_status.set_wrap(true);
    col.append(&resolver_status);

    let radius_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let radius_label = gtk::Label::new(Some("Radius (deg)"));
    radius_label.add_css_class("caption");
    radius_label.set_halign(gtk::Align::Start);
    radius_box.append(&radius_label);
    let radius = gtk::SpinButton::with_range(0.0, 10.0, 0.01);
    radius.set_digits(4);
    radius.set_value(0.0167);
    radius_box.append(&radius);
    col.append(&radius_box);

    let (w, pixel_scale, pixel_scale_unit) =
        labeled_entry_with_combo("Pixel Scale", "e.g. 0.1..1.0", &["arcsec", "arcmin", "deg"]);
    col.append(&w);

    let spatial_cutout = gtk::CheckButton::with_label("Spatial cutout");
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
    let col = gtk::Box::new(gtk::Orientation::Vertical, 8);

    let heading = gtk::Label::new(Some("Temporal"));
    heading.add_css_class("heading");
    heading.set_halign(gtk::Align::Start);
    col.append(&heading);

    let (w, obs_date, date_preset) = labeled_entry_with_combo(
        "Observation Date",
        "e.g. 2020..2021",
        &["", "Last24h", "LastWeek", "LastMonth"],
    );
    col.append(&w);

    let (w, integration_time, time_unit) =
        labeled_entry_with_combo("Integration Time", "e.g. 100..3600", &["s", "m", "h", "d"]);
    col.append(&w);

    let (w, time_span) = labeled_entry("Time Span", "e.g. 1..10 d");
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
    let col = gtk::Box::new(gtk::Orientation::Vertical, 8);

    let heading = gtk::Label::new(Some("Spectral"));
    heading.add_css_class("heading");
    heading.set_halign(gtk::Align::Start);
    col.append(&heading);

    let (w, spectral_coverage, spectral_unit) = labeled_entry_with_combo(
        "Spectral Coverage",
        "e.g. 400..700",
        &["nm", "Angstrom", "um", "mm"],
    );
    col.append(&w);

    let (w, spectral_sampling) = labeled_entry("Spectral Sampling", "");
    col.append(&w);
    let (w, resolving_power) = labeled_entry("Resolving Power", "e.g. 1000..5000");
    col.append(&w);
    let (w, bandpass_width) = labeled_entry("Bandpass Width", "");
    col.append(&w);
    let (w, rest_frame_energy) = labeled_entry("Rest Frame Energy", "");
    col.append(&w);

    let spectral_cutout = gtk::CheckButton::with_label("Spectral cutout");
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
    grid.set_column_spacing(8);
    grid.set_column_homogeneous(true);
    grid.set_margin_top(8);

    let train_labels = [
        "Band",
        "Collection",
        "Instrument",
        "Filter",
        "Cal. Level",
        "Data Type",
        "Obs. Type",
    ];

    let mut lists: Vec<gtk::ListBox> = Vec::new();

    for (i, label_text) in train_labels.iter().enumerate() {
        let col_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let label = gtk::Label::new(Some(label_text));
        label.add_css_class("caption");
        label.set_halign(gtk::Align::Start);
        col_box.append(&label);

        let scroll = gtk::ScrolledWindow::new();
        scroll.set_min_content_height(120);
        scroll.set_max_content_height(180);
        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::Multiple);
        let placeholder = gtk::Label::new(Some("Loading..."));
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
