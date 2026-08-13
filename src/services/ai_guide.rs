//! AI Guide state: per-tool description **overrides** and user-authored
//! read-only **guide tools**. Ported from the Windows reference
//! `Services/AiGuide/*.cs`, but backed by a single JSON file instead of SQLite.
//!
//! Two kinds of state:
//!  * **Overrides** — a sparse delta over a built-in tool's default description.
//!    A built-in tool's default is the single source of truth and is *never*
//!    stored; only the user's replacement text is. "Reset" simply drops the key.
//!  * **Guide tools** — named, read-only instructions the agent discovers in
//!    `tools/list` and can *call* to receive their body (a generic handler in
//!    the MCP bridge returns the stored text — there is no execution). The
//!    agent-facing name is a sanitized slug; uniqueness among live guides is
//!    enforced here.
//!
//! The Windows SQLite sync-only columns (`version` / `deviceID` / `deletedAt`)
//! are intentionally dropped — they are meaningless without cross-device sync.
//!
//! State lives behind a [`std::sync::Mutex`] so the MCP serve loop can capture
//! an immutable [`AiGuideSnapshot`] while the UI thread edits. Every mutation is
//! persisted (under the lock) to
//! `ProjectDirs("net","canfar","Verbinal").data_dir()/ai_guide.json` via an
//! atomic temp-write + rename.

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

/// Description cap — generous enough for a re-tuning paragraph, bounded so a
/// pathological value can't bloat the wire manifest. Mirrors the Windows cap.
/// Public so the edit dialog can render a `n/limit` counter without duplicating it.
pub const MAX_DESCRIPTION_CHARS: usize = 600;
/// Body cap for a guide tool's instruction text. Mirrors the Windows cap.
/// Public for the edit dialog's live character counter.
pub const MAX_BODY_CHARS: usize = 4000;

/// A user-authored guide tool: an agent-facing slug `name`, the one-line
/// `description` shown in `tools/list`, and the `body` returned when the agent
/// calls it. `body` may be empty — in that case the description stands alone as
/// the call payload (see [`AiGuideSnapshot::guide_body`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuideTool {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub body: String,
}

/// Immutable snapshot the MCP bridge reads to (a) substitute descriptions in
/// `tools/list` and (b) list + answer user guide tools. Built from
/// [`AiGuideService`] state under the lock, so it crosses to the MCP thread
/// without a race.
#[derive(Debug, Clone, Default)]
pub struct AiGuideSnapshot {
    pub overrides: HashMap<String, String>,
    pub guides: Vec<GuideTool>,
}

impl AiGuideSnapshot {
    /// Effective description for a built-in tool: the override if present, else
    /// the caller's built-in default.
    pub fn description_for_tool(&self, name: &str, default: &str) -> String {
        match self.overrides.get(name) {
            Some(d) => d.clone(),
            None => default.to_string(),
        }
    }

    /// The payload a guide-tool call returns, or `None` if `name` isn't a live
    /// guide. A guide with an empty body falls back to its description (a
    /// one-liner can stand alone as its own answer), matching the Windows
    /// `CallPayload`.
    pub fn guide_body(&self, name: &str) -> Option<String> {
        self.guides.iter().find(|g| g.name == name).map(|g| {
            if g.body.trim().is_empty() {
                g.description.clone()
            } else {
                g.body.clone()
            }
        })
    }
}

/// On-disk shape: `{ "overrides": { tool: desc }, "guides": [ {name,description,body} ] }`.
#[derive(Debug, Default, Serialize, Deserialize)]
struct PersistState {
    #[serde(default)]
    overrides: HashMap<String, String>,
    #[serde(default)]
    guides: Vec<GuideTool>,
}

/// Owns the AI Guide state (overrides + guide tools), guarded by a mutex and
/// persisted to a JSON file. Synchronous throughout: the MCP serve loop reads a
/// [`AiGuideSnapshot`], the UI edits.
pub struct AiGuideService {
    state: Mutex<PersistState>,
    file_path: PathBuf,
}

impl AiGuideService {
    /// Construct the service, loading any existing state from
    /// `data_dir()/ai_guide.json`.
    pub fn new() -> Self {
        let file_path = ProjectDirs::from("net", "canfar", "Verbinal")
            .map(|dirs| dirs.data_dir().join("ai_guide.json"))
            .unwrap_or_else(|| PathBuf::from("ai_guide.json"));
        let state = load(&file_path);
        AiGuideService {
            state: Mutex::new(state),
            file_path,
        }
    }

    /// Test-only constructor pointing at an explicit JSON file.
    #[cfg(test)]
    fn with_file(file_path: PathBuf) -> Self {
        let state = load(&file_path);
        AiGuideService {
            state: Mutex::new(state),
            file_path,
        }
    }

    /// Immutable snapshot for the MCP bridge (built under the lock).
    pub fn snapshot(&self) -> AiGuideSnapshot {
        let state = self.state.lock().unwrap();
        AiGuideSnapshot {
            overrides: state.overrides.clone(),
            guides: state.guides.clone(),
        }
    }

    /// Set (or, with blank text, clear) the override for a built-in tool. Trims
    /// whitespace and caps the length to [`MAX_DESCRIPTION_CHARS`] characters.
    pub fn set_override(&self, tool: &str, description: &str) {
        let trimmed = description.trim();
        if trimmed.is_empty() {
            self.clear_override(tool);
            return;
        }
        let capped = cap_chars(trimmed, MAX_DESCRIPTION_CHARS);
        let mut state = self.state.lock().unwrap();
        state.overrides.insert(tool.to_string(), capped);
        self.persist_locked(&state);
    }

    /// Reset a tool to its built-in description (drop the override).
    pub fn clear_override(&self, tool: &str) {
        let mut state = self.state.lock().unwrap();
        if state.overrides.remove(tool).is_some() {
            self.persist_locked(&state);
        }
    }

    /// Create a new guide tool. The display `name` is sanitized into an
    /// agent-facing slug; the slug must be non-empty and unique among live
    /// guides, and the description must be non-empty (it is the `tools/list`
    /// one-liner and the call fallback). `body` may be empty. Returns an error
    /// string on any validation failure.
    pub fn add_guide(&self, name: &str, description: &str, body: &str) -> Result<(), String> {
        let slug = slug(name);
        if slug.is_empty() {
            return Err("Enter a name using letters, numbers, spaces, or underscores.".to_string());
        }
        let desc = description.trim();
        if desc.is_empty() {
            return Err(
                "Enter a one-line description the agent will see in the tool list.".to_string(),
            );
        }
        let desc = cap_chars(desc, MAX_DESCRIPTION_CHARS);
        let body = cap_chars(body.trim(), MAX_BODY_CHARS);

        let mut state = self.state.lock().unwrap();
        if state.guides.iter().any(|g| g.name == slug) {
            return Err("You already have a guide tool with this name.".to_string());
        }
        state.guides.push(GuideTool {
            name: slug,
            description: desc,
            body,
        });
        self.persist_locked(&state);
        Ok(())
    }

    /// Update an existing guide tool, identified by its **current** slug. The
    /// (possibly changed) display `name` is re-slugged and re-validated; the new
    /// slug must be non-empty and must not collide with a *different* live guide.
    /// The description must be non-empty; `body` may be empty. Mirrors the Windows
    /// `AiGuideService.UpdateGuide` (which keys off an id — here the slug is the
    /// identity). Returns an error string on any validation failure or if the
    /// target guide no longer exists. Ported for the AI Guide "Edit" affordance.
    pub fn update_guide(
        &self,
        current_name: &str,
        name: &str,
        description: &str,
        body: &str,
    ) -> Result<(), String> {
        let current_slug = slug(current_name);
        let new_slug = slug(name);
        if new_slug.is_empty() {
            return Err("Enter a name using letters, numbers, spaces, or underscores.".to_string());
        }
        let desc = description.trim();
        if desc.is_empty() {
            return Err(
                "Enter a one-line description the agent will see in the tool list.".to_string(),
            );
        }
        let desc = cap_chars(desc, MAX_DESCRIPTION_CHARS);
        let body = cap_chars(body.trim(), MAX_BODY_CHARS);

        let mut state = self.state.lock().unwrap();
        // Locate the target by its current slug (accept the raw current name too,
        // so callers may pass either the stored slug or a display form).
        let idx = match state
            .guides
            .iter()
            .position(|g| g.name == current_slug || g.name == current_name)
        {
            Some(i) => i,
            None => return Err("That guide tool no longer exists.".to_string()),
        };
        // The new slug may equal the target's own slug (a description/body-only
        // edit) but must not shadow any *other* live guide.
        if state
            .guides
            .iter()
            .enumerate()
            .any(|(i, g)| i != idx && g.name == new_slug)
        {
            return Err("You already have a guide tool with this name.".to_string());
        }
        let entry = &mut state.guides[idx];
        entry.name = new_slug;
        entry.description = desc;
        entry.body = body;
        self.persist_locked(&state);
        Ok(())
    }

    /// Public slug preview — turns a display name into the agent-facing tool name.
    /// Mirrors the Windows `AiGuideService.Slug`; used by the edit dialog to show a
    /// live "the agent will see …" hint as the user types.
    pub fn slug(s: &str) -> String {
        slug(s)
    }

    /// Remove a guide tool by name. Matches either the stored slug or the slug
    /// of the supplied (possibly display-form) name, so callers may pass either.
    pub fn remove_guide(&self, name: &str) {
        let slug = slug(name);
        let mut state = self.state.lock().unwrap();
        let before = state.guides.len();
        state.guides.retain(|g| g.name != name && g.name != slug);
        if state.guides.len() != before {
            self.persist_locked(&state);
        }
    }

    #[cfg(test)]
    /// All overrides as `(tool, description)` pairs, sorted by tool name for a
    /// stable UI ordering.
    pub fn list_overrides(&self) -> Vec<(String, String)> {
        let state = self.state.lock().unwrap();
        let mut out: Vec<(String, String)> = state
            .overrides
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// All live guide tools, in insertion order.
    pub fn list_guides(&self) -> Vec<GuideTool> {
        self.state.lock().unwrap().guides.clone()
    }

    /// Serialize the current state to `file_path` via an atomic temp-write +
    /// rename. IO errors are swallowed (best-effort persistence): the in-memory
    /// mirror remains the source of truth for the running session.
    fn persist_locked(&self, state: &PersistState) {
        let _ = write_atomic(&self.file_path, state);
    }
}

impl Default for AiGuideService {
    fn default() -> Self {
        Self::new()
    }
}

/// Load persisted state, tolerating a missing or corrupt file (returns empty).
fn load(path: &PathBuf) -> PersistState {
    match std::fs::read_to_string(path) {
        Ok(contents) => serde_json::from_str(&contents).unwrap_or_default(),
        Err(_) => PersistState::default(),
    }
}

/// Write `state` as pretty JSON to `path` atomically: serialize to a sibling
/// `.tmp` file then rename over the target (same directory ⇒ same filesystem, so
/// the rename is atomic and never leaves a half-written file).
fn write_atomic(path: &PathBuf, state: &PersistState) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(state).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

/// Truncate `s` to at most `max` characters (never splitting a UTF-8 codepoint).
fn cap_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// Turn a display name into a valid MCP tool name: lowercase ASCII
/// alphanumerics, with spaces/dashes/dots/underscores collapsed to a single
/// `_`, trimmed of leading/trailing underscores. Non-ASCII letters are dropped
/// (the agent-facing name must be wire-safe). Ported from the Windows
/// `AiGuideService.Slug`.
fn slug(s: &str) -> String {
    let mut sb = String::with_capacity(s.len());
    for ch in s.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            sb.push(ch);
        } else if matches!(ch, ' ' | '-' | '_' | '.') {
            sb.push('_');
        }
    }
    // Collapse runs of '_' then trim the ends.
    let mut collapsed = String::with_capacity(sb.len());
    let mut prev_underscore = false;
    for ch in sb.chars() {
        if ch == '_' {
            if !prev_underscore {
                collapsed.push('_');
            }
            prev_underscore = true;
        } else {
            collapsed.push(ch);
            prev_underscore = false;
        }
    }
    collapsed.trim_matches('_').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn temp_file() -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir =
            std::env::temp_dir().join(format!("verbinal_ai_guide_{}_{}", std::process::id(), n));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("ai_guide.json")
    }

    #[test]
    fn override_round_trip_and_persists() {
        let path = temp_file();
        let svc = AiGuideService::with_file(path.clone());

        svc.set_override("read_file", "Reads a VOSpace file for the agent.");
        assert_eq!(
            svc.snapshot().description_for_tool("read_file", "DEFAULT"),
            "Reads a VOSpace file for the agent."
        );
        assert_eq!(
            svc.list_overrides(),
            vec![(
                "read_file".to_string(),
                "Reads a VOSpace file for the agent.".to_string()
            )]
        );

        // A fresh service over the same file sees the persisted override.
        let reopened = AiGuideService::with_file(path.clone());
        assert_eq!(
            reopened
                .snapshot()
                .description_for_tool("read_file", "DEFAULT"),
            "Reads a VOSpace file for the agent."
        );

        // Clearing drops the key everywhere, and blank text clears too.
        svc.clear_override("read_file");
        assert!(svc.list_overrides().is_empty());
        svc.set_override("read_file", "temp");
        svc.set_override("read_file", "   ");
        assert!(svc.list_overrides().is_empty());
    }

    #[test]
    fn add_and_remove_guide() {
        let path = temp_file();
        let svc = AiGuideService::with_file(path);

        svc.add_guide("My Guide", "How to observe", "Step 1. Step 2.")
            .unwrap();
        let guides = svc.list_guides();
        assert_eq!(guides.len(), 1);
        // Name is slugged.
        assert_eq!(guides[0].name, "my_guide");
        assert_eq!(guides[0].description, "How to observe");
        assert_eq!(guides[0].body, "Step 1. Step 2.");

        // Duplicate slug is rejected.
        let dup = svc.add_guide("my  guide", "again", "");
        assert!(dup.is_err());

        // Empty name and empty description are rejected.
        assert!(svc.add_guide("!!!", "desc", "").is_err());
        assert!(svc.add_guide("Valid Name", "   ", "body").is_err());

        // Removal by either the slug or the display form works.
        svc.remove_guide("My Guide");
        assert!(svc.list_guides().is_empty());
    }

    #[test]
    fn snapshot_description_fallback() {
        let path = temp_file();
        let svc = AiGuideService::with_file(path);
        let snap = svc.snapshot();

        // No override ⇒ the built-in default flows through.
        assert_eq!(
            snap.description_for_tool("list_dir", "List a directory."),
            "List a directory."
        );

        // guide_body: body when present, description when the body is blank,
        // None for an unknown guide.
        svc.add_guide("with_body", "one liner", "the full body")
            .unwrap();
        svc.add_guide("no_body", "the one liner answer", "   ")
            .unwrap();
        let snap = svc.snapshot();
        assert_eq!(
            snap.guide_body("with_body").as_deref(),
            Some("the full body")
        );
        assert_eq!(
            snap.guide_body("no_body").as_deref(),
            Some("the one liner answer")
        );
        assert_eq!(snap.guide_body("missing"), None);
    }

    #[test]
    fn update_guide_edits_and_revalidates() {
        let path = temp_file();
        let svc = AiGuideService::with_file(path.clone());

        svc.add_guide("My Guide", "How to observe", "Step 1.")
            .unwrap();
        svc.add_guide("Other Guide", "Second", "Body.").unwrap();

        // Description/body-only edit keeps the same slug.
        svc.update_guide("my_guide", "My Guide", "Revised desc", "New body")
            .unwrap();
        let g = svc.list_guides();
        assert_eq!(g[0].name, "my_guide");
        assert_eq!(g[0].description, "Revised desc");
        assert_eq!(g[0].body, "New body");

        // Renaming re-slugs the entry; accepts the raw current name too.
        svc.update_guide("My Guide", "Survey Plan", "desc", "")
            .unwrap();
        let g = svc.list_guides();
        assert_eq!(g[0].name, "survey_plan");
        assert_eq!(g[0].body, "");

        // Collision with a *different* live guide is rejected; the entry is unchanged.
        let err = svc.update_guide("survey_plan", "Other Guide", "x", "");
        assert!(err.is_err());
        assert_eq!(svc.list_guides()[0].name, "survey_plan");

        // Empty name / empty description are rejected.
        assert!(svc.update_guide("survey_plan", "!!!", "d", "").is_err());
        assert!(svc
            .update_guide("survey_plan", "Survey Plan", "  ", "b")
            .is_err());

        // Unknown target errors out.
        assert!(svc.update_guide("nope", "Whatever", "d", "").is_err());

        // Edit persists across a reopen.
        let reopened = AiGuideService::with_file(path);
        assert_eq!(reopened.list_guides()[0].name, "survey_plan");
    }

    #[test]
    fn public_slug_matches_free_fn() {
        assert_eq!(AiGuideService::slug("My Guide"), "my_guide");
        assert_eq!(AiGuideService::slug("!!!"), "");
    }

    #[test]
    fn slug_matches_windows_rules() {
        assert_eq!(slug("My Guide"), "my_guide");
        assert_eq!(slug("  Hello--World.. "), "hello_world");
        assert_eq!(slug("a__b___c"), "a_b_c");
        assert_eq!(slug("Café ☕"), "caf"); // non-ASCII letters dropped
        assert_eq!(slug("!!!"), "");
    }
}
