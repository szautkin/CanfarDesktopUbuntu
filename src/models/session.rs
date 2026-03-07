use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct SkahaSessionResponse {
    pub id: String,
    pub userid: Option<String>,
    pub image: Option<String>,
    #[serde(rename = "type")]
    pub session_type: Option<String>,
    pub status: Option<String>,
    pub name: Option<String>,
    #[serde(rename = "startTime")]
    pub start_time: Option<String>,
    #[serde(rename = "expiryTime")]
    pub expiry_time: Option<String>,
    #[serde(rename = "connectURL")]
    pub connect_url: Option<String>,
    #[serde(rename = "requestedRAM")]
    pub requested_ram: Option<String>,
    #[serde(rename = "requestedCPUCores")]
    pub requested_cpu_cores: Option<String>,
    #[serde(rename = "requestedGPUCores")]
    pub requested_gpu_cores: Option<String>,
    #[serde(rename = "ramInUse")]
    pub ram_in_use: Option<String>,
    #[serde(rename = "cpuCoresInUse")]
    pub cpu_cores_in_use: Option<String>,
    #[serde(rename = "isFixedResources")]
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_skaha_response() {
        let json = r#"{
            "id": "abc123",
            "userid": "testuser",
            "image": "images.canfar.net/skaha/notebook:1.0",
            "type": "notebook",
            "status": "Running",
            "name": "notebook1",
            "startTime": "2024-01-15T10:00:00Z",
            "expiryTime": "2024-01-22T10:00:00Z",
            "connectURL": "https://example.com/session/abc123",
            "requestedRAM": "8G",
            "requestedCPUCores": "2",
            "requestedGPUCores": "0",
            "ramInUse": "4G",
            "cpuCoresInUse": "1",
            "isFixedResources": true
        }"#;

        let resp: SkahaSessionResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.id, "abc123");
        assert_eq!(resp.session_type.as_deref(), Some("notebook"));
        assert_eq!(resp.connect_url.as_deref(), Some("https://example.com/session/abc123"));
        assert_eq!(resp.requested_ram.as_deref(), Some("8G"));
        assert_eq!(resp.requested_cpu_cores.as_deref(), Some("2"));
        assert_eq!(resp.requested_gpu_cores.as_deref(), Some("0"));
        assert_eq!(resp.is_fixed_resources, Some(true));
    }

    #[test]
    fn deserialize_minimal_response() {
        let json = r#"{"id": "xyz"}"#;
        let resp: SkahaSessionResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.id, "xyz");
        assert!(resp.session_type.is_none());
        assert!(resp.connect_url.is_none());
    }

    #[test]
    fn session_from_response_defaults() {
        let resp = SkahaSessionResponse {
            id: "abc".to_string(),
            userid: None,
            image: None,
            session_type: None,
            status: None,
            name: None,
            start_time: None,
            expiry_time: None,
            connect_url: None,
            requested_ram: None,
            requested_cpu_cores: None,
            requested_gpu_cores: None,
            ram_in_use: None,
            cpu_cores_in_use: None,
            is_fixed_resources: None,
        };
        let session = Session::from(resp);
        assert_eq!(session.id, "abc");
        assert_eq!(session.userid, "");
        assert_eq!(session.requested_gpu_cores, "0");
        assert!(session.is_fixed_resources);
    }

    #[test]
    fn session_status_checks() {
        let mut session = Session::from(SkahaSessionResponse {
            id: "1".to_string(),
            userid: None,
            image: None,
            session_type: None,
            status: Some("Running".to_string()),
            name: None,
            start_time: None,
            expiry_time: None,
            connect_url: None,
            requested_ram: None,
            requested_cpu_cores: None,
            requested_gpu_cores: None,
            ram_in_use: None,
            cpu_cores_in_use: None,
            is_fixed_resources: None,
        });
        assert!(session.is_running());
        assert!(!session.is_pending());

        session.status = "Pending".to_string();
        assert!(!session.is_running());
        assert!(session.is_pending());

        session.status = "RUNNING".to_string();
        assert!(session.is_running());
    }
}
