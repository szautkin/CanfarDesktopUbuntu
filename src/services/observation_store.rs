use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// On-disk schema version for the observations envelope. Bump when the record
/// shape changes incompatibly so an older build refuses to load (and refuses to
/// overwrite) a file written by a newer build. Mirrors
/// `ObservationStore.SchemaVersion` in the Windows reference.
const SCHEMA_VERSION: u32 = 1;

/// Serialisation view of the versioned envelope written to disk:
/// `{ "schema_version": N, "value": [ ... ] }`.  Borrows the slice so `write`
/// never has to clone the whole list.  Mirrors `DiskPersistence.Envelope<T>`.
#[derive(Serialize)]
struct EnvelopeRef<'a> {
    schema_version: u32,
    value: &'a [DownloadedObservation],
}

/// Read the `schema_version` off a parsed envelope root, accepting both the
/// snake_case field this build writes and the camelCase variant the Windows
/// reference emits.  Defaults to the current version when the field is absent so
/// a `{ "value": [...] }` object (no explicit version) still loads — matching
/// the "add the version field with serde default" contract.
fn envelope_version(root: &serde_json::Value) -> u64 {
    root.get("schema_version")
        .or_else(|| root.get("schemaVersion"))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(SCHEMA_VERSION as u64)
}

/// A single observation that the user has either bookmarked (metadata only)
/// or downloaded (with a local FITS file) from the CADC archive.
///
/// When `local_path` is empty the entry is a bookmark; otherwise it has a
/// downloaded file on disk.  `thumbnail_url` / `preview_url` carry optional
/// DataLink preview URLs so the Research page can show a thumbnail without
/// re-hitting the network.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DownloadedObservation {
    /// Locally generated UUID identifying this record.
    pub id: String,
    /// CADC publisher DID (e.g. `ivo://cadc.nrc.ca/CFHT?123456`).
    pub publisher_id: String,
    pub collection: String,
    pub observation_id: String,
    pub target_name: String,
    pub instrument: String,
    pub filter: String,
    pub ra: String,
    pub dec: String,
    pub start_date: String,
    pub cal_level: String,
    /// Absolute path to the file on disk. Empty string means "bookmarked only".
    pub local_path: String,
    /// File size in bytes.  Zero when bookmarked only.
    pub file_size: u64,
    /// ISO-8601 timestamp of when the file was downloaded or bookmarked.
    pub downloaded_at: String,
    /// DataLink `#thumbnail` URL, if available. Optional for backwards compat.
    #[serde(default)]
    pub thumbnail_url: String,
    /// DataLink `#preview` URL, if available. Optional for backwards compat.
    #[serde(default)]
    pub preview_url: String,
    /// Absolute path to the locally downloaded preview image, if any.
    /// The Research page reads previews from this path and never touches
    /// the network once an observation is saved.
    #[serde(default)]
    pub local_preview_path: String,
    /// Optional AI-agent provenance recorded when this record was created over
    /// MCP.  Holds either a JSON-serialised `AgentAttribution` or a bare client
    /// label; the Research page renders an `agent_badge` when it is present.
    /// Optional + `#[serde(default)]` so pre-existing JSON stays readable.
    /// Mirrors `DownloadedObservation.AgentAttribution` in the Windows reference.
    #[serde(default)]
    pub agent_attribution: Option<String>,

    // ── Citation handle ─────────────────────────────────────────────────────
    //
    // CADC assigns no DOI or bibcode to an individual observation, so the
    // originating proposal is the closest citable handle — which is exactly what
    // the exported `notes.md` tells the user to cite. It said so while these
    // fields did not exist, so the bundle promised a citation it never carried.
    //
    // All four are `#[serde(default)]`: records saved before they existed load
    // as empty, and an empty field is simply omitted from the citation block.
    /// Proposal / program id (CAOM2 `Observation.proposal_id`).
    #[serde(default)]
    pub proposal_id: String,
    /// Principal investigator (CAOM2 `Observation.proposal_pi`).
    #[serde(default)]
    pub proposal_pi: String,
    /// Proposal title (CAOM2 `Observation.proposal_title`).
    #[serde(default)]
    pub proposal_title: String,
    /// Data-release date (CAOM2 `Plane.dataRelease`) — when the data became, or
    /// becomes, public. Part of citing a proprietary-period observation.
    #[serde(default)]
    pub data_release: String,
}

impl DownloadedObservation {
    /// True when the FITS file path is empty (metadata-only record).
    /// These are legacy records saved before the full-save redesign —
    /// the new "Save to Research" flow always downloads both files.
    pub fn is_bookmarked(&self) -> bool {
        self.local_path.is_empty()
    }

    /// True when `local_path` points to a file that currently exists on disk.
    pub fn has_fits(&self) -> bool {
        !self.local_path.is_empty() && std::path::Path::new(&self.local_path).exists()
    }

    /// True when `local_preview_path` points to a file that currently exists on disk.
    pub fn has_local_preview(&self) -> bool {
        !self.local_preview_path.is_empty()
            && std::path::Path::new(&self.local_preview_path).exists()
    }

    /// Human-readable file size (e.g. "3.4 MB"). Returns empty string for
    /// records with no downloaded FITS file.
    pub fn formatted_size(&self) -> String {
        if self.is_bookmarked() {
            String::new()
        } else {
            format_bytes(self.file_size)
        }
    }
}

// ---------------------------------------------------------------------------
// Managed storage — one subdirectory per observation under
// `~/.local/share/verbinal/observations/{obs_id}/`.
// ---------------------------------------------------------------------------

/// Return the base directory that holds all managed observation files.
pub fn observations_base_dir() -> PathBuf {
    ProjectDirs::from("net", "canfar", "Verbinal")
        .map(|dirs| dirs.data_dir().join("observations"))
        .unwrap_or_else(|| PathBuf::from("observations"))
}

/// Return the managed subdirectory path for a given observation id.
/// Does NOT create the directory — callers should `mkdir_p` as needed.
pub fn managed_dir_for(obs_id: &str) -> PathBuf {
    observations_base_dir().join(sanitize_obs_id(obs_id))
}

/// Delete an observation's managed subdirectory (ignoring errors).
pub fn delete_managed_dir(obs_id: &str) {
    let dir = managed_dir_for(obs_id);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Best-effort sanitization of an observation id into a filesystem-safe name.
fn sanitize_obs_id(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Persistent JSON-backed store for downloaded observations.
///
/// Stored at `~/.local/share/net.canfar/Verbinal/observations.json`.
pub struct ObservationStore {
    data_path: PathBuf,
}

impl ObservationStore {
    pub fn new() -> Self {
        let data_path = ProjectDirs::from("net", "canfar", "Verbinal")
            .map(|dirs| dirs.data_dir().join("observations.json"))
            .unwrap_or_else(|| PathBuf::from("observations.json"));
        ObservationStore { data_path }
    }

    /// Load all observations from disk.
    ///
    /// **Nothing is pruned.** A record whose `local_path` no longer resolves is
    /// returned unchanged: the file may sit on an unmounted volume or a
    /// disconnected /arc mount, and the record carries metadata — target,
    /// instrument, notes, provenance — that the file itself does not. Dropping
    /// it at load time would destroy that permanently, and `save` writes the
    /// loaded list straight back, so the loss would be committed on the next
    /// write. The UI shows a "file missing" affordance and offers a re-download
    /// instead. This matches the reference's explicit no-prune contract.
    ///
    /// (This doc comment previously described the opposite — a load-time purge
    /// of records with missing files — while the code correctly refused to do
    /// it. Restated because the next person to trust the comment over the code
    /// would have "fixed" the code into exactly the data loss above.)
    ///
    /// Returns an empty list on any parse or I/O error — but, crucially, a
    /// corrupt file is *quarantined* (renamed to a `.corrupt-<n>` sibling)
    /// rather than silently loaded as empty, and a file written by a NEWER
    /// schema version is left untouched.  Both guards exist because the next
    /// `save` writes the loaded list straight back: loading a corrupt/newer file
    /// as empty would clobber it and destroy the user's data.  Mirrors the
    /// Windows `DiskPersistence.Read`.
    pub fn load(&self) -> Vec<DownloadedObservation> {
        if !self.data_path.exists() {
            return Vec::new();
        }
        let raw = match std::fs::read_to_string(&self.data_path) {
            Ok(json) => json,
            // Transient read failure (file locked, permissions) — report empty
            // but do NOT quarantine; the file may read fine on the next attempt.
            Err(_) => return Vec::new(),
        };

        // Discriminate the on-disk shape before deserializing the records so a
        // NEWER-schema file is refused (not clobbered) and a truly corrupt file
        // is quarantined instead of silently loaded as empty.
        let root: serde_json::Value = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(_) => {
                self.quarantine();
                return Vec::new();
            }
        };

        // Do NOT prune records whose on-disk file is currently missing: the file
        // may live on an offline/unmounted volume, and dropping the record would
        // lose its metadata permanently. The UI shows a "file missing" affordance
        // (and offers to re-download) rather than deleting the record. (Matches the
        // reference ObservationStore's explicit no-prune contract.)

        // Versioned envelope: `{ "schema_version": N, "value": [ ... ] }`.
        if root.is_object() {
            match root.get("value") {
                Some(value) => {
                    if envelope_version(&root) > SCHEMA_VERSION as u64 {
                        // Written by a newer build: load nothing and leave the
                        // file intact so a `save` here never downgrades it.
                        return Vec::new();
                    }
                    match serde_json::from_value::<Vec<DownloadedObservation>>(value.clone()) {
                        Ok(list) => list,
                        Err(_) => {
                            self.quarantine();
                            Vec::new()
                        }
                    }
                }
                // An object that is not an envelope is unexpected → quarantine.
                None => {
                    self.quarantine();
                    Vec::new()
                }
            }
        } else {
            // Legacy bare array (pre-envelope): still readable so existing
            // users' files are never lost.
            match serde_json::from_value::<Vec<DownloadedObservation>>(root) {
                Ok(list) => list,
                Err(_) => {
                    self.quarantine();
                    Vec::new()
                }
            }
        }
    }

    /// Append (or replace by `id`) an observation and flush to disk.
    ///
    /// This is a blocking call — prefer `save_async` from a tokio context.
    pub fn save(&self, obs: DownloadedObservation) -> Result<(), String> {
        let mut list = self.load();
        list.retain(|o| o.id != obs.id);
        list.insert(0, obs);
        self.write(&list)
    }

    /// Remove an observation by its local `id`.
    ///
    /// This is a blocking call — prefer `remove_async` from a tokio context.
    pub fn remove(&self, id: &str) -> Result<(), String> {
        let mut list = self.load();
        list.retain(|o| o.id != id);
        self.write(&list)
    }

    /// Async variant of `save` that offloads disk I/O to the tokio
    /// blocking thread pool.  Call this from any async context.
    pub async fn save_async(&self, obs: DownloadedObservation) -> Result<(), String> {
        let path = self.data_path.clone();
        tokio::task::spawn_blocking(move || {
            let tmp_store = ObservationStore { data_path: path };
            tmp_store.save(obs)
        })
        .await
        .unwrap_or_else(|e| Err(format!("blocking pool error: {e}")))
    }

    /// Async variant of `remove` that offloads disk I/O to the tokio
    /// blocking thread pool.
    pub async fn remove_async(&self, id: &str) -> Result<(), String> {
        let path = self.data_path.clone();
        let id = id.to_string();
        tokio::task::spawn_blocking(move || {
            let tmp_store = ObservationStore { data_path: path };
            tmp_store.remove(&id)
        })
        .await
        .unwrap_or_else(|e| Err(format!("blocking pool error: {e}")))
    }

    /// Async variant of `load` that offloads disk I/O.
    pub async fn load_async(&self) -> Vec<DownloadedObservation> {
        let path = self.data_path.clone();
        tokio::task::spawn_blocking(move || {
            let tmp_store = ObservationStore { data_path: path };
            tmp_store.load()
        })
        .await
        .unwrap_or_default()
    }

    /// Returns `true` if an observation with the given CADC publisher ID already exists.
    pub fn contains_publisher_id(&self, publisher_id: &str) -> bool {
        self.load().iter().any(|o| o.publisher_id == publisher_id)
    }

    /// Return observations whose collection, observation_id, target, or instrument
    /// contain `text` (case-insensitive).  An empty `text` returns everything.
    pub fn filter(&self, text: &str) -> Vec<DownloadedObservation> {
        let list = self.load();
        if text.is_empty() {
            return list;
        }
        let needle = text.to_lowercase();
        list.into_iter()
            .filter(|o| {
                o.collection.to_lowercase().contains(&needle)
                    || o.observation_id.to_lowercase().contains(&needle)
                    || o.target_name.to_lowercase().contains(&needle)
                    || o.instrument.to_lowercase().contains(&needle)
                    || o.filter.to_lowercase().contains(&needle)
            })
            .collect()
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    fn write(&self, list: &[DownloadedObservation]) -> Result<(), String> {
        // Never clobber a file written by a newer schema version.
        if let Some(existing) = self.peek_version() {
            if existing > SCHEMA_VERSION as u64 {
                return Err(format!(
                    "refusing to overwrite observations.json written by a newer app \
                     version (on-disk schema {existing} > {SCHEMA_VERSION})"
                ));
            }
        }
        if let Some(parent) = self.data_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        // Wrap the records in the versioned envelope so a future build can detect
        // and refuse an older writer.
        let envelope = EnvelopeRef {
            schema_version: SCHEMA_VERSION,
            value: list,
        };
        let json = serde_json::to_string_pretty(&envelope).map_err(|e| e.to_string())?;
        // Atomic write: write to a .tmp sibling then rename to avoid data
        // corruption on crash or NFS partial writes.
        let tmp = self.data_path.with_extension("json.tmp");
        std::fs::write(&tmp, &json).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &self.data_path).map_err(|e| e.to_string())
    }

    /// Peek the on-disk envelope's `schema_version` without deserializing the
    /// records.  Returns `None` for a missing/legacy/unreadable file (all safe
    /// to overwrite).
    fn peek_version(&self) -> Option<u64> {
        let raw = std::fs::read_to_string(&self.data_path).ok()?;
        let root: serde_json::Value = serde_json::from_str(&raw).ok()?;
        if root.is_object() {
            root.get("schema_version")
                .or_else(|| root.get("schemaVersion"))
                .and_then(serde_json::Value::as_u64)
        } else {
            None
        }
    }

    /// Move a corrupt data file aside to a `.corrupt-<n>` sibling (never
    /// overwriting a previous quarantine) so its bytes survive for recovery
    /// instead of being clobbered by the next `save`.  Best effort — never
    /// throws from the load path.  Mirrors `DiskPersistence.Quarantine`.
    fn quarantine(&self) {
        let base = self.data_path.as_os_str().to_string_lossy().into_owned();
        for n in 0..1000u32 {
            let dest = PathBuf::from(format!("{base}.corrupt-{n}"));
            if !dest.exists() {
                let _ = std::fs::rename(&self.data_path, &dest);
                return;
            }
        }
        // Exhausted the numbered slots — fall back to clobbering the first.
        let _ = std::fs::rename(&self.data_path, PathBuf::from(format!("{base}.corrupt-0")));
    }
}

impl Default for ObservationStore {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Utility
// ---------------------------------------------------------------------------

fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1_024;
    const MB: u64 = KB * 1_024;
    const GB: u64 = MB * 1_024;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    // Test-only constructor pointing the store at an arbitrary path so tests
    // never touch the user's real observations file.
    impl ObservationStore {
        fn with_path(path: PathBuf) -> Self {
            ObservationStore { data_path: path }
        }
    }

    /// A unique temp path per test, with its data + quarantine siblings cleaned
    /// up on drop.
    struct TempStore {
        path: PathBuf,
    }

    impl TempStore {
        fn new() -> Self {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "verbinal_obs_store_test_{}_{}_{}.json",
                std::process::id(),
                nanos,
                n
            ));
            TempStore { path }
        }

        fn store(&self) -> ObservationStore {
            ObservationStore::with_path(self.path.clone())
        }
    }

    impl Drop for TempStore {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
            let _ = std::fs::remove_file(self.path.with_extension("json.tmp"));
            let base = self.path.as_os_str().to_string_lossy().into_owned();
            for n in 0..8u32 {
                let _ = std::fs::remove_file(PathBuf::from(format!("{base}.corrupt-{n}")));
            }
        }
    }

    #[test]
    fn save_then_load_roundtrips_via_envelope() {
        let tmp = TempStore::new();
        let store = tmp.store();
        store.save(sample_obs()).unwrap();

        // On-disk file is the versioned envelope, not a bare array.
        let raw = std::fs::read_to_string(&tmp.path).unwrap();
        assert!(raw.contains("\"schema_version\""));
        assert!(raw.contains("\"value\""));

        let loaded = store.load();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].collection, "CFHT");
    }

    #[test]
    fn legacy_bare_array_still_loads() {
        let tmp = TempStore::new();
        // Pre-envelope on-disk format: a bare JSON array.
        std::fs::write(
            &tmp.path,
            serde_json::to_string(&vec![sample_obs()]).unwrap(),
        )
        .unwrap();
        let loaded = tmp.store().load();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].observation_id, "obs-001");
        // File was readable → NOT quarantined.
        let base = tmp.path.as_os_str().to_string_lossy().into_owned();
        assert!(!PathBuf::from(format!("{base}.corrupt-0")).exists());
    }

    #[test]
    fn corrupt_file_is_quarantined_not_clobbered() {
        let tmp = TempStore::new();
        std::fs::write(&tmp.path, b"{ this is not valid json ][").unwrap();

        let loaded = tmp.store().load();
        assert!(loaded.is_empty());

        // The bad bytes were moved aside, not deleted — the original path is gone
        // and a `.corrupt-<n>` sibling holds the quarantined content.
        assert!(
            !tmp.path.exists(),
            "corrupt file should have been renamed away"
        );
        let base = tmp.path.as_os_str().to_string_lossy().into_owned();
        let quarantined = PathBuf::from(format!("{base}.corrupt-0"));
        assert!(quarantined.exists(), "quarantine sibling should exist");
        assert_eq!(
            std::fs::read_to_string(&quarantined).unwrap(),
            "{ this is not valid json ]["
        );
    }

    #[test]
    fn newer_schema_is_not_clobbered() {
        let tmp = TempStore::new();
        // A file written by a hypothetical future build (schema 999).
        let future = format!(
            r#"{{ "schema_version": 999, "value": [ {} ] }}"#,
            serde_json::to_string(&sample_obs()).unwrap()
        );
        std::fs::write(&tmp.path, &future).unwrap();

        let store = tmp.store();
        // Refuse to load the newer file (returns empty, leaves it intact).
        assert!(store.load().is_empty());

        // A save must NOT overwrite the newer file.
        let err = store.save(sample_obs()).unwrap_err();
        assert!(err.contains("newer"), "save should refuse: {err}");

        // The original newer-schema bytes are still on disk, untouched.
        let raw = std::fs::read_to_string(&tmp.path).unwrap();
        assert!(raw.contains("999"));
    }

    #[test]
    fn format_bytes_scales_correctly() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1023), "1023 B");
        assert_eq!(format_bytes(1024), "1.0 KB");
        assert_eq!(format_bytes(1_048_576), "1.0 MB");
        assert_eq!(format_bytes(1_073_741_824), "1.0 GB");
    }

    /// The no-prune contract, pinned.
    ///
    /// A record pointing at a path that does not exist must survive a load —
    /// and, because `save` writes the loaded list back, must survive a
    /// save/reload cycle too. The doc comment on `load` used to promise the
    /// opposite, so a maintainer trusting it would have "restored" a purge that
    /// permanently discards the target, instrument, notes and provenance of
    /// every observation sitting on an unmounted volume.
    #[test]
    fn a_record_whose_file_is_missing_survives_load_and_save() {
        let temp = TempStore::new();
        let store = temp.store();

        let mut obs = sample_obs();
        obs.local_path = "/nonexistent/volume/never-mounted.fits".into();
        assert!(
            !std::path::Path::new(&obs.local_path).exists(),
            "the fixture must really be missing for this to prove anything"
        );
        store.save(obs.clone()).expect("save");

        let loaded = store.load();
        assert_eq!(loaded.len(), 1, "the record must not be pruned on load");
        assert_eq!(loaded[0].target_name, "M31", "its metadata is intact");

        // The dangerous half: `save` rewrites whatever `load` returned, so a
        // prune at load time would be committed to disk on the next write.
        let mut second = sample_obs();
        second.id = "2".into();
        second.local_path = String::new(); // a bookmark-only record
        store.save(second).expect("save");

        let after = store.load();
        assert_eq!(after.len(), 2, "the missing-file record survived a rewrite");
        assert!(
            after.iter().any(|o| o.id == "1"),
            "the record with the missing file is still there"
        );
    }

    fn sample_obs() -> DownloadedObservation {
        DownloadedObservation {
            id: "1".into(),
            publisher_id: "pub1".into(),
            collection: "CFHT".into(),
            observation_id: "obs-001".into(),
            target_name: "M31".into(),
            instrument: "MegaCam".into(),
            filter: "g".into(),
            ra: "10.6".into(),
            dec: "41.2".into(),
            start_date: "2020-01-01".into(),
            cal_level: "1".into(),
            local_path: "/tmp/test.fits".into(),
            file_size: 1024,
            downloaded_at: "2024-01-01T00:00:00Z".into(),
            thumbnail_url: String::new(),
            preview_url: String::new(),
            local_preview_path: String::new(),
            agent_attribution: None,
            proposal_id: String::new(),
            proposal_pi: String::new(),
            proposal_title: String::new(),
            data_release: String::new(),
        }
    }

    #[test]
    fn filter_is_case_insensitive() {
        let obs = sample_obs();
        let list = [obs];
        let needle = "cfht";
        let filtered: Vec<_> = list
            .iter()
            .filter(|o| o.collection.to_lowercase().contains(needle))
            .cloned()
            .collect();
        assert_eq!(filtered.len(), 1);
    }

    #[test]
    fn is_bookmarked_by_empty_local_path() {
        let mut obs = sample_obs();
        assert!(!obs.is_bookmarked());
        obs.local_path = String::new();
        assert!(obs.is_bookmarked());
    }

    #[test]
    fn formatted_size_empty_for_bookmark() {
        let mut obs = sample_obs();
        obs.local_path = String::new();
        obs.file_size = 0;
        assert_eq!(obs.formatted_size(), "");
    }

    #[test]
    fn managed_dir_sanitises_id() {
        let dir = managed_dir_for("obs-abc123");
        assert!(dir.ends_with("observations/obs-abc123"));
        // Weird characters should be replaced with underscores
        let dir2 = managed_dir_for("ivo://cadc/CFHT?123");
        let name = dir2.file_name().unwrap().to_string_lossy().to_string();
        assert!(!name.contains('/'));
        assert!(!name.contains(':'));
        assert!(!name.contains('?'));
    }

    #[test]
    fn backwards_compat_json_without_preview_urls() {
        // Older JSON format without thumbnail_url/preview_url fields should
        // still deserialize thanks to #[serde(default)].
        let legacy_json = r#"[
            {
                "id": "1",
                "publisher_id": "pub1",
                "collection": "CFHT",
                "observation_id": "obs-001",
                "target_name": "M31",
                "instrument": "MegaCam",
                "filter": "g",
                "ra": "10.6",
                "dec": "41.2",
                "start_date": "2020-01-01",
                "cal_level": "1",
                "local_path": "/tmp/test.fits",
                "file_size": 1024,
                "downloaded_at": "2024-01-01T00:00:00Z"
            }
        ]"#;
        let parsed: Vec<DownloadedObservation> = serde_json::from_str(legacy_json).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].thumbnail_url, "");
        assert_eq!(parsed[0].preview_url, "");
        // Legacy records predate agent attribution → defaults to None.
        assert_eq!(parsed[0].agent_attribution, None);
    }

    #[test]
    fn agent_attribution_round_trips() {
        let mut obs = sample_obs();
        obs.agent_attribution = Some("Claude Desktop".into());
        let json = serde_json::to_string(&obs).unwrap();
        let back: DownloadedObservation = serde_json::from_str(&json).unwrap();
        assert_eq!(back.agent_attribution.as_deref(), Some("Claude Desktop"));
    }
}
