//! What `describe_tap_schema` actually answers, against the live archive.
//!
//!     cargo run --example tap_schema_probe
use std::sync::Arc;
use verbinal::config::{ApiEndpoints, AppConfig};
use verbinal::services::tap_schema_service::TapSchemaService;
use verbinal::services::tap_service::TAPService;

fn main() {
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    rt.block_on(async {
        let endpoints = Arc::new(ApiEndpoints::new(AppConfig::default()));
        let tap = Arc::new(TAPService::new(reqwest::Client::new(), endpoints));
        let svc = TapSchemaService::new(tap);

        let t0 = std::time::Instant::now();
        let schema = match svc.schema().await {
            Ok(s) => s,
            Err(e) => {
                println!("FAILED: {e}");
                std::process::exit(1);
            }
        };
        println!(
            "first call:  {} tables, {} joins, {:?}",
            schema.tables.len(),
            schema.keys.len(),
            t0.elapsed()
        );

        let t1 = std::time::Instant::now();
        let again = svc.schema().await.expect("cached");
        println!(
            "second call: {:?} (cached: {})",
            t1.elapsed(),
            Arc::ptr_eq(&schema, &again)
        );

        for name in ["caom2.Observation", "caom2.Plane", "ivoa.ObsCore"] {
            match schema.table(name) {
                Some(t) => println!("  {name}: {} columns — {}", t.columns.len(), t.description),
                None => println!("  {name}: MISSING"),
            }
        }

        let plane = schema.table("caom2.Plane").expect("plane");
        println!("\nsample of what an agent gets for caom2.Plane:");
        for c in plane
            .columns
            .iter()
            .filter(|c| c.name.starts_with("energy_"))
            .take(4)
        {
            println!(
                "  {:<26} {:<8} {:<6} {}",
                c.name, c.datatype, c.unit, c.description
            );
        }
        println!("\njoins touching caom2.Plane:");
        for k in schema.keys_touching("caom2.Plane") {
            println!(
                "  {}.{} -> {}.{}  ({})",
                k.from_table, k.from_column, k.target_table, k.target_column, k.description
            );
        }
        std::process::exit(0);
    });
}
