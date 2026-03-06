use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionContext {
    pub cores: Option<ResourceOption>,
    pub memory_gb: Option<ResourceOption>,
    pub gpus: Option<GpuOption>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceOption {
    pub default: Option<serde_json::Value>,
    pub options: Option<Vec<serde_json::Value>>,
    pub default_request: Option<String>,
    pub available_values: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuOption {
    pub options: Option<Vec<serde_json::Value>>,
}

impl SessionContext {
    pub fn core_options(&self) -> Vec<u32> {
        self.cores
            .as_ref()
            .and_then(|c| c.available_values.as_ref())
            .map(|v| v.iter().filter_map(|s| s.parse().ok()).collect())
            .or_else(|| {
                self.cores.as_ref().and_then(|c| c.options.as_ref()).map(|v| {
                    v.iter()
                        .filter_map(|s| s.as_u64().map(|n| n as u32))
                        .collect()
                })
            })
            .unwrap_or_else(|| vec![1, 2, 4, 8, 16])
    }

    pub fn memory_options(&self) -> Vec<u32> {
        self.memory_gb
            .as_ref()
            .and_then(|m| m.available_values.as_ref())
            .map(|v| v.iter().filter_map(|s| s.parse().ok()).collect())
            .or_else(|| {
                self.memory_gb
                    .as_ref()
                    .and_then(|m| m.options.as_ref())
                    .map(|v| {
                        v.iter()
                            .filter_map(|s| s.as_u64().map(|n| n as u32))
                            .collect()
                    })
            })
            .unwrap_or_else(|| vec![1, 2, 4, 8, 16, 32])
    }

    pub fn gpu_options(&self) -> Vec<u32> {
        self.gpus
            .as_ref()
            .and_then(|g| g.options.as_ref())
            .map(|v| {
                v.iter()
                    .filter_map(|s| s.as_u64().map(|n| n as u32))
                    .collect()
            })
            .unwrap_or_else(|| vec![0, 1, 2])
    }

    pub fn default_cores(&self) -> u32 {
        self.cores
            .as_ref()
            .and_then(|c| c.default_request.as_ref())
            .and_then(|s| s.parse().ok())
            .or_else(|| {
                self.cores
                    .as_ref()
                    .and_then(|c| c.default.as_ref())
                    .and_then(|v| v.as_u64())
                    .map(|n| n as u32)
            })
            .unwrap_or(2)
    }

    pub fn default_memory(&self) -> u32 {
        self.memory_gb
            .as_ref()
            .and_then(|m| m.default_request.as_ref())
            .and_then(|s| s.parse().ok())
            .or_else(|| {
                self.memory_gb
                    .as_ref()
                    .and_then(|m| m.default.as_ref())
                    .and_then(|v| v.as_u64())
                    .map(|n| n as u32)
            })
            .unwrap_or(8)
    }
}
