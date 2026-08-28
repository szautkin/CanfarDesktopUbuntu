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
                id: Some(cell_id),
                ..NotebookCell::new("code")
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
    // Omitted when unset, never written as `null`: the nbformat schema types
    // these as objects, so `"kernelspec": null` fails validation — and Jupyter
    // reads a notebook with no kernelspec as "pick a kernel", which is the
    // honest state, while a null is a malformed one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kernelspec: Option<KernelSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
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
    // Absent, not null: the schema types it as a string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}

/// Language information embedded in the notebook metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LanguageInfo {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

// ---------------------------------------------------------------------------
// Cells
// ---------------------------------------------------------------------------

/// A single notebook cell.  `cell_type` is `"code"`, `"markdown"`, or `"raw"`.
#[derive(Debug, Clone, Deserialize)]
pub struct NotebookCell {
    pub cell_type: String,
    pub source: CellSource,
    // `outputs` and `execution_count` belong to a CODE cell. The nbformat 4.5
    // schema declares `additionalProperties: false` on markdown and raw cells,
    // so writing them there produces a file `nbformat.validate` rejects — and
    // ours wrote `"outputs": [], "execution_count": null` on every markdown
    // cell. `is_not_code` reads the sibling field, which is why these are
    // serialized through a custom path rather than a `skip_serializing_if`
    // that cannot see it.
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

impl NotebookCell {
    /// A new empty cell of `cell_type`, carrying a fresh id.
    ///
    /// The id is not optional in practice: nbformat 4.5 requires one on every
    /// cell, and a document written without them is a document other tools may
    /// refuse. Building a cell by hand meant remembering that; this does not.
    pub fn new(cell_type: &str) -> Self {
        NotebookCell {
            cell_type: cell_type.to_string(),
            source: CellSource::Single(String::new()),
            outputs: Vec::new(),
            execution_count: None,
            id: Some(NotebookDocument::generate_cell_id()),
            metadata: serde_json::Map::new(),
        }
    }

    /// The same, with `source` already set.
    pub fn with_source(cell_type: &str, source: impl Into<String>) -> Self {
        let mut cell = Self::new(cell_type);
        cell.source = CellSource::Single(source.into());
        cell
    }
}

impl Serialize for NotebookCell {
    /// Write the cell the way nbformat 4.5 defines it.
    ///
    /// A code cell carries `outputs` and `execution_count` — both REQUIRED,
    /// even when empty or null. A markdown or raw cell carries neither, and the
    /// schema says `additionalProperties: false`, so writing them there is a
    /// file `nbformat.validate` rejects and other tools may refuse. Ours wrote
    /// `"outputs": [], "execution_count": null` on every markdown cell.
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;

        let is_code = self.cell_type == "code";
        let mut len = 3; // cell_type, source, metadata
        if self.id.is_some() {
            len += 1;
        }
        if is_code {
            len += 2;
        }

        let mut map = serializer.serialize_map(Some(len))?;
        map.serialize_entry("cell_type", &self.cell_type)?;
        if let Some(id) = &self.id {
            map.serialize_entry("id", id)?;
        }
        map.serialize_entry("metadata", &self.metadata)?;
        map.serialize_entry("source", &self.source)?;
        if is_code {
            map.serialize_entry("execution_count", &self.execution_count)?;
            map.serialize_entry("outputs", &self.outputs)?;
        }
        map.end()
    }
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

    /// Every MIME type this output carries, sorted.
    ///
    /// `has_image` and `has_html` answer two yes/no questions, which is all the
    /// renderer needed when it could only draw those two. A caller that wants
    /// to know whether a cell produced markdown, SVG, LaTeX or JSON — an agent
    /// checking its own work, or a test asserting a non-image rich repr — had
    /// no way to ask. This lists what is actually there.
    ///
    /// Sorted so a caller comparing two outputs, or a test asserting on the
    /// list, does not depend on `HashMap` iteration order.
    pub fn mime_types(&self) -> Vec<String> {
        let mut types: Vec<String> = self
            .extra
            .keys()
            .cloned()
            .chain(self.text_plain.iter().map(|_| "text/plain".to_string()))
            .chain(self.text_html.iter().map(|_| "text/html".to_string()))
            .chain(self.image_png.iter().map(|_| "image/png".to_string()))
            .chain(self.image_jpeg.iter().map(|_| "image/jpeg".to_string()))
            .collect();
        types.sort_unstable();
        types.dedup();
        types
    }

    /// The image this output carries, as `(base64 bytes, MIME type)`.
    ///
    /// Separate from [`richest`](Self::richest) because the two answer
    /// different questions. `richest` picks what a HUMAN should look at, and
    /// deliberately keeps image bytes out of anything that crosses the tool
    /// boundary — a base64 PNG inlined into every `get_cell_output` would cost
    /// an agent its context window on every call, mostly for outputs it never
    /// wanted to look at.
    ///
    /// This is the other half: asked for explicitly, one output at a time, so a
    /// client that CAN see pictures can fetch the pixels and one that cannot
    /// never pays for them.
    ///
    /// PNG before JPEG, matching `richest`, so the picture an agent retrieves
    /// is the picture the notebook drew.
    pub fn image_bytes(&self) -> Option<(&str, &'static str)> {
        if let Some(b64) = &self.image_png {
            return Some((b64, "image/png"));
        }
        self.image_jpeg.as_deref().map(|b64| (b64, "image/jpeg"))
    }

    /// The richest representation this output carries, or `None` if it carries
    /// nothing renderable.
    ///
    /// A display bundle holds the SAME value several ways and the front end
    /// picks one — that is the whole point of the MIME bundle. Ours used to ask
    /// two questions in a fixed order (is there a PNG? is there text?) with the
    /// other two representations parsed and then ignored, so an
    /// `astropy.table.Table` arrived as its `repr()` even though the HTML was
    /// sitting right there in the same bundle.
    ///
    /// Kept out of the widget so the choice can be tested without a display,
    /// and so there is exactly one place that knows the order.
    pub fn richest(&self) -> Option<Representation<'_>> {
        // Images first, which is where this departs from nbconvert's order —
        // it puts `text/html` ahead of `image/png`. nbconvert emits into a
        // browser; we render HTML with a deliberately small renderer that has
        // no JavaScript and no CSS. When a library offers both (plotly, and
        // anything wrapping a figure) the HTML is the interactive version we
        // cannot run, and the image is the one we can actually show.
        if let Some(b64) = &self.image_png {
            return Some(Representation::Image(b64));
        }
        if let Some(b64) = &self.image_jpeg {
            return Some(Representation::Image(b64));
        }
        // SVG is a picture too, and `Texture::from_bytes` reads it directly.
        if let Some(svg) = self.extra_text("image/svg+xml") {
            return Some(Representation::Svg(svg));
        }
        if let Some(html) = &self.text_html {
            let html = html.joined();
            if !html.trim().is_empty() {
                return Some(Representation::Html(html));
            }
        }
        if let Some(md) = self.extra_text("text/markdown") {
            return Some(Representation::Markdown(md));
        }
        if let Some(latex) = self.extra_text("text/latex") {
            return Some(Representation::Latex(latex));
        }
        if let Some(json) = self.extra.get("application/json") {
            // Pretty-printed here rather than in the widget: the widget shows
            // text, and how a JSON document reads is a property of the
            // document. `to_string` on a failure keeps the content either way.
            let text = serde_json::to_string_pretty(json).unwrap_or_else(|_| json.to_string());
            if !text.trim().is_empty() {
                return Some(Representation::Json(text));
            }
        }
        let text = self.plain_text()?;
        // An empty `text/plain` is not worth an empty label. It is what a bare
        // `display()` of something with no representations leaves behind.
        (!text.is_empty()).then_some(Representation::Text(text))
    }
}

impl OutputData {
    /// A text representation from the un-modelled part of the bundle.
    ///
    /// nbformat allows any text value to arrive either as a string or as a list
    /// of lines — the modelled fields go through [`CellSource`] for exactly
    /// that reason, and the flattened `extra` map has to do it by hand.
    /// Whitespace-only content is treated as absent, so it never displaces a
    /// representation that would actually show something.
    fn extra_text(&self, mime: &str) -> Option<String> {
        let value = self.extra.get(mime)?;
        let text = match value {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Array(lines) => lines
                .iter()
                .filter_map(|l| l.as_str())
                .collect::<Vec<_>>()
                .concat(),
            _ => return None,
        };
        (!text.trim().is_empty()).then_some(text)
    }
}

/// The one representation of an output that gets rendered.
///
/// `Image` covers PNG and JPEG together: the decoder sniffs the format from the
/// bytes, so telling them apart would only give the caller a distinction it has
/// no use for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Representation<'a> {
    /// Base64-encoded image bytes, in whatever format the bundle carried.
    Image(&'a str),
    /// SVG source. Text, not base64 — that is how nbformat carries it.
    Svg(String),
    /// HTML source, to be rendered by
    /// [`simple_html`](crate::helpers::simple_html).
    Html(String),
    /// Markdown source, for the same renderer markdown CELLS use.
    Markdown(String),
    /// LaTeX source. There is no renderer for it yet, so the widget shows the
    /// source — which is at least the thing the author wrote, and readable.
    Latex(String),
    /// A JSON document, already pretty-printed.
    Json(String),
    /// Plain text: the representation every bundle is required to have.
    Text(String),
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

    // --- Which representation gets rendered ---

    fn bundle(json: &str) -> OutputData {
        serde_json::from_str(json).expect("bundle parses")
    }

    /// Every captured MIME is reported, not just the two the renderer knew.
    #[test]
    fn mime_types_lists_everything_in_the_bundle() {
        // `r##"…"##`: the markdown value contains `"#`, which would close a
        // single-hash raw string in the middle of the JSON.
        let data = bundle(
            r##"{"text/html": "<b>x</b>", "text/plain": "x",
                 "image/png": "iVBOR", "text/markdown": "# h",
                 "image/svg+xml": "<svg/>", "application/json": {"a": 1}}"##,
        );
        assert_eq!(
            data.mime_types(),
            [
                "application/json",
                "image/png",
                "image/svg+xml",
                "text/html",
                "text/markdown",
                "text/plain",
            ]
        );
        // The yes/no questions still agree with the list.
        assert!(data.has_image());
        assert!(data.mime_types().contains(&"text/html".to_string()));
    }

    #[test]
    fn an_empty_bundle_lists_nothing() {
        assert!(bundle("{}").mime_types().is_empty());
    }

    /// An agent that can see pictures must be able to fetch the pixels.
    #[test]
    fn image_bytes_returns_the_data_and_its_type() {
        let data = bundle(r#"{"image/png": "iVBORw0=", "text/plain": "<Figure>"}"#);
        assert_eq!(data.image_bytes(), Some(("iVBORw0=", "image/png")));

        // A JPEG-only bundle — a PIL image with `_repr_jpeg_` and no PNG.
        let data = bundle(r#"{"image/jpeg": "/9j/AAA=", "text/plain": "<Image>"}"#);
        assert_eq!(data.image_bytes(), Some(("/9j/AAA=", "image/jpeg")));

        // When both are offered, PNG — the same one `richest` draws, so what an
        // agent fetches is what the notebook showed.
        let data = bundle(r#"{"image/png": "PNG", "image/jpeg": "JPEG"}"#);
        assert_eq!(data.image_bytes(), Some(("PNG", "image/png")));
    }

    #[test]
    fn an_output_with_no_image_has_no_bytes() {
        assert_eq!(bundle(r#"{"text/plain": "42"}"#).image_bytes(), None);
        // SVG is a picture, but it is text and needs no base64 round trip; it
        // travels in `richTypes` and is not what this accessor is for.
        assert_eq!(bundle(r#"{"image/svg+xml": "<svg/>"}"#).image_bytes(), None);
    }

    /// An astropy Table: HTML and text in one bundle. The HTML is the point.
    #[test]
    fn html_beats_plain_text() {
        let data = bundle(
            r#"{"text/html": "<table><tr><td>1</td></tr></table>",
                "text/plain": "<Table length=1>"}"#,
        );
        assert_eq!(
            data.richest(),
            Some(Representation::Html(
                "<table><tr><td>1</td></tr></table>".to_string()
            ))
        );
    }

    /// A PIL image via `_repr_jpeg_` with no PNG alongside it.
    ///
    /// `image/jpeg` was modelled from the start and never rendered — the
    /// renderer asked about PNG, then gave up and printed the text. This is
    /// that gap.
    #[test]
    fn a_jpeg_only_bundle_still_renders_as_an_image() {
        let data = bundle(r#"{"image/jpeg": "/9j/AAA=", "text/plain": "<PIL.Image>"}"#);
        assert_eq!(data.richest(), Some(Representation::Image("/9j/AAA=")));
    }

    /// A matplotlib figure: PNG and text.
    #[test]
    fn an_image_beats_plain_text() {
        let data = bundle(r#"{"image/png": "iVBORw0=", "text/plain": "<Figure>"}"#);
        assert_eq!(data.richest(), Some(Representation::Image("iVBORw0=")));
    }

    /// When a bundle offers both, the one this app can actually draw wins.
    ///
    /// Deliberately the opposite of nbconvert, which puts `text/html` first
    /// because it renders into a browser. Ours would show plotly's interactive
    /// HTML as a stack of empty divs and drop the static image that works.
    #[test]
    fn an_image_beats_html_here() {
        let data = bundle(r#"{"image/png": "iVBORw0=", "text/html": "<div id=plot></div>"}"#);
        assert_eq!(data.richest(), Some(Representation::Image("iVBORw0=")));
    }

    /// SVG is a picture, and ranks with the other pictures.
    #[test]
    fn svg_is_treated_as_an_image_not_as_text() {
        let data = bundle(r#"{"image/svg+xml": "<svg/>", "text/plain": "<S object>"}"#);
        assert_eq!(
            data.richest(),
            Some(Representation::Svg("<svg/>".to_string()))
        );
    }

    /// The MIME types that used to fall through to an object address.
    ///
    /// Each of these was captured by the harness, ignored by the renderer, and
    /// shown as whatever `text/plain` happened to be — for an object with no
    /// `__repr__` that is `<__main__.M object at 0x7f…>`, a memory address.
    #[test]
    fn markdown_latex_and_json_all_have_a_representation() {
        let data = bundle(r##"{"text/markdown": "# h", "text/plain": "<M object>"}"##);
        assert_eq!(
            data.richest(),
            Some(Representation::Markdown("# h".to_string()))
        );

        // `\\alpha` in the JSON source is the two characters a LaTeX author
        // types: a backslash and a word.
        let data = bundle(r#"{"text/latex": "$\\alpha$", "text/plain": "<L object>"}"#);
        assert_eq!(
            data.richest(),
            Some(Representation::Latex(r"$\alpha$".to_string()))
        );

        // JSON arrives as a document and is pretty-printed, not re-serialised
        // to one line.
        let data = bundle(r#"{"application/json": {"a": 1}, "text/plain": "<J object>"}"#);
        let Some(Representation::Json(text)) = data.richest() else {
            panic!("expected json, got {:?}", data.richest());
        };
        assert!(text.contains('\n'), "not pretty-printed: {text}");
        assert!(text.contains(r#""a": 1"#), "{text}");
    }

    /// nbformat lets any text value arrive as a list of lines.
    #[test]
    fn a_line_list_in_the_unmodelled_part_is_joined() {
        // Raw string: `\n` here is the two characters JSON needs to see in
        // order to decode a newline.
        let data = bundle(r##"{"text/markdown": ["# head\n", "body"]}"##);
        assert_eq!(
            data.richest(),
            Some(Representation::Markdown("# head\nbody".to_string()))
        );
    }

    /// A rich key holding only whitespace must not displace real content.
    #[test]
    fn an_empty_rich_value_does_not_hide_the_text() {
        let data = bundle(r#"{"text/markdown": "   ", "text/plain": "the real thing"}"#);
        assert_eq!(
            data.richest(),
            Some(Representation::Text("the real thing".to_string()))
        );
    }

    /// Plain text is what is left, and it is always allowed to be the answer.
    #[test]
    fn plain_text_is_used_when_it_is_all_there_is() {
        let data = bundle(r#"{"text/plain": "42"}"#);
        assert_eq!(data.richest(), Some(Representation::Text("42".to_string())));
    }

    /// Nothing renderable renders nothing, rather than an empty label.
    #[test]
    fn an_empty_bundle_has_no_representation() {
        assert_eq!(bundle("{}").richest(), None);
        assert_eq!(bundle(r#"{"text/plain": ""}"#).richest(), None);
        // An HTML key holding only whitespace is not a reason to skip the text
        // that sits beside it.
        let data = bundle(r#"{"text/html": "  \n ", "text/plain": "fallback"}"#);
        assert_eq!(
            data.richest(),
            Some(Representation::Text("fallback".to_string()))
        );
    }

    /// A bundle whose text arrived as a line list, as nbformat allows.
    #[test]
    fn a_line_list_representation_is_joined() {
        let data = bundle(r#"{"text/plain": ["line one\n", "line two"]}"#);
        assert_eq!(
            data.richest(),
            Some(Representation::Text("line one\nline two".to_string()))
        );
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
