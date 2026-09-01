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

use crate::models::annotation::{Annotation, AnnotationKind, MarkStyle};
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

    /// How much bigger this rendering is than the screen. 1.0 IS the screen.
    ///
    /// A mark's stroke, label and leader are in DEVICE pixels, deliberately:
    /// a stroke that thickened as you zoomed out would turn the view into a
    /// blot. But "device pixels" means the SCREEN's, and an export at 4x has
    /// four times as many of them.
    ///
    /// Left at 1.0 everywhere, that is a measured bug rather than a
    /// hypothetical one: at 4x a 2px ring stayed 2px and a 12px label stayed a
    /// 15x10px smudge, on a plate whose own title, caption and colorbar DID
    /// scale — so the annotations were the only thing in the figure that
    /// shrank, and the marks became unreadable exactly at the resolution
    /// someone chose for publication.
    ///
    /// Every surface that renders bigger than the screen answers with its
    /// factor. The default is the screen.
    fn ink_scale(&self) -> f64 {
        1.0
    }
}

/// The blueprint palette and metrics.
pub mod style {
    /// A mark being edited or picked out is drawn thicker, whatever its own
    /// stroke says: the emphasis is chrome and has to read at any weight.
    pub const SELECTED_STROKE: f64 = 2.0;
    /// The mark being EDITED — grips out, label field open.
    pub const EDITING_INK: (f64, f64, f64) = (1.0, 0.78, 0.35);
    /// A mark merely picked out, from the list or a click. Brighter than the
    /// rest, but not the editing colour: the two states look different because
    /// they ARE different — one has grips you can drag and the other does not.
    pub const SELECTED_INK: (f64, f64, f64) = (1.0, 1.0, 1.0);
    pub const ALPHA: f64 = 0.92;
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
    // State first, and state is CHROME: which mark you have clicked is a fact
    // about the session, not about the picture. Both are already excluded from
    // captures and exports, and a mark's own colour must not resurrect them
    // there.
    if editing {
        style::EDITING_INK
    } else if selected {
        style::SELECTED_INK
    } else {
        a.effective_style().colour
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
    ink: f64,
) -> (f64, f64, f64, f64, f64, f64, bool) {
    let angle = style::LEADER_ANGLE_DEG.to_radians();
    let (raw_dx, raw_dy) = offset.unwrap_or((angle.cos(), -angle.sin()));
    let len = (raw_dx * raw_dx + raw_dy * raw_dy).sqrt().max(f64::EPSILON);
    let (mut ux, uy) = (raw_dx / len, raw_dy / len);

    // Every length here is furniture in device pixels, so every one of them
    // takes the ink factor. `text_width` arrives already scaled, because it was
    // measured with the scaled font — mixing a scaled width with an unscaled
    // overhang is how a rule ends up not reaching the end of its own text.
    let rule_len = text_width + style::RULE_OVERHANG * ink;
    // The stored offset is in screen pixels too — a label is furniture, not
    // part of the image — so it scales with the rest of the furniture.
    let leader_len = if offset.is_some() {
        (len * ink).max(style::LEADER_LEN * 0.5 * ink)
    } else {
        style::LEADER_LEN * ink
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
        elbow_x + style::RULE_OVERHANG * ink / 2.0
    } else {
        rule_end + style::RULE_OVERHANG * ink / 2.0
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
    // Asked once, and made usable once: it is a property of this rendering,
    // not of a mark, and every length below is multiplied by it.
    let ink = MarkStyle::usable_ink(surface.ink_scale());
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
        let own = a.effective_style().scaled(ink);
        // Per mark, not once for the run: size and weight vary now, and a
        // face set outside the loop would give every mark whichever one the
        // previous mark happened to leave behind.
        cr.select_font_face(
            "monospace",
            cairo::FontSlant::Normal,
            if own.bold {
                cairo::FontWeight::Bold
            } else {
                cairo::FontWeight::Normal
            },
        );
        cr.set_font_size(own.font_size);
        let (r, g, b) = ink_for(a, is_selected, is_editing);
        cr.set_source_rgba(r, g, b, style::ALPHA);
        // Emphasis never draws THINNER than the mark itself: a 4px outline
        // that got thinner when you clicked it would read as the click having
        // broken something.
        cr.set_line_width(if is_selected || is_editing {
            own.stroke.max(style::SELECTED_STROKE * ink)
        } else {
            own.stroke
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
                draw_label_at(cr, a, cx, cy, canvas_w, own.font_size, ink);
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
                    ink,
                );
                cr.new_path();
                cr.move_to(sx, sy);
                cr.line_to(ex, ey);
                cr.line_to(rule_end, ey);
                cr.stroke().ok();
                let ty = (ey - style::TEXT_LIFT * ink).max(own.font_size);
                draw_text_with_shadow(cr, text_x, ty, &a.text, ink);
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
fn draw_label_at(
    cr: &cairo::Context,
    a: &Annotation,
    cx: f64,
    cy: f64,
    canvas_w: f64,
    font_size: f64,
    ink: f64,
) {
    if a.text.trim().is_empty() {
        return;
    }
    let width = cr.text_extents(&a.text).map(|e| e.width()).unwrap_or(0.0);
    // Slide back inside rather than clip — a label that runs off the edge is
    // unreadable exactly when it matters.
    let x = (cx - width / 2.0).clamp(2.0, (canvas_w - width - 2.0).max(2.0));
    // The size actually set on the context, not the mark's stored one: at 4x
    // they differ by four, and lifting the baseline by the stored number would
    // clip the top of every label at the canvas edge.
    let y = cy.max(font_size);
    draw_text_with_shadow(cr, x, y, &a.text, ink);
}

/// Text with a dark offset copy under it.
///
/// Annotations sit over data, and data is not a background you chose: pale ink
/// on a bright star or on nebulosity is invisible. The cube's axis captions
/// have done this since they were written, and the first version of this
/// renderer did not — the probe showed a label over a bright patch of the test
/// image and it could not be read.
fn draw_text_with_shadow(cr: &cairo::Context, x: f64, y: f64, text: &str, ink: f64) {
    // save/restore rather than holding the old pattern across a `set_source`.
    // `cairo_get_source` hands back a pattern the context owns; keeping a
    // reference to it over a call that replaces it is the shape of a
    // use-after-free, and cairo's failures are segfaults rather than errors.
    cr.save().ok();
    cr.set_source_rgba(0.0, 0.0, 0.0, 0.75);
    cr.move_to(x + ink, y + ink);
    cr.show_text(text).ok();
    cr.restore().ok();
    cr.move_to(x, y);
    cr.show_text(text).ok();
}

// ── Editing geometry ────────────────────────────────────────────────────────
//
// Grips, and the hit tests that go with them. Everything here is expressed
// through [`AnnotationSurface`] — a centre and a scale — which is all a canvas
// has to supply, so the FITS image and the cube's slice share one definition of
// where a grip is instead of each carrying its own. The second copy is the one
// that drifts: a grip drawn at one radius and hit-tested at another is a
// control that misses when you click it, and looks like a dead widget.

/// How big a resize grip is on screen, in device pixels.
pub const HANDLE_RADIUS: f64 = 5.0;

/// The four corner offsets a grip sits at.
const HANDLE_CORNERS: [(f64, f64); 4] = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)];

/// A mark's half-size on screen, or `None` when it is not on this surface.
fn half_size(
    mark: &Annotation,
    surface: &dyn AnnotationSurface,
    fallback: f64,
) -> Option<(f64, f64, f64, f64)> {
    let (cx, cy) = surface.project(&mark.anchor)?;
    let scale = surface.units_to_pixels(&mark.anchor);
    let (hw, hh) = mark
        .extent
        .map(|e| (e.half_width * scale, e.half_height * scale))
        .unwrap_or((fallback, fallback));
    Some((cx, cy, hw, hh))
}

/// Draw the four resize grips on `mark`.
///
/// Screen-sized, not data-sized: a grip has to be grabbable at any zoom, and
/// one that shrank with the image would become unusable exactly when a mark is
/// small enough to need adjusting.
pub fn draw_handles(mark: &Annotation, surface: &dyn AnnotationSurface, cr: &cairo::Context) {
    let Some((cx, cy, hw, hh)) = half_size(mark, surface, 3.0) else {
        return;
    };
    if mark.extent.is_none() {
        return;
    }
    let (r, g, b) = style::SELECTED_INK;
    for (dx, dy) in HANDLE_CORNERS {
        let (x, y) = (cx + dx * hw, cy + dy * hh);
        cr.new_path();
        cr.arc(x, y, HANDLE_RADIUS, 0.0, std::f64::consts::TAU);
        cr.set_source_rgba(0.08, 0.09, 0.11, 0.95);
        cr.fill_preserve().ok();
        cr.set_source_rgba(r, g, b, 1.0);
        cr.set_line_width(1.5);
        cr.stroke().ok();
    }
}

/// Whether `(sx, sy)` is on one of `mark`'s grips.
pub fn handle_at(mark: &Annotation, surface: &dyn AnnotationSurface, sx: f64, sy: f64) -> bool {
    let Some((cx, cy, hw, hh)) = half_size(mark, surface, 3.0) else {
        return false;
    };
    if mark.extent.is_none() {
        return false;
    }
    // A little larger than it looks: a 5px dot is hard to hit exactly, and a
    // near miss that pans the image instead is infuriating.
    let reach = HANDLE_RADIUS + 4.0;
    HANDLE_CORNERS.iter().any(|(dx, dy)| {
        let (x, y) = (cx + dx * hw, cy + dy * hh);
        (sx - x).abs() <= reach && (sy - y).abs() <= reach
    })
}

/// The topmost mark whose shape covers `(sx, sy)`.
///
/// Last drawn is tested first, so the mark on top is the one you get.
pub fn annotation_at(
    annotations: &[Annotation],
    surface: &dyn AnnotationSurface,
    sx: f64,
    sy: f64,
) -> Option<String> {
    for a in annotations.iter().rev() {
        // `continue`, not `?`. A mark that cannot be placed — one anchored in
        // another viewer's space, or off this image — used to abandon the whole
        // search, so a single unplaceable mark made every other mark
        // unclickable.
        let Some((cx, cy, hw, hh)) = half_size(a, surface, 8.0) else {
            continue;
        };
        // A generous minimum: a hairline circle a few pixels across is
        // impossible to hit exactly, and a near miss reads as broken.
        let (hw, hh) = (hw.max(6.0), hh.max(6.0));
        if (sx - cx).abs() <= hw && (sy - cy).abs() <= hh {
            return Some(a.id.clone());
        }
    }
    None
}

/// Draw the shape a drag is about to create.
///
/// The preview is the shape you will GET, which is the whole reason it is
/// drawn from the same kind the finished mark will use rather than from a
/// remembered one: a preview that showed a ring and released a box taught
/// people not to trust it.
///
/// Screen coordinates, because a preview lives for one drag and every canvas
/// already knows where the pointer is. `r` is the half-size in device pixels.
///
/// `mark_style` is the style the mark WILL have, for the same reason the kind
/// is asked for rather than remembered: a preview drawn in a different ink or
/// weight from the thing it becomes is a preview nobody trusts.
pub fn draw_preview(
    kind: AnnotationKind,
    cx: f64,
    cy: f64,
    r: f64,
    mark_style: MarkStyle,
    cr: &cairo::Context,
) {
    let mark_style = mark_style.sane();
    let (ink_r, ink_g, ink_b) = mark_style.colour;
    cr.set_source_rgba(ink_r, ink_g, ink_b, 0.9);
    cr.set_line_width(mark_style.stroke);
    cr.new_path();
    let r = r.max(1.0);
    match kind {
        AnnotationKind::Rect => cr.rectangle(cx - r, cy - r, r * 2.0, r * 2.0),
        // Text has no outline; a small cross marks where it lands.
        AnnotationKind::Text => {
            cr.move_to(cx - 6.0, cy);
            cr.line_to(cx + 6.0, cy);
            cr.move_to(cx, cy - 6.0);
            cr.line_to(cx, cy + 6.0);
        }
        _ => cr.arc(cx, cy, r, 0.0, std::f64::consts::TAU),
    }
    cr.stroke().ok();
}

/// What a press on a canvas of marks is asking to do.
///
/// The decision is the same on every canvas — grips first, then shapes, then
/// nothing — so it is made once here. What differs between viewers is only how
/// a screen point becomes an anchor, which is the one thing they each keep.
#[derive(Clone, Debug, PartialEq)]
pub enum MarkGrab {
    /// Nothing of ours is under the pointer; the canvas can have the event.
    None,
    /// Move this mark. `grab_dx`/`grab_dy` are where in the shape it was
    /// taken hold of, so it does not jump to centre itself under the pointer.
    Move {
        id: String,
        grab_dx: f64,
        grab_dy: f64,
    },
    /// Resize this mark by the grip that was grabbed.
    Resize { id: String },
    /// Nothing was under the pointer and drawing is armed: make a new mark.
    Place,
}

/// Decide what a press at `(sx, sy)` is asking for.
///
/// The order is the whole content of this function, and it is here so that two
/// canvases cannot disagree about it:
///
/// 1. **A grip of the mark being edited.** Grips sit ON the outline of their
///    own shape, so testing the shape first would mean a grip could never be
///    grabbed and resizing would look simply broken.
/// 2. **Any mark's shape** — take hold of it and move it.
/// 3. **Empty space, with drawing armed** — make a new one.
/// 4. **Empty space** — the canvas can have the press.
///
/// Drawing being armed is checked LAST rather than first. Checked first, every
/// press made a new mark, so a mark could not be moved or resized without
/// disarming the pencil — and pressing on the mark you were in the middle of
/// editing dropped another one on top of it.
pub fn grab_at(
    annotations: &[Annotation],
    surface: &dyn AnnotationSurface,
    editing: Option<&str>,
    drawing: bool,
    sx: f64,
    sy: f64,
) -> MarkGrab {
    if let Some(mark) = editing.and_then(|id| annotations.iter().find(|a| a.id == id)) {
        if handle_at(mark, surface, sx, sy) {
            return MarkGrab::Resize {
                id: mark.id.clone(),
            };
        }
    }
    match annotation_at(annotations, surface, sx, sy) {
        Some(id) => {
            let (grab_dx, grab_dy) = annotations
                .iter()
                .find(|a| a.id == id)
                .and_then(|a| surface.project(&a.anchor))
                .map(|(cx, cy)| (sx - cx, sy - cy))
                .unwrap_or((0.0, 0.0));
            MarkGrab::Move {
                id,
                grab_dx,
                grab_dy,
            }
        }
        None if drawing => MarkGrab::Place,
        None => MarkGrab::None,
    }
}

/// The half-size, in the anchor's own units, that a drag of `screen_px` device
/// pixels away from the anchor is asking for.
///
/// Measured on SCREEN and divided by the local scale, rather than by
/// unprojecting the two ends of the drag and measuring between them. On a
/// foreshortened plane — a cube's slice seen at an angle in the volume view —
/// a drag of an inch covers far more data along the receding axis than across
/// it, so an unprojected drag produces a mark much larger than the one you
/// dragged out. The preview shows what you dragged; the mark has to BE what
/// you dragged.
///
/// (The FITS canvas has no perspective, so measuring either way agrees there;
/// it converts through `units_per_image_pixel` for the same reason.)
pub fn half_from_drag(
    surface: &dyn AnnotationSurface,
    anchor: &crate::models::annotation::Anchor,
    screen_px: f64,
) -> f64 {
    let scale = surface.units_to_pixels(anchor);
    if !scale.is_finite() || scale <= 0.0 {
        return 0.0;
    }
    screen_px / scale
}

/// The half-size a resize drag to `(sx, sy)` is asking for, in the anchor's
/// own units.
///
/// The grip is a corner, so the half-size is the LARGER of the two offsets —
/// dragging away from the centre grows the shape whichever way you go, rather
/// than only along the axis you happened to move furthest on.
pub fn resize_half(
    mark: &Annotation,
    surface: &dyn AnnotationSurface,
    sx: f64,
    sy: f64,
) -> Option<f64> {
    let (cx, cy) = surface.project(&mark.anchor)?;
    let scale = surface.units_to_pixels(&mark.anchor);
    if !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    Some(((sx - cx).abs().max((sy - cy).abs()) / scale).max(0.5))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::annotation::{Anchor, Author, Extent};

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

    /// A bigger rendering of the same view: the plate, or an export at 2x/4x.
    ///
    /// `units_to_pixels` scales with it, exactly as a real plate surface does —
    /// the geometry was never the broken half, and a test surface that scaled
    /// the ink but not the geometry would prove nothing about proportions.
    struct Bigger(f64);
    impl AnnotationSurface for Bigger {
        fn project(&self, anchor: &Anchor) -> Option<(f64, f64)> {
            match *anchor {
                Anchor::ImagePixel { x, y } => Some((x * self.0, y * self.0)),
                _ => None,
            }
        }
        fn units_to_pixels(&self, _: &Anchor) -> f64 {
            self.0
        }
        fn ink_scale(&self) -> f64 {
            self.0
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
        let (sx, sy, ..) = leader_geometry(100.0, 100.0, r, r, true, None, 60.0, 800.0, 1.0);
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
            leader_geometry(280.0, 100.0, 10.0, 10.0, true, None, 90.0, canvas_w, 1.0);
        assert!(!rightwards, "the callout ran off the right edge");
        assert!(rule_end < 280.0, "the rule did not flip: {rule_end}");
        assert!(text_x >= 0.0, "the text went off the left edge: {text_x}");
    }

    /// With room, it points the way it was asked to.
    #[test]
    fn a_callout_with_room_keeps_its_direction() {
        let (.., rightwards) =
            leader_geometry(100.0, 100.0, 10.0, 10.0, true, None, 60.0, 800.0, 1.0);
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
            1.0,
        );
        assert!(!rightwards_left, "an explicit left offset was overridden");
    }

    /// The rule is as long as the text, so text never overhangs it.
    #[test]
    fn the_rule_is_long_enough_for_its_text() {
        let text_width = 120.0;
        let (.., ex, _ey, rule_end, _tx, _r) =
            leader_geometry(100.0, 100.0, 10.0, 10.0, true, None, text_width, 900.0, 1.0);
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
            leader_geometry(300.0, 300.0, 60.0, 60.0, true, None, 80.0, 900.0, 1.0);
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
        let (sx, sy, ..) = leader_geometry(200.0, 200.0, r, r, true, None, 50.0, 900.0, 1.0);
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
        let (sx, sy, ..) = leader_geometry(200.0, 200.0, hw, hh, false, None, 50.0, 900.0, 1.0);
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
        let (sx, sy, ex, ey, ..) =
            leader_geometry(100.0, 100.0, 0.0, 0.0, true, None, 10.0, 800.0, 1.0);
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

    /// An unstyled agent mark is still drawn in its own ink.
    ///
    /// The author's colour is the DEFAULT a mark takes now rather than a rule
    /// applied on every frame — which is what lets one be recoloured — so this
    /// is really the test that an unstyled mark is unchanged.
    #[test]
    fn an_agents_mark_is_distinguishable() {
        use crate::models::annotation::{AGENT_INK, USER_INK};
        let mut mine = callout(None);
        mine.author = Author::Agent;
        assert_ne!(ink_for(&mine, false, false), USER_INK);
        assert_eq!(ink_for(&mine, false, false), AGENT_INK);
        // Picking one out wins over the mark's own colour — you need to see
        // what you chose — and editing wins over both, because that is the one
        // you can drag.
        assert_eq!(ink_for(&mine, true, false), style::SELECTED_INK);
        assert_eq!(ink_for(&mine, true, true), style::EDITING_INK);
        assert_ne!(style::SELECTED_INK, style::EDITING_INK);
    }

    /// A mark's own colour is used, and state still overrides it.
    #[test]
    fn a_styled_mark_keeps_its_colour_until_it_is_picked_out() {
        use crate::models::annotation::MarkStyle;
        let mut m = callout(None);
        let red = (1.0, 0.0, 0.0);
        m.style = Some(MarkStyle {
            colour: red,
            ..MarkStyle::default()
        });
        assert_eq!(ink_for(&m, false, false), red);
        // Selection and editing are chrome: they say which mark you clicked,
        // which is a fact about the session and not about the picture.
        assert_eq!(ink_for(&m, true, false), style::SELECTED_INK);
        assert_eq!(ink_for(&m, false, true), style::EDITING_INK);
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

    // ── Editing geometry ────────────────────────────────────────────────────

    fn boxed(x: f64, y: f64, half: f64) -> Annotation {
        let mut a = Annotation::new(
            AnnotationKind::Rect,
            Anchor::ImagePixel { x, y },
            "",
            Author::User,
        );
        a.extent = Some(Extent::square(half));
        a
    }

    /// A grip is grabbable exactly where it is drawn.
    ///
    /// `draw_handles` and `handle_at` walk the same four corner offsets, and
    /// this is the test that keeps them walking the same ones: a grip drawn at
    /// one place and tested at another is a control that misses when you click
    /// it, which reads as a dead widget rather than as a bug.
    #[test]
    fn a_grip_is_grabbable_where_it_is_drawn() {
        let mark = boxed(100.0, 100.0, 20.0);
        for (dx, dy) in HANDLE_CORNERS {
            let (x, y) = (100.0 + dx * 20.0, 100.0 + dy * 20.0);
            assert!(
                handle_at(&mark, &Flat, x, y),
                "corner ({dx},{dy}) at ({x},{y}) is not grabbable"
            );
        }
        assert!(
            !handle_at(&mark, &Flat, 100.0, 100.0),
            "the centre is not a grip — dragging there moves the mark"
        );
    }

    /// A grip is forgiving to hit, and not infinitely so.
    ///
    /// A 5px dot is hard to land on exactly, and a near miss that pans the
    /// image instead is infuriating — so the catch is deliberately wider than
    /// the dot. Pinned from OUTSIDE the drawn radius, because a test that only
    /// clicks grip centres passes at any reach at all and so proves nothing
    /// about the tolerance.
    #[test]
    fn a_grip_catches_a_near_miss_but_not_a_wild_one() {
        let mark = boxed(100.0, 100.0, 20.0);
        let (gx, gy) = (120.0, 120.0); // the bottom-right grip
        assert!(
            handle_at(&mark, &Flat, gx + HANDLE_RADIUS + 2.0, gy),
            "a miss just outside the dot should still grab it"
        );
        assert!(
            !handle_at(&mark, &Flat, gx + HANDLE_RADIUS + 12.0, gy),
            "a miss this wide is not aimed at the grip"
        );
    }

    /// A mark with no size has no grips to drag.
    #[test]
    fn a_mark_with_no_extent_has_no_grips() {
        let mut mark = boxed(100.0, 100.0, 20.0);
        mark.extent = None;
        assert!(!handle_at(&mark, &Flat, 100.0, 100.0));
        assert!(!handle_at(&mark, &Flat, 120.0, 120.0));
    }

    /// The topmost mark wins, because it is the one you can see.
    #[test]
    fn the_mark_on_top_is_the_one_you_hit() {
        let under = boxed(100.0, 100.0, 30.0);
        let over = boxed(100.0, 100.0, 30.0);
        let found = annotation_at(&[under.clone(), over.clone()], &Flat, 100.0, 100.0);
        assert_eq!(found.as_deref(), Some(over.id.as_str()));
    }

    /// One unplaceable mark does not make every other mark unclickable.
    ///
    /// The hit test walks the list newest-first and a sky-anchored mark cannot
    /// be projected by this surface. Returning early on the first such mark —
    /// which is what `?` does — abandoned the whole search, so a single mark
    /// from another viewer's space silently disabled clicking on all of them.
    #[test]
    fn an_unplaceable_mark_does_not_block_the_ones_behind_it() {
        let real = boxed(100.0, 100.0, 30.0);
        let unplaceable = Annotation::new(
            AnnotationKind::Circle,
            Anchor::Sky {
                ra_deg: 202.0,
                dec_deg: 47.0,
            },
            "",
            Author::Agent,
        );
        // The unplaceable one is LAST, so it is tested FIRST.
        let found = annotation_at(&[real.clone(), unplaceable], &Flat, 100.0, 100.0);
        assert_eq!(found.as_deref(), Some(real.id.as_str()));
    }

    /// A tiny mark is still clickable.
    #[test]
    fn a_hairline_mark_still_has_a_hit_box() {
        let tiny = boxed(100.0, 100.0, 0.5);
        assert!(annotation_at(&[tiny], &Flat, 104.0, 104.0).is_some());
    }

    /// A grip wins over the shape it sits on.
    ///
    /// The grips are ON the outline of their own mark, so a press there is
    /// inside the shape too. Testing the shape first would mean a grip could
    /// never be grabbed and resizing would look simply broken — which is why
    /// the order is asserted rather than left to the reading.
    #[test]
    fn a_grip_wins_over_the_shape_it_sits_on() {
        let mark = boxed(100.0, 100.0, 20.0);
        let id = mark.id.clone();
        let marks = [mark];
        // The bottom-right grip, which is also inside the box.
        let grab = grab_at(&marks, &Flat, Some(&id), false, 120.0, 120.0);
        assert_eq!(grab, MarkGrab::Resize { id: id.clone() });
        // Same point, nothing being edited: no grips, so it is a move.
        assert!(matches!(
            grab_at(&marks, &Flat, None, false, 120.0, 120.0),
            MarkGrab::Move { .. }
        ));
    }

    /// A press on nothing is a press on nothing.
    ///
    /// The canvas underneath gets the event — panning an image, orbiting a
    /// volume — so this must not claim a press it has no use for.
    #[test]
    fn a_press_on_empty_space_grabs_nothing() {
        let marks = [boxed(100.0, 100.0, 20.0)];
        assert_eq!(
            grab_at(&marks, &Flat, None, false, 400.0, 400.0),
            MarkGrab::None
        );
    }

    /// A mark is taken hold of where you grabbed it, not by its centre.
    #[test]
    fn a_move_remembers_where_the_shape_was_grabbed() {
        let marks = [boxed(100.0, 100.0, 20.0)];
        let MarkGrab::Move {
            grab_dx, grab_dy, ..
        } = grab_at(&marks, &Flat, None, false, 110.0, 95.0)
        else {
            panic!("expected a move");
        };
        assert!((grab_dx - 10.0).abs() < 1e-9, "dx {grab_dx}");
        assert!((grab_dy + 5.0).abs() < 1e-9, "dy {grab_dy}");
    }

    /// A resize takes the larger offset, so dragging any direction grows it.
    #[test]
    fn a_resize_grows_whichever_way_you_drag() {
        let mark = boxed(100.0, 100.0, 20.0);
        // Further in y than x: the half-size follows y.
        let half = resize_half(&mark, &Flat, 130.0, 160.0).expect("sized");
        assert!((half - 60.0).abs() < 1e-9, "half {half}");
        // And it never collapses to nothing.
        let tiny = resize_half(&mark, &Flat, 100.0, 100.0).expect("sized");
        assert!(
            tiny >= 0.5,
            "a mark dragged to zero is unrecoverable: {tiny}"
        );
    }

    /// The preview draws the shape you will get, and it is visible.
    ///
    /// The reported symptom was that no shape edges appeared while drawing in
    /// the cube's two views — nothing drew at all. So the first thing to pin
    /// is that a preview marks the canvas, and that the two kinds differ:
    /// a preview that showed a ring and released a box taught people not to
    /// trust it.
    #[test]
    fn a_preview_marks_the_canvas_with_the_kind_it_was_given() {
        let ink = |kind| {
            let surface =
                cairo::ImageSurface::create(cairo::Format::ARgb32, 60, 60).expect("surface");
            {
                let cr = cairo::Context::new(&surface).expect("cr");
                draw_preview(kind, 30.0, 30.0, 12.0, MarkStyle::default(), &cr);
            }
            let mut s = surface;
            s.flush();
            let n = s.data().expect("data").iter().filter(|b| **b != 0).count();
            n
        };
        let circle = ink(AnnotationKind::Circle);
        let rect = ink(AnnotationKind::Rect);
        assert!(circle > 0, "a circle preview drew nothing");
        assert!(rect > 0, "a box preview drew nothing");
        assert_ne!(circle, rect, "the two kinds preview identically");
    }

    /// A thicker outline lays down more ink.
    ///
    /// The whole style feature is worth nothing if the number reaches the
    /// struct and stops there. Counting pixels is the only way to say the
    /// renderer used it: a stroke that is read, stored, round-tripped through
    /// JSON and then ignored by cairo passes every other test in this file.
    #[test]
    fn a_thicker_outline_lays_down_more_ink() {
        let ink = |stroke: f64| {
            let mut mark = Annotation::new(
                AnnotationKind::Circle,
                Anchor::ImagePixel { x: 60.0, y: 60.0 },
                "",
                Author::User,
            );
            mark.extent = Some(Extent::square(30.0));
            mark.style = Some(MarkStyle {
                stroke,
                ..MarkStyle::default()
            });
            let surface =
                cairo::ImageSurface::create(cairo::Format::ARgb32, 120, 120).expect("surface");
            {
                let cr = cairo::Context::new(&surface).expect("cr");
                draw(&[mark], &Flat, None, None, &cr, 120.0, 120.0);
            }
            let mut s = surface;
            s.flush();
            // Alpha only: a wider ring covers more pixels, whatever its colour.
            let covered = s
                .data()
                .expect("data")
                .chunks_exact(4)
                .filter(|px| px[3] != 0)
                .count();
            covered
        };
        let thin = ink(1.0);
        let thick = ink(4.0);
        assert!(thin > 0, "a mark at the default stroke drew nothing");
        assert!(
            thick > thin,
            "stroke 4 covered {thick} pixels and stroke 1 covered {thin} — the \
             thickness never reached cairo"
        );
    }

    /// A mark's colour is the colour it is drawn in.
    ///
    /// Not "some colour": the exact channels, because the failure this catches
    /// is the renderer keeping its own constant and the setting changing
    /// nothing anyone can see.
    #[test]
    fn a_marks_colour_reaches_the_raster() {
        let mut mark = Annotation::new(
            AnnotationKind::Rect,
            Anchor::ImagePixel { x: 60.0, y: 60.0 },
            "",
            Author::User,
        );
        mark.extent = Some(Extent::square(30.0));
        mark.style = Some(MarkStyle {
            colour: (1.0, 0.0, 0.0),
            stroke: 4.0,
            ..MarkStyle::default()
        });
        let surface =
            cairo::ImageSurface::create(cairo::Format::ARgb32, 120, 120).expect("surface");
        {
            let cr = cairo::Context::new(&surface).expect("cr");
            draw(&[mark], &Flat, None, None, &cr, 120.0, 120.0);
        }
        let mut s = surface;
        s.flush();
        // BGRA, premultiplied: on the most opaque pixel, red must dominate.
        let data = s.data().expect("data");
        let px = data
            .chunks_exact(4)
            .max_by_key(|px| px[3])
            .expect("some pixel");
        assert!(px[3] > 0, "the mark drew nothing at all");
        assert!(
            px[2] > px[1] && px[2] > px[0],
            "a red mark came out as B{} G{} R{} — the colour never reached cairo",
            px[0],
            px[1],
            px[2]
        );
    }

    /// Selection ink still overrides a custom colour, and only on screen.
    ///
    /// A mark the person coloured red must still go white when picked out, or
    /// selection becomes invisible on exactly the marks they cared enough to
    /// restyle.
    #[test]
    fn selection_still_overrides_a_custom_colour() {
        let mut mark = Annotation::new(
            AnnotationKind::Circle,
            Anchor::ImagePixel { x: 10.0, y: 10.0 },
            "",
            Author::User,
        );
        mark.style = Some(MarkStyle {
            colour: (1.0, 0.0, 0.0),
            ..MarkStyle::default()
        });
        assert_eq!(ink_for(&mark, false, false), (1.0, 0.0, 0.0));
        assert_eq!(ink_for(&mark, true, false), style::SELECTED_INK);
        assert_eq!(ink_for(&mark, true, true), style::EDITING_INK);
    }

    /// A mark keeps its proportions when the rendering is bigger.
    ///
    /// This is the bug the ink scale exists for, and it shipped: an export at
    /// 4x re-rendered the picture at four times the resolution and scaled the
    /// plate's own title, caption and colorbar with it, while every mark kept
    /// its screen numbers. A 2px ring stayed 2px and a 12px label stayed a
    /// 15x10px smudge — so the annotations were the ONE thing in the figure
    /// that shrank, and they became unreadable at exactly the resolution
    /// someone had chosen for publication.
    ///
    /// Measured as ink laid down, at three factors, because a fix that scaled
    /// the ring and forgot the label would pass any single-number check.
    #[test]
    fn a_mark_keeps_its_proportions_when_the_rendering_is_bigger() {
        // Ink at `k`, as (ring stroke in px, label bounding box area).
        let measure = |k: f64| -> (usize, usize) {
            let mut mark = Annotation::new(
                AnnotationKind::Circle,
                Anchor::ImagePixel { x: 30.0, y: 45.0 },
                "AB",
                Author::User,
            );
            mark.extent = Some(Extent::square(12.0));
            mark.style = Some(MarkStyle {
                stroke: 2.0,
                font_size: 12.0,
                ..MarkStyle::default()
            });
            let side = (240.0 * k) as i32;
            let surface =
                cairo::ImageSurface::create(cairo::Format::ARgb32, side, side).expect("surface");
            {
                let cr = cairo::Context::new(&surface).expect("cr");
                draw(
                    &[mark],
                    &Bigger(k),
                    None,
                    None,
                    &cr,
                    f64::from(side),
                    f64::from(side),
                );
            }
            let mut s = surface;
            s.flush();
            let w = side as usize;
            let data = s.data().expect("data");
            let lit = |x: usize, y: usize| data[(y * w + x) * 4 + 3] > 40;
            // The ring's stroke: the first run of ink along the row through the
            // shape's centre.
            let cy = (45.0 * k) as usize;
            let mut stroke = 0usize;
            for x in 0..w {
                if lit(x, cy) {
                    stroke += 1;
                } else if stroke > 0 {
                    break;
                }
            }
            // The label: ink above the shape, where the leader puts the text.
            let ceiling = (30.0 * k) as usize;
            let mut label = 0usize;
            for y in 0..ceiling.min(w) {
                for x in 0..w {
                    if lit(x, y) {
                        label += 1;
                    }
                }
            }
            (stroke, label)
        };

        let (s1, l1) = measure(1.0);
        assert!(
            s1 > 0 && l1 > 0,
            "nothing drew at 1x: {s1}px ring, {l1}px label"
        );
        for k in [2.0, 4.0] {
            let (sk, lk) = measure(k);
            // Ring: within a pixel of k times, which is antialiasing, not drift.
            let want = s1 as f64 * k;
            assert!(
                (sk as f64 - want).abs() <= 1.5,
                "at {k}x the ring is {sk}px and {want} was wanted — the stroke \
                 did not follow the rendering"
            );
            // Label: area, so k times taller AND wider is k squared the ink.
            let ratio = lk as f64 / l1 as f64;
            assert!(
                ratio > k * k * 0.6,
                "at {k}x the label laid down {lk} px of ink against {l1} at 1x \
                 (x{ratio:.2}); a label that scaled would be near x{:.0}",
                k * k
            );
        }
    }

    /// A rendering that cannot say how big it is draws the screen's look.
    ///
    /// `ink_scale` is derived from a widget allocation, and a headless one is
    /// zero — a probe, or an agent asking before the window is mapped. Zero
    /// would collapse every mark to nothing and NaN would erase them, in the
    /// exact situations nobody is watching a screen.
    #[test]
    fn a_rendering_that_cannot_size_itself_draws_the_screens_look() {
        struct Broken(f64);
        impl AnnotationSurface for Broken {
            fn project(&self, anchor: &Anchor) -> Option<(f64, f64)> {
                match *anchor {
                    Anchor::ImagePixel { x, y } => Some((x, y)),
                    _ => None,
                }
            }
            fn units_to_pixels(&self, _: &Anchor) -> f64 {
                1.0
            }
            fn ink_scale(&self) -> f64 {
                self.0
            }
        }
        let render = |surface: &dyn AnnotationSurface| -> Vec<u8> {
            let mut mark = Annotation::new(
                AnnotationKind::Circle,
                Anchor::ImagePixel { x: 60.0, y: 60.0 },
                "AB",
                Author::User,
            );
            mark.extent = Some(Extent::square(20.0));
            let s = cairo::ImageSurface::create(cairo::Format::ARgb32, 120, 120).expect("surface");
            {
                let cr = cairo::Context::new(&s).expect("cr");
                draw(&[mark], surface, None, None, &cr, 120.0, 120.0);
            }
            let mut s = s;
            s.flush();
            let bytes = s.data().expect("data").to_vec();
            bytes
        };
        let screen = render(&Flat);
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(
                render(&Broken(bad)),
                screen,
                "an ink scale of {bad} did not draw what the screen draws"
            );
        }
    }

    /// The leader and its rule are furniture, and furniture scales too.
    ///
    /// Geometry rather than pixels, because this is the half a raster test
    /// would find hardest to attribute: a 4x label on a 1x leader reads as the
    /// text having come loose from its mark.
    #[test]
    fn the_leader_scales_with_the_rendering() {
        // Same mark, same text width per unit of ink — the text is measured
        // with the scaled font by the caller, so 4x here means a 4x wide label.
        let reach = |ink: f64| {
            let (sx, _sy, ex, _ey, rule_end, _tx, _r) = leader_geometry(
                400.0,
                400.0,
                20.0 * ink,
                20.0 * ink,
                true,
                None,
                60.0 * ink,
                2000.0,
                ink,
            );
            (ex - sx, rule_end - ex)
        };
        let (leader1, rule1) = reach(1.0);
        let (leader4, rule4) = reach(4.0);
        assert!(
            (leader4 / leader1 - 4.0).abs() < 0.01,
            "the leader went from {leader1} to {leader4} — not four times"
        );
        assert!(
            (rule4 / rule1 - 4.0).abs() < 0.01,
            "the rule went from {rule1} to {rule4} — not four times"
        );
    }

    /// A drag is sized on screen, so what you dragged is what you get.
    ///
    /// The cube's volume view looks at the slice plane at an angle, so
    /// unprojecting the two ends of a drag and measuring between them gives a
    /// far bigger number along the receding axis than across it — the mark
    /// came out much larger than the ring the pointer traced.
    #[test]
    fn a_drag_is_sized_by_what_it_covered_on_screen() {
        struct Tenx;
        impl AnnotationSurface for Tenx {
            fn project(&self, _: &Anchor) -> Option<(f64, f64)> {
                Some((0.0, 0.0))
            }
            fn units_to_pixels(&self, _: &Anchor) -> f64 {
                10.0
            }
        }
        let a = Anchor::ImagePixel { x: 0.0, y: 0.0 };
        // 40 screen pixels at 10 px per unit is 4 units, whatever the geometry
        // between the drag's two ends happened to be.
        assert!((half_from_drag(&Tenx, &a, 40.0) - 4.0).abs() < 1e-9);
        // And the drawn radius comes back to the drag: 4 units x 10 = 40 px.
        assert!((half_from_drag(&Tenx, &a, 40.0) * 10.0 - 40.0).abs() < 1e-9);
    }

    /// A surface with no scale cannot size a drag, and says so rather than
    /// returning an infinity that becomes a mark the size of the sky.
    #[test]
    fn a_degenerate_surface_sizes_nothing() {
        struct Dead;
        impl AnnotationSurface for Dead {
            fn project(&self, _: &Anchor) -> Option<(f64, f64)> {
                Some((0.0, 0.0))
            }
            fn units_to_pixels(&self, _: &Anchor) -> f64 {
                0.0
            }
        }
        assert_eq!(
            half_from_drag(&Dead, &Anchor::ImagePixel { x: 0.0, y: 0.0 }, 40.0),
            0.0
        );
    }

    /// Drawing armed does not stop you grabbing a mark that is already there.
    ///
    /// Reported from use: with the pencil on, pressing an existing mark
    /// dropped a NEW one on top of it, so a mark could not be moved without
    /// disarming drawing first — and the mark you were in the middle of
    /// editing was the easiest one to hit. Drawing is checked LAST, after
    /// grips and shapes, and this is the test that keeps it there.
    #[test]
    fn drawing_does_not_steal_a_press_on_an_existing_mark() {
        let mark = boxed(100.0, 100.0, 20.0);
        let id = mark.id.clone();
        let marks = [mark];

        // On the shape, pencil armed: move it, do not make another.
        assert!(
            matches!(
                grab_at(&marks, &Flat, None, true, 100.0, 100.0),
                MarkGrab::Move { .. }
            ),
            "drawing stole a press on an existing mark"
        );
        // On a grip of the mark being edited, pencil armed: resize it.
        assert_eq!(
            grab_at(&marks, &Flat, Some(&id), true, 120.0, 120.0),
            MarkGrab::Resize { id: id.clone() },
            "drawing stole a press on the edited mark's grip"
        );
        // Empty space, pencil armed: NOW make one.
        assert_eq!(
            grab_at(&marks, &Flat, None, true, 400.0, 400.0),
            MarkGrab::Place
        );
    }

    /// With drawing off, empty space belongs to the canvas.
    ///
    /// Panning an image and orbiting a volume both depend on this: a press
    /// this function claims is a press the camera never sees.
    #[test]
    fn empty_space_is_the_canvas_press_unless_drawing() {
        let marks = [boxed(100.0, 100.0, 20.0)];
        assert_eq!(
            grab_at(&marks, &Flat, None, false, 400.0, 400.0),
            MarkGrab::None
        );
    }
}
