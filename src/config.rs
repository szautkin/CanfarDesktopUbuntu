use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub api_base_url: String,
    pub skaha_api_path: String,
    pub login_api_path: String,
    pub ac_api_path: String,
    pub storage_api_path: String,
    pub theme: String,
    pub default_session_type: String,
    pub default_cores: u32,
    pub default_ram: u32,
}

impl Default for AppConfig {
    fn default() -> Self {
        AppConfig {
            api_base_url: "https://ws-uv.canfar.net".to_string(),
            skaha_api_path: "/skaha/v1".to_string(),
            login_api_path: "/cred/auth/priv".to_string(),
            ac_api_path: "/ac".to_string(),
            storage_api_path: "/arc/nodes/home".to_string(),
            theme: "System".to_string(),
            default_session_type: "notebook".to_string(),
            default_cores: 2,
            default_ram: 8,
        }
    }
}

pub struct ApiEndpoints {
    config: AppConfig,
}

impl ApiEndpoints {
    pub fn new(config: AppConfig) -> Self {
        ApiEndpoints { config }
    }

    pub fn login_url(&self) -> String {
        "https://ws-cadc.canfar.net/ac/login".to_string()
    }

    pub fn whoami_url(&self) -> String {
        "https://ws-cadc.canfar.net/ac/whoami".to_string()
    }

    pub fn sessions_url(&self) -> String {
        format!(
            "{}{}/session",
            self.config.api_base_url, self.config.skaha_api_path
        )
    }

    pub fn session_url(&self, session_id: &str) -> String {
        format!(
            "{}{}/session/{}",
            self.config.api_base_url, self.config.skaha_api_path, session_id
        )
    }

    pub fn session_renew_url(&self, session_id: &str) -> String {
        format!(
            "{}{}/session/{}?action=renew",
            self.config.api_base_url, self.config.skaha_api_path, session_id
        )
    }

    pub fn session_events_url(&self, session_id: &str) -> String {
        format!(
            "{}{}/session/{}?view=events",
            self.config.api_base_url, self.config.skaha_api_path, session_id
        )
    }

    pub fn session_logs_url(&self, session_id: &str) -> String {
        format!(
            "{}{}/session/{}?view=logs",
            self.config.api_base_url, self.config.skaha_api_path, session_id
        )
    }

    pub fn repository_url(&self) -> String {
        format!(
            "{}{}/repository",
            self.config.api_base_url, self.config.skaha_api_path
        )
    }

    pub fn images_url(&self) -> String {
        format!(
            "{}{}/image",
            self.config.api_base_url, self.config.skaha_api_path
        )
    }

    pub fn context_url(&self) -> String {
        format!(
            "{}{}/context",
            self.config.api_base_url, self.config.skaha_api_path
        )
    }

    pub fn stats_url(&self) -> String {
        format!(
            "{}{}/session?view=stats",
            self.config.api_base_url, self.config.skaha_api_path
        )
    }

    pub fn storage_url(&self, username: &str) -> String {
        format!(
            "{}{}/{}?limit=0",
            self.config.api_base_url, self.config.storage_api_path, username
        )
    }

    pub fn config(&self) -> &AppConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn endpoints() -> ApiEndpoints {
        ApiEndpoints::new(AppConfig::default())
    }

    #[test]
    fn sessions_url() {
        assert_eq!(
            endpoints().sessions_url(),
            "https://ws-uv.canfar.net/skaha/v1/session"
        );
    }

    #[test]
    fn session_url() {
        assert_eq!(
            endpoints().session_url("abc123"),
            "https://ws-uv.canfar.net/skaha/v1/session/abc123"
        );
    }

    #[test]
    fn session_renew_url() {
        assert_eq!(
            endpoints().session_renew_url("abc123"),
            "https://ws-uv.canfar.net/skaha/v1/session/abc123?action=renew"
        );
    }

    #[test]
    fn session_events_url() {
        assert_eq!(
            endpoints().session_events_url("abc123"),
            "https://ws-uv.canfar.net/skaha/v1/session/abc123?view=events"
        );
    }

    #[test]
    fn session_logs_url() {
        assert_eq!(
            endpoints().session_logs_url("abc123"),
            "https://ws-uv.canfar.net/skaha/v1/session/abc123?view=logs"
        );
    }

    #[test]
    fn images_url() {
        assert_eq!(
            endpoints().images_url(),
            "https://ws-uv.canfar.net/skaha/v1/image"
        );
    }

    #[test]
    fn repository_url() {
        assert_eq!(
            endpoints().repository_url(),
            "https://ws-uv.canfar.net/skaha/v1/repository"
        );
    }

    #[test]
    fn storage_url() {
        assert_eq!(
            endpoints().storage_url("testuser"),
            "https://ws-uv.canfar.net/arc/nodes/home/testuser?limit=0"
        );
    }

    #[test]
    fn login_url_is_cadc() {
        assert_eq!(
            endpoints().login_url(),
            "https://ws-cadc.canfar.net/ac/login"
        );
    }

    #[test]
    fn default_config_values() {
        let cfg = AppConfig::default();
        assert_eq!(cfg.default_cores, 2);
        assert_eq!(cfg.default_ram, 8);
        assert_eq!(cfg.default_session_type, "notebook");
    }
}
