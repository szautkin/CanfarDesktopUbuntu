use crate::config::ApiEndpoints;
use crate::models::SkahaStatsResponse;
use reqwest::Client;
use std::sync::Arc;

pub struct PlatformService {
    client: Client,
    endpoints: Arc<ApiEndpoints>,
}

impl PlatformService {
    pub fn new(client: Client, endpoints: Arc<ApiEndpoints>) -> Self {
        PlatformService { client, endpoints }
    }

    pub async fn get_stats(&self, token: &str) -> Result<SkahaStatsResponse, String> {
        let url = self.endpoints.stats_url();
        let resp = self
            .client
            .get(&url)
            .bearer_auth(token)
            .send()
            .await
            .map_err(|e| format!("Network error: {}", e))?;

        if resp.status().is_success() {
            resp.json::<SkahaStatsResponse>()
                .await
                .map_err(|e| format!("Parse error: {}", e))
        } else {
            Err(format!("Failed to fetch stats ({})", resp.status()))
        }
    }
}
