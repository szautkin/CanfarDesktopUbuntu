#[allow(dead_code)]
pub struct AuthResult {
    pub success: bool,
    pub token: Option<String>,
    pub username: Option<String>,
    pub error: Option<String>,
}
