//! Persisted notebook preferences.
//!
//! Port of `Services/Notebook/NotebookSettings.cs`. A plain serde struct with
//! sensible defaults, saved as JSON by
//! [`crate::services::notebook_settings_service::NotebookSettingsService`].
//!
//! `#[serde(default)]` on the container means a settings file written by an
//! older build (missing newer fields) still loads cleanly — each absent field
//! falls back to its [`Default`] value rather than failing the whole parse.

use serde::{Deserialize, Serialize};

/// User-configurable notebook editor / execution preferences.
/// Default for [`NotebookSettings::max_open_file_mb`].
pub const DEFAULT_MAX_OPEN_FILE_MB: u32 = 64;

/// Default for [`NotebookSettings::agent_image_max_dimension`].
///
/// About what a vision model resolves. Large enough that a FITS field is
/// readable, small enough that a capture is not most of a caller's context.
pub const DEFAULT_AGENT_IMAGE_MAX_DIMENSION: u32 = 1024;

/// Default for [`NotebookSettings::agent_image_max_bytes_mb`].
///
/// The budget `get_preview_image` has used since it was written; it is now the
/// budget for every image source rather than for one of them.
pub const DEFAULT_AGENT_IMAGE_MAX_BYTES_MB: u32 = 16;

/// Default for [`NotebookSettings::agent_result_max_kb`].
///
/// About 16 000 tokens of JSON — large enough for a real page of results, small
/// enough that a client does not spool it to a file. QA measured a single
/// search at 622 KB against this.
pub const DEFAULT_AGENT_RESULT_MAX_KB: u32 = 64;

/// Default for [`NotebookSettings::mcp_slim_tool_list`].
///
/// On. Measured: the full list is ~24 450 tokens before an agent has read the
/// task; the slim list plus the map in `instructions` is ~2 290, and every tool
/// stays callable by name. A client that drops `instructions` still receives
/// `list_apps`, `describe_app` and `search_tools`, so the discovery path
/// survives even there.
pub const DEFAULT_MCP_SLIM_TOOL_LIST: bool = true;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NotebookSettings {
    /// Explicit Python interpreter path. `None` means auto-detect.
    pub python_path: Option<String>,
    /// Code-cell font size in points/pixels.
    pub font_size: u32,
    /// Tab width in spaces.
    pub tab_size: u32,
    /// Whether code cells wrap long lines.
    pub word_wrap: bool,
    /// Whether the periodic autosave checkpoint is enabled.
    pub autosave_enabled: bool,
    /// Autosave interval in seconds.
    pub autosave_interval_secs: u32,
    /// Execution timeout warning threshold in seconds (`0` means never warn).
    pub execution_timeout_secs: u32,
    /// Whether the notebook toolbar is shown.
    pub show_toolbar: bool,
    /// Largest file, in MB, the notebook will open.
    ///
    /// The loader reads a file into memory whole before it can look at it, and
    /// the only limit was on the number of CELLS — which a `.txt` or `.py`
    /// reaches only after the bytes are already in RAM. In an astronomy folder
    /// the file most likely to be enormous is exactly the kind now openable: a
    /// `.txt` source catalogue dump beside the notes it belongs to.
    ///
    /// A setting rather than a fixed number because the right answer depends on
    /// the machine, and someone on a workstation should be able to raise it.
    pub max_open_file_mb: u32,
    /// Longest edge, in pixels, of an image handed to an AI agent.
    ///
    /// A capture of a viewer's working area is sent to a model that reads it at
    /// a few hundred pixels; a 4000px render costs roughly sixteen times the
    /// context for no more understanding. Scaled down, never up.
    pub agent_image_max_dimension: u32,
    /// Largest agent image, in MB, after scaling.
    pub agent_image_max_bytes_mb: u32,
    /// Advertise a SLIM `tools/list`: the catalog and foundational tools only.
    ///
    /// A client must call `tools/list` and put the result in context — that is
    /// the protocol, and no server can opt out. All 149 schemas measured 96 KB,
    /// about 24 000 tokens, spent before the agent has read the task.
    ///
    /// When this is on, `tools/list` carries the tools an agent needs without
    /// being told about, and `initialize`'s `instructions` carry the map: every
    /// app, what it is for, and the names it owns. Nothing becomes unreachable
    /// — an unadvertised agent-safe tool is still callable by name, and
    /// `describe_app` returns its arguments on demand.
    pub mcp_slim_tool_list: bool,
    /// Largest tool RESULT handed to an agent, in KB.
    ///
    /// A row cap is not a size cap: a `SELECT *` over `caom2.Observation` is
    /// some sixty columns, so the 1000-row limit still measured 622 KB in QA and
    /// the client wrote it to a file for the agent to grep. Rows are kept whole
    /// until this runs out, and what was dropped is always reported.
    pub agent_result_max_kb: u32,
}

impl Default for NotebookSettings {
    fn default() -> Self {
        // Mirrors the defaults in the C# reference.
        Self {
            python_path: None,
            font_size: 13,
            tab_size: 4,
            word_wrap: true,
            autosave_enabled: true,
            autosave_interval_secs: 30,
            execution_timeout_secs: 60,
            show_toolbar: true,
            // Comfortably above any real notebook — the largest in this repo's
            // fixtures is a few hundred KB — and far below the size at which
            // reading a file stalls the UI.
            max_open_file_mb: DEFAULT_MAX_OPEN_FILE_MB,
            agent_image_max_dimension: DEFAULT_AGENT_IMAGE_MAX_DIMENSION,
            agent_image_max_bytes_mb: DEFAULT_AGENT_IMAGE_MAX_BYTES_MB,
            agent_result_max_kb: DEFAULT_AGENT_RESULT_MAX_KB,
            mcp_slim_tool_list: DEFAULT_MCP_SLIM_TOOL_LIST,
        }
    }
}

impl NotebookSettings {
    /// Return a copy with all numeric fields clamped to sane ranges, so a
    /// hand-edited or corrupt settings file can never push absurd values into
    /// the UI (e.g. a 0px font). Applied on load.
    pub fn sanitized(mut self) -> Self {
        self.font_size = self.font_size.clamp(6, 48);
        self.tab_size = self.tab_size.clamp(1, 16);
        // autosave interval: at least 5s, at most an hour.
        self.autosave_interval_secs = self.autosave_interval_secs.clamp(5, 3600);
        // execution timeout: 0 (never) is allowed, otherwise cap at an hour.
        if self.execution_timeout_secs != 0 {
            self.execution_timeout_secs = self.execution_timeout_secs.clamp(5, 3600);
        }
        // A zero would make every file too big to open, which is not a
        // preference anyone holds; the ceiling stops a hand-edited value from
        // meaning "read whatever you find" on a machine that cannot.
        self.max_open_file_mb = self.max_open_file_mb.clamp(1, 4096);
        // A zero dimension would scale every capture to nothing, and a zero
        // budget would refuse every one of them — neither is a preference.
        self.agent_image_max_dimension = self.agent_image_max_dimension.clamp(64, 8192);
        self.agent_image_max_bytes_mb = self.agent_image_max_bytes_mb.clamp(1, 256);
        // Below a few KB nothing useful fits; above a few MB the client spools
        // the reply to a file and the agent is reading a document again.
        self.agent_result_max_kb = self.agent_result_max_kb.clamp(4, 4096);
        // Normalise an all-whitespace python path to None.
        if let Some(p) = &self.python_path {
            if p.trim().is_empty() {
                self.python_path = None;
            }
        }
        self
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_reference() {
        let s = NotebookSettings::default();
        assert_eq!(s.font_size, 13);
        assert_eq!(s.tab_size, 4);
        assert!(s.word_wrap);
        assert!(s.autosave_enabled);
        assert_eq!(s.autosave_interval_secs, 30);
        assert_eq!(s.execution_timeout_secs, 60);
        assert!(s.show_toolbar);
        assert_eq!(s.python_path, None);
    }

    /// The open-size limit is clamped, in both directions.
    ///
    /// Zero would make every file too large to open — not a preference anyone
    /// holds — and an unbounded value from a hand-edited file would mean "read
    /// whatever you find" on a machine that cannot.
    #[test]
    fn the_open_size_limit_cannot_be_set_to_something_useless() {
        let s = NotebookSettings {
            max_open_file_mb: 0,
            ..Default::default()
        }
        .sanitized();
        assert!(s.max_open_file_mb >= 1, "zero blocks every file");

        let s = NotebookSettings {
            max_open_file_mb: u32::MAX,
            ..Default::default()
        }
        .sanitized();
        assert!(s.max_open_file_mb <= 4096, "no ceiling at all");

        // A sensible value is left alone.
        let s = NotebookSettings {
            max_open_file_mb: 128,
            ..Default::default()
        }
        .sanitized();
        assert_eq!(s.max_open_file_mb, 128);
    }

    #[test]
    fn serde_round_trip_preserves_all_fields() {
        let s = NotebookSettings {
            python_path: Some("/opt/py/bin/python3".to_string()),
            font_size: 16,
            tab_size: 2,
            word_wrap: false,
            autosave_enabled: false,
            autosave_interval_secs: 60,
            execution_timeout_secs: 0,
            show_toolbar: false,
            max_open_file_mb: 256,
            agent_image_max_dimension: 1024,
            agent_image_max_bytes_mb: 16,
            agent_result_max_kb: 64,
            mcp_slim_tool_list: false,
        };
        let json = serde_json::to_string_pretty(&s).expect("serialise");
        let back: NotebookSettings = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(s, back);
    }

    #[test]
    fn partial_json_fills_defaults() {
        // Only two fields present — the rest must fall back to defaults.
        let json = r#"{"font_size":20,"word_wrap":false}"#;
        let s: NotebookSettings = serde_json::from_str(json).expect("deserialise");
        assert_eq!(s.font_size, 20);
        assert!(!s.word_wrap);
        // Untouched fields keep their defaults.
        assert_eq!(s.tab_size, 4);
        assert_eq!(s.autosave_interval_secs, 30);
        assert!(s.show_toolbar);
    }

    #[test]
    fn empty_json_object_is_all_defaults() {
        let s: NotebookSettings = serde_json::from_str("{}").expect("deserialise");
        assert_eq!(s, NotebookSettings::default());
    }

    #[test]
    fn sanitized_clamps_absurd_values() {
        let s = NotebookSettings {
            font_size: 0,
            tab_size: 999,
            autosave_interval_secs: 1,
            execution_timeout_secs: 100_000,
            python_path: Some("   ".to_string()),
            ..Default::default()
        }
        .sanitized();
        assert_eq!(s.font_size, 6);
        assert_eq!(s.tab_size, 16);
        assert_eq!(s.autosave_interval_secs, 5);
        assert_eq!(s.execution_timeout_secs, 3600);
        assert_eq!(s.python_path, None);
    }

    #[test]
    fn sanitized_allows_never_timeout() {
        let s = NotebookSettings {
            execution_timeout_secs: 0,
            ..Default::default()
        }
        .sanitized();
        assert_eq!(s.execution_timeout_secs, 0);
    }
}
