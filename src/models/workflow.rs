//! Data model for the Workflows (research-protocols) module.
//!
//! Ported from `Services/Workflows/WorkflowFormat.cs` (records) — a `.workflow.md`
//! document is a title + description + metadata + an ordered list of check-off
//! steps. The file itself IS the state (see `helpers::workflow_format`).

/// One step of a workflow: title, body, agent-tool hints, an optional app view
/// deep-link, an optional free-text note, and its check-off state.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkflowStep {
    pub index: usize,
    pub title: String,
    pub body: String,
    pub tools: Vec<String>,
    pub view: Option<String>,
    pub note: Option<String>,
    pub done: bool,
}

/// A parsed workflow document. `warnings` carries tolerant-parse diagnostics
/// (never fatal) for the editor and template tests.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkflowDoc {
    pub title: String,
    pub description: String,
    /// Preamble `Key: value` metadata, in document order (lookup is case-insensitive).
    pub metadata: Vec<(String, String)>,
    pub steps: Vec<WorkflowStep>,
    pub warnings: Vec<String>,
}

impl WorkflowDoc {
    /// Case-insensitive metadata lookup.
    pub fn metadata_get(&self, key: &str) -> Option<&str> {
        self.metadata
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
    }

    /// The comma-split `Tags` metadata value (trimmed, empties removed).
    pub fn tags(&self) -> Vec<String> {
        match self.metadata_get("Tags") {
            Some(t) => t
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
            None => Vec::new(),
        }
    }

    /// Number of checked-off steps.
    pub fn done_count(&self) -> usize {
        self.steps.iter().filter(|s| s.done).count()
    }
}

/// Where a workflow comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowSource {
    /// Read-only bundled template (`builtin:<slug>`).
    BuiltIn,
    /// User working copy on disk (`local:<slug>`) — the only writable tier.
    Local,
    /// A copy stored in VOSpace (`vospace:<path>`).
    VoSpace,
}

/// A workflow together with its identity, source, and raw text.
#[derive(Debug, Clone)]
pub struct WorkflowInfo {
    pub id: String,
    pub source: WorkflowSource,
    pub doc: WorkflowDoc,
    pub raw_text: String,
}
