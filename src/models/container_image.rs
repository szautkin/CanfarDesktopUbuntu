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

pub fn parse_image_id(id: &str) -> (String, String, String, String) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_full_image_id() {
        let (reg, proj, name, ver) = parse_image_id("images.canfar.net/skaha/notebook:1.0");
        assert_eq!(reg, "images.canfar.net");
        assert_eq!(proj, "skaha");
        assert_eq!(name, "notebook");
        assert_eq!(ver, "1.0");
    }

    #[test]
    fn parse_image_id_no_registry() {
        let (reg, proj, name, ver) = parse_image_id("skaha/notebook:latest");
        assert_eq!(reg, "");
        assert_eq!(proj, "skaha");
        assert_eq!(name, "notebook");
        assert_eq!(ver, "latest");
    }

    #[test]
    fn parse_image_id_name_only() {
        let (reg, proj, name, ver) = parse_image_id("notebook");
        assert_eq!(reg, "");
        assert_eq!(proj, "");
        assert_eq!(name, "notebook");
        assert_eq!(ver, "latest");
    }

    #[test]
    fn parse_image_id_no_version() {
        let (reg, proj, name, ver) = parse_image_id("images.canfar.net/skaha/notebook");
        assert_eq!(reg, "images.canfar.net");
        assert_eq!(proj, "skaha");
        assert_eq!(name, "notebook");
        assert_eq!(ver, "latest");
    }

    #[test]
    fn parse_image_id_deep_path() {
        let (reg, proj, name, ver) =
            parse_image_id("registry.example.com/org/sub/image:v2.3");
        assert_eq!(reg, "registry.example.com");
        assert_eq!(proj, "org/sub");
        assert_eq!(name, "image");
        assert_eq!(ver, "v2.3");
    }

    #[test]
    fn parsed_image_from_raw() {
        let raw = RawImage {
            id: "images.canfar.net/skaha/notebook:1.0".to_string(),
            types: vec!["notebook".to_string()],
        };
        let parsed = ParsedImage::from_raw(&raw);
        assert_eq!(parsed.display_name, "skaha/notebook:1.0");
        assert_eq!(parsed.registry, "images.canfar.net");
        assert_eq!(parsed.project, "skaha");
        assert_eq!(parsed.name, "notebook");
        assert_eq!(parsed.version, "1.0");
        assert_eq!(parsed.types, vec!["notebook"]);
    }
}
