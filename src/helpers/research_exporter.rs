//! Build a Claude-friendly research bundle from the downloaded observations +
//! astronomer notes, then pack it into a single `.zip`.
//!
//! Rust port of `Services/Export/ResearchExporter.cs` + `ResearchExportBuilder.cs`.
//! The bundle contains three files, keyed by publisher DID so the notes
//! cross-reference the observations:
//!
//! * `observations.json` — every saved/downloaded observation (pretty JSON).
//! * `notes.json`        — every astronomer note (pretty JSON).
//! * `notes.md`          — one human-readable markdown section per observation
//!                         that has a note (citation + rating + tags + body).
//!
//! ## Why a hand-rolled ZIP writer
//!
//! There is no `zip` crate in the dependency tree, so rather than pull one in
//! (or drop a loose directory of files) we emit a **store-only** (uncompressed,
//! method 0) ZIP with a tiny writer below. Store-only ZIPs are trivially valid —
//! local file headers + a central directory + an end-of-central-directory
//! record, each entry carrying its own CRC-32 — and every unzip tool reads them.
//! This keeps the export a single portable file, matching the reference which
//! also produces a `.zip`.
//!
//! The [`build_bundle`] renderer is deliberately GTK-free and pure so the JSON +
//! markdown rendering is unit-testable with fabricated data (mirrors the split
//! between `ResearchExporter` and `ResearchExportBuilder`).

use crate::models::observation_note::ObservationNote;
use crate::services::observation_store::DownloadedObservation;
use chrono::{DateTime, Datelike, Timelike, Utc};
use std::collections::HashMap;
use std::path::Path;

/// The rendered payload of a research export, before it is zipped. Kept separate
/// from the disk-writing step so the rendering is unit-testable.
#[derive(Debug, Clone)]
pub struct ResearchBundle {
    pub observations_json: String,
    pub notes_json: String,
    pub notes_md: String,
    pub observation_count: usize,
    pub note_count: usize,
}

/// Render the JSON + markdown payload from in-memory data. Pure — no I/O.
///
/// `now` is stamped into the markdown header so callers can pass a fixed clock
/// in tests.
pub fn build_bundle(
    observations: &[DownloadedObservation],
    notes: &[ObservationNote],
    now: DateTime<Utc>,
) -> ResearchBundle {
    let observations_json =
        serde_json::to_string_pretty(observations).unwrap_or_else(|_| "[]".to_string());
    let notes_json = serde_json::to_string_pretty(notes).unwrap_or_else(|_| "[]".to_string());
    let notes_md = render_notes_markdown(observations, notes, now);

    ResearchBundle {
        observations_json,
        notes_json,
        notes_md,
        observation_count: observations.len(),
        note_count: notes.len(),
    }
}

/// Build the bundle and write it to `path` as a store-only ZIP.
///
/// Returns the number of observations written on success. Blocking disk I/O —
/// call from a blocking thread (e.g. `tokio::task::spawn_blocking`).
pub fn write_bundle_zip(
    path: &Path,
    observations: &[DownloadedObservation],
    notes: &[ObservationNote],
) -> Result<usize, String> {
    let bundle = build_bundle(observations, notes, Utc::now());
    let entries: [(&str, &[u8]); 3] = [
        ("observations.json", bundle.observations_json.as_bytes()),
        ("notes.json", bundle.notes_json.as_bytes()),
        ("notes.md", bundle.notes_md.as_bytes()),
    ];
    write_zip(path, &entries)?;
    Ok(bundle.observation_count)
}

// ---------------------------------------------------------------------------
// Markdown rendering (1-to-1 with ResearchExportBuilder.RenderNotesMarkdown)
// ---------------------------------------------------------------------------

/// One markdown document, one section per observation that has a note, in the
/// observations' stored (download) order.
fn render_notes_markdown(
    observations: &[DownloadedObservation],
    notes: &[ObservationNote],
    now: DateTime<Utc>,
) -> String {
    let mut by_id: HashMap<&str, &ObservationNote> = HashMap::new();
    for n in notes {
        if !n.publisher_id.is_empty() {
            by_id.insert(n.publisher_id.as_str(), n);
        }
    }

    let with_notes: Vec<&DownloadedObservation> = observations
        .iter()
        .filter(|o| by_id.contains_key(o.publisher_id.as_str()))
        .collect();

    let mut md = String::new();
    md.push_str("# Research Notes\n\n");
    md.push_str(&format!(
        "Exported {}. {} of {} observations have notes.\n\n",
        iso_utc(now),
        with_notes.len(),
        observations.len()
    ));
    md.push_str("---\n\n");

    if with_notes.is_empty() {
        md.push_str("_No notes have been written yet._\n");
        return md;
    }

    for obs in with_notes {
        // `by_id` guarantees this lookup succeeds.
        if let Some(note) = by_id.get(obs.publisher_id.as_str()) {
            md.push_str(&render_observation_section(obs, note));
        }
    }

    md
}

fn render_observation_section(obs: &DownloadedObservation, note: &ObservationNote) -> String {
    let title = if obs.target_name.is_empty() {
        obs.observation_id.as_str()
    } else {
        obs.target_name.as_str()
    };

    let mut md = String::new();
    md.push_str(&format!(
        "## {} — {} {}\n\n",
        title, obs.collection, obs.observation_id
    ));

    md.push_str(&format!("- **Publisher ID:** `{}`\n", obs.publisher_id));
    if !obs.target_name.is_empty() {
        md.push_str(&format!("- **Target:** {}\n", obs.target_name));
    }
    if !obs.collection.is_empty() {
        md.push_str(&format!("- **Collection:** {}\n", obs.collection));
    }
    if !obs.observation_id.is_empty() {
        md.push_str(&format!("- **Observation ID:** {}\n", obs.observation_id));
    }
    if !obs.instrument.is_empty() {
        let instrument = if obs.filter.is_empty() {
            obs.instrument.clone()
        } else {
            format!("{} / {}", obs.instrument, obs.filter)
        };
        md.push_str(&format!("- **Instrument:** {}\n", instrument));
    }
    if !obs.ra.is_empty() || !obs.dec.is_empty() {
        md.push_str(&format!("- **Coordinates:** RA {}, Dec {}\n", obs.ra, obs.dec));
    }
    if !obs.start_date.is_empty() {
        md.push_str(&format!("- **Start date:** {}\n", obs.start_date));
    }
    if !obs.cal_level.is_empty() {
        md.push_str(&format!("- **Calibration level:** {}\n", obs.cal_level));
    }
    if !obs.downloaded_at.is_empty() {
        md.push_str(&format!("- **Downloaded:** {}\n", iso_or_raw(&obs.downloaded_at)));
    }
    if note.rating > 0 {
        md.push_str(&format!(
            "- **Quality:** {} ({})\n",
            stars(note.rating),
            quality_label(note.rating)
        ));
    }
    if !note.tags.is_empty() {
        let tags = note
            .tags
            .iter()
            .map(|t| format!("`{}`", t))
            .collect::<Vec<_>>()
            .join(", ");
        md.push_str(&format!("- **Tags:** {}\n", tags));
    }
    if !note.updated.is_empty() {
        md.push_str(&format!("- **Note modified:** {}\n", iso_or_raw(&note.updated)));
    }

    let trimmed = note.note.trim();
    if !trimmed.is_empty() {
        md.push_str("\n### Notes\n\n");
        md.push_str(trimmed);
        md.push('\n');
    }

    md.push_str("\n---\n\n");
    md
}

fn stars(n: u8) -> String {
    let filled = n.min(5) as usize;
    let mut s = String::with_capacity(15);
    for _ in 0..filled {
        s.push('★');
    }
    for _ in 0..(5 - filled) {
        s.push('☆');
    }
    s
}

fn quality_label(stars: u8) -> &'static str {
    match stars {
        1 => "Unusable",
        2 => "Poor",
        3 => "Fair",
        4 => "Good",
        5 => "Excellent",
        _ => "",
    }
}

/// Format a UTC instant as `yyyy-MM-ddTHH:mm:ssZ`.
fn iso_utc(dt: DateTime<Utc>) -> String {
    dt.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// Normalise an RFC-3339 timestamp to `yyyy-MM-ddTHH:mm:ssZ`; if it does not
/// parse, return it unchanged (records may carry pre-existing free-form dates).
fn iso_or_raw(s: &str) -> String {
    match DateTime::parse_from_rfc3339(s) {
        Ok(dt) => iso_utc(dt.with_timezone(&Utc)),
        Err(_) => s.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Minimal store-only ZIP writer
// ---------------------------------------------------------------------------

/// Public entry point over the internal store-only ZIP writer.
///
/// Sibling exporters (e.g. [`crate::helpers::search_exporter`]) reuse this to
/// pack their own `(name, bytes)` payloads — and the combined research + search
/// bundle — into the *same* archive format without duplicating the ZIP
/// machinery below. Blocking disk I/O.
pub fn write_store_zip(path: &Path, entries: &[(&str, &[u8])]) -> Result<(), String> {
    write_zip(path, entries)
}

/// Write `entries` (name, bytes) to `path` as a store-only (method 0) ZIP.
///
/// Each entry is stored uncompressed with its CRC-32; a central directory and
/// end-of-central-directory record follow. Atomic-ish: writes the whole archive
/// in one `std::fs::write`.
fn write_zip(path: &Path, entries: &[(&str, &[u8])]) -> Result<(), String> {
    let (dos_time, dos_date) = dos_datetime_now();
    let mut out: Vec<u8> = Vec::new();
    let mut central: Vec<u8> = Vec::new();

    for (name, data) in entries {
        let name_bytes = name.as_bytes();
        let crc = crc32(data);
        let size = data.len() as u32;
        let offset = out.len() as u32;

        // ── Local file header ──
        push_u32(&mut out, 0x0403_4b50);
        push_u16(&mut out, 20); // version needed to extract
        push_u16(&mut out, 0); // general-purpose bit flag
        push_u16(&mut out, 0); // compression method: 0 = stored
        push_u16(&mut out, dos_time);
        push_u16(&mut out, dos_date);
        push_u32(&mut out, crc);
        push_u32(&mut out, size); // compressed size
        push_u32(&mut out, size); // uncompressed size
        push_u16(&mut out, name_bytes.len() as u16);
        push_u16(&mut out, 0); // extra field length
        out.extend_from_slice(name_bytes);
        out.extend_from_slice(data);

        // ── Central directory header ──
        push_u32(&mut central, 0x0201_4b50);
        push_u16(&mut central, 20); // version made by
        push_u16(&mut central, 20); // version needed to extract
        push_u16(&mut central, 0); // general-purpose bit flag
        push_u16(&mut central, 0); // compression method
        push_u16(&mut central, dos_time);
        push_u16(&mut central, dos_date);
        push_u32(&mut central, crc);
        push_u32(&mut central, size); // compressed size
        push_u32(&mut central, size); // uncompressed size
        push_u16(&mut central, name_bytes.len() as u16);
        push_u16(&mut central, 0); // extra field length
        push_u16(&mut central, 0); // file comment length
        push_u16(&mut central, 0); // disk number start
        push_u16(&mut central, 0); // internal file attributes
        push_u32(&mut central, 0); // external file attributes
        push_u32(&mut central, offset); // relative offset of local header
        central.extend_from_slice(name_bytes);
    }

    let central_offset = out.len() as u32;
    let central_size = central.len() as u32;
    out.extend_from_slice(&central);

    // ── End of central directory record ──
    push_u32(&mut out, 0x0605_4b50);
    push_u16(&mut out, 0); // number of this disk
    push_u16(&mut out, 0); // disk where central directory starts
    push_u16(&mut out, entries.len() as u16); // central dir records on this disk
    push_u16(&mut out, entries.len() as u16); // total central dir records
    push_u32(&mut out, central_size);
    push_u32(&mut out, central_offset);
    push_u16(&mut out, 0); // ZIP file comment length

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, &out).map_err(|e| e.to_string())
}

#[inline]
fn push_u16(buf: &mut Vec<u8>, v: u16) {
    buf.extend_from_slice(&v.to_le_bytes());
}

#[inline]
fn push_u32(buf: &mut Vec<u8>, v: u32) {
    buf.extend_from_slice(&v.to_le_bytes());
}

/// Standard IEEE CRC-32 (bit-by-bit, polynomial 0xEDB88320).
fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// Current local time packed into MS-DOS (time, date) fields for the ZIP header.
fn dos_datetime_now() -> (u16, u16) {
    let now = chrono::Local::now();
    let year = now.year();
    let dos_year = if year < 1980 { 0u16 } else { (year - 1980) as u16 };
    let date = (dos_year << 9) | ((now.month() as u16) << 5) | (now.day() as u16);
    let time =
        ((now.hour() as u16) << 11) | ((now.minute() as u16) << 5) | ((now.second() as u16) / 2);
    (time, date)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn obs(pub_id: &str, target: &str, collection: &str, obs_id: &str) -> DownloadedObservation {
        DownloadedObservation {
            id: pub_id.to_string(),
            publisher_id: pub_id.to_string(),
            collection: collection.to_string(),
            observation_id: obs_id.to_string(),
            target_name: target.to_string(),
            instrument: "MegaCam".into(),
            filter: "g".into(),
            ra: "10.6".into(),
            dec: "41.2".into(),
            start_date: "2020-01-01".into(),
            cal_level: "2".into(),
            local_path: String::new(),
            file_size: 0,
            downloaded_at: "2024-01-01T00:00:00Z".into(),
            thumbnail_url: String::new(),
            preview_url: String::new(),
            local_preview_path: String::new(),
            agent_attribution: None,
        }
    }

    fn note(pub_id: &str, rating: u8, text: &str, tags: &[&str]) -> ObservationNote {
        ObservationNote {
            publisher_id: pub_id.to_string(),
            rating,
            note: text.to_string(),
            tags: tags.iter().map(|s| s.to_string()).collect(),
            updated: "2024-02-02T12:00:00Z".to_string(),
            agent_attribution: None,
        }
    }

    fn fixed_now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-07-07T08:30:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn build_bundle_counts_and_json() {
        let observations = vec![
            obs("ivo://x?1", "M31", "CFHT", "obs-1"),
            obs("ivo://x?2", "", "CFHT", "obs-2"),
        ];
        let notes = vec![note("ivo://x?1", 4, "Nice galaxy", &["galaxy"])];
        let bundle = build_bundle(&observations, &notes, fixed_now());

        assert_eq!(bundle.observation_count, 2);
        assert_eq!(bundle.note_count, 1);
        // JSON is valid and round-trips the publisher id.
        assert!(bundle.observations_json.contains("ivo://x?1"));
        assert!(bundle.notes_json.contains("Nice galaxy"));
        let _: serde_json::Value = serde_json::from_str(&bundle.observations_json).unwrap();
        let _: serde_json::Value = serde_json::from_str(&bundle.notes_json).unwrap();
    }

    #[test]
    fn markdown_only_includes_observations_with_notes() {
        let observations = vec![
            obs("ivo://x?1", "M31", "CFHT", "obs-1"),
            obs("ivo://x?2", "NGC 5194", "CFHT", "obs-2"),
        ];
        // Only the second observation has a note.
        let notes = vec![note("ivo://x?2", 5, "Whirlpool", &["spiral", "deep"])];
        let md = build_bundle(&observations, &notes, fixed_now()).notes_md;

        assert!(md.contains("Exported 2026-07-07T08:30:00Z"));
        assert!(md.contains("1 of 2 observations have notes"));
        // Section for the noted observation only.
        assert!(md.contains("## NGC 5194 — CFHT obs-2"));
        assert!(!md.contains("## M31"));
        // Rating stars + label, tags, and note body.
        assert!(md.contains("★★★★★ (Excellent)"));
        assert!(md.contains("`spiral`, `deep`"));
        assert!(md.contains("### Notes"));
        assert!(md.contains("Whirlpool"));
    }

    #[test]
    fn markdown_empty_when_no_notes() {
        let observations = vec![obs("ivo://x?1", "M31", "CFHT", "obs-1")];
        let md = build_bundle(&observations, &[], fixed_now()).notes_md;
        assert!(md.contains("0 of 1 observations have notes"));
        assert!(md.contains("_No notes have been written yet._"));
    }

    #[test]
    fn stars_render_filled_and_empty() {
        assert_eq!(stars(0), "☆☆☆☆☆");
        assert_eq!(stars(3), "★★★☆☆");
        assert_eq!(stars(5), "★★★★★");
        // Clamps above 5.
        assert_eq!(stars(9), "★★★★★");
    }

    #[test]
    fn crc32_known_vector() {
        // The canonical CRC-32 test vector.
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn write_zip_produces_readable_archive() {
        let observations = vec![obs("ivo://x?1", "M31", "CFHT", "obs-1")];
        let notes = vec![note("ivo://x?1", 3, "Fair seeing", &["queue"])];

        let path = std::env::temp_dir().join(format!(
            "verbinal_export_test_{}_{}.zip",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));

        let count = write_bundle_zip(&path, &observations, &notes).unwrap();
        assert_eq!(count, 1);

        let bytes = std::fs::read(&path).unwrap();
        // Local file header magic "PK\x03\x04".
        assert_eq!(&bytes[0..4], &[0x50, 0x4b, 0x03, 0x04]);
        // End-of-central-directory magic "PK\x05\x06" appears near the tail.
        assert!(bytes.windows(4).any(|w| w == [0x50, 0x4b, 0x05, 0x06]));
        // All three member names are present in the raw archive.
        let haystack = bytes.as_slice();
        for name in ["observations.json", "notes.json", "notes.md"] {
            assert!(
                haystack.windows(name.len()).any(|w| w == name.as_bytes()),
                "archive missing {name}"
            );
        }

        let _ = std::fs::remove_file(&path);
    }
}
