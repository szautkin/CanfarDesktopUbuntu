//! CAOM2 / DataLink / VizieR read-tool family.
//!
//! Ported from `Mcp/Tools/Read/Caom2ReadTools.cs` (`get_observation_caom2` +
//! `get_data_links`) and `Mcp/Tools/Read/VizierConeSearchTool.cs`
//! (`vizier_cone_search`). Every tool here is a pure READ, `agent_safe: true`:
//!
//! * `get_observation_caom2` — the CAOM2 metadata document (collection, target,
//!   proposal, telescope, instrument, planes) for one publisher id, fetched via
//!   the shared [`CAOM2Service`].
//! * `get_data_links` — the downloadable DataLink artifacts (url / semantics /
//!   content-type / size, HTTPS-only) for one publisher id, via
//!   `services.datalink`.
//! * `vizier_cone_search` — a public VizieR cone search (no auth) via
//!   [`VizierService`], with mirror failover.
//!
//! This family owns no write tools, so [`apply`] always returns `None` (kept for
//! contract-parity with the service-backed families).

use crate::mcp::tools::proposals::{InMemoryProposalStore, PendingProposal};
use crate::mcp::tools::{ToolDescriptor, ToolResult, VerbClass};
use crate::models::caom2::{CAOM2Observation, Caom2Plane};
use crate::services::caom2_service::{CAOM2Service, Caom2Status};
use crate::services::vizier_service::VizierService;
use crate::state::AppServices;
use serde_json::{json, Value};
use std::sync::Arc;

// ─────────────────────────────────────────────────────────────────────────────
// VizieR argument defaults / caps (mirror the reference tool)
// ─────────────────────────────────────────────────────────────────────────────

/// Default VizieR catalogue when the caller omits `catalog` — Gaia DR3, the
/// most common all-sky cross-match reference.
const DEFAULT_CATALOG: &str = "I/355/gaiadr3";
const DEFAULT_RA_COLUMN: &str = "RAJ2000";
const DEFAULT_DEC_COLUMN: &str = "DEJ2000";
const DEFAULT_MAX_REC: i64 = 500;
const MAX_REC_CAP: i64 = 5000;

// ─────────────────────────────────────────────────────────────────────────────
// Manifest
// ─────────────────────────────────────────────────────────────────────────────

/// Descriptors advertised for the CAOM2 / DataLink / VizieR family. All reads,
/// all agent-safe.
pub fn descriptors() -> Vec<ToolDescriptor> {
    vec![
        ToolDescriptor {
            name: "get_observation_caom2".to_string(),
            description: "Get the CAOM2 metadata (collection, target, proposal, telescope, \
                instrument, planes) for one observation by its CADC publisher id \
                (e.g. ivo://cadc.nrc.ca/CFHT?...)."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "publisher_id": {
                        "type": "string",
                        "description": "Observation publisher id / DID."
                    }
                },
                "required": ["publisher_id"],
                "additionalProperties": false
            }),
            verb: VerbClass::Read,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "get_data_links".to_string(),
            description: "Get the downloadable DataLink artifacts (access url, semantics, \
                content-type, byte size) for one observation by its publisher id. Only HTTPS \
                links are returned."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "publisher_id": {
                        "type": "string",
                        "description": "Observation publisher id / DID."
                    }
                },
                "required": ["publisher_id"],
                "additionalProperties": false
            }),
            verb: VerbClass::Read,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "vizier_cone_search".to_string(),
            description: "Cone-search a VizieR catalogue at CDS (public, no auth). `ra`/`dec` are \
                in degrees (ICRS), `radius_deg` is the cone radius in degrees. `catalog` is the \
                VizieR identifier exactly (e.g. I/355/gaiadr3 — the default — V/97/catalog, \
                B/vsx/vsx). Override `ra_column`/`dec_column` if the catalogue uses names other \
                than RAJ2000/DEJ2000. Returns parsed rows plus a `probablyTruncated` hint when the \
                row count hit `max_rec`."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "ra": { "type": "number", "description": "Cone-centre RA in degrees (ICRS)." },
                    "dec": { "type": "number", "description": "Cone-centre Dec in degrees (ICRS)." },
                    "radius_deg": {
                        "type": "number",
                        "minimum": 0,
                        "description": "Cone radius in degrees."
                    },
                    "catalog": {
                        "type": "string",
                        "description": "VizieR catalogue identifier. Default: I/355/gaiadr3 (Gaia DR3)."
                    },
                    "ra_column": {
                        "type": "string",
                        "description": "Override the RA column name. Default: RAJ2000."
                    },
                    "dec_column": {
                        "type": "string",
                        "description": "Override the Dec column name. Default: DEJ2000."
                    },
                    "max_rec": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": MAX_REC_CAP,
                        "description": "Row cap; default 500."
                    }
                },
                "required": ["ra", "dec", "radius_deg"],
                "additionalProperties": false
            }),
            verb: VerbClass::Read,
            agent_safe: true,
        },
    ]
}

// ─────────────────────────────────────────────────────────────────────────────
// Dispatch
// ─────────────────────────────────────────────────────────────────────────────

/// Handle a call for this family. Returns `Some(..)` if `name` is one of the
/// family's tools (so the router stops chaining), or `None` otherwise.
pub async fn dispatch(
    name: &str,
    services: &AppServices,
    args: &Value,
    _proposals: &Arc<InMemoryProposalStore>,
) -> Option<ToolResult> {
    let result = match name {
        "get_observation_caom2" => get_observation_caom2(services, args).await,
        "get_data_links" => get_data_links(services, args).await,
        "vizier_cone_search" => vizier_cone_search(args).await,
        _ => return None,
    };
    Some(result)
}

async fn get_observation_caom2(services: &AppServices, args: &Value) -> ToolResult {
    let pid = str_arg(args, "publisher_id");
    if pid.is_empty() {
        return ToolResult::Failed("publisher_id is required".to_string());
    }

    let service = CAOM2Service::new(reqwest::Client::new(), services.endpoints.clone());
    let token = services.get_token().await;
    let result = service.get_by_publisher_id(token.as_deref(), &pid).await;

    match result.status {
        Caom2Status::Success => match result.observation {
            Some(obs) => ToolResult::Data(observation_to_json(&obs)),
            None => ToolResult::Failed("CAOM2 fetch reported success but returned no document.".to_string()),
        },
        Caom2Status::AuthRequired => ToolResult::Failed(
            result
                .error
                .unwrap_or_else(|| "This observation requires CADC sign-in.".to_string()),
        ),
        Caom2Status::NotFound => {
            ToolResult::Failed(format!("No observation found for publisher id '{pid}'."))
        }
        Caom2Status::InvalidId => ToolResult::Failed(
            result
                .error
                .unwrap_or_else(|| format!("Invalid publisher id: {pid}")),
        ),
        Caom2Status::Parse | Caom2Status::ServerError => ToolResult::Failed(
            result
                .error
                .unwrap_or_else(|| "CAOM2 metadata fetch failed.".to_string()),
        ),
    }
}

async fn get_data_links(services: &AppServices, args: &Value) -> ToolResult {
    let pid = str_arg(args, "publisher_id");
    if pid.is_empty() {
        return ToolResult::Failed("publisher_id is required".to_string());
    }

    let token = services.get_token().await;
    match services.datalink.resolve(&pid, token.as_deref()).await {
        Ok(res) => {
            // The service already filters access_url to HTTPS-only, so every
            // surviving file is safe to advertise.
            let files: Vec<Value> = res
                .files
                .iter()
                .map(|f| {
                    json!({
                        "url": f.url,
                        "semantics": f.semantics,
                        "contentType": f.content_type,
                        "sizeBytes": f.size,
                        "description": f.description,
                        "filename": f.filename(),
                    })
                })
                .collect();
            ToolResult::Data(json!({
                "publisherId": res.publisher_id,
                "downloadUrl": res.download_url,
                "fileCount": files.len(),
                "files": files,
            }))
        }
        Err(e) => ToolResult::Failed(format!("DataLink resolve failed for '{pid}': {e}")),
    }
}

async fn vizier_cone_search(args: &Value) -> ToolResult {
    let (ra, dec, radius_deg) =
        match (num_arg(args, "ra"), num_arg(args, "dec"), num_arg(args, "radius_deg")) {
            (Some(ra), Some(dec), Some(r)) => (ra, dec, r),
            _ => return ToolResult::Failed("ra, dec and radius_deg are required numbers".to_string()),
        };
    if radius_deg < 0.0 {
        return ToolResult::Failed("radius_deg must be >= 0".to_string());
    }

    let max_rec = args.get("max_rec").and_then(Value::as_i64).unwrap_or(DEFAULT_MAX_REC);
    if !(1..=MAX_REC_CAP).contains(&max_rec) {
        return ToolResult::Failed(format!("max_rec must be between 1 and {MAX_REC_CAP}"));
    }

    let catalog = opt_str_arg(args, "catalog").unwrap_or_else(|| DEFAULT_CATALOG.to_string());
    let ra_column = opt_str_arg(args, "ra_column").unwrap_or_else(|| DEFAULT_RA_COLUMN.to_string());
    let dec_column =
        opt_str_arg(args, "dec_column").unwrap_or_else(|| DEFAULT_DEC_COLUMN.to_string());

    let service = VizierService::new(reqwest::Client::new());
    match service
        .cone_search(
            &catalog,
            ra,
            dec,
            radius_deg,
            &ra_column,
            &dec_column,
            max_rec as usize,
        )
        .await
    {
        Ok(res) => {
            let row_count = res.rows.len();
            // Hitting the cap means the server likely had more matches.
            let probably_truncated = row_count as i64 >= max_rec;
            ToolResult::Data(json!({
                "catalog": catalog,
                "headers": res.headers,
                "rows": res.rows,
                "rowCount": row_count,
                "probablyTruncated": probably_truncated,
            }))
        }
        Err(e) => ToolResult::Failed(e),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Apply — this family is read-only, so it never owns a proposal.
// ─────────────────────────────────────────────────────────────────────────────

/// Read-only family: never owns an approved proposal, so always `None`.
pub async fn apply(
    _services: &AppServices,
    _proposal: &PendingProposal,
) -> Option<Result<String, String>> {
    None
}

// ─────────────────────────────────────────────────────────────────────────────
// CAOM2 → JSON
// ─────────────────────────────────────────────────────────────────────────────

/// Serialize a [`CAOM2Observation`] to a compact JSON document (mirrors the C#
/// `Caom2ObservationSummary`, but keeps the nested plane/artifact detail so an
/// agent can pick a specific data product).
fn observation_to_json(o: &CAOM2Observation) -> Value {
    let proposal = o.proposal.as_ref().map(|p| {
        json!({
            "id": p.id,
            "pi": p.pi,
            "project": p.project,
            "title": p.title,
            "keywords": p.keywords,
        })
    });
    let target = o.target.as_ref().map(|t| {
        json!({
            "name": t.name,
            "type": t.kind,
            "standard": t.standard,
            "redshift": t.redshift,
            "moving": t.moving,
            "keywords": t.keywords,
        })
    });
    let telescope = o.telescope.as_ref().map(|t| {
        json!({
            "name": t.name,
            "geoLocation": t.geo_location.map(|(x, y, z)| json!([x, y, z])),
            "keywords": t.keywords,
        })
    });
    let instrument = o.instrument.as_ref().map(|i| {
        json!({ "name": i.name, "keywords": i.keywords })
    });
    let environment = o.environment.as_ref().map(|e| {
        json!({
            "seeing": e.seeing,
            "humidity": e.humidity,
            "elevation": e.elevation,
            "tau": e.tau,
        })
    });
    let planes: Vec<Value> = o.planes.iter().map(plane_to_json).collect();

    json!({
        "collection": o.collection,
        "observationId": o.observation_id,
        "observationType": o.observation_type,
        "intent": o.intent,
        "sequenceNumber": o.sequence_number,
        "algorithm": o.algorithm,
        "proposal": proposal,
        "target": target,
        "telescope": telescope,
        "instrument": instrument,
        "environment": environment,
        "planeCount": planes.len(),
        "planes": planes,
    })
}

fn plane_to_json(p: &Caom2Plane) -> Value {
    let artifacts: Vec<Value> = p
        .artifacts
        .iter()
        .map(|a| {
            json!({
                "uri": a.uri,
                "productType": a.product_type,
                "contentType": a.content_type,
                "contentLength": a.content_length,
            })
        })
        .collect();
    json!({
        "productId": p.product_id,
        "calibrationLevel": p.calibration_level,
        "dataProductType": p.data_product_type,
        "quality": p.quality,
        "positionBounds": p
            .position_bounds
            .iter()
            .map(|(ra, dec)| json!([ra, dec]))
            .collect::<Vec<_>>(),
        "energyLowerMeters": p.energy_lower,
        "energyUpperMeters": p.energy_upper,
        "timeLowerMjd": p.time_lower,
        "timeUpperMjd": p.time_upper,
        "artifactCount": artifacts.len(),
        "artifacts": artifacts,
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Arg helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Extract a trimmed required string (empty if missing / not a string).
fn str_arg(args: &Value, key: &str) -> String {
    args.get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string()
}

/// Extract a trimmed optional string; `None` if missing, non-string, or blank.
fn opt_str_arg(args: &Value, key: &str) -> Option<String> {
    let s = args.get(key).and_then(Value::as_str)?.trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Extract a numeric argument (accepts JSON number, or a numeric string).
fn num_arg(args: &Value, key: &str) -> Option<f64> {
    let v = args.get(key)?;
    v.as_f64().or_else(|| v.as_str().and_then(|s| s.trim().parse::<f64>().ok()))
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::caom2::{Caom2Artifact, Caom2Proposal, Caom2Target};
    use std::collections::HashSet;

    #[test]
    fn descriptor_names_unique_read_and_agent_safe() {
        let ds = descriptors();
        assert_eq!(ds.len(), 3);
        let mut seen = HashSet::new();
        for d in &ds {
            assert!(!d.name.is_empty());
            assert!(!d.description.is_empty(), "{} needs a description", d.name);
            assert_eq!(d.verb, VerbClass::Read, "{} must be a read", d.name);
            assert!(d.agent_safe, "{} must be agent_safe", d.name);
            assert!(seen.insert(d.name.clone()), "duplicate: {}", d.name);
        }
    }

    #[test]
    fn vizier_schema_requires_position() {
        let d = descriptors()
            .into_iter()
            .find(|d| d.name == "vizier_cone_search")
            .unwrap();
        let required = d.input_schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "ra"));
        assert!(required.iter().any(|v| v == "dec"));
        assert!(required.iter().any(|v| v == "radius_deg"));
    }

    #[test]
    fn num_arg_accepts_number_and_numeric_string() {
        assert_eq!(num_arg(&json!({ "ra": 10.5 }), "ra"), Some(10.5));
        assert_eq!(num_arg(&json!({ "ra": "41.2" }), "ra"), Some(41.2));
        assert_eq!(num_arg(&json!({ "ra": "nope" }), "ra"), None);
        assert_eq!(num_arg(&json!({}), "ra"), None);
    }

    #[test]
    fn opt_str_arg_blanks_to_none() {
        assert_eq!(opt_str_arg(&json!({ "c": "  V/97  " }), "c"), Some("V/97".to_string()));
        assert_eq!(opt_str_arg(&json!({ "c": "   " }), "c"), None);
        assert_eq!(opt_str_arg(&json!({}), "c"), None);
    }

    #[test]
    fn observation_json_includes_nested_detail() {
        let obs = CAOM2Observation {
            collection: "CFHT".into(),
            observation_id: "obs-9".into(),
            observation_type: Some("OBJECT".into()),
            intent: Some("science".into()),
            proposal: Some(Caom2Proposal {
                id: Some("P1".into()),
                pi: Some("Alice".into()),
                ..Default::default()
            }),
            target: Some(Caom2Target {
                name: Some("M31".into()),
                redshift: Some(0.001),
                ..Default::default()
            }),
            planes: vec![Caom2Plane {
                product_id: "prod-1".into(),
                calibration_level: Some(2),
                data_product_type: Some("cube".into()),
                artifacts: vec![Caom2Artifact {
                    uri: "cadc:CFHT/x.fits".into(),
                    product_type: Some("science".into()),
                    content_length: Some(2048),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        let v = observation_to_json(&obs);
        assert_eq!(v["collection"], "CFHT");
        assert_eq!(v["target"]["name"], "M31");
        assert_eq!(v["proposal"]["pi"], "Alice");
        assert_eq!(v["planeCount"], 1);
        assert_eq!(v["planes"][0]["productId"], "prod-1");
        assert_eq!(v["planes"][0]["artifactCount"], 1);
        assert_eq!(v["planes"][0]["artifacts"][0]["contentLength"], 2048);
    }
}
