pub struct StorageQuota {
    pub quota_bytes: u64,
    pub used_bytes: u64,
    pub last_update: Option<String>,
}

impl StorageQuota {
    pub fn quota_gb(&self) -> f64 {
        self.quota_bytes as f64 / (1024.0 * 1024.0 * 1024.0)
    }

    pub fn used_gb(&self) -> f64 {
        self.used_bytes as f64 / (1024.0 * 1024.0 * 1024.0)
    }

    pub fn usage_percent(&self) -> f64 {
        if self.quota_bytes == 0 {
            0.0
        } else {
            (self.used_bytes as f64 / self.quota_bytes as f64) * 100.0
        }
    }

    pub fn is_warning(&self) -> bool {
        self.usage_percent() > 90.0
    }
}
