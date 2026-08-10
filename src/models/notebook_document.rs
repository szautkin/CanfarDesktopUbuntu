use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Top-level document
// ---------------------------------------------------------------------------

/// Jupyter notebook in nbformat 4.x.
///
/// The wire format is defined at
/// <https://nbformat.readthedocs.io/en/latest/format_description.html>
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotebookDocument {
    pub nbformat: u32,
    pub nbformat_minor: u32,
    pub metadata: NotebookMetadata,
    pub cells: Vec<NotebookCell>,
}

impl NotebookDocument {
    /// Return a blank notebook that is valid nbformat 4.5:
    /// one empty code cell with a fresh cell id.
    pub fn create_empty() -> Self {
        let cell_id = Self::generate_cell_id();
        NotebookDocument {
            nbformat: 4,
            nbformat_minor: 5,
            metadata: NotebookMetadata {
                kernelspec: None,
                language_info: None,
                extra: HashMap::new(),
            },
            cells: vec![NotebookCell {
                cell_type: "code".to_string(),
                source: CellSource::Single(String::new()),
                outputs: Vec::new(),
                execution_count: None,
                id: Some(cell_id),
                metadata: serde_json::Map::new(),
            }],
        }
    }

    /// Generate a random 8-character lowercase hex cell ID.
    ///
    /// Uses the system time as entropy source; good enough for a desktop app
    /// where strict uniqueness requirements are relaxed.
    pub fn generate_cell_id() -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        // Mix in a thread-local counter to avoid collisions when called rapidly.
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
        format!("{:04x}{:04x}", nanos & 0xFFFF, seq & 0xFFFF)
    }
}

// ---------------------------------------------------------------------------
// Metadata
// ---------------------------------------------------------------------------

/// Top-level notebook metadata block.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotebookMetadata {
    #[serde(default)]
    pub kernelspec: Option<KernelSpec>,
    #[serde(default)]
    pub language_info: Option<LanguageInfo>,
    /// Any additional metadata fields the notebook may carry.
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

/// Kernel specification embedded in the notebook metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KernelSpec {
    pub name: String,
    pub display_name: String,
    #[serde(default)]
    pub language: Option<String>,
}

/// Language information embedded in the notebook metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LanguageInfo {
    pub name: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Cells
// ---------------------------------------------------------------------------

/// A single notebook cell.  `cell_type` is `"code"`, `"markdown"`, or `"raw"`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotebookCell {
    pub cell_type: String,
    pub source: CellSource,
    #[serde(default)]
    pub outputs: Vec<CellOutput>,
    #[serde(default)]
    pub execution_count: Option<u32>,
    /// nbformat 4.5+ requires each cell to have a unique id.
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub metadata: serde_json::Map<String, serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Cell source
// ---------------------------------------------------------------------------

/// nbformat allows cell source to be either a single string or an array of
/// lines.  Both representations are valid on the wire.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CellSource {
    /// Source stored as a single pre-joined string.
    Single(String),
    /// Source stored as an array of lines (each may or may not end in `\n`).
    Lines(Vec<String>),
}

impl CellSource {
    /// Join all lines into a single string.
    ///
    /// When the source is already a single string it is returned as-is.
    /// When it is an array the lines are concatenated without any extra
    /// separator — each line in the nbformat spec already carries its own
    /// trailing `\n` if required.
    pub fn joined(&self) -> String {
        match self {
            CellSource::Single(s) => s.clone(),
            CellSource::Lines(lines) => lines.concat(),
        }
    }

    /// Return the source as an owned `Vec<String>` of lines.
    ///
    /// When the source is already an array it is cloned directly.
    /// When it is a single string it is split on `\n`, re-attaching the
    /// newline to every line except the last (matching nbformat convention).
    pub fn lines(&self) -> Vec<String> {
        match self {
            CellSource::Lines(v) => v.clone(),
            CellSource::Single(s) => {
                if s.is_empty() {
                    return Vec::new();
                }
                let mut result: Vec<String> = s.split('\n').map(|part| part.to_string()).collect();
                // Re-attach newlines to all lines except the last.
                let last = result.len().saturating_sub(1);
                for (i, line) in result.iter_mut().enumerate() {
                    if i < last {
                        line.push('\n');
                    }
                }
                // Drop trailing empty string produced by a terminal newline.
                if result.last().map(|l| l.is_empty()).unwrap_or(false) {
                    result.pop();
                }
                result
            }
        }
    }

    /// Return `true` if the source contains no text.
    pub fn is_empty(&self) -> bool {
        match self {
            CellSource::Single(s) => s.is_empty(),
            CellSource::Lines(v) => v.is_empty() || v.iter().all(|l| l.is_empty()),
        }
    }
}

// ---------------------------------------------------------------------------
// Cell outputs
// ---------------------------------------------------------------------------

/// A tagged output attached to a code cell.
///
/// The `output_type` field on the wire drives the enum variant selection.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "output_type")]
pub enum CellOutput {
    /// Stdout/stderr stream output.
    #[serde(rename = "stream")]
    Stream { name: String, text: CellSource },

    /// Output produced by a cell execution (rich display).
    #[serde(rename = "execute_result")]
    ExecuteResult {
        execution_count: Option<u32>,
        data: OutputData,
        #[serde(default)]
        metadata: serde_json::Map<String, serde_json::Value>,
    },

    /// Display output not tied to a specific execution count.
    #[serde(rename = "display_data")]
    DisplayData {
        data: OutputData,
        #[serde(default)]
        metadata: serde_json::Map<String, serde_json::Value>,
    },

    /// Error / exception output.
    #[serde(rename = "error")]
    Error {
        ename: String,
        evalue: String,
        traceback: Vec<String>,
    },
}

// ---------------------------------------------------------------------------
// Output data MIME bundle
// ---------------------------------------------------------------------------

/// A MIME-keyed bundle of output representations.
///
/// The well-known MIME types are given named fields for ergonomic access;
/// any additional MIME types are captured by `extra`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OutputData {
    /// Plain-text representation.
    #[serde(rename = "text/plain", default)]
    pub text_plain: Option<CellSource>,

    /// HTML representation.
    #[serde(rename = "text/html", default)]
    pub text_html: Option<CellSource>,

    /// PNG image as a base64-encoded string.
    #[serde(rename = "image/png", default)]
    pub image_png: Option<String>,

    /// JPEG image as a base64-encoded string.
    #[serde(rename = "image/jpeg", default)]
    pub image_jpeg: Option<String>,

    /// All other MIME types (e.g. `application/json`, `text/latex`).
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

impl OutputData {
    /// Return the best available plain-text representation.
    pub fn plain_text(&self) -> Option<String> {
        self.text_plain.as_ref().map(|s| s.joined())
    }

    /// Return `true` if there is at least one image representation.
    pub fn has_image(&self) -> bool {
        self.image_png.is_some() || self.image_jpeg.is_some()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- CellSource ---

    #[test]
    fn cell_source_single_joined() {
        let src = CellSource::Single("hello\nworld".to_string());
        assert_eq!(src.joined(), "hello\nworld");
    }

    #[test]
    fn cell_source_lines_joined() {
        let src = CellSource::Lines(vec!["hello\n".to_string(), "world".to_string()]);
        assert_eq!(src.joined(), "hello\nworld");
    }

    #[test]
    fn cell_source_single_to_lines() {
        let src = CellSource::Single("hello\nworld".to_string());
        let lines = src.lines();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "hello\n");
        assert_eq!(lines[1], "world");
    }

    #[test]
    fn cell_source_empty_single() {
        let src = CellSource::Single(String::new());
        assert!(src.is_empty());
        assert!(src.lines().is_empty());
    }

    #[test]
    fn cell_source_lines_is_empty() {
        let src = CellSource::Lines(vec!["".to_string()]);
        assert!(src.is_empty());
    }

    // --- Serialisation round-trip ---

    #[test]
    fn notebook_round_trip_minimal() {
        let nb = NotebookDocument::create_empty();
        let json = serde_json::to_string(&nb).expect("serialise");
        let back: NotebookDocument = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(back.nbformat, 4);
        assert_eq!(back.nbformat_minor, 5);
        assert_eq!(back.cells.len(), 1);
        assert_eq!(back.cells[0].cell_type, "code");
    }

    #[test]
    fn notebook_round_trip_with_outputs() {
        let mut nb = NotebookDocument::create_empty();
        nb.cells[0].outputs.push(CellOutput::Stream {
            name: "stdout".to_string(),
            text: CellSource::Single("hello\n".to_string()),
        });
        nb.cells[0].outputs.push(CellOutput::ExecuteResult {
            execution_count: Some(1),
            data: OutputData {
                text_plain: Some(CellSource::Single("42".to_string())),
                ..Default::default()
            },
            metadata: serde_json::Map::new(),
        });
        let json = serde_json::to_string_pretty(&nb).expect("serialise");
        let back: NotebookDocument = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(back.cells[0].outputs.len(), 2);
    }

    #[test]
    fn cell_source_array_round_trip() {
        // Build JSON programmatically to avoid backslash escaping in raw strings.
        let json = "[\"import os\\n\",\"import sys\"]";
        let src: CellSource = serde_json::from_str(json).expect("deserialise");
        assert_eq!(src.joined(), "import os\nimport sys");
        let re_serialised = serde_json::to_string(&src).expect("serialise");
        let back: CellSource = serde_json::from_str(&re_serialised).expect("deserialise");
        assert_eq!(back.joined(), src.joined());
    }

    #[test]
    fn output_data_plain_text() {
        let od = OutputData {
            text_plain: Some(CellSource::Single("result = 7".to_string())),
            ..Default::default()
        };
        assert_eq!(od.plain_text().as_deref(), Some("result = 7"));
        assert!(!od.has_image());
    }

    #[test]
    fn output_data_has_image() {
        let od = OutputData {
            image_png: Some("base64data".to_string()),
            ..Default::default()
        };
        assert!(od.has_image());
    }

    #[test]
    fn create_empty_has_cell_id() {
        let nb = NotebookDocument::create_empty();
        assert!(nb.cells[0].id.is_some());
        let id = nb.cells[0].id.as_deref().unwrap();
        assert_eq!(id.len(), 8);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn generate_cell_id_unique() {
        let ids: Vec<String> = (0..20)
            .map(|_| NotebookDocument::generate_cell_id())
            .collect();
        let unique: std::collections::HashSet<&String> = ids.iter().collect();
        assert_eq!(unique.len(), ids.len(), "IDs must all be unique");
    }

    #[test]
    fn kernel_spec_round_trip() {
        let json = r#"{"name":"python3","display_name":"Python 3","language":"python"}"#;
        let ks: KernelSpec = serde_json::from_str(json).expect("deserialise");
        assert_eq!(ks.name, "python3");
        assert_eq!(ks.language.as_deref(), Some("python"));
        let back = serde_json::to_string(&ks).expect("serialise");
        assert!(back.contains("python3"));
    }

    #[test]
    fn metadata_extra_fields_preserved() {
        let json =
            r#"{"kernelspec":null,"language_info":null,"custom_key":"custom_value","number":42}"#;
        let meta: NotebookMetadata = serde_json::from_str(json).expect("deserialise");
        assert!(meta.extra.contains_key("custom_key"));
        assert_eq!(
            meta.extra["custom_key"],
            serde_json::Value::String("custom_value".to_string())
        );
    }

    #[test]
    fn error_output_round_trip() {
        let output = CellOutput::Error {
            ename: "ValueError".to_string(),
            evalue: "bad input".to_string(),
            traceback: vec!["line 1".to_string(), "line 2".to_string()],
        };
        let json = serde_json::to_string(&output).expect("serialise");
        assert!(json.contains("\"output_type\":\"error\""));
        let back: CellOutput = serde_json::from_str(&json).expect("deserialise");
        if let CellOutput::Error { ename, .. } = back {
            assert_eq!(ename, "ValueError");
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn display_data_round_trip() {
        let output = CellOutput::DisplayData {
            data: OutputData {
                image_png: Some("abc123".to_string()),
                ..Default::default()
            },
            metadata: serde_json::Map::new(),
        };
        let json = serde_json::to_string(&output).expect("serialise");
        assert!(json.contains("\"output_type\":\"display_data\""));
        assert!(json.contains("image/png"));
    }

    #[test]
    fn nbformat_real_cell_source_array() {
        // Real nbformat uses arrays of strings for multi-line source.
        // JSON \n escapes are written as two chars inside a raw string literal;
        // serde_json parses them as actual newline characters.
        let cell_json = concat!(
            r#"{"cell_type":"code","source":["x = 1\n","y = 2\n","x + y"],"#,
            r#""outputs":[],"execution_count":null,"metadata":{}}"#,
        );
        let cell: NotebookCell = serde_json::from_str(cell_json).expect("deserialise");
        assert_eq!(cell.source.joined(), "x = 1\ny = 2\nx + y");
    }

    #[test]
    fn nbformat_real_notebook_parse() {
        // Minimal real-world-shaped nbformat 4 document.
        // Note: JSON string escapes (\n) are written as two-char sequences
        // inside the Rust raw string; serde_json interprets them as newlines.
        let json = concat!(
            r#"{"nbformat":4,"nbformat_minor":5,"metadata":{"#,
            r#""kernelspec":{"name":"python3","display_name":"Python 3"},"#,
            r#""language_info":{"name":"python","version":"3.10.0"}},"#,
            r#""cells":[{"cell_type":"markdown","id":"aabb1122","#,
            r#""source":"Title line","outputs":[],"metadata":{}}]}"#,
        );
        let nb: NotebookDocument = serde_json::from_str(json).expect("deserialise");
        assert_eq!(nb.nbformat, 4);
        assert_eq!(nb.cells[0].cell_type, "markdown");
        assert!(nb.metadata.kernelspec.is_some());
        assert!(nb.cells[0].source.joined().contains("Title"));
    }
}
