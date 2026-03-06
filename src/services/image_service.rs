use crate::config::ApiEndpoints;
use crate::models::{RawImage, SessionContext};
use reqwest::Client;
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct ImageService {
    client: Client,
    endpoints: Arc<ApiEndpoints>,
    cache: Mutex<Option<(std::time::Instant, Vec<RawImage>)>>,
}

impl ImageService {
    pub fn new(client: Client, endpoints: Arc<ApiEndpoints>) -> Self {
        ImageService {
            client,
            endpoints,
            cache: Mutex::new(None),
        }
    }

    pub async fn get_images(&self, token: &str) -> Result<Vec<RawImage>, String> {
        {
            let cache = self.cache.lock().await;
            if let Some((time, ref images)) = *cache {
                if time.elapsed() < std::time::Duration::from_secs(300) {
                    return Ok(images.clone());
                }
            }
        }

        let url = self.endpoints.images_url();
        let resp = self
            .client
            .get(&url)
            .bearer_auth(token)
            .send()
            .await
            .map_err(|e| format!("Network error: {}", e))?;

        if resp.status().is_success() {
            let images: Vec<RawImage> =
                resp.json().await.map_err(|e| format!("Parse error: {}", e))?;
            let mut cache = self.cache.lock().await;
            *cache = Some((std::time::Instant::now(), images.clone()));
            Ok(images)
        } else {
            Err(format!("Failed to fetch images ({})", resp.status()))
        }
    }

    pub async fn get_context(&self, token: &str) -> Result<SessionContext, String> {
        let url = self.endpoints.context_url();
        let resp = self
            .client
            .get(&url)
            .bearer_auth(token)
            .send()
            .await
            .map_err(|e| format!("Network error: {}", e))?;

        if resp.status().is_success() {
            resp.json::<SessionContext>()
                .await
                .map_err(|e| format!("Parse error: {}", e))
        } else {
            Err(format!("Failed to fetch context ({})", resp.status()))
        }
    }

}
