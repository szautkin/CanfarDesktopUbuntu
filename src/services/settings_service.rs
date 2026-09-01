use crate::config::AppConfig;
use crate::models::annotation::{Author, MarkStyle};
use directories::ProjectDirs;
use std::path::PathBuf;

pub struct SettingsService {
    config_path: PathBuf,
}

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

/// What a new mark by `author` looks like, per the user's saved settings.
///
/// Read when a mark is CREATED and copied into it. Nothing reads this at draw
/// time, so changing the setting leaves every mark already drawn alone.
pub fn default_mark_style(author: Author) -> MarkStyle {
    MarkStyle::from_settings(author, &SettingsService::new().load())
}

/// Remember `style` as the look of the next mark.
///
/// Best effort: a settings file that cannot be written is not a reason to
/// refuse a style change the person can see happening on screen.
pub fn remember_mark_style(style: MarkStyle) {
    let service = SettingsService::new();
    let mut cfg = service.load();
    style.store_in(&mut cfg);
    let _ = service.save(&cfg);
}

impl Default for SettingsService {
    fn default() -> Self {
        Self::new()
    }
}
