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
use crate::mcp::tools::{num_arg, opt_str_arg, str_arg, ToolDescriptor, ToolResult, VerbClass};
use crate::models::caom2::{CAOM2Observation, Caom2Plane};
use crate::services::caom2_service::Caom2Status;
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
                    "publisherId": {
                        "type": "string",
                        "description": "Observation publisher id / DID."
                    }
                },
                "required": ["publisherId"],
                "additionalProperties": false
            }),
            verb: VerbClass::Read,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "get_data_links".to_string(),
            description: "Get the DataLink artifacts for one observation by its publisher id, split \
                into `directFiles` (the science products), `previews` and `thumbnails` (image URLs), \
                plus `downloadUrl` for the whole package. Each entry's 0-based position in \
                `directFiles` is its `artifactIndex` for download_observation — use it to fetch a \
                SPECIFIC product (e.g. the science cube, a moment map _mom0/1/2, or the integrated \
                spectrum _spec) instead of the default first one; previews and thumbnails are NOT \
                indexed. `otherFiles` appears only when the observation publishes artifacts outside \
                those three kinds. Only HTTPS links are returned."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "publisherId": {
                        "type": "string",
                        "description": "Observation publisher id / DID."
                    }
                },
                "required": ["publisherId"],
                "additionalProperties": false
            }),
            verb: VerbClass::Read,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "vizier_cone_search".to_string(),
            description: "Cone-search a VizieR catalogue at CDS. The standard pattern for catalogue \
                cross-matches against any of VizieR's holdings (Clement+2001 variables-in-globular- \
                clusters as V/97, OGLE, ASAS-SN, ZTF, …). Public, no auth. `catalogue` is the VizieR \
                identifier exactly (V/97/catalog, B/vsx/vsx, I/355/gaiadr3 — the default). \
                `radiusArcsec` is in ARCSECONDS for typical cluster work; the tool converts to \
                degrees internally. Position columns default to RAJ2000/DEJ2000 — override \
                `raColumn`/`decColumn` if the catalogue uses different names. Returns parsed rows \
                plus a `probablyTruncated` hint when the row count hit `maxRec`."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "raDeg": { "type": "number", "description": "Cone-centre RA in degrees (ICRS)." },
                    "decDeg": { "type": "number", "description": "Cone-centre Dec in degrees (ICRS)." },
                    "radiusArcsec": {
                        "type": "number",
                        "minimum": 0,
                        "description": "Cone radius in ARCSECONDS (not degrees)."
                    },
                    "catalogue": {
                        "type": "string",
                        "minLength": 1,
                        "description": "VizieR catalogue identifier, e.g. V/97/catalog. Default: I/355/gaiadr3 (Gaia DR3)."
                    },
                    "raColumn": {
                        "type": "string",
                        "description": "Override the RA column name. Default: RAJ2000."
                    },
                    "decColumn": {
                        "type": "string",
                        "description": "Override the Dec column name. Default: DEJ2000."
                    },
                    "maxRec": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": MAX_REC_CAP,
                        "description": "Row cap; default 500."
                    }
                },
                "required": ["catalogue", "raDeg", "decDeg", "radiusArcsec"],
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

    let service = &services.caom2;
    let token = services.get_token().await;
    let result = service.get_by_publisher_id(token.as_deref(), &pid).await;

    match result.status {
        Caom2Status::Success => match result.observation {
            Some(obs) => ToolResult::Data(observation_to_json(&obs)),
            None => ToolResult::Failed(
                "CAOM2 fetch reported success but returned no document.".to_string(),
            ),
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
            //
            // Split into the reference's three buckets. This is not cosmetic:
            // `artifactIndex` addresses `directFiles`, so if previews and
            // thumbnails shared that list — as they did when this returned one
            // flat `files` array — index 1 could name a thumbnail in one app and
            // a science frame in the other.
            let view = |f: &crate::models::search_result::DataLinkFile| {
                json!({
                    "url": f.url,
                    "contentType": f.content_type,
                    "description": f.description,
                    "filename": f.filename(),
                    // Beyond the reference's DataLinkFileView, and both free:
                    // the raw semantics tag, and the size an agent needs to
                    // decide whether a download is worth starting.
                    "semantics": f.semantics,
                    "sizeBytes": f.size,
                })
            };

            let direct: Vec<Value> = res.direct_files().into_iter().map(view).collect();
            let previews = res.preview_urls();
            let thumbnails = res.thumbnail_urls();
            let other: Vec<Value> = res.other_files().into_iter().map(view).collect();

            let mut payload = json!({
                "publisherId": res.publisher_id,
                "downloadUrl": res.download_url,
                "directFileCount": direct.len(),
                "directFiles": direct,
                "previewCount": previews.len(),
                "previews": previews,
                "thumbnailCount": thumbnails.len(),
                "thumbnails": thumbnails,
            });
            // Only surfaced when non-empty: the reference drops these rows, so
            // an always-present empty array would suggest a bucket that
            // normally has content.
            if !other.is_empty() {
                payload["otherFileCount"] = json!(other.len());
                payload["otherFiles"] = json!(other);
            }
            ToolResult::Data(payload)
        }
        Err(e) => ToolResult::Failed(format!("DataLink resolve failed for '{pid}': {e}")),
    }
}

/// Resolve the cone radius to DEGREES from whichever unit the caller supplied.
///
/// The reference takes `radiusArcsec`; Verbinal shipped `radiusDeg`. Reading one
/// as the other is not a naming annoyance but a 3600× error — an agent asking
/// for a 10-arcsecond cone would have searched 10 degrees of sky, hammering
/// VizieR and burying the intended match in an unrelated catalogue dump. Each
/// name is therefore read with its own unit and never used as a fallback for
/// the other's value.
fn cone_radius_deg(args: &Value) -> Result<f64, String> {
    let (radius, unit, name) = match num_arg(args, "radiusArcsec") {
        Some(arcsec) => (arcsec / 3600.0, "arcsec", "radiusArcsec"),
        None => match num_arg(args, "radiusDeg") {
            Some(deg) => (deg, "deg", "radiusDeg"),
            None => return Err("radiusArcsec is required (a number, in arcseconds)".to_string()),
        },
    };
    if radius < 0.0 {
        return Err(format!("{name} must be >= 0 {unit}"));
    }
    Ok(radius)
}

async fn vizier_cone_search(args: &Value) -> ToolResult {
    let (ra, dec) = match (
        num_arg(args, "raDeg").or_else(|| num_arg(args, "ra")),
        num_arg(args, "decDeg").or_else(|| num_arg(args, "dec")),
    ) {
        (Some(ra), Some(dec)) => (ra, dec),
        _ => return ToolResult::Failed("raDeg and decDeg are required numbers".to_string()),
    };

    let radius_deg = match cone_radius_deg(args) {
        Ok(r) => r,
        Err(e) => return ToolResult::Failed(e),
    };

    // `arg`, not a raw map lookup: the schema advertises `maxRec` and this read
    // `max_rec` straight off the object, so a caller asking for 5,000 rows —
    // spelled exactly as the tool documents — silently got the default. The
    // error names the advertised spelling too; refusing a value by a name the
    // caller never used is a second puzzle on top of the first.
    let max_rec = crate::mcp::tools::arg(args, "maxRec")
        .and_then(Value::as_i64)
        .unwrap_or(DEFAULT_MAX_REC);
    if !(1..=MAX_REC_CAP).contains(&max_rec) {
        return ToolResult::Failed(format!("maxRec must be between 1 and {MAX_REC_CAP}"));
    }

    let catalog = opt_str_arg(args, "catalogue")
        .or_else(|| opt_str_arg(args, "catalog"))
        .unwrap_or_else(|| DEFAULT_CATALOG.to_string());
    let ra_column = opt_str_arg(args, "raColumn").unwrap_or_else(|| DEFAULT_RA_COLUMN.to_string());
    let dec_column =
        opt_str_arg(args, "decColumn").unwrap_or_else(|| DEFAULT_DEC_COLUMN.to_string());

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
                "catalogue": catalog,
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
    let instrument = o
        .instrument
        .as_ref()
        .map(|i| json!({ "name": i.name, "keywords": i.keywords }));
    let environment = o.environment.as_ref().map(|e| {
        json!({
            "seeing": e.seeing,
            "humidity": e.humidity,
            "elevation": e.elevation,
            "tau": e.tau,
        })
    });
    let planes: Vec<Value> = o.planes.iter().map(plane_to_json).collect();

    // The reference's Caom2ObservationSummary is FLAT: proposalId, targetName,
    // telescopeName and friends sit at the top level. Ours groups them into
    // nested objects, which carries more (keywords, target.moving, the
    // telescope's geolocation, the whole environment block) but leaves an agent
    // reading `targetName` with nothing.
    //
    // Both are emitted. The flat scalars ARE the contract; the nested objects
    // are the detail the flat form cannot express. Each nested value is read
    // through the same Option chain that fills its flat twin, so the two can
    // never disagree.
    let proposal_field = |f: fn(&crate::models::caom2::Caom2Proposal) -> Option<String>| {
        o.proposal.as_ref().and_then(f)
    };

    json!({
        "collection": o.collection,
        "observationId": o.observation_id,
        "observationType": o.observation_type,
        "intent": o.intent,
        "algorithm": o.algorithm,
        "metaRelease": o.meta_release,
        "proposalId": proposal_field(|p| p.id.clone()),
        "proposalPi": proposal_field(|p| p.pi.clone()),
        "proposalProject": proposal_field(|p| p.project.clone()),
        "proposalTitle": proposal_field(|p| p.title.clone()),
        "targetName": o.target.as_ref().and_then(|t| t.name.clone()),
        "targetType": o.target.as_ref().and_then(|t| t.kind.clone()),
        "targetRedshift": o.target.as_ref().and_then(|t| t.redshift),
        "telescopeName": o.telescope.as_ref().and_then(|t| t.name.clone()),
        "instrumentName": o.instrument.as_ref().and_then(|i| i.name.clone()),
        "planeCount": planes.len(),
        "planes": planes,
        // Beyond the reference's record.
        "sequenceNumber": o.sequence_number,
        "proposal": proposal,
        "target": target,
        "telescope": telescope,
        "instrument": instrument,
        "environment": environment,
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
        // Release dates matter to an agent deciding whether data is public yet,
        // and `dataRelease` is the citation handle for a proprietary-period
        // observation. Both were parsed but never reported.
        "creatorId": p.creator_id,
        "metaRelease": p.meta_release,
        "dataRelease": p.data_release,
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
        for name in ["catalogue", "raDeg", "decDeg", "radiusArcsec"] {
            assert!(
                required.iter().any(|v| v == name),
                "the reference requires `{name}`"
            );
        }
    }

    #[test]
    fn the_cone_radius_is_read_in_the_unit_its_name_declares() {
        // The 3600x trap: the reference takes arcseconds, Verbinal shipped
        // degrees. Reading one as the other turns a 10-arcsecond cross-match
        // into a 10-DEGREE sweep of the sky.
        assert_eq!(
            cone_radius_deg(&json!({ "radiusArcsec": 3600.0 })).unwrap(),
            1.0
        );
        assert_eq!(cone_radius_deg(&json!({ "radiusDeg": 1.0 })).unwrap(), 1.0);
    }

    #[test]
    fn the_declared_radius_argument_wins_when_both_are_sent() {
        // A client sending both is most likely a reference-written one carrying
        // our older name along; the documented argument decides, and it must not
        // be silently reinterpreted in the other unit.
        let r = cone_radius_deg(&json!({ "radiusArcsec": 3600.0, "radiusDeg": 5.0 })).unwrap();
        assert_eq!(r, 1.0);
    }

    #[test]
    fn a_missing_or_negative_radius_is_refused() {
        assert!(cone_radius_deg(&json!({})).is_err());
        assert!(cone_radius_deg(&json!({ "radiusArcsec": -1.0 })).is_err());
        assert!(cone_radius_deg(&json!({ "radiusDeg": -1.0 })).is_err());
        // Zero is a degenerate but legal cone, not an error.
        assert_eq!(
            cone_radius_deg(&json!({ "radiusArcsec": 0.0 })).unwrap(),
            0.0
        );
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
        assert_eq!(
            opt_str_arg(&json!({ "c": "  V/97  " }), "c"),
            Some("V/97".to_string())
        );
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
