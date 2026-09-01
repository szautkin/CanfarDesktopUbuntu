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
use crate::mcp::tools::proposals::InMemoryProposalStore;
use crate::mcp::view_state::viewer_command;
use crate::state::AppServices;
use serde_json::{json, Value};
use std::sync::Arc;

pub fn descriptors() -> Vec<ToolDescriptor> {
    let empty = json!({"type":"object","properties":{},"additionalProperties":false});
    // `hdu` is 0-BASED, matching the reference and astropy: 0 is the primary
    // HDU. It used to be CFITSIO-native 1-based, which silently addressed the
    // wrong extension for anyone following either of those conventions. The
    // shift needed a header-only reader first — the image loader refuses an HDU
    // with fewer than two axes, and a multi-extension file's primary usually has
    // none — which `fits_loader::read_hdu_header` now provides.
    let with_hdu = json!({
        "type":"object",
        "properties": {
            "localPath": { "type":"string", "description":"Local filesystem path to a FITS file" },
            "hdu": { "type":"integer", "minimum":0, "description":"0-based HDU index (default 0, the primary HDU) — astropy's numbering. NOTE: the viewer's extension selector and set_fits_view are 1-BASED, so what get_fits_view lists as `2: SCI` is hdu 1 here; its `headerHdu` field gives this number directly. The reply echoes `extname` so you can check you got the extension you meant." }
        },
        "required": ["localPath"], "additionalProperties": false
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
            name: "get_fits_image".into(),
            description: "SEE the FITS viewer's working area — the active tab exactly as the user \
                          is looking at it, with its pan, zoom, rotation, colormap, stretch, cut \
                          levels, crosshair and any blink overlay. Returns the picture as image \
                          content, plus the view it was captured from and the scale between that \
                          view and the returned raster, so a position in the image can be turned \
                          back into image or sky coordinates. Use get_fits_view for the numbers \
                          alone; use this to look. Errors if no FITS is open."
                .into(),
            input_schema: empty.clone(),
            verb: VerbClass::Read,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "set_fits_view".into(),
            description: "Steer the 2D FITS viewer's ACTIVE tab — every control the UI exposes. \
                          Display: stretch, colormap, black/white cut levels (minCut/maxCut in physical pixel \
                          units — read the current data range from get_fits_view), zoom (percent, 100 = 1:1), \
                          North-Up, reset. HDU: `hdu` switches the displayed extension (image HDUs only — \
                          get_fits_view lists them). Crosshair: crosshairX/Y places it at a 0-based display \
                          pixel (works WITHOUT a WCS; fits_goto_coordinate is the RA/Dec route), \
                          clearCrosshair removes it. Navigation: centerX/Y pans the viewport to a display \
                          pixel. Cross-tab: syncZoom and linkedCrosshair (the toolbar toggles). Panels: \
                          showHeaderPanel (header + image info), showBookmarksPanel (saved coordinates). \
                          Only the fields you pass change. Returns the resulting view state. Live-applied."
                .into(),
            input_schema: json!({
                "type":"object",
                "properties": {
                    "zoomPercent": {
                        "type":"number",
                        "minimum": crate::ui::fits_canvas::ZOOM_SCALE_RANGE.0 * 100.0,
                        "maximum": crate::ui::fits_canvas::ZOOM_SCALE_RANGE.1 * 100.0,
                        "description":"Zoom percent (100 = 1:1)"
                    },
                    "centerX": { "type":"number", "description":"Image x-pixel to centre the viewport on" },
                    "centerY": { "type":"number", "description":"Image y-pixel to centre the viewport on" },
                    "stretch": { "type":"string", "enum": crate::ui::fits_viewer::STRETCH_NAMES },
                    "colormap": { "type":"string", "enum": crate::ui::fits_viewer::COLORMAP_NAMES },
                    "minCut": { "type":"number", "description":"Black point, in the image's own pixel units (BUNIT). Use cutPercentile instead to say it as a percentile, which is scale-free." },
                    "maxCut": { "type":"number", "description":"White point, in the image's own pixel units." },
                    "cutPreset": {
                        "type":"string",
                        "enum":["percentile","zscale","minmax"],
                        "description":"Set both cut levels the way astronomers do. \"zscale\" is the IRAF/DS9 default and usually the right choice: a percentile cut asks where most pixels are, which is the wrong question for faint structure under a few bright stars. \"percentile\" is p0.5-p99.5. \"minmax\" uses the full data range and on a frame with one saturated star shows almost nothing."
                    },
                    "minCutPercentile": { "type":"number", "minimum":0, "maximum":100, "description":"Black point as a percentile of this image's pixels (0.5 is the default). Scale-free: it means the same thing on any image." },
                    "maxCutPercentile": { "type":"number", "minimum":0, "maximum":100, "description":"White point as a percentile (99.5 is the default)." },
                    "northUp": { "type":"boolean" },
                    "reset": { "type":"boolean", "description":"Reset stretch + zoom/pan to defaults" },
                    "hdu": { "type":"integer", "minimum":0, "description":"Switch the displayed HDU/extension (image HDUs only; get_fits_view lists them)" },
                    "crosshairX": { "type":"integer", "minimum":0, "description":"Place the crosshair at this 0-based display pixel (pass with crosshairY; works without a WCS)" },
                    "crosshairY": { "type":"integer", "minimum":0, "description":"Place the crosshair at this 0-based display pixel (pass with crosshairX)" },
                    "clearCrosshair": { "type":"boolean", "description":"Remove the placed crosshair" },
                    "syncZoom": { "type":"boolean", "description":"The sync-zoom toolbar toggle: match angular extent across tabs" },
                    "linkedCrosshair": { "type":"boolean", "description":"The linked-crosshair toolbar toggle: share the crosshair across tabs by sky position" },
                    "showHeaderPanel": { "type":"boolean", "description":"Show/hide the header + image-info panel" },
                    "showBookmarksPanel": { "type":"boolean", "description":"Show/hide the saved-coordinates panel" }
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
                          FITS BUNIT) when present. A BLANKED pixel (NaN/Inf in the data) omits `value` \
                          entirely and sets `blanked: true` — do not read a missing value as zero. Errors \
                          if no FITS is open or (x, y) is out of range."
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
            name: "annotate_fits".into(),
            description: "DRAW on the FITS viewer, to show a person where you mean. A rect or \
                          circle around a subject, a callout (a shape with a leader line to a \
                          label set clear of it), or text alone. Place it with `ra`/`dec` in \
                          degrees where the image has WCS — that anchor survives reopening the \
                          file and lands correctly on another image of the same field — or with \
                          image pixel `x`/`y`. Marks appear on the user's screen and in \
                          get_fits_image, and are labelled as yours. Read the view first: \
                          get_fits_image shows what they are looking at, and its `view` gives \
                          the coordinates to aim at."
                .into(),
            input_schema: json!({
                "type":"object",
                "properties": {
                    "kind": {
                        "type":"string", "enum": ["rect","circle","callout","text"],
                        "description": "Default circle; callout and text need `text`."
                    },
                    "ra": {"type":"number","description":"Degrees. With `dec`, anchors to the sky."},
                    "dec": {"type":"number","description":"Degrees."},
                    "x": {"type":"number","description":"Image pixel, when there is no WCS."},
                    "y": {"type":"number","description":"Image pixel."},
                    "text": {"type":"string","description":"The label. Required for callout and text."},
                    "colour": {"type":"string","description":"Ink, as #rrggbb. Also accepted as `color`. A mark keeps this in the file and in an exported figure; picking it out on screen highlights it too, which is session state and never reaches an export — so use a colour, not selection, to make one mark stand out in a handout."},
                    "fontSize": {"type":"number","minimum":6,"maximum":72,"description":"Label size in device pixels, not scaled by zoom. 11 by default; an export scales it."},
                    "bold": {"type":"boolean","description":"Draw the label bold."},
                    "stroke": {"type":"number","minimum":0.5,"maximum":20,"description":"Outline width in device pixels, not scaled by zoom. 1 by default. Raise it to make one mark carry across a printed page."},
                    "radius": {
                        "type":"number",
                        "description":"Half-size, in IMAGE PIXELS unless you pass `ra`/`dec`, \
                                       in which case it is in degrees like they are. A mark \
                                       placed by pixel is stored against the sky when the image \
                                       has WCS, and the radius is converted with it — so \
                                       `radius: 80` beside `x`/`y` is eighty pixels, as you \
                                       would expect. Omit it for a size that is visible on this \
                                       image whatever its pixel scale."
                    }
                },
                "additionalProperties": false
            }),
            verb: VerbClass::Write,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "list_fits_annotations".into(),
            description: "Every mark on the active FITS tab — id, kind, text, where it is \
                          anchored, and whether a person or an agent drew it. Use it to find an \
                          id for remove_annotation, or to see what is already marked before \
                          adding more."
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
            name: "close_fits_tab".into(),
            description: "Close an open FITS tab by its 0-based index (see list_open_tabs), or the \
                          ACTIVE tab when no index is given. close_active_tab is app-level and does \
                          not reach the FITS viewer, so this is how a FITS tab is closed. Returns \
                          which tab closed and how many remain."
                .into(),
            input_schema: json!({
                "type":"object",
                "properties": {
                    "tabIndex": { "type":"integer", "minimum":0, "description":"0-based FITS tab index from list_open_tabs; omit for the active tab" }
                },
                "additionalProperties": false
            }),
            verb: VerbClass::Write,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "switch_fits_tab".into(),
            description: "Bring one of the open FITS tabs to the front by its 0-based index (see \
                          list_open_tabs). Every other FITS tool acts on the ACTIVE tab, so this is how \
                          you choose which file they address. Returns the newly active tab's view state."
                .into(),
            input_schema: json!({
                "type":"object",
                "properties": {
                    "index": { "type":"integer", "minimum":0, "description":"0-based FITS tab index from list_open_tabs" }
                },
                "required": ["index"],
                "additionalProperties": false
            }),
            verb: VerbClass::Write,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "blink_fits_tabs".into(),
            description: "Blink-compare two open FITS tabs for the user: a WCS-aligned fade between the \
                          ACTIVE tab and a partner. action `start` (withTabIndex = the partner's 0-based \
                          index from list_open_tabs; both images need a valid WCS), `pause` / `resume` \
                          (freeze the fade on the current frame), or `stop` (restores the active tab's \
                          pre-blink zoom and centre). intervalMs (500–5000) sets the fade cycle speed and \
                          applies with any action. Returns the blink state. Live-applied."
                .into(),
            input_schema: json!({
                "type":"object",
                "properties": {
                    "action": { "type":"string", "enum":["start","stop","pause","resume"] },
                    "withTabIndex": { "type":"integer", "minimum":0, "description":"Partner tab index; required for `start`" },
                    "intervalMs": { "type":"integer", "minimum":500, "maximum":5000, "description":"Fade cycle length in milliseconds" }
                },
                "required": ["action"],
                "additionalProperties": false
            }),
            verb: VerbClass::Write,
            agent_safe: true,
        },
        ToolDescriptor {
            name: "export_fits_figure".into(),
            description: "Save what the FITS viewer is showing — or part of it — as a PNG or a \
                          PDF. The marks are in it, because they are drawn by the same function \
                          that draws the screen; the editing grips are not, and neither is the \
                          ink that says which mark happens to be selected. With `path` it writes \
                          a file and returns where; without one it returns the image, which \
                          costs an agent context, so prefer a path when a person wants the file. \
                          The result is a captioned figure unless you pass `plate: false`, so a \
                          person opening it can see where on the sky it points."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "region": {
                        "description": "What to export. \"view\" (default) is what is on screen — the picture get_fits_image returns. \"image\" is the whole frame. A box in image pixels is {x, y, width, height}. A box on the sky is {ra, dec, widthArcsec, heightArcsec}, which needs a WCS and is the form that cuts the same region from another frame of the same field.",
                        "oneOf": [
                            { "type": "string", "enum": ["view", "image"] },
                            {
                                "type": "object",
                                "properties": {
                                    "x": {"type":"number"}, "y": {"type":"number"},
                                    "width": {"type":"number","exclusiveMinimum":0},
                                    "height": {"type":"number","exclusiveMinimum":0}
                                },
                                "required": ["x","y","width","height"],
                                "additionalProperties": false
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "ra": {"type":"number"}, "dec": {"type":"number"},
                                    "widthArcsec": {"type":"number","exclusiveMinimum":0},
                                    "heightArcsec": {"type":"number","exclusiveMinimum":0}
                                },
                                "required": ["ra","dec","widthArcsec","heightArcsec"],
                                "additionalProperties": false
                            }
                        ]
                    },
                    "plate": {
                        "type":"boolean",
                        "description":"A publication figure (default): the picture framed, with the region's real sky coordinates captioned under it, a colorbar in the image's own units, and the cut levels and stretch it was drawn at — what the Export button produces. Set false for the bare pixels with nothing around them."
                    },
                    "scale": {"type":"integer","minimum":1,"maximum":4,"description":"Pixel multiplier, as the Export dialog offers. The output keeps the region's shape."},
                    "transparent": {"type":"boolean","description":"Leave the ground unpainted, so anything the image does not cover keeps its alpha. PNG only."},
                    "format": {"type":"string","enum":["png","pdf"],"description":"Defaults to png, or to the extension of `path`."},
                    "path": {"type":"string","description":"Absolute local path to write. Omit to get the image back instead."}
                },
                "additionalProperties": false
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
        | "get_fits_image"
        | "set_fits_view"
        | "probe_fits_pixel"
        | "fits_goto_coordinate"
        | "list_fits_bookmarks"
        | "annotate_fits"
        | "list_fits_annotations"
        | "save_fits_bookmark"
        | "delete_fits_bookmark"
        | "switch_fits_tab"
        | "close_fits_tab"
        | "blink_fits_tabs"
        | "export_fits_figure" => Some(to_tool_result(
            viewer_command("fits", name, args.clone()).await,
        )),
        // Stateless ops — read the file directly, NOT through the bridge.
        "get_fits_header" => Some(to_tool_result(get_fits_header(args))),
        "get_fits_wcs" => Some(to_tool_result(get_fits_wcs(args))),
        _ => None,
    }
}

/// Map a JSON result into a `ToolResult`, promoting an `imageBase64` payload to
/// an image.
fn to_tool_result(r: Result<Value, String>) -> ToolResult {
    match r {
        Ok(v) => crate::mcp::agent_image::promote(
            v,
            crate::mcp::agent_image::ImageLimits::from_settings(),
        ),
        Err(e) => ToolResult::Failed(e),
    }
}

// ─── Stateless disk readers ──────────────────────────────────────────────────

/// The local FITS path. Declared as `localPath` (the reference's name); `path`
/// is still accepted, since Verbinal shipped that spelling first.
fn require_path(args: &Value) -> Result<String, String> {
    let from = |key: &str| super::opt_str_arg(args, key).filter(|s: &String| !s.is_empty());
    let path = from("localPath")
        .or_else(|| from("path"))
        .ok_or_else(|| "localPath is required".to_string())?;
    // cfitsio opens files, not URLs. Without this the failure surfaces as a
    // cfitsio error about a file that does not exist — true, and no help at all
    // to someone whose file plainly does exist, just not here.
    crate::helpers::local_path::reject_remote(&path, crate::helpers::local_path::FETCH_IT_FIRST)?;
    Ok(path)
}

fn get_fits_header(args: &Value) -> Result<Value, String> {
    let path = require_path(args)?;
    let hdu = requested_hdu(args);
    let (_, ordered, width, height) = crate::helpers::fits_loader::read_hdu_header(
        std::path::Path::new(&path),
        cfitsio_hdu(hdu),
    )?;
    let cards: Vec<Value> = ordered
        .iter()
        .map(|(k, v, c)| json!({ "keyword": k, "value": v, "comment": c }))
        .collect();
    // Say WHICH extension this is. `hdu` here is 0-based (astropy's numbering,
    // because this reads the file directly) while the viewer's selector and
    // `set_fits_view` are 1-based — so the same extension has two numbers, and
    // an agent that carried one across silently got the neighbouring one. The
    // name it actually read makes that visible in the reply instead.
    let extname = cards
        .iter()
        .find(|c| c["keyword"] == "EXTNAME")
        .and_then(|c| c["value"].as_str())
        .unwrap_or("")
        .to_string();
    Ok(json!({
        "localPath": path,
        "hdu": hdu,
        "extname": extname,
        // What the viewer calls this extension, so the two numberings can be
        // lined up without guessing.
        "viewerHdu": hdu + 1,
        "count": cards.len(),
        "cards": cards,
        // Beyond the reference's record: read straight off NAXIS1/NAXIS2, so it
        // costs nothing and saves an agent a second call to size the frame.
        // Both are 0 for a table or header-only HDU.
        "width": width,
        "height": height,
    }))
}

/// The 0-based HDU index the caller asked for, defaulting to the primary.
fn requested_hdu(args: &Value) -> i64 {
    args.get("hdu").and_then(|v| v.as_i64()).unwrap_or(0).max(0)
}

/// Translate a 0-based wire index to CFITSIO's 1-based absolute number.
fn cfitsio_hdu(hdu: i64) -> usize {
    (hdu as usize).saturating_add(1)
}

/// Key names follow the reference's `Output` record exactly, including its
/// unusual capitalisation: .NET's camelCase policy lowercases only the leading
/// capital, so `CType1` → `cType1` and `CrPix1` → `crPix1`, while `Cd1_1` →
/// `cd1_1`. They look inconsistent because they are — but they are what an agent
/// written against the Windows app reads.
///
/// Every key is present in every response, `null` where a value does not apply.
/// The derived three used to vanish when the solution was invalid, so a client
/// could not tell "no WCS" from "a field I forgot to handle".
fn get_fits_wcs(args: &Value) -> Result<Value, String> {
    let path = require_path(args)?;
    let hdu = requested_hdu(args);
    let (header, _, width, height) = crate::helpers::fits_loader::read_hdu_header(
        std::path::Path::new(&path),
        cfitsio_hdu(hdu),
    )?;
    // The WCS comes from the header alone; the canonical parser lives on
    // `FitsImageData`, so it is run over an empty image (the same route
    // `cube_wcs::from_header` takes). No pixels are read.
    let wcs = crate::models::FitsImageData::new(0, 0, Vec::new(), header).wcs;
    Ok(wcs_payload(&path, hdu, width, height, wcs.as_ref()))
}

/// The `get_fits_wcs` payload, split from the file read so the shape can be
/// tested without a FITS fixture on disk.
fn wcs_payload(
    path: &str,
    hdu: i64,
    width: usize,
    height: usize,
    wcs: Option<&crate::models::fits_image::WcsInfo>,
) -> Value {
    let mut out = json!({
        "localPath": path,
        "hdu": hdu,
        "width": width,
        "height": height,
        "isValid": false,
        "isApproximate": false,
        "solutionKind": "none",
        "cType1": Value::Null,
        "cType2": Value::Null,
        "projection": Value::Null,
        "crPix1": Value::Null,
        "crPix2": Value::Null,
        "crVal1": Value::Null,
        "crVal2": Value::Null,
        "cd1_1": Value::Null,
        "cd1_2": Value::Null,
        "cd2_1": Value::Null,
        "cd2_2": Value::Null,
        "pixelScaleArcsec": Value::Null,
        "northAngle": Value::Null,
        "hasParityFlip": Value::Null,
    });
    if let Some(w) = wcs {
        let valid = w.is_valid();
        out["isValid"] = json!(valid);
        out["isApproximate"] = json!(w.is_approximate);
        out["solutionKind"] = json!(w.solution_kind());
        out["cType1"] = json!(w.ctype1);
        out["cType2"] = json!(w.ctype2);
        out["projection"] = json!(format!("{:?}", w.proj()));
        out["crPix1"] = json!(w.crpix1);
        out["crPix2"] = json!(w.crpix2);
        out["crVal1"] = json!(w.crval1);
        out["crVal2"] = json!(w.crval2);
        out["cd1_1"] = json!(w.cd1_1);
        out["cd1_2"] = json!(w.cd1_2);
        out["cd2_1"] = json!(w.cd2_1);
        out["cd2_2"] = json!(w.cd2_2);
        // The reference leaves the derived quantities null unless the solution
        // is valid — an angle computed from a degenerate CD matrix is noise.
        if valid {
            out["pixelScaleArcsec"] = json!(w.pixel_scale_arcsec());
            out["northAngle"] = json!(w.north_angle());
            out["hasParityFlip"] = json!(w.has_parity_flip());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every field of the reference's `get_fits_wcs` Output record, camelCased
    /// by its serializer. Transcribed from `Read/VoSpaceFitsReadTools.cs`.
    const REFERENCE_WCS_FIELDS: &[&str] = &[
        "localPath",
        "hdu",
        "isValid",
        "isApproximate",
        "cType1",
        "cType2",
        "projection",
        "crPix1",
        "crPix2",
        "crVal1",
        "crVal2",
        "cd1_1",
        "cd1_2",
        "cd2_1",
        "cd2_2",
        "pixelScaleArcsec",
        "northAngle",
        "hasParityFlip",
    ];

    /// A WCS with a plain 1"/pixel scale and no rotation.
    fn sample_wcs() -> crate::models::fits_image::WcsInfo {
        crate::models::fits_image::WcsInfo {
            crpix1: 512.0,
            crpix2: 512.0,
            crval1: 10.68,
            crval2: 41.27,
            cd1_1: -1.0 / 3600.0,
            cd1_2: 0.0,
            cd2_1: 0.0,
            cd2_2: 1.0 / 3600.0,
            ctype1: "RA---TAN".into(),
            ctype2: "DEC--TAN".into(),
            ..Default::default()
        }
    }

    #[test]
    fn the_wcs_payload_carries_every_field_the_reference_promises() {
        let payload = wcs_payload("/data/a.fits", 0, 1024, 1024, Some(&sample_wcs()));
        let obj = payload.as_object().expect("an object");
        for field in REFERENCE_WCS_FIELDS {
            assert!(obj.contains_key(*field), "`{field}` is missing");
        }
        assert_eq!(payload["isValid"], true);
        assert_eq!(payload["cType1"], "RA---TAN");
        assert_eq!(payload["crPix1"], 512.0);
        // Not `path`, `is_valid`, `ctype1`, `crpix1` — the snake_case names this
        // tool used to emit, which no reference-written client reads.
        for leaked in ["path", "is_valid", "ctype1", "crpix1", "pixel_scale_arcsec"] {
            assert!(
                !obj.contains_key(leaked),
                "`{leaked}` is the old internal name"
            );
        }
    }

    #[test]
    fn a_file_without_wcs_still_reports_the_full_shape() {
        // Keys must not vanish: a client could not otherwise tell "no WCS
        // solution" from "a field the server forgot".
        let payload = wcs_payload("/data/a.fits", 0, 64, 64, None);
        let obj = payload.as_object().expect("an object");
        for field in REFERENCE_WCS_FIELDS {
            assert!(obj.contains_key(*field), "`{field}` vanished without a WCS");
        }
        assert_eq!(payload["isValid"], false);
        assert!(payload["cType1"].is_null());
        assert!(payload["pixelScaleArcsec"].is_null());
    }

    #[test]
    fn an_invalid_solution_leaves_the_derived_values_null() {
        // A degenerate CD matrix has no meaningful scale or North angle, so the
        // reference reports null rather than a number computed from noise.
        let mut wcs = sample_wcs();
        wcs.cd1_1 = 0.0;
        wcs.cd1_2 = 0.0;
        wcs.cd2_1 = 0.0;
        wcs.cd2_2 = 0.0;

        let payload = wcs_payload("/data/a.fits", 0, 64, 64, Some(&wcs));
        assert_eq!(payload["isValid"], false);
        assert!(payload["pixelScaleArcsec"].is_null());
        assert!(payload["northAngle"].is_null());
        assert!(payload["hasParityFlip"].is_null());
        // The raw header values are still reported — they were read, after all.
        assert_eq!(payload["cType1"], "RA---TAN");
    }

    #[test]
    fn the_zoom_range_advertised_is_the_zoom_range_enforced() {
        // Three ranges disagreed: the canvas clamped 1–10000%, the scroll wheel
        // 10–5000%, and this tool advertised 5–2000%. "How far can I zoom" had a
        // different answer depending on whether you dragged, typed or asked over
        // MCP — and the advertised one matched neither.
        use crate::ui::fits_canvas::ZOOM_SCALE_RANGE;

        let schema = descriptors()
            .into_iter()
            .find(|d| d.name == "set_fits_view")
            .expect("the tool is declared")
            .input_schema;
        let zoom = &schema["properties"]["zoomPercent"];

        // The tool speaks percent; the canvas stores a scale factor.
        assert_eq!(zoom["minimum"].as_f64(), Some(ZOOM_SCALE_RANGE.0 * 100.0));
        assert_eq!(zoom["maximum"].as_f64(), Some(ZOOM_SCALE_RANGE.1 * 100.0));
    }

    #[test]
    fn the_zoom_range_spans_one_to_one() {
        // 100% — one image pixel per screen pixel — has to be reachable, or the
        // most useful zoom level of all is outside the range.
        use crate::ui::fits_canvas::ZOOM_SCALE_RANGE;
        assert!(
            ZOOM_SCALE_RANGE.0 < 1.0 && ZOOM_SCALE_RANGE.1 > 1.0,
            "{ZOOM_SCALE_RANGE:?}"
        );
    }

    #[test]
    fn hdu_zero_means_the_primary_hdu() {
        // 0-based on the wire, CFITSIO's 1-based underneath. Getting this
        // backwards addresses the wrong extension in every multi-extension
        // file — and reads plausible data, so nothing looks wrong.
        assert_eq!(cfitsio_hdu(0), 1, "wire 0 is the primary HDU");
        assert_eq!(cfitsio_hdu(1), 2, "wire 1 is the first extension");
        assert_eq!(cfitsio_hdu(7), 8);
    }

    #[test]
    fn an_omitted_hdu_defaults_to_the_primary() {
        assert_eq!(requested_hdu(&json!({})), 0);
        assert_eq!(requested_hdu(&json!({ "hdu": 2 })), 2);
    }

    #[test]
    fn a_negative_hdu_is_clamped_rather_than_wrapping() {
        // `hdu: -1` would otherwise become a huge usize through the +1 and read
        // as "past the end" instead of the obvious mistake it is.
        assert_eq!(requested_hdu(&json!({ "hdu": -1 })), 0);
        assert_eq!(cfitsio_hdu(requested_hdu(&json!({ "hdu": -5 }))), 1);
    }

    #[test]
    fn the_declared_hdu_minimum_matches_the_convention() {
        let schema = descriptors()
            .into_iter()
            .find(|d| d.name == "get_fits_header")
            .expect("the tool is declared")
            .input_schema;
        assert_eq!(
            schema["properties"]["hdu"]["minimum"], 0,
            "0-based indexing must be advertised, or an agent adds one itself"
        );
    }

    #[test]
    fn the_declared_path_argument_is_the_one_the_tool_reads() {
        let schema = descriptors()
            .into_iter()
            .find(|d| d.name == "get_fits_wcs")
            .expect("the tool is declared")
            .input_schema;
        assert_eq!(schema["required"][0], "localPath");

        assert_eq!(
            require_path(&json!({ "localPath": "/data/a.fits" })).unwrap(),
            "/data/a.fits"
        );
        // Back-compat with the spelling Verbinal shipped first.
        assert_eq!(
            require_path(&json!({ "path": "/data/a.fits" })).unwrap(),
            "/data/a.fits"
        );
        assert!(require_path(&json!({})).is_err());
        assert!(require_path(&json!({ "localPath": "   " })).is_err());
    }

    #[test]
    fn a_vospace_path_is_refused_before_cfitsio_sees_it() {
        // cfitsio would report a file that does not exist. It does exist — the
        // caller is simply not on the machine that has it, and the error has to
        // say which of those two things went wrong.
        for key in ["localPath", "path"] {
            let err = require_path(&json!({ key: "vos://cadc.nrc.ca~arc/home/a/x.fits" }))
                .expect_err(key);
            assert!(err.contains("download_vospace_file"), "{err}");
        }
    }

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

    /// Every control in the viewer's DISPLAY section has a tool input.
    ///
    /// That section is where an image is made readable, and it is the one an
    /// agent most needs: a frame at the wrong cut levels is a blank rectangle,
    /// and an agent looking at `get_fits_image` cannot tell that from an empty
    /// field. Asserted against the SCHEMA, because a field the handler honours
    /// but never advertises may as well not exist.
    #[test]
    fn every_display_control_is_reachable_by_an_agent() {
        let d = descriptors();
        let set = d
            .iter()
            .find(|t| t.name == "set_fits_view")
            .expect("set_fits_view is advertised");
        let props = set.input_schema["properties"]
            .as_object()
            .expect("an object schema");
        // Colormap and stretch pick the mapping; the two cut levels bound it;
        // the preset sets both the way astronomers do; reset puts it back.
        for field in [
            "colormap",
            "stretch",
            "minCut",
            "maxCut",
            "minCutPercentile",
            "maxCutPercentile",
            "cutPreset",
            "reset",
        ] {
            assert!(
                props.contains_key(field),
                "the DISPLAY panel can set `{field}` and an agent cannot"
            );
        }
    }

    /// The cut presets an agent may ask for are the ones the panel offers.
    ///
    /// Three names in two places drift: the dropdown gained ZScale and the
    /// tool would have kept accepting only what it knew.
    #[test]
    fn the_advertised_cut_presets_are_the_ones_the_panel_offers() {
        let d = descriptors();
        let set = d
            .iter()
            .find(|t| t.name == "set_fits_view")
            .expect("advertised");
        let presets = set.input_schema["properties"]["cutPreset"]["enum"]
            .as_array()
            .expect("cutPreset is an enum");
        for name in ["percentile", "zscale", "minmax"] {
            assert!(
                presets.iter().any(|v| v == name),
                "the panel offers a cut preset `{name}` the tool refuses"
            );
        }
    }

    /// A cut level can be set as a percentile, which is the scale-free way.
    ///
    /// A data value is meaningless without the frame's range — a black point
    /// of 30 is most of a JWST frame's useful span and nothing at all on a
    /// MegaCam one — so an agent that has not read `dataMin`/`dataMax` cannot
    /// pick one. The percentile inputs exist so it does not have to.
    #[test]
    fn a_cut_can_be_asked_for_as_a_percentile() {
        let d = descriptors();
        let set = d
            .iter()
            .find(|t| t.name == "set_fits_view")
            .expect("advertised");
        for field in ["minCutPercentile", "maxCutPercentile"] {
            let p = &set.input_schema["properties"][field];
            assert_eq!(p["minimum"], 0, "{field} should be bounded at 0");
            assert_eq!(p["maximum"], 100, "{field} should be bounded at 100");
        }
    }

    /// Every region form an agent can ask in is advertised.
    ///
    /// The four are not interchangeable: a dragged box is screen-space, an
    /// agent's is image pixels or sky, and the sky one is the only form that
    /// cuts the same field out of a second frame. A schema that offered only
    /// some of them would quietly remove the reason the tool exists.
    #[test]
    fn every_region_form_is_advertised() {
        let d = descriptors();
        let t = d
            .iter()
            .find(|t| t.name == "export_fits_figure")
            .expect("export_fits_figure is advertised");
        let forms = t.input_schema["properties"]["region"]["oneOf"]
            .as_array()
            .expect("region is a oneOf");
        let named = forms
            .iter()
            .find_map(|f| f["enum"].as_array())
            .expect("a named form");
        for name in ["view", "image"] {
            assert!(named.iter().any(|v| v == name), "no `{name}` region");
        }
        let has_required = |fields: &[&str]| {
            forms.iter().any(|f| {
                f["required"]
                    .as_array()
                    .map(|r| fields.iter().all(|k| r.iter().any(|v| v == k)))
                    .unwrap_or(false)
            })
        };
        assert!(
            has_required(&["x", "y", "width", "height"]),
            "no image-pixel region form"
        );
        assert!(
            has_required(&["ra", "dec", "widthArcsec", "heightArcsec"]),
            "no sky region form"
        );
    }

    /// The scale an agent may ask for is the scale the panel offers.
    ///
    /// Two lists in two places drift, and the failure is quiet: the same export
    /// would succeed from the dialog and come back smaller for an agent.
    #[test]
    fn the_advertised_scale_matches_the_dialog() {
        let d = descriptors();
        let t = d
            .iter()
            .find(|t| t.name == "export_fits_figure")
            .expect("advertised");
        let max = t.input_schema["properties"]["scale"]["maximum"]
            .as_i64()
            .expect("a stated maximum");
        let ui_max = crate::ui::export_dialog::EXPORT_SCALES
            .iter()
            .copied()
            .max()
            .unwrap_or(1) as i64;
        assert_eq!(
            max, ui_max,
            "the tool caps scale at {max}, the dialog offers {ui_max}"
        );
    }

    /// An agent can ask for the figure a person gets, and knows which it got.
    ///
    /// The tool returned a bare crop while the Export button produced a
    /// captioned plate, and nothing in either surface said so — an agent
    /// handing a file to someone had no idea it carried no coordinates. Both
    /// tools take `plate` now, both say what their default is, and both echo
    /// which one they produced.
    #[test]
    fn both_export_tools_can_produce_the_figure_the_button_does() {
        let fits = descriptors();
        let cube = crate::mcp::tools::cube::descriptors();
        for (name, d) in [("export_fits_figure", &fits), ("export_cube_figure", &cube)] {
            let t = d
                .iter()
                .find(|t| t.name == name)
                .unwrap_or_else(|| panic!("{name} is advertised"));
            let plate = &t.input_schema["properties"]["plate"];
            assert_eq!(
                plate["type"], "boolean",
                "{name} cannot be asked for the figure"
            );
            let desc = plate["description"].as_str().unwrap_or_default();
            assert!(
                desc.to_lowercase().contains("default"),
                "{name}'s `plate` does not say which way it defaults, and the two \
                 tools default differently"
            );
        }
    }
}
