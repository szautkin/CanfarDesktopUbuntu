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
            let body = resp
                .text()
                .await
                .map_err(|e| format!("Read error: {}", e))?;

            // Try as single object first, then as array (API may wrap in array)
            if let Ok(stats) = serde_json::from_str::<SkahaStatsResponse>(&body) {
                return Ok(stats);
            }
            if let Ok(arr) = serde_json::from_str::<Vec<SkahaStatsResponse>>(&body) {
                if let Some(stats) = arr.into_iter().next() {
                    return Ok(stats);
                }
            }
            Err(format!(
                "Parse error: unexpected response: {}",
                &body[..body.len().min(200)]
            ))
        } else {
            Err(format!("Failed to fetch stats ({})", resp.status()))
        }
    }
}
