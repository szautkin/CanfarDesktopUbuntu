use crate::config::ApiEndpoints;
use crate::models::{AuthResult, UserInfo};
use reqwest::Client;

pub struct AuthService {
    client: Client,
    endpoints: std::sync::Arc<ApiEndpoints>,
}

impl AuthService {
    pub fn new(client: Client, endpoints: std::sync::Arc<ApiEndpoints>) -> Self {
        AuthService { client, endpoints }
    }

    pub async fn login(&self, username: &str, password: &str) -> AuthResult {
        let url = self.endpoints.login_url();
        let params = [("username", username), ("password", password)];

        match self.client.post(&url).form(&params).send().await {
            Ok(resp) => {
                if resp.status().is_success() {
                    match resp.text().await {
                        Ok(token) => {
                            let token = token.trim().to_string();
                            if token.is_empty() {
                                AuthResult {
                                    success: false,
                                    token: None,
                                    username: None,
                                    error: Some("Empty token received".to_string()),
                                }
                            } else {
                                AuthResult {
                                    success: true,
                                    token: Some(token),
                                    username: Some(username.to_string()),
                                    error: None,
                                }
                            }
                        }
                        Err(e) => AuthResult {
                            success: false,
                            token: None,
                            username: None,
                            error: Some(format!("Failed to read response: {}", e)),
                        },
                    }
                } else if resp.status().as_u16() == 401 {
                    AuthResult {
                        success: false,
                        token: None,
                        username: None,
                        error: Some("Invalid username or password".to_string()),
                    }
                } else {
                    AuthResult {
                        success: false,
                        token: None,
                        username: None,
                        error: Some(format!("Server error: {}", resp.status())),
                    }
                }
            }
            Err(e) => AuthResult {
                success: false,
                token: None,
                username: None,
                error: Some(format!("Network error: {}", e)),
            },
        }
    }

    pub async fn validate_token(&self, token: &str) -> Result<String, String> {
        let url = self.endpoints.whoami_url();
        let resp = self
            .client
            .get(&url)
            .bearer_auth(token)
            .send()
            .await
            .map_err(|e| format!("Network error: {}", e))?;

        if resp.status().is_success() {
            let body = resp.text().await.map_err(|e| e.to_string())?;
            let body = body.trim();

            // Server may return XML or plain text depending on Accept header
            if body.starts_with("<?xml") || body.starts_with("<user") {
                let info = parse_whoami_xml(body)?;
                info.username
                    .ok_or_else(|| "No username in response".to_string())
            } else {
                Ok(body.to_string())
            }
        } else {
            Err(format!("Token invalid ({})", resp.status()))
        }
    }

    pub async fn get_user_info(&self, token: &str) -> Result<UserInfo, String> {
        let url = self.endpoints.whoami_url();
        let resp = self
            .client
            .get(&url)
            .bearer_auth(token)
            .send()
            .await
            .map_err(|e| format!("Network error: {}", e))?;

        if resp.status().is_success() {
            let xml = resp.text().await.map_err(|e| e.to_string())?;
            parse_whoami_xml(&xml)
        } else {
            Err(format!("Failed to get user info ({})", resp.status()))
        }
    }
}

pub(crate) fn parse_whoami_xml(xml: &str) -> Result<UserInfo, String> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| format!("XML parse error: {}", e))?;

    let text_of = |tag: &str| -> Option<String> {
        doc.descendants()
            .find(|n| n.tag_name().name() == tag)
            .and_then(|n| n.text())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };

    Ok(UserInfo {
        first_name: text_of("firstName"),
        last_name: text_of("lastName"),
        email: text_of("email"),
        institute: text_of("institute"),
        username: text_of("username"),
        internal_id: text_of("internalID"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_whoami_xml_full() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
        <user>
            <username>jdoe</username>
            <firstName>John</firstName>
            <lastName>Doe</lastName>
            <email>jdoe@example.com</email>
            <institute>CADC</institute>
            <internalID>12345</internalID>
        </user>"#;

        let info = parse_whoami_xml(xml).unwrap();
        assert_eq!(info.username.as_deref(), Some("jdoe"));
        assert_eq!(info.first_name.as_deref(), Some("John"));
        assert_eq!(info.last_name.as_deref(), Some("Doe"));
        assert_eq!(info.email.as_deref(), Some("jdoe@example.com"));
        assert_eq!(info.institute.as_deref(), Some("CADC"));
        assert_eq!(info.internal_id.as_deref(), Some("12345"));
    }

    #[test]
    fn parse_whoami_xml_minimal() {
        let xml = r#"<user><username>alice</username></user>"#;
        let info = parse_whoami_xml(xml).unwrap();
        assert_eq!(info.username.as_deref(), Some("alice"));
        assert!(info.first_name.is_none());
        assert!(info.email.is_none());
    }

    #[test]
    fn parse_whoami_xml_empty_tags() {
        let xml = r#"<user><username>  </username><firstName></firstName></user>"#;
        let info = parse_whoami_xml(xml).unwrap();
        // Whitespace-only and empty are filtered out
        assert!(info.username.is_none());
        assert!(info.first_name.is_none());
    }

    #[test]
    fn parse_whoami_xml_invalid() {
        let result = parse_whoami_xml("not xml at all <>");
        assert!(result.is_err());
    }
}
