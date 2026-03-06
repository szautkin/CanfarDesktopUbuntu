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
            .filter(|img| {
                img.types.iter().any(|t| t == session_type) && img.project == project
            })
            .cloned()
            .collect();
        filtered.sort_by(|a, b| b.version.cmp(&a.version));
        filtered
    }

    pub fn available_types(images: &[ParsedImage]) -> Vec<String> {
        let mut types: Vec<String> = images
            .iter()
            .flat_map(|img| img.types.clone())
            .collect();
        types.sort();
        types.dedup();

        let order = ["notebook", "desktop", "carta", "contributed", "firefly", "headless"];
        types.sort_by_key(|t| {
            order.iter().position(|o| o == t).unwrap_or(order.len())
        });
        types
    }
}
