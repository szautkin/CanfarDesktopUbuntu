//! The tool taxonomy: which app each MCP tool belongs to, and what that app is.
//!
//! Lived inside `ui::ai_guide_page` as a private module labelled "UI grouping
//! only — no logic, no MCP". It is now read by both the AI Guide and the MCP
//! catalog tools, so it sits where neither depends on the other.
//!
//! One table, not two. The alternative — a second app taxonomy for agents
//! beside this one for the window — is the shape of a bug this codebase has
//! already had: a duplicated extension map drifted within a day, and a file the
//! editor could open was reported to agents as unsupported. A category added
//! here appears in the window and in `list_apps` at once, or in neither.
//!
//! Ported from the Windows `AiGuideCatalog`: an ordered set of named categories
//! (Other last) plus a tool-name → category-id map. The map is a superset — it
//! keeps the macOS/Windows tool names, which simply never match a live tool —
//! so a newly added tool is never silently dropped; it surfaces under "Other",
//! and `every_tool_lands_in_a_real_category` fails the build when it does.

/// One app: a named group of MCP tools.
///
/// Read by the AI Guide, which renders it as a tile, and by the MCP catalog
/// tools, which describe it to an agent. `icon` is the window's business and is
/// carried here so there is one table rather than a second icon map to keep
/// true.
#[derive(Clone, Copy)]
pub struct Category {
    pub id: &'static str,
    pub title: &'static str,
    /// A GTK symbolic icon name (the native analogue of the Windows Segoe glyph).
    pub icon: &'static str,
    pub summary: &'static str,
}

/// Ordered named categories — the order tiles render, top-to-bottom /
/// left-to-right. Ported 1:1 from `AiGuideCatalog.Builtin`.
pub static NAMED: [Category; 17] = [
    Category {
        id: "foundational",
        title: "Foundational",
        icon: "emblem-system-symbolic",
        summary: "App identity, auth, service health, platform load, and current view.",
    },
    Category {
        id: "search",
        title: "Search & Archive",
        icon: "system-search-symbolic",
        summary: "Find observations in CADC, then fetch their metadata, links, and previews.",
    },
    Category {
        id: "queries",
        title: "Saved Queries",
        icon: "view-list-symbolic",
        summary: "Save, recall, and edit reusable ADQL queries.",
    },
    Category {
        id: "research",
        title: "Research & Notes",
        icon: "emblem-documents-symbolic",
        summary: "Inspect downloaded observations and notes; export a research bundle.",
    },
    Category {
        id: "downloads",
        title: "Downloads",
        icon: "folder-download-symbolic",
        summary: "Pull observations into the local research archive.",
    },
    Category {
        id: "fits",
        title: "FITS Viewer",
        icon: "image-x-generic-symbolic",
        summary: "Read FITS headers/WCS, open files, steer the 2D viewer, bookmark coordinates.",
    },
    Category {
        id: "cube",
        title: "Cube Viewer",
        icon: "view-paged-symbolic",
        summary: "Open and steer the 3D spectral cube viewer; probe spectra; export figures.",
    },
    Category {
        id: "notebook",
        title: "Notebook",
        icon: "accessories-text-editor-symbolic",
        summary: "Drive the native notebook editor: cells, kernel, and execution.",
    },
    Category {
        id: "storage",
        title: "Storage (VOSpace)",
        icon: "drive-harddisk-symbolic",
        summary: "Browse, read, upload, download, and tidy files in VOSpace/ARC.",
    },
    Category {
        id: "sessions",
        title: "Sessions",
        icon: "computer-symbolic",
        summary: "Launch and manage interactive compute sessions.",
    },
    Category {
        id: "headless",
        title: "Headless / Batch",
        icon: "system-run-symbolic",
        summary: "Submit batch jobs and follow their logs and events.",
    },
    Category {
        id: "discovery",
        title: "Image Discovery",
        icon: "folder-saved-search-symbolic",
        summary: "Find container images by the packages they contain.",
    },
    Category {
        id: "compute",
        title: "AI Compute",
        icon: "verbinal-agent-symbolic",
        summary: "Run agent-authored code on a warm remote session.",
    },
    Category {
        id: "workflows",
        title: "Workflows",
        icon: "checkbox-checked-symbolic",
        summary: "Read, follow, author, and check off step-by-step research protocols.",
    },
    Category {
        id: "navigation",
        title: "View & Navigation",
        icon: "go-jump-symbolic",
        summary: "Steer the app's views and focus the search field.",
    },
    Category {
        id: "control",
        title: "Agent Control",
        icon: "security-high-symbolic",
        summary: "Inspect and withdraw the agent's pending proposals.",
    },
    Category {
        id: "guide",
        title: "AI Guide",
        icon: "dialog-information-symbolic",
        summary: "Re-tune tool descriptions and add your own guide tools (agent-editable).",
    },
];

/// Fallback bucket for any tool not explicitly categorized (renders last).
pub static OTHER: Category = Category {
    id: "other",
    title: "Other",
    icon: "view-grid-symbolic",
    summary: "Tools not yet sorted into a category.",
};

/// All categories including the fallback, in render order (Other last).
pub fn all() -> impl Iterator<Item = &'static Category> {
    NAMED.iter().chain(std::iter::once(&OTHER))
}

/// Categories always advertised, even when the tool list is slimmed.
///
/// `foundational` is where an agent starts — identity, auth, health, and the
/// catalog that finds everything else.
///
/// `control` is not optional either, and that is less obvious: it holds the
/// proposal lifecycle. An agent whose write enqueues a proposal needs
/// `list_pending_proposals` and `apply_proposal` to finish what it began, and a
/// slim list that dropped them left it holding a job it could not complete. A
/// test caught that; reasoning about it had not.
pub const ALWAYS_ADVERTISED: &[&str] = &["foundational", "control"];

/// Whether `tool` stays in `tools/list` when the list is slimmed.
pub fn is_always_advertised(tool: &str) -> bool {
    ALWAYS_ADVERTISED.contains(&category_id_for_tool(tool))
}

/// The category with `id`, if it is one.
pub fn by_id(id: &str) -> Option<&'static Category> {
    all().find(|c| c.id.eq_ignore_ascii_case(id))
}

/// Group `items` by the category of the name `name_of` returns.
///
/// Categories come back in render order, empty ones dropped, and each group's
/// items sorted by name. Generic over the item so the AI Guide (which groups
/// owned `ToolDescriptor`s) and the MCP catalog (which groups borrowed ones)
/// share the ordering rather than each writing a loop that agrees with the
/// other until someone changes one.
pub fn group_by_category<T>(
    items: Vec<T>,
    name_of: impl Fn(&T) -> String,
) -> Vec<(&'static Category, Vec<T>)> {
    let mut by_cat: std::collections::HashMap<&'static str, Vec<T>> =
        std::collections::HashMap::new();
    for item in items {
        let id = category_id_for_tool(&name_of(&item));
        by_cat.entry(id).or_default().push(item);
    }
    let mut out = Vec::new();
    for cat in all() {
        if let Some(mut group) = by_cat.remove(cat.id) {
            if group.is_empty() {
                continue;
            }
            group.sort_by_key(|i| name_of(i));
            out.push((cat, group));
        }
    }
    out
}

/// Category id for a tool name, defaulting to `other`. Ported from the Windows
/// `AiGuideCatalog.CategoryByTool`, extended with the live Verbinal tool names.
pub fn category_id_for_tool(name: &str) -> &'static str {
    match name {
        // Foundational
        "describe_app" | "get_auth_state" | "get_current_view" | "get_service_health"
        | "get_platform_load" | "get_job_status"
        // The map of the other 145. Foundational because an agent that does not
        // know where to start, starts here.
        | "list_apps" | "search_tools" | "man" => "foundational",
        // The search UI: the form, the ADQL editor, the results grid and
        // recent/saved history. Twenty-seven tools were landing in "Other"
        // because this match is a hand-kept list and whole families were
        // added without it — including the ones an agent uses most.
        "get_search_form"
        | "set_search_form"
        | "reset_search_form"
        | "get_search_constraints"
        | "set_search_constraints"
        | "run_search"
        | "set_adql_query"
        | "execute_adql_query"
        | "get_search_results"
        | "set_search_results_view"
        | "export_search_results"
        | "load_recent_search"
        | "run_saved_query"
        | "remove_recent_search"
        | "clear_recent_searches"
        | "describe_tap_schema" => "search",
        // FITS viewer tabs.
        "switch_fits_tab" | "close_fits_tab" | "blink_fits_tabs" => "fits",
        // Cube viewer.
        "switch_cube_tab"
        | "list_recent_cubes"
        | "set_cube_transfer"
        | "show_cube_spectrum"
        | "get_cube_channel_profile"
        | "get_cube_image"
        | "annotate_cube"
        | "list_cube_annotations" => "cube",
        // Notebook dependencies.
        "check_notebook_dependencies" | "install_notebook_dependencies" => "notebook",
        // Search & Archive
        "search_observations"
        | "vizier_cone_search"
        | "resolve_target"
        | "get_observation_caom2"
        | "get_data_links"
        | "get_preview_image"
        | "list_recent_searches" => "search",
        // Saved Queries
        "list_saved_queries" | "get_saved_query" | "save_query" | "update_saved_query"
        | "delete_saved_query" => "queries",
        // Research & Notes
        "list_downloaded_observations"
        | "list_observations"
        | "get_downloaded_observation"
        | "get_observation_notes"
        | "update_observation_note"
        | "bulk_update_observation_notes"
        | "export_research_bundle" => "research",
        // Downloads
        "download_observation"
        | "download_observations_bulk"
        | "delete_downloaded_observation"
        | "clear_research_archive" => "downloads",
        // FITS Viewer
        "get_fits_header"
        | "get_fits_wcs"
        | "open_fits_file"
        | "set_fits_view"
        | "get_fits_view"
        | "get_fits_image"
        | "annotate_fits"
        | "list_fits_annotations"
        | "probe_fits_pixel"
        | "fits_goto_coordinate"
        | "list_fits_bookmarks"
        | "list_fits_bookmark"
        | "save_fits_bookmark"
        | "delete_fits_bookmark" => "fits",
        // Cube Viewer
        "open_cube"
        | "set_cube_view"
        | "get_cube_view"
        | "probe_cube_spectrum"
        | "export_cube_figure" => "cube",
        // Notebook
        "list_notebooks"
        | "list_open_notebooks"
        | "get_notebook"
        | "get_cell_output"
        | "get_cell_image"
        | "get_kernel_state"
        | "open_notebook"
        | "create_notebook"
        | "save_notebook"
        | "edit_cell"
        | "add_cell"
        | "delete_cell"
        | "change_cell_type"
        | "move_cell"
        | "run_cell"
        | "run_all_cells"
        | "run_all"
        | "clear_cell_outputs"
        | "clear_outputs"
        | "start_kernel"
        | "interrupt_kernel"
        | "restart_kernel"
        | "create_analysis_notebook" => "notebook",
        // Storage (VOSpace) — Windows names + live Verbinal names
        "list_vospace_path"
        | "get_vospace_node"
        | "read_vospace_file"
        | "upload_to_vospace"
        | "upload_text_to_vospace"
        | "upload_file_to_vospace"
        | "download_from_vospace"
        | "download_vospace_file"
        | "vospace_mkdir"
        | "create_vospace_folder"
        | "set_vospace_acl"
        | "delete_vospace_node"
        | "get_storage_quota"
        | "clear_user_site"
        | "list_storage"
        | "get_node"
        | "read_file"
        | "get_quota"
        | "upload_text"
        | "create_folder"
        | "set_acl"
        | "delete_node" => "storage",
        // Sessions
        "list_sessions"
        | "get_session"
        | "list_session_types"
        | "list_session_images"
        | "list_recent_launches"
        | "launch_session"
        | "delete_session"
        | "delete_sessions_bulk"
        | "renew_session"
        | "get_session_events"
        | "get_session_logs" => "sessions",
        // Headless / Batch
        "list_headless_jobs"
        | "get_headless_job"
        | "get_headless_job_logs"
        | "get_headless_job_events"
        | "launch_headless_job" => "headless",
        // Image Discovery
        "find_images_with_packages" | "discover_image_packages" => "discovery",
        // AI Compute
        "run_code" | "run_code_output" | "start_compute" | "stop_compute" => "compute",
        // Workflows
        "list_workflows" | "get_workflow" | "save_workflow" | "update_workflow"
        | "set_workflow_step" | "use_workflow" | "delete_workflow" => "workflows",
        // View & Navigation. The annotation lifecycle tools live here rather
        // than under a viewer because they work on either one — an id
        // identifies a mark, and the caller need not know which viewer holds
        // it.
        "set_search_focus"
        | "navigate_to"
        | "close_active_tab"
        | "list_open_tabs"
        | "remove_annotation"
        | "update_annotation"
        | "clear_annotations" => {
            "navigation"
        }
        // Agent Control
        "list_pending_proposals"
        | "get_proposal_state"
        | "withdraw_proposal"
        | "list_events" => "control",
        // AI Guide management
        "list_guide_tools"
        | "set_tool_description"
        | "clear_tool_description"
        | "add_guide_tool"
        | "update_guide_tool"
        | "delete_guide_tool" => "guide",
        _ => OTHER.id,
    }
}

#[cfg(test)]
mod always_advertised_tests {
    use super::*;

    /// An agent must be able to finish an approval flow it started.
    ///
    /// A slim `tools/list` first kept only `foundational`, which dropped the
    /// proposal lifecycle: a write would enqueue a proposal and the agent had
    /// no advertised way to list or apply it. The end-to-end handshake test
    /// caught it. These pin the rule so the next narrowing cannot lose them
    /// again.
    #[test]
    fn the_proposal_lifecycle_survives_a_slim_list() {
        // The agent's half of the lifecycle. Applying is the USER's action, in
        // the window — an agent proposes and waits, so there is no
        // `apply_proposal` to advertise.
        for tool in [
            "list_pending_proposals",
            "get_proposal_state",
            "withdraw_proposal",
        ] {
            assert!(
                is_always_advertised(tool),
                "{tool} would be dropped from a slim tools/list, so an agent whose \
                 write enqueued a proposal could not see or withdraw it"
            );
        }
    }

    /// The way in survives too.
    #[test]
    fn the_catalog_and_the_basics_survive_a_slim_list() {
        for tool in [
            "list_apps",
            "describe_app",
            "search_tools",
            "get_auth_state",
            "get_current_view",
        ] {
            assert!(is_always_advertised(tool), "{tool} is how an agent starts");
        }
    }

    /// Ordinary tools are the ones a slim list leaves out.
    ///
    /// If this ever passes for everything, the slim list has stopped being
    /// slim and the setting is doing nothing.
    #[test]
    fn ordinary_tools_are_not_always_advertised() {
        for tool in [
            "get_fits_image",
            "run_cell",
            "upload_to_vospace",
            "open_cube",
        ] {
            assert!(
                !is_always_advertised(tool),
                "{tool} is advertised even when slim — nothing is being saved"
            );
        }
    }

    /// Every always-advertised id is a real category.
    #[test]
    fn the_always_advertised_ids_exist() {
        for id in ALWAYS_ADVERTISED {
            assert!(by_id(id).is_some(), "{id} is not a category");
        }
    }
}
