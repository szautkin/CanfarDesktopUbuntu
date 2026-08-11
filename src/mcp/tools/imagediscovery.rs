//! Image-discovery tool family — a read tool that searches the local image-content
//! cache for images containing a set of packages/capabilities, and a write tool that
//! schedules a probe of one image so it becomes searchable.
//!
//! Ported from the CanfarDesktop (Windows) reference:
//!   * `Mcp/Tools/Read/ImageDiscoveryReadTool.cs`  → `find_images_with_packages`
//!   * `Mcp/Tools/Write/ImageDiscoveryWriteTools.cs` → `discover_image_packages`
//!
//! Both back onto services the integrator adds to [`AppServices`]:
//!   * `services.image_manifests` — an `Arc<JsonManifestStore>` (the read side's cache).
//!   * `services.image_discovery` — an `Arc<ImageDiscoveryCoordinator>` (the write side's
//!     probe orchestrator, owned by Agent A).
//!
//! The Linux port has **no external image catalogue with per-session-type metadata**
//! (the Windows tool takes a `catalogue` of `(id, types)`); instead the manifest cache
//! IS the catalogue — an image is "known" only after a probe was attempted, and
//! "discovered" once that probe yielded a usable manifest. `find_images_with_packages`
//! reads straight from the store; `discover_image_packages` never probes at propose
//! time — it enqueues a NON-destructive [`PendingProposal`] whose [`apply`] invokes the
//! coordinator.

use crate::mcp::tools::proposals::{InMemoryProposalStore, PendingProposal};
use crate::mcp::tools::{bool_arg, opt_str_arg, str_arg, ToolDescriptor, ToolResult, VerbClass};
use crate::models::image_manifest::{capability, DiscoveryOutcome, PackageQuery};
use crate::state::AppServices;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::Arc;

/// `candidatesToProbe` is a next-step shortlist, not an exhaustive dump.
const CANDIDATES_CAP: usize = 10;
/// A near-miss image is only surfaced once it satisfies at least half the query's terms.
const PARTIAL_MIN_SCORE: f64 = 0.5;
/// At most this many ranked near-misses are returned.
const PARTIAL_LIMIT: usize = 5;

/// Session types accepted by the (parity-only) `type` filter. The Linux manifest cache
/// records no per-image session type, so the filter is accepted but does not scope
/// results — it exists so clients written against the Windows/macOS contract don't error.
const SESSION_TYPES: &[&str] = &[
    "notebook",
    "desktop",
    "carta",
    "firefly",
    "contributed",
    "headless",
];

// ─────────────────────────────────────────────────────────────────────────────
// Manifest
// ─────────────────────────────────────────────────────────────────────────────

/// Descriptors advertised for the image-discovery family. `find_images_with_packages`
/// is `Read`/`agent_safe`; `discover_image_packages` is `Write`/`agent_safe` — it is
/// non-destructive (a cache-miss runs one small probe job), so the proposal pipeline
/// may auto-apply it, but it still flows through a [`PendingProposal`].
pub fn descriptors() -> Vec<ToolDescriptor> {
    vec![
        ToolDescriptor {
            name: "find_images_with_packages".to_string(),
            description: "Search the user's local image-content cache for images that contain ALL \
                listed packages / capabilities (intersection). Free — no Skaha jobs run. `packages` \
                match case-insensitively by substring across every package family (apt/rpm/apk + pip \
                + R). `capabilities` filter on behavioural flags the probe detects beyond raw package \
                names. Optional `osFamily`/`osVersion`/`hasPython`/`hasR` add OS + interpreter \
                constraints. `type` is accepted for client parity but does NOT scope results (the \
                Linux cache stores no per-session-type metadata). Returns: (1) imageIDs — strict-match \
                hits, ranked by coverage; (2) count — imageIDs length; (3) unfiltered — true when no \
                constraint was given; (4) coverage {total, discovered, matching} over the cache; (5) \
                candidatesToProbe — up to 10 known-but-not-yet-discovered images to probe next; (6) \
                allDiscovered — every image the cache knows about; (7) knownPackageCount — distinct \
                package names across the cache; (8) partialMatches — ranked near-misses \
                {imageID, score(0-1)}, populated ONLY when imageIDs is empty AND you supplied \
                constraints. When imageIDs is empty but candidatesToProbe is non-empty, the answer is \
                \"unknown — here's what to probe next.\""
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "packages": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Package names required across ANY family (apt/rpm/apk/pip/R). \
                            Matched case-insensitively by substring, so 'cfitsio' hits 'libcfitsio-dev'."
                    },
                    "capabilities": {
                        "type": "array",
                        "items": { "type": "string", "enum": capability_enum() },
                        "description": "Required behavioural capability flags the probe detects."
                    },
                    "osFamily": {
                        "type": "string",
                        "description": "Required OS family (e.g. 'ubuntu', 'almalinux'), case-insensitive."
                    },
                    "osVersion": {
                        "type": "string",
                        "description": "Required OS version (e.g. '22.04'), case-insensitive."
                    },
                    "hasPython": {
                        "type": "boolean",
                        "description": "true → require a usable Python; false → require none."
                    },
                    "hasR": {
                        "type": "boolean",
                        "description": "true → require R present; false → require it absent."
                    },
                    "type": {
                        "type": "string",
                        "enum": session_types_enum(),
                        "description": "Parity-only session-type hint; accepted but not used to scope \
                            results in the Linux port."
                    }
                },
                "additionalProperties": false
            }),
            verb: VerbClass::Read,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "discover_image_packages".to_string(),
            description: "Run a probe job to enumerate the named image's installed packages \
                (apt/rpm/apk + pip + conda + R) and cache the result so it becomes queryable via \
                find_images_with_packages. A cache hit short-circuits with no Skaha cost; a cache \
                miss runs one small probe job. Pass force=true to bypass a fresh cache entry (e.g. \
                after an image rebuild). Non-destructive: queues a proposal that, once applied, makes \
                the image matchable."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "image": {
                        "type": "string",
                        "minLength": 1,
                        "description": "Full image reference to probe, e.g. \
                            'images.canfar.net/skaha/astroml:24.07'."
                    },
                    "force": {
                        "type": "boolean",
                        "description": "Bypass the cache and re-probe even if a manifest already exists."
                    }
                },
                "required": ["image"],
                "additionalProperties": false
            }),
            verb: VerbClass::Write,
            agent_safe: true,
        },
    ]
}

/// The capability keys advertised in the `capabilities` enum, built from the canonical
/// [`capability::ALL`] so the schema never drifts from the model.
fn capability_enum() -> Value {
    Value::Array(
        capability::ALL
            .iter()
            .map(|c| Value::String((*c).to_string()))
            .collect(),
    )
}

/// The session-type keys advertised in the parity-only `type` enum.
fn session_types_enum() -> Value {
    Value::Array(
        SESSION_TYPES
            .iter()
            .map(|t| Value::String((*t).to_string()))
            .collect(),
    )
}

// ─────────────────────────────────────────────────────────────────────────────
// Dispatch — the read hits the store directly; the write enqueues a proposal
// ─────────────────────────────────────────────────────────────────────────────

/// Handle an image-discovery call. Returns `Some(..)` when `name` is one of this
/// family's tools (so the router stops chaining), or `None` otherwise.
pub async fn dispatch(
    name: &str,
    services: &AppServices,
    args: &Value,
    proposals: &Arc<InMemoryProposalStore>,
) -> Option<ToolResult> {
    let result = match name {
        "find_images_with_packages" => find_images(services, args),
        "discover_image_packages" => propose_discover(args, proposals),
        _ => return None,
    };
    Some(result)
}

/// Search the cache and shape the rich output. Synchronous — every `JsonManifestStore`
/// query is in-memory — but called from an async context.
fn find_images(services: &AppServices, args: &Value) -> ToolResult {
    let query = build_query(args);
    let store = &services.image_manifests;

    let matched = store.search(&query);
    let known = store.known_images();
    // An empty query returns every SUCCESSFULLY-discovered image id — our "discovered" set.
    let successful = store.search(&PackageQuery::default());
    let known_package_count = store.all_packages().len();

    // Partial ranking is only meaningful when the strict AND-match came back empty and
    // the user actually constrained the search.
    let partials = if matched.is_empty() && !query.is_empty() {
        store.search_partial(&query)
    } else {
        Vec::new()
    };

    ToolResult::Data(shape_find_output(
        &query,
        matched,
        known,
        successful,
        partials,
        known_package_count,
    ))
}

/// The image reference to probe.
///
/// The reference declares this argument as `image`, and that is the name agents
/// are written against. Verbinal shipped it as `imageId` first, so that spelling
/// is still accepted — an older client should not start failing over a rename it
/// never saw. The same accessor reads the proposal payload, so propose and apply
/// can never disagree about which key holds the image.
fn image_arg(args: &Value) -> String {
    let image = str_arg(args, "image");
    if image.is_empty() {
        str_arg(args, "imageId")
    } else {
        image
    }
}

/// Enqueue a NON-destructive `discover_image_packages` proposal. The real probe runs in
/// [`apply`] via the coordinator once the proposal is accepted.
fn propose_discover(args: &Value, proposals: &Arc<InMemoryProposalStore>) -> ToolResult {
    let image_id = image_arg(args);
    if image_id.is_empty() {
        return ToolResult::Failed("image is required".to_string());
    }
    let force = bool_arg(args, "force");
    let summary = if force {
        format!("Re-probe packages installed in '{}'", image_id)
    } else {
        format!("Discover packages installed in '{}'", image_id)
    };
    // Payload echoes the args so the applier can reconstruct the call verbatim.
    let payload = json!({ "image": image_id, "force": force });
    let p = proposals.enqueue("discover_image_packages", &summary, false, payload);
    ToolResult::Proposed(p)
}

// ─────────────────────────────────────────────────────────────────────────────
// Apply — decode an accepted proposal and run the coordinator probe
// ─────────────────────────────────────────────────────────────────────────────

/// Execute an accepted image-discovery proposal. Returns `Some(..)` when
/// `proposal.kind` belongs to this family, or `None` so the router can try another
/// family's applier.
pub async fn apply(
    services: &AppServices,
    proposal: &PendingProposal,
) -> Option<Result<String, String>> {
    match proposal.kind.as_str() {
        "discover_image_packages" => Some(apply_discover(services, &proposal.payload).await),
        _ => None,
    }
}

async fn apply_discover(services: &AppServices, payload: &Value) -> Result<String, String> {
    let image_id = image_arg(payload);
    if image_id.is_empty() {
        return Err("discover_image_packages payload missing image".to_string());
    }
    let force = bool_arg(payload, "force");
    let outcome = services
        .image_discovery
        .discover_image(services, &image_id, force)
        .await;
    discovery_status(&image_id, &outcome)
}

/// Turn a coordinator [`DiscoveryOutcome`] into a short apply status (`Ok`) or a typed
/// error string (`Err`).
fn discovery_status(image_id: &str, outcome: &DiscoveryOutcome) -> Result<String, String> {
    match outcome {
        DiscoveryOutcome::Manifest(m) => {
            let pkgs = m.all_package_names().len();
            let caps = m.capabilities.len();
            Ok(format!(
                "Discovered '{}': {} package{}, {} capabilit{}",
                image_id,
                pkgs,
                if pkgs == 1 { "" } else { "s" },
                caps,
                if caps == 1 { "y" } else { "ies" }
            ))
        }
        DiscoveryOutcome::Failure {
            category,
            message,
            job_id,
        } => {
            let mut msg = format!(
                "discovery failed for '{}' ({}): {}",
                image_id, category, message
            );
            if let Some(j) = job_id {
                msg.push_str(&format!(" [job {}]", j));
            }
            Err(msg)
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Pure shaping / argument helpers (no service dependency — unit-tested below)
// ─────────────────────────────────────────────────────────────────────────────

/// Build a [`PackageQuery`] from the read tool's arguments.
fn build_query(args: &Value) -> PackageQuery {
    PackageQuery {
        packages: str_array(args, "packages"),
        capabilities: str_array(args, "capabilities"),
        os_family: opt_str_arg(args, "osFamily"),
        os_version: opt_str_arg(args, "osVersion"),
        python: args.get("hasPython").and_then(Value::as_bool),
        r: args.get("hasR").and_then(Value::as_bool),
    }
}

/// Shape the `find_images_with_packages` output from already-fetched store data.
///
/// * `matched`  — strict AND-match ids, already ranked by the store (score desc, id asc).
/// * `known`    — every image the cache knows about (sorted; success + failure attempts).
/// * `successful` — ids with a usable manifest (the store's empty-query result).
/// * `partials` — `(id, satisfied_terms)` near-misses (empty unless we asked for them).
/// * `known_package_count` — distinct package names across the cache.
fn shape_find_output(
    query: &PackageQuery,
    matched: Vec<String>,
    known: Vec<String>,
    successful: Vec<String>,
    partials: Vec<(String, u32)>,
    known_package_count: usize,
) -> Value {
    let match_count = matched.len();
    let known_total = known.len();
    let discovered_count = successful.len();

    // candidatesToProbe = known images without a usable manifest yet, minus anything we
    // already matched, in the store's stable (sorted) order, capped.
    let successful_set: HashSet<&str> = successful.iter().map(String::as_str).collect();
    let matched_set: HashSet<&str> = matched.iter().map(String::as_str).collect();
    let candidates: Vec<String> = known
        .iter()
        .filter(|id| {
            let id = id.as_str();
            !successful_set.contains(id) && !matched_set.contains(id)
        })
        .take(CANDIDATES_CAP)
        .cloned()
        .collect();

    // partialMatches only when the strict match is empty AND the user constrained the
    // search. Convert the raw satisfied-term count into a 0..1 coverage fraction, drop
    // anything under the threshold, and cap the list.
    let partial_out: Vec<Value> = if match_count == 0 && !query.is_empty() {
        let total = query.total_terms().max(1) as f64;
        partials
            .into_iter()
            .map(|(id, score)| (id, round4(score as f64 / total)))
            .filter(|(_, frac)| *frac >= PARTIAL_MIN_SCORE)
            .take(PARTIAL_LIMIT)
            .map(|(id, frac)| json!({ "imageID": id, "score": frac }))
            .collect()
    } else {
        Vec::new()
    };

    json!({
        "imageIDs": matched,
        "count": match_count,
        "unfiltered": query.is_empty(),
        "coverage": {
            "total": known_total,
            "discovered": discovered_count,
            "matching": match_count
        },
        "candidatesToProbe": candidates,
        "allDiscovered": known,
        "knownPackageCount": known_package_count,
        "partialMatches": partial_out
    })
}

/// Round to 4 decimal places (keeps coverage fractions tidy on the wire).
fn round4(x: f64) -> f64 {
    (x * 10_000.0).round() / 10_000.0
}

/// Collect trimmed, non-empty strings from a JSON array argument (empty vec if absent).
fn str_array(args: &Value, key: &str) -> Vec<String> {
    args.get(key)
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::image_manifest::ImageManifest;

    fn ids(v: &Value, key: &str) -> Vec<String> {
        v[key]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_str().unwrap().to_string())
            .collect()
    }

    fn s(list: &[&str]) -> Vec<String> {
        list.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn descriptor_names_unique_and_verbs_correct() {
        let ds = descriptors();
        assert_eq!(ds.len(), 2);
        let mut seen = HashSet::new();
        for d in &ds {
            assert!(!d.name.is_empty());
            assert!(!d.description.is_empty(), "{} needs a description", d.name);
            assert!(d.agent_safe, "{} must be agent_safe", d.name);
            assert!(seen.insert(d.name.clone()), "duplicate: {}", d.name);
        }
        let find = ds
            .iter()
            .find(|d| d.name == "find_images_with_packages")
            .unwrap();
        assert_eq!(find.verb, VerbClass::Read);
        let disc = ds
            .iter()
            .find(|d| d.name == "discover_image_packages")
            .unwrap();
        assert_eq!(disc.verb, VerbClass::Write);
        // The capability enum in the schema is built from the canonical list.
        let enum_len = find.input_schema["properties"]["capabilities"]["items"]["enum"]
            .as_array()
            .unwrap()
            .len();
        assert_eq!(enum_len, capability::ALL.len());
    }

    #[test]
    fn build_query_maps_every_field() {
        let q = build_query(&json!({
            "packages": ["numpy", "  ", "astropy"],
            "capabilities": ["gpu"],
            "osFamily": " Ubuntu ",
            "osVersion": "22.04",
            "hasPython": true,
            "hasR": false,
            "type": "headless"
        }));
        assert_eq!(q.packages, s(&["numpy", "astropy"])); // blank dropped
        assert_eq!(q.capabilities, s(&["gpu"]));
        assert_eq!(q.os_family.as_deref(), Some("Ubuntu")); // trimmed, case preserved (store compares CI)
        assert_eq!(q.os_version.as_deref(), Some("22.04"));
        assert_eq!(q.python, Some(true));
        assert_eq!(q.r, Some(false));
        assert!(!q.is_empty());
    }

    #[test]
    fn empty_args_build_empty_query() {
        let q = build_query(&json!({}));
        assert!(q.is_empty());
        let q2 = build_query(&Value::Null);
        assert!(q2.is_empty());
    }

    #[test]
    fn matches_pass_through_and_unfiltered_flag() {
        let out = shape_find_output(
            &PackageQuery::default(),
            s(&["a:1", "b:1"]),
            s(&["a:1", "b:1"]),
            s(&["a:1", "b:1"]),
            Vec::new(),
            7,
        );
        assert_eq!(ids(&out, "imageIDs"), s(&["a:1", "b:1"]));
        assert_eq!(out["count"], 2);
        assert_eq!(out["unfiltered"], true);
        assert_eq!(out["knownPackageCount"], 7);
        assert_eq!(out["partialMatches"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn candidates_exclude_discovered_and_matched_sorted_and_capped() {
        // known has a match (a:1), a discovered non-match (d:1) and 15 unprobed failures.
        let mut known = vec!["a:1".to_string(), "d:1".to_string()];
        for i in 1..=15 {
            known.push(format!("img:{:02}", i));
        }
        known.sort();
        let out = shape_find_output(
            &build_query(&json!({ "packages": ["numpy"] })),
            s(&["a:1"]),        // matched
            known.clone(),      // known
            s(&["a:1", "d:1"]), // successful (a:1 matched, d:1 discovered but not matched)
            Vec::new(),
            0,
        );
        let candidates = ids(&out, "candidatesToProbe");
        assert_eq!(candidates.len(), CANDIDATES_CAP);
        assert!(!candidates.contains(&"a:1".to_string()), "matched excluded");
        assert!(
            !candidates.contains(&"d:1".to_string()),
            "discovered excluded"
        );
        // Stable sorted order: first ten of img:01..img:15.
        assert_eq!(candidates[0], "img:01");
        assert_eq!(candidates[9], "img:10");
        // Coverage reflects the full cache.
        assert_eq!(out["coverage"]["total"], known.len());
        assert_eq!(out["coverage"]["discovered"], 2);
        assert_eq!(out["coverage"]["matching"], 1);
        assert_eq!(ids(&out, "allDiscovered"), known);
    }

    #[test]
    fn partials_only_when_matched_empty_and_query_non_empty() {
        // total_terms = 6 packages → fractions 5/6≈0.833, 4/6≈0.667, 2/6≈0.333.
        let query = build_query(&json!({
            "packages": ["a", "b", "c", "d", "e", "f"]
        }));
        let out = shape_find_output(
            &query,
            Vec::new(), // no strict match
            s(&["img:near", "img:other", "img:low"]),
            s(&["img:near", "img:other", "img:low"]),
            vec![
                ("img:near".to_string(), 5),
                ("img:other".to_string(), 4),
                ("img:low".to_string(), 2), // below 0.5 → filtered out
            ],
            3,
        );
        let pm = out["partialMatches"].as_array().unwrap();
        assert_eq!(pm.len(), 2);
        assert_eq!(pm[0]["imageID"], "img:near");
        assert_eq!(pm[0]["score"], round4(5.0 / 6.0));
        assert_eq!(pm[1]["imageID"], "img:other");
        assert!(pm.iter().all(|p| p["imageID"] != "img:low"));
    }

    #[test]
    fn strict_match_suppresses_partials() {
        let query = build_query(&json!({ "packages": ["astropy"] }));
        let out = shape_find_output(
            &query,
            s(&["img:hit"]),
            s(&["img:hit"]),
            s(&["img:hit"]),
            vec![("img:fake".to_string(), 1)],
            1,
        );
        assert_eq!(ids(&out, "imageIDs"), s(&["img:hit"]));
        assert_eq!(out["partialMatches"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn empty_query_yields_no_partials_even_with_data() {
        let out = shape_find_output(
            &PackageQuery::default(),
            Vec::new(),
            s(&["img:x"]),
            s(&["img:x"]),
            vec![("img:x".to_string(), 3)],
            5,
        );
        assert_eq!(out["partialMatches"].as_array().unwrap().len(), 0);
        assert_eq!(out["unfiltered"], true);
    }

    #[test]
    fn empty_cache_does_not_crash() {
        let out = shape_find_output(
            &PackageQuery::default(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            0,
        );
        assert_eq!(out["coverage"]["total"], 0);
        assert_eq!(out["candidatesToProbe"].as_array().unwrap().len(), 0);
        assert_eq!(out["allDiscovered"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn propose_discover_default_not_force() {
        let store = Arc::new(InMemoryProposalStore::new());
        match propose_discover(&json!({ "image": "img:a" }), &store) {
            ToolResult::Proposed(p) => {
                assert_eq!(p.kind, "discover_image_packages");
                assert!(!p.destructive, "discovery is non-destructive");
                assert_eq!(p.summary, "Discover packages installed in 'img:a'");
                assert_eq!(p.payload["image"], "img:a");
                assert_eq!(p.payload["force"], false);
            }
            _ => panic!("expected Proposed"),
        }
        assert_eq!(store.pending_count(), 1);
    }

    #[test]
    fn the_declared_image_argument_is_the_one_the_tool_reads() {
        // The reference names this argument `image`, and its schema is
        // `additionalProperties: false` — so an agent sends exactly that. When
        // ours declared `imageId`, every reference-written call failed with
        // "image is required". Bind the declaration to the reader.
        let schema = descriptors()
            .into_iter()
            .find(|d| d.name == "discover_image_packages")
            .expect("the tool is declared")
            .input_schema;
        let required = schema["required"].as_array().expect("a required list");
        assert_eq!(required.len(), 1);
        let declared = required[0].as_str().unwrap();
        assert_eq!(
            declared, "image",
            "must match the reference's argument name"
        );

        let store = Arc::new(InMemoryProposalStore::new());
        assert!(
            matches!(
                propose_discover(&json!({ declared: "img:a" }), &store),
                ToolResult::Proposed(_)
            ),
            "the declared argument name must be the one the tool actually reads"
        );
    }

    #[test]
    fn the_original_image_id_spelling_still_works() {
        // Back-compat: Verbinal shipped `imageId` before the rename, so a client
        // written against that must not break.
        let store = Arc::new(InMemoryProposalStore::new());
        match propose_discover(&json!({ "imageId": "img:a" }), &store) {
            ToolResult::Proposed(p) => assert_eq!(p.payload["image"], "img:a"),
            _ => panic!("expected Proposed"),
        }
    }

    #[test]
    fn propose_discover_force_changes_summary_and_payload() {
        let store = Arc::new(InMemoryProposalStore::new());
        match propose_discover(&json!({ "image": "img:a", "force": true }), &store) {
            ToolResult::Proposed(p) => {
                assert_eq!(p.summary, "Re-probe packages installed in 'img:a'");
                assert_eq!(p.payload["force"], true);
            }
            _ => panic!("expected Proposed"),
        }
    }

    #[test]
    fn propose_discover_requires_image_id() {
        let store = Arc::new(InMemoryProposalStore::new());
        assert!(matches!(
            propose_discover(&json!({ "imageId": "   " }), &store),
            ToolResult::Failed(_)
        ));
        assert!(matches!(
            propose_discover(&json!({}), &store),
            ToolResult::Failed(_)
        ));
        assert_eq!(store.pending_count(), 0);
    }

    #[test]
    fn discovery_status_success_and_failure() {
        let mut m = ImageManifest {
            image_id: "img:a".to_string(),
            ..Default::default()
        };
        m.python = s(&["numpy", "astropy"]);
        m.capabilities = s(&["gpu"]);
        let ok = discovery_status("img:a", &DiscoveryOutcome::Manifest(m)).unwrap();
        assert!(ok.contains("2 packages"), "got: {ok}");
        assert!(ok.contains("1 capability"), "got: {ok}");

        let fail = discovery_status(
            "img:b",
            &DiscoveryOutcome::Failure {
                category: "JobTimedOut".to_string(),
                message: "timed out".to_string(),
                job_id: Some("job-9".to_string()),
            },
        );
        let err = fail.unwrap_err();
        assert!(err.contains("JobTimedOut"));
        assert!(err.contains("job-9"));
    }
}
