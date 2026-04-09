use crate::models::SessionTemplate;
use directories::ProjectDirs;
use std::path::PathBuf;

pub struct TemplateService {
    data_path: PathBuf,
}

impl TemplateService {
    pub fn new() -> Self {
        let data_path = ProjectDirs::from("net", "canfar", "Verbinal")
            .map(|dirs| dirs.data_dir().join("templates.json"))
            .unwrap_or_else(|| PathBuf::from("templates.json"));
        TemplateService { data_path }
    }

    pub fn load(&self) -> Vec<SessionTemplate> {
        if self.data_path.exists() {
            match std::fs::read_to_string(&self.data_path) {
                Ok(contents) => serde_json::from_str(&contents).unwrap_or_default(),
                Err(_) => Vec::new(),
            }
        } else {
            Vec::new()
        }
    }

    pub fn save(&self, templates: &[SessionTemplate]) -> Result<(), String> {
        if let Some(parent) = self.data_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let json = serde_json::to_string_pretty(templates).map_err(|e| e.to_string())?;
        std::fs::write(&self.data_path, json).map_err(|e| e.to_string())
    }

    pub fn add(&self, template: SessionTemplate) -> Result<(), String> {
        let mut templates = self.load();
        templates.push(template);
        self.save(&templates)
    }

    pub fn remove(&self, name: &str) -> Result<(), String> {
        let mut templates = self.load();
        templates.retain(|t| t.name != name);
        self.save(&templates)
    }
}
