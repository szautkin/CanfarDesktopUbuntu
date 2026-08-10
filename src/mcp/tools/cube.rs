//! Live per-viewer MCP tools for the 3D Cube Viewer.
//!
//! Each tool forwards over the view-state bridge
//! ([`crate::mcp::view_state::viewer_command`]) to the live [`CubeTabHost`] on the
//! GTK main thread (target `"cube"`), which reads/mutates the open cube's GL
//! ray-marcher and replies with JSON. All tools are agent-safe: getters/probes are
//! [`VerbClass::Read`], view mutations + export are [`VerbClass::Write`], but none
//! route through the write-proposal pipeline — they act directly on the viewer the
//! user is already looking at.
//!
//! Ported in spirit from `Mcp/Tools/*` (the Windows reference exposes the same
//! live-viewer control surface).

use super::{ToolDescriptor, ToolResult, VerbClass};
use crate::mcp::tools::proposals::{InMemoryProposalStore, PendingProposal};
use crate::mcp::view_state;
use crate::state::AppServices;
use serde_json::{json, Value};
use std::sync::Arc;

/// The five cube tools; also the exact `op` strings the host matches on.
const TOOLS: &[&str] = &[
    "open_cube",
    "get_cube_view",
    "set_cube_view",
    "probe_cube_spectrum",
    "export_cube_figure",
];

pub fn descriptors() -> Vec<ToolDescriptor> {
    vec![
        ToolDescriptor {
            name: "open_cube".into(),
            description:
                "Open a FITS spectral cube (NAXIS≥3) from a local path in the 3D Cube Viewer."
                    .into(),
            input_schema: json!({
                "type": "object",
                "properties": { "path": { "type": "string", "description": "Local .fits/.fits.fz cube path." } },
                "required": ["path"],
                "additionalProperties": false
            }),
            verb: VerbClass::Write,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "get_cube_view".into(),
            description:
                "Read the active cube's 3D view: camera az/el/dist, quality steps, spectral scale, current channel, BUNIT value unit, and voxel dimensions."
                    .into(),
            input_schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
            verb: VerbClass::Read,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "set_cube_view".into(),
            description:
                "Update any subset of the active cube's 3D volume view — orbit az/el, dolly dist, camera reset, quality steps, spectral (Z) stretch, opacity density, MIP / render mode, background preset, idle auto-orbit, and slice-plane channel. Returns the resulting view."
                    .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "az": { "type": "number", "description": "Camera azimuth (radians)." },
                    "el": { "type": "number", "description": "Camera elevation (radians, clamped ±1.4)." },
                    "dist": { "type": "number", "description": "Camera distance (clamped 0.5–8)." },
                    "reset_camera": { "type": "boolean", "description": "Reset the orbit camera to the default framing (applied before any az/el/dist override)." },
                    "steps": { "type": "number", "description": "Ray-march quality steps (32–1024)." },
                    "spectral_scale": { "type": "number", "description": "Spectral (Z) axis stretch (0.5–4)." },
                    "density": { "type": "number", "description": "Volume opacity/density multiplier (> 0)." },
                    "mip": { "type": "boolean", "description": "Max-intensity projection on/off." },
                    "render_mode": { "type": "string", "description": "Volume render mode: \"emission\" or \"max-intensity\" (alias for the MIP toggle)." },
                    "background": { "type": "string", "enum": ["dark", "black", "light"], "description": "3D background preset." },
                    "auto_orbit": { "type": "boolean", "description": "Idle auto-orbit on/off." },
                    "channel": { "type": "integer", "minimum": 0, "description": "Slice-plane channel index." }
                },
                "additionalProperties": false
            }),
            verb: VerbClass::Write,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "probe_cube_spectrum".into(),
            description:
                "Sample the spectrum through the cube at spatial voxel (x, y) across every channel, returning normalized and physical values."
                    .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "x": { "type": "integer", "minimum": 0, "description": "Voxel X (column)." },
                    "y": { "type": "integer", "minimum": 0, "description": "Voxel Y (row)." }
                },
                "required": ["x", "y"],
                "additionalProperties": false
            }),
            verb: VerbClass::Read,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "export_cube_figure".into(),
            description:
                "Render the current cube view (3D volume or 2D slice) to a figure. With a 'path' set, write it to disk as PNG or PDF and return the path (mirrors ExportCubeToPathAsync); otherwise return the PNG inline as base64."
                    .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "width": { "type": "integer", "minimum": 16, "maximum": 4096, "description": "Base output width px (default 1024), before 'scale'." },
                    "height": { "type": "integer", "minimum": 16, "maximum": 4096, "description": "Base output height px (default 768), before 'scale'." },
                    "scale": { "type": "integer", "minimum": 1, "maximum": 4, "description": "Resolution multiplier applied to width/height (default 1)." },
                    "transparent": { "type": "boolean", "description": "Clear the 3D background to alpha 0 (default false)." },
                    "path": { "type": "string", "description": "Absolute output file path. When set, the figure is written to disk (PNG/PDF) and the path is returned instead of base64." },
                    "format": { "type": "string", "enum": ["png", "pdf"], "description": "Output format for a path export; defaults to the path's extension." }
                },
                "additionalProperties": false
            }),
            verb: VerbClass::Write,
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
    if !TOOLS.contains(&name) {
        return None;
    }
    // Tool name == bridge op; the host matches these verbatim.
    let result = match view_state::viewer_command("cube", name, args.clone()).await {
        Ok(v) => {
            // A figure export returns { image_base64: ".." } → surface as an image.
            match v.get("image_base64").and_then(|b| b.as_str()) {
                Some(b64) => ToolResult::Image {
                    data_base64: b64.to_string(),
                    mime: "image/png".into(),
                    caption: None,
                },
                None => ToolResult::Data(v),
            }
        }
        Err(e) => ToolResult::Failed(e),
    };
    Some(result)
}

/// Cube tools never enqueue write proposals — they act directly on the live viewer.
pub async fn apply(
    _s: &AppServices,
    _p: &PendingProposal,
) -> Option<Result<String, String>> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptors_unique_nonempty_and_agent_safe() {
        let d = descriptors();
        assert!(!d.is_empty());
        for desc in &d {
            assert!(!desc.name.is_empty(), "tool name must be non-empty");
            assert!(desc.agent_safe, "cube tools must all be agent-safe");
        }
        let mut names: Vec<_> = d.iter().map(|x| x.name.clone()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), d.len(), "tool names must be unique");
    }

    #[test]
    fn tool_names_match_op_table() {
        let names: Vec<String> = descriptors().iter().map(|d| d.name.clone()).collect();
        for op in TOOLS {
            assert!(names.contains(&op.to_string()), "missing descriptor for op {op}");
        }
        assert_eq!(names.len(), TOOLS.len());
    }
}
