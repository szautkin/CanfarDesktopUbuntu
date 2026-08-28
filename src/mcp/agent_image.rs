//! Handing a picture to a client, once.
//!
//! Four tool families return images, and each turned bytes into a
//! [`ToolResult::Image`] its own way:
//!
//! | family | MIME | caption |
//! | --- | --- | --- |
//! | notebook | read from `imageMime` | none |
//! | cube | hard-coded `image/png` | none |
//! | fits | hard-coded `image/png`, for a payload no FITS op produced | none |
//! | research | the real type from the fetch | `Preview of …` |
//!
//! Four copies, three behaviours, and the two viewers about to gain
//! working-area capture would have made six. Worse, only one of them bounded
//! the size: an agent asking for a 4000×4000 render got about 21 MB of base64
//! into its context window, and nothing said so.
//!
//! So the decisions live here — how large a picture may be, how far to scale it
//! down, what type to call it — and the families keep only what is theirs: which
//! pixels, and which arguments name them.

use crate::mcp::tools::ToolResult;
use base64::Engine as _;
use serde_json::Value;

/// What a picture is allowed to cost the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageLimits {
    /// Longest edge, in pixels, before the image is scaled down.
    pub max_dimension: u32,
    /// Largest encoded size, in bytes, after scaling.
    pub max_bytes: usize,
}

impl ImageLimits {
    /// The limits from the user's settings.
    pub fn from_settings() -> Self {
        let s = crate::services::notebook_settings_service::NotebookSettingsService::new().load();
        Self {
            max_dimension: s.agent_image_max_dimension,
            max_bytes: s.agent_image_max_bytes_mb as usize * 1024 * 1024,
        }
    }

    /// Limits that never scale and never refuse.
    ///
    /// For a caller whose payload is already bounded by something else, and for
    /// tests that are about a different property than the budget.
    pub fn unbounded() -> Self {
        Self {
            max_dimension: u32::MAX,
            max_bytes: usize::MAX,
        }
    }
}

/// An image on its way to a client.
#[derive(Debug, Clone)]
pub struct AgentImage {
    bytes: Vec<u8>,
    mime: String,
    caption: Option<String>,
    view: Option<Value>,
}

impl AgentImage {
    /// From bytes already in an image format.
    ///
    /// The MIME is taken from the BYTES, not from what the caller believes:
    /// `image/png` was hard-coded in two of the four families, so a JPEG would
    /// have been announced as a PNG and handed to a client that was told
    /// otherwise. A caller's `declared` type is used only when the bytes are
    /// not a format we recognise.
    pub fn from_bytes(bytes: Vec<u8>, declared: &str) -> Result<Self, String> {
        if bytes.is_empty() {
            return Err("image is empty".to_string());
        }
        let mime = sniff_mime(&bytes)
            .map(str::to_string)
            .unwrap_or_else(|| declared.to_string());
        Ok(Self {
            bytes,
            mime,
            caption: None,
            view: None,
        })
    }

    /// From base64 a host op produced.
    pub fn from_base64(b64: &str, declared: &str) -> Result<Self, String> {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64.trim())
            .map_err(|e| format!("image is not valid base64: {e}"))?;
        Self::from_bytes(bytes, declared)
    }

    /// Say what this is a picture OF.
    ///
    /// An agent holding four images needs to tell them apart, and only
    /// `get_preview_image` ever said.
    pub fn with_caption(mut self, caption: impl Into<String>) -> Self {
        let caption = caption.into();
        if !caption.trim().is_empty() {
            self.caption = Some(caption);
        }
        self
    }

    /// Attach the view the picture was captured from.
    ///
    /// The transform between this raster and the viewer's own coordinates: an
    /// agent asked to ring a source has to say WHERE in a frame the app shares,
    /// and pixels alone cannot express it.
    pub fn with_view(mut self, view: Value) -> Self {
        self.view = Some(view);
        self
    }

    /// The encoded size, before any budget is applied.
    pub fn byte_len(&self) -> usize {
        self.bytes.len()
    }

    pub fn mime(&self) -> &str {
        &self.mime
    }

    /// Hand it over, or say why not.
    pub fn into_tool_result(self, limits: ImageLimits) -> ToolResult {
        if self.bytes.len() > limits.max_bytes {
            return ToolResult::Failed(over_budget_message(
                self.bytes.len(),
                limits.max_bytes,
                self.caption.as_deref(),
            ));
        }
        ToolResult::Image {
            data_base64: base64::engine::general_purpose::STANDARD.encode(&self.bytes),
            mime: self.mime,
            caption: self.caption,
            payload: self.view,
        }
    }
}

/// Turn a host reply carrying `imageBase64` into an image result.
///
/// The ONLY reader of that convention. `imageMime` is honoured when the host
/// states it; otherwise the bytes decide.
pub fn promote(value: Value, limits: ImageLimits) -> ToolResult {
    let Some(b64) = value.get("imageBase64").and_then(|v| v.as_str()) else {
        return ToolResult::Data(value);
    };
    let declared = value
        .get("imageMime")
        .and_then(|v| v.as_str())
        .unwrap_or("image/png");
    match AgentImage::from_base64(b64, declared) {
        Ok(mut image) => {
            if let Some(caption) = value.get("caption").and_then(|v| v.as_str()) {
                image = image.with_caption(caption);
            }
            // Everything the host said EXCEPT the pixels: the view, the
            // transform, the tab. The bytes are stripped so the coordinates do
            // not arrive twice, once as data and once as an image.
            let mut rest = value;
            if let Some(map) = rest.as_object_mut() {
                map.remove("imageBase64");
            }
            image = image.with_view(rest);
            image.into_tool_result(limits)
        }
        Err(e) => ToolResult::Failed(e),
    }
}

/// The format of `bytes`, from its opening bytes.
fn sniff_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) {
        return Some("image/png");
    }
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        return Some("image/jpeg");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if bytes.len() > 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    // SVG is text and may open with a declaration or a comment.
    let head = &bytes[..bytes.len().min(256)];
    if let Ok(text) = std::str::from_utf8(head) {
        if text.trim_start().starts_with("<?xml") || text.contains("<svg") {
            return Some("image/svg+xml");
        }
    }
    None
}

/// Why a picture was refused, and what to do about it.
fn over_budget_message(actual: usize, limit: usize, caption: Option<&str>) -> String {
    let mb = |n: usize| n as f64 / (1024.0 * 1024.0);
    let what = caption.unwrap_or("the image");
    format!(
        "{what} is {:.1} MB, over the {:.0} MB an agent image may be. Raise \
         \"Largest agent image\" in the notebook settings, or ask for a smaller region.",
        mb(actual),
        mb(limit)
    )
}

/// The size to capture at when the widget may not be on screen.
///
/// A viewer whose tab is not the visible page has no allocation, so its widget
/// reports 0x0. Refusing there would mean an agent could only look at whatever
/// the user happened to be looking at, which is the opposite of the point: the
/// viewer still HOLDS the camera, the channel, the colormap: only the pixel
/// dimensions are missing.
///
/// So a default stands in, and the caller is told which it got — a capture at a
/// made-up aspect ratio is fine as long as nobody believes it came from the
/// screen.
pub fn capture_size(view_w: i32, view_h: i32, limits: ImageLimits) -> (i32, i32, bool) {
    const FALLBACK: (i32, i32) = (1024, 768);
    let allocated = view_w > 0 && view_h > 0;
    let (w, h) = if allocated {
        (view_w, view_h)
    } else {
        FALLBACK
    };
    let (w, h) = fit_within(w, h, limits.max_dimension);
    (w, h, allocated)
}

/// The size to render at, so the longest edge fits `max_dimension`.
///
/// Never enlarges: a 400px view asked for at a 1024px limit stays 400px. A
/// vision model does not read more from an upscaled image, and the caller pays
/// for every pixel either way.
pub fn fit_within(width: i32, height: i32, max_dimension: u32) -> (i32, i32) {
    if width <= 0 || height <= 0 || max_dimension == 0 {
        return (width, height);
    }
    let longest = width.max(height) as u32;
    if longest <= max_dimension {
        return (width, height);
    }
    let scale = f64::from(max_dimension) / f64::from(longest);
    (
        ((width as f64) * scale).round().max(1.0) as i32,
        ((height as f64) * scale).round().max(1.0) as i32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A name for a result, since `ToolResult` is not `Debug`.
    fn kind_of(result: &ToolResult) -> &'static str {
        match result {
            ToolResult::Data(_) => "Data",
            ToolResult::Text(_) => "Text",
            ToolResult::Failed(_) => "Failed",
            ToolResult::Image { .. } => "Image",
            ToolResult::Proposed(_) => "Proposed",
        }
    }

    fn png_bytes() -> Vec<u8> {
        crate::helpers::png::encode_rgba(2, 2, &[255u8; 2 * 2 * 4]).expect("encode")
    }

    #[test]
    fn the_type_comes_from_the_bytes_not_the_claim() {
        // Two of the four families hard-coded `image/png`. A JPEG announced as
        // a PNG hands a client bytes that do not match what it was told.
        let jpeg = vec![0xff, 0xd8, 0xff, 0xe0, 0, 0];
        let image = AgentImage::from_bytes(jpeg, "image/png").expect("build");
        assert_eq!(image.mime(), "image/jpeg");

        let image = AgentImage::from_bytes(png_bytes(), "image/jpeg").expect("build");
        assert_eq!(image.mime(), "image/png");
    }

    #[test]
    fn an_unrecognised_format_keeps_what_the_caller_declared() {
        let odd = b"\x00\x01\x02\x03 not an image we know".to_vec();
        let image = AgentImage::from_bytes(odd, "image/tiff").expect("build");
        assert_eq!(image.mime(), "image/tiff");
    }

    #[test]
    fn an_empty_image_is_refused() {
        assert!(AgentImage::from_bytes(Vec::new(), "image/png").is_err());
        assert!(AgentImage::from_base64("", "image/png").is_err());
        assert!(AgentImage::from_base64("not base64!!", "image/png").is_err());
    }

    #[test]
    fn a_picture_over_budget_is_refused_with_the_remedy() {
        let image = AgentImage::from_bytes(vec![0xff, 0xd8, 0xff, 0, 0, 0], "image/jpeg")
            .expect("build")
            .with_caption("FITS tab 2");
        let limits = ImageLimits {
            max_dimension: 1024,
            max_bytes: 4,
        };
        match image.into_tool_result(limits) {
            ToolResult::Failed(message) => {
                assert!(message.contains("FITS tab 2"), "{message}");
                assert!(message.contains("Largest agent image"), "{message}");
            }
            other => panic!("expected a refusal, got {}", kind_of(&other)),
        }
    }

    #[test]
    fn a_picture_within_budget_arrives_as_an_image() {
        let image = AgentImage::from_bytes(png_bytes(), "image/png")
            .expect("build")
            .with_caption("cube tab 1");
        match image.into_tool_result(ImageLimits::unbounded()) {
            ToolResult::Image {
                mime,
                caption,
                data_base64,
                ..
            } => {
                assert_eq!(mime, "image/png");
                assert_eq!(caption.as_deref(), Some("cube tab 1"));
                assert!(!data_base64.is_empty());
            }
            other => panic!("expected an image, got {}", kind_of(&other)),
        }
    }

    /// A blank caption is no caption, rather than an empty line.
    #[test]
    fn a_blank_caption_is_not_attached() {
        let image = AgentImage::from_bytes(png_bytes(), "image/png")
            .expect("build")
            .with_caption("   ");
        match image.into_tool_result(ImageLimits::unbounded()) {
            ToolResult::Image { caption, .. } => assert_eq!(caption, None),
            other => panic!("expected an image, got {}", kind_of(&other)),
        }
    }

    /// A reply with no image is data, unchanged.
    #[test]
    fn a_reply_without_an_image_passes_through() {
        let value = serde_json::json!({"zoom": 100, "centerX": 512});
        match promote(value.clone(), ImageLimits::unbounded()) {
            ToolResult::Data(v) => assert_eq!(v, value),
            other => panic!("expected data, got {}", kind_of(&other)),
        }
    }

    #[test]
    fn promote_reads_the_hosts_mime_and_caption() {
        let value = serde_json::json!({
            "imageBase64": base64::engine::general_purpose::STANDARD.encode(png_bytes()),
            "imageMime": "image/png",
            "caption": "FITS tab 0",
        });
        match promote(value, ImageLimits::unbounded()) {
            ToolResult::Image { mime, caption, .. } => {
                assert_eq!(mime, "image/png");
                assert_eq!(caption.as_deref(), Some("FITS tab 0"));
            }
            other => panic!("expected an image, got {}", kind_of(&other)),
        }
    }

    /// Scaling keeps the shape and never enlarges.
    #[test]
    fn a_large_view_is_scaled_down_keeping_its_shape() {
        assert_eq!(fit_within(4000, 2000, 1024), (1024, 512));
        assert_eq!(fit_within(2000, 4000, 1024), (512, 1024));
        // Square.
        assert_eq!(fit_within(2048, 2048, 1024), (1024, 1024));
    }

    #[test]
    fn a_small_view_is_left_alone() {
        // Upscaling costs the caller pixels and tells a model nothing more.
        assert_eq!(fit_within(400, 300, 1024), (400, 300));
        assert_eq!(fit_within(1024, 768, 1024), (1024, 768));
    }

    #[test]
    fn scaling_never_rounds_a_dimension_away() {
        // An extreme aspect ratio must not produce a zero-height image.
        let (w, h) = fit_within(40_000, 3, 1024);
        assert_eq!(w, 1024);
        assert!(h >= 1, "height rounded to {h}");
    }

    /// A viewer on a hidden tab is still capturable.
    #[test]
    fn a_view_with_no_allocation_falls_back_and_says_so() {
        let limits = ImageLimits {
            max_dimension: 1024,
            max_bytes: usize::MAX,
        };
        let (w, h, allocated) = capture_size(0, 0, limits);
        assert!(w > 0 && h > 0, "no size to render at: {w}x{h}");
        assert!(
            !allocated,
            "a fallback size must not be reported as the screen's"
        );

        // A real allocation is used as-is, and reported as real.
        let (w, h, allocated) = capture_size(800, 600, limits);
        assert_eq!((w, h), (800, 600));
        assert!(allocated);

        // ...and is still scaled when it is too large.
        let (w, h, allocated) = capture_size(4000, 3000, limits);
        assert_eq!((w, h), (1024, 768));
        assert!(
            allocated,
            "scaling is not the same thing as not being on screen"
        );
    }

    #[test]
    fn nonsense_sizes_are_passed_through_rather_than_divided_by() {
        assert_eq!(fit_within(0, 0, 1024), (0, 0));
        assert_eq!(fit_within(-4, 10, 1024), (-4, 10));
        assert_eq!(fit_within(100, 100, 0), (100, 100));
    }
}
