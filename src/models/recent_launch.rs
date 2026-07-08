use crate::models::SessionLaunchParams;
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
    /// Project the image belongs to (Standard tab); `None`/empty for custom
    /// (Advanced) images and headless jobs. Mirrors `RecentLaunch.Project`.
    #[serde(default)]
    pub project: Option<String>,
    /// `"fixed"` (send exact cores/ram/gpus) or `"flexible"` (platform-managed).
    /// `None` on legacy records is treated as flexible. Mirrors `ResourceType`.
    #[serde(default)]
    pub resource_type: Option<String>,
    /// Headless-only: the container command to replay on relaunch.
    #[serde(default)]
    pub cmd: Option<String>,
    /// Headless-only: command arguments to replay on relaunch.
    #[serde(default)]
    pub args: Option<Vec<String>>,
    /// Headless-only: replica count (1–20). `None` for interactive sessions.
    #[serde(default)]
    pub replicas: Option<u32>,
    /// RFC-3339 launch time. Falls back to `timestamp` on legacy records.
    #[serde(default)]
    pub launched_at: Option<String>,
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

    /// True when this batch job should replay through the headless path.
    pub fn is_headless(&self) -> bool {
        self.session_type.eq_ignore_ascii_case("headless")
    }

    /// Flexible unless `resource_type` is explicitly `"fixed"`. Legacy records
    /// (`resource_type == None`) are flexible, matching the reference default.
    pub fn is_flexible(&self) -> bool {
        self.resource_type.as_deref() != Some("fixed")
    }

    /// `"Flexible"` / `"Fixed"` label for the recent-launch card.
    pub fn resource_type_display(&self) -> &'static str {
        if self.is_flexible() {
            "Flexible"
        } else {
            "Fixed"
        }
    }

    /// Non-empty project name for display, if any.
    pub fn project_display(&self) -> Option<&str> {
        self.project.as_deref().filter(|p| !p.is_empty())
    }

    /// RFC-3339 launch time, preferring `launched_at` and falling back to the
    /// legacy `timestamp` field.
    pub fn launched_at_or_timestamp(&self) -> &str {
        self.launched_at
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or(&self.timestamp)
    }

    /// Compact relative date (`"5m ago"`, `"3d ago"`, …) for the card.
    pub fn relative_date(&self, now_rfc3339: &str) -> String {
        crate::helpers::discovery_formatting::time_ago(self.launched_at_or_timestamp(), now_rfc3339)
    }

    /// Rebuild launch parameters for a relaunch. Honours `resource_type`
    /// (flexible → cores/ram/gpus = 0) and replays the saved headless command
    /// line (cmd/args/replicas); interactive entries carry `None` for those, so
    /// they relaunch as interactive sessions. `name` is supplied by the caller
    /// (typically a freshly generated, non-colliding session name).
    pub fn to_launch_params(&self, name: String) -> SessionLaunchParams {
        let flexible = self.is_flexible();
        SessionLaunchParams {
            name,
            image: self.image.clone(),
            session_type: self.session_type.clone(),
            cores: if flexible { 0 } else { self.cores },
            ram: if flexible { 0 } else { self.ram },
            gpus: if flexible { 0 } else { self.gpus },
            cmd: self.cmd.clone(),
            env: None,
            registry_username: None,
            registry_secret: None,
            args: self.args.clone(),
            replicas: self.replicas.map(|r| r.max(1)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> RecentLaunch {
        RecentLaunch {
            name: "notebook1".to_string(),
            session_type: "notebook".to_string(),
            image: "images.canfar.net/skaha/astroml:latest".to_string(),
            cores: 4,
            ram: 16,
            gpus: 1,
            timestamp: "2026-07-07T12:00:00Z".to_string(),
            project: Some("skaha".to_string()),
            resource_type: Some("flexible".to_string()),
            cmd: None,
            args: None,
            replicas: None,
            launched_at: Some("2026-07-07T12:00:00Z".to_string()),
        }
    }

    #[test]
    fn legacy_json_without_new_fields_still_loads() {
        // Records written before the extra fields existed must still deserialize.
        let json = r#"{
            "name": "old1",
            "session_type": "desktop",
            "image": "images.canfar.net/skaha/desktop:latest",
            "cores": 2,
            "ram": 8,
            "gpus": 0,
            "timestamp": "2026-01-01T00:00:00Z"
        }"#;
        let rl: RecentLaunch = serde_json::from_str(json).unwrap();
        assert_eq!(rl.name, "old1");
        assert_eq!(rl.project, None);
        assert_eq!(rl.resource_type, None);
        assert_eq!(rl.replicas, None);
        assert_eq!(rl.launched_at, None);
        // Legacy record with no resource_type is treated as flexible.
        assert!(rl.is_flexible());
        // Relative date falls back to the legacy timestamp field.
        assert_eq!(rl.launched_at_or_timestamp(), "2026-01-01T00:00:00Z");
    }

    #[test]
    fn flexible_zeroes_resources_on_relaunch() {
        let rl = base(); // resource_type = flexible, cores/ram/gpus = 4/16/1
        let p = rl.to_launch_params("notebook2".to_string());
        assert_eq!(p.name, "notebook2");
        assert_eq!(p.session_type, "notebook");
        assert_eq!((p.cores, p.ram, p.gpus), (0, 0, 0));
        assert!(p.cmd.is_none());
        assert!(p.args.is_none());
        assert!(p.replicas.is_none());
    }

    #[test]
    fn fixed_preserves_resources_on_relaunch() {
        let mut rl = base();
        rl.resource_type = Some("fixed".to_string());
        let p = rl.to_launch_params("notebook2".to_string());
        assert_eq!((p.cores, p.ram, p.gpus), (4, 16, 1));
        assert!(!rl.is_flexible());
        assert_eq!(rl.resource_type_display(), "Fixed");
    }

    #[test]
    fn headless_relaunch_replays_command_line() {
        let mut rl = base();
        rl.session_type = "headless".to_string();
        rl.resource_type = Some("fixed".to_string());
        rl.cmd = Some("python".to_string());
        rl.args = Some(vec!["run.py".to_string(), "--fast".to_string()]);
        rl.replicas = Some(3);
        assert!(rl.is_headless());
        let p = rl.to_launch_params("headless5".to_string());
        assert_eq!(p.session_type, "headless");
        assert_eq!(p.cmd.as_deref(), Some("python"));
        assert_eq!(p.args.as_deref().unwrap().len(), 2);
        assert_eq!(p.replicas, Some(3));
        // Fixed headless keeps its exact resources.
        assert_eq!((p.cores, p.ram), (4, 16));
    }

    #[test]
    fn replicas_never_below_one_on_relaunch() {
        let mut rl = base();
        rl.session_type = "headless".to_string();
        rl.replicas = Some(0);
        let p = rl.to_launch_params("headless2".to_string());
        assert_eq!(p.replicas, Some(1));
    }

    #[test]
    fn project_display_hides_empty() {
        let mut rl = base();
        assert_eq!(rl.project_display(), Some("skaha"));
        rl.project = Some(String::new());
        assert_eq!(rl.project_display(), None);
        rl.project = None;
        assert_eq!(rl.project_display(), None);
    }

    #[test]
    fn relative_date_prefers_launched_at() {
        let rl = base(); // launched_at = 2026-07-07T12:00:00Z
        assert_eq!(rl.relative_date("2026-07-07T12:05:00Z"), "5m ago");
    }
}
