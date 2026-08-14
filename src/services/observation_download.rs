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
    /// DataLink preview / thumbnail URLs resolved along the way, recorded so the
    /// caller can store them without a second round trip.
    pub preview_url: String,
    pub thumbnail_url: String,
    /// Where the preview image was cached on disk, when one was available.
    ///
    /// The Research page renders previews from a LOCAL file and never touches
    /// the network, so a URL alone is not enough: without this an
    /// agent-downloaded observation showed the "legacy record — re-save from the
    /// Search page" banner, which is both wrong and unactionable for a record
    /// that was just created.
    pub local_preview_path: String,
}

/// File extension for a cached preview, from its content type.
fn preview_extension(content_type: &str) -> &'static str {
    let lower = content_type.to_lowercase();
    if lower.contains("jpeg") || lower.contains("jpg") {
        "jpg"
    } else if lower.contains("png") {
        "png"
    } else if lower.contains("gif") {
        "gif"
    } else if lower.contains("webp") {
        "webp"
    } else {
        "bin"
    }
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

    // Pick the artifact: an explicit index addresses the SCIENCE files, in the
    // order `get_data_links` reports them under `directFiles`; otherwise the
    // first science row, and failing that the synthesised package URL.
    //
    // Indexing `direct_files()` rather than the raw row list is what keeps the
    // two tools honest — a preview or thumbnail row ahead of the science data
    // would otherwise shift every index the agent was given.
    let (url, filename) = match (&resolved, artifact_index) {
        (Ok(dl), Some(index)) => {
            let direct = dl.direct_files();
            let file = direct.get(index).ok_or_else(|| {
                format!(
                    "artifactIndex {index} is out of range — this observation resolved {} science artifact(s)",
                    direct.len()
                )
            })?;
            (file.url.clone(), Some(file.filename()))
        }
        (Ok(dl), None) => match dl.direct_files().first() {
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

    let file_size = crate::services::transfer::download_to_file(
        &url,
        token.as_deref(),
        &dest,
        &services.toast,
        label,
    )
    .await?;

    // Cache the preview alongside the data, best effort. The Research page shows
    // previews from disk and never touches the network, so skipping this leaves
    // an agent-fetched observation looking like a legacy record with no image.
    // A failure here is not a failed download — the science file is already
    // safely on disk.
    let (preview_url, thumbnail_url) = match &resolved {
        Ok(dl) => (
            dl.preview_urls().first().cloned().unwrap_or_default(),
            dl.thumbnail_urls().first().cloned().unwrap_or_default(),
        ),
        Err(_) => (String::new(), String::new()),
    };
    let local_preview_path = cache_preview(services, &dir, &preview_url, &thumbnail_url).await;

    Ok(DownloadOutcome {
        local_path: dest,
        file_size,
        filename,
        preview_url,
        thumbnail_url,
        local_preview_path,
    })
}

/// Which image to cache: the full preview when there is one, else the thumbnail.
///
/// The detail pane renders it at 420x260, where a thumbnail looks soft — so the
/// larger image wins whenever it exists. `None` when the observation publishes
/// neither, which is "no preview", never an error.
fn preferred_preview_url<'a>(preview_url: &'a str, thumbnail_url: &'a str) -> Option<&'a str> {
    [preview_url, thumbnail_url]
        .into_iter()
        .find(|candidate| !candidate.is_empty())
}

/// Download the preview image into the observation's managed directory.
///
/// Prefers the full preview over the thumbnail — the detail pane renders it at
/// 420x260, where a thumbnail looks soft. Returns an empty string when there is
/// nothing to fetch or the fetch fails; the caller treats that as "no preview",
/// never as a failed download.
async fn cache_preview(
    services: &AppServices,
    dir: &std::path::Path,
    preview_url: &str,
    thumbnail_url: &str,
) -> String {
    let Some(url) = preferred_preview_url(preview_url, thumbnail_url) else {
        return String::new();
    };

    let token = services.get_token().await;
    let Ok(bytes) = services
        .datalink
        .download_image(url, token.as_deref())
        .await
    else {
        return String::new();
    };

    // The content type is not returned here, so infer from the URL and fall back
    // to the sniffed magic bytes — a mislabelled extension only affects the
    // filename, since the page loads by content.
    let extension = preview_extension(url);
    let path = dir.join(format!("preview.{extension}"));
    match std::fs::write(&path, &bytes) {
        Ok(()) => path.to_string_lossy().into_owned(),
        Err(_) => String::new(),
    }
}

/// Fetch an observation and record it in the Research library.
///
/// Registration happens only after the bytes are on disk, so a failed transfer
/// never leaves a library entry pointing at a file that isn't there.
pub async fn download_and_register(
    services: &AppServices,
    publisher_id: &str,
    artifact_index: Option<usize>,
    attribution: Option<crate::helpers::agent_attribution::AgentAttribution>,
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
    // Fill the preview only when this download actually resolved one, so a
    // re-download that failed to reach DataLink cannot blank a cached image an
    // earlier save captured.
    if !outcome.preview_url.is_empty() {
        record.preview_url = outcome.preview_url.clone();
    }
    if !outcome.thumbnail_url.is_empty() {
        record.thumbnail_url = outcome.thumbnail_url.clone();
    }
    if !outcome.local_preview_path.is_empty() {
        record.local_preview_path = outcome.local_preview_path.clone();
    }
    // Stamp WHO fetched it. An unstamped agent download is indistinguishable
    // from one the user made themselves — the badge in the Research list is the
    // only thing that tells them apart. `None` (a user-initiated download)
    // deliberately clears any previous stamp: the user has now taken ownership.
    //
    // Serialised as JSON so client / tool / timestamp all survive; the Research
    // page falls back to reading a bare label, but that loses the detail.
    record.agent_attribution = attribution
        .as_ref()
        .and_then(|a| serde_json::to_string(a).ok());

    services.observation_store.save(record)?;
    Ok(format!(
        "Downloaded {} ({} bytes)",
        outcome.filename, outcome.file_size
    ))
}

#[cfg(test)]
mod tests {
    use super::{preferred_preview_url, preview_extension};

    #[test]
    fn the_full_preview_beats_the_thumbnail() {
        // The detail pane shows it at 420x260; a thumbnail looks soft there.
        assert_eq!(
            preferred_preview_url("https://x/preview.png", "https://x/thumb.png"),
            Some("https://x/preview.png")
        );
    }

    #[test]
    fn a_thumbnail_is_used_when_there_is_no_preview() {
        assert_eq!(
            preferred_preview_url("", "https://x/thumb.png"),
            Some("https://x/thumb.png")
        );
    }

    #[test]
    fn neither_is_no_preview_not_an_error() {
        // Plenty of observations publish no image at all; that must not look
        // like a failed download.
        assert_eq!(preferred_preview_url("", ""), None);
    }

    #[test]
    fn the_cached_file_keeps_the_image_type_in_its_name() {
        assert_eq!(preview_extension("https://x/preview.png"), "png");
        assert_eq!(preview_extension("https://x/preview.JPG"), "jpg");
        assert_eq!(preview_extension("image/jpeg"), "jpg");
        assert_eq!(preview_extension("https://x/anim.gif"), "gif");
        assert_eq!(preview_extension("https://x/pic.webp"), "webp");
    }

    #[test]
    fn an_unrecognised_type_still_produces_a_usable_filename() {
        // The page loads by content, not extension, so an unknown type is
        // stored rather than dropped.
        assert_eq!(preview_extension("https://x/preview"), "bin");
        assert_eq!(preview_extension(""), "bin");
    }
}
