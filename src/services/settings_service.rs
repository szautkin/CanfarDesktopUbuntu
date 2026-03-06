use crate::config::AppConfig;
use directories::ProjectDirs;
use std::path::PathBuf;

pub struct SettingsService {
    config_path: PathBuf,
}

#[allow(dead_code)]
impl SettingsService {
    pub fn new() -> Self {
        let config_path = ProjectDirs::from("net", "canfar", "Verbinal")
            .map(|dirs| dirs.config_dir().join("settings.json"))
            .unwrap_or_else(|| PathBuf::from("settings.json"));
        SettingsService { config_path }
    }

    pub fn load(&self) -> AppConfig {
        if self.config_path.exists() {
            match std::fs::read_to_string(&self.config_path) {
                Ok(contents) => serde_json::from_str(&contents).unwrap_or_default(),
                Err(_) => AppConfig::default(),
            }
        } else {
            AppConfig::default()
        }
    }

    pub fn save(&self, config: &AppConfig) -> Result<(), String> {
        if let Some(parent) = self.config_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let json = serde_json::to_string_pretty(config).map_err(|e| e.to_string())?;
        std::fs::write(&self.config_path, json).map_err(|e| e.to_string())
    }
}
