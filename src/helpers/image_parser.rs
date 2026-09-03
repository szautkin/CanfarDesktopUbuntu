use crate::models::{ParsedImage, RawImage};

pub struct ImageParser;

impl ImageParser {
    pub fn parse_all(raw_images: &[RawImage]) -> Vec<ParsedImage> {
        raw_images.iter().map(ParsedImage::from_raw).collect()
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

    /// The distinct type GROUPS present, in the canonical order.
    ///
    /// Grouped rather than raw, so `desktop` and `desktop-app` are one button
    /// covering both rather than two buttons splitting the desktop images
    /// between them — see [`crate::models::session::type_group`].
    pub fn available_types(images: &[ParsedImage]) -> Vec<String> {
        let mut types: Vec<String> = images
            .iter()
            .flat_map(|img| img.types.iter())
            .map(|t| crate::models::session::type_group(t).to_string())
            .collect();
        types.sort();
        types.dedup();

        // The canonical order, from the one list every session-type surface
        // reads; anything the registry reports that is not in it sorts last.
        let order = crate::models::session::LAUNCHABLE_SESSION_TYPES;
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
    fn available_types_ordered() {
        let images = sample_images();
        let types = ImageParser::available_types(&images);
        assert_eq!(types, vec!["notebook", "desktop", "carta", "contributed"]);
    }

    #[test]
    fn desktop_app_is_offered_under_desktop() {
        // Skaha reports `desktop-app` as its own type — an application published
        // inside a desktop session, not a different thing to launch. Offering
        // both gave the filter bar two buttons for one idea and split the
        // desktop images between them.
        let raws = vec![
            RawImage {
                id: "images.canfar.net/skaha/desktop:1.0".to_string(),
                types: vec!["desktop".to_string()],
            },
            RawImage {
                id: "images.canfar.net/casa-4/casa:4.5.1".to_string(),
                types: vec!["headless".to_string(), "desktop-app".to_string()],
            },
        ];
        let types = ImageParser::available_types(&ImageParser::parse_all(&raws));
        assert!(
            !types.iter().any(|t| t == "desktop-app"),
            "desktop-app is still its own filter: {types:?}"
        );
        assert_eq!(types, vec!["desktop", "headless"]);
    }

    #[test]
    fn registries_for_type() {
        let images = sample_images();
        let regs = ImageParser::registries_for_type(&images, "notebook");
        assert_eq!(regs, vec!["harbor.canfar.net", "images.canfar.net"]);
    }

    #[test]
    fn projects_for_type_and_registry() {
        let images = sample_images();
        let projects =
            ImageParser::projects_for_type_and_registry(&images, "notebook", "images.canfar.net");
        assert_eq!(projects, vec!["skaha"]);
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
}
