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
    const NOT_YET_PORTED: &[&str] = &[];

    /// **The wire contract.** Everything CanfarDesktop 1.3.3 advertises, we must
    /// advertise under the same name — otherwise a prompt or agent written
    /// against the reference calls a tool this build does not have.
    ///
    /// Three separate failures, each naming the offending tools:
    ///  * **extra** — we advertise something the reference doesn't. Always a bug:
    ///    either a rename went the wrong way or a tool needs adding upstream.
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

        let extra: Vec<&String> = ours.difference(&theirs).collect();
        assert!(
            extra.is_empty(),
            "we advertise {} tool(s) CanfarDesktop 1.3.3 does not: {extra:?}",
            extra.len()
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
