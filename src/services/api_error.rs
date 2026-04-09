use std::fmt;

#[derive(Debug, Clone)]
pub enum ApiError {
    Unauthorized,
    Network(String),
    Server { status: u16, body: String },
    Parse(String),
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApiError::Unauthorized => write!(f, "Session expired. Please log in again."),
            ApiError::Network(msg) => write!(f, "Network error: {}", msg),
            ApiError::Server { status, body } => write!(f, "Server error ({}): {}", status, body),
            ApiError::Parse(msg) => write!(f, "Parse error: {}", msg),
        }
    }
}

impl ApiError {
    pub fn is_unauthorized(&self) -> bool {
        matches!(self, ApiError::Unauthorized)
    }
}

/// Check an HTTP response for auth/server errors before consuming the body.
/// Returns the response if OK, or an ApiError.
pub async fn check_response(resp: reqwest::Response) -> Result<reqwest::Response, ApiError> {
    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(ApiError::Unauthorized);
    }
    if !status.is_success() {
        let code = status.as_u16();
        let body = resp.text().await.unwrap_or_default();
        return Err(ApiError::Server { status: code, body });
    }
    Ok(resp)
}

impl From<reqwest::Error> for ApiError {
    fn from(e: reqwest::Error) -> Self {
        ApiError::Network(e.to_string())
    }
}
