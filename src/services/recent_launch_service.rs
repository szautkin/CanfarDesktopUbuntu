use crate::models::RecentLaunch;
use directories::ProjectDirs;
use std::path::PathBuf;

const MAX_RECENT: usize = 10;

pub struct RecentLaunchService {
    file_path: PathBuf,
}

impl RecentLaunchService {
    pub fn new() -> Self {
        let file_path = ProjectDirs::from("net", "canfar", "Verbinal")
            .map(|dirs| dirs.data_dir().join("recent_launches.json"))
            .unwrap_or_else(|| PathBuf::from("recent_launches.json"));
        RecentLaunchService { file_path }
    }

    pub fn load(&self) -> Vec<RecentLaunch> {
        if self.file_path.exists() {
            match std::fs::read_to_string(&self.file_path) {
                Ok(contents) => serde_json::from_str(&contents).unwrap_or_default(),
                Err(_) => Vec::new(),
            }
        } else {
            Vec::new()
        }
    }

    pub fn save(&self, launch: RecentLaunch) -> Result<(), String> {
        let mut launches = self.load();

        launches.retain(|l| !(l.image == launch.image && l.session_type == launch.session_type));
        launches.insert(0, launch);
        launches.truncate(MAX_RECENT);

        if let Some(parent) = self.file_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let json = serde_json::to_string_pretty(&launches).map_err(|e| e.to_string())?;
        std::fs::write(&self.file_path, json).map_err(|e| e.to_string())
    }

    pub fn remove(&self, index: usize) -> Result<(), String> {
        let mut launches = self.load();
        if index < launches.len() {
            launches.remove(index);
            if let Some(parent) = self.file_path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            let json = serde_json::to_string_pretty(&launches).map_err(|e| e.to_string())?;
            std::fs::write(&self.file_path, json).map_err(|e| e.to_string())
        } else {
            Ok(())
        }
    }

    pub fn clear(&self) -> Result<(), String> {
        if self.file_path.exists() {
            std::fs::remove_file(&self.file_path).map_err(|e| e.to_string())
        } else {
            Ok(())
        }
    }
}
