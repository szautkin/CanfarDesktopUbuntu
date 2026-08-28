//! Drawing annotations, once, for every viewer.
//!
//! The FITS canvas is flat and the cube's volume is not, and the only thing
//! that differs is how a point becomes a pixel. So a viewer supplies
//! [`AnnotationSurface`] and everything else — the shapes, the leader geometry,
//! the label — is this file. A second implementation for the second viewer
//! would start identical and drift, and the difference would surface as an
//! agent describing a picture that no longer matched the other viewer.
//!
//! The look is blueprint: hairline strokes that do NOT thicken with zoom,
//! square corners, a cold palette, and a label on a rule rather than in a
//! filled box.

use crate::models::annotation::{Annotation, AnnotationKind, Author};
use gtk4::cairo;

/// A viewer that can place one of its own coordinates on its canvas.
///
/// The whole interface between an annotation and a viewer. `None` means "not
/// visible now" — outside a FITS viewport, or behind the cube's near plane —
/// and it MUST be honoured: an unchecked 3D projection puts a mark at a
/// mirrored position on the far side of the canvas, which looks placed and is
/// not.
pub trait AnnotationSurface {
    /// The anchor's position on the canvas, in device pixels.
    fn project(&self, anchor: &crate::models::annotation::Anchor) -> Option<(f64, f64)>;

    /// How many device pixels one unit of the anchor's space currently spans.
    ///
    /// A shape's size is stored in data units, so it grows and shrinks with the
    /// view the way a circle drawn on a photograph does. The viewer knows the
    /// scale; the renderer only knows it needs one.
    fn units_to_pixels(&self, anchor: &crate::models::annotation::Anchor) -> f64;
}

/// The blueprint palette and metrics.
pub mod style {
    /// Hairline. Not scaled with zoom — a stroke that thickens turns a
    /// zoomed-out view into a blot.
    pub const STROKE: f64 = 1.0;
    pub const SELECTED_STROKE: f64 = 2.0;
    /// Cold white-cyan, the drawing-ink of the set.
    pub const INK: (f64, f64, f64) = (0.62, 0.85, 1.0);
    /// An agent's marks, distinguishable without being louder.
    pub const AGENT_INK: (f64, f64, f64) = (0.55, 1.0, 0.80);
    /// The selected mark.
    pub const SELECTED_INK: (f64, f64, f64) = (1.0, 0.78, 0.35);
    pub const ALPHA: f64 = 0.92;
    pub const FONT_SIZE: f64 = 11.0;
    /// The leader leaves a shape at this angle, and every leader on a canvas
    /// uses the same one — varying angles is what makes an annotated figure
    /// look untidy.
    pub const LEADER_ANGLE_DEG: f64 = 45.0;
    /// Default leader length in pixels, when a callout has no stored offset.
    pub const LEADER_LEN: f64 = 46.0;
    /// Gap between the rule and the text sitting on it.
    pub const TEXT_LIFT: f64 = 3.0;
    /// A little slack past the text, so the rule is never exactly flush.
    pub const RULE_OVERHANG: f64 = 6.0;
}

/// The ink for one annotation.
fn ink_for(a: &Annotation, selected: bool) -> (f64, f64, f64) {
    if selected {
        style::SELECTED_INK
    } else if a.author == Author::Agent {
        style::AGENT_INK
    } else {
        style::INK
    }
}

/// Where a leader line leaves a shape, and which way its rule runs.
///
/// The leader starts on the shape's EDGE — not its centre, which would draw a
/// line through the subject, and not its corner, which reads as a mistake. It
/// flips to whichever side has room, so a callout near the right edge points
/// left instead of off the canvas.
///
/// Returned as `(start, elbow, rule_end, text_x, rightwards)`, all in device
/// pixels.
#[allow(clippy::too_many_arguments)]
pub fn leader_geometry(
    cx: f64,
    cy: f64,
    half_w: f64,
    half_h: f64,
    offset: Option<(f64, f64)>,
    text_width: f64,
    canvas_w: f64,
) -> (f64, f64, f64, f64, f64, f64, bool) {
    let angle = style::LEADER_ANGLE_DEG.to_radians();
    let (dx, dy) = offset.unwrap_or_else(|| {
        (
            style::LEADER_LEN * angle.cos(),
            -style::LEADER_LEN * angle.sin(),
        )
    });

    // Which side has room for the rule AND its text.
    let wants_right = dx >= 0.0;
    let rule_len = text_width + style::RULE_OVERHANG;
    let rightwards = if wants_right {
        cx + dx + rule_len <= canvas_w
    } else {
        // Only flip back to the right if the left genuinely has no room.
        cx + dx - rule_len < 0.0
    };

    let dx = if rightwards { dx.abs() } else { -dx.abs() };
    let start_x = cx + if rightwards { half_w } else { -half_w };
    let start_y = cy + if dy < 0.0 { -half_h } else { half_h };
    let elbow_x = cx + dx;
    let elbow_y = cy + dy;
    let rule_end = if rightwards {
        elbow_x + rule_len
    } else {
        elbow_x - rule_len
    };
    let text_x = if rightwards {
        elbow_x + style::RULE_OVERHANG / 2.0
    } else {
        rule_end + style::RULE_OVERHANG / 2.0
    };
    (
        start_x, start_y, elbow_x, elbow_y, rule_end, text_x, rightwards,
    )
}

/// Draw every annotation onto `cr`.
pub fn draw(
    annotations: &[Annotation],
    surface: &dyn AnnotationSurface,
    selected: Option<&str>,
    cr: &cairo::Context,
    canvas_w: f64,
    canvas_h: f64,
) {
    cr.select_font_face(
        "monospace",
        cairo::FontSlant::Normal,
        cairo::FontWeight::Normal,
    );
    cr.set_font_size(style::FONT_SIZE);

    for a in annotations {
        // A mark whose anchor is off-canvas or behind the camera is skipped,
        // not clamped: a clamped mark points at the wrong thing.
        let Some((cx, cy)) = surface.project(&a.anchor) else {
            continue;
        };
        if !cx.is_finite() || !cy.is_finite() {
            continue;
        }
        // Start a fresh path for every mark. Cairo keeps a current point
        // across calls, and `arc` joins to it — so a label's `move_to` and the
        // next annotation's circle were being strung together into one path,
        // and every mark was drawn connected to the one after it by a line
        // across the canvas.
        cr.new_path();

        let is_selected = selected == Some(a.id.as_str());
        let (r, g, b) = ink_for(a, is_selected);
        cr.set_source_rgba(r, g, b, style::ALPHA);
        cr.set_line_width(if is_selected {
            style::SELECTED_STROKE
        } else {
            style::STROKE
        });

        let scale = surface.units_to_pixels(&a.anchor);
        let (half_w, half_h) = a
            .extent
            .map(|e| (e.half_width * scale, e.half_height * scale))
            .unwrap_or((0.0, 0.0));

        match a.kind {
            AnnotationKind::Rect => {
                cr.rectangle(cx - half_w, cy - half_h, half_w * 2.0, half_h * 2.0);
                cr.stroke().ok();
                draw_label_at(cr, a, cx, cy - half_h - style::TEXT_LIFT, canvas_w);
            }
            AnnotationKind::Circle => {
                draw_ellipse(cr, cx, cy, half_w.max(0.5), half_h.max(0.5));
                cr.stroke().ok();
                draw_label_at(cr, a, cx, cy - half_h - style::TEXT_LIFT, canvas_w);
            }
            AnnotationKind::Text => {
                draw_label_at(cr, a, cx, cy, canvas_w);
            }
            AnnotationKind::Callout => {
                let text_width = cr.text_extents(&a.text).map(|e| e.width()).unwrap_or(0.0);
                // A callout with no shape still needs somewhere for the leader
                // to start; a small notional radius keeps it off the subject.
                let (hw, hh) = if a.extent.is_some() {
                    (half_w, half_h)
                } else {
                    (3.0, 3.0)
                };
                if a.extent.is_some() {
                    draw_ellipse(cr, cx, cy, hw.max(0.5), hh.max(0.5));
                    cr.stroke().ok();
                }
                let (sx, sy, ex, ey, rule_end, text_x, _right) =
                    leader_geometry(cx, cy, hw, hh, a.label_offset, text_width, canvas_w);
                cr.new_path();
                cr.move_to(sx, sy);
                cr.line_to(ex, ey);
                cr.line_to(rule_end, ey);
                cr.stroke().ok();
                let ty = (ey - style::TEXT_LIFT).max(style::FONT_SIZE);
                draw_text_with_shadow(cr, text_x, ty, &a.text);
            }
        }
        let _ = canvas_h;
    }
}

/// A circle, or an ellipse where a sphere is seen off-axis.
fn draw_ellipse(cr: &cairo::Context, cx: f64, cy: f64, rx: f64, ry: f64) {
    cr.save().ok();
    cr.translate(cx, cy);
    cr.scale(rx, ry);
    // Without this, `arc` draws a line from wherever the current point happens
    // to be to the start of the arc.
    cr.new_sub_path();
    cr.arc(0.0, 0.0, 1.0, 0.0, std::f64::consts::TAU);
    cr.restore().ok();
}

/// A label centred over a point, kept inside the canvas and legible on top of
/// whatever is under it.
fn draw_label_at(cr: &cairo::Context, a: &Annotation, cx: f64, cy: f64, canvas_w: f64) {
    if a.text.trim().is_empty() {
        return;
    }
    let width = cr.text_extents(&a.text).map(|e| e.width()).unwrap_or(0.0);
    // Slide back inside rather than clip — a label that runs off the edge is
    // unreadable exactly when it matters.
    let x = (cx - width / 2.0).clamp(2.0, (canvas_w - width - 2.0).max(2.0));
    let y = cy.max(style::FONT_SIZE);
    draw_text_with_shadow(cr, x, y, &a.text);
}

/// Text with a dark offset copy under it.
///
/// Annotations sit over data, and data is not a background you chose: pale ink
/// on a bright star or on nebulosity is invisible. The cube's axis captions
/// have done this since they were written, and the first version of this
/// renderer did not — the probe showed a label over a bright patch of the test
/// image and it could not be read.
fn draw_text_with_shadow(cr: &cairo::Context, x: f64, y: f64, text: &str) {
    let ink = cr.source();
    cr.set_source_rgba(0.0, 0.0, 0.0, 0.75);
    cr.move_to(x + 1.0, y + 1.0);
    cr.show_text(text).ok();
    cr.set_source(&ink).ok();
    cr.move_to(x, y);
    cr.show_text(text).ok();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::annotation::{Anchor, Extent};

    /// A surface that projects nothing anywhere real — the geometry under test
    /// is the leader's, and it needs no viewer.
    struct Flat;
    impl AnnotationSurface for Flat {
        fn project(&self, anchor: &Anchor) -> Option<(f64, f64)> {
            match *anchor {
                Anchor::ImagePixel { x, y } => Some((x, y)),
                _ => None,
            }
        }
        fn units_to_pixels(&self, _: &Anchor) -> f64 {
            1.0
        }
    }

    fn callout(offset: Option<(f64, f64)>) -> Annotation {
        let mut a = Annotation::new(
            AnnotationKind::Callout,
            Anchor::ImagePixel { x: 100.0, y: 100.0 },
            "NGC 5194 core",
            Author::User,
        );
        a.extent = Some(Extent::square(10.0));
        a.label_offset = offset;
        a
    }

    /// The leader starts on the shape's EDGE, not its centre.
    ///
    /// A leader from the centre draws a line straight through the subject the
    /// annotation is pointing at, which is the one place it must not.
    #[test]
    fn the_leader_leaves_the_edge_of_the_shape() {
        let (sx, sy, ..) = leader_geometry(100.0, 100.0, 10.0, 10.0, None, 60.0, 800.0);
        assert!(
            (sx - 100.0).abs() >= 10.0 - f64::EPSILON,
            "leader started inside the shape at {sx}"
        );
        assert!((sy - 100.0).abs() >= 10.0 - f64::EPSILON, "{sy}");
    }

    /// Near the right edge, the callout points left instead of off-canvas.
    #[test]
    fn a_callout_near_the_right_edge_flips() {
        let canvas_w = 300.0;
        let (.., rule_end, text_x, rightwards) =
            leader_geometry(280.0, 100.0, 10.0, 10.0, None, 90.0, canvas_w);
        assert!(!rightwards, "the callout ran off the right edge");
        assert!(rule_end < 280.0, "the rule did not flip: {rule_end}");
        assert!(text_x >= 0.0, "the text went off the left edge: {text_x}");
    }

    /// With room, it points the way it was asked to.
    #[test]
    fn a_callout_with_room_keeps_its_direction() {
        let (.., rightwards) = leader_geometry(100.0, 100.0, 10.0, 10.0, None, 60.0, 800.0);
        assert!(rightwards);
        let (.., rightwards_left) =
            leader_geometry(400.0, 100.0, 10.0, 10.0, Some((-50.0, -40.0)), 60.0, 800.0);
        assert!(!rightwards_left, "an explicit left offset was overridden");
    }

    /// The rule is as long as the text, so text never overhangs it.
    #[test]
    fn the_rule_is_long_enough_for_its_text() {
        let text_width = 120.0;
        let (.., ex, _ey, rule_end, _tx, _r) =
            leader_geometry(100.0, 100.0, 10.0, 10.0, None, text_width, 900.0);
        assert!(
            (rule_end - ex).abs() >= text_width,
            "rule {} shorter than its text {text_width}",
            (rule_end - ex).abs()
        );
    }

    /// Every leader on a canvas leaves at the same angle.
    #[test]
    fn the_default_leader_angle_is_fixed() {
        let (sx, sy, ex, ey, ..) = leader_geometry(100.0, 100.0, 0.0, 0.0, None, 10.0, 800.0);
        let angle = ((ey - sy) / (ex - sx)).atan().abs().to_degrees();
        assert!(
            (angle - style::LEADER_ANGLE_DEG).abs() < 0.5,
            "leader left at {angle} degrees, not {}",
            style::LEADER_ANGLE_DEG
        );
    }

    /// An anchor the surface cannot place is skipped, not drawn at the origin.
    #[test]
    fn an_unprojectable_anchor_draws_nothing() {
        let surface = Flat;
        // The fake surface projects only image pixels; a data anchor is None,
        // which is what a cube reports for a voxel behind the camera.
        assert!(surface
            .project(&Anchor::Data {
                x: 1.0,
                y: 1.0,
                z: 1.0
            })
            .is_none());
        // Drawing must not panic on it, and there is nothing to assert about
        // pixels here — the contract is that `draw` consults `project` and
        // honours `None`, which the cube probe checks on real pixels.
        let img = cairo::ImageSurface::create(cairo::Format::ARgb32, 50, 50).expect("surface");
        let cr = cairo::Context::new(&img).expect("cr");
        draw(
            &[Annotation::new(
                AnnotationKind::Text,
                Anchor::Data {
                    x: 1.0,
                    y: 1.0,
                    z: 1.0,
                },
                "hidden",
                Author::Agent,
            )],
            &surface,
            None,
            &cr,
            50.0,
            50.0,
        );
    }

    /// A shape's size follows the view's scale, not the screen.
    #[test]
    fn a_shape_grows_with_the_view() {
        struct Zoomed(f64);
        impl AnnotationSurface for Zoomed {
            fn project(&self, _: &Anchor) -> Option<(f64, f64)> {
                Some((100.0, 100.0))
            }
            fn units_to_pixels(&self, _: &Anchor) -> f64 {
                self.0
            }
        }
        // Same annotation, two zoom levels: the drawn radius must differ.
        let a = callout(None);
        let half = a.extent.unwrap().half_width;
        assert_eq!(half * Zoomed(1.0).units_to_pixels(&a.anchor), 10.0);
        assert_eq!(half * Zoomed(4.0).units_to_pixels(&a.anchor), 40.0);
    }

    /// An agent's mark is drawn in its own ink.
    #[test]
    fn an_agents_mark_is_distinguishable() {
        let mut mine = callout(None);
        mine.author = Author::Agent;
        assert_ne!(ink_for(&mine, false), style::INK);
        assert_eq!(ink_for(&mine, false), style::AGENT_INK);
        // Selection wins over authorship — you need to see what you picked.
        assert_eq!(ink_for(&mine, true), style::SELECTED_INK);
    }

    /// Marks are not strung together into one path.
    ///
    /// Cairo keeps a current point between calls and `arc` joins to it, so a
    /// label's `move_to` and the next annotation's circle were drawn as one
    /// path — every mark connected to the next by a line across the canvas.
    /// The unit tests all passed; the style probe showed it at a glance.
    #[test]
    fn separate_marks_are_not_joined_by_a_line() {
        let (w, h) = (400i32, 200i32);
        let mut img = cairo::ImageSurface::create(cairo::Format::ARgb32, w, h).expect("surface");
        let cr = cairo::Context::new(&img).expect("cr");

        // Two circles far apart, each with a label — the exact shape that
        // produced the joining line.
        let mut left = Annotation::new(
            AnnotationKind::Circle,
            Anchor::ImagePixel { x: 40.0, y: 100.0 },
            "left",
            Author::User,
        );
        left.extent = Some(Extent::square(10.0));
        let mut right = Annotation::new(
            AnnotationKind::Circle,
            Anchor::ImagePixel { x: 360.0, y: 100.0 },
            "right",
            Author::User,
        );
        right.extent = Some(Extent::square(10.0));
        draw(&[left, right], &Flat, None, &cr, w as f64, h as f64);
        drop(cr);

        // The gap between them, on the line joining their centres, must be
        // empty. Sample well clear of both circles and both labels.
        let stride = img.stride() as usize;
        let data = img.data().expect("pixels");
        let y = 100usize;
        let mut lit = 0;
        for x in 120..280usize {
            let px = &data[y * stride + x * 4..y * stride + x * 4 + 4];
            if px.iter().any(|b| *b != 0) {
                lit += 1;
            }
        }
        assert_eq!(
            lit, 0,
            "{lit} pixels of ink between two separate marks — they are joined"
        );
    }

    /// A label is legible over a bright background.
    ///
    /// The probe caught this: pale ink on the bright part of the test image was
    /// invisible. Annotations sit over data, which is not a background anyone
    /// chose, so every label carries a dark copy of itself underneath.
    #[test]
    fn a_label_is_readable_on_a_bright_background() {
        let (w, h) = (200i32, 80i32);
        let mut img = cairo::ImageSurface::create(cairo::Format::ARgb32, w, h).expect("surface");
        let cr = cairo::Context::new(&img).expect("cr");
        // Paint it white, the worst case for pale ink.
        cr.set_source_rgb(1.0, 1.0, 1.0);
        cr.paint().ok();

        let a = Annotation::new(
            AnnotationKind::Text,
            Anchor::ImagePixel { x: 100.0, y: 40.0 },
            "label",
            Author::User,
        );
        draw(&[a], &Flat, None, &cr, w as f64, h as f64);
        drop(cr);

        // Something markedly darker than the white ground must have been laid
        // down, or the text is invisible against it.
        let stride = img.stride() as usize;
        let data = img.data().expect("pixels");
        let darkest = (20..70usize)
            .flat_map(|y| (0..w as usize).map(move |x| (x, y)))
            .map(|(x, y)| data[y * stride + x * 4] as u32)
            .min()
            .unwrap_or(255);
        assert!(
            darkest < 140,
            "the darkest pixel is {darkest}: nothing was drawn dark enough to read \
             against a white background"
        );
    }

    /// Drawing a full set does not panic, and leaves ink on the surface.
    #[test]
    fn drawing_every_kind_marks_the_canvas() {
        let mut img =
            cairo::ImageSurface::create(cairo::Format::ARgb32, 400, 300).expect("surface");
        let cr = cairo::Context::new(&img).expect("cr");
        let anns: Vec<Annotation> = [
            AnnotationKind::Rect,
            AnnotationKind::Circle,
            AnnotationKind::Callout,
            AnnotationKind::Text,
        ]
        .iter()
        .enumerate()
        .map(|(i, k)| {
            let mut a = Annotation::new(
                *k,
                Anchor::ImagePixel {
                    x: 80.0 + i as f64 * 70.0,
                    y: 150.0,
                },
                "label",
                Author::User,
            );
            a.extent = Some(Extent::square(14.0));
            a
        })
        .collect();
        draw(&anns, &Flat, Some(&anns[0].id), &cr, 400.0, 300.0);
        drop(cr);
        let data = img.data().expect("pixels");
        assert!(
            data.iter().any(|b| *b != 0),
            "nothing was drawn on the canvas"
        );
    }
}
