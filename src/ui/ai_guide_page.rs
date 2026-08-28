//! The AI Guide dashboard — a control panel for tuning how an MCP agent perceives
//! each tool. Ported from `Views/AiGuidePage.xaml(.cs)`, `ViewModels/AiGuideViewModel.cs`,
//! and `Services/AiGuide/AiGuideCatalog.cs`.
//!
//! Rather than one flat Read/Write list, the built-in tools (the read + write
//! descriptors plus every service-backed family advertised in `tools/list`) are
//! grouped into ~16 named categories (+ an "Other" fallback for any tool the
//! catalog doesn't yet know). The page renders as:
//!  * a **launchpad** of category tiles (icon + summary + tool count + an accent
//!    dot when the category has overrides) that open a focused per-category list;
//!  * a flat **search** view (a filter box + a match count) that supersedes the
//!    launchpad while a query is typed;
//!  * header **stat chips** (total tools, overridden count, category count).
//!
//! Each tool is an inline accordion: the header shows the description the agent
//! currently sees; the body edits the override. Edits flow straight to the
//! [`AiGuideService`] (autosave-on-change): blanking a field — or typing the
//! built-in text back in — clears the override, and the MCP server reads the new
//! wording live on the next `tools/list`.
//!
//! A second section lets the user author their own read-only "guide tools"
//! (name + description + a body returned verbatim when the agent calls the tool),
//! each with **Edit** (a pre-filled dialog with a live slug preview + char
//! counters) and a **Remove** confirmation.

use crate::mcp::tools::{ToolDescriptor, VerbClass};
use crate::services::ai_guide::{
    AiGuideService, AiGuideSnapshot, GuideTool, MAX_BODY_CHARS, MAX_DESCRIPTION_CHARS,
};
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use libadwaita as adw;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

// ─────────────────────────────────────────────────────────────────────────────
// Category catalog (UI grouping only — no logic, no MCP). Ports the Windows
// `AiGuideCatalog`: an ordered set of named categories (Other last) plus a
// tool-name → category-id map. The map is a superset — it keeps the macOS/Windows
// tool names (harmless: they simply never match a live tool) alongside the live
// Verbinal names, so a newly-added tool is never silently dropped (it surfaces
// under "Other").
// ─────────────────────────────────────────────────────────────────────────────
mod catalog {
    /// One AI Guide category: a UI grouping of built-in tools.
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
            summary:
                "Read FITS headers/WCS, open files, steer the 2D viewer, bookmark coordinates.",
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

    /// Category id for a tool name, defaulting to `other`. Ported from the Windows
    /// `AiGuideCatalog.CategoryByTool`, extended with the live Verbinal tool names.
    pub fn category_id_for_tool(name: &str) -> &'static str {
        match name {
            // Foundational
            "describe_app" | "get_auth_state" | "get_current_view" | "get_service_health"
            | "get_platform_load" | "get_job_status" => "foundational",
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
            | "get_cube_channel_profile" => "cube",
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
            // View & Navigation
            "set_search_focus" | "navigate_to" | "close_active_tab" | "list_open_tabs" => {
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
}

/// A category with its resolved tool descriptors — built once from the live tool
/// surface and stored on the page (the descriptors never change; only overrides do).
struct CatData {
    /// Read only by the grouping test, which names a category by its stable id
    /// rather than its display title — the title is localized, so asserting on
    /// it would make the test fail in French.
    #[allow(dead_code)]
    id: &'static str,
    title: &'static str,
    icon: &'static str,
    summary: &'static str,
    tools: Vec<ToolDescriptor>,
}

/// The full live built-in tool surface the AI Guide governs: read + write +
/// every service-backed family (the same set the router advertises, minus the
/// proposal-lifecycle tools).
fn all_live_descriptors() -> Vec<ToolDescriptor> {
    let mut v = crate::mcp::tools::read::descriptors();
    v.extend(crate::mcp::tools::write::descriptors());
    v.extend(crate::mcp::tools::family_descriptors());
    v
}

/// Group the descriptors into ordered, non-empty categories (Other last), each
/// with its tools sorted by name. Mirrors `AiGuideViewModel.Load`.
fn categorize(descriptors: Vec<ToolDescriptor>) -> Vec<CatData> {
    let mut by_cat: HashMap<&'static str, Vec<ToolDescriptor>> = HashMap::new();
    for d in descriptors {
        let id = catalog::category_id_for_tool(&d.name);
        by_cat.entry(id).or_default().push(d);
    }
    let mut out = Vec::new();
    for cat in catalog::all() {
        if let Some(mut tools) = by_cat.remove(cat.id) {
            if tools.is_empty() {
                continue;
            }
            tools.sort_by(|a, b| a.name.cmp(&b.name));
            out.push(CatData {
                id: cat.id,
                title: cat.title,
                icon: cat.icon,
                summary: cat.summary,
                tools,
            });
        }
    }
    out
}

pub struct AiGuidePage {
    pub widget: gtk::Box,
    guide: Arc<AiGuideService>,

    /// The categorized live tool surface (immutable after construction).
    categories: Vec<CatData>,
    total_tools: usize,

    // ── Header stat chips + search ──
    tools_chip: gtk::Label,
    overridden_chip: gtk::Label,
    overridden_chip_box: gtk::Box,
    categories_chip: gtk::Label,
    search_entry: gtk::SearchEntry,
    match_count: gtk::Label,

    // ── Content stack: launchpad ▸ focus ▸ search ──
    stack: gtk::Stack,
    launchpad_flow: gtk::FlowBox,
    focus_container: gtk::Box,
    /// Every tool in one list, for the "See everything" view.
    everything_container: gtk::Box,
    /// The launchpad's view switch. Only the "everything" half is held: the two
    /// are one radio group, so its state answers the question the page asks —
    /// "which view did the user choose?" — and a second field would be a second
    /// answer.
    everything_btn: gtk::ToggleButton,
    search_container: gtk::Box,

    // ── "My guide tools" group ──
    guides_group: adw::PreferencesGroup,
    guide_rows: RefCell<Vec<adw::ActionRow>>,
}

impl AiGuidePage {
    pub fn new(guide: Arc<AiGuideService>) -> Rc<Self> {
        let categories = categorize(all_live_descriptors());
        let total_tools: usize = categories.iter().map(|c| c.tools.len()).sum();

        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);

        let scroller = gtk::ScrolledWindow::new();
        scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroller.set_hexpand(true);
        scroller.set_vexpand(true);
        widget.append(&scroller);

        let clamp = adw::Clamp::new();
        clamp.set_maximum_size(1000);
        clamp.set_tightening_threshold(800);
        scroller.set_child(Some(&clamp));

        let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
        content.set_margin_start(12);
        content.set_margin_end(12);
        content.set_margin_top(12);
        content.set_margin_bottom(12);
        clamp.set_child(Some(&content));

        // ── Header card: intro + stat chips + filter box + match count ──
        let header = gtk::Box::new(gtk::Orientation::Vertical, 12);
        header.add_css_class("card");
        let header_inner = gtk::Box::new(gtk::Orientation::Vertical, 12);
        header_inner.set_margin_start(12);
        header_inner.set_margin_end(12);
        header_inner.set_margin_top(12);
        header_inner.set_margin_bottom(12);
        header.append(&header_inner);

        let intro = gtk::Label::new(Some(crate::tr_en!(
            "Re-tune how the AI agent sees each tool. Your edits override the built-in \
             description the MCP server advertises, live — the next tools/list an agent runs \
             uses your wording. Pick a category to focus, or filter across every tool."
        )));
        intro.set_wrap(true);
        intro.set_xalign(0.0);
        intro.add_css_class("body");
        header_inner.append(&intro);

        // Stat chips.
        let chips = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let (tools_box, tools_chip) = make_chip("");
        let (overridden_chip_box, overridden_chip) = make_chip("");
        let (categories_box, categories_chip) = make_chip("");
        overridden_chip_box.set_visible(false);
        chips.append(&tools_box);
        chips.append(&overridden_chip_box);
        chips.append(&categories_box);
        header_inner.append(&chips);

        // Filter box (SearchEntry provides its own clear affordance).
        let search_entry = gtk::SearchEntry::new();
        search_entry
            .set_placeholder_text(Some(crate::tr_en!("Filter tools by name or description…")));
        search_entry.set_hexpand(true);
        header_inner.append(&search_entry);

        let match_count = gtk::Label::new(None);
        match_count.set_xalign(0.0);
        match_count.add_css_class("dim-label");
        match_count.add_css_class("caption");
        match_count.set_visible(false);
        header_inner.append(&match_count);

        content.append(&header);

        // ── "My guide tools" group ──
        let guides_group = adw::PreferencesGroup::new();
        guides_group.set_title(crate::tr_en!("My guide tools"));
        guides_group.set_description(Some(crate::tr_en!(
            "Custom read-only tools you author. Calling one returns your instructions \
             verbatim to the agent — a place to encode your protocols and conventions."
        )));
        content.append(&guides_group);

        // ── Content stack ──
        let stack = gtk::Stack::new();
        stack.set_hexpand(true);

        let launchpad_flow = gtk::FlowBox::new();
        launchpad_flow.set_selection_mode(gtk::SelectionMode::None);
        launchpad_flow.set_homogeneous(true);
        launchpad_flow.set_min_children_per_line(1);
        launchpad_flow.set_max_children_per_line(3);
        launchpad_flow.set_row_spacing(12);
        launchpad_flow.set_column_spacing(12);
        // View switch: tiles, or every tool at once. Without it the only way to
        // read all 137 descriptions was to open each of the seventeen
        // categories in turn, or to invent a search string that matches
        // everything — the reference offers it as a plain choice, so we do too.
        let view_switch = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        view_switch.set_halign(gtk::Align::End);
        let view_label = gtk::Label::new(Some(crate::tr_en!("View:")));
        view_label.add_css_class("caption");
        view_label.add_css_class("dim-label");
        view_switch.append(&view_label);
        let modes = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        modes.add_css_class("linked");
        let tiles_btn = gtk::ToggleButton::with_label(crate::tr_en!("Tiles"));
        tiles_btn.set_active(true);
        let everything_btn = gtk::ToggleButton::with_label(crate::tr_en!("See everything"));
        everything_btn.set_group(Some(&tiles_btn));
        modes.append(&tiles_btn);
        modes.append(&everything_btn);
        view_switch.append(&modes);

        let launchpad_page = gtk::Box::new(gtk::Orientation::Vertical, 12);
        launchpad_page.append(&view_switch);
        launchpad_page.append(&launchpad_flow);
        stack.add_named(&launchpad_page, Some("launchpad"));

        let everything_container = gtk::Box::new(gtk::Orientation::Vertical, 12);
        stack.add_named(&everything_container, Some("everything"));

        let focus_container = gtk::Box::new(gtk::Orientation::Vertical, 12);
        stack.add_named(&focus_container, Some("focus"));

        let search_container = gtk::Box::new(gtk::Orientation::Vertical, 0);
        stack.add_named(&search_container, Some("search"));

        content.append(&stack);

        let page = Rc::new(AiGuidePage {
            widget,
            guide,
            categories,
            total_tools,
            tools_chip,
            overridden_chip,
            overridden_chip_box,
            categories_chip,
            search_entry,
            match_count,
            stack,
            launchpad_flow,
            focus_container,
            everything_container,
            everything_btn: everything_btn.clone(),
            search_container,
            guides_group,
            guide_rows: RefCell::new(Vec::new()),
        });
        page.build();
        page
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.widget
    }

    /// Wire the interactive bits and seed the initial view.
    fn build(self: &Rc<Self>) {
        // "New guide" affordance in the guides group header.
        let add_btn = gtk::Button::with_label(crate::tr_en!("New guide"));
        add_btn.add_css_class("flat");
        add_btn.set_valign(gtk::Align::Center);
        add_btn.set_tooltip_text(Some(crate::tr_en!("Author a new guide tool")));
        {
            let page = self.clone();
            add_btn.connect_clicked(move |_| {
                let page = page.clone();
                let parent = page.widget.clone();
                let guide = page.guide.clone();
                glib::spawn_future_local(async move {
                    if show_guide_dialog(&parent, guide, None).await {
                        page.rebuild_guides();
                    }
                });
            });
        }
        self.guides_group.set_header_suffix(Some(&add_btn));

        // Filter box drives the search view (empty ⇒ back to the launchpad).
        {
            let page = self.clone();
            self.search_entry.connect_search_changed(move |entry| {
                let text = entry.text();
                page.apply_search(&text);
            });
        }

        // The view switch: Tiles shows the launchpad, See everything the flat
        // list. Only the "everything" side needs a handler — leaving Tiles is
        // what activates it, and the pair is a radio group.
        {
            let page = Rc::clone(self);
            self.everything_btn.connect_toggled(move |btn| {
                if btn.is_active() {
                    page.show_everything();
                } else {
                    page.show_launchpad();
                }
            });
        }

        self.rebuild_guides();
        self.show_launchpad();
    }

    /// Recompute the header stat chips from a fresh snapshot.
    fn refresh_stats(&self) {
        let snap = self.guide.snapshot();
        let overridden = self
            .categories
            .iter()
            .flat_map(|c| c.tools.iter())
            .filter(|d| snap.overrides.contains_key(&d.name))
            .count();

        self.tools_chip
            .set_text(&crate::tr_fmt!("{} tools", self.total_tools));
        self.categories_chip
            .set_text(&crate::tr_fmt!("{} categories", self.categories.len()));
        self.overridden_chip
            .set_text(&crate::tr_fmt!("{} overridden", overridden));
        self.overridden_chip_box.set_visible(overridden > 0);
    }

    /// Rebuild the launchpad tiles from a fresh snapshot and show it.
    fn show_launchpad(self: &Rc<Self>) {
        // Coming back from a focused category or a cleared search: honour the
        // view the user chose rather than silently resetting them to Tiles.
        if self.everything_btn.is_active() {
            self.show_everything();
            return;
        }
        while let Some(child) = self.launchpad_flow.first_child() {
            self.launchpad_flow.remove(&child);
        }
        let snap = self.guide.snapshot();
        for (i, cat) in self.categories.iter().enumerate() {
            let has_override = cat
                .tools
                .iter()
                .any(|d| snap.overrides.contains_key(&d.name));
            let tile = self.build_tile(i, cat, has_override);
            self.launchpad_flow.append(&tile);
        }
        self.refresh_stats();
        self.stack.set_visible_child_name("launchpad");
    }

    /// One launchpad tile: icon + title, a 2-line summary, and a footer with an
    /// accent dot (when overridden) + the tool count. Mirrors the Windows `TileTemplate`.
    fn build_tile(self: &Rc<Self>, index: usize, cat: &CatData, has_override: bool) -> gtk::Button {
        let btn = gtk::Button::new();
        btn.add_css_class("card");
        btn.set_hexpand(true);
        btn.set_size_request(-1, 118);

        let v = gtk::Box::new(gtk::Orientation::Vertical, 6);
        v.set_margin_start(12);
        v.set_margin_end(12);
        v.set_margin_top(12);
        v.set_margin_bottom(12);

        let top = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let icon = gtk::Image::from_icon_name(cat.icon);
        icon.add_css_class("accent");
        icon.set_valign(gtk::Align::Center);
        // Translated here rather than at definition: `NAMED` is a static, and
        // a static cannot call a function. The French forms are guarded by
        // `every_category_label_has_a_french_form` — the generic i18n scan only
        // sees `tr_en!("literal")` call sites, and these are variables.
        let title = gtk::Label::new(Some(crate::i18n::tr_en(cat.title)));
        title.add_css_class("heading");
        title.set_xalign(0.0);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        top.append(&icon);
        top.append(&title);
        v.append(&top);

        let summary = gtk::Label::new(Some(crate::i18n::tr_en(cat.summary)));
        summary.set_wrap(true);
        summary.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        summary.set_lines(2);
        summary.set_ellipsize(gtk::pango::EllipsizeMode::End);
        summary.set_xalign(0.0);
        summary.set_valign(gtk::Align::Start);
        summary.set_vexpand(true);
        summary.add_css_class("caption");
        summary.add_css_class("dim-label");
        v.append(&summary);

        let footer = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        footer.set_halign(gtk::Align::End);
        let dot = gtk::Label::new(Some("●"));
        dot.add_css_class("accent");
        dot.set_visible(has_override);
        let count = gtk::Label::new(Some(&tool_count_text(cat.tools.len())));
        count.add_css_class("caption");
        count.add_css_class("dim-label");
        footer.append(&dot);
        footer.append(&count);
        v.append(&footer);

        btn.set_child(Some(&v));

        {
            let page = self.clone();
            btn.connect_clicked(move |_| page.open_focus(index));
        }
        btn
    }

    /// Open a category's focused list: a back link, the category header, and the
    /// category's tool accordions. Mirrors the Windows focus panel.
    fn open_focus(self: &Rc<Self>, index: usize) {
        let cat = match self.categories.get(index) {
            Some(c) => c,
            None => return,
        };

        while let Some(child) = self.focus_container.first_child() {
            self.focus_container.remove(&child);
        }

        let back = gtk::Button::new();
        back.add_css_class("flat");
        back.set_halign(gtk::Align::Start);
        let back_content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        back_content.append(&gtk::Image::from_icon_name("go-previous-symbolic"));
        back_content.append(&gtk::Label::new(Some(crate::tr_en!("All categories"))));
        back.set_child(Some(&back_content));
        {
            let page = self.clone();
            back.connect_clicked(move |_| page.show_launchpad());
        }
        self.focus_container.append(&back);

        let head = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let icon = gtk::Image::from_icon_name(cat.icon);
        icon.set_pixel_size(24);
        icon.add_css_class("accent");
        icon.set_valign(gtk::Align::Center);
        let titles = gtk::Box::new(gtk::Orientation::Vertical, 2);
        titles.set_hexpand(true);
        // Translated here rather than at definition: `NAMED` is a static, and
        // a static cannot call a function. The French forms are guarded by
        // `every_category_label_has_a_french_form` — the generic i18n scan only
        // sees `tr_en!("literal")` call sites, and these are variables.
        let title = gtk::Label::new(Some(crate::i18n::tr_en(cat.title)));
        title.add_css_class("title-4");
        title.set_xalign(0.0);
        let summary = gtk::Label::new(Some(cat.summary));
        summary.set_wrap(true);
        summary.set_xalign(0.0);
        summary.add_css_class("caption");
        summary.add_css_class("dim-label");
        titles.append(&title);
        titles.append(&summary);
        let count = gtk::Label::new(Some(&tool_count_text(cat.tools.len())));
        count.add_css_class("caption");
        count.add_css_class("dim-label");
        count.set_valign(gtk::Align::Center);
        head.append(&icon);
        head.append(&titles);
        head.append(&count);
        self.focus_container.append(&head);

        let snapshot = self.guide.snapshot();
        let group = adw::PreferencesGroup::new();
        for d in &cat.tools {
            group.add(&self.build_tool_row(&snapshot, d));
        }
        self.focus_container.append(&group);

        self.stack.set_visible_child_name("focus");
    }

    /// Render every tool in one flat list, alphabetically.
    ///
    /// The same row builder the focused and search views use — a second way to
    /// draw a tool row would be a second place for the override editor to drift.
    fn show_everything(self: &Rc<Self>) {
        while let Some(child) = self.everything_container.first_child() {
            self.everything_container.remove(&child);
        }

        let snapshot = self.guide.snapshot();
        let mut all: Vec<&ToolDescriptor> = self
            .categories
            .iter()
            .flat_map(|c| c.tools.iter())
            .collect();
        all.sort_by(|a, b| a.name.cmp(&b.name));

        let heading = gtk::Label::new(Some(&tool_count_text(all.len())));
        heading.add_css_class("caption");
        heading.add_css_class("dim-label");
        heading.set_xalign(0.0);
        self.everything_container.append(&heading);

        let group = adw::PreferencesGroup::new();
        for d in all {
            group.add(&self.build_tool_row(&snapshot, d));
        }
        self.everything_container.append(&group);

        self.stack.set_visible_child_name("everything");
    }

    /// Filter every tool by name or effective description; render a flat list plus
    /// a match count. An empty query returns to the launchpad. Mirrors
    /// `AiGuideViewModel.ApplyFilter`.
    fn apply_search(self: &Rc<Self>, query: &str) {
        let q = query.trim();
        if q.is_empty() {
            self.match_count.set_visible(false);
            self.show_launchpad();
            return;
        }

        let ql = q.to_lowercase();
        let snap = self.guide.snapshot();
        let mut matches: Vec<&ToolDescriptor> = Vec::new();
        for cat in &self.categories {
            for d in &cat.tools {
                let eff = snap.description_for_tool(&d.name, &d.description);
                if d.name.to_lowercase().contains(&ql) || eff.to_lowercase().contains(&ql) {
                    matches.push(d);
                }
            }
        }
        matches.sort_by(|a, b| a.name.cmp(&b.name));

        while let Some(child) = self.search_container.first_child() {
            self.search_container.remove(&child);
        }

        self.match_count.set_text(&crate::tr_fmt!(
            "{} of {} tools match \u{201c}{}\u{201d}",
            matches.len(),
            self.total_tools,
            q
        ));
        self.match_count.set_visible(true);

        if matches.is_empty() {
            let empty = gtk::Label::new(Some(crate::tr_en!("No tools match your filter.")));
            empty.add_css_class("dim-label");
            empty.set_xalign(0.0);
            empty.set_margin_top(12);
            self.search_container.append(&empty);
        } else {
            let group = adw::PreferencesGroup::new();
            for d in matches {
                group.add(&self.build_tool_row(&snap, d));
            }
            self.search_container.append(&group);
        }
        self.stack.set_visible_child_name("search");
    }

    /// Build one expandable editor for a built-in tool: header shows the tool name +
    /// the description the agent currently sees; the body edits the override.
    fn build_tool_row(
        self: &Rc<Self>,
        snapshot: &AiGuideSnapshot,
        d: &ToolDescriptor,
    ) -> adw::ExpanderRow {
        let effective = snapshot.description_for_tool(&d.name, &d.description);
        let override_text = snapshot.overrides.get(&d.name).cloned().unwrap_or_default();
        let is_overridden = snapshot.overrides.contains_key(&d.name);

        let row = adw::ExpanderRow::new();
        row.set_title(&d.name);
        row.set_subtitle(&effective);
        row.set_subtitle_lines(0);

        // Verb tag as a prefix icon.
        let icon = match d.verb {
            VerbClass::Read => "emblem-ok-symbolic",
            VerbClass::Write => "document-edit-symbolic",
        };
        let prefix = gtk::Image::from_icon_name(icon);
        prefix.set_valign(gtk::Align::Center);
        row.add_prefix(&prefix);

        // "overridden" pill, visible only while an override is in effect.
        let badge = gtk::Label::new(Some(crate::tr_en!("overridden")));
        crate::ui::fit::fit_label(&badge);
        badge.add_css_class("accent");
        badge.add_css_class("caption-heading");
        badge.set_valign(gtk::Align::Center);
        badge.set_visible(is_overridden);
        row.add_suffix(&badge);

        // ── Editor body ──
        let content = gtk::Box::new(gtk::Orientation::Vertical, 6);
        content.set_margin_start(12);
        content.set_margin_end(12);
        content.set_margin_top(12);
        content.set_margin_bottom(12);

        let default_label = gtk::Label::new(Some(&crate::tr_fmt!("Built-in: {}", d.description)));
        default_label.set_wrap(true);
        default_label.set_xalign(0.0);
        default_label.add_css_class("dim-label");
        default_label.add_css_class("caption");
        content.append(&default_label);

        let hint = gtk::Label::new(Some(crate::tr_en!(
            "Shown to the agent in tools/list. Blank (or the built-in text) uses the default."
        )));
        hint.set_wrap(true);
        hint.set_xalign(0.0);
        hint.add_css_class("dim-label");
        hint.add_css_class("caption");
        content.append(&hint);

        let buffer = gtk::TextBuffer::new(None);
        buffer.set_text(&override_text); // seed BEFORE connecting so it doesn't self-fire

        let text_view = gtk::TextView::with_buffer(&buffer);
        text_view.set_wrap_mode(gtk::WrapMode::WordChar);
        text_view.set_top_margin(6);
        text_view.set_bottom_margin(6);
        text_view.set_left_margin(8);
        text_view.set_right_margin(8);

        let scroller = gtk::ScrolledWindow::new();
        scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroller.set_min_content_height(84);
        scroller.set_child(Some(&text_view));

        let frame = gtk::Frame::new(None);
        frame.set_child(Some(&scroller));
        content.append(&frame);

        let reset_btn = gtk::Button::with_label(crate::tr_en!("Reset to default"));
        reset_btn.add_css_class("flat");
        reset_btn.set_halign(gtk::Align::Start);
        {
            let buffer = buffer.clone();
            reset_btn.connect_clicked(move |_| buffer.set_text(""));
        }
        content.append(&reset_btn);

        row.add_row(&content);

        // Autosave-on-change: blank / built-in text clears the override, anything else
        // sets it. Keeps the header subtitle + pill + stat chips in sync.
        {
            let page = self.clone();
            let name = d.name.clone();
            let default = d.description.clone();
            let badge = badge.clone();
            let row = row.clone();
            buffer.connect_changed(move |buf| {
                let (start, end) = buf.bounds();
                let text = buf.text(&start, &end, false).to_string();
                let trimmed = text.trim();
                if trimmed.is_empty() || trimmed == default.trim() {
                    page.guide.clear_override(&name);
                    badge.set_visible(false);
                    row.set_subtitle(&default);
                } else {
                    page.guide.set_override(&name, trimmed);
                    badge.set_visible(true);
                    row.set_subtitle(trimmed);
                }
                page.refresh_stats();
            });
        }

        row
    }

    /// Clear and repopulate the "My guide tools" group from the service, giving each
    /// row an Edit and a (confirmed) Remove affordance.
    fn rebuild_guides(self: &Rc<Self>) {
        {
            let mut rows = self.guide_rows.borrow_mut();
            for row in rows.drain(..) {
                self.guides_group.remove(&row);
            }
        }

        let guides = self.guide.list_guides();
        if guides.is_empty() {
            let row = adw::ActionRow::new();
            row.set_title(crate::tr_en!("No guide tools yet"));
            row.set_subtitle(crate::tr_en!(
                "Click New guide to author a custom read-only tool the agent can call."
            ));
            row.set_subtitle_lines(0);
            self.guides_group.add(&row);
            self.guide_rows.borrow_mut().push(row);
            return;
        }

        for g in guides {
            let row = adw::ActionRow::new();
            row.set_title(&g.name);
            row.set_subtitle(&g.description);
            row.set_subtitle_lines(0);

            let chars = g.body.trim().chars().count();
            if chars > 0 {
                let info = gtk::Label::new(Some(&crate::tr_fmt!("returns {} chars", chars)));
                crate::ui::fit::fit_label(&info);
                info.add_css_class("dim-label");
                info.add_css_class("caption");
                info.set_valign(gtk::Align::Center);
                row.add_suffix(&info);
            }

            // Edit — reopen the dialog pre-filled with this guide.
            let edit_btn = gtk::Button::from_icon_name("document-edit-symbolic");
            edit_btn.add_css_class("flat");
            edit_btn.set_valign(gtk::Align::Center);
            edit_btn.set_tooltip_text(Some(crate::tr_en!("Edit")));
            {
                let page = self.clone();
                let existing = g.clone();
                edit_btn.connect_clicked(move |_| {
                    let page = page.clone();
                    let parent = page.widget.clone();
                    let guide = page.guide.clone();
                    let existing = existing.clone();
                    glib::spawn_future_local(async move {
                        if show_guide_dialog(&parent, guide, Some(existing)).await {
                            page.rebuild_guides();
                        }
                    });
                });
            }
            row.add_suffix(&edit_btn);

            // Remove — with a confirmation.
            let remove_btn = gtk::Button::from_icon_name("user-trash-symbolic");
            remove_btn.add_css_class("flat");
            remove_btn.set_valign(gtk::Align::Center);
            remove_btn.set_tooltip_text(Some(crate::tr_en!("Remove")));
            {
                let page = self.clone();
                let name = g.name.clone();
                remove_btn.connect_clicked(move |_| {
                    let page = page.clone();
                    let parent = page.widget.clone();
                    let name = name.clone();
                    glib::spawn_future_local(async move {
                        if confirm_remove_guide(&parent, &name).await {
                            page.guide.remove_guide(&name);
                            page.rebuild_guides();
                        }
                    });
                });
            }
            row.add_suffix(&remove_btn);

            self.guides_group.add(&row);
            self.guide_rows.borrow_mut().push(row);
        }
    }
}

/// A small rounded "chip" (a caption label in a card box). Returns the container
/// (to toggle visibility) and its label (to update text).
fn make_chip(text: &str) -> (gtk::Box, gtk::Label) {
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    b.add_css_class("card");
    b.set_valign(gtk::Align::Center);
    let l = gtk::Label::new(Some(text));
    l.add_css_class("caption");
    l.set_margin_start(10);
    l.set_margin_end(10);
    l.set_margin_top(3);
    l.set_margin_bottom(3);
    b.append(&l);
    (b, l)
}

/// `"1 tool"` / `"N tools"`.
fn tool_count_text(n: usize) -> String {
    if n == 1 {
        format!("{n} tool")
    } else {
        format!("{n} tools")
    }
}

/// Confirmation for removing a user guide tool.
async fn confirm_remove_guide(parent: &impl IsA<gtk::Widget>, name: &str) -> bool {
    let dialog = adw::MessageDialog::new(
        parent
            .root()
            .and_then(|r| r.downcast::<gtk::Window>().ok())
            .as_ref(),
        Some(crate::tr_en!("Remove guide tool")),
        Some(&format!(
            "Remove the guide tool '{name}'? The agent will no longer see it in tools/list.\n\n\
             This cannot be undone."
        )),
    );
    dialog.add_response("cancel", crate::tr_en!("Cancel"));
    dialog.add_response("remove", crate::tr_en!("Remove"));
    dialog.set_response_appearance("remove", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");

    let (tx, rx) = tokio::sync::oneshot::channel::<bool>();
    let tx = Rc::new(RefCell::new(Some(tx)));
    {
        let tx = tx.clone();
        dialog.connect_response(None, move |_, response| {
            if let Some(tx) = tx.borrow_mut().take() {
                let _ = tx.send(response == "remove");
            }
        });
    }

    dialog.present();
    rx.await.unwrap_or(false)
}

/// Modal dialog to author or edit a guide tool. When `existing` is `Some`, the
/// form is pre-filled and Save updates that guide (keyed by its current slug);
/// otherwise Save adds a new one. Returns `true` if the service accepted the
/// change (so the caller can refresh). Ports `AiGuideEditDialog`.
async fn show_guide_dialog(
    parent: &impl IsA<gtk::Widget>,
    guide: Arc<AiGuideService>,
    existing: Option<GuideTool>,
) -> bool {
    let editing = existing.is_some();

    let dialog = adw::Window::builder()
        .title(if editing {
            crate::tr_en!("Edit guide tool")
        } else {
            crate::tr_en!("New guide tool")
        })
        .default_width(crate::ui::fit::FORM)
        .default_height(520)
        .modal(true)
        .build();

    if let Some(root) = parent.root().and_downcast::<gtk::Window>() {
        dialog.set_transient_for(Some(&root));
    }

    let toolbar_view = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    toolbar_view.add_top_bar(&header);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    content.set_margin_start(12);
    content.set_margin_end(12);
    content.set_margin_top(12);
    content.set_margin_bottom(12);

    let blurb = gtk::Label::new(Some(crate::tr_en!(
        "A guide tool is read-only guidance the agent can call by name. Give it a name and a \
         one-line description (shown in the tool list); the optional instructions are returned \
         verbatim when the agent calls it."
    )));
    blurb.set_wrap(true);
    blurb.set_xalign(0.0);
    blurb.add_css_class("dim-label");
    blurb.add_css_class("caption");
    content.append(&blurb);

    let fields = adw::PreferencesGroup::new();
    let name_row = adw::EntryRow::new();
    name_row.set_title(crate::tr_en!("Name (e.g. my_review_protocol)"));
    let desc_row = adw::EntryRow::new();
    desc_row.set_title(crate::tr_en!("Short description (shown in tools/list)"));
    fields.add(&name_row);
    fields.add(&desc_row);
    content.append(&fields);

    // Live slug preview under the name.
    let slug_label = gtk::Label::new(None);
    slug_label.set_xalign(0.0);
    slug_label.add_css_class("dim-label");
    slug_label.add_css_class("caption");
    content.append(&slug_label);

    // Description char counter.
    let desc_counter = gtk::Label::new(None);
    desc_counter.set_halign(gtk::Align::End);
    desc_counter.add_css_class("dim-label");
    desc_counter.add_css_class("caption");
    content.append(&desc_counter);

    let body_label = gtk::Label::new(Some(crate::tr_en!(
        "Instructions returned to the agent (optional)"
    )));
    body_label.set_xalign(0.0);
    body_label.add_css_class("dim-label");
    body_label.add_css_class("caption");
    content.append(&body_label);

    let body_buffer = gtk::TextBuffer::new(None);
    let body_view = gtk::TextView::with_buffer(&body_buffer);
    body_view.set_wrap_mode(gtk::WrapMode::WordChar);
    body_view.set_top_margin(6);
    body_view.set_bottom_margin(6);
    body_view.set_left_margin(8);
    body_view.set_right_margin(8);

    let scroller = gtk::ScrolledWindow::new();
    scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroller.set_min_content_height(140);
    scroller.set_vexpand(true);
    scroller.set_child(Some(&body_view));

    let frame = gtk::Frame::new(None);
    frame.set_vexpand(true);
    frame.set_child(Some(&scroller));
    content.append(&frame);

    // Body char counter.
    let body_counter = gtk::Label::new(None);
    body_counter.set_halign(gtk::Align::End);
    body_counter.add_css_class("dim-label");
    body_counter.add_css_class("caption");
    content.append(&body_counter);

    let error = gtk::Label::new(None);
    error.add_css_class("error");
    error.set_xalign(0.0);
    error.set_wrap(true);
    error.set_visible(false);
    content.append(&error);

    let btn_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    btn_row.set_halign(gtk::Align::End);
    let cancel_btn = gtk::Button::with_label(crate::tr_en!("Cancel"));
    let save_btn = gtk::Button::with_label(crate::tr_en!("Save"));
    save_btn.add_css_class("suggested-action");
    save_btn.set_receives_default(true);
    btn_row.append(&cancel_btn);
    btn_row.append(&save_btn);
    content.append(&btn_row);

    toolbar_view.set_content(Some(&content));
    dialog.set_content(Some(&toolbar_view));

    // Pre-fill when editing.
    let current_name = existing.as_ref().map(|g| g.name.clone());
    if let Some(g) = &existing {
        name_row.set_text(&g.name);
        desc_row.set_text(&g.description);
        body_buffer.set_text(&g.body);
    }

    // ── Live slug preview + counters ──
    let update_slug = {
        let name_row = name_row.clone();
        let slug_label = slug_label.clone();
        move || {
            let slug = AiGuideService::slug(&name_row.text());
            if slug.is_empty() {
                slug_label.set_text(crate::tr_en!(
                    "Enter a name using letters, numbers, spaces, or underscores."
                ));
            } else {
                slug_label.set_text(&crate::tr_fmt!("The agent will see: {}", slug));
            }
        }
    };
    let update_desc_counter = {
        let desc_row = desc_row.clone();
        let desc_counter = desc_counter.clone();
        move || {
            let n = desc_row.text().trim().chars().count();
            desc_counter.set_text(&format!("{n}/{MAX_DESCRIPTION_CHARS}"));
        }
    };
    let update_body_counter = {
        let body_buffer = body_buffer.clone();
        let body_counter = body_counter.clone();
        move || {
            let (s, e) = body_buffer.bounds();
            let n = body_buffer.text(&s, &e, false).trim().chars().count();
            body_counter.set_text(&format!("{n}/{MAX_BODY_CHARS}"));
        }
    };

    update_slug();
    update_desc_counter();
    update_body_counter();

    {
        let update_slug = update_slug.clone();
        name_row.connect_changed(move |_| update_slug());
    }
    {
        let update_desc_counter = update_desc_counter.clone();
        desc_row.connect_changed(move |_| update_desc_counter());
    }
    {
        let update_body_counter = update_body_counter.clone();
        body_buffer.connect_changed(move |_| update_body_counter());
    }

    let (tx, rx) = tokio::sync::oneshot::channel::<bool>();
    let tx = Rc::new(RefCell::new(Some(tx)));

    {
        let dialog = dialog.clone();
        let tx = tx.clone();
        cancel_btn.connect_clicked(move |_| {
            if let Some(tx) = tx.borrow_mut().take() {
                let _ = tx.send(false);
            }
            dialog.close();
        });
    }
    {
        let dialog = dialog.clone();
        let tx = tx.clone();
        let guide = guide.clone();
        let name_row = name_row.clone();
        let desc_row = desc_row.clone();
        let body_buffer = body_buffer.clone();
        let error = error.clone();
        save_btn.connect_clicked(move |_| {
            let name = name_row.text().to_string();
            let description = desc_row.text().to_string();
            let (start, end) = body_buffer.bounds();
            let body = body_buffer.text(&start, &end, false).to_string();

            let result = match &current_name {
                Some(current) => {
                    guide.update_guide(current, name.trim(), description.trim(), body.trim())
                }
                None => guide.add_guide(name.trim(), description.trim(), body.trim()),
            };
            match result {
                Ok(()) => {
                    if let Some(tx) = tx.borrow_mut().take() {
                        let _ = tx.send(true);
                    }
                    dialog.close();
                }
                Err(e) => {
                    error.set_text(&e);
                    error.set_visible(true);
                }
            }
        });
    }
    {
        let tx = tx.clone();
        dialog.connect_close_request(move |_| {
            if let Some(tx) = tx.borrow_mut().take() {
                let _ = tx.send(false);
            }
            glib::Propagation::Proceed
        });
    }

    dialog.present();
    rx.await.unwrap_or(false)
}

#[cfg(test)]
mod affordance_tests {
    //! Reading every tool's description had to be possible in one place.
    //!
    //! The page groups 137 tools into seventeen categories, which is the right
    //! default and the wrong only option: to read them all you had to open each
    //! category in turn, or invent a search string that matches everything. The
    //! reference offers the choice outright (`Guide_TilesRadio` /
    //! `Guide_EverythingRadio`), so this checks we do too.

    fn source() -> &'static str {
        crate::testing::code(include_str!("ai_guide_page.rs"))
    }

    #[test]
    fn every_tool_can_be_seen_at_once() {
        let code = source();
        assert!(
            code.contains(r#"crate::tr_en!("See everything")"#),
            "the launchpad needs a view that shows every tool, not only tiles"
        );
        assert!(
            code.contains("fn show_everything"),
            "and something that renders it"
        );
    }

    #[test]
    fn every_view_draws_a_tool_the_same_way() {
        // Focused category, search results and see-everything all render tool
        // rows. Three builders would be three places for the override editor to
        // drift; there is one.
        let code = source();
        let builders = code.matches("fn build_tool_row").count();
        assert_eq!(builders, 1, "found {builders} tool-row builders");
        assert!(
            code.matches("self.build_tool_row(").count() >= 3,
            "each view should render rows through the shared builder"
        );
    }

    #[test]
    fn leaving_a_category_keeps_the_chosen_view() {
        // Back-from-focus and cleared-search both call show_launchpad, so that
        // is where the choice has to be honoured — resetting a reader to Tiles
        // because they closed a category is losing their place.
        let after = source()
            .split("fn show_launchpad")
            .nth(1)
            .expect("the launchpad renderer exists");
        let body = &after[..after.find("\n    }\n").unwrap_or(after.len())];
        assert!(
            body.contains("everything_btn.is_active()"),
            "returning to the launchpad should honour the chosen view"
        );
    }
}

#[cfg(test)]
mod tests {
    /// Every advertised tool appears in the guide, or is excluded on purpose.
    ///
    /// The guide is where a user reads and re-tunes what the agent is told, so
    /// a tool missing from it is a tool nobody can see or correct.
    #[test]
    fn every_advertised_tool_is_in_the_guide() {
        use std::collections::HashSet;

        // Advertised but deliberately absent.
        //
        // The proposal-lifecycle four are plumbing: they act on the queue, not
        // on the archive, and re-describing them would not change what an agent
        // does. The other three are the macOS-parity ALIASES — the same tools
        // under a second name, already listed under their canonical one.
        const DELIBERATELY_ABSENT: &[&str] = &[
            "download_from_vospace",
            "get_proposal_state",
            "list_events",
            "list_pending_proposals",
            "upload_to_vospace",
            "vospace_mkdir",
            "withdraw_proposal",
        ];

        let advertised: HashSet<String> =
            crate::mcp::tools::router::McpToolRouter::all_descriptors()
                .into_iter()
                .filter(|d| d.agent_safe)
                .map(|d| d.name)
                .collect();
        let guided: HashSet<String> = super::all_live_descriptors()
            .into_iter()
            .map(|d| d.name)
            .collect();

        let mut missing: Vec<&str> = advertised
            .iter()
            .map(String::as_str)
            .filter(|n| !guided.contains(*n))
            .filter(|n| !DELIBERATELY_ABSENT.contains(n))
            .collect();
        missing.sort();

        assert!(
            missing.is_empty(),
            "tool(s) an agent is offered that the user cannot see or re-describe \
             in the AI Guide: {missing:#?}"
        );
    }

    /// No tool is dumped in the catch-all category.
    ///
    /// `category_id_for_tool` is a hand-kept match, and whole families were
    /// added without touching it: twenty-seven tools sat in "Other", including
    /// every search-UI tool an agent uses most. Nothing failed — they were
    /// listed, just under a heading that says nothing.
    #[test]
    fn every_tool_lands_in_a_real_category() {
        let mut uncategorised: Vec<String> = super::all_live_descriptors()
            .into_iter()
            .filter(|d| super::catalog::category_id_for_tool(&d.name) == "other")
            .map(|d| d.name)
            .collect();
        uncategorised.sort();

        assert!(
            uncategorised.is_empty(),
            "tool(s) in the catch-all category — add them to \
             `category_id_for_tool`: {uncategorised:#?}"
        );
    }

    /// Every category heading a user reads has a French form.
    ///
    /// These are user-facing chrome, not wire text, but they are variables at
    /// the call site — `NAMED` is a static and a static cannot call a function
    /// — so the generic `tr_en!` source scan cannot see them. Without this they
    /// would sit in a French UI in English and no guard would notice.
    #[test]
    fn every_category_label_has_a_french_form() {
        let mut untranslated = Vec::new();
        for cat in super::catalog::all() {
            for text in [cat.title, cat.summary] {
                if crate::i18n::french(text).is_none() {
                    untranslated.push(text.to_string());
                }
            }
        }
        assert!(
            untranslated.is_empty(),
            "AI Guide category text with no French form: {untranslated:#?}"
        );
    }

    use super::*;
    use serde_json::json;

    fn td(name: &str, verb: VerbClass) -> ToolDescriptor {
        ToolDescriptor {
            name: name.to_string(),
            description: format!("desc for {name}"),
            input_schema: json!({ "type": "object" }),
            verb,
            agent_safe: true,
        }
    }

    #[test]
    fn category_map_covers_live_tools_and_falls_back_to_other() {
        // Representative live names from each family map to their category.
        assert_eq!(
            catalog::category_id_for_tool("describe_app"),
            "foundational"
        );
        assert_eq!(catalog::category_id_for_tool("get_node"), "storage");
        assert_eq!(catalog::category_id_for_tool("read_file"), "storage");
        assert_eq!(catalog::category_id_for_tool("clear_outputs"), "notebook");
        assert_eq!(catalog::category_id_for_tool("run_all"), "notebook");
        assert_eq!(
            catalog::category_id_for_tool("get_session_logs"),
            "sessions"
        );
        assert_eq!(
            catalog::category_id_for_tool("list_observations"),
            "research"
        );
        assert_eq!(catalog::category_id_for_tool("open_fits_file"), "fits");
        assert_eq!(catalog::category_id_for_tool("list_guide_tools"), "guide");
        // Unknown → Other.
        assert_eq!(catalog::category_id_for_tool("totally_new_tool"), "other");
    }

    #[test]
    fn all_categories_are_ordered_with_other_last() {
        let ids: Vec<&str> = catalog::all().map(|c| c.id).collect();
        assert_eq!(ids.first().copied(), Some("foundational"));
        assert_eq!(ids.last().copied(), Some("other"));
        assert_eq!(ids.len(), catalog::NAMED.len() + 1);
    }

    #[test]
    fn categorize_groups_sorts_and_drops_empties() {
        let cats = categorize(vec![
            td("get_node", VerbClass::Read),         // storage
            td("delete_node", VerbClass::Write),     // storage
            td("describe_app", VerbClass::Read),     // foundational
            td("totally_new_tool", VerbClass::Read), // other
        ]);

        // Only the three non-empty categories survive, in catalog order.
        let ids: Vec<&str> = cats.iter().map(|c| c.id).collect();
        assert_eq!(ids, vec!["foundational", "storage", "other"]);

        // Storage's tools are sorted by name.
        let storage = cats.iter().find(|c| c.id == "storage").unwrap();
        let names: Vec<&str> = storage.tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["delete_node", "get_node"]);
    }

    #[test]
    fn every_live_descriptor_lands_in_a_known_category() {
        // No live built-in should silently vanish: each maps to a real category id.
        let known: std::collections::HashSet<&str> = catalog::all().map(|c| c.id).collect();
        for d in all_live_descriptors() {
            let id = catalog::category_id_for_tool(&d.name);
            assert!(
                known.contains(id),
                "tool {} → unknown category {id}",
                d.name
            );
        }
    }

    #[test]
    fn tool_count_text_pluralizes() {
        assert_eq!(tool_count_text(1), "1 tool");
        assert_eq!(tool_count_text(0), "0 tools");
        assert_eq!(tool_count_text(7), "7 tools");
    }
}

#[cfg(test)]
mod guide_descriptor_tests {
    //! The guide shows the descriptors the MCP server advertises, live.
    //!
    //! It does NOT keep its own copy of the text — which is the only reason a
    //! tool description written once is correct in both places. This pins that,
    //! because a second copy is exactly the drift that has bitten this file
    //! before.
    use super::*;

    #[test]
    fn a_new_tool_and_its_description_reach_the_guide() {
        let live = all_live_descriptors();
        let image = live
            .iter()
            .find(|d| d.name == "get_cell_image")
            .expect("get_cell_image is not in the guide's tool list");
        assert!(
            image.description.contains("image content"),
            "the guide is showing stale text: {}",
            image.description
        );

        // And a description edited in the descriptor shows through unchanged.
        let cell_output = live
            .iter()
            .find(|d| d.name == "get_cell_output")
            .expect("get_cell_output missing");
        assert!(
            cell_output.description.contains("richTypes"),
            "the guide does not mention richTypes: {}",
            cell_output.description
        );
        let run_cell = live
            .iter()
            .find(|d| d.name == "run_cell")
            .expect("run_cell missing");
        assert!(
            run_cell.description.contains("timeout"),
            "the guide does not mention the timeout: {}",
            run_cell.description
        );
    }
}
