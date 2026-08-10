//! Live per-viewer MCP tools for the 2D FITS viewer. Port of
//! `Mcp/Tools/Write/FitsViewerTools.cs` (live viewport ops) +
//! `Mcp/Tools/Read/VoSpaceFitsReadTools.cs` (stateless header/WCS reads).
//!
//! The live ops (`get_fits_view`, `set_fits_view`, `probe_fits_pixel`,
//! `fits_goto_coordinate`, and the bookmark trio) marshal to the GTK main
//! thread via [`crate::mcp::view_state::viewer_command`] and act on the open
//! FITS tab. The two stateless ops (`get_fits_header`, `get_fits_wcs`) read a
//! file straight from disk with the FITS loader — no live viewer required.
//! All tools are `agent_safe`.

use super::{ToolDescriptor, ToolResult, VerbClass};
use crate::mcp::tools::proposals::{InMemoryProposalStore, PendingProposal};
use crate::mcp::view_state::viewer_command;
use crate::state::AppServices;
use serde_json::{json, Value};
use std::sync::Arc;

pub fn descriptors() -> Vec<ToolDescriptor> {
    let empty = json!({"type":"object","properties":{},"additionalProperties":false});
    let with_hdu = json!({
        "type":"object",
        "properties": {
            "path": { "type":"string", "description":"Local filesystem path to a FITS file" },
            "hdu": { "type":"integer", "minimum":1, "description":"1-based HDU number (default: the first image HDU)" }
        },
        "required": ["path"], "additionalProperties": false
    });
    vec![
        ToolDescriptor {
            name: "get_fits_view".into(),
            description: "Read the active FITS tab's state: file name + path, image dimensions, HDU name, \
                          zoom percent, viewport centre (image pixel), stretch, colormap, black/white cut \
                          levels, North-Up, whether WCS is present, and the crosshair sky position if placed. \
                          Errors if no FITS is open."
                .into(),
            input_schema: empty.clone(),
            verb: VerbClass::Read,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "set_fits_view".into(),
            description: "Steer the active FITS tab: zoom (percent, 100 = 1:1), viewport centre \
                          (center_x/center_y in image pixels), stretch (linear|log|sqrt|squared|asinh|histogram), \
                          colormap (grayscale|inverted|heat|viridis|plasma|inferno|magma|coolwarm), black/white cut \
                          levels (min_cut/max_cut in physical pixel units), North-Up, or reset. Only the fields you \
                          pass change. Returns the resulting view state. Live-applied."
                .into(),
            input_schema: json!({
                "type":"object",
                "properties": {
                    "zoom": { "type":"number", "minimum":5, "maximum":2000, "description":"Zoom percent (100 = 1:1)" },
                    "centerX": { "type":"number", "description":"Image x-pixel to centre the viewport on" },
                    "centerY": { "type":"number", "description":"Image y-pixel to centre the viewport on" },
                    "stretch": { "type":"string", "enum":["linear","log","sqrt","squared","asinh","histogram"] },
                    "colormap": { "type":"string", "enum":["grayscale","inverted","heat","viridis","plasma","inferno","magma","coolwarm"] },
                    "minCut": { "type":"number" },
                    "maxCut": { "type":"number" },
                    "northUp": { "type":"boolean" },
                    "reset": { "type":"boolean", "description":"Reset stretch + zoom/pan to defaults" }
                },
                "additionalProperties": false
            }),
            verb: VerbClass::Write,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "probe_fits_pixel".into(),
            description: "Read the pixel value and sky coordinate (RA/Dec, if the FITS has WCS) at a 0-based image \
                          pixel (x, y) of the active FITS tab. The value carries its physical unit in `unit` (the \
                          FITS BUNIT) when present. Errors if no FITS is open or (x, y) is out of range."
                .into(),
            input_schema: json!({
                "type":"object",
                "properties": {
                    "x": { "type":"integer", "minimum":0 },
                    "y": { "type":"integer", "minimum":0 }
                },
                "required": ["x","y"], "additionalProperties": false
            }),
            verb: VerbClass::Read,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "fits_goto_coordinate".into(),
            description: "Centre the active FITS viewport on a sky coordinate (RA/Dec in degrees) and place the \
                          crosshair there. Requires the loaded FITS to have valid WCS. Distinct from \
                          set_search_focus (which targets the Search form). Live-applied."
                .into(),
            input_schema: json!({
                "type":"object",
                "properties": { "ra": { "type":"number" }, "dec": { "type":"number" } },
                "required": ["ra","dec"], "additionalProperties": false
            }),
            verb: VerbClass::Write,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "list_fits_bookmarks".into(),
            description: "List the in-memory FITS sky-coordinate bookmarks (name, RA/Dec in degrees, source file). \
                          Use fits_goto_coordinate with a bookmark's ra/dec to jump the viewport there."
                .into(),
            input_schema: empty.clone(),
            verb: VerbClass::Read,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "save_fits_bookmark".into(),
            description: "Save (or update, by name) a FITS sky-coordinate bookmark. Provide ra/dec in degrees, or \
                          omit them to capture the active tab's current crosshair position. Returns the saved \
                          bookmark. Live-applied."
                .into(),
            input_schema: json!({
                "type":"object",
                "properties": {
                    "name": { "type":"string" },
                    "ra": { "type":"number" },
                    "dec": { "type":"number" }
                },
                "required": ["name"], "additionalProperties": false
            }),
            verb: VerbClass::Write,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "delete_fits_bookmark".into(),
            description: "Delete an in-memory FITS bookmark by its name (from list_fits_bookmark). Returns whether a \
                          bookmark was found and removed. Live-applied."
                .into(),
            input_schema: json!({
                "type":"object",
                "properties": { "name": { "type":"string" } },
                "required": ["name"], "additionalProperties": false
            }),
            verb: VerbClass::Write,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "get_fits_header".into(),
            description: "Read the FITS header cards (keyword/value/comment) of one HDU in a local FITS file on \
                          disk. Stateless — no open viewer required."
                .into(),
            input_schema: with_hdu.clone(),
            verb: VerbClass::Read,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "get_fits_wcs".into(),
            description: "Read the World Coordinate System (WCS) solution of one HDU in a local FITS file on disk: \
                          reference pixel/value, CD matrix, projection, pixel scale, North angle, parity. Stateless."
                .into(),
            input_schema: with_hdu,
            verb: VerbClass::Read,
            agent_safe: true,
        },
    ]
}

pub async fn dispatch(
    name: &str,
    _services: &AppServices,
    args: &Value,
    _proposals: &Arc<InMemoryProposalStore>,
) -> Option<ToolResult> {
    match name {
        // Live ops — the op name equals the tool name; forward to the open viewer.
        "get_fits_view"
        | "set_fits_view"
        | "probe_fits_pixel"
        | "fits_goto_coordinate"
        | "list_fits_bookmarks"
        | "save_fits_bookmark"
        | "delete_fits_bookmark" => Some(to_tool_result(
            viewer_command("fits", name, args.clone()).await,
        )),
        // Stateless ops — read the file directly, NOT through the bridge.
        "get_fits_header" => Some(to_tool_result(get_fits_header(args))),
        "get_fits_wcs" => Some(to_tool_result(get_fits_wcs(args))),
        _ => None,
    }
}

/// FITS tools execute directly (agent-safe) — they never enqueue proposals.
pub async fn apply(_s: &AppServices, _p: &PendingProposal) -> Option<Result<String, String>> {
    None
}

/// Map a JSON result into a `ToolResult`, promoting an `image_base64` payload to
/// a PNG image (per the family contract; unused by the current FITS ops).
fn to_tool_result(r: Result<Value, String>) -> ToolResult {
    match r {
        Ok(v) => match v.get("image_base64").and_then(|x| x.as_str()) {
            Some(b64) => ToolResult::Image {
                data_base64: b64.to_string(),
                mime: "image/png".into(),
                caption: None,
            },
            None => ToolResult::Data(v),
        },
        Err(e) => ToolResult::Failed(e),
    }
}

// ─── Stateless disk readers ──────────────────────────────────────────────────

/// Load a FITS image (whole first-image HDU, or a specific 1-based HDU) from
/// disk. Behind the `fits` feature; returns an error when it is not compiled.
#[cfg(feature = "fits")]
fn load_from_disk(path: &str, hdu: Option<i64>) -> Result<crate::models::FitsImageData, String> {
    let p = std::path::Path::new(path);
    match hdu {
        Some(h) if h >= 1 => crate::helpers::fits_loader::load_fits_image_hdu(p, h as usize),
        _ => crate::helpers::fits_loader::load_fits_image(p),
    }
}

#[cfg(not(feature = "fits"))]
fn load_from_disk(_path: &str, _hdu: Option<i64>) -> Result<crate::models::FitsImageData, String> {
    Err("fits feature not built".into())
}

fn require_path(args: &Value) -> Result<String, String> {
    args.get("path")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "path is required".to_string())
}

fn get_fits_header(args: &Value) -> Result<Value, String> {
    let path = require_path(args)?;
    let hdu = args.get("hdu").and_then(|v| v.as_i64());
    let data = load_from_disk(&path, hdu)?;
    let cards: Vec<Value> = data
        .header_ordered
        .iter()
        .map(|(k, v, c)| json!({ "keyword": k, "value": v, "comment": c }))
        .collect();
    Ok(json!({
        "path": path,
        "hdu": hdu,
        "width": data.width,
        "height": data.height,
        "count": cards.len(),
        "cards": cards,
    }))
}

fn get_fits_wcs(args: &Value) -> Result<Value, String> {
    let path = require_path(args)?;
    let hdu = args.get("hdu").and_then(|v| v.as_i64());
    let data = load_from_disk(&path, hdu)?;
    let base = json!({ "path": path, "hdu": hdu, "width": data.width, "height": data.height });
    let mut out = base;
    match data.wcs.as_ref() {
        Some(w) => {
            let valid = w.is_valid();
            out["is_valid"] = json!(valid);
            out["is_approximate"] = json!(w.is_approximate);
            out["solution_kind"] = json!(w.solution_kind());
            out["ctype1"] = json!(w.ctype1);
            out["ctype2"] = json!(w.ctype2);
            out["projection"] = json!(format!("{:?}", w.proj()));
            out["crpix1"] = json!(w.crpix1);
            out["crpix2"] = json!(w.crpix2);
            out["crval1"] = json!(w.crval1);
            out["crval2"] = json!(w.crval2);
            out["cd1_1"] = json!(w.cd1_1);
            out["cd1_2"] = json!(w.cd1_2);
            out["cd2_1"] = json!(w.cd2_1);
            out["cd2_2"] = json!(w.cd2_2);
            if valid {
                out["pixel_scale_arcsec"] = json!(w.pixel_scale_arcsec());
                out["north_angle"] = json!(w.north_angle());
                out["has_parity_flip"] = json!(w.has_parity_flip());
            }
        }
        None => {
            out["is_valid"] = json!(false);
            out["is_approximate"] = json!(false);
            out["solution_kind"] = json!("none");
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptors_unique_and_agent_safe() {
        let d = descriptors();
        assert!(!d.is_empty());
        assert!(
            d.iter().all(|x| !x.name.is_empty()),
            "every tool name is non-empty"
        );
        let mut names: Vec<_> = d.iter().map(|x| x.name.clone()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), d.len(), "tool names are unique");
        assert!(
            d.iter().all(|x| x.agent_safe),
            "all FITS tools are agent-safe"
        );
    }
}
