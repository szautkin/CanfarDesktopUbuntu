//! Verify registry credentials via the Docker Registry V2 token-auth dance.
//!
//! Port of `Services/ImageDiscovery/RegistryCredentialTest.cs`. Pings `/v2/`
//! to discover the Bearer `realm`/`service`, then requests a token with HTTP
//! Basic auth. This lets the user confirm their Harbor CLI secret *before* a
//! probe job fails minutes later with `ImagePullBackOff`. Works against any
//! OCI-compliant registry (Harbor, Docker Hub, Quay, GHCR).
//!
//! The registry only ever sees Basic auth built from the user's registry
//! secret — never the CADC bearer token.

use std::time::Duration;

/// Outcome of a registry credential probe. Only [`CredTestResult::NetworkError`]
/// carries detail (the transport/HTTP message); the other variants are
/// classifications the UI maps to fixed guidance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredTestResult {
    /// Credentials valid, or the registry is public (no auth needed).
    Success,
    /// The registry rejected the credentials (401/403 on the token request).
    Unauthorized,
    /// Host, username, or secret was not configured.
    MissingConfiguration,
    /// The `WWW-Authenticate` challenge was present but not a parseable Bearer
    /// challenge (or its realm was missing/malformed).
    InvalidChallenge,
    /// A transport error, timeout, or unexpected HTTP status; carries a message.
    NetworkError(String),
}

/// A parsed Docker Registry V2 `Bearer realm="…", service="…"` challenge.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BearerChallenge {
    pub realm: Option<String>,
    pub service: Option<String>,
}

const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Verify `username`/`secret` against `host` using the Docker V2 token dance.
///
/// Steps: `GET https://{host}/v2/` → if 2xx the registry is public; if 401,
/// parse the `WWW-Authenticate` Bearer challenge and `GET <realm>?service=…`
/// with Basic auth, classifying the token response.
pub async fn test_registry_credentials(host: &str, username: &str, secret: &str) -> CredTestResult {
    if host.is_empty() {
        return CredTestResult::MissingConfiguration;
    }
    if username.is_empty() {
        return CredTestResult::MissingConfiguration;
    }
    if secret.is_empty() {
        return CredTestResult::MissingConfiguration;
    }

    let client = match reqwest::Client::builder().timeout(PROBE_TIMEOUT).build() {
        Ok(c) => c,
        Err(e) => return CredTestResult::NetworkError(e.to_string()),
    };

    // Step 1: ping /v2/ to discover the auth realm.
    let ping_url = format!("https://{host}/v2/");
    let ping = match client.get(&ping_url).send().await {
        Ok(r) => r,
        Err(e) => return CredTestResult::NetworkError(e.to_string()),
    };

    let status = ping.status().as_u16();
    if (200..300).contains(&status) {
        return CredTestResult::Success; // publicly accessible — no credentials needed
    }
    if status != 401 {
        return CredTestResult::NetworkError(format!("Unexpected HTTP {status} from {host}/v2/."));
    }

    let challenge = match extract_challenge(&ping) {
        Some(c) => c,
        None => {
            return CredTestResult::NetworkError(format!(
                "Registry returned 401 without a WWW-Authenticate challenge from {host}/v2/."
            ))
        }
    };

    let parsed = match parse_bearer_challenge(&challenge) {
        Some(p) => p,
        None => return CredTestResult::InvalidChallenge,
    };

    let realm = match parsed.realm.as_deref() {
        Some(r) if !r.is_empty() => r,
        _ => return CredTestResult::InvalidChallenge,
    };
    let mut token_url = match reqwest::Url::parse(realm) {
        Ok(u) => u,
        Err(_) => return CredTestResult::InvalidChallenge,
    };
    if let Some(service) = parsed.service.as_deref().filter(|s| !s.is_empty()) {
        token_url.query_pairs_mut().append_pair("service", service);
    }

    // Step 2: GET <realm>?service=<service> with Basic auth.
    let token = match client
        .get(token_url)
        .basic_auth(username, Some(secret))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => return CredTestResult::NetworkError(e.to_string()),
    };

    let code = token.status().as_u16();
    if code == 401 || code == 403 {
        CredTestResult::Unauthorized
    } else if (200..300).contains(&code) {
        CredTestResult::Success
    } else {
        CredTestResult::NetworkError(format!("Token endpoint returned HTTP {code}."))
    }
}

/// Join every `WWW-Authenticate` header value into one challenge string.
fn extract_challenge(response: &reqwest::Response) -> Option<String> {
    let joined = response
        .headers()
        .get_all(reqwest::header::WWW_AUTHENTICATE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .collect::<Vec<_>>()
        .join(", ");
    if joined.trim().is_empty() {
        None
    } else {
        Some(joined)
    }
}

/// Parse a Docker Registry V2 `Bearer realm="x", service="y"` challenge into
/// its realm + service. Tolerates single/double quotes, extra params, and stray
/// whitespace; returns `None` when the scheme isn't Bearer.
pub fn parse_bearer_challenge(challenge: &str) -> Option<BearerChallenge> {
    let trimmed = challenge.trim();
    let lower = trimmed.to_ascii_lowercase();
    if !(lower == "bearer" || lower.starts_with("bearer ")) {
        return None;
    }

    // "Bearer" is 6 ASCII bytes, so byte index 6 is a char boundary.
    let after_scheme = trimmed[6..].trim();
    let mut realm = None;
    let mut service = None;
    for part in after_scheme.split(',') {
        let kv = part.trim();
        let eq = match kv.find('=') {
            Some(i) => i,
            None => continue,
        };
        let key = kv[..eq].trim().to_ascii_lowercase();
        let mut value = kv[eq + 1..].trim();
        let bytes = value.as_bytes();
        if bytes.len() >= 2 {
            let first = bytes[0];
            let last = bytes[bytes.len() - 1];
            if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
                value = &value[1..value.len() - 1];
            }
        }
        match key.as_str() {
            "realm" => realm = Some(value.to_string()),
            "service" => service = Some(value.to_string()),
            _ => {}
        }
    }
    Some(BearerChallenge { realm, service })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_standard_bearer_challenge() {
        let c = parse_bearer_challenge(
            r#"Bearer realm="https://images.canfar.net/service/token",service="harbor-registry""#,
        )
        .expect("bearer");
        assert_eq!(c.realm.as_deref(), Some("https://images.canfar.net/service/token"));
        assert_eq!(c.service.as_deref(), Some("harbor-registry"));
    }

    #[test]
    fn parses_single_quotes_extra_params_and_whitespace() {
        let c = parse_bearer_challenge(
            "  bearer  realm='https://auth.docker.io/token' , service='registry.docker.io', scope=\"repository:x:pull\"  ",
        )
        .expect("bearer");
        assert_eq!(c.realm.as_deref(), Some("https://auth.docker.io/token"));
        assert_eq!(c.service.as_deref(), Some("registry.docker.io"));
    }

    #[test]
    fn non_bearer_scheme_returns_none() {
        assert!(parse_bearer_challenge(r#"Basic realm="x""#).is_none());
        assert!(parse_bearer_challenge("").is_none());
    }

    #[test]
    fn bearer_without_params_yields_empty_challenge() {
        let c = parse_bearer_challenge("Bearer").expect("bearer");
        assert!(c.realm.is_none());
        assert!(c.service.is_none());
    }

    #[tokio::test]
    async fn empty_host_is_missing_configuration() {
        assert_eq!(
            test_registry_credentials("", "user", "secret").await,
            CredTestResult::MissingConfiguration
        );
    }

    #[tokio::test]
    async fn empty_username_is_missing_configuration() {
        assert_eq!(
            test_registry_credentials("images.canfar.net", "", "secret").await,
            CredTestResult::MissingConfiguration
        );
    }

    #[tokio::test]
    async fn empty_secret_is_missing_configuration() {
        assert_eq!(
            test_registry_credentials("images.canfar.net", "user", "").await,
            CredTestResult::MissingConfiguration
        );
    }
}
