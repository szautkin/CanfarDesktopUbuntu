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
    "set_cube_transfer",
    "show_cube_spectrum",
    "get_cube_channel_profile",
    "switch_cube_tab",
    "list_recent_cubes",
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
                    "resetCamera": { "type": "boolean", "description": "Reset the orbit camera to the default framing (applied before any az/el/dist override)." },
                    "steps": {
                        "type": "number",
                        "minimum": crate::ui::cube_volume_gl::STEPS_RANGE.0,
                        "maximum": crate::ui::cube_volume_gl::STEPS_RANGE.1,
                        "description": "Ray-march quality steps. Fewer breaks the volume into visible slabs; more costs frame time with nothing to show."
                    },
                    "spectralScale": {
                        "type": "number",
                        "minimum": crate::ui::cube_volume_gl::SPECTRAL_SCALE_RANGE.0,
                        "maximum": crate::ui::cube_volume_gl::SPECTRAL_SCALE_RANGE.1,
                        "description": "Spectral (Z) axis stretch."
                    },
                    "density": {
                        "type": "number",
                        "minimum": crate::ui::cube_volume_gl::DENSITY_MIN,
                        "description": "Volume opacity/density multiplier. Zero would render nothing."
                    },
                    "mip": { "type": "boolean", "description": "Max-intensity projection on/off." },
                    "renderMode": { "type": "string", "description": "Volume render mode: \"emission\" or \"max-intensity\" (alias for the MIP toggle)." },
                    "background": { "type": "string", "enum": ["dark", "black", "light"], "description": "3D background preset." },
                    "autoOrbit": { "type": "boolean", "description": "Idle auto-orbit on/off." },
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
        },        ToolDescriptor {
            name: "switch_cube_tab".into(),
            description: "Bring one of the open cube tabs to the front by its 0-based index (see \
                          list_open_tabs). Every other cube tool acts on the ACTIVE tab, so this is how \
                          you choose which cube they address."
                .into(),
            input_schema: json!({
                "type":"object",
                "properties": { "index": { "type":"integer", "minimum":0, "description":"0-based cube tab index" } },
                "required": ["index"],
                "additionalProperties": false
            }),
            verb: VerbClass::Write,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "list_recent_cubes".into(),
            description: "List the cubes the user has opened recently, newest first, with each path and \
                          whether the file is still present (a recent entry can outlive its file on an \
                          unmounted volume). Feed a path to open_cube."
                .into(),
            input_schema: json!({ "type":"object", "properties": {}, "additionalProperties": false }),
            verb: VerbClass::Read,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "set_cube_transfer".into(),
            description: "Edit the 3D volume's opacity transfer curve — the control that decides which \
                          value ranges are transparent and which are solid. Pass `points` (x = normalized \
                          value 0..1, y = opacity 0..1; at least two, order does not matter — they are \
                          sorted and the endpoints pinned to x=0 and x=1), or `reset: true` for the \
                          default ramp. Returns the applied curve. Live-applied."
                .into(),
            input_schema: json!({
                "type":"object",
                "properties": {
                    "points": {
                        "type":"array", "minItems":2,
                        "items": {
                            "type":"object",
                            "properties": {
                                "x": {"type":"number","minimum":0,"maximum":1},
                                "y": {"type":"number","minimum":0,"maximum":1}
                            },
                            "required":["x","y"],
                            "additionalProperties": false
                        }
                    },
                    "reset": { "type":"boolean", "description":"Restore the default opacity ramp" }
                },
                "additionalProperties": false
            }),
            verb: VerbClass::Write,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "show_cube_spectrum".into(),
            description: "Open the on-screen spectrum panel at a spaxel, as a click in the slice view \
                          does — this is what the USER sees, whereas probe_cube_spectrum only returns \
                          data. Coordinates are NATIVE cube pixels. Pass `close: true` to hide the panel. \
                          Live-applied."
                .into(),
            input_schema: json!({
                "type":"object",
                "properties": {
                    "x": { "type":"integer", "minimum":0, "description":"Native cube pixel x" },
                    "y": { "type":"integer", "minimum":0, "description":"Native cube pixel y" },
                    "close": { "type":"boolean", "description":"Hide the panel instead of opening it" }
                },
                "additionalProperties": false
            }),
            verb: VerbClass::Write,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "get_cube_channel_profile".into(),
            description: "The channel-scrubber waveform: the mean value of every spectral channel, with \
                          each channel's world value from the spectral WCS. Blank (NaN) voxels are \
                          excluded from each mean rather than counted as zero, and a wholly blank channel \
                          reports a null mean. Channel numbers are NATIVE (file) channels even when the \
                          resident volume was strided down — `downsampled` says whether that happened."
                .into(),
            input_schema: json!({ "type":"object", "properties": {}, "additionalProperties": false }),
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
    if !TOOLS.contains(&name) {
        return None;
    }
    // Tool name == bridge op; the host matches these verbatim.
    let result = match view_state::viewer_command("cube", name, args.clone()).await {
        Ok(v) => {
            // A figure export returns { imageBase64: ".." } → surface as an image.
            match v.get("imageBase64").and_then(|b| b.as_str()) {
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
pub async fn apply(_s: &AppServices, _p: &PendingProposal) -> Option<Result<String, String>> {
    None
}

#[cfg(test)]
mod tests {
    use crate::ui::cube_volume_gl::{DENSITY_MIN, SPECTRAL_SCALE_RANGE, STEPS_RANGE};

    /// Bounds the renderer ENFORCES must be the bounds the tool ADVERTISES.
    ///
    /// They were stated in prose only ("32–1024"), so a client could not
    /// validate before sending and an out-of-range value was silently clamped
    /// rather than refused — the caller believed it had set something it had
    /// not. Reading them from the same constants the clamps use is what keeps
    /// the two honest.
    #[test]
    fn the_volume_bounds_advertised_are_the_bounds_enforced() {
        let schema = descriptors()
            .into_iter()
            .find(|d| d.name == "set_cube_view")
            .expect("the tool is declared")
            .input_schema;
        let props = &schema["properties"];

        assert_eq!(
            props["steps"]["minimum"].as_f64(),
            Some(STEPS_RANGE.0 as f64)
        );
        assert_eq!(
            props["steps"]["maximum"].as_f64(),
            Some(STEPS_RANGE.1 as f64)
        );
        assert_eq!(
            props["spectralScale"]["minimum"].as_f64(),
            Some(SPECTRAL_SCALE_RANGE.0 as f64)
        );
        assert_eq!(
            props["spectralScale"]["maximum"].as_f64(),
            Some(SPECTRAL_SCALE_RANGE.1 as f64)
        );
        assert_eq!(
            props["density"]["minimum"].as_f64(),
            Some(DENSITY_MIN as f64)
        );
    }

    #[test]
    fn the_volume_bounds_are_usable_ranges() {
        assert!(STEPS_RANGE.0 < STEPS_RANGE.1, "{STEPS_RANGE:?}");
        assert!(
            SPECTRAL_SCALE_RANGE.0 < SPECTRAL_SCALE_RANGE.1,
            "{SPECTRAL_SCALE_RANGE:?}"
        );
        // Zero density renders nothing at all, so the floor has to be positive.
        assert!(DENSITY_MIN > 0.0, "{DENSITY_MIN}");
    }

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
            assert!(
                names.contains(&op.to_string()),
                "missing descriptor for op {op}"
            );
        }
        assert_eq!(names.len(), TOOLS.len());
    }
}
