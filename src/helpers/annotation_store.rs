//! Annotations on disk, keyed by the file they were drawn on.
//!
//! Same shape as [`crate::helpers::fits_bookmarks`], for the same reasons: JSON
//! under the data dir, and a read that cannot fail loudly. A viewer must open
//! whether or not this file is readable — losing annotations is a
//! disappointment, and refusing to show an image because of them would be a
//! bug.
//!
//! Keyed by target path so marks come back with the image, and so two FITS
//! files never show each other's.

use crate::models::annotation::Annotation;
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Every target's annotations.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AnnotationFile {
    /// Target path → its marks.
    #[serde(default)]
    pub targets: BTreeMap<String, Vec<Annotation>>,
}

fn store_path() -> Option<PathBuf> {
    ProjectDirs::from("net", "canfar", "Verbinal")
        .map(|dirs| dirs.data_dir().join("annotations.json"))
}

/// Load everything. An unreadable or corrupt file is an empty set.
pub fn load_all() -> AnnotationFile {
    let Some(path) = store_path() else {
        return AnnotationFile::default();
    };
    let Ok(bytes) = std::fs::read(&path) else {
        return AnnotationFile::default();
    };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

/// The marks on one target, in creation order.
pub fn load_for(target: &str) -> Vec<Annotation> {
    load_all().targets.remove(target).unwrap_or_default()
}

/// Replace one target's marks.
///
/// Read-modify-write of the whole file: the sets are small, and a partial
/// write that dropped another file's annotations would be a silent loss of
/// someone's work.
pub fn save_for(target: &str, annotations: &[Annotation]) -> Result<(), String> {
    let path =
        store_path().ok_or_else(|| "no data directory to save annotations in".to_string())?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("could not create {dir:?}: {e}"))?;
    }
    let mut all = load_all();
    if annotations.is_empty() {
        all.targets.remove(target);
    } else {
        all.targets.insert(target.to_string(), annotations.to_vec());
    }
    let json = serde_json::to_vec_pretty(&all).map_err(|e| format!("could not encode: {e}"))?;
    std::fs::write(&path, json).map_err(|e| format!("could not write {path:?}: {e}"))
}

/// Add one mark to a target, returning what is now stored.
pub fn add(target: &str, annotation: Annotation) -> Result<Vec<Annotation>, String> {
    annotation.validate()?;
    let mut current = load_for(target);
    current.push(annotation);
    save_for(target, &current)?;
    Ok(current)
}

/// Remove one mark by id. `Ok(false)` when there was no such id.
pub fn remove(target: &str, id: &str) -> Result<bool, String> {
    let mut current = load_for(target);
    let before = current.len();
    current.retain(|a| a.id != id);
    if current.len() == before {
        return Ok(false);
    }
    save_for(target, &current)?;
    Ok(true)
}

/// Remove every mark on a target, returning how many went.
pub fn clear(target: &str) -> Result<usize, String> {
    let count = load_for(target).len();
    save_for(target, &[])?;
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::annotation::{Anchor, AnnotationKind, Author};

    fn mark(text: &str) -> Annotation {
        Annotation::new(
            AnnotationKind::Text,
            Anchor::ImagePixel { x: 10.0, y: 20.0 },
            text,
            Author::User,
        )
    }

    /// A corrupt file is an empty set, not a crash.
    ///
    /// A viewer must open whether or not this file is readable. Refusing to
    /// show an image because its annotations would not parse is a worse
    /// outcome than losing the annotations.
    #[test]
    fn a_corrupt_file_reads_as_empty() {
        let parsed: AnnotationFile = serde_json::from_slice(b"{ not json").unwrap_or_default();
        assert!(parsed.targets.is_empty());
        let half: AnnotationFile =
            serde_json::from_slice(br#"{"targets": {"a.fits": [{"id": "#).unwrap_or_default();
        assert!(half.targets.is_empty());
    }

    /// An empty file is a valid file.
    #[test]
    fn an_empty_document_loads() {
        let parsed: AnnotationFile = serde_json::from_slice(b"{}").expect("loads");
        assert!(parsed.targets.is_empty());
    }

    /// Targets do not see each other's marks.
    #[test]
    fn two_targets_keep_their_own() {
        let mut file = AnnotationFile::default();
        file.targets.insert("a.fits".into(), vec![mark("in a")]);
        file.targets
            .insert("b.fits".into(), vec![mark("in b"), mark("also b")]);
        let json = serde_json::to_string(&file).expect("encode");
        let back: AnnotationFile = serde_json::from_str(&json).expect("decode");
        assert_eq!(back.targets["a.fits"].len(), 1);
        assert_eq!(back.targets["b.fits"].len(), 2);
        assert_eq!(back.targets["a.fits"][0].text, "in a");
    }

    /// The order marks were made in survives the round trip.
    #[test]
    fn creation_order_is_kept() {
        let marks: Vec<Annotation> = ["first", "second", "third"]
            .iter()
            .map(|t| mark(t))
            .collect();
        let mut file = AnnotationFile::default();
        file.targets.insert("c.fits".into(), marks);
        let back: AnnotationFile =
            serde_json::from_str(&serde_json::to_string(&file).unwrap()).unwrap();
        let texts: Vec<&str> = back.targets["c.fits"]
            .iter()
            .map(|a| a.text.as_str())
            .collect();
        assert_eq!(texts, ["first", "second", "third"]);
    }

    /// An invalid mark is refused before it reaches the file.
    #[test]
    fn an_unusable_mark_is_not_stored() {
        let bad = Annotation::new(
            AnnotationKind::Callout,
            Anchor::ImagePixel { x: 1.0, y: 1.0 },
            "",
            Author::Agent,
        );
        assert!(
            bad.validate().is_err(),
            "an empty callout should not validate"
        );
    }
}
