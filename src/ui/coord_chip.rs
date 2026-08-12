//! The little dark box of coordinates that follows the pointer over an image.
//!
//! Drawn with cairo, inside the same frame as the image, and that is the whole
//! point. The cube's slice view used a `GtkLabel` in an overlay moved by
//! margins on every motion event: each move re-ran a layout pass, and the
//! position was computed from a `measure()` of the PREVIOUS text — so a chip
//! whose width changes with every RA digit chased the pointer a frame behind and
//! flickered between two places at once. A chip painted in the draw function
//! cannot lag its own text, cannot thrash layout, and cannot appear twice.
//!
//! Shared by the FITS canvas and the cube slice view so the readout looks and
//! behaves the same in both.

use gtk4::cairo;

/// Padding inside the chip, and the gap between the pointer and the chip.
const PADDING: f64 = 4.0;
const POINTER_GAP: f64 = 8.0;
const FONT_SIZE: f64 = 11.0;
/// Line spacing for a multi-line chip, as a multiple of the font size.
const LINE_STEP: f64 = 1.25;

/// Draw `lines` in a translucent box anchored near `(x, y)`, kept inside a
/// `width` × `height` viewport.
///
/// The anchor is a suggestion: a chip near the right or bottom edge slides back
/// inside rather than being clipped, because a coordinate readout that runs off
/// the screen is the one moment the reader needs it most.
pub fn draw(cr: &cairo::Context, x: f64, y: f64, lines: &[String], width: f64, height: f64) {
    if lines.is_empty() {
        return;
    }

    cr.select_font_face(
        "monospace",
        cairo::FontSlant::Normal,
        cairo::FontWeight::Normal,
    );
    cr.set_font_size(FONT_SIZE);

    let mut text_w: f64 = 0.0;
    for line in lines {
        if let Ok(extents) = cr.text_extents(line) {
            text_w = text_w.max(extents.width());
        }
    }
    let line_h = FONT_SIZE * LINE_STEP;
    let box_w = text_w + PADDING * 2.0;
    let box_h = line_h * lines.len() as f64 + PADDING * 2.0;

    let box_x = (x + POINTER_GAP).min(width - box_w - PADDING).max(0.0);
    let box_y = (y + POINTER_GAP).min(height - box_h - PADDING).max(0.0);

    cr.set_source_rgba(0.0, 0.0, 0.0, 0.7);
    cr.rectangle(box_x, box_y, box_w, box_h);
    let _ = cr.fill();

    cr.set_source_rgba(1.0, 1.0, 1.0, 1.0);
    for (i, line) in lines.iter().enumerate() {
        // Baseline of row i: one full line down from the box top, so the first
        // row sits inside the padding rather than on it.
        cr.move_to(
            box_x + PADDING,
            box_y + PADDING + line_h * (i as f64 + 1.0) - FONT_SIZE * 0.25,
        );
        let _ = cr.show_text(line);
    }
}

#[cfg(test)]
mod tests {
    //! The geometry is pure arithmetic, so it is testable without a surface;
    //! what it must guarantee is that the chip stays on screen.

    /// The same clamp the drawing does, factored for the test to check.
    fn clamp(anchor: f64, box_size: f64, viewport: f64) -> f64 {
        (anchor + super::POINTER_GAP)
            .min(viewport - box_size - super::PADDING)
            .max(0.0)
    }

    #[test]
    fn a_chip_near_the_edge_slides_back_inside() {
        // Pointer at the far right: the chip must not run off, because that is
        // exactly when the reader is looking at it.
        let placed = clamp(990.0, 120.0, 1000.0);
        assert!(placed + 120.0 <= 1000.0, "{placed}");
    }

    #[test]
    fn a_chip_wider_than_the_viewport_still_starts_on_screen() {
        // Degenerate, but a negative origin would put the text off the left edge
        // and there would be nothing to read at all.
        assert_eq!(clamp(10.0, 400.0, 200.0), 0.0);
    }

    #[test]
    fn a_chip_away_from_the_edges_sits_just_past_the_pointer() {
        assert_eq!(clamp(100.0, 120.0, 1000.0), 100.0 + super::POINTER_GAP);
    }
}
