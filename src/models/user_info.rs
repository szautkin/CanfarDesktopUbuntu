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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_name_full() {
        let info = UserInfo {
            first_name: Some("John".to_string()),
            last_name: Some("Doe".to_string()),
            username: Some("jdoe".to_string()),
            ..Default::default()
        };
        assert_eq!(info.display_name(), "John Doe");
    }

    #[test]
    fn display_name_falls_back_to_username() {
        let info = UserInfo {
            username: Some("jdoe".to_string()),
            ..Default::default()
        };
        assert_eq!(info.display_name(), "jdoe");
    }

    #[test]
    fn display_name_empty_names_falls_back() {
        let info = UserInfo {
            first_name: Some("".to_string()),
            last_name: Some("".to_string()),
            username: Some("jdoe".to_string()),
            ..Default::default()
        };
        assert_eq!(info.display_name(), "jdoe");
    }

    #[test]
    fn display_name_no_info() {
        let info = UserInfo::default();
        assert_eq!(info.display_name(), "Unknown");
    }
}
