//! Marks a person or an agent has drawn on a viewer.
//!
//! One model for both viewers. The FITS canvas is flat and the cube's volume is
//! not, and the only thing that differs between them is how a point becomes a
//! pixel — so the anchor carries the coordinates and the viewer supplies the
//! projection. Everything else, from the callout geometry to the label, is the
//! same code in both places.
//!
//! **Anchors are never screen pixels.** A mark pinned to the window slides off
//! its subject the moment anyone pans, and nobody notices until they zoom. They
//! are stored in the coordinates the data itself uses: image pixels or sky for
//! FITS, voxels for a cube.

use serde::{Deserialize, Serialize};

/// Where a mark is pinned, in the viewer's own coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "space", rename_all = "camelCase")]
pub enum Anchor {
    /// FITS image pixels. Survives pan, zoom and rotation.
    ImagePixel { x: f64, y: f64 },
    /// Sky position in degrees. Survives reopening the file, and points at the
    /// same place in a DIFFERENT image of the same field — which is why it is
    /// preferred whenever the FITS has WCS.
    Sky { ra_deg: f64, dec_deg: f64 },
    /// Cube voxel space `(x, y, channel)`.
    Data { x: f64, y: f64, z: f64 },
}

impl Anchor {
    /// Every coordinate, for validation.
    fn coords(&self) -> [f64; 3] {
        match *self {
            Anchor::ImagePixel { x, y } => [x, y, 0.0],
            Anchor::Sky { ra_deg, dec_deg } => [ra_deg, dec_deg, 0.0],
            Anchor::Data { x, y, z } => [x, y, z],
        }
    }

    /// Whether this anchor can be drawn at all.
    ///
    /// A NaN reaches cairo, draws nothing, and reports no error — the mark is
    /// simply absent, which is indistinguishable from one that was never
    /// created. Sky coordinates are range-checked too: a Dec of 120° is not a
    /// place, and silently drawing it somewhere is worse than refusing it.
    pub fn is_valid(&self) -> bool {
        if self.coords().iter().any(|c| !c.is_finite()) {
            return false;
        }
        match *self {
            Anchor::Sky { ra_deg, dec_deg } => {
                (0.0..360.0).contains(&ra_deg) && (-90.0..=90.0).contains(&dec_deg)
            }
            _ => true,
        }
    }

    /// The space this anchor belongs to, for a message or a payload.
    pub fn space(&self) -> &'static str {
        match self {
            Anchor::ImagePixel { .. } => "imagePixel",
            Anchor::Sky { .. } => "sky",
            Anchor::Data { .. } => "data",
        }
    }
}

/// What a mark looks like.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AnnotationKind {
    /// A box around the subject.
    Rect,
    /// A circle around it — a sphere, in a cube, so it projects to an ellipse
    /// when the camera is off-axis.
    Circle,
    /// A shape with a leader line to a label set clear of the subject.
    Callout,
    /// A label alone, at the anchor.
    Text,
}

impl AnnotationKind {
    /// Whether this kind draws a shape that needs an extent.
    pub fn needs_extent(self) -> bool {
        matches!(self, AnnotationKind::Rect | AnnotationKind::Circle)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            AnnotationKind::Rect => "rect",
            AnnotationKind::Circle => "circle",
            AnnotationKind::Callout => "callout",
            AnnotationKind::Text => "text",
        }
    }

    /// Parse a kind from a tool argument.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "rect" | "rectangle" | "box" | "square" => Some(AnnotationKind::Rect),
            "circle" | "ellipse" => Some(AnnotationKind::Circle),
            "callout" | "label" | "leader" => Some(AnnotationKind::Callout),
            "text" | "note" => Some(AnnotationKind::Text),
            _ => None,
        }
    }
}

/// How big a shape is, in the anchor's own units.
///
/// Data units, not screen pixels: a circle drawn around a source should stay
/// around that source as the view zooms, the way a circle drawn on a photograph
/// does. A screen-sized shape looks right at one zoom level and at no other.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Extent {
    pub half_width: f64,
    pub half_height: f64,
}

impl Extent {
    pub fn square(half: f64) -> Self {
        Self {
            half_width: half,
            half_height: half,
        }
    }

    /// A shape with no area cannot be seen or clicked.
    pub fn is_valid(&self) -> bool {
        self.half_width.is_finite()
            && self.half_height.is_finite()
            && self.half_width > 0.0
            && self.half_height > 0.0
    }
}

/// Who drew a mark.
///
/// An agent drawing on someone's screen without saying so is how a feature like
/// this loses trust. The panel shows it, the payload reports it, and the style
/// gives an agent's marks their own accent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum Author {
    #[default]
    User,
    Agent,
}

impl Author {
    pub fn as_str(self) -> &'static str {
        match self {
            Author::User => "user",
            Author::Agent => "agent",
        }
    }
}

/// A mark on a viewer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Annotation {
    /// Stable id, unique within one target.
    pub id: String,
    pub kind: AnnotationKind,
    pub anchor: Anchor,
    /// Size, for the kinds that have one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extent: Option<Extent>,
    /// The label. May be empty for a bare shape.
    #[serde(default)]
    pub text: String,
    /// Where a callout's label sits, in SCREEN pixels from the anchor.
    ///
    /// Screen and not data units, deliberately, and the one place that is
    /// right: a label is furniture, not part of the image. In a cube it would
    /// otherwise shear and shrink as the camera moved, and the text would stop
    /// being readable — which is the one thing a label has to be.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label_offset: Option<(f64, f64)>,
    #[serde(default)]
    pub author: Author,
    /// How this mark is drawn, when it has been said explicitly.
    ///
    /// `None` means "however a mark by this author is drawn" — which is what
    /// every mark on disk before styling existed means, and what it has always
    /// meant. That is why this is an `Option` rather than a struct with
    /// defaults: `#[serde(default)]` on a plain `MarkStyle` would give every
    /// stored agent mark the USER colour on load, silently restyling work
    /// people had already done, with no error to notice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<MarkStyle>,
    /// RFC-3339, for the panel's ordering.
    #[serde(default)]
    pub created_at: String,
}

/// How a mark is drawn: its ink, its label, and the weight of its outline.
///
/// Per-mark rather than global because a mark persists with its file, travels
/// over MCP and ends up in an exported figure that has to look the same when
/// it is opened again.
///
/// Sizes are in DEVICE PIXELS and are not scaled by zoom, for the reason the
/// hairline stroke always had: a stroke that thickens as you zoom out turns the
/// view into a blot. An export still scales them, because cairo's line width
/// and font size live in user space and the capture scales the context.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MarkStyle {
    /// Ink, as linear RGB in 0..=1.
    pub colour: (f64, f64, f64),
    /// Label size in device pixels.
    pub font_size: f64,
    pub bold: bool,
    /// Outline width in device pixels.
    pub stroke: f64,
}

/// The drawing ink: cold white-cyan.
pub const USER_INK: (f64, f64, f64) = (0.62, 0.85, 1.0);
/// An agent's marks, distinguishable without being louder.
pub const AGENT_INK: (f64, f64, f64) = (0.55, 1.0, 0.80);
pub const DEFAULT_FONT_SIZE: f64 = 11.0;
pub const DEFAULT_STROKE: f64 = 1.0;

impl MarkStyle {
    /// What a mark by `author` looks like when nothing has been said about it.
    pub fn for_author(author: Author) -> Self {
        Self {
            colour: match author {
                Author::Agent => AGENT_INK,
                _ => USER_INK,
            },
            font_size: DEFAULT_FONT_SIZE,
            bold: false,
            stroke: DEFAULT_STROKE,
        }
    }

    /// Clamped to what can actually be drawn and read.
    ///
    /// A zero stroke draws nothing and a zero font size is an invisible label —
    /// both look like the mark having been lost. The ceilings stop one mark
    /// from covering the frame.
    pub fn sane(mut self) -> Self {
        self.font_size = self.font_size.clamp(6.0, 72.0);
        self.stroke = self.stroke.clamp(0.5, 20.0);
        let c = |v: f64| {
            if v.is_finite() {
                v.clamp(0.0, 1.0)
            } else {
                0.0
            }
        };
        self.colour = (c(self.colour.0), c(self.colour.1), c(self.colour.2));
        self
    }
}

impl Default for MarkStyle {
    fn default() -> Self {
        Self::for_author(Author::User)
    }
}

impl Annotation {
    /// How this mark should actually be drawn.
    ///
    /// Its own style if it has one, else the look of its author. One answer, so
    /// the renderer, the label hit box and the export cannot each decide
    /// differently — they did not, but only because there was one constant.
    pub fn effective_style(&self) -> MarkStyle {
        self.style
            .unwrap_or_else(|| MarkStyle::for_author(self.author))
            .sane()
    }
}

impl Annotation {
    /// A new mark with a generated id and timestamp.
    pub fn new(
        kind: AnnotationKind,
        anchor: Anchor,
        text: impl Into<String>,
        author: Author,
    ) -> Self {
        Self {
            id: new_id(),
            kind,
            anchor,
            extent: kind
                .needs_extent()
                .then(|| Extent::square(default_half_extent(&anchor))),
            text: text.into(),
            label_offset: None,
            author,
            // Unstyled: a new mark looks like every other mark by its author
            // until something says otherwise.
            style: None,
            created_at: chrono::Utc::now().to_rfc3339(),
        }
    }

    pub fn with_extent(mut self, extent: Extent) -> Self {
        self.extent = Some(extent);
        self
    }

    pub fn with_label_offset(mut self, dx: f64, dy: f64) -> Self {
        self.label_offset = Some((dx, dy));
        self
    }

    /// Why this mark cannot be drawn, if it cannot.
    ///
    /// Returns a message aimed at whoever sent it — a tool argument that is
    /// wrong should say so, rather than being stored and silently never
    /// appearing.
    pub fn validate(&self) -> Result<(), String> {
        if self.id.trim().is_empty() {
            return Err("an annotation needs an id".to_string());
        }
        if !self.anchor.is_valid() {
            return Err(format!(
                "the {} position is not a place that can be drawn (not finite, or off the sky)",
                self.anchor.space()
            ));
        }
        if let Some(extent) = self.extent {
            if !extent.is_valid() {
                return Err("a shape needs a width and height greater than zero".to_string());
            }
        } else if self.kind.needs_extent() {
            return Err(format!(
                "a {} needs a size — give it a radius or a width and height",
                self.kind.as_str()
            ));
        }
        if let Some((dx, dy)) = self.label_offset {
            if !dx.is_finite() || !dy.is_finite() {
                return Err("the label offset is not a finite distance".to_string());
            }
        }
        // A callout with nothing to say is a leader line pointing at a blank
        // rule — it looks like a rendering fault.
        if self.kind == AnnotationKind::Callout && self.text.trim().is_empty() {
            return Err("a callout needs text — its whole purpose is the label".to_string());
        }
        if self.kind == AnnotationKind::Text && self.text.trim().is_empty() {
            return Err("a text annotation needs text".to_string());
        }
        Ok(())
    }
}

/// A default size for a new shape, in the anchor's units.
///
/// Pixels and voxels are counted in ones; degrees are not, and a 12-degree
/// circle would swallow the sky. The unit decides the number.
fn default_half_extent(anchor: &Anchor) -> f64 {
    match anchor {
        Anchor::Sky { .. } => 0.005,
        _ => 12.0,
    }
}

/// A short unique id.
fn new_id() -> String {
    format!("ann-{}", uuid::Uuid::new_v4().simple())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_shape_gets_an_id_a_time_and_a_size() {
        let a = Annotation::new(
            AnnotationKind::Circle,
            Anchor::ImagePixel { x: 100.0, y: 200.0 },
            "core",
            Author::User,
        );
        assert!(a.id.starts_with("ann-"));
        assert!(!a.created_at.is_empty());
        assert!(a.extent.is_some(), "a circle with no size cannot be drawn");
        assert!(a.validate().is_ok());
    }

    #[test]
    fn two_annotations_do_not_share_an_id() {
        let a = Annotation::new(
            AnnotationKind::Rect,
            Anchor::ImagePixel { x: 1.0, y: 1.0 },
            "",
            Author::User,
        );
        let b = Annotation::new(
            AnnotationKind::Rect,
            Anchor::ImagePixel { x: 1.0, y: 1.0 },
            "",
            Author::User,
        );
        assert_ne!(a.id, b.id);
    }

    /// A NaN never reaches cairo.
    ///
    /// It draws nothing and reports nothing, so the mark is simply absent — and
    /// absent is indistinguishable from never created, which is the worst way
    /// for this to fail.
    #[test]
    fn a_position_that_is_not_a_number_is_refused() {
        for anchor in [
            Anchor::ImagePixel {
                x: f64::NAN,
                y: 1.0,
            },
            Anchor::ImagePixel {
                x: 1.0,
                y: f64::INFINITY,
            },
            Anchor::Data {
                x: 1.0,
                y: 1.0,
                z: f64::NAN,
            },
        ] {
            assert!(!anchor.is_valid(), "{anchor:?} should be refused");
            let a = Annotation::new(AnnotationKind::Text, anchor, "x", Author::Agent);
            let err = a.validate().expect_err("should not validate");
            assert!(err.contains("not a place"), "{err}");
        }
    }

    /// A sky position outside the sky is refused, not drawn somewhere.
    #[test]
    fn an_impossible_sky_position_is_refused() {
        let bad = Anchor::Sky {
            ra_deg: 10.0,
            dec_deg: 120.0,
        };
        assert!(!bad.is_valid(), "a Dec of 120 degrees is not a place");
        let wrapped = Anchor::Sky {
            ra_deg: 400.0,
            dec_deg: 10.0,
        };
        assert!(!wrapped.is_valid(), "an RA of 400 degrees is not a place");
        assert!(Anchor::Sky {
            ra_deg: 202.4696,
            dec_deg: 47.1953
        }
        .is_valid());
    }

    #[test]
    fn a_shape_with_no_area_is_refused() {
        let a = Annotation::new(
            AnnotationKind::Rect,
            Anchor::ImagePixel { x: 5.0, y: 5.0 },
            "",
            Author::User,
        )
        .with_extent(Extent {
            half_width: 0.0,
            half_height: 4.0,
        });
        let err = a.validate().expect_err("zero width");
        assert!(err.contains("greater than zero"), "{err}");
    }

    /// A callout with no text is a leader pointing at a blank rule.
    #[test]
    fn a_callout_must_have_something_to_say() {
        let a = Annotation::new(
            AnnotationKind::Callout,
            Anchor::ImagePixel { x: 5.0, y: 5.0 },
            "   ",
            Author::Agent,
        );
        let err = a.validate().expect_err("empty callout");
        assert!(err.contains("needs text"), "{err}");
    }

    /// A bare shape needs no text.
    #[test]
    fn a_shape_without_a_label_is_fine() {
        let a = Annotation::new(
            AnnotationKind::Circle,
            Anchor::ImagePixel { x: 5.0, y: 5.0 },
            "",
            Author::User,
        );
        assert!(a.validate().is_ok());
    }

    /// The default size suits the unit it is measured in.
    #[test]
    fn a_sky_shape_is_not_twelve_degrees_across() {
        let sky = Annotation::new(
            AnnotationKind::Circle,
            Anchor::Sky {
                ra_deg: 202.0,
                dec_deg: 47.0,
            },
            "",
            Author::User,
        );
        let pixels = Annotation::new(
            AnnotationKind::Circle,
            Anchor::ImagePixel { x: 10.0, y: 10.0 },
            "",
            Author::User,
        );
        assert!(
            sky.extent.unwrap().half_width < 0.1,
            "a default sky circle would swallow the field"
        );
        assert!(pixels.extent.unwrap().half_width > 1.0);
    }

    #[test]
    fn kinds_parse_from_what_a_caller_would_write() {
        assert_eq!(AnnotationKind::parse("Rect"), Some(AnnotationKind::Rect));
        assert_eq!(AnnotationKind::parse("square"), Some(AnnotationKind::Rect));
        assert_eq!(
            AnnotationKind::parse(" circle "),
            Some(AnnotationKind::Circle)
        );
        assert_eq!(
            AnnotationKind::parse("label"),
            Some(AnnotationKind::Callout)
        );
        assert_eq!(AnnotationKind::parse("banana"), None);
    }

    /// The stored form survives a round trip, including the anchor's space.
    #[test]
    fn an_annotation_round_trips_through_json() {
        let a = Annotation::new(
            AnnotationKind::Callout,
            Anchor::Data {
                x: 32.0,
                y: 32.0,
                z: 12.0,
            },
            "peak",
            Author::Agent,
        )
        .with_label_offset(40.0, -30.0);
        let json = serde_json::to_string(&a).expect("serialize");
        let back: Annotation = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(a, back);
        assert!(json.contains("\"space\":\"data\""), "{json}");
        assert!(json.contains("\"author\":\"agent\""), "{json}");
    }

    /// A file written by an older build still loads.
    #[test]
    fn an_annotation_without_the_optional_fields_loads() {
        let json = r#"{
            "id": "ann-1", "kind": "text",
            "anchor": {"space": "imagePixel", "x": 1.0, "y": 2.0}
        }"#;
        let a: Annotation = serde_json::from_str(json).expect("older form loads");
        assert_eq!(a.author, Author::User, "an unattributed mark is the user's");
        assert!(a.text.is_empty());
        assert!(a.label_offset.is_none());
    }
}

#[cfg(test)]
mod style_tests {
    use super::*;

    /// A mark saved before styling existed loads exactly as it always did.
    ///
    /// Against a JSON FIXTURE rather than a round-tripped struct, because the
    /// fixture is what is actually on disk. Marks persist per file, and a
    /// release that silently restyled everything anyone had drawn would be a
    /// bug with no error message — which is the whole reason `style` is an
    /// Option rather than a struct with defaults.
    #[test]
    fn a_mark_saved_before_styling_is_unchanged() {
        // The real on-disk shape, taken from a serialised mark rather than
        // guessed — the first version of this fixture invented an anchor
        // encoding and proved nothing about what is actually stored.
        let stored = r#"{
            "id": "ann-d412bbb071cf420cb6f732856487766d",
            "kind": "circle",
            "anchor": { "space": "imagePixel", "x": 10.0, "y": 20.0 },
            "extent": { "halfWidth": 12.0, "halfHeight": 12.0 },
            "text": "NGC 5194",
            "author": "agent",
            "createdAt": "2026-01-01T00:00:00Z"
        }"#;
        let a: Annotation = serde_json::from_str(stored).expect("an old mark still loads");
        assert_eq!(a.style, None, "an old mark must not acquire a style");
        // And it draws the way it always drew: an agent's mark in agent ink.
        assert_eq!(a.effective_style().colour, AGENT_INK);
        assert_eq!(a.effective_style().font_size, DEFAULT_FONT_SIZE);
        assert_eq!(a.effective_style().stroke, DEFAULT_STROKE);
    }

    /// An unstyled mark stays unstyled on disk.
    ///
    /// `skip_serializing_if` keeps the key out entirely, so saving a file with
    /// this build and opening it with the previous one changes nothing.
    #[test]
    fn an_unstyled_mark_writes_no_style_key() {
        let a = Annotation::new(
            AnnotationKind::Circle,
            Anchor::ImagePixel { x: 1.0, y: 2.0 },
            "",
            Author::User,
        );
        let json = serde_json::to_string(&a).expect("serialises");
        assert!(
            !json.contains("style"),
            "an unstyled mark wrote a style key: {json}"
        );
    }

    /// A styled mark round-trips.
    #[test]
    fn a_styled_mark_survives_the_store() {
        let mut a = Annotation::new(
            AnnotationKind::Rect,
            Anchor::ImagePixel { x: 1.0, y: 2.0 },
            "x",
            Author::User,
        );
        a.style = Some(MarkStyle {
            colour: (1.0, 0.5, 0.0),
            font_size: 18.0,
            bold: true,
            stroke: 3.0,
        });
        let json = serde_json::to_string(&a).expect("serialises");
        let back: Annotation = serde_json::from_str(&json).expect("deserialises");
        assert_eq!(back.style, a.style);
    }

    /// A style that cannot be drawn is clamped, not honoured.
    ///
    /// A zero stroke draws nothing and a zero font size is an invisible label;
    /// both look like the mark having been lost rather than like a setting.
    /// An agent can send any number, so this is the boundary that has to hold.
    #[test]
    fn an_unusable_style_is_clamped_to_something_visible() {
        let mut a = Annotation::new(
            AnnotationKind::Circle,
            Anchor::ImagePixel { x: 0.0, y: 0.0 },
            "",
            Author::User,
        );
        a.style = Some(MarkStyle {
            colour: (5.0, -1.0, f64::NAN),
            font_size: 0.0,
            bold: false,
            stroke: 0.0,
        });
        let s = a.effective_style();
        assert!(
            s.font_size >= 6.0,
            "font size {} is unreadable",
            s.font_size
        );
        assert!(s.stroke >= 0.5, "stroke {} draws nothing", s.stroke);
        assert_eq!(s.colour, (1.0, 0.0, 0.0), "colour left the 0..1 range");
    }
}
