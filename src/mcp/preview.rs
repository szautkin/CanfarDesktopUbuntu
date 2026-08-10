//! Server-side preview-image fetcher for the MCP `get_preview_image` tool.
//!
//! Resolves a CADC observation's DataLink set, picks an image `#preview` row
//! (HTTPS only), and fetches it with an authenticated (bearer), redirect-
//! following, size-bounded GET — returning the raw bytes plus the resolved MIME
//! type for the tool arm to wrap as an inline image. Kept self-contained so this
//! HTTP-heavy logic is isolated and independently testable, mirroring the
//! reference `McpPreviewFetcher` / `GetPreviewImageTool` on Windows.

use crate::models::search_result::DataLinkFile;
use crate::state::AppServices;

/// Resolve and fetch an observation's preview image server-side.
///
/// Returns `(image_bytes, mime)` on success. The fetch is authenticated with the
/// user's CADC bearer token (stripped by reqwest on the cross-host redirect to
/// pre-signed storage), follows redirects, and refuses any body that grows past
/// `max_bytes` — both via the declared `Content-Length` and while streaming.
pub async fn fetch_observation_preview(
    services: &AppServices,
    publisher_id: &str,
    max_bytes: usize,
) -> Result<(Vec<u8>, String), String> {
    let token = services.get_token().await;

    // 1. Resolve the observation's DataLink set.
    let links = services
        .datalink
        .resolve(publisher_id, token.as_deref())
        .await
        .map_err(|e| e.to_string())?;

    // 2. Pick an image `#preview` row (HTTPS only); never a science frame.
    let preview =
        pick_preview(&links.files).ok_or_else(|| format!("no preview image for {publisher_id}"))?;
    let url = preview.url.clone();
    let declared_type = preview.content_type.clone();

    // 3. Authenticated, redirect-following, size-bounded GET.
    fetch_bounded(&url, token.as_deref(), max_bytes, &declared_type).await
}

/// First image `#preview` DataLink row served over HTTPS.
///
/// `is_preview()` already requires `semantics == "#preview"` *and* an
/// image content-type; the extra HTTPS guard is defence-in-depth against a
/// downgrade / SSRF even though `parse_votable` already drops non-HTTPS rows.
fn pick_preview(files: &[DataLinkFile]) -> Option<&DataLinkFile> {
    files.iter().find(|f| f.is_preview() && is_https(&f.url))
}

fn is_https(url: &str) -> bool {
    url.trim().to_ascii_lowercase().starts_with("https://")
}

/// Content type from the response header (parameters stripped), falling back to
/// the DataLink-declared type when the header is absent or empty.
fn parse_mime(header: Option<&str>, fallback: &str) -> String {
    header
        .map(|s| s.split(';').next().unwrap_or(s).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

/// Authenticated, redirect-following, size-bounded image GET.
async fn fetch_bounded(
    url: &str,
    token: Option<&str>,
    max_bytes: usize,
    declared_type: &str,
) -> Result<(Vec<u8>, String), String> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(10))
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;

    // The bearer rides only to the CADC host; reqwest strips Authorization on the
    // cross-host redirect to pre-signed storage (which needs no token).
    let mut req = client.get(url);
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    let mut resp = req.send().await.map_err(|e| e.to_string())?;

    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(format!(
            "authentication required to fetch preview (HTTP {})",
            status.as_u16()
        ));
    }
    if !status.is_success() {
        return Err(format!("HTTP {} fetching preview", status.as_u16()));
    }

    // Refuse before transferring if the server declares an oversize body.
    if let Some(len) = resp.content_length() {
        if len > max_bytes as u64 {
            return Err(format!(
                "preview too large: declared {len} bytes exceeds cap of {max_bytes}"
            ));
        }
    }

    // Resolve MIME before the mutable streaming borrow of `resp` begins.
    let mime = {
        let header = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok());
        parse_mime(header, declared_type)
    };

    // Stream the body, stopping as soon as it grows past the cap.
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
        buf.extend_from_slice(&chunk);
        if buf.len() > max_bytes {
            return Err(format!(
                "preview too large: body exceeds cap of {max_bytes} bytes"
            ));
        }
    }

    Ok((buf, mime))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::search_result::DataLinkFile;

    fn file(url: &str, semantics: &str, content_type: &str) -> DataLinkFile {
        DataLinkFile {
            url: url.into(),
            semantics: semantics.into(),
            content_type: content_type.into(),
            size: None,
            description: String::new(),
        }
    }

    #[test]
    fn picks_first_https_image_preview() {
        let files = vec![
            file("https://x/sci.fits", "#this", "application/fits"),
            file("http://x/p.jpg", "#preview", "image/jpeg"), // non-HTTPS -> rejected
            file("https://x/p.png", "#preview", "image/png"),
        ];
        assert_eq!(pick_preview(&files).unwrap().url, "https://x/p.png");
    }

    #[test]
    fn rejects_non_image_preview() {
        // #preview but not an image content-type -> not selectable.
        let files = vec![file("https://x/p.fits", "#preview", "application/fits")];
        assert!(pick_preview(&files).is_none());
    }

    #[test]
    fn none_when_no_preview_row() {
        let files = vec![file("https://x/sci.fits", "#this", "application/fits")];
        assert!(pick_preview(&files).is_none());
    }

    #[test]
    fn https_guard_is_case_insensitive() {
        assert!(is_https("https://a"));
        assert!(is_https("HTTPS://a"));
        assert!(is_https("  https://a"));
        assert!(!is_https("http://a"));
        assert!(!is_https("ftp://a"));
    }

    #[test]
    fn mime_prefers_header_and_strips_params() {
        assert_eq!(
            parse_mime(Some("image/png; charset=binary"), "image/jpeg"),
            "image/png"
        );
        assert_eq!(parse_mime(Some("image/webp"), "image/jpeg"), "image/webp");
        assert_eq!(parse_mime(None, "image/jpeg"), "image/jpeg");
        assert_eq!(parse_mime(Some(""), "image/jpeg"), "image/jpeg");
        assert_eq!(parse_mime(Some("   "), "image/jpeg"), "image/jpeg");
    }
}
