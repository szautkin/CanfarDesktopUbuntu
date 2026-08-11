//! The one place an observation is resolved and fetched into the Research library.
//!
//! Port of `Services/ObservationDownloadService.cs`. Before this existed the
//! resolve → pick-artifact → stream → register sequence was written out three
//! separate times (the Research page's re-download, the Search page's
//! save-to-research, and the observation detail page), each with its own idea of
//! which DataLink row counts as "the science file" and where the bytes should
//! land. Three answers to one question is three chances to be wrong, so new
//! callers — starting with the `download_observation` MCP applier — come here.
//!
//! Downloads are STREAMED to a sibling `.tmp` and renamed on success, so an
//! interrupted multi-GB cube never leaves a half-file that looks complete.

use crate::services::observation_store::{managed_dir_for, DownloadedObservation};
use crate::state::AppServices;
use std::path::PathBuf;

/// What a completed download produced.
pub struct DownloadOutcome {
    pub local_path: PathBuf,
    pub file_size: u64,
    /// The artifact actually fetched, for the caller's summary line.
    pub filename: String,
}

/// Resolve `publisher_id` through DataLink and fetch one artifact into the
/// observation's managed directory.
///
/// `artifact_index` picks a SPECIFIC product from the resolved link set (a moment
/// map, an integrated spectrum) instead of the default science file; it is
/// bounds-checked against the resolved list, because an out-of-range index that
/// silently fell back to the primary artifact would hand the caller a different
/// file than it asked for without saying so.
pub async fn download_observation(
    services: &AppServices,
    publisher_id: &str,
    artifact_index: Option<usize>,
    label: &str,
) -> Result<DownloadOutcome, String> {
    let publisher_id = publisher_id.trim();
    if publisher_id.is_empty() {
        return Err("publisherId is required".to_string());
    }

    let token = services.get_token().await;
    let resolved = services
        .datalink
        .resolve(publisher_id, token.as_deref())
        .await;

    // Pick the artifact: an explicit index addresses the resolved list; otherwise
    // the #this science row, and failing that the synthesised package URL.
    let (url, filename) = match (&resolved, artifact_index) {
        (Ok(dl), Some(index)) => {
            let file = dl.files.get(index).ok_or_else(|| {
                format!(
                    "artifactIndex {index} is out of range — this observation resolved {} artifact(s)",
                    dl.files.len()
                )
            })?;
            (file.url.clone(), Some(file.filename()))
        }
        (Ok(dl), None) => match dl.files.iter().find(|f| f.is_science_data()) {
            Some(f) => (f.url.clone(), Some(f.filename())),
            None => (
                dl.download_url
                    .clone()
                    .unwrap_or_else(|| services.datalink.download_url(publisher_id)),
                None,
            ),
        },
        // A DataLink failure is not fatal on its own: the package endpoint is a
        // valid fallback. But it IS fatal when a specific artifact was asked for,
        // since the fallback cannot honour that request.
        (Err(e), Some(_)) => {
            return Err(format!(
                "cannot select artifactIndex — DataLink did not resolve: {e}"
            ))
        }
        (Err(_), None) => (services.datalink.download_url(publisher_id), None),
    };

    let id = crate::helpers::caom2_uri::uuid_from_publisher_id(publisher_id);
    let filename = filename.unwrap_or_else(|| format!("{id}.fits"));
    let dir = managed_dir_for(&id);
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let dest = dir.join(&filename);

    let file_size = crate::ui::search_page::stream_download_to_file(
        &url,
        token.as_deref(),
        &dest,
        &services.toast,
        label,
    )
    .await?;

    Ok(DownloadOutcome {
        local_path: dest,
        file_size,
        filename,
    })
}

/// Fetch an observation and record it in the Research library.
///
/// Registration happens only after the bytes are on disk, so a failed transfer
/// never leaves a library entry pointing at a file that isn't there.
pub async fn download_and_register(
    services: &AppServices,
    publisher_id: &str,
    artifact_index: Option<usize>,
) -> Result<String, String> {
    let outcome =
        download_observation(services, publisher_id, artifact_index, publisher_id).await?;

    let id = crate::helpers::caom2_uri::uuid_from_publisher_id(publisher_id);
    let existing = services.observation_store.load();
    // Preserve everything already known about this observation (target, preview
    // URLs, instrument) — an agent-initiated download must not blank metadata a
    // previous search filled in.
    let mut record = existing
        .into_iter()
        .find(|o| o.publisher_id == publisher_id)
        .unwrap_or_else(|| DownloadedObservation {
            id: id.clone(),
            publisher_id: publisher_id.to_string(),
            ..Default::default()
        });
    record.local_path = outcome.local_path.display().to_string();
    record.file_size = outcome.file_size;
    record.downloaded_at = chrono::Utc::now().to_rfc3339();

    services.observation_store.save(record)?;
    Ok(format!(
        "Downloaded {} ({} bytes)",
        outcome.filename, outcome.file_size
    ))
}
