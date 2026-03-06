use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecentLaunch {
    pub name: String,
    pub session_type: String,
    pub image: String,
    pub cores: u32,
    pub ram: u32,
    pub gpus: u32,
    pub timestamp: String,
}

impl RecentLaunch {
    pub fn display_image(&self) -> String {
        match self.image.rsplit_once('/') {
            Some((_, name)) => name.to_string(),
            None => self.image.clone(),
        }
    }

    pub fn type_display(&self) -> &str {
        match self.session_type.to_lowercase().as_str() {
            "notebook" => "Notebook",
            "desktop" => "Desktop",
            "carta" => "CARTA",
            "contributed" => "Contributed",
            "firefly" => "Firefly",
            "headless" => "Headless",
            _ => &self.session_type,
        }
    }
}
