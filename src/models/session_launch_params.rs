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
            ("gpus", self.gpus.to_string()),
        ];
        if let Some(ref cmd) = self.cmd {
            pairs.push(("cmd", cmd.clone()));
        }
        if let Some(ref env) = self.env {
            pairs.push(("env", env.clone()));
        }
        pairs
    }
}
