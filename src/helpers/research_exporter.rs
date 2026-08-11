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
//!   that has a note (citation + rating + tags + body).
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
use crate::models::search_result::{RecentSearch, SavedQuery};
use crate::services::observation_store::DownloadedObservation;
use chrono::{DateTime, Datelike, Timelike, Utc};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
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

// ---------------------------------------------------------------------------
// Wrapped bundle: manifest.json + README.md + per-module subdirectories
//
// Port of `ExportService.BuildBundleAsync` + `ZipBundle`. Where the flat
// an earlier flat writer emitted three loose files, this assembles a proper,
// Claude-friendly bundle: a top-level `manifest.json` (machine index) and
// `README.md` (human/LLM guide) alongside a `research/` and `search/`
// subdirectory, all nested under a timestamped base folder inside the zip.
// ---------------------------------------------------------------------------

/// Everything a bundle build needs: the four data modules, what to include, and
/// the provenance stamp written into `manifest.json` / `README.md`.
///
/// A struct rather than eight-plus positional arguments — the four slices are
/// adjacent and same-shaped, so a transposed pair would compile and silently
/// export the wrong module.
#[derive(Clone, Copy)]
pub struct BundleRequest<'a> {
    pub observations: &'a [DownloadedObservation],
    pub notes: &'a [ObservationNote],
    pub saved: &'a [SavedQuery],
    pub recent: &'a [RecentSearch],
    pub options: BundleOptions,
    /// Export timestamp — names the bundle folder and stamps the manifest.
    pub now: DateTime<Utc>,
    pub app_version: &'a str,
    pub host_name: &'a str,
}

/// What a wrapped research bundle should contain. Mirrors the subset of the
/// Windows `ExportOptions` surfaced by the Research page's export dialog
/// (file copies are not offered on Linux).
#[derive(Debug, Clone, Copy)]
pub struct BundleOptions {
    /// Include `research/notes.json` + `research/notes.md`.
    pub include_notes: bool,
    /// Include `search/recent_searches.json` (recent-search history).
    pub include_search_history: bool,
}

/// Item counts + the bundle's base folder name, returned so the caller can toast
/// a summary without re-reading the archive.
#[derive(Debug, Clone)]
pub struct BundleSummary {
    pub bundle_name: String,
    pub observation_count: usize,
    pub note_count: usize,
    pub saved_count: usize,
    pub recent_count: usize,
}

const EXPORT_VERSION: &str = "1.0";
const APP_NAME: &str = "Verbinal";

/// VOSpace folder that finished bundles are published into, relative to the
/// user's home. Matches the reference's `Verbinal-Exports`.
pub const EXPORT_FOLDER: &str = "Verbinal-Exports";

/// The VOSpace path a bundle at `local_path` would be published to.
///
/// Separated from the upload itself so the destination can be shown, logged and
/// tested without a network.
pub fn remote_bundle_path(local_path: &Path) -> Option<String> {
    let filename = local_path.file_name()?.to_string_lossy();
    (!filename.is_empty()).then(|| format!("{EXPORT_FOLDER}/{filename}"))
}

/// Publish a written bundle to `Verbinal-Exports/<name>.zip` in the user's
/// VOSpace, returning the remote path.
///
/// One implementation for both callers — the Research page's Export button and
/// the `export_research_bundle` applier. They had separate copies of the folder
/// name, the idempotent create and the content type, which is three chances for
/// an agent-made bundle to land somewhere the user's own export would not.
///
/// Blocking file read; call from a blocking-tolerant context.
pub async fn upload_bundle(
    vospace: &crate::services::VoSpaceService,
    token: &str,
    username: &str,
    local_path: &Path,
) -> Result<String, String> {
    let remote = remote_bundle_path(local_path)
        .ok_or_else(|| format!("{} has no filename", local_path.display()))?;

    // The folder usually exists already — every export after the first — so a
    // failure here is not an error; the upload below is the real test.
    let _ = vospace.create_folder(token, username, EXPORT_FOLDER).await;

    let bytes = std::fs::read(local_path)
        .map_err(|e| format!("could not read {} back: {e}", local_path.display()))?;
    vospace
        .upload_file(token, username, &remote, bytes, "application/zip")
        .await
        .map_err(|e| format!("upload to {remote} failed: {e}"))?;
    Ok(remote)
}

/// Best-effort machine name for the bundle's provenance stamp.
///
/// Lives here rather than in a page because BOTH export paths — the Research
/// page and the MCP applier — stamp the manifest with it, and they must agree.
pub fn host_name() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| "unknown".to_string())
}

/// Timestamped base-folder name, e.g. `Verbinal-Export-2026-07-08_143000`.
/// Matches `ExportService.BundleName`.
pub fn bundle_name(now: DateTime<Utc>) -> String {
    format!("Verbinal-Export-{}", now.format("%Y-%m-%d_%H%M%S"))
}

// ── Manifest model (camelCase JSON, 1-to-1 with Models/Export/ExportManifest) ─

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ExportManifest {
    export_version: String,
    app_name: String,
    app_version: String,
    exported_at: String,
    host_name: String,
    modules: Vec<ExportManifestModule>,
    claude_hints: ExportClaudeHints,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ExportManifestModule {
    id: String,
    display_name: String,
    files: Vec<String>,
    item_counts: BTreeMap<String, usize>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ExportClaudeHints {
    primary_context: Option<String>,
    metadata_schema: Option<String>,
    read_me_first: String,
}

/// One module's assembled payload before it is placed into the bundle tree.
struct ModuleBuild {
    id: &'static str,
    display_name: &'static str,
    /// (module-relative filename, bytes), e.g. `("observations.json", …)`.
    files: Vec<(String, Vec<u8>)>,
    item_counts: BTreeMap<String, usize>,
}

/// The fully assembled bundle: zip entries (paths already nested under the
/// timestamped base folder) plus the summary counts.
pub struct WrappedBundle {
    /// (zip entry path, bytes) — e.g. `("Verbinal-Export-…/research/observations.json", …)`.
    pub entries: Vec<(String, Vec<u8>)>,
    pub summary: BundleSummary,
}

/// Assemble the wrapped bundle in memory. Pure — no I/O — so the manifest +
/// README rendering is unit-testable with fabricated data (mirrors the split
/// between `ExportService` and its inputs). `now` / `app_version` / `host_name`
/// are injected for deterministic tests.
pub fn build_wrapped_bundle(req: &BundleRequest) -> WrappedBundle {
    let BundleRequest {
        observations,
        notes,
        saved,
        recent,
        options,
        now,
        app_version,
        host_name,
    } = *req;
    let base = bundle_name(now);

    // ── Research module ─────────────────────────────────────────────────
    let research = build_bundle(observations, notes, now);
    let mut research_files: Vec<(String, Vec<u8>)> = vec![(
        "observations.json".to_string(),
        research.observations_json.into_bytes(),
    )];
    let mut research_counts: BTreeMap<String, usize> = BTreeMap::new();
    research_counts.insert("observations".to_string(), observations.len());
    if options.include_notes {
        research_files.push(("notes.json".to_string(), research.notes_json.into_bytes()));
        research_files.push(("notes.md".to_string(), research.notes_md.into_bytes()));
        research_counts.insert("notes".to_string(), notes.len());
    }

    // ── Search module (reuses the sibling search exporter's renderer) ────
    let search_raw = crate::helpers::search_exporter::build_search_bundle(
        saved,
        recent,
        options.include_search_history,
        now,
    );
    let search_files: Vec<(String, Vec<u8>)> = search_raw
        .into_iter()
        .map(|(name, content)| (name, content.into_bytes()))
        .collect();
    let mut search_counts: BTreeMap<String, usize> = BTreeMap::new();
    search_counts.insert("saved_queries".to_string(), saved.len());
    if options.include_search_history {
        search_counts.insert("recent_searches".to_string(), recent.len());
    }

    let modules = [
        ModuleBuild {
            id: "research",
            display_name: "Research",
            files: research_files,
            item_counts: research_counts,
        },
        ModuleBuild {
            id: "search",
            display_name: "Search",
            files: search_files,
            item_counts: search_counts,
        },
    ];

    // ── Manifest ────────────────────────────────────────────────────────
    let mut manifest_modules: Vec<ExportManifestModule> = Vec::new();
    let mut all_files: Vec<String> = Vec::new();
    for m in &modules {
        let mut rel: Vec<String> = m
            .files
            .iter()
            .map(|(name, _)| format!("{}/{}", m.id, name))
            .collect();
        rel.sort(); // stable, ordinal-ish order for the manifest
        all_files.extend(rel.iter().cloned());
        manifest_modules.push(ExportManifestModule {
            id: m.id.to_string(),
            display_name: m.display_name.to_string(),
            files: rel,
            item_counts: m.item_counts.clone(),
        });
    }

    let manifest = ExportManifest {
        export_version: EXPORT_VERSION.to_string(),
        app_name: APP_NAME.to_string(),
        app_version: app_version.to_string(),
        exported_at: iso_utc(now),
        host_name: host_name.to_string(),
        claude_hints: ExportClaudeHints {
            primary_context: all_files.iter().find(|f| f.ends_with(".md")).cloned(),
            metadata_schema: all_files.iter().find(|f| f.ends_with(".json")).cloned(),
            read_me_first: "README.md".to_string(),
        },
        modules: manifest_modules,
    };

    let manifest_json =
        serde_json::to_string_pretty(&manifest).unwrap_or_else(|_| "{}".to_string());
    let readme = render_readme(&manifest, now);

    // ── Assemble zip entries under the timestamped base folder ──────────
    let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
    entries.push((format!("{base}/manifest.json"), manifest_json.into_bytes()));
    entries.push((format!("{base}/README.md"), readme.into_bytes()));
    for m in modules {
        for (name, bytes) in m.files {
            entries.push((format!("{base}/{}/{name}", m.id), bytes));
        }
    }

    WrappedBundle {
        summary: BundleSummary {
            bundle_name: base,
            observation_count: observations.len(),
            note_count: notes.len(),
            saved_count: saved.len(),
            recent_count: if options.include_search_history {
                recent.len()
            } else {
                0
            },
        },
        entries,
    }
}

/// Build the wrapped bundle and write it to `path` as a store-only ZIP. Returns
/// the summary counts. Blocking disk I/O — call from a blocking thread.
pub fn write_research_bundle_zip(
    path: &Path,
    req: &BundleRequest,
) -> Result<BundleSummary, String> {
    let wrapped = build_wrapped_bundle(req);
    let entries: Vec<(&str, &[u8])> = wrapped
        .entries
        .iter()
        .map(|(name, bytes)| (name.as_str(), bytes.as_slice()))
        .collect();
    write_zip(path, &entries)?;
    Ok(wrapped.summary)
}

/// Render the bundle `README.md`. Port of `ExportService.RenderReadme`.
fn render_readme(m: &ExportManifest, now: DateTime<Utc>) -> String {
    // "Wednesday, 08 July 2026 14:30" — matches the reference "dddd, dd MMMM yyyy HH:mm".
    let date = now.format("%A, %d %B %Y %H:%M").to_string();
    let mut sb = String::new();
    sb.push_str(&format!("# Verbinal Export — {date}\n\n"));
    sb.push_str(&format!(
        "This bundle was exported from Verbinal v{} on `{}`.\n",
        m.app_version, m.host_name
    ));
    sb.push_str(
        "It is structured for consumption by Claude, other LLMs, and human collaborators.\n\n",
    );

    sb.push_str("## Contents\n\n");
    for module in &m.modules {
        let counts = module
            .item_counts
            .iter()
            .map(|(k, v)| format!("{v} {k}"))
            .collect::<Vec<_>>()
            .join(", ");
        sb.push_str(&format!(
            "- **{}** (`{}/`) — {}\n",
            module.display_name,
            module.id,
            if counts.is_empty() {
                "no items".to_string()
            } else {
                counts
            }
        ));
    }
    sb.push('\n');

    sb.push_str("## For Claude / LLM ingestion\n\n");
    if let Some(primary) = &m.claude_hints.primary_context {
        sb.push_str("1. Start with `manifest.json` to understand the bundle shape.\n");
        sb.push_str(&format!(
            "2. Read `{primary}` for human-readable per-item content.\n"
        ));
        if let Some(schema) = &m.claude_hints.metadata_schema {
            sb.push_str(&format!(
                "3. Cross-reference with `{schema}` for full metadata.\n\n"
            ));
        } else {
            sb.push('\n');
        }
    }

    sb.push_str("### Suggested prompts\n\n");
    sb.push_str("- *\"Summarize the data in this export, grouped by module.\"*\n");
    sb.push_str("- *\"Which items stand out as needing further investigation?\"*\n");
    sb.push_str("- *\"List everything tagged `calibration` across all modules.\"*\n\n");

    sb.push_str("## Data citation & provenance\n\n");
    sb.push_str(&format!(
        "- **Retrieved:** {date} — from CADC/CANFAR via Verbinal v{}.\n",
        m.app_version
    ));
    sb.push_str(
        "- **How to cite:** acknowledge the Canadian Astronomy Data Centre (CADC) and the ",
    );
    sb.push_str(
        "originating collection/telescope of each observation. Each downloaded observation in the ",
    );
    sb.push_str(
        "research module records its collection, instrument, calibration level, and data-release ",
    );
    sb.push_str(
        "date — cite the collection's standard reference and include the retrieval date above.\n",
    );
    sb.push_str(
        "- **No per-observation DOI:** CADC's CAOM2 metadata does not assign a DOI or bibcode to ",
    );
    sb.push_str(
        "individual observations. The closest citable handle is the originating **proposal** ",
    );
    sb.push_str("(id / PI / title), recorded per observation in `notes.md` where available. DOIs, when they ");
    sb.push_str(
        "exist, are assigned at the collection level — see the CADC collection page below.\n",
    );
    sb.push_str(
        "- **Reproducibility:** saved/recent searches keep the exact ADQL; re-running it against ",
    );
    sb.push_str(
        "CADC's TAP service reproduces the selection (name-resolver coordinates can drift between ",
    );
    sb.push_str(
        "services/epochs — `queries.md` freezes which resolver and epoch produced them).\n",
    );
    sb.push_str("- See https://www.cadc-ccda.hia-iha.nrc-cnrc.gc.ca/ for collection documentation and DOIs.\n\n");

    sb.push_str("## Privacy note\n\n");
    sb.push_str(
        "This bundle excludes all authentication tokens, Keychain entries, session state, ",
    );
    sb.push_str(
        "and cached credentials. Only user-authored data and public CADC metadata are exported.\n",
    );

    sb
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
        md.push_str(&format!(
            "- **Coordinates:** RA {}, Dec {}\n",
            obs.ra, obs.dec
        ));
    }
    if !obs.start_date.is_empty() {
        md.push_str(&format!("- **Start date:** {}\n", obs.start_date));
    }
    if !obs.cal_level.is_empty() {
        md.push_str(&format!("- **Calibration level:** {}\n", obs.cal_level));
    }
    // The citation handle. The "How to cite" section below names the proposal
    // as the closest citable identifier and says it is recorded here — for a
    // long time it was not, because the record model had no such fields.
    if !obs.proposal_id.is_empty() {
        md.push_str(&format!("- **Proposal:** {}\n", obs.proposal_id));
    }
    if !obs.proposal_pi.is_empty() {
        md.push_str(&format!("- **PI:** {}\n", obs.proposal_pi));
    }
    if !obs.proposal_title.is_empty() {
        md.push_str(&format!("- **Proposal title:** {}\n", obs.proposal_title));
    }
    if !obs.data_release.is_empty() {
        md.push_str(&format!(
            "- **Data release:** {}\n",
            iso_or_raw(&obs.data_release)
        ));
    }
    if !obs.downloaded_at.is_empty() {
        md.push_str(&format!(
            "- **Downloaded:** {}\n",
            iso_or_raw(&obs.downloaded_at)
        ));
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
        md.push_str(&format!(
            "- **Note modified:** {}\n",
            iso_or_raw(&note.updated)
        ));
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
    let dos_year = if year < 1980 {
        0u16
    } else {
        (year - 1980) as u16
    };
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
            proposal_id: "20AC99".into(),
            proposal_pi: "Doe".into(),
            proposal_title: "A survey of things".into(),
            data_release: "2022-01-01T00:00:00Z".into(),
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
    fn a_bundle_publishes_under_the_shared_export_folder() {
        // One destination for both callers. Two copies of this path meant an
        // agent-made bundle could land somewhere the user's own export would not.
        assert_eq!(
            remote_bundle_path(Path::new("/home/u/Verbinal-Export-2026-08-11_120000.zip")),
            Some("Verbinal-Exports/Verbinal-Export-2026-08-11_120000.zip".to_string())
        );
        assert!(remote_bundle_path(Path::new("/")).is_none());
    }

    #[test]
    fn the_remote_name_is_the_local_one() {
        // The user picked the filename in the save dialog; renaming it on the
        // way up would make the two copies hard to match by eye.
        let local = Path::new("/tmp/my-run.zip");
        let remote = remote_bundle_path(local).unwrap();
        assert!(remote.ends_with("my-run.zip"), "{remote}");
        assert!(remote.starts_with(EXPORT_FOLDER), "{remote}");
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
    fn the_bundle_delivers_the_citation_it_tells_the_user_to_make() {
        // notes.md's "How to cite" section names the proposal (id / PI / title)
        // as the closest citable handle and states it is recorded per
        // observation. It said that while the record model had no such fields,
        // so the bundle asked the user to cite something it never carried.
        let obs = vec![obs("ivo://x?1", "M31", "CFHT", "1234567p")];
        let notes = vec![note("ivo://x?1", 4, "Good seeing.", &["deep"])];
        let md = render_notes_markdown(&obs, &notes, fixed_now());

        assert!(md.contains("- **Proposal:** 20AC99"), "{md}");
        assert!(md.contains("- **PI:** Doe"), "{md}");
        assert!(
            md.contains("- **Proposal title:** A survey of things"),
            "{md}"
        );
        assert!(md.contains("- **Data release:**"), "{md}");
    }

    #[test]
    fn the_bundle_readme_points_at_a_citation_that_is_really_there() {
        // The README's "How to cite" guidance names the proposal as the closest
        // citable handle and says it is recorded per observation in notes.md.
        // Assert the promise and the payload together — they lived in separate
        // functions, which is how the guidance came to describe fields that did
        // not exist.
        let observations = vec![obs("ivo://x?1", "M31", "CFHT", "1234567p")];
        let notes = vec![note("ivo://x?1", 4, "Good seeing.", &["deep"])];
        let bundle = build_wrapped_bundle(&BundleRequest {
            observations: &observations,
            notes: &notes,
            saved: &[],
            recent: &[],
            options: BundleOptions {
                include_notes: true,
                include_search_history: false,
            },
            now: fixed_now(),
            app_version: "1.3.1",
            host_name: "test-host",
        });

        let text_of = |suffix: &str| -> String {
            let (_, bytes) = bundle
                .entries
                .iter()
                .find(|(path, _)| path.ends_with(suffix))
                .unwrap_or_else(|| panic!("the bundle should contain {suffix}"));
            String::from_utf8_lossy(bytes).into_owned()
        };

        let readme = text_of("README.md");
        assert!(readme.contains("citable handle"), "{readme}");
        assert!(readme.contains("notes.md"), "{readme}");

        let notes_md = text_of("research/notes.md");
        assert!(notes_md.contains("- **Proposal:** 20AC99"), "{notes_md}");
        assert!(notes_md.contains("- **PI:** Doe"), "{notes_md}");
    }

    #[test]
    fn a_record_without_citation_data_omits_the_block_rather_than_showing_blanks() {
        // Records saved before these fields existed load as empty strings. An
        // empty "- **PI:** " line would look like missing data in the archive.
        let mut bare = obs("ivo://x?2", "M51", "JWST", "jw001");
        bare.proposal_id = String::new();
        bare.proposal_pi = String::new();
        bare.proposal_title = String::new();
        bare.data_release = String::new();
        let notes = vec![note("ivo://x?2", 3, "Fine.", &[])];
        let md = render_notes_markdown(&[bare], &notes, fixed_now());

        assert!(!md.contains("**Proposal:**"), "{md}");
        assert!(!md.contains("**PI:**"), "{md}");
        assert!(!md.contains("**Data release:**"), "{md}");
        // The rest of the section is unaffected.
        assert!(md.contains("M51"), "{md}");
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

    fn req<'a>(
        observations: &'a [DownloadedObservation],
        notes: &'a [ObservationNote],
        options: BundleOptions,
        app_version: &'a str,
    ) -> BundleRequest<'a> {
        BundleRequest {
            observations,
            notes,
            saved: &[],
            recent: &[],
            options,
            now: fixed_now(),
            app_version,
            host_name: "test-host",
        }
    }

    fn opts(include_notes: bool, include_history: bool) -> BundleOptions {
        BundleOptions {
            include_notes,
            include_search_history: include_history,
        }
    }

    /// Find a wrapped-bundle entry whose path ends with `suffix`.
    fn entry<'a>(b: &'a WrappedBundle, suffix: &str) -> Option<&'a (String, Vec<u8>)> {
        b.entries.iter().find(|(name, _)| name.ends_with(suffix))
    }

    #[test]
    fn wrapped_bundle_lays_out_manifest_readme_and_modules() {
        let observations = vec![
            obs("ivo://x?1", "M31", "CFHT", "obs-1"),
            obs("ivo://x?2", "", "CFHT", "obs-2"),
        ];
        let notes = vec![note("ivo://x?1", 4, "Nice galaxy", &["galaxy"])];

        let b = build_wrapped_bundle(&req(&observations, &notes, opts(true, false), "1.2.3"));

        // Base folder name is timestamped, and every entry nests under it.
        let base = bundle_name(fixed_now());
        assert_eq!(base, "Verbinal-Export-2026-07-07_083000");
        assert!(b
            .entries
            .iter()
            .all(|(name, _)| name.starts_with(&format!("{base}/"))));

        // Top-level wrapper files exist.
        assert!(entry(&b, "/manifest.json").is_some());
        assert!(entry(&b, "/README.md").is_some());

        // Research module files (notes included).
        assert!(entry(&b, "research/observations.json").is_some());
        assert!(entry(&b, "research/notes.json").is_some());
        assert!(entry(&b, "research/notes.md").is_some());
        // Search module always contributes saved_queries.json + queries.md.
        assert!(entry(&b, "search/saved_queries.json").is_some());
        assert!(entry(&b, "search/queries.md").is_some());
        // History off → no recent_searches.json.
        assert!(entry(&b, "recent_searches.json").is_none());

        // Summary counts.
        assert_eq!(b.summary.observation_count, 2);
        assert_eq!(b.summary.note_count, 1);
        assert_eq!(b.summary.recent_count, 0);

        // Manifest is valid, camelCase JSON describing both modules.
        let manifest_bytes = &entry(&b, "/manifest.json").unwrap().1;
        let manifest: serde_json::Value = serde_json::from_slice(manifest_bytes).unwrap();
        assert_eq!(manifest["appName"], "Verbinal");
        assert_eq!(manifest["exportVersion"], "1.0");
        assert_eq!(manifest["appVersion"], "1.2.3");
        assert_eq!(manifest["hostName"], "test-host");
        assert_eq!(manifest["claudeHints"]["readMeFirst"], "README.md");
        // First .md across modules is research/notes.md (research module first).
        assert_eq!(
            manifest["claudeHints"]["primaryContext"],
            "research/notes.md"
        );
        // First .json in ordinal-sorted order is research/notes.json (< observations.json).
        assert_eq!(
            manifest["claudeHints"]["metadataSchema"],
            "research/notes.json"
        );

        let modules = manifest["modules"].as_array().unwrap();
        assert_eq!(modules.len(), 2);
        let research = &modules[0];
        assert_eq!(research["id"], "research");
        assert_eq!(research["displayName"], "Research");
        assert_eq!(research["itemCounts"]["observations"], 2);
        assert_eq!(research["itemCounts"]["notes"], 1);
        assert!(research["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f == "research/observations.json"));

        // README names both modules and cites CADC.
        let readme = String::from_utf8(entry(&b, "/README.md").unwrap().1.clone()).unwrap();
        assert!(readme.contains("# Verbinal Export"));
        assert!(readme.contains("**Research** (`research/`)"));
        assert!(readme.contains("**Search** (`search/`)"));
        assert!(readme.contains("2 observations"));
        assert!(readme.contains("Canadian Astronomy Data Centre"));
    }

    #[test]
    fn wrapped_bundle_omits_notes_when_disabled() {
        let observations = vec![obs("ivo://x?1", "M31", "CFHT", "obs-1")];
        let notes = vec![note("ivo://x?1", 4, "hidden", &["x"])];
        let b = build_wrapped_bundle(&req(&observations, &notes, opts(false, true), "9"));

        assert!(entry(&b, "research/observations.json").is_some());
        assert!(entry(&b, "research/notes.json").is_none());
        assert!(entry(&b, "research/notes.md").is_none());

        let manifest: serde_json::Value =
            serde_json::from_slice(&entry(&b, "/manifest.json").unwrap().1).unwrap();
        let research = &manifest["modules"][0];
        assert!(research["itemCounts"].get("notes").is_none());
        // Primary context falls back to the search module's markdown.
        assert_eq!(
            manifest["claudeHints"]["primaryContext"],
            "search/queries.md"
        );
    }

    #[test]
    fn write_research_bundle_zip_produces_wrapped_archive() {
        let observations = vec![obs("ivo://x?1", "M31", "CFHT", "obs-1")];
        let notes = vec![note("ivo://x?1", 3, "Fair", &["queue"])];

        let path = std::env::temp_dir().join(format!(
            "verbinal_wrapped_export_test_{}_{}.zip",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));

        let summary = write_research_bundle_zip(
            &path,
            &req(&observations, &notes, opts(true, true), "1.0.0"),
        )
        .unwrap();
        assert_eq!(summary.observation_count, 1);

        let bytes = std::fs::read(&path).unwrap();
        // Local file header magic "PK\x03\x04" …
        assert_eq!(&bytes[0..4], &[0x50, 0x4b, 0x03, 0x04]);
        // … and an end-of-central-directory record "PK\x05\x06" near the tail,
        // without which no unzip tool will open the archive at all.
        assert!(bytes.windows(4).any(|w| w == [0x50, 0x4b, 0x05, 0x06]));
        let haystack = bytes.as_slice();
        for name in [
            "manifest.json",
            "README.md",
            "research/observations.json",
            "search/saved_queries.json",
        ] {
            assert!(
                haystack.windows(name.len()).any(|w| w == name.as_bytes()),
                "archive missing {name}"
            );
        }

        let _ = std::fs::remove_file(&path);
    }
}
