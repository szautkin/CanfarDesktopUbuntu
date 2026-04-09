use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionTemplate {
    pub name: String,
    pub description: String,
    pub session_type: String,
    pub image: String,
    pub cores: u32,
    pub ram: u32,
    pub gpus: u32,
    pub created_at: DateTime<Utc>,
}

impl SessionTemplate {
    pub fn new(
        name: String,
        description: String,
        session_type: String,
        image: String,
        cores: u32,
        ram: u32,
        gpus: u32,
    ) -> Self {
        SessionTemplate {
            name,
            description,
            session_type,
            image,
            cores,
            ram,
            gpus,
            created_at: Utc::now(),
        }
    }
}
