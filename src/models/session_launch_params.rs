use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct SessionLaunchParams {
    pub name: String,
    pub image: String,
    pub session_type: String,
    pub cores: u32,
    pub ram: u32,
    pub gpus: u32,
    pub cmd: Option<String>,
    pub env: Option<String>,
    pub registry_username: Option<String>,
    pub registry_secret: Option<String>,
    /// Headless-job command arguments (each becomes a repeated `args` form field).
    #[serde(default)]
    pub args: Option<Vec<String>>,
    /// Headless-job replica count (1–20); `None` for interactive sessions.
    #[serde(default)]
    pub replicas: Option<u32>,
}

impl SessionLaunchParams {
    pub fn to_form_pairs(&self) -> Vec<(&str, String)> {
        // Cores/RAM are sent verbatim: a value of 0 means "platform-managed"
        // (flexible allocation), matching the reference's flexible-mode contract.
        let mut pairs = vec![
            ("name", self.name.clone()),
            ("image", self.image.clone()),
            ("type", self.session_type.clone()),
            ("cores", self.cores.to_string()),
            ("ram", self.ram.to_string()),
        ];
        if self.gpus > 0 {
            pairs.push(("gpus", self.gpus.to_string()));
        }
        if let Some(ref cmd) = self.cmd {
            pairs.push(("cmd", cmd.clone()));
        }
        if let Some(ref args) = self.args {
            for arg in args {
                pairs.push(("args", arg.clone()));
            }
        }
        if let Some(replicas) = self.replicas {
            pairs.push(("replicas", replicas.to_string()));
        }
        if let Some(ref env) = self.env {
            pairs.push(("env", env.clone()));
        }
        pairs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_params() -> SessionLaunchParams {
        SessionLaunchParams {
            name: "test1".to_string(),
            image: "images.canfar.net/skaha/notebook:1.0".to_string(),
            session_type: "notebook".to_string(),
            cores: 2,
            ram: 8,
            gpus: 0,
            cmd: None,
            env: None,
            registry_username: None,
            registry_secret: None,
            args: None,
            replicas: None,
        }
    }

    #[test]
    fn form_pairs_basic() {
        let params = base_params();
        let pairs = params.to_form_pairs();
        assert_eq!(pairs.len(), 5);
        assert_eq!(pairs[0], ("name", "test1".to_string()));
        assert_eq!(
            pairs[1],
            ("image", "images.canfar.net/skaha/notebook:1.0".to_string())
        );
        assert_eq!(pairs[2], ("type", "notebook".to_string()));
        assert_eq!(pairs[3], ("cores", "2".to_string()));
        assert_eq!(pairs[4], ("ram", "8".to_string()));
    }

    #[test]
    fn form_pairs_with_gpus() {
        let mut params = base_params();
        params.gpus = 1;
        let pairs = params.to_form_pairs();
        assert_eq!(pairs.len(), 6);
        assert_eq!(pairs[5], ("gpus", "1".to_string()));
    }

    #[test]
    fn form_pairs_no_gpus_when_zero() {
        let params = base_params();
        let pairs = params.to_form_pairs();
        assert!(!pairs.iter().any(|(k, _)| *k == "gpus"));
    }

    #[test]
    fn form_pairs_with_cmd_and_env() {
        let mut params = base_params();
        params.cmd = Some("/bin/bash".to_string());
        params.env = Some("FOO=bar".to_string());
        let pairs = params.to_form_pairs();
        assert_eq!(pairs.len(), 7);
        assert!(pairs.contains(&("cmd", "/bin/bash".to_string())));
        assert!(pairs.contains(&("env", "FOO=bar".to_string())));
    }

    #[test]
    fn form_pairs_headless_args_and_replicas() {
        let mut params = base_params();
        params.session_type = "headless".to_string();
        params.cmd = Some("python".to_string());
        params.args = Some(vec!["run.py".to_string(), "--fast".to_string()]);
        params.replicas = Some(5);
        let pairs = params.to_form_pairs();
        assert_eq!(pairs.iter().filter(|(k, _)| *k == "args").count(), 2);
        assert!(pairs.contains(&("args", "run.py".to_string())));
        assert!(pairs.contains(&("replicas", "5".to_string())));
        assert!(pairs.contains(&("cmd", "python".to_string())));
    }

    #[test]
    fn flexible_sends_zero_cores_and_ram() {
        let mut params = base_params();
        params.cores = 0;
        params.ram = 0;
        let pairs = params.to_form_pairs();
        assert!(pairs.contains(&("cores", "0".to_string())));
        assert!(pairs.contains(&("ram", "0".to_string())));
    }

    #[test]
    fn registry_credentials_not_in_form() {
        let mut params = base_params();
        params.registry_username = Some("user".to_string());
        params.registry_secret = Some("pass".to_string());
        let pairs = params.to_form_pairs();
        // Registry creds go in headers, not form data
        assert!(!pairs.iter().any(|(k, _)| k.contains("registry")));
    }
}
