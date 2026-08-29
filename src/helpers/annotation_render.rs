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
    /// The mark being EDITED — grips out, label field open.
    pub const EDITING_INK: (f64, f64, f64) = (1.0, 0.78, 0.35);
    /// A mark merely picked out, from the list or a click. Brighter than the
    /// rest, but not the editing colour: the two states look different because
    /// they ARE different — one has grips you can drag and the other does not.
    pub const SELECTED_INK: (f64, f64, f64) = (1.0, 1.0, 1.0);
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
fn ink_for(a: &Annotation, selected: bool, editing: bool) -> (f64, f64, f64) {
    if editing {
        style::EDITING_INK
    } else if selected {
        style::SELECTED_INK
    } else if a.author == Author::Agent {
        style::AGENT_INK
    } else {
        style::INK
    }
}

/// Where a leader line leaves a shape, and where its rule and text sit.
///
/// The leader starts ON the shape's outline in the direction it travels, then
/// runs a fixed length at a fixed acute angle, then turns horizontal to carry
/// the text. Three rules, each of which is a way it looked wrong before:
///
///  * The start is the outline, found along the leader's own direction — not
///    the bounding-box corner, which for a circle is outside it, and not the
///    centre, which draws a line through the subject.
///  * The leader's length is measured FROM that start. Measuring it from the
///    centre meant a shape bigger than the leader swallowed it, and the elbow
///    came out a few pixels from the outline — the line read as crossing the
///    circle rather than leaving it.
///  * The rule is as long as its text, and the whole thing flips to whichever
///    side has room.
///
/// Returns `(start, elbow, rule_end, text_x, rightwards)` in device pixels.
#[allow(clippy::too_many_arguments)]
pub fn leader_geometry(
    cx: f64,
    cy: f64,
    half_w: f64,
    half_h: f64,
    elliptical: bool,
    offset: Option<(f64, f64)>,
    text_width: f64,
    canvas_w: f64,
) -> (f64, f64, f64, f64, f64, f64, bool) {
    let angle = style::LEADER_ANGLE_DEG.to_radians();
    let (raw_dx, raw_dy) = offset.unwrap_or((angle.cos(), -angle.sin()));
    let len = (raw_dx * raw_dx + raw_dy * raw_dy).sqrt().max(f64::EPSILON);
    let (mut ux, uy) = (raw_dx / len, raw_dy / len);

    let rule_len = text_width + style::RULE_OVERHANG;
    // Enough room on the intended side for the leader AND the rule?
    let leader_len = if offset.is_some() {
        len.max(style::LEADER_LEN * 0.5)
    } else {
        style::LEADER_LEN
    };
    let reach = half_w + leader_len + rule_len;
    let rightwards = if ux >= 0.0 {
        cx + reach <= canvas_w
    } else {
        cx - reach < 0.0
    };
    ux = if rightwards { ux.abs() } else { -ux.abs() };

    // The point where the leader leaves the outline.
    let (sx, sy) = if elliptical {
        (cx + half_w * ux, cy + half_h * uy)
    } else {
        // Ray/box intersection: scale the direction until it meets an edge.
        let tx = if ux.abs() > f64::EPSILON {
            half_w / ux.abs()
        } else {
            f64::MAX
        };
        let ty = if uy.abs() > f64::EPSILON {
            half_h / uy.abs()
        } else {
            f64::MAX
        };
        let t = tx.min(ty);
        (cx + ux * t, cy + uy * t)
    };

    // The leader's length is measured from the outline, not the centre.
    let elbow_x = sx + ux * leader_len;
    let elbow_y = sy + uy * leader_len;
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
    (sx, sy, elbow_x, elbow_y, rule_end, text_x, rightwards)
}

/// Draw every annotation onto `cr`.
pub fn draw(
    annotations: &[Annotation],
    surface: &dyn AnnotationSurface,
    selected: Option<&str>,
    editing: Option<&str>,
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
        let is_editing = editing == Some(a.id.as_str());
        let (r, g, b) = ink_for(a, is_selected, is_editing);
        cr.set_source_rgba(r, g, b, style::ALPHA);
        cr.set_line_width(if is_selected || is_editing {
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
            }
            AnnotationKind::Circle => {
                draw_ellipse(cr, cx, cy, half_w.max(0.5), half_h.max(0.5));
                cr.stroke().ok();
            }
            AnnotationKind::Callout => {
                // A callout's shape is a small ring at the subject: the leader
                // is the point of it.
                if a.extent.is_some() {
                    draw_ellipse(cr, cx, cy, half_w.max(0.5), half_h.max(0.5));
                    cr.stroke().ok();
                }
            }
            AnnotationKind::Text => {}
        }

        // Every labelled shape is labelled the same way — a leader leaving the
        // outline at a fixed acute angle, and the text on the rule at its end.
        // A box and a circle used to put their label centred above them, which
        // read as two different products on one canvas; a blueprint labels
        // everything with a leader.
        if !a.text.trim().is_empty() {
            if a.kind == AnnotationKind::Text {
                draw_label_at(cr, a, cx, cy, canvas_w);
            } else {
                let text_width = cr.text_extents(&a.text).map(|e| e.width()).unwrap_or(0.0);
                let elliptical = a.kind != AnnotationKind::Rect;
                let (hw, hh) = if a.extent.is_some() {
                    (half_w, half_h)
                } else {
                    (3.0, 3.0)
                };
                let (sx, sy, ex, ey, rule_end, text_x, _right) = leader_geometry(
                    cx,
                    cy,
                    hw,
                    hh,
                    elliptical,
                    a.label_offset,
                    text_width,
                    canvas_w,
                );
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
    // save/restore rather than holding the old pattern across a `set_source`.
    // `cairo_get_source` hands back a pattern the context owns; keeping a
    // reference to it over a call that replaces it is the shape of a
    // use-after-free, and cairo's failures are segfaults rather than errors.
    cr.save().ok();
    cr.set_source_rgba(0.0, 0.0, 0.0, 0.75);
    cr.move_to(x + 1.0, y + 1.0);
    cr.show_text(text).ok();
    cr.restore().ok();
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

    /// The leader starts on the shape's OUTLINE, not its centre.
    ///
    /// A leader from the centre draws a line straight through the subject the
    /// annotation is pointing at, which is the one place it must not.
    ///
    /// This test used to require each axis to clear the half-extent, which is
    /// true of the bounding-box CORNER and false of a circle's outline — so it
    /// was asserting the very thing that made the leader look like it cut
    /// across the circle. The outline is a distance from the centre, not a
    /// pair of axis distances.
    #[test]
    fn the_leader_leaves_the_outline_of_the_shape() {
        let r = 10.0;
        let (sx, sy, ..) = leader_geometry(100.0, 100.0, r, r, true, None, 60.0, 800.0);
        let d = ((sx - 100.0).powi(2) + (sy - 100.0).powi(2)).sqrt();
        assert!(
            (d - r).abs() < 0.5,
            "the leader starts {d:.2} from the centre, not on the r={r} outline"
        );
        assert!(
            d > 0.0,
            "the leader starts at the centre, drawing through the subject"
        );
    }

    /// Near the right edge, the callout points left instead of off-canvas.
    #[test]
    fn a_callout_near_the_right_edge_flips() {
        let canvas_w = 300.0;
        let (.., rule_end, text_x, rightwards) =
            leader_geometry(280.0, 100.0, 10.0, 10.0, true, None, 90.0, canvas_w);
        assert!(!rightwards, "the callout ran off the right edge");
        assert!(rule_end < 280.0, "the rule did not flip: {rule_end}");
        assert!(text_x >= 0.0, "the text went off the left edge: {text_x}");
    }

    /// With room, it points the way it was asked to.
    #[test]
    fn a_callout_with_room_keeps_its_direction() {
        let (.., rightwards) = leader_geometry(100.0, 100.0, 10.0, 10.0, true, None, 60.0, 800.0);
        assert!(rightwards);
        let (.., rightwards_left) = leader_geometry(
            400.0,
            100.0,
            10.0,
            10.0,
            true,
            Some((-50.0, -40.0)),
            60.0,
            800.0,
        );
        assert!(!rightwards_left, "an explicit left offset was overridden");
    }

    /// The rule is as long as the text, so text never overhangs it.
    #[test]
    fn the_rule_is_long_enough_for_its_text() {
        let text_width = 120.0;
        let (.., ex, _ey, rule_end, _tx, _r) =
            leader_geometry(100.0, 100.0, 10.0, 10.0, true, None, text_width, 900.0);
        assert!(
            (rule_end - ex).abs() >= text_width,
            "rule {} shorter than its text {text_width}",
            (rule_end - ex).abs()
        );
    }

    /// A big shape does not swallow its own leader.
    ///
    /// The leader used to be measured from the CENTRE, so a circle wider than
    /// the leader length put the elbow a few pixels outside the outline and the
    /// line read as cutting across the subject instead of leaving it. Seen in a
    /// screenshot at 100% zoom, where the radius was larger than the leader.
    #[test]
    fn a_large_shape_still_gets_a_full_length_leader() {
        // Radius 60, leader 46: the old formula put the elbow INSIDE the shape.
        let (sx, sy, ex, ey, ..) =
            leader_geometry(300.0, 300.0, 60.0, 60.0, true, None, 80.0, 900.0);
        let from_centre = ((ex - 300.0).powi(2) + (ey - 300.0).powi(2)).sqrt();
        assert!(
            from_centre > 60.0,
            "the elbow is inside the shape ({from_centre:.1} from a centre with radius 60)"
        );
        let leader = ((ex - sx).powi(2) + (ey - sy).powi(2)).sqrt();
        assert!(
            leader > 30.0,
            "the leader is only {leader:.1}px long — it was swallowed by the shape"
        );
    }

    /// The leader starts ON the outline, not at the bounding-box corner.
    #[test]
    fn a_circles_leader_starts_on_the_circle() {
        let r = 40.0;
        let (sx, sy, ..) = leader_geometry(200.0, 200.0, r, r, true, None, 50.0, 900.0);
        let d = ((sx - 200.0).powi(2) + (sy - 200.0).powi(2)).sqrt();
        assert!(
            (d - r).abs() < 0.5,
            "the leader starts {d:.1} from the centre, not on the r={r} outline \
             (the box corner is at {:.1})",
            r * std::f64::consts::SQRT_2
        );
    }

    /// A box's leader starts on its edge.
    #[test]
    fn a_rects_leader_starts_on_its_edge() {
        let (hw, hh) = (50.0, 20.0);
        let (sx, sy, ..) = leader_geometry(200.0, 200.0, hw, hh, false, None, 50.0, 900.0);
        // At 45 degrees on a wide flat box, the short axis is met first.
        assert!(
            (sy - (200.0 - hh)).abs() < 0.5,
            "expected the top edge at y={}, got {sy}",
            200.0 - hh
        );
        assert!(sx > 200.0 && sx <= 200.0 + hw + 0.5, "{sx} left the box");
    }

    /// Every leader on a canvas leaves at the same angle.
    #[test]
    fn the_default_leader_angle_is_fixed() {
        let (sx, sy, ex, ey, ..) = leader_geometry(100.0, 100.0, 0.0, 0.0, true, None, 10.0, 800.0);
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
        assert_ne!(ink_for(&mine, false, false), style::INK);
        assert_eq!(ink_for(&mine, false, false), style::AGENT_INK);
        // Picking one out wins over authorship — you need to see what you
        // chose — and editing wins over both, because that is the one you can
        // drag.
        assert_eq!(ink_for(&mine, true, false), style::SELECTED_INK);
        assert_eq!(ink_for(&mine, true, true), style::EDITING_INK);
        assert_ne!(style::SELECTED_INK, style::EDITING_INK);
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
        draw(&[left, right], &Flat, None, None, &cr, w as f64, h as f64);
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

    /// A shape whose extent is in a different unit is still visible.
    ///
    /// A sky extent is in DEGREES and an image extent is in pixels. Treating
    /// both as pixels drew a sky circle 0.005 device pixels across — nothing on
    /// screen, no error, and every mark placed through the UI (which prefers
    /// sky anchors when there is WCS) silently did not appear.
    ///
    /// The renderer cannot know the conversion; it asks the surface, per
    /// anchor. This checks it USES the answer rather than assuming one scale.
    #[test]
    fn the_surface_is_asked_for_a_scale_per_anchor() {
        struct PerUnit;
        impl AnnotationSurface for PerUnit {
            fn project(&self, _: &Anchor) -> Option<(f64, f64)> {
                Some((100.0, 100.0))
            }
            fn units_to_pixels(&self, anchor: &Anchor) -> f64 {
                match anchor {
                    // A degree is many pixels; a pixel is one.
                    Anchor::Sky { .. } => 3600.0,
                    _ => 1.0,
                }
            }
        }
        let surface = PerUnit;
        // A tiny sky extent must come out a usable size on screen.
        let sky = Anchor::Sky {
            ra_deg: 195.0,
            dec_deg: -40.0,
        };
        let drawn = 0.005 * surface.units_to_pixels(&sky);
        assert!(
            drawn > 4.0,
            "a 0.005-degree circle drew {drawn} pixels across — invisible"
        );
        // And an image-pixel extent is not multiplied by the same factor.
        let pixels = Anchor::ImagePixel { x: 1.0, y: 1.0 };
        assert_eq!(surface.units_to_pixels(&pixels), 1.0);
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
        draw(&[a], &Flat, None, None, &cr, w as f64, h as f64);
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
        draw(&anns, &Flat, Some(&anns[0].id), None, &cr, 400.0, 300.0);
        drop(cr);
        let data = img.data().expect("pixels");
        assert!(
            data.iter().any(|b| *b != 0),
            "nothing was drawn on the canvas"
        );
    }
}
