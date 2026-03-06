use crate::config::ApiEndpoints;
use crate::models::{Session, SessionLaunchParams, SkahaSessionResponse};
use reqwest::Client;
use std::sync::Arc;

pub struct SessionService {
    client: Client,
    endpoints: Arc<ApiEndpoints>,
}

impl SessionService {
    pub fn new(client: Client, endpoints: Arc<ApiEndpoints>) -> Self {
        SessionService { client, endpoints }
    }

    pub async fn get_sessions(&self, token: &str) -> Result<Vec<Session>, String> {
        let url = self.endpoints.sessions_url();
        let resp = self
            .client
            .get(&url)
            .bearer_auth(token)
            .send()
            .await
            .map_err(|e| format!("Network error: {}", e))?;

        if resp.status().is_success() {
            let raw: Vec<SkahaSessionResponse> =
                resp.json().await.map_err(|e| format!("Parse error: {}", e))?;
            Ok(raw.into_iter().map(Session::from).collect())
        } else {
            Err(format!("Failed to fetch sessions ({})", resp.status()))
        }
    }

    pub async fn launch_session(
        &self,
        token: &str,
        params: &SessionLaunchParams,
    ) -> Result<String, String> {
        let url = self.endpoints.sessions_url();
        let form_pairs = params.to_form_pairs();

        let mut req = self
            .client
            .post(&url)
            .bearer_auth(token)
            .form(&form_pairs);

        if let (Some(ref user), Some(ref secret)) =
            (&params.registry_username, &params.registry_secret)
        {
            req = req
                .header("x-skaha-registry-username", user.as_str())
                .header("x-skaha-registry-secret", secret.as_str());
        }

        let resp = req.send().await.map_err(|e| format!("Network error: {}", e))?;

        if resp.status().is_success() {
            let body = resp.text().await.map_err(|e| e.to_string())?;
            let body = body.trim().to_string();
            if body.starts_with('[') {
                let ids: Vec<String> =
                    serde_json::from_str(&body).unwrap_or_else(|_| vec![body.clone()]);
                Ok(ids.into_iter().next().unwrap_or(body))
            } else {
                Ok(body)
            }
        } else {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            Err(format!("Launch failed ({}): {}", status, body))
        }
    }

    pub async fn delete_session(&self, token: &str, session_id: &str) -> Result<(), String> {
        let url = self.endpoints.session_url(session_id);
        let resp = self
            .client
            .delete(&url)
            .bearer_auth(token)
            .send()
            .await
            .map_err(|e| format!("Network error: {}", e))?;

        if resp.status().is_success() {
            Ok(())
        } else {
            Err(format!("Delete failed ({})", resp.status()))
        }
    }

    pub async fn renew_session(&self, token: &str, session_id: &str) -> Result<(), String> {
        let url = self.endpoints.session_renew_url(session_id);
        let resp = self
            .client
            .post(&url)
            .bearer_auth(token)
            .send()
            .await
            .map_err(|e| format!("Network error: {}", e))?;

        if resp.status().is_success() {
            Ok(())
        } else {
            Err(format!("Renew failed ({})", resp.status()))
        }
    }

    pub async fn get_events(&self, token: &str, session_id: &str) -> Result<String, String> {
        let url = self.endpoints.session_events_url(session_id);
        let resp = self
            .client
            .get(&url)
            .bearer_auth(token)
            .send()
            .await
            .map_err(|e| format!("Network error: {}", e))?;

        if resp.status().is_success() {
            resp.text().await.map_err(|e| e.to_string())
        } else {
            Err(format!("Failed to get events ({})", resp.status()))
        }
    }

    pub async fn get_logs(&self, token: &str, session_id: &str) -> Result<String, String> {
        let url = self.endpoints.session_logs_url(session_id);
        let resp = self
            .client
            .get(&url)
            .bearer_auth(token)
            .send()
            .await
            .map_err(|e| format!("Network error: {}", e))?;

        if resp.status().is_success() {
            resp.text().await.map_err(|e| e.to_string())
        } else {
            Err(format!("Failed to get logs ({})", resp.status()))
        }
    }
}
