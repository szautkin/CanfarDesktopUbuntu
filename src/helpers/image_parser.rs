use crate::models::{ParsedImage, RawImage};
use std::collections::BTreeMap;

#[allow(dead_code)]
pub struct ImageParser;

#[allow(dead_code)]
impl ImageParser {
    pub fn parse_all(raw_images: &[RawImage]) -> Vec<ParsedImage> {
        raw_images.iter().map(ParsedImage::from_raw).collect()
    }

    pub fn group_by_type(images: &[ParsedImage]) -> BTreeMap<String, Vec<ParsedImage>> {
        let mut map: BTreeMap<String, Vec<ParsedImage>> = BTreeMap::new();
        for img in images {
            for t in &img.types {
                map.entry(t.clone()).or_default().push(img.clone());
            }
        }
        map
    }

    pub fn registries_for_type(images: &[ParsedImage], session_type: &str) -> Vec<String> {
        let mut registries: Vec<String> = images
            .iter()
            .filter(|img| img.types.iter().any(|t| t == session_type))
            .map(|img| img.registry.clone())
            .filter(|r| !r.is_empty())
            .collect();
        registries.sort();
        registries.dedup();
        registries
    }

    pub fn projects_for_type_and_registry(
        images: &[ParsedImage],
        session_type: &str,
        registry: &str,
    ) -> Vec<String> {
        let mut projects: Vec<String> = images
            .iter()
            .filter(|img| img.types.iter().any(|t| t == session_type) && img.registry == registry)
            .map(|img| img.project.clone())
            .collect();
        projects.sort();
        projects.dedup();
        projects
    }

    pub fn images_for_type_registry_and_project(
        images: &[ParsedImage],
        session_type: &str,
        registry: &str,
        project: &str,
    ) -> Vec<ParsedImage> {
        let mut filtered: Vec<ParsedImage> = images
            .iter()
            .filter(|img| {
                img.types.iter().any(|t| t == session_type)
                    && img.registry == registry
                    && img.project == project
            })
            .cloned()
            .collect();
        filtered.sort_by(|a, b| b.version.cmp(&a.version));
        filtered
    }

    pub fn projects_for_type(images: &[ParsedImage], session_type: &str) -> Vec<String> {
        let mut projects: Vec<String> = images
            .iter()
            .filter(|img| img.types.iter().any(|t| t == session_type))
            .map(|img| img.project.clone())
            .collect();
        projects.sort();
        projects.dedup();
        projects
    }

    pub fn images_for_type_and_project(
        images: &[ParsedImage],
        session_type: &str,
        project: &str,
    ) -> Vec<ParsedImage> {
        let mut filtered: Vec<ParsedImage> = images
            .iter()
            .filter(|img| img.types.iter().any(|t| t == session_type) && img.project == project)
            .cloned()
            .collect();
        filtered.sort_by(|a, b| b.version.cmp(&a.version));
        filtered
    }

    pub fn available_types(images: &[ParsedImage]) -> Vec<String> {
        let mut types: Vec<String> = images.iter().flat_map(|img| img.types.clone()).collect();
        types.sort();
        types.dedup();

        let order = [
            "notebook",
            "desktop",
            "carta",
            "contributed",
            "firefly",
            "headless",
        ];
        types.sort_by_key(|t| order.iter().position(|o| o == t).unwrap_or(order.len()));
        types
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_images() -> Vec<ParsedImage> {
        let raws = vec![
            RawImage {
                id: "images.canfar.net/skaha/notebook-scipy:1.0".to_string(),
                types: vec!["notebook".to_string()],
            },
            RawImage {
                id: "images.canfar.net/skaha/notebook-scipy:2.0".to_string(),
                types: vec!["notebook".to_string()],
            },
            RawImage {
                id: "images.canfar.net/skaha/desktop:1.0".to_string(),
                types: vec!["desktop".to_string()],
            },
            RawImage {
                id: "images.canfar.net/canucs/carta:4.0".to_string(),
                types: vec!["carta".to_string()],
            },
            RawImage {
                id: "harbor.canfar.net/contrib/myapp:0.1".to_string(),
                types: vec!["contributed".to_string(), "notebook".to_string()],
            },
        ];
        ImageParser::parse_all(&raws)
    }

    #[test]
    fn parse_all_count() {
        let images = sample_images();
        assert_eq!(images.len(), 5);
    }

    #[test]
    fn group_by_type() {
        let images = sample_images();
        let grouped = ImageParser::group_by_type(&images);
        assert_eq!(grouped["notebook"].len(), 3); // 2 scipy + 1 contributed
        assert_eq!(grouped["desktop"].len(), 1);
        assert_eq!(grouped["carta"].len(), 1);
        assert_eq!(grouped["contributed"].len(), 1);
    }

    #[test]
    fn available_types_ordered() {
        let images = sample_images();
        let types = ImageParser::available_types(&images);
        assert_eq!(types, vec!["notebook", "desktop", "carta", "contributed"]);
    }

    #[test]
    fn registries_for_type() {
        let images = sample_images();
        let regs = ImageParser::registries_for_type(&images, "notebook");
        assert_eq!(regs, vec!["harbor.canfar.net", "images.canfar.net"]);
    }

    #[test]
    fn projects_for_type() {
        let images = sample_images();
        let projects = ImageParser::projects_for_type(&images, "notebook");
        assert_eq!(projects, vec!["contrib", "skaha"]);
    }

    #[test]
    fn projects_for_type_and_registry() {
        let images = sample_images();
        let projects =
            ImageParser::projects_for_type_and_registry(&images, "notebook", "images.canfar.net");
        assert_eq!(projects, vec!["skaha"]);
    }

    #[test]
    fn images_for_type_and_project_sorted_desc() {
        let images = sample_images();
        let filtered = ImageParser::images_for_type_and_project(&images, "notebook", "skaha");
        assert_eq!(filtered.len(), 2);
        // Descending by version
        assert_eq!(filtered[0].version, "2.0");
        assert_eq!(filtered[1].version, "1.0");
    }

    #[test]
    fn images_for_type_registry_and_project() {
        let images = sample_images();
        let filtered = ImageParser::images_for_type_registry_and_project(
            &images,
            "notebook",
            "images.canfar.net",
            "skaha",
        );
        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].version, "2.0");
    }

    #[test]
    fn empty_results_for_unknown_type() {
        let images = sample_images();
        let regs = ImageParser::registries_for_type(&images, "unknown");
        assert!(regs.is_empty());
        let projects = ImageParser::projects_for_type(&images, "unknown");
        assert!(projects.is_empty());
    }
}
