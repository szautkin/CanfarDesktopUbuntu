//! What `get_service_health` now reports, against the live services.
//!
//! Reported three times: 400/400/401 all shown as `ok: true`. The cause was the
//! probe, not the label — it GET the WORKING endpoints, and a bare GET on a TAP
//! sync endpoint is a malformed request that a healthy service answers 400.
//!
//!     cargo run --example health_probe_check
use verbinal::config::{ApiEndpoints, AppConfig};
use verbinal::services::probe_core;

fn main() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(async {
        let endpoints = ApiEndpoints::new(AppConfig::default());
        let client = reqwest::Client::new();
        let results = probe_core(&client, &endpoints).await;

        let mut odd = 0;
        for r in &results {
            let status = r.status.map(|s| s.to_string()).unwrap_or("-".into());
            println!(
                "{:<24} {:<4} ok={:<5} reachable={:<5} {}",
                r.name,
                status,
                r.ok,
                r.reachable,
                r.url.rsplit('/').next().unwrap_or("")
            );
            // The complaint: a 4xx sitting beside ok=true.
            if r.ok && r.status.map(|s| s >= 400).unwrap_or(false) {
                odd += 1;
            }
        }
        println!(
            "\n{}",
            if odd == 0 {
                "PASS: no service reports ok=true on a 4xx"
            } else {
                "FAIL: a 4xx is still reported as ok"
            }
        );
        std::process::exit(if odd == 0 { 0 } else { 1 });
    });
}
