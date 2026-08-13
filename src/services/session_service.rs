use crate::config::ApiEndpoints;
use crate::models::{Session, SessionLaunchParams, SkahaSessionResponse};
use crate::services::api_error::{check_response, ApiError};
use reqwest::Client;
use std::sync::Arc;

/// A headless launch that failed after some replicas were already running.
///
/// Carries them: those jobs exist and are spending quota, and an error that
/// mentions only the failure leaves the user with nothing to clean up by.
#[derive(Debug)]
pub struct HeadlessLaunchError {
    pub message: String,
    pub launched: Vec<String>,
}

/// Read one session out of a `GET /session/{id}` body.
///
/// Skaha answers with the bare record, but has been seen to wrap it in a
/// single-element array; accepting either beats failing to parse a session that
/// is plainly there. A body that is neither is "no such session", not an error —
/// the caller's question was whether the job exists.
fn parse_session_body(body: &str) -> Option<Session> {
    if let Ok(one) = serde_json::from_str::<SkahaSessionResponse>(body) {
        return Some(Session::from(one));
    }
    serde_json::from_str::<Vec<SkahaSessionResponse>>(body)
        .ok()
        .and_then(|many| many.into_iter().next())
        .map(Session::from)
}

impl std::fmt::Display for HeadlessLaunchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.launched.is_empty() {
            write!(f, "{}", self.message)
        } else {
            write!(
                f,
                "{} — {} replica(s) already launched: {}",
                self.message,
                self.launched.len(),
                self.launched.join(", ")
            )
        }
    }
}

pub struct SessionService {
    client: Client,
    endpoints: Arc<ApiEndpoints>,
}

impl SessionService {
    pub fn new(client: Client, endpoints: Arc<ApiEndpoints>) -> Self {
        SessionService { client, endpoints }
    }

    /// One session by id, or `None` when Skaha does not know it.
    ///
    /// A GET on the session's own URL, as the reference does — not a filter over
    /// `get_sessions`. The list is the user's LIVE sessions and drops a headless
    /// job once it is reaped, so filtering it answers "no such session" for a
    /// job that finished ten minutes ago, which is the single most likely thing
    /// to ask about a batch job.
    pub async fn get_session(&self, token: &str, id: &str) -> Result<Option<Session>, ApiError> {
        let url = self.endpoints.session_url(id);
        let resp = self.client.get(&url).bearer_auth(token).send().await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let resp = check_response(resp).await?;
        let body = resp
            .text()
            .await
            .map_err(|e| ApiError::Parse(e.to_string()))?;
        // Through `parse_session_body`, which is what the tests exercise. The
        // same either-shape logic was inlined here as well, so the tests were
        // covering a copy of the code rather than the code.
        Ok(parse_session_body(&body))
    }

    pub async fn get_sessions(&self, token: &str) -> Result<Vec<Session>, ApiError> {
        let url = self.endpoints.sessions_url();
        let resp = self.client.get(&url).bearer_auth(token).send().await?;

        let resp = check_response(resp).await?;
        let raw: Vec<SkahaSessionResponse> = resp
            .json()
            .await
            .map_err(|e| ApiError::Parse(e.to_string()))?;
        Ok(raw.into_iter().map(Session::from).collect())
    }

    pub async fn launch_session(
        &self,
        token: &str,
        params: &SessionLaunchParams,
    ) -> Result<String, String> {
        self.post_launch(token, params, params.to_form_pairs())
            .await
    }

    /// Launch every replica of a headless job, returning the session ids in
    /// launch order.
    ///
    /// The reference posts once PER REPLICA (`HeadlessRequestBuilder`), which is
    /// also the canonical Python client's wire shape; we sent a single request
    /// carrying `replicas=N`, so a user who asked for eight jobs got one, with
    /// no `REPLICA_ID` to tell it which slice of the work was its own.
    ///
    /// A failure part-way is reported WITH the ids already launched: those jobs
    /// are running and spending quota, and an error that hides them leaves the
    /// user unable to find what to clean up. Mirrors the reference's
    /// `HeadlessLaunchException`.
    pub async fn launch_headless(
        &self,
        token: &str,
        params: &SessionLaunchParams,
    ) -> Result<Vec<String>, HeadlessLaunchError> {
        let count = params.replica_count();
        let mut ids = Vec::with_capacity(count as usize);
        for index in 0..count {
            match self
                .post_launch(token, params, params.headless_form_pairs(index, count))
                .await
            {
                Ok(id) => ids.push(id),
                Err(message) => {
                    return Err(HeadlessLaunchError {
                        message,
                        launched: ids,
                    })
                }
            }
        }
        Ok(ids)
    }

    /// POST one launch and read back its session id.
    async fn post_launch(
        &self,
        token: &str,
        params: &SessionLaunchParams,
        form_pairs: Vec<(&'static str, String)>,
    ) -> Result<String, String> {
        let url = self.endpoints.sessions_url();
        let mut req = self.client.post(&url).bearer_auth(token).form(&form_pairs);

        if let Some(ref user) = params.registry_username {
            use base64::Engine;
            let secret = params.registry_secret.as_deref().unwrap_or("");
            let auth_value =
                base64::engine::general_purpose::STANDARD.encode(format!("{}:{}", user, secret));
            req = req.header("x-skaha-registry-auth", &auth_value);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| format!("Network error: {}", e))?;

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

#[cfg(test)]
mod tests {
    use super::parse_session_body;

    const RECORD: &str = r#"{"id":"abc123","userid":"u","image":"images.canfar.net/skaha/notebook:1","type":"headless","status":"Succeeded","name":"stack-1","startTime":"2026-08-12T10:00:00Z"}"#;

    #[test]
    fn a_bare_record_parses() {
        let s = parse_session_body(RECORD).expect("the record is a session");
        assert_eq!(s.id, "abc123");
        assert_eq!(s.status, "Succeeded");
    }

    #[test]
    fn a_single_element_array_parses_too() {
        // Skaha has been seen to wrap it; failing to read a session that is
        // plainly in the body would report the job as missing.
        let body = format!("[{RECORD}]");
        assert_eq!(
            parse_session_body(&body).map(|s| s.id),
            Some("abc123".into())
        );
    }

    #[test]
    fn an_empty_array_is_no_such_session() {
        assert!(parse_session_body("[]").is_none());
    }

    #[test]
    fn a_body_that_is_not_a_session_is_no_such_session() {
        // The caller asked whether the job exists; "no" is an answer, and it
        // must not arrive as a parse error the agent has to interpret.
        assert!(parse_session_body("not json").is_none());
        assert!(parse_session_body("").is_none());
    }
}
