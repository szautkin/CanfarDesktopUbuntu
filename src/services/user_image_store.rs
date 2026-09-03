//! The images the user added from the registry, kept between runs.
//!
//! A single JSON file, mirrored in memory because the images widget reads the
//! whole list on every rebuild — a filter toggle, a probe finishing, a
//! catalogue refresh — and re-reading a file on each of those would put disk
//! I/O on the frame path.
//!
//! `<data_dir>` is the same place the rest of the app keeps its state:
//! `ProjectDirs::from("net","canfar","Verbinal").data_dir()`.

use crate::models::RegistryImage;
use directories::ProjectDirs;
use std::path::PathBuf;
use std::sync::Mutex;

/// How many images the list may hold.
///
/// Not a storage limit — the file is tiny — but a list limit. These are images
/// the user picked out by hand, and a list long enough to need its own search
/// has stopped being that; the platform catalogue and the package search are
/// what scale.
pub const MAX_USER_IMAGES: usize = 200;

pub struct UserImageStore {
    file_path: PathBuf,
    /// `None` until first read. Hydrated lazily so constructing the store —
    /// which happens during start-up, on the main thread — touches no disk.
    cache: Mutex<Option<Vec<RegistryImage>>>,
}

impl UserImageStore {
    pub fn new() -> Self {
        let file_path = ProjectDirs::from("net", "canfar", "Verbinal")
            .map(|dirs| dirs.data_dir().join("user_images.json"))
            .unwrap_or_else(|| PathBuf::from("user_images.json"));
        UserImageStore {
            file_path,
            cache: Mutex::new(None),
        }
    }

    /// Every image the user has added, newest first.
    pub fn list(&self) -> Vec<RegistryImage> {
        self.hydrated().clone()
    }

    /// Whether `id` is in the list.
    ///
    /// Its own method rather than `list().iter().any(..)` at each call site:
    /// the registry browser asks it once per result row, and cloning the whole
    /// list to answer one question is the kind of thing that turns a search
    /// result into a stutter.
    pub fn contains(&self, id: &str) -> bool {
        let cache = self.cache.lock().unwrap();
        match cache.as_ref() {
            Some(list) => list.iter().any(|i| i.id == id),
            None => {
                drop(cache);
                self.hydrated().iter().any(|i| i.id == id)
            }
        }
    }

    /// Add `image`, or move it to the front if it is already there.
    pub fn add(&self, image: RegistryImage) -> Result<(), String> {
        let mut list = self.hydrated();
        list.retain(|i| i.id != image.id);
        list.insert(0, image.added());
        list.truncate(MAX_USER_IMAGES);
        self.commit(list)
    }

    /// Remove `id`. Removing something that is not there is not an error — the
    /// caller wanted it gone, and it is.
    pub fn remove(&self, id: &str) -> Result<(), String> {
        let mut list = self.hydrated();
        list.retain(|i| i.id != id);
        self.commit(list)
    }

    /// The list, read from disk on first use.
    fn hydrated(&self) -> Vec<RegistryImage> {
        let mut cache = self.cache.lock().unwrap();
        if cache.is_none() {
            *cache = Some(self.read_file());
        }
        cache.clone().unwrap_or_default()
    }

    fn read_file(&self) -> Vec<RegistryImage> {
        match std::fs::read_to_string(&self.file_path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
            Err(_) => Vec::new(),
        }
    }

    /// Write the list and keep the memory mirror in step.
    ///
    /// Written through a temp file and renamed, so a crash mid-write leaves the
    /// previous list rather than a truncated one — the same rule the manifest
    /// store follows.
    fn commit(&self, list: Vec<RegistryImage>) -> Result<(), String> {
        let json = serde_json::to_string_pretty(&list).map_err(|e| e.to_string())?;
        if let Some(parent) = self.file_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let temp = self.file_path.with_extension("json.tmp");
        std::fs::write(&temp, json).map_err(|e| e.to_string())?;
        std::fs::rename(&temp, &self.file_path).map_err(|e| e.to_string())?;

        *self.cache.lock().unwrap() = Some(list);
        Ok(())
    }
}

impl Default for UserImageStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A store over a throwaway path, so a test never touches the real list.
    fn store(name: &str) -> UserImageStore {
        let path = std::env::temp_dir().join(format!("verbinal-user-images-{name}.json"));
        let _ = std::fs::remove_file(&path);
        UserImageStore {
            file_path: path,
            cache: Mutex::new(None),
        }
    }

    fn image(id: &str) -> RegistryImage {
        RegistryImage::new(id, &["notebook".into()])
    }

    #[test]
    fn an_added_image_survives_a_restart() {
        // The whole point of the list: the user found something once and should
        // not have to find it again.
        let s = store("restart");
        s.add(image("images.canfar.net/me/a:1")).unwrap();

        let reopened = UserImageStore {
            file_path: s.file_path.clone(),
            cache: Mutex::new(None),
        };
        assert_eq!(reopened.list().len(), 1);
        assert!(reopened.contains("images.canfar.net/me/a:1"));
    }

    #[test]
    fn adding_the_same_image_twice_leaves_one() {
        // Reachable: the browser shows Remove for an added image, but a second
        // window, or a stale list, can still offer Add.
        let s = store("dupe");
        s.add(image("x:1")).unwrap();
        s.add(image("x:1")).unwrap();
        assert_eq!(s.list().len(), 1);
    }

    #[test]
    fn the_newest_addition_is_first() {
        let s = store("order");
        s.add(image("a:1")).unwrap();
        s.add(image("b:1")).unwrap();
        assert_eq!(s.list()[0].id, "b:1");
    }

    #[test]
    fn removing_something_absent_is_not_an_error() {
        // Two windows open on the same list, or a double click: the caller
        // wanted it gone and it is, which is not a failure to report.
        let s = store("absent");
        assert!(s.remove("never-there").is_ok());
    }

    #[test]
    fn an_added_image_records_when() {
        let s = store("when");
        s.add(image("a:1")).unwrap();
        assert!(
            s.list()[0].added_at.is_some(),
            "the list cannot tell added images from search results"
        );
    }

    #[test]
    fn the_list_stays_a_list() {
        let s = store("cap");
        for n in 0..MAX_USER_IMAGES + 10 {
            s.add(image(&format!("img:{n}"))).unwrap();
        }
        assert_eq!(s.list().len(), MAX_USER_IMAGES);
        // Newest kept, oldest dropped.
        assert_eq!(s.list()[0].id, format!("img:{}", MAX_USER_IMAGES + 9));
    }

    #[test]
    fn a_corrupt_file_reads_as_an_empty_list() {
        // Never a crash on start-up over a file the user can edit.
        let s = store("corrupt");
        std::fs::write(&s.file_path, "{not json").unwrap();
        assert!(s.list().is_empty());
    }
}
