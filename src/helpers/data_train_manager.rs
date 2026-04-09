/// Data Train Manager — cascade filtering logic matching the Windows implementation.
/// When the user selects bands, only collections available for those bands are shown.
/// When collections are selected, only instruments for those bands+collections are shown, etc.
use std::collections::{BTreeSet, HashSet};

#[derive(Debug, Clone, Default)]
pub struct DataTrainRow {
    pub band: String,
    pub collection: String,
    pub instrument: String,
    pub filter: String,
    pub calibration_level: String,
    pub data_product_type: String,
    pub observation_type: String,
}

pub struct DataTrainManager {
    rows: Vec<DataTrainRow>,
    // All distinct values (unfiltered)
    pub all_bands: Vec<String>,
    pub all_collections: Vec<String>,
    pub all_instruments: Vec<String>,
    pub all_filters: Vec<String>,
    pub all_cal_levels: Vec<String>,
    pub all_data_types: Vec<String>,
    pub all_obs_types: Vec<String>,
    // Available after cascade filtering
    pub available_bands: HashSet<String>,
    pub available_collections: HashSet<String>,
    pub available_instruments: HashSet<String>,
    pub available_filters: HashSet<String>,
    pub available_cal_levels: HashSet<String>,
    pub available_data_types: HashSet<String>,
    pub available_obs_types: HashSet<String>,
    // User selections
    pub selected_bands: HashSet<String>,
    pub selected_collections: HashSet<String>,
    pub selected_instruments: HashSet<String>,
    pub selected_filters: HashSet<String>,
    pub selected_cal_levels: HashSet<String>,
    pub selected_data_types: HashSet<String>,
    pub selected_obs_types: HashSet<String>,
}

impl DataTrainManager {
    pub fn new() -> Self {
        DataTrainManager {
            rows: Vec::new(),
            all_bands: Vec::new(),
            all_collections: Vec::new(),
            all_instruments: Vec::new(),
            all_filters: Vec::new(),
            all_cal_levels: Vec::new(),
            all_data_types: Vec::new(),
            all_obs_types: Vec::new(),
            available_bands: HashSet::new(),
            available_collections: HashSet::new(),
            available_instruments: HashSet::new(),
            available_filters: HashSet::new(),
            available_cal_levels: HashSet::new(),
            available_data_types: HashSet::new(),
            available_obs_types: HashSet::new(),
            selected_bands: HashSet::new(),
            selected_collections: HashSet::new(),
            selected_instruments: HashSet::new(),
            selected_filters: HashSet::new(),
            selected_cal_levels: HashSet::new(),
            selected_data_types: HashSet::new(),
            selected_obs_types: HashSet::new(),
        }
    }

    /// Load raw data train rows and compute all distinct values.
    pub fn load(&mut self, rows: Vec<DataTrainRow>) {
        self.rows = rows;

        let mut bands = BTreeSet::new();
        let mut collections = BTreeSet::new();
        let mut instruments = BTreeSet::new();
        let mut filters = BTreeSet::new();
        let mut cal_levels = BTreeSet::new();
        let mut data_types = BTreeSet::new();
        let mut obs_types = BTreeSet::new();

        for row in &self.rows {
            if !row.band.is_empty() {
                bands.insert(row.band.clone());
            }
            if !row.collection.is_empty() {
                collections.insert(row.collection.clone());
            }
            if !row.instrument.is_empty() {
                instruments.insert(row.instrument.clone());
            }
            if !row.filter.is_empty() {
                filters.insert(row.filter.clone());
            }
            if !row.calibration_level.is_empty() {
                cal_levels.insert(row.calibration_level.clone());
            }
            if !row.data_product_type.is_empty() {
                data_types.insert(row.data_product_type.clone());
            }
            if !row.observation_type.is_empty() {
                obs_types.insert(row.observation_type.clone());
            }
        }

        self.all_bands = bands.into_iter().collect();
        self.all_collections = collections.into_iter().collect();
        self.all_instruments = instruments.into_iter().collect();
        self.all_filters = filters.into_iter().collect();
        self.all_cal_levels = cal_levels.into_iter().collect();
        self.all_data_types = data_types.into_iter().collect();
        self.all_obs_types = obs_types.into_iter().collect();

        self.refresh();
    }

    /// Toggle a value in a column's selection. Clears downstream selections.
    /// column_index: 0=bands, 1=collections, 2=instruments, 3=filters, 4=cal_levels, 5=data_types, 6=obs_types
    pub fn toggle(&mut self, column_index: usize, value: &str) {
        let set = match column_index {
            0 => &mut self.selected_bands,
            1 => &mut self.selected_collections,
            2 => &mut self.selected_instruments,
            3 => &mut self.selected_filters,
            4 => &mut self.selected_cal_levels,
            5 => &mut self.selected_data_types,
            6 => &mut self.selected_obs_types,
            _ => return,
        };

        if set.contains(value) {
            set.remove(value);
        } else {
            set.insert(value.to_string());
        }

        // Clear downstream selections
        for i in (column_index + 1)..7 {
            match i {
                1 => self.selected_collections.clear(),
                2 => self.selected_instruments.clear(),
                3 => self.selected_filters.clear(),
                4 => self.selected_cal_levels.clear(),
                5 => self.selected_data_types.clear(),
                6 => self.selected_obs_types.clear(),
                _ => {}
            }
        }

        self.refresh();
    }

    /// Clear all selections and refresh.
    pub fn clear_all(&mut self) {
        self.selected_bands.clear();
        self.selected_collections.clear();
        self.selected_instruments.clear();
        self.selected_filters.clear();
        self.selected_cal_levels.clear();
        self.selected_data_types.clear();
        self.selected_obs_types.clear();
        self.refresh();
    }

    /// CASCADE FILTERING ALGORITHM (matches Windows exactly):
    /// Bands are never filtered. Each subsequent column is filtered by upstream selections.
    pub fn refresh(&mut self) {
        // Bands: always show all
        self.available_bands = self.all_bands.iter().cloned().collect();

        let mut filtered: Vec<&DataTrainRow> = self.rows.iter().collect();

        // 1. Filter by selected bands
        if !self.selected_bands.is_empty() {
            filtered.retain(|r| self.selected_bands.contains(&r.band));
        }
        self.available_collections = distinct(&filtered, |r| &r.collection);
        self.selected_collections
            .retain(|v| self.available_collections.contains(v));

        // 2. Filter by selected collections
        if !self.selected_collections.is_empty() {
            filtered.retain(|r| self.selected_collections.contains(&r.collection));
        }
        self.available_instruments = distinct(&filtered, |r| &r.instrument);
        self.selected_instruments
            .retain(|v| self.available_instruments.contains(v));

        // 3. Filter by selected instruments
        if !self.selected_instruments.is_empty() {
            filtered.retain(|r| self.selected_instruments.contains(&r.instrument));
        }
        self.available_filters = distinct(&filtered, |r| &r.filter);
        self.selected_filters
            .retain(|v| self.available_filters.contains(v));

        // 4. Filter by selected filters
        if !self.selected_filters.is_empty() {
            filtered.retain(|r| self.selected_filters.contains(&r.filter));
        }
        self.available_cal_levels = distinct(&filtered, |r| &r.calibration_level);
        self.selected_cal_levels
            .retain(|v| self.available_cal_levels.contains(v));

        // 5. Filter by selected cal levels
        if !self.selected_cal_levels.is_empty() {
            filtered.retain(|r| self.selected_cal_levels.contains(&r.calibration_level));
        }
        self.available_data_types = distinct(&filtered, |r| &r.data_product_type);
        self.selected_data_types
            .retain(|v| self.available_data_types.contains(v));

        // 6. Filter by selected data types
        if !self.selected_data_types.is_empty() {
            filtered.retain(|r| self.selected_data_types.contains(&r.data_product_type));
        }
        self.available_obs_types = distinct(&filtered, |r| &r.observation_type);
        self.selected_obs_types
            .retain(|v| self.available_obs_types.contains(v));
    }

    /// Get comma-separated selection strings for ADQL builder.
    pub fn bands_string(&self) -> String {
        join_set(&self.selected_bands)
    }
    pub fn collections_string(&self) -> String {
        join_set(&self.selected_collections)
    }
    pub fn instruments_string(&self) -> String {
        join_set(&self.selected_instruments)
    }
    pub fn filters_string(&self) -> String {
        join_set(&self.selected_filters)
    }
    pub fn cal_levels_string(&self) -> String {
        join_set(&self.selected_cal_levels)
    }
    pub fn data_types_string(&self) -> String {
        join_set(&self.selected_data_types)
    }
    pub fn obs_types_string(&self) -> String {
        join_set(&self.selected_obs_types)
    }
}

fn distinct<F>(rows: &[&DataTrainRow], field: F) -> HashSet<String>
where
    F: Fn(&DataTrainRow) -> &str,
{
    rows.iter()
        .map(|r| field(r).to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn join_set(set: &HashSet<String>) -> String {
    let mut sorted: Vec<&str> = set.iter().map(|s| s.as_str()).collect();
    sorted.sort();
    sorted.join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_rows() -> Vec<DataTrainRow> {
        vec![
            DataTrainRow {
                band: "Optical".into(),
                collection: "HST".into(),
                instrument: "ACS".into(),
                filter: "F606W".into(),
                calibration_level: "2".into(),
                data_product_type: "image".into(),
                observation_type: "science".into(),
            },
            DataTrainRow {
                band: "Optical".into(),
                collection: "HST".into(),
                instrument: "WFC3".into(),
                filter: "F110W".into(),
                calibration_level: "1".into(),
                data_product_type: "image".into(),
                observation_type: "science".into(),
            },
            DataTrainRow {
                band: "Infrared".into(),
                collection: "JWST".into(),
                instrument: "NIRCAM".into(),
                filter: "F070W".into(),
                calibration_level: "2".into(),
                data_product_type: "image".into(),
                observation_type: "science".into(),
            },
        ]
    }

    #[test]
    fn load_computes_all_values() {
        let mut mgr = DataTrainManager::new();
        mgr.load(sample_rows());
        assert_eq!(mgr.all_bands.len(), 2); // Infrared, Optical
        assert_eq!(mgr.all_collections.len(), 2); // HST, JWST
        assert_eq!(mgr.all_instruments.len(), 3); // ACS, NIRCAM, WFC3
    }

    #[test]
    fn cascade_filtering() {
        let mut mgr = DataTrainManager::new();
        mgr.load(sample_rows());

        // All collections available initially
        assert!(mgr.available_collections.contains("HST"));
        assert!(mgr.available_collections.contains("JWST"));

        // Select "Optical" band -> JWST should disappear from available collections
        mgr.toggle(0, "Optical");
        assert!(mgr.available_collections.contains("HST"));
        assert!(!mgr.available_collections.contains("JWST"));

        // Instruments: only ACS and WFC3 (HST instruments)
        assert_eq!(mgr.available_instruments.len(), 2);
        assert!(mgr.available_instruments.contains("ACS"));
        assert!(mgr.available_instruments.contains("WFC3"));
        assert!(!mgr.available_instruments.contains("NIRCAM"));
    }

    #[test]
    fn toggle_clears_downstream() {
        let mut mgr = DataTrainManager::new();
        mgr.load(sample_rows());

        mgr.toggle(1, "HST"); // select HST collection
        assert!(mgr.selected_collections.contains("HST"));

        mgr.toggle(0, "Infrared"); // select Infrared band -> clears collection selection
        assert!(mgr.selected_collections.is_empty());
    }

    #[test]
    fn clear_all() {
        let mut mgr = DataTrainManager::new();
        mgr.load(sample_rows());
        mgr.toggle(0, "Optical");
        mgr.toggle(1, "HST");
        mgr.clear_all();
        assert!(mgr.selected_bands.is_empty());
        assert!(mgr.selected_collections.is_empty());
        assert_eq!(mgr.available_collections.len(), 2); // all available again
    }

    #[test]
    fn string_output() {
        let mut mgr = DataTrainManager::new();
        mgr.load(sample_rows());
        mgr.toggle(0, "Optical");
        mgr.toggle(0, "Infrared");
        assert_eq!(mgr.bands_string(), "Infrared,Optical");
    }
}
