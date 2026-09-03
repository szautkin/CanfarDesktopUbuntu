//! An image as the container registry describes it.
//!
//! The platform's own catalogue (`/v1/image`) is the app's default source of
//! images, and it is a curated subset: Skaha lists what it will launch. The
//! registry behind it holds a great deal more, and someone who knows the image
//! they want — a colleague's build, a tag Skaha has not picked up — has no way
//! to reach it from a list that does not contain it.
//!
//! So this is the other door. A registry image enters the app only because
//! someone searched for it and added it, never by a background sweep: pulling a
//! whole Harbor instance to populate a dashboard card would be a great deal of
//! traffic to answer a question nobody asked.
//!
//! One type serves both roles, because they are the same image at two moments.
//! [`added_at`] is what separates them: `None` for a search result the user is
//! looking at, `Some` once it is in their list.
//!
//! [`added_at`]: RegistryImage::added_at

use serde::{Deserialize, Serialize};

/// The session types the platform recognises, which is what the CANFAR registry
/// names its labels after.
///
/// A registry image carries labels for whatever its authors chose; only the
/// ones that name a session type mean anything to a launch, and those are what
/// the images widget filters by. Kept here rather than in the widget because
/// the registry search and the widget must agree on it — one list, one meaning.
const SESSION_TYPE_LABELS: [&str; 6] = [
    "notebook",
    "desktop",
    "desktop-app",
    "carta",
    "headless",
    "contributed",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryImage {
    /// Fully qualified: `host/project/name:tag`. The same form `/v1/image`
    /// reports and the launch form submits, so an added image needs no
    /// translation to be launched.
    pub id: String,
    /// Session types, taken from the registry's labels.
    ///
    /// May be empty — an image whose labels say nothing about session types is
    /// still launchable from the Advanced tab, which takes an image reference
    /// directly.
    #[serde(default)]
    pub types: Vec<String>,
    /// When the user added it to their list; `None` for a search result they
    /// have not added.
    #[serde(default)]
    pub added_at: Option<String>,
}

impl RegistryImage {
    /// Build one from a registry reference and the labels the registry reports.
    ///
    /// Labels that do not name a session type are dropped rather than kept as
    /// pseudo-types: the widget's filter bar and the launch form's type list
    /// are both built from types, and seeding them with "gpu" or "v2" would put
    /// choices there that mean nothing to either.
    pub fn new(id: impl Into<String>, labels: &[String]) -> Self {
        let types = labels
            .iter()
            .filter(|l| {
                let l = l.trim().to_ascii_lowercase();
                SESSION_TYPE_LABELS.contains(&l.as_str())
            })
            .map(|l| l.trim().to_ascii_lowercase())
            .collect();
        RegistryImage {
            id: id.into(),
            types,
            added_at: None,
        }
    }

    /// The same image, marked as added now.
    pub fn added(mut self) -> Self {
        self.added_at = Some(chrono::Utc::now().to_rfc3339());
        self
    }

    /// As the platform's own catalogue would report it, so that one merged list
    /// can be built without the rest of the app caring where an image came
    /// from.
    pub fn as_raw(&self) -> crate::models::RawImage {
        crate::models::RawImage {
            id: self.id.clone(),
            types: self.types.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_labels_that_name_a_session_type_become_types() {
        let img = RegistryImage::new(
            "images.canfar.net/skaha/astroml:latest",
            &[
                "notebook".into(),
                "gpu".into(),
                "cuda-12".into(),
                "headless".into(),
            ],
        );
        // "gpu" and "cuda-12" describe the image, not how it is launched. In
        // the filter bar they would be types the user could select and get
        // nothing launchable from.
        assert_eq!(img.types, vec!["notebook", "headless"]);
    }

    #[test]
    fn a_label_is_matched_however_the_registry_cased_it() {
        let img = RegistryImage::new("x", &["  NoteBook ".into()]);
        assert_eq!(img.types, vec!["notebook"]);
    }

    #[test]
    fn an_image_with_no_recognised_label_is_still_an_image() {
        // It cannot be filtered by type, and the Standard tab will not offer
        // it, but the Advanced tab takes an image reference directly — so
        // dropping it here would be taking away the one thing the user added it
        // for.
        let img = RegistryImage::new("images.canfar.net/me/private:1", &["internal".into()]);
        assert!(img.types.is_empty());
        assert_eq!(img.id, "images.canfar.net/me/private:1");
    }

    #[test]
    fn adding_records_when() {
        let img = RegistryImage::new("x", &[]);
        assert!(img.added_at.is_none(), "a search result is not yet added");
        assert!(img.added().added_at.is_some());
    }
}
