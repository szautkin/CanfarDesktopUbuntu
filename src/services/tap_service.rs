use crate::config::ApiEndpoints;
use crate::models::search_result::{
    parse_csv, parse_resolver_response, ResolverResult, SearchResults,
};
use crate::services::api_error::ApiError;
use reqwest::Client;
use std::collections::BTreeSet;
use std::sync::Arc;

pub struct TAPService {
    client: Client,
    endpoints: Arc<ApiEndpoints>,
}

impl TAPService {
    pub fn new(client: Client, endpoints: Arc<ApiEndpoints>) -> Self {
        TAPService { client, endpoints }
    }

    /// Execute an ADQL query against the CADC TAP service.
    /// Returns parsed CSV results.
    pub async fn execute_query(
        &self,
        adql: &str,
        max_records: u32,
        token: Option<&str>,
    ) -> Result<SearchResults, ApiError> {
        let params = [
            ("LANG", "ADQL"),
            ("FORMAT", "csv"),
            ("MAXREC", &max_records.to_string()),
            ("QUERY", adql),
        ];

        let mut req = self
            .client
            .post(self.endpoints.tap_sync_url())
            .form(&params)
            .timeout(std::time::Duration::from_secs(300));
        if let Some(t) = token {
            req = req.bearer_auth(t);
        }
        let resp = req.send().await?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(ApiError::Server { status, body });
        }

        let csv = resp
            .text()
            .await
            .map_err(|e| ApiError::Parse(e.to_string()))?;

        Ok(parse_csv(&csv, Some(adql)))
    }

    /// Resolve a target name to RA/Dec coordinates using the CADC name resolver.
    pub async fn resolve_target(
        &self,
        target: &str,
        service: &str,
        token: Option<&str>,
    ) -> Result<ResolverResult, ApiError> {
        let url = format!(
            "{}?target={}&service={}&format=ascii&detail=max&cached=true",
            self.endpoints.resolver_find_url(),
            urlencoding::encode(target),
            service
        );

        let mut req = self
            .client
            .get(&url)
            .timeout(std::time::Duration::from_secs(15));
        if let Some(t) = token {
            req = req.bearer_auth(t);
        }
        let resp = req.send().await?;

        if !resp.status().is_success() {
            return Err(ApiError::Server {
                status: resp.status().as_u16(),
                body: format!("Could not resolve target '{}'", target),
            });
        }

        let text = resp
            .text()
            .await
            .map_err(|e| ApiError::Parse(e.to_string()))?;

        parse_resolver_response(&text, target)
            .ok_or_else(|| ApiError::Parse(format!("No coordinates found for '{}'", target)))
    }

    /// Fetch the data train (enumfield) from CADC.
    /// Returns 7 sorted lists: bands, collections, instruments, filters, cal_levels, data_types, obs_types.
    pub async fn fetch_data_train(&self, token: Option<&str>) -> Result<DataTrain, ApiError> {
        let adql = "SELECT energy_emBand, collection, instrument_name, \
                    energy_bandpassName, calibrationLevel, dataProductType, type \
                    FROM caom2.enumfield \
                    ORDER BY collection, instrument_name";

        let results = self.execute_query(adql, 50000, token).await?;

        let mut bands = BTreeSet::new();
        let mut collections = BTreeSet::new();
        let mut instruments = BTreeSet::new();
        let mut filters = BTreeSet::new();
        let mut cal_levels = BTreeSet::new();
        let mut data_types = BTreeSet::new();
        let mut obs_types = BTreeSet::new();

        for row in &results.rows {
            let add = |set: &mut BTreeSet<String>, key: &str| {
                let val = row.get(key).trim().to_string();
                if !val.is_empty() {
                    set.insert(val);
                }
            };
            add(&mut bands, "energy_emBand");
            add(&mut collections, "collection");
            add(&mut instruments, "instrument_name");
            add(&mut filters, "energy_bandpassName");
            add(&mut cal_levels, "calibrationLevel");
            add(&mut data_types, "dataProductType");
            add(&mut obs_types, "type");
        }

        Ok(DataTrain {
            bands: bands.into_iter().collect(),
            collections: collections.into_iter().collect(),
            instruments: instruments.into_iter().collect(),
            filters: filters.into_iter().collect(),
            cal_levels: cal_levels.into_iter().collect(),
            data_types: data_types.into_iter().collect(),
            obs_types: obs_types.into_iter().collect(),
        })
    }

    /// Fetch raw data train rows for cascade filtering.
    pub async fn fetch_data_train_rows(
        &self,
        token: Option<&str>,
    ) -> Result<Vec<crate::helpers::data_train_manager::DataTrainRow>, ApiError> {
        let adql = "SELECT energy_emBand, collection, instrument_name, \
                    energy_bandpassName, calibrationLevel, dataProductType, type \
                    FROM caom2.enumfield \
                    ORDER BY energy_emBand, collection, instrument_name, \
                    energy_bandpassName, calibrationLevel, dataProductType, type";

        let results = self.execute_query(adql, 50000, token).await?;

        let rows = results
            .rows
            .iter()
            .map(|r| crate::helpers::data_train_manager::DataTrainRow {
                band: r.get("energy_emBand").trim().to_string(),
                collection: r.get("collection").trim().to_string(),
                instrument: r.get("instrument_name").trim().to_string(),
                filter: r.get("energy_bandpassName").trim().to_string(),
                calibration_level: r.get("calibrationLevel").trim().to_string(),
                data_product_type: r.get("dataProductType").trim().to_string(),
                observation_type: r.get("type").trim().to_string(),
            })
            .collect();

        Ok(rows)
    }
}

/// Holds the distinct values for each data train column.
#[derive(Debug, Clone, Default)]
pub struct DataTrain {
    pub bands: Vec<String>,
    pub collections: Vec<String>,
    pub instruments: Vec<String>,
    pub filters: Vec<String>,
    pub cal_levels: Vec<String>,
    pub data_types: Vec<String>,
    pub obs_types: Vec<String>,
}
