//! Does a publisher id that resolves to nothing still "succeed"?
//!
//! Reported: `download_observation` writes 0-byte files for JWST planes and the
//! job reports "Downloaded obs-….fits (0 bytes)" as succeeded. Measured cause:
//! `caom2ops/pkg` answers HTTP 200 with an empty body and no content type for
//! an id it cannot resolve, so the status check passes and the empty file lands.
//!
//!     cargo run --example empty_download_probe
use verbinal::services::observation_download::download_observation;
use verbinal::state::AppServices;

fn main() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let handle = rt.handle().clone();
    let _guard = rt.enter();

    rt.block_on(async move {
        let (services, _rx) = AppServices::new(handle.clone());

        // The id from the bug report: no `mirror/` segment, no observation path
        // step. DataLink rejects it; the package endpoint returns nothing.
        let bad = "ivo://cadc.nrc.ca/JWST?jw03435-o002_t001_miri_f2100w-PRODUCT";

        // The same observation under its REAL publisher id, from argus. This
        // must still work: the fix must refuse what is broken without refusing
        // what is not.
        let good = "ivo://cadc.nrc.ca/mirror/JWST?jw03435-o002_t001_miri_f1000w/jw03435-o002_t001_miri_f1000w-PRODUCT";
        match download_observation(&services, good, None, "probe", None).await {
            Ok(o) => println!(
                "valid id:   OK  {} ({} bytes)",
                o.local_path.file_name().unwrap_or_default().to_string_lossy(),
                o.file_size
            ),
            Err(e) => {
                println!("valid id:   FAILED — {e}");
                println!("\nFAIL: the fix broke a working download");
                std::process::exit(1);
            }
        }

        match download_observation(&services, bad, None, "probe", None).await {
            Ok(outcome) => {
                println!(
                    "returned Ok: {} ({} bytes)",
                    outcome.local_path.display(),
                    outcome.file_size
                );
                let on_disk = std::fs::metadata(&outcome.local_path)
                    .map(|m| m.len())
                    .unwrap_or(0);
                println!("on disk: {on_disk} bytes");
                println!("\nFAIL: an unresolvable id reported success");
                std::process::exit(1);
            }
            Err(e) => {
                println!("refused, with: {e}\n");
                let names_the_shape = e.contains("mirror/JWST");
                let says_empty = e.to_lowercase().contains("empty");
                println!(
                    "{}",
                    if names_the_shape && says_empty {
                        "PASS: it fails, says the body was empty, and shows a valid id shape"
                    } else {
                        "FAIL: it fails, but the message is not actionable"
                    }
                );
                std::process::exit(if names_the_shape && says_empty { 0 } else { 1 });
            }
        }
    });
}
