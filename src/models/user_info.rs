#[derive(Debug, Clone, Default)]
pub struct UserInfo {
    pub username: Option<String>,
    pub email: Option<String>,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub institute: Option<String>,
    pub internal_id: Option<String>,
}

impl UserInfo {
    pub fn display_name(&self) -> String {
        match (&self.first_name, &self.last_name) {
            (Some(f), Some(l)) if !f.is_empty() && !l.is_empty() => format!("{} {}", f, l),
            _ => self
                .username
                .clone()
                .unwrap_or_else(|| "Unknown".to_string()),
        }
    }
}
