//! Live per-viewer MCP tools for the native Jupyter notebook editor.
//!
//! Every tool forwards to the notebook host on the GTK main thread through the
//! view-state bridge (`viewer_command("notebook", <op>, args)`). Read tools
//! return a snapshot; mutation tools drive the active notebook tab's existing
//! mutators and return the resulting notebook state. All tools are agent-safe:
//! the notebook host is the user's live editor, not a proposal target.
//!
//! Mirrors `Mcp/Tools/Write/NotebookTools.cs` from the CanfarDesktop reference.
//! Tool names are identical to the bridge ops, so dispatch forwards by name.

use super::{ToolDescriptor, ToolResult, VerbClass};
use crate::mcp::tools::proposals::InMemoryProposalStore;
use crate::mcp::tools::str_arg;
use crate::mcp::view_state;
use crate::state::AppServices;
use serde_json::{json, Value};
use std::sync::Arc;

/// The optional `notebook` selector shared by most tools: target a specific open
/// tab by index, id (from list_open_notebooks), or file path; omit for the active tab.
fn nb_sel() -> Value {
    json!({
        "type": "string",
        "description": "Optional: target a specific open notebook by tab index, id \
                        (from list_open_notebooks), or file path; omit to target the active tab"
    })
}

fn desc(name: &str, description: &str, input_schema: Value, verb: VerbClass) -> ToolDescriptor {
    ToolDescriptor {
        name: name.to_string(),
        description: description.to_string(),
        input_schema,
        verb,
        agent_safe: true,
    }
}

pub fn descriptors() -> Vec<ToolDescriptor> {
    let sel = nb_sel();
    vec![
        desc(
            "list_open_notebooks",
            "List the notebook tabs currently OPEN in the editor: each notebook's id, title, file \
             path, active flag, dirty flag, cell count, and kernel state. Pass an id/path/index as \
             the `notebook` argument of the other tools to target a specific open notebook.",
            json!({"type":"object","properties":{},"additionalProperties":false}),
            VerbClass::Read,
        ),
        desc(
            "list_notebooks",
            "Recently opened notebooks, from the on-disk history — independent of which tabs are \
             open (use list_open_notebooks for those). Each entry carries `kind`: \
             \"notebook\" (.ipynb), \"python\" (.py) or \"markdown\" (.md), because this editor \
             opens all three as notebooks; and `exists`, since a recents list outlives its files.",
            json!({"type":"object","properties":{},"additionalProperties":false}),
            VerbClass::Read,
        ),
        desc(
            "get_notebook",
            "Read a notebook tab: id, title, file path, dirty flag, kernel state, selected cell, and \
             the list of cells (index, type, source, execution count, output count). Use \
             get_cell_output for a cell's outputs.",
            json!({"type":"object","properties":{"notebook":sel},"additionalProperties":false}),
            VerbClass::Read,
        ),
        desc(
            "get_cell_output",
            "Read the outputs of a code cell (by 0-based index): each output's type, text, \
             error/image/html flags, and `richTypes` — every MIME type that output carries \
             (text/html, image/png, text/markdown, image/svg+xml, text/latex, \
             application/json...). Binary image data is flagged in `richTypes`, never returned. \
             `richTypes` is present on every output and empty where there is nothing rich.",
            json!({"type":"object","properties":{
                "cell":{"type":"integer","minimum":0,"description":"0-based cell index"},
                "notebook":sel
            },"required":["cell"],"additionalProperties":false}),
            VerbClass::Read,
        ),
        desc(
            "get_cell_image",
            "Fetch a cell's rendered image as real image content (a matplotlib figure, a PIL \
             image, any output whose `richTypes` include image/png or image/jpeg). \
             `get_cell_output` deliberately describes images without carrying them, so a caller \
             does not spend its context on pixels it did not ask for; this is how to ask. Use \
             `output` to pick among several images in one cell (0-based, default the first).",
            json!({"type":"object","properties":{
                "cell":{"type":"integer","minimum":0,"description":"0-based cell index"},
                "output":{"type":"integer","minimum":0,
                          "description":"0-based index among that cell's IMAGE outputs"},
                "notebook":sel
            },"required":["cell"],"additionalProperties":false}),
            VerbClass::Read,
        ),
        desc(
            "check_notebook_dependencies",
            "List the third-party modules a notebook imports and which of them the kernel's Python \
             cannot import. Read-only — it installs nothing. Returns the interpreter it asked, every \
             import found, and for each missing one its module name and the pip package that \
             provides it (they differ: cv2 is opencv-python). Use before running a notebook whose \
             imports you have not seen succeed.",
            // Wrapped, like every other use of `sel`. Passed raw it made the
            // tool advertise `inputSchema: {"type":"string"}` — not an object
            // schema, which is what the spec requires and what a strict client
            // validates the whole list against.
            json!({"type":"object","properties":{"notebook":sel.clone()},"additionalProperties":false}),
            VerbClass::Read,
        ),
        desc(
            "install_notebook_dependencies",
            "Install packages into the kernel's Python with pip. Queues for the user's approval — it \
             changes their machine. Answers `installed`, and on failure pip's own error plus \
             `externallyManaged`: true when the interpreter is one the distribution manages (PEP \
             668), where a plain install cannot work. Retrying with allowSystemPythonOverride uses \
             --break-system-packages, which can leave the system Python inconsistent with its \
             package manager — ask the user before setting it.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "packages": { "type": "array", "items": { "type": "string" },
                        "description": "pip package names, as check_notebook_dependencies reports them." },
                    "allowSystemPythonOverride": { "type": "boolean",
                        "description": "Use --break-system-packages. Only after externallyManaged came back true, and only with the user's agreement." }
                },
                "required": ["packages"],
                "additionalProperties": false
            }),
            VerbClass::Write,
        ),
        desc(
            "get_kernel_state",
            "Read a notebook's kernel status (dead / starting / idle / busy / error) + kernel name. \
             Lighter than get_notebook for polling while a cell runs.",
            json!({"type":"object","properties":{"notebook":sel},"additionalProperties":false}),
            VerbClass::Read,
        ),
        desc(
            "add_cell",
            "Insert a new cell. cell_type is 'code' (default) or 'markdown'; index is the 0-based \
             position to insert at (default: append at the end).",
            json!({"type":"object","properties":{
                "cellType":{"type":"string","enum":["code","markdown"]},
                "index":{"type":"integer","minimum":0},
                "notebook":sel
            },"required":[],"additionalProperties":false}),
            VerbClass::Write,
        ),
        desc(
            "edit_cell",
            "Replace the source text of the cell at a 0-based index.",
            json!({"type":"object","properties":{
                "index":{"type":"integer","minimum":0},
                "source":{"type":"string"},
                "notebook":sel
            },"required":["index","source"],"additionalProperties":false}),
            VerbClass::Write,
        ),
        desc(
            "delete_cell",
            "Delete the cell at a 0-based index (deleting the last cell leaves one empty code cell).",
            json!({"type":"object","properties":{
                "index":{"type":"integer","minimum":0},
                "notebook":sel
            },"required":["index"],"additionalProperties":false}),
            VerbClass::Write,
        ),
        desc(
            "change_cell_type",
            "Change the type of the cell at a 0-based index to 'code' or 'markdown'.",
            json!({"type":"object","properties":{
                "index":{"type":"integer","minimum":0},
                "cellType":{"type":"string","enum":["code","markdown"]},
                "notebook":sel
            },"required":["index","cellType"],"additionalProperties":false}),
            VerbClass::Write,
        ),
        desc(
            "move_cell",
            "Move the cell at a 0-based index one position up or down.",
            json!({"type":"object","properties":{
                "index":{"type":"integer","minimum":0},
                "direction":{"type":"string","enum":["up","down"]},
                "notebook":sel
            },"required":["index","direction"],"additionalProperties":false}),
            VerbClass::Write,
        ),
        desc(
            "run_cell",
            "Execute the code cell at a 0-based index (starts the kernel if needed). Waits for \
             the cell and returns its outputs. A cell still running when `timeout` expires does \
             NOT stop — the reply carries `timedOut: true` and `running: true`, and you poll \
             get_cell_output or call interrupt_kernel.",
            json!({"type":"object","properties":{
                "index":{"type":"integer","minimum":0},
                "timeout":{"type":"number","exclusiveMinimum":0,
                           "description":"seconds to wait before returning while it keeps running (capped by the transport budget; the reply states waitedSeconds)"},
                "notebook":sel
            },"required":["index"],"additionalProperties":false}),
            VerbClass::Write,
        ),
        desc(
            "run_all_cells",
            "Run every code cell in order, stopping at the first kernel failure. Waits for the \
             sweep like run_cell does: `running: false` means every output is final. If it is \
             still going when `timeout` expires the reply carries `timedOut: true` and \
             `running: true` — poll get_kernel_state until idle, then read get_cell_output. \
             `cellsWithErrors` lists the 0-based cells that raised.",
            json!({"type":"object","properties":{
                "timeout":{"type":"number","exclusiveMinimum":0,
                           "description":"seconds to wait before returning while it keeps running (capped by the transport budget; the reply states waitedSeconds)"},
                "notebook":sel
            },"additionalProperties":false}),
            VerbClass::Write,
        ),
        desc(
            "clear_cell_outputs",
            "Clear all cell outputs and execution counts.",
            json!({"type":"object","properties":{"notebook":sel},"additionalProperties":false}),
            VerbClass::Write,
        ),
        desc(
            "start_kernel",
            "Start the Python kernel and wait for it to become ready (running a cell also auto-starts it).",
            json!({"type":"object","properties":{"notebook":sel},"additionalProperties":false}),
            VerbClass::Write,
        ),
        desc(
            "interrupt_kernel",
            "Interrupt the kernel (stops a long-running cell).",
            json!({"type":"object","properties":{"notebook":sel},"additionalProperties":false}),
            VerbClass::Write,
        ),
        desc(
            "restart_kernel",
            "Restart the Python kernel from a clean state.",
            json!({"type":"object","properties":{"notebook":sel},"additionalProperties":false}),
            VerbClass::Write,
        ),
        desc(
            "save_notebook",
            "Save a notebook to the LOCAL filesystem. With no path it saves to the current file \
             (fails for an unsaved notebook — pass a full local .ipynb path to save-as). This \
             never writes to VOSpace/ARC: to put a notebook there, save it locally first and then \
             upload it with upload_vospace_file.",
            json!({"type":"object","properties":{
                "path":{"type":"string","description":"Optional absolute LOCAL .ipynb path to \
                     save-as. Not a VOSpace path — a `vos:` or `arc:` path is refused rather than \
                     written to a local directory of the same name."},
                "notebook":sel
            },"required":[],"additionalProperties":false}),
            VerbClass::Write,
        ),
        desc(
            "open_notebook",
            "Open a notebook/script file (.ipynb / .py, full local path) in the editor as a new tab \
             and switch to it. Returns the resulting notebook state.",
            json!({"type":"object","properties":{
                "path":{"type":"string","description":"Full local path to a .ipynb/.py file"}
            },"required":["path"],"additionalProperties":false}),
            VerbClass::Write,
        ),
        desc(
            "create_notebook",
            "Open a new empty (Untitled) notebook tab and switch to it. Save it with save_notebook \
             (provide a path).",
            json!({"type":"object","properties":{},"additionalProperties":false}),
            VerbClass::Write,
        ),
        desc(
            "create_analysis_notebook",
            "Build a ready-to-run astropy analysis notebook for a downloaded observation (by CADC \
             publisher id, from the Research library): a metadata header, a FITS + WCS load cell, \
             and a template stub. template is 'image' (zscale quick-look, default), 'photometry' \
             (aperture photometry), or 'cube' (moment map + spectrum). Writes the .ipynb under the \
             app data dir and opens it in the editor.",
            json!({"type":"object","properties":{
                "publisherId":{"type":"string","description":"The observation's CADC publisher id (from list_downloaded_observations)"},
                // The enum comes from the builder's own list: an advertised
                // template the builder does not know is a tool call that fails,
                // and one it knows but does not advertise is refused by the
                // schema. "auto" is not a stub, it is "pick one for me".
                "template":{"type":"string","enum":crate::helpers::analysis_notebook::TEMPLATES.iter().chain(["auto"].iter()).collect::<Vec<_>>(),"description":"Template stub (default image)"}
            },"required":["publisherId"],"additionalProperties":false}),
            VerbClass::Write,
        ),
    ]
}

pub async fn dispatch(
    name: &str,
    services: &AppServices,
    args: &Value,
    _proposals: &Arc<InMemoryProposalStore>,
) -> Option<ToolResult> {
    // Not a bridge op: builds a fresh notebook from Research metadata, writes it,
    // then opens it through the bridge.
    if name == "create_analysis_notebook" {
        return Some(create_analysis_notebook(services, args).await);
    }

    // Not a bridge op: reads the on-disk recent-notebooks history (independent of
    // which tabs are open), mirroring the Windows `ListNotebooksTool`.
    if name == "list_notebooks" {
        let recents = crate::services::notebook_store::NotebookStore::new().load();
        let notebooks: Vec<Value> = recents
            .iter()
            .map(|r| {
                let path = std::path::Path::new(&r.path);
                json!({
                    "path": r.path,
                    "name": r.name,
                    "openedAt": r.opened_at.to_rfc3339(),
                    // What the entry IS. A `.md` in this list looked like a
                    // stray file, and the instinct was to filter it out — but
                    // the notebook editor really does open Markdown and Python
                    // as notebooks, so hiding them would hide something the app
                    // can do. Naming the kind is the honest answer.
                    "kind": crate::helpers::notebook_formats::NotebookFormat::for_path(path).kind(),
                    // A recents list outlives the files in it. Saying so beats
                    // a caller discovering it through a failed open.
                    "exists": path.exists(),
                })
            })
            .collect();
        return Some(ToolResult::Data(json!({
            "count": notebooks.len(),
            "notebooks": notebooks,
        })));
    }

    // Tool names are identical to the bridge ops.
    let op = match name {
        "list_open_notebooks"
        | "get_notebook"
        | "get_cell_output"
        | "get_cell_image"
        | "get_kernel_state"
        | "add_cell"
        | "edit_cell"
        | "delete_cell"
        | "change_cell_type"
        | "move_cell"
        | "run_cell"
        | "run_all_cells"
        | "clear_cell_outputs"
        | "start_kernel"
        | "interrupt_kernel"
        | "restart_kernel"
        | "save_notebook"
        | "open_notebook"
        | "create_notebook"
        // Announced since the dependency work went in, and dispatched by
        // nobody: the host implemented them as `check_dependencies` /
        // `install_dependencies` while the catalogue advertised the
        // `_notebook_` spelling, so both answered "no such tool". The host ops
        // are renamed to match rather than mapped here — the invariant above
        // is worth more than an exception to it.
        | "check_notebook_dependencies"
        | "install_notebook_dependencies" => name,
        _ => return None,
    };

    match view_state::viewer_command("notebook", op, args.clone()).await {
        Ok(v) => Some(crate::mcp::agent_image::promote(
            v,
            crate::mcp::agent_image::ImageLimits::from_settings(),
        )),
        Err(e) => Some(ToolResult::Failed(e)),
    }
}

/// Build an astropy analysis notebook for a downloaded observation, write it to
/// the app data dir as an `.ipynb`, then open it in the editor via the bridge.
///
/// The file is written regardless of whether the viewer bridge is available, so
/// a headless/agent caller still gets a usable notebook path back (`opened:
/// false`) when no window is open.
async fn create_analysis_notebook(services: &AppServices, args: &Value) -> ToolResult {
    let pid = str_arg(args, "publisher_id");
    if pid.is_empty() {
        return ToolResult::Failed("publisher_id is required".to_string());
    }
    let template = args
        .get("template")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // Resolve the observation from the Research library (publisher id first, then
    // local id as a fallback).
    let list = services.observation_store.load_async().await;
    let obs = match list
        .iter()
        .find(|o| o.publisher_id == pid)
        .or_else(|| list.iter().find(|o| o.id == pid))
    {
        Some(o) => o.clone(),
        None => {
            return ToolResult::Failed(format!(
                "no downloaded observation with publisher id '{}'",
                pid
            ))
        }
    };

    let doc = crate::helpers::analysis_notebook::build_analysis_notebook(&obs, template.as_deref());
    let stem = crate::helpers::analysis_notebook::suggested_file_stem(&obs);

    // Serialize + write the .ipynb off the async executor (blocking fs I/O).
    let written = tokio::task::spawn_blocking(move || -> Result<String, String> {
        let json = serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())?;
        let dir = directories::ProjectDirs::from("net", "canfar", "Verbinal")
            .map(|d| d.data_dir().join("analysis-notebooks"))
            .unwrap_or_else(|| std::path::PathBuf::from("analysis-notebooks"));
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let path = dir.join(format!("{stem}.ipynb"));
        std::fs::write(&path, json).map_err(|e| e.to_string())?;
        Ok(path.to_string_lossy().into_owned())
    })
    .await;

    let path = match written {
        Ok(Ok(p)) => p,
        Ok(Err(e)) => return ToolResult::Failed(format!("failed to write analysis notebook: {e}")),
        Err(e) => return ToolResult::Failed(format!("notebook write task failed: {e}")),
    };

    // Try to open it live; a missing bridge is not a failure — the file exists.
    match view_state::viewer_command("notebook", "open_notebook", json!({ "path": path.clone() }))
        .await
    {
        Ok(v) => ToolResult::Data(json!({
            "created": true,
            "opened": true,
            "path": path,
            "publisherId": pid,
            "notebook": v,
        })),
        Err(e) => ToolResult::Data(json!({
            "created": true,
            "opened": false,
            "path": path,
            "publisherId": pid,
            "note": format!("notebook written but not opened in the editor: {e}"),
        })),
    }
}

#[cfg(test)]
mod wiring_tests {
    /// Every notebook tool this module advertises is one it will dispatch.
    ///
    /// `check_notebook_dependencies` and `install_notebook_dependencies` were
    /// announced in `tools/list` for a week and answered "no such tool" when
    /// called: the viewer host implemented them under a shorter name, and the
    /// op match — which maps tool name to bridge op 1:1 — had no arm for
    /// either, so both fell through to `None`.
    ///
    /// Nothing failed. The descriptors were valid, the handlers existed, and
    /// the two halves simply never met. An agent following the catalogue is
    /// the only thing that touches that seam.
    #[test]
    fn every_advertised_notebook_tool_is_dispatchable() {
        let source = include_str!("notebook.rs");
        let code = crate::testing::without_comments(crate::testing::code(source));

        // Scope to `dispatch` itself. The first version searched the whole
        // file, which contains `descriptors()` — so every tool "matched" its
        // own declaration and the guard passed with both tools unwired again.
        let dispatch = code
            .find("pub async fn dispatch(")
            .expect("the dispatch fn");
        let body = &code[dispatch..];

        // The op match, and whatever handled a name before reaching it.
        let match_at = body.find("let op = match name {").expect("the op match");
        let end = body[match_at..].find("_ =>").expect("its fallthrough") + match_at;
        let handled = &body[..end];

        let mut unwired = Vec::new();
        for d in super::descriptors() {
            let quoted = format!("\"{}\"", d.name);
            if !handled.contains(&quoted) {
                unwired.push(d.name.clone());
            }
        }

        assert!(
            unwired.is_empty(),
            "notebook tool(s) advertised in tools/list that dispatch to nothing — \
             an agent calling them gets \"no such tool\": {unwired:#?}"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptors_unique_nonempty_and_agent_safe() {
        let d = descriptors();
        assert!(!d.is_empty());
        assert!(
            d.iter().all(|x| !x.name.is_empty()),
            "names must be non-empty"
        );
        let mut names: Vec<_> = d.iter().map(|x| x.name.clone()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), d.len(), "tool names must be unique");
        assert!(
            d.iter().all(|x| x.agent_safe),
            "all notebook tools are agent-safe"
        );
    }

    #[test]
    fn create_analysis_notebook_descriptor_present_and_write() {
        let d = descriptors()
            .into_iter()
            .find(|x| x.name == "create_analysis_notebook")
            .expect("create_analysis_notebook descriptor present");
        assert_eq!(d.verb, VerbClass::Write);
        assert!(d.agent_safe);
        // publisher_id is the required argument.
        let required = d.input_schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v == "publisherId"));
    }
}
