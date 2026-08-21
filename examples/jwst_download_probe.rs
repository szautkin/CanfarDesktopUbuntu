//! Does our own transfer code write a 0-byte file for a JWST artifact?
//!
//! Report: `download_observation` returns 0-byte files for JWST planes, while
//! the same CADC-indexed artifacts fetch fine from their public mirror. Every
//! URL involved returns real bytes to curl unauthenticated — the science image
//! is 46 MB, the package tar 58 MB — so if a 0-byte file appears it is ours.
//!
//!     cargo run --example jwst_download_probe -- <url> [more urls...]
use verbinal::services::notification_service::ToastNotifier;
use verbinal::services::transfer::{download_with_progress, Cancel};

fn main() {
    let urls: Vec<String> = std::env::args().skip(1).collect();
    if urls.is_empty() {
        eprintln!("usage: jwst_download_probe <url>...");
        std::process::exit(2);
    }

    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(async {
        let (toast, _rx) = ToastNotifier::new();
        let dir = std::env::temp_dir().join(format!("verbinal-jwst-probe-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");

        let mut bad = 0;
        for url in &urls {
            let name = url.rsplit('/').next().unwrap_or("unknown");
            let dest = dir.join(name);
            let started = std::time::Instant::now();
            match download_with_progress(url, None, &dest, &toast, name, None, &Cancel::never())
                .await
            {
                Ok(reported) => {
                    let on_disk = std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
                    let ok = on_disk > 0 && on_disk == reported;
                    if !ok {
                        bad += 1;
                    }
                    println!(
                        "{:<52} reported={:>10}  on-disk={:>10}  {:?}  {}",
                        name,
                        reported,
                        on_disk,
                        started.elapsed(),
                        if ok { "ok" } else { "MISMATCH / EMPTY" }
                    );
                }
                Err(e) => {
                    bad += 1;
                    println!("{name:<52} ERROR {e}");
                }
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
        println!(
            "\n{}",
            if bad == 0 {
                "PASS: every artifact landed with its bytes"
            } else {
                "FAIL"
            }
        );
        std::process::exit(if bad == 0 { 0 } else { 1 });
    });
}
