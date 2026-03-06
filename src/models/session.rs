use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkahaSessionResponse {
    pub id: String,
    pub userid: Option<String>,
    pub image: Option<String>,
    #[serde(rename = "type")]
    pub session_type: Option<String>,
    pub status: Option<String>,
    pub name: Option<String>,
    pub start_time: Option<String>,
    pub expiry_time: Option<String>,
    pub connect_url: Option<String>,
    pub requested_ram: Option<String>,
    pub requested_cpu_cores: Option<String>,
    pub requested_gpu_cores: Option<String>,
    pub ram_in_use: Option<String>,
    pub cpu_cores_in_use: Option<String>,
    pub is_fixed_resources: Option<bool>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Session {
    pub id: String,
    pub userid: String,
    pub image: String,
    pub session_type: String,
    pub status: String,
    pub name: String,
    pub start_time: String,
    pub expiry_time: String,
    pub connect_url: String,
    pub requested_ram: String,
    pub requested_cpu_cores: String,
    pub requested_gpu_cores: String,
    pub ram_in_use: String,
    pub cpu_cores_in_use: String,
    pub is_fixed_resources: bool,
}

impl From<SkahaSessionResponse> for Session {
    fn from(r: SkahaSessionResponse) -> Self {
        Session {
            id: r.id,
            userid: r.userid.unwrap_or_default(),
            image: r.image.unwrap_or_default(),
            session_type: r.session_type.unwrap_or_default(),
            status: r.status.unwrap_or_default(),
            name: r.name.unwrap_or_default(),
            start_time: r.start_time.unwrap_or_default(),
            expiry_time: r.expiry_time.unwrap_or_default(),
            connect_url: r.connect_url.unwrap_or_default(),
            requested_ram: r.requested_ram.unwrap_or_default(),
            requested_cpu_cores: r.requested_cpu_cores.unwrap_or_default(),
            requested_gpu_cores: r.requested_gpu_cores.unwrap_or("0".into()),
            ram_in_use: r.ram_in_use.unwrap_or_default(),
            cpu_cores_in_use: r.cpu_cores_in_use.unwrap_or_default(),
            is_fixed_resources: r.is_fixed_resources.unwrap_or(true),
        }
    }
}

impl Session {
    pub fn is_running(&self) -> bool {
        self.status.eq_ignore_ascii_case("running")
    }

    pub fn is_pending(&self) -> bool {
        self.status.eq_ignore_ascii_case("pending")
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
