use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct SkahaStatsResponse {
    pub instances: Option<InstanceStats>,
    pub cores: Option<CoreStats>,
    pub ram: Option<RamStats>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceStats {
    pub session: Option<u32>,
    pub desktop_app: Option<u32>,
    pub headless: Option<u32>,
    pub total: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CoreStats {
    #[serde(rename = "requestedCPUCores")]
    pub requested_cpu_cores: Option<serde_json::Value>,
    #[serde(rename = "cpuCoresAvailable")]
    pub cpu_cores_available: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RamStats {
    #[serde(rename = "requestedRAM")]
    pub requested_ram: Option<String>,
    #[serde(rename = "ramAvailable")]
    pub ram_available: Option<String>,
}

impl CoreStats {
    pub fn requested(&self) -> f64 {
        self.requested_cpu_cores
            .as_ref()
            .and_then(|v| {
                v.as_f64()
                    .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
            })
            .unwrap_or(0.0)
    }

    pub fn available(&self) -> f64 {
        self.cpu_cores_available
            .as_ref()
            .and_then(|v| {
                v.as_f64()
                    .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
            })
            .unwrap_or(0.0)
    }

    pub fn total(&self) -> f64 {
        self.requested() + self.available()
    }
}

impl RamStats {
    pub fn requested_gb(&self) -> f64 {
        self.requested_ram
            .as_ref()
            .map(|s| parse_ram_string(s))
            .unwrap_or(0.0)
    }

    pub fn available_gb(&self) -> f64 {
        self.ram_available
            .as_ref()
            .map(|s| parse_ram_string(s))
            .unwrap_or(0.0)
    }

    pub fn total_gb(&self) -> f64 {
        self.requested_gb() + self.available_gb()
    }
}

fn parse_ram_string(s: &str) -> f64 {
    let s = s.trim();
    if let Some(num) = s.strip_suffix('G').or_else(|| s.strip_suffix("Gi")) {
        num.parse().unwrap_or(0.0)
    } else if let Some(num) = s.strip_suffix('M').or_else(|| s.strip_suffix("Mi")) {
        num.parse::<f64>().unwrap_or(0.0) / 1024.0
    } else if let Some(num) = s.strip_suffix('T').or_else(|| s.strip_suffix("Ti")) {
        num.parse::<f64>().unwrap_or(0.0) * 1024.0
    } else {
        s.parse().unwrap_or(0.0)
    }
}
