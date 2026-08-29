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
use crate::mcp::tools::proposals::InMemoryProposalStore;
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
    "get_cube_image",
    "annotate_cube",
    "list_cube_annotations",
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
                "Update any subset of the active cube's view — every control the panel exposes. \
                 Display: colormap, stretch, window levels (windowLo/windowHi, or windowPreset \
                 minmax/p99), background preset. Camera: orbit az/el, dolly dist, camera reset, \
                 idle auto-orbit. Volume: quality steps, spectral (Z) stretch, opacity density, \
                 MIP / render mode. Overlays: showCaptions, showSlicePlane. Plus the slice-plane \
                 channel. Returns the resulting view."
                    .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "azimuth": { "type": "number", "description": "Camera azimuth in radians. Also accepted as `az`." },
                    "elevation": {
                        "type": "number",
                        "minimum": crate::ui::cube_volume_gl::ELEVATION_RANGE.0,
                        "maximum": crate::ui::cube_volume_gl::ELEVATION_RANGE.1,
                        "description": "Camera elevation in radians — stops short of the poles, where the orbit basis degenerates. Also accepted as `el`."
                    },
                    "distance": {
                        "type": "number",
                        "minimum": crate::ui::cube_volume_gl::DISTANCE_RANGE.0,
                        "maximum": crate::ui::cube_volume_gl::DISTANCE_RANGE.1,
                        "description": "Camera distance. Closer clips into the volume; further leaves it a speck. Also accepted as `dist`."
                    },
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
                    "background": { "type": "string", "enum": crate::ui::cube_volume_gl::BACKGROUND_NAMES, "description": "3D background preset." },
                    "colormap": {
                        "type": "string",
                        "enum": crate::helpers::cube_colormaps::NAMES,
                        "description": "Colour map applied to both the volume and the slice view."
                    },
                    "stretch": {
                        "type": "string",
                        "enum": crate::ui::cube_viewer::STRETCH_NAMES,
                        "description": "Display stretch curve."
                    },
                    "windowLo": {
                        "type": "number", "minimum": 0, "maximum": 1,
                        "description": "Display window low cut, normalised 0..1."
                    },
                    "windowHi": {
                        "type": "number", "minimum": 0, "maximum": 1,
                        "description": "Display window high cut, normalised 0..1."
                    },
                    "windowPreset": {
                        "type": "string", "enum": ["minmax", "p99"],
                        "description": "Window shortcut: full range, or the 1st-99th percentile."
                    },
                    "showCaptions": { "type": "boolean", "description": "WCS axis-caption overlay." },
                    "showSlicePlane": { "type": "boolean", "description": "Slice-plane marker in the volume." },
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
            name: "annotate_cube".into(),
            description: "DRAW on the cube's 3D volume, to show a person where you mean. A ring \
                          or box around a feature, a callout with a label, or text. Anchored to a \
                          VOXEL — `x`/`y` in pixels and `z` as the channel — so the mark is \
                          pinned to the data and rotates with the cube. Marks appear on the \
                          user's screen and in get_cube_image, and are labelled as yours. Read \
                          get_cube_image first to see the view, and probe_cube_spectrum or \
                          get_cube_view to find the voxel worth marking."
                .into(),
            input_schema: serde_json::json!({
                "type":"object",
                "properties": {
                    "kind": {
                        "type":"string", "enum": ["rect","circle","callout","text"],
                        "description": "Default circle; callout and text need `text`."
                    },
                    "x": {"type":"number","description":"Voxel X (pixel along the first axis)."},
                    "y": {"type":"number","description":"Voxel Y."},
                    "z": {
                        "type":"number",
                        "description":"Channel. Omit for the channel the viewer is showing."
                    },
                    "text": {"type":"string","description":"The label. Required for callout and text."},
                    "radius": {
                        "type":"number",
                        "description":"Half-size in voxels. Omit it for a size that is visible \
                                       on this cube whatever its dimensions."
                    }
                },
                "required": ["x","y"],
                "additionalProperties": false
            }),
            verb: VerbClass::Write,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "list_cube_annotations".into(),
            description: "Every mark on the active cube — id, kind, text, the voxel it is \
                          anchored to, and whether a person or an agent drew it."
                .into(),
            input_schema: serde_json::json!({
                "type":"object","properties":{},"additionalProperties":false
            }),
            verb: VerbClass::Read,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "get_cube_image".into(),
            description: "SEE the cube viewer's working area — the active cube exactly as the \
                          user is looking at it: the 3D volume WITH its wireframe box, WCS axis \
                          captions and slice-plane marker, or the 2D slice when that is the \
                          visible mode. Returns the picture as image content plus the view it \
                          was captured from and the scale between that view and the raster, so a \
                          position in the image can be turned back into cube coordinates. \
                          export_cube_figure is a figure EXPORT and returns the render without \
                          the overlay; use this to look at what is on screen. Errors if no cube \
                          is open, or if GL is unavailable."
                .into(),
            input_schema: serde_json::json!({
                "type":"object","properties":{},"additionalProperties":false
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
                    "path": { "type": "string", "description": "Absolute output path on the LOCAL filesystem — not a VOSpace path. When set, the figure is written to disk (PNG/PDF) and the path is returned instead of base64." },
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
        // A reply carrying `imageBase64` becomes an image; anything else is
        // data. One reader for every family — this arm used to announce
        // `image/png` for whatever bytes it was handed.
        Ok(v) => crate::mcp::agent_image::promote(
            v,
            crate::mcp::agent_image::ImageLimits::from_settings(),
        ),
        Err(e) => ToolResult::Failed(e),
    };
    Some(result)
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
    /// Every display control the cube panel offers must be reachable over MCP,
    /// and readable back.
    ///
    /// Colormap, stretch, the window levels and the two overlay toggles are all
    /// controls the user can change; none of them was declared or parsed, so
    /// "100% UI coverage" was not true for the cube. A control an agent can set
    /// but not read is only half-useful too — it cannot tell what it changed
    /// FROM, so it cannot put the user's view back.
    #[test]
    fn every_cube_display_control_is_settable() {
        let schema = descriptors()
            .into_iter()
            .find(|d| d.name == "set_cube_view")
            .expect("the tool is declared")
            .input_schema;
        let props = &schema["properties"];

        for control in [
            "colormap",
            "stretch",
            "windowLo",
            "windowHi",
            "windowPreset",
            "showCaptions",
            "showSlicePlane",
        ] {
            assert!(
                props.get(control).is_some(),
                "`{control}` is a control the panel offers but the tool does not"
            );
        }
    }

    #[test]
    fn the_cube_enums_are_the_lists_the_viewer_parses() {
        let schema = descriptors()
            .into_iter()
            .find(|d| d.name == "set_cube_view")
            .expect("the tool is declared")
            .input_schema;
        let names = |key: &str| -> Vec<String> {
            schema["properties"][key]["enum"]
                .as_array()
                .unwrap_or_else(|| panic!("`{key}` should declare an enum"))
                .iter()
                .map(|v| v.as_str().unwrap_or_default().to_string())
                .collect()
        };

        assert_eq!(
            names("colormap"),
            crate::helpers::cube_colormaps::NAMES.to_vec()
        );
        assert_eq!(
            names("stretch"),
            crate::ui::cube_viewer::STRETCH_NAMES.to_vec()
        );
        // The stretch list is indexed into `StretchMode::from_index`, so its
        // length has to match the mode count — an extra name would select
        // Linear, silently applying a curve the caller did not ask for.
        assert_eq!(
            crate::ui::cube_viewer::STRETCH_NAMES.len(),
            crate::helpers::cube_slice::StretchMode::ALL.len()
        );
    }

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
        // The density floor is pinned at compile time beside the constant
        // itself — a runtime assertion on a `const` can only ever restate it.
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
