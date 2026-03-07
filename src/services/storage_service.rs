use crate::config::ApiEndpoints;
use crate::models::StorageQuota;
use reqwest::Client;
use std::sync::Arc;

pub struct StorageService {
    client: Client,
    endpoints: Arc<ApiEndpoints>,
}

impl StorageService {
    pub fn new(client: Client, endpoints: Arc<ApiEndpoints>) -> Self {
        StorageService { client, endpoints }
    }

    pub async fn get_quota(&self, token: &str, username: &str) -> Result<StorageQuota, String> {
        let url = self.endpoints.storage_url(username);
        let resp = self
            .client
            .get(&url)
            .bearer_auth(token)
            .header("Accept", "text/xml")
            .send()
            .await
            .map_err(|e| format!("Network error: {}", e))?;

        if resp.status().is_success() {
            let xml_text = resp.text().await.map_err(|e| e.to_string())?;
            parse_vospace_xml(&xml_text)
        } else {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            Err(format!("{} → {} {}", url, status, body))
        }
    }
}

fn parse_vospace_xml(xml: &str) -> Result<StorageQuota, String> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| format!("XML parse error: {}", e))?;

    let mut quota_bytes: u64 = 0;
    let mut used_bytes: u64 = 0;
    let mut last_update: Option<String> = None;

    for node in doc.descendants() {
        if node.tag_name().name() == "property" {
            if let Some(uri) = node.attribute("uri") {
                let text = node.text().unwrap_or("0");
                if uri.contains("quota") {
                    quota_bytes = text.parse().unwrap_or(0);
                } else if uri.contains("length") {
                    used_bytes = text.parse().unwrap_or(0);
                } else if uri.contains("date") {
                    last_update = Some(text.to_string());
                }
            }
        }
    }

    Ok(StorageQuota {
        quota_bytes,
        used_bytes,
        last_update,
    })
}
