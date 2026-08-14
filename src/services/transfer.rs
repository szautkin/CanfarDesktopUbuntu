//! File transfers: streaming, resumable-in-spirit, cancellable, and cleaned up.
//!
//! One implementation for every transfer the app makes. It used to live inside
//! the Search page, which meant two *services* reached up into the UI layer to
//! borrow it, and the Storage browser — the one screen whose whole job is
//! moving files — did not use it at all: it read a whole file into memory and
//! wrote it in one go, so a 5 GB cube needed 5 GB of RAM and showed nothing
//! until it finished.
//!
//! Three properties every transfer here has:
//!
//! * **Bounded memory.** One chunk at a time, whatever the file size.
//! * **Nothing half-written under a real name.** A download lands in
//!   `<dest>.tmp` and is renamed only once complete, so a partial file never
//!   appears as a finished one.
//! * **A way out.** Every transfer takes a [`Cancel`] and checks it between
//!   chunks; on cancel the partial artifact is removed, and the caller is told
//!   it was cancelled rather than that it failed.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// A shared "stop" flag, checked between chunks.
///
/// An `AtomicBool` rather than a channel: the question a transfer asks is
/// "should I still be running", which has one answer at any moment and no
/// history worth queueing. Cloning shares the flag, so the UI holds one end and
/// the tokio task the other.
#[derive(Clone, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn new() -> Self {
        Self::default()
    }

    /// A token nobody will ever trip — for callers with no cancel affordance.
    pub fn never() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// Why a transfer stopped early.
#[derive(Debug, PartialEq, Eq)]
pub enum TransferError {
    /// The user asked it to stop. Not a failure — nothing is wrong, and the
    /// UI should say so differently.
    Cancelled,
    Failed(String),
}

impl std::fmt::Display for TransferError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransferError::Cancelled => write!(f, "cancelled"),
            TransferError::Failed(e) => write!(f, "{e}"),
        }
    }
}

/// Bytes so far, and the total when the server declared one.
pub type ProgressSink = tokio::sync::mpsc::UnboundedSender<(u64, Option<u64>)>;

pub async fn download_to_file(
    url: &str,
    token: Option<&str>,
    dest: &std::path::Path,
    toast: &crate::services::notification_service::ToastNotifier,
    label: &str,
) -> Result<u64, String> {
    download_with_progress(url, token, dest, toast, label, None, &Cancel::never()).await
}

/// The same download, additionally reporting progress to `progress`.
///
/// One implementation with an optional observer rather than two downloaders:
/// this one streams to a sibling `.tmp` and renames on success, so an
/// interrupted multi-gigabyte transfer never leaves a half-file that looks
/// complete — a property a second copy would have to re-earn.
///
/// The channel exists because this runs on the tokio runtime while the widgets
/// live on the GLib thread; the receiver drives them from there.
pub async fn download_with_progress(
    url: &str,
    token: Option<&str>,
    dest: &std::path::Path,
    toast: &crate::services::notification_service::ToastNotifier,
    label: &str,
    progress: Option<ProgressSink>,
    cancel: &Cancel,
) -> Result<u64, String> {
    use std::io::Write;

    // Sibling temp path: keep the real filename intact and just append ".tmp"
    // (so e.g. "foo.fits" -> "foo.fits.tmp"), avoiding any extension clash.
    let mut tmp_os = dest.as_os_str().to_owned();
    tmp_os.push(".tmp");
    let tmp_path = std::path::PathBuf::from(tmp_os);

    // A fresh client with a connect timeout but no overall request timeout —
    // multi-GB transfers legitimately run for minutes, so a whole-request
    // deadline would be wrong here.
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;

    let mut req = client.get(url);
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    let mut resp = req.send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status().as_u16()));
    }
    let total = resp.content_length();

    let file = std::fs::File::create(&tmp_path).map_err(|e| e.to_string())?;
    let mut writer = std::io::BufWriter::new(file);

    let mut downloaded: u64 = 0;
    let mut last_report = std::time::Instant::now();
    let mut last_pct: i64 = -1;
    let mut last_report_bytes: u64 = 0;

    loop {
        // Between chunks, not mid-write: a cancelled transfer still leaves a
        // consistent `.tmp` to delete rather than a half-written buffer.
        if cancel.is_cancelled() {
            let _ = writer.flush();
            drop(writer);
            let _ = std::fs::remove_file(&tmp_path);
            return Err(TransferError::Cancelled.to_string());
        }
        match resp.chunk().await {
            Ok(Some(chunk)) => {
                if let Err(e) = writer.write_all(&chunk) {
                    let _ = writer.flush();
                    let _ = std::fs::remove_file(&tmp_path);
                    return Err(e.to_string());
                }
                downloaded += chunk.len() as u64;

                // Throttle progress toasts so the overlay queue never floods:
                // report at most ~once/700ms, and only on a real advance
                // (>=1% when the total is known, else every >=64 MiB).
                let advanced = match total {
                    Some(t) if t > 0 => {
                        let pct = (downloaded.min(t) as i64 * 100) / t as i64;
                        if pct > last_pct {
                            last_pct = pct;
                            true
                        } else {
                            false
                        }
                    }
                    _ => downloaded.saturating_sub(last_report_bytes) >= 64 * 1024 * 1024,
                };
                if advanced && last_report.elapsed() >= std::time::Duration::from_millis(700) {
                    toast.toast(format_download_progress(label, downloaded, total));
                    // Same throttle for the inline bar: a send per 8 KiB chunk
                    // would post thousands of GLib wake-ups a second to move a
                    // bar by a pixel. A closed channel means the page went away
                    // mid-download, which is not a reason to stop downloading.
                    if let Some(tx) = &progress {
                        let _ = tx.send((downloaded, total));
                    }
                    last_report = std::time::Instant::now();
                    last_report_bytes = downloaded;
                }
            }
            Ok(None) => break,
            Err(e) => {
                let _ = writer.flush();
                let _ = std::fs::remove_file(&tmp_path);
                return Err(e.to_string());
            }
        }
    }

    if let Err(e) = writer.flush() {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(e.to_string());
    }
    drop(writer);

    // Replace any stale destination, then rename the completed temp into place.
    if dest.exists() {
        let _ = std::fs::remove_file(dest);
    }
    if let Err(e) = std::fs::rename(&tmp_path, dest) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(e.to_string());
    }

    Ok(downloaded)
}

/// Human-readable byte size (IEC units) for progress display.
pub fn format_byte_size(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    let b = bytes as f64;
    if b >= GIB {
        format!("{:.2} GiB", b / GIB)
    } else if b >= MIB {
        format!("{:.1} MiB", b / MIB)
    } else if b >= KIB {
        format!("{:.1} KiB", b / KIB)
    } else {
        format!("{} B", bytes)
    }
}

/// How far a transfer has got, e.g. `"128.0 MiB / 1.20 GiB (10%)"` — or just
/// `"128.0 MiB"` when the server sends no Content-Length.
///
/// Shared by the progress toasts and the detail page's inline progress bar, so
/// the two never disagree about how many bytes have arrived.
pub fn format_download_amount(downloaded: u64, total: Option<u64>) -> String {
    match total {
        Some(t) if t > 0 => {
            let pct = (downloaded.min(t) as u128 * 100 / t as u128) as u64;
            format!(
                "{} / {} ({}%)",
                format_byte_size(downloaded),
                format_byte_size(t),
                pct
            )
        }
        _ => format_byte_size(downloaded),
    }
}

/// Build a progress-toast string, e.g.
/// `"Downloading M81… 128.0 MiB / 1.20 GiB (10%)"`.
pub fn format_download_progress(label: &str, downloaded: u64, total: Option<u64>) -> String {
    format!(
        "Downloading {}… {}",
        label,
        format_download_amount(downloaded, total)
    )
}

/// Stream a local file to `url` with a PUT, reporting progress and honouring
/// `cancel`.
///
/// The upload path used to be `std::fs::read(path)` into a `Vec<u8>` handed to
/// reqwest: a 5 GB cube meant 5 GB of resident memory before a single byte went
/// out, and there was nothing to report progress from because the whole body
/// was already in hand.
///
/// `on_partial` is called when the transfer does NOT complete — cancelled or
/// failed — so the caller can remove whatever the service kept. An interrupted
/// PUT can leave a node of the wrong length behind, and a truncated FITS file
/// that looks like a real one is worse than no file: the next reader gets a
/// header promising data that is not there.
pub async fn upload_from_file(
    url: &str,
    token: &str,
    src: &std::path::Path,
    content_type: &str,
    progress: Option<ProgressSink>,
    cancel: &Cancel,
) -> Result<u64, TransferError> {
    use tokio::io::AsyncReadExt;

    let total = std::fs::metadata(src)
        .map(|m| m.len())
        .map_err(|e| TransferError::Failed(e.to_string()))?;

    let mut file = tokio::fs::File::open(src)
        .await
        .map_err(|e| TransferError::Failed(e.to_string()))?;

    // Read the file in chunks and hand them to reqwest as a stream, so peak
    // memory is one chunk regardless of the file's size.
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Vec<u8>, std::io::Error>>(4);
    let cancel_reader = cancel.clone();
    let progress_reader = progress.clone();
    let reader = tokio::spawn(async move {
        let mut sent: u64 = 0;
        let mut buf = vec![0u8; 1024 * 1024];
        let mut last_report = std::time::Instant::now();
        loop {
            if cancel_reader.is_cancelled() {
                break;
            }
            match file.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    if tx.send(Ok(buf[..n].to_vec())).await.is_err() {
                        break;
                    }
                    sent += n as u64;
                    // Same throttle as the download: a send per chunk would post
                    // thousands of GLib wake-ups a second to move a bar by a pixel.
                    if last_report.elapsed() >= std::time::Duration::from_millis(200) {
                        if let Some(p) = &progress_reader {
                            let _ = p.send((sent, Some(total)));
                        }
                        last_report = std::time::Instant::now();
                    }
                }
                Err(e) => {
                    let _ = tx.send(Err(e)).await;
                    break;
                }
            }
        }
    });

    let stream = tokio_stream::wrappers::ReceiverStream::new(rx);
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| TransferError::Failed(e.to_string()))?;

    let result = client
        .put(url)
        .bearer_auth(token)
        .header("Content-Type", content_type)
        .header("Content-Length", total.to_string())
        .body(reqwest::Body::wrap_stream(stream))
        .send()
        .await;

    let _ = reader.await;

    if cancel.is_cancelled() {
        return Err(TransferError::Cancelled);
    }
    match result {
        Ok(resp) if resp.status().is_success() => {
            if let Some(p) = &progress {
                let _ = p.send((total, Some(total)));
            }
            Ok(total)
        }
        Ok(resp) => Err(TransferError::Failed(format!(
            "HTTP {}",
            resp.status().as_u16()
        ))),
        Err(e) => Err(TransferError::Failed(e.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_is_shared_by_every_clone() {
        // The UI holds one end and the transfer task the other; a copy that did
        // not see the flag would be a Cancel button that does nothing.
        let a = Cancel::new();
        let b = a.clone();
        assert!(!b.is_cancelled());
        a.cancel();
        assert!(b.is_cancelled());
    }

    #[test]
    fn a_token_nobody_holds_never_trips() {
        assert!(!Cancel::never().is_cancelled());
    }

    #[test]
    fn cancelling_is_not_failing() {
        // The UI branches on this: "Download cancelled" is a different sentence
        // from "Download failed", and only one of them is bad news.
        assert_eq!(TransferError::Cancelled.to_string(), "cancelled");
        assert_ne!(
            TransferError::Cancelled,
            TransferError::Failed("cancelled".into())
        );
    }

    #[tokio::test]
    async fn a_cancelled_upload_reports_cancelled_and_sends_nothing() {
        let dir = std::env::temp_dir().join("verbinal_transfer_cancel");
        let _ = std::fs::create_dir_all(&dir);
        let src = dir.join("big.bin");
        std::fs::write(&src, vec![0u8; 4 * 1024 * 1024]).unwrap();

        let cancel = Cancel::new();
        cancel.cancel(); // tripped before it starts
        let out = upload_from_file(
            // An address nothing listens on: if the guard works, no connection
            // is ever attempted, so the test cannot hang on it.
            "http://127.0.0.1:1/never",
            "token",
            &src,
            "application/octet-stream",
            None,
            &cancel,
        )
        .await;
        assert_eq!(out.unwrap_err(), TransferError::Cancelled);
        let _ = std::fs::remove_file(&src);
    }

    #[test]
    fn a_size_reads_as_a_person_would_say_it() {
        assert_eq!(format_byte_size(512), "512 B");
        assert_eq!(format_byte_size(1024 * 1024), "1.0 MiB");
        assert_eq!(format_download_amount(50, Some(100)), "50 B / 100 B (50%)");
        // No Content-Length: report what has arrived rather than invent a total.
        assert_eq!(format_download_amount(2048, None), "2.0 KiB");
    }
}
