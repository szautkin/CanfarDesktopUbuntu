use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct RawImage {
    pub id: String,
    pub types: Vec<String>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ParsedImage {
    pub id: String,
    pub registry: String,
    pub project: String,
    pub name: String,
    pub version: String,
    pub types: Vec<String>,
    pub display_name: String,
}

impl ParsedImage {
    pub fn from_raw(raw: &RawImage) -> Self {
        let id = &raw.id;
        let (registry, project, name, version) = parse_image_id(id);
        let display_name = format!("{}/{}:{}", project, name, version);
        ParsedImage {
            id: id.clone(),
            registry,
            project,
            name,
            version,
            types: raw.types.clone(),
            display_name,
        }
    }
}

fn parse_image_id(id: &str) -> (String, String, String, String) {
    let (main, version) = match id.rsplit_once(':') {
        Some((m, v)) => (m, v.to_string()),
        None => (id, "latest".to_string()),
    };

    let parts: Vec<&str> = main.split('/').collect();
    match parts.len() {
        1 => (String::new(), String::new(), parts[0].to_string(), version),
        2 => (
            String::new(),
            parts[0].to_string(),
            parts[1].to_string(),
            version,
        ),
        3 => (
            parts[0].to_string(),
            parts[1].to_string(),
            parts[2].to_string(),
            version,
        ),
        _ => (
            parts[0].to_string(),
            parts[1..parts.len() - 1].join("/"),
            parts[parts.len() - 1].to_string(),
            version,
        ),
    }
}
