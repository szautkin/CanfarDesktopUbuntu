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
}

impl SessionLaunchParams {
    pub fn to_form_pairs(&self) -> Vec<(&str, String)> {
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
        }
    }

    #[test]
    fn form_pairs_basic() {
        let params = base_params();
        let pairs = params.to_form_pairs();
        assert_eq!(pairs.len(), 5);
        assert_eq!(pairs[0], ("name", "test1".to_string()));
        assert_eq!(pairs[1], ("image", "images.canfar.net/skaha/notebook:1.0".to_string()));
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
    fn registry_credentials_not_in_form() {
        let mut params = base_params();
        params.registry_username = Some("user".to_string());
        params.registry_secret = Some("pass".to_string());
        let pairs = params.to_form_pairs();
        // Registry creds go in headers, not form data
        assert!(!pairs.iter().any(|(k, _)| k.contains("registry")));
    }
}
