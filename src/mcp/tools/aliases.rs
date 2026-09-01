//! Alternate wire names for built-in tools.
//!
//! Every tool has exactly one **canonical** name — the one CanfarDesktop 1.3.3
//! advertises — and dispatch happens under that name alone. This module is the
//! single place that maps every other accepted spelling onto it, so no tool
//! module has to know an alias exists.
//!
//! Two kinds live here, distinguished by whether they appear in `tools/list`:
//!
//! * **Advertised** aliases the reference itself ships (the macOS-parity trio).
//!   An agent written against Verbinal-macOS calls `upload_to_vospace`; the
//!   reference exposes that name with macOS wording, so we must too, or the
//!   manifest diverges.
//! * **Deprecated** aliases: names this port used before it aligned with the
//!   reference. They still dispatch, so an agent or saved prompt written against
//!   an older Verbinal keeps working, but they are hidden from `tools/list` —
//!   otherwise the advertised name set could never equal the reference's.
//!
//! Deprecated entries are scheduled for removal one release after the rename
//! that introduced them.

use crate::mcp::tools::ToolDescriptor;

/// One accepted alternate spelling of a canonical tool name.
pub struct ToolAlias {
    /// The name a client may call.
    pub alias: &'static str,
    /// The canonical tool the call is routed to.
    pub canonical: &'static str,
    /// `Some(description)` when the alias is advertised in `tools/list` under
    /// its own wording (the reference ships it); `None` for a hidden,
    /// deprecated shim that dispatches but never advertises.
    pub advertised_description: Option<&'static str>,
}

/// Every accepted alias, in one table.
pub const ALIASES: &[ToolAlias] = &[
    // ── macOS-parity aliases (advertised — the reference ships these too) ────
    ToolAlias {
        alias: "upload_to_vospace",
        canonical: "upload_file_to_vospace",
        advertised_description: Some(
            "Upload a downloaded observation's local file to a VOSpace path. Use \
             `upload_text_to_vospace` instead if your source is in-conversation text (script, \
             config, JSON) rather than a downloaded file. Synchronous with a 150s applier \
             deadline; a stuck transfer surfaces as `backendError` with the deadline named, not a \
             silent hang. For files > ~100 MB on slow links: the underlying transfer can outlast \
             the MCP transport timeout — on `Request timed out`, re-poll `list_vospace_path` after \
             30–60s, the bytes are often there.",
        ),
    },
    ToolAlias {
        alias: "download_from_vospace",
        canonical: "download_vospace_file",
        advertised_description: Some(
            "Download a VOSpace file to the user's Downloads folder. Synchronous with a 150s \
             applier deadline; a stuck transfer surfaces as `backendError` with the deadline \
             named, not a silent hang. For files > ~100 MB on slow links: the underlying transfer \
             can outlast the MCP transport timeout — on `Request timed out` re-check the Downloads \
             folder before retrying, the bytes are often there.",
        ),
    },
    ToolAlias {
        alias: "vospace_mkdir",
        canonical: "create_vospace_folder",
        advertised_description: Some("Create a folder under a VOSpace path."),
    },
    // ── Deprecated Verbinal names (hidden; kept so older callers still work) ──
    ToolAlias {
        alias: "get_node",
        canonical: "get_vospace_node",
        advertised_description: None,
    },
    ToolAlias {
        alias: "read_file",
        canonical: "read_vospace_file",
        advertised_description: None,
    },
    ToolAlias {
        alias: "get_quota",
        canonical: "get_storage_quota",
        advertised_description: None,
    },
    ToolAlias {
        alias: "upload_text",
        canonical: "upload_text_to_vospace",
        advertised_description: None,
    },
    ToolAlias {
        alias: "create_folder",
        canonical: "create_vospace_folder",
        advertised_description: None,
    },
    ToolAlias {
        alias: "set_acl",
        canonical: "set_vospace_acl",
        advertised_description: None,
    },
    ToolAlias {
        alias: "delete_node",
        canonical: "delete_vospace_node",
        advertised_description: None,
    },
    ToolAlias {
        alias: "list_storage",
        canonical: "list_vospace_path",
        advertised_description: None,
    },
    ToolAlias {
        alias: "list_observations",
        canonical: "list_downloaded_observations",
        advertised_description: None,
    },
    ToolAlias {
        alias: "clear_outputs",
        canonical: "clear_cell_outputs",
        advertised_description: None,
    },
    ToolAlias {
        alias: "run_all",
        canonical: "run_all_cells",
        advertised_description: None,
    },
    ToolAlias {
        alias: "list_fits_bookmark",
        canonical: "list_fits_bookmarks",
        advertised_description: None,
    },
    ToolAlias {
        alias: "get_session_logs",
        canonical: "get_headless_job_logs",
        advertised_description: None,
    },
    ToolAlias {
        alias: "get_session_events",
        canonical: "get_headless_job_events",
        advertised_description: None,
    },
];

/// Resolve an incoming tool name to the canonical name dispatch uses.
///
/// Returns `name` unchanged when it is already canonical (or unknown) — an
/// unknown name must survive intact so the router can answer "no such tool"
/// naming what the caller actually asked for.
pub fn canonical(name: &str) -> &str {
    ALIASES
        .iter()
        .find(|a| a.alias == name)
        .map(|a| a.canonical)
        .unwrap_or(name)
}

/// Descriptors for the advertised aliases, each derived from its canonical
/// tool's descriptor so schema and verb class can never drift apart.
///
/// An alias whose canonical tool is missing from `canonical_descriptors` is
/// skipped rather than guessed at; [`tests::every_alias_resolves`] fails the
/// build if that ever happens.
pub fn advertised_descriptors(canonical_descriptors: &[ToolDescriptor]) -> Vec<ToolDescriptor> {
    ALIASES
        .iter()
        .filter_map(|a| {
            let description = a.advertised_description?;
            let target = canonical_descriptors
                .iter()
                .find(|d| d.name == a.canonical)?;
            Some(ToolDescriptor {
                name: a.alias.to_string(),
                description: description.to_string(),
                input_schema: target.input_schema.clone(),
                verb: target.verb,
                agent_safe: target.agent_safe,
            })
        })
        .collect()
}

/// Every wire name this port has ever answered to.
///
/// Not derived from [`ALIASES`] — written down separately on purpose. Derived,
/// it would shrink whenever an entry was deleted, which is precisely the event
/// it exists to catch.
///
/// QA report #1 (#10) called this churn: an agent's saved workflow says
/// `get_quota`, the catalogue now says `get_storage_quota`, and nothing
/// connects the two. Dispatch already bridges it — verified against a live
/// server, all three cited names still answer — but the module says deprecated
/// entries are "scheduled for removal one release after the rename", and the
/// day one is removed, every prompt written against it fails with "no such
/// tool" and no explanation.
///
/// A name is cheap. Removing one from this list is a decision about somebody
/// else's saved work, and it should read like one.
#[cfg(test)]
const ONCE_ADVERTISED: &[&str] = &[
    "clear_outputs",
    "create_folder",
    "delete_node",
    "get_node",
    "get_quota",
    "get_session_events",
    "get_session_logs",
    "list_fits_bookmark",
    "list_observations",
    "list_storage",
    "read_file",
    "run_all",
    "set_acl",
    "upload_text",
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::tools::router::McpToolRouter;

    #[test]
    fn canonical_maps_aliases_and_passes_others_through() {
        assert_eq!(canonical("get_node"), "get_vospace_node");
        assert_eq!(canonical("vospace_mkdir"), "create_vospace_folder");
        // Already canonical.
        assert_eq!(canonical("get_vospace_node"), "get_vospace_node");
        // Unknown names survive intact so "no such tool: x" names what was asked.
        assert_eq!(canonical("not_a_tool"), "not_a_tool");
    }

    /// Every alias must point at a tool that actually exists, or a caller using
    /// the old name gets a baffling "no such tool" for a name we do advertise.
    /// A name this port has answered to keeps answering.
    ///
    /// Deleting an alias is invisible from inside: the other guards only check
    /// the entries that are still there, so a removal passes everything. What
    /// breaks is somebody's saved prompt, a release later, with "no such tool".
    #[test]
    fn a_name_this_port_has_advertised_never_stops_resolving() {
        let real: std::collections::HashSet<String> = McpToolRouter::canonical_descriptors()
            .into_iter()
            .map(|d| d.name)
            .collect();

        let mut broken = Vec::new();
        for name in ONCE_ADVERTISED {
            let target = canonical(name);
            if target == *name {
                broken.push(format!("{name}: no longer maps to anything"));
            } else if !real.contains(target) {
                broken.push(format!("{name} -> {target}, which is not a tool"));
            }
        }

        assert!(
            broken.is_empty(),
            "wire name(s) this port used to answer to that would now fail with \
             \"no such tool\". Removing one is a decision about somebody else's \
             saved work — if it is deliberate, take it out of ONCE_ADVERTISED in \
             the same commit: {broken:#?}"
        );
    }

    #[test]
    fn every_alias_resolves_to_a_real_tool() {
        let names: Vec<String> = McpToolRouter::canonical_descriptors()
            .into_iter()
            .map(|d| d.name)
            .collect();
        for a in ALIASES {
            assert!(
                names.iter().any(|n| n == a.canonical),
                "alias `{}` points at `{}`, which is not a registered tool",
                a.alias,
                a.canonical
            );
        }
    }

    /// The reference's exact advertised name set, embedded at build time.
    const REFERENCE_TOOLS: &str = include_str!("../../../data/canfardesktop_1.3.3_tools.txt");

    fn reference_names() -> Vec<String> {
        REFERENCE_TOOLS
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(str::to_string)
            .collect()
    }

    /// Tools the reference advertises that this build has not ported YET, with
    /// the plan phase that will close each one out.
    ///
    /// This list is a **ratchet, and may only ever shrink**. Delete an entry the
    /// moment its tool lands; `advertised_names_match_the_reference` fails if a
    /// name here is actually implemented, so a stale entry cannot linger.
    /// Tools Verbinal advertises that CanfarDesktop 1.3.3 does not, each with the
    /// reason it exists here.
    ///
    /// The parity guard treats an extra tool as a bug — a rename gone the wrong
    /// way, or something that ought to be ported upstream. This list is for the
    /// third case its own comment names: a tool the reference has not got yet.
    /// Deliberate divergence stays visible instead of the guard being loosened,
    /// and an entry the reference later gains is itself reported.
    const VERBINAL_FIRST: &[(&str, &str)] = &[
    (
        "describe_tap_schema",
        "Neither app tells an agent what the CAOM2 tables contain. An agent writing ADQL had \
         two table names and one join, both from a sentence in a tool description, and had to \
         guess every column — caom2.Plane alone has 78. The service publishes all of it in \
         TAP_SCHEMA, with prose and units and UCDs, and even states in words that Plane joins \
         Observation on obsID.",
    ),
    (
        "validate_adql_query",
        "The service refuses `FROM caom2.Observation JOIN caom2.Plane ON Plane.obsID=…` with \
         \"Column [obsID] is ambiguous\", and until now the only way to find that out was to \
         run it — a round trip, and on a broad query a slow one. describe_tap_schema tells an \
         agent what the columns ARE; this tells it whether the query it wrote uses them in a \
         way the service accepts. Neither reference app has it because neither validates \
         before sending; ours does, in the editor and in execute_adql_query, and this is the \
         same check offered on its own.",
    ),
    (
        "show_search_row_detail",
        "Clicking a row in the results table opens a dialog with every column of it — the \
         forty-one a query returns, not the dozen the grid shows. An agent could read those \
         values (get_search_results with allColumns) and could not put them in front of the \
         person it was explaining them to. Paired with set_search_results_view's selectRow, \
         which highlights the row so \"this one\" means something on screen. The reference has \
         neither because its agent surface does not steer the window.",
    ),
    (
        "show_observation_detail",
        "Every other control in the Search results table has a tool — sort, filter, page, \
         rows per page, column visibility and units, CSV and TSV export — and each row's three \
         buttons have one for the DATA behind them: get_preview_image, download_observation, \
         get_observation_caom2. What had no tool was the row's Details button itself, so an \
         agent could read an observation's metadata but not put it in front of the person it \
         was explaining it to. The reference has no equivalent because its agent surface does \
         not steer the window; ours does, and set_search_focus is the precedent.",
    ),
    (
        "export_fits_figure",
        "The reference exports a cube figure and nothing else; the FITS viewer had no export \
         at all, in either app. An astronomer who has marked up a frame wants that frame in a \
         slide, and an agent that has drawn on one wants to hand over a picture rather than a \
         description of it. The region forms — a box in image pixels or on the sky — are what \
         make it more than a screenshot: the sky form cuts the same field out of a second \
         frame.",
    ),
    (
        "close_cube_tab",
        "The same hole as `close_fits_tab`, left open on the other viewer. A cube could be \
         opened and switched to and never closed, so an agent working through a list of cubes \
         piled up tabs it had no way to clear — and each one holds a decoded volume in memory. \
         The FITS side gained this and the cube was not revisited.",
    ),
    (
        "close_fits_tab",
        "The reference closes tabs only through the window chrome. `close_active_tab` is \
         app-level and never reached a module's own tabs — it answered `closed: false` for \
         every call with no reason, and the documented switch-then-close sequence could not \
         work because `switch_fits_tab` moves the viewer's focus and not the app's. An agent \
         that opens FITS tabs needs a way to close them.",
    ),
    (
        "list_apps",
        "The reference has 147 tools and no map of them. `tools/list` is 96 KB — about 24 000 \
         tokens, measured — and an agent reads all of it before it starts, then chooses worse for \
         having more to choose between. This is the ~17 areas and what each is for, small enough \
         to keep in context, with `describe_app` to fetch one area's tools when a task needs \
         them. It does not shrink `tools/list`; it makes the 147 navigable.",
    ),
    (
        "man",
        "One tool in full — its whole description, every argument, its area and the tools beside \
         it. The reference has no such page: an agent unsure of a tool's arguments can only \
         re-read the whole catalogue, and after an error it does not understand it has nowhere to \
         look. It is also what would make shorter descriptions in `tools/list` survivable, since \
         the detail has somewhere to live.",
    ),
    (
        "search_tools",
        "The other half of the map, for the common case: a model that knows what it wants to do \
         and not which of the seventeen areas owns it. Neither app has anything to answer \
         \"which tool draws a region?\" with, so the alternative is reading every description.",
    ),
    (
        "annotate_cube",
        "Pointing, in three dimensions. A mark is anchored to a VOXEL, so it rotates with the cube \
         and stays on the feature it describes from any angle — which is the only way a mark on a \
         volume can be useful. The reference has nothing like it, and an agent describing a \
         feature in a 3D cube in prose is asking a person to find it twice.",
    ),
    (
        "list_cube_annotations",
        "What is already marked on the cube, and by whom.",
    ),
    (
        "get_cube_image",
        "The reference exports a cube figure and has no way to show one to an agent. \
         `export_cube_figure` returns the volume render ALONE — the wireframe box, the WCS axis \
         captions and the slice-plane marker are a separate overlay widget drawn on top of it — \
         so an agent handed that export got the data without the frame of reference the user \
         reads it by, and nothing said so. This composites the two the way the widgets are \
         stacked, and is the step before an agent can mark a feature in a cube for a person.",
    ),
    (
        "update_annotation",
        "An agent could draw a mark and delete a mark and nothing in between, so correcting one \
         meant destroying it and drawing another with a new id — and any reference it had already \
         given a person was then wrong. This changes the label, the position or the size in place.",
    ),
    (
        "remove_annotation",
        "One tool for both viewers: an id identifies a mark uniquely, and a caller should not have \
         to know — and often would not know — which viewer is holding it.",
    ),
    (
        "clear_annotations",
        "Deletes a viewer's marks including the USER's, which nothing undoes, so it is a separate \
         tool from remove_annotation rather than a flag on it.",
    ),
    (
        "annotate_fits",
        "The reference has no way for an agent to point at anything. It can describe a source in \
         prose and the person has to find it themselves, which for a crowded field is most of the \
         work. This draws on the image they are already looking at — a ring, a box, a labelled \
         callout — and the mark is attributed to the agent rather than appearing as if the user \
         had made it. It is the other half of get_fits_image: seeing a viewer is only useful if \
         you can point back at it.",
    ),
    (
        "select_annotation",
        "An agent could draw marks and change them and had no way to say WHICH one it meant — so \
         \"the source I ringed, second from the left\" was left to a person to work out, which is \
         exactly the work marking it was supposed to save. This highlights one on the image and in \
         the sidebar list.",
    ),
    (
        "list_fits_annotations",
        "What is already marked, and by whom. Without it an agent cannot find the id of a mark to \
         remove, and cannot tell its own marks from the user's before adding more.",
    ),
    (
        "get_fits_image",
        "The reference shows the FITS viewer to a person and has no way to show it to anything \
         else. Twelve tools steer that viewer — pan, zoom, colormap, cut levels, crosshair — and \
         an agent using them was working blind, with `get_fits_view` reporting the NUMBERS of a \
         picture it could not see. This returns the working area itself, drawn by the same \
         function that draws it on screen, and is the step an agent has to have before it can \
         mark a region on that image and draw a person's attention to it.",
    ),
    (
        "get_cell_image",
        "Neither app can hand a cell's rendered figure to a caller that is not a GUI. The \
         reference holds the live figure object and paints it into its own window; over a tool \
         boundary there is nothing but a description — `hasImage: true` and \
         `<Figure size 640x480>`. An agent with vision could be looking at the plot it just \
         asked for and instead gets a sentence about one. The bytes are kept OUT of \
         `get_cell_output` on purpose, since inlining base64 into every read would spend a \
         caller's context on pixels it did not ask for; this is the explicit way to ask.",
    ),
    (
        "check_notebook_dependencies",
        "The reference has the scanner (Helpers/Notebook/DependencyScanner.cs) but only behind its \
         notebook UI. An agent asked to run a notebook could not find out what it would need \
         until an import failed mid-cell.",
    ),
    (
        "install_notebook_dependencies",
        "The other half of the same gap, and it needs Linux behaviour the reference has no reason \
         to have: pip refuses to write into a distribution-managed Python (PEP 668), so the tool \
         reports `externallyManaged` and takes an explicit override rather than failing opaquely.",
    ),
    (
        "get_job_status",
        "The reference applies a download inside the tool call, so a 332 MB \
     observation times the client out and the transfer vanishes with no id, no \
     progress and no error. Long applies here run as background jobs, which \
     only helps if the caller can ask about them.",
    )];

    const NOT_YET_PORTED: &[&str] = &[];

    /// **The wire contract.** Everything CanfarDesktop 1.3.3 advertises, we must
    /// advertise under the same name — otherwise a prompt or agent written
    /// against the reference calls a tool this build does not have.
    ///
    /// Three separate failures, each naming the offending tools:
    ///  * **extra** — we advertise something the reference doesn't and
    ///    [`VERBINAL_FIRST`] does not account for. A rename gone the wrong way,
    ///    or a deliberate addition nobody wrote down. Parity is a floor: adding
    ///    a tool is allowed, adding one silently is not.
    ///  * **unexpectedly missing** — a tool vanished that isn't on the porting
    ///    backlog. Catches an accidental rename or deletion.
    ///  * **stale backlog** — a tool is listed as unported but actually exists.
    ///    Forces `NOT_YET_PORTED` to shrink as the phases land.
    #[test]
    fn advertised_names_match_the_reference() {
        let ours: std::collections::BTreeSet<String> = McpToolRouter::all_descriptors()
            .into_iter()
            .filter(|d| d.agent_safe)
            .map(|d| d.name)
            .collect();
        let theirs: std::collections::BTreeSet<String> = reference_names().into_iter().collect();
        let pending: std::collections::BTreeSet<String> =
            NOT_YET_PORTED.iter().map(|s| s.to_string()).collect();

        let extra: Vec<&String> = ours
            .difference(&theirs)
            .filter(|name| {
                !VERBINAL_FIRST
                    .iter()
                    .any(|(tool, _)| *tool == name.as_str())
            })
            .collect();
        assert!(
            extra.is_empty(),
            "we advertise {} tool(s) CanfarDesktop 1.3.3 does not: {extra:?}. If the addition is \
             deliberate, put it in VERBINAL_FIRST with the reason.",
            extra.len()
        );

        // And the allowance itself has to stay honest: an entry that the
        // reference has since gained is no longer a divergence, and leaving it
        // listed hides the next accidental one behind it.
        let absorbed: Vec<&str> = VERBINAL_FIRST
            .iter()
            .map(|(tool, _)| *tool)
            .filter(|tool| theirs.contains(*tool))
            .collect();
        assert!(
            absorbed.is_empty(),
            "listed as Verbinal-only but the reference has them now: {absorbed:?}"
        );
        let unadvertised: Vec<&str> = VERBINAL_FIRST
            .iter()
            .map(|(tool, _)| *tool)
            .filter(|tool| !ours.contains(*tool))
            .collect();
        assert!(
            unadvertised.is_empty(),
            "listed as Verbinal-only but we do not advertise them: {unadvertised:?}"
        );

        let missing: std::collections::BTreeSet<String> =
            theirs.difference(&ours).cloned().collect();
        let unexpected: Vec<&String> = missing.difference(&pending).collect();
        assert!(
            unexpected.is_empty(),
            "tool(s) missing from tools/list that are NOT on the porting backlog — \
             did a rename or deletion go wrong? {unexpected:?}"
        );

        let stale: Vec<&String> = pending.difference(&missing).collect();
        assert!(
            stale.is_empty(),
            "NOT_YET_PORTED lists tool(s) that are already implemented — remove them: {stale:?}"
        );
    }

    /// Every name in a schema's `required` list must exist in its `properties`.
    ///
    /// These are the same identifier written twice, so a rename that touches only
    /// one side produces a schema demanding an argument the tool never reads —
    /// and a strict client refuses the call outright.
    #[test]
    fn every_required_argument_is_a_declared_property() {
        for d in McpToolRouter::all_descriptors() {
            let schema = &d.input_schema;
            let Some(required) = schema.get("required").and_then(|v| v.as_array()) else {
                continue;
            };
            let properties = schema.get("properties").and_then(|v| v.as_object());
            for name in required.iter().filter_map(|v| v.as_str()) {
                let declared = properties.map(|p| p.contains_key(name)).unwrap_or(false);
                assert!(
                    declared,
                    "tool `{}` requires argument `{}`, which its `properties` does not declare",
                    d.name, name
                );
            }
        }
    }

    /// Arguments the reference itself declares in snake_case, and which must
    /// therefore stay that way here.
    ///
    /// Exactly one so far: `ListEventsTool.Args.SinceToken` carries an explicit
    /// `[JsonPropertyName("since_token")]`, overriding the camelCase policy that
    /// governs every other argument. Matching the reference beats matching our
    /// own convention — an agent sends what the reference declares. Additions
    /// need the same evidence: a JsonPropertyName in the reference source.
    const REFERENCE_SNAKE_CASE_ARGS: &[&str] = &["list_events.since_token"];

    /// Argument names are camelCase, matching the reference's serializer. A
    /// snake_case leftover means a rename pass missed a schema.
    #[test]
    fn declared_arguments_are_camel_case() {
        let mut offenders: Vec<String> = Vec::new();
        for d in McpToolRouter::all_descriptors() {
            let Some(props) = d.input_schema.get("properties").and_then(|v| v.as_object()) else {
                continue;
            };
            for name in props.keys().filter(|n| n.contains('_')) {
                let qualified = format!("{}.{}", d.name, name);
                if !REFERENCE_SNAKE_CASE_ARGS.contains(&qualified.as_str()) {
                    offenders.push(qualified);
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "snake_case argument names left in tool schemas: {offenders:?}"
        );
    }

    /// The exception list may not outlive the exceptions.
    ///
    /// A stale entry is worse than no list: it silently re-permits snake_case
    /// for a tool that has since been fixed or removed.
    #[test]
    fn every_snake_case_exception_is_still_in_use() {
        let declared: Vec<String> = McpToolRouter::all_descriptors()
            .iter()
            .filter_map(|d| {
                let props = d.input_schema.get("properties")?.as_object()?;
                Some(
                    props
                        .keys()
                        .map(|k| format!("{}.{}", d.name, k))
                        .collect::<Vec<_>>(),
                )
            })
            .flatten()
            .collect();

        for allowed in REFERENCE_SNAKE_CASE_ARGS {
            assert!(
                declared.iter().any(|d| d == allowed),
                "`{allowed}` is allow-listed as snake_case but no tool declares it — drop the entry"
            );
        }
    }

    /// An alias may never collide with a real tool name — dispatch resolves
    /// aliases first, so a collision would silently shadow the real tool.
    #[test]
    fn no_alias_shadows_a_canonical_tool() {
        let names: Vec<String> = McpToolRouter::canonical_descriptors()
            .into_iter()
            .map(|d| d.name)
            .collect();
        for a in ALIASES {
            assert!(
                !names.iter().any(|n| n == a.alias),
                "alias `{}` collides with a registered tool of the same name",
                a.alias
            );
        }
    }
}
