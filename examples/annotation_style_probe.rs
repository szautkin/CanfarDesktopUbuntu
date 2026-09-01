//! What the annotations actually look like.
//!
//!     cargo run --example annotation_style_probe
//!
//! Writes a PNG of every kind, including callouts at the four corners where the
//! leader has to flip. The geometry rules are unit-tested; whether the result
//! reads as blueprint schematics is not a thing a test can answer, and the
//! flipping in particular has to be looked at once.
use verbinal::helpers::annotation_render::{draw, AnnotationSurface};
use verbinal::models::annotation::{Anchor, Annotation, AnnotationKind, Author, Extent, MarkStyle};

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

fn at(kind: AnnotationKind, x: f64, y: f64, text: &str, author: Author) -> Annotation {
    let mut a = Annotation::new(kind, Anchor::ImagePixel { x, y }, text, author);
    a.extent = Some(Extent::square(16.0));
    a
}

/// The same mark, given a look of its own.
fn styled(
    mut a: Annotation,
    colour: (f64, f64, f64),
    font_size: f64,
    bold: bool,
    stroke: f64,
) -> Annotation {
    a.style = Some(MarkStyle {
        colour,
        font_size,
        bold,
        stroke,
    });
    a
}

fn main() {
    let (w, h) = (900, 700);
    let surface =
        gtk4::cairo::ImageSurface::create(gtk4::cairo::Format::ARgb32, w, h).expect("surface");
    let cr = gtk4::cairo::Context::new(&surface).expect("cr");

    // The viewers' dark ground, so the ink is judged against what it sits on.
    cr.set_source_rgb(0.10, 0.11, 0.13);
    cr.paint().ok();

    let anns = vec![
        at(
            AnnotationKind::Rect,
            140.0,
            110.0,
            "rectangle",
            Author::User,
        ),
        at(AnnotationKind::Circle, 330.0, 110.0, "circle", Author::User),
        at(
            AnnotationKind::Text,
            520.0,
            110.0,
            "bare text",
            Author::User,
        ),
        at(
            AnnotationKind::Circle,
            700.0,
            110.0,
            "agent's mark",
            Author::Agent,
        ),
        // The corners: each leader must stay on the canvas.
        at(
            AnnotationKind::Callout,
            90.0,
            300.0,
            "top-left subject",
            Author::User,
        ),
        at(
            AnnotationKind::Callout,
            820.0,
            300.0,
            "top-right subject",
            Author::User,
        ),
        at(
            AnnotationKind::Callout,
            90.0,
            500.0,
            "bottom-left subject",
            Author::User,
        ),
        at(
            AnnotationKind::Callout,
            820.0,
            500.0,
            "bottom-right subject",
            Author::User,
        ),
        at(
            AnnotationKind::Callout,
            450.0,
            380.0,
            "NGC 5194 core",
            Author::Agent,
        ),
        // A styled row, which is the question this probe now also answers:
        // whether four numbers a person picked make a figure that still reads
        // as one set of annotations rather than four unrelated scribbles.
        styled(
            at(
                AnnotationKind::Circle,
                140.0,
                610.0,
                "red, thick",
                Author::User,
            ),
            (1.0, 0.35, 0.35),
            11.0,
            false,
            4.0,
        ),
        styled(
            at(
                AnnotationKind::Rect,
                330.0,
                610.0,
                "big and bold",
                Author::User,
            ),
            (1.0, 0.85, 0.4),
            22.0,
            true,
            1.0,
        ),
        styled(
            at(
                AnnotationKind::Circle,
                520.0,
                610.0,
                "hairline",
                Author::User,
            ),
            (0.7, 0.9, 1.0),
            8.0,
            false,
            0.5,
        ),
        styled(
            at(
                AnnotationKind::Rect,
                700.0,
                610.0,
                "heavy green",
                Author::Agent,
            ),
            (0.4, 1.0, 0.6),
            16.0,
            true,
            3.0,
        ),
    ];
    // One mark merely selected, another being edited: the probe is where the
    // two inks are compared side by side.
    let selected = anns[1].id.clone();
    let editing = anns[3].id.clone();
    draw(
        &anns,
        &Flat,
        Some(&selected),
        Some(&editing),
        &cr,
        w as f64,
        h as f64,
    );
    drop(cr);

    let out = std::env::temp_dir().join("annotation_style_probe.png");
    let mut file = std::fs::File::create(&out).expect("create");
    surface.write_to_png(&mut file).expect("write");
    println!("wrote {}", out.display());
    println!("look for: leaders leaving the EDGE, all at the same angle, every");
    println!("callout's rule and text on the canvas, the selected circle in white,");
    println!("the edited one in amber, and the agent's marks in their own green.");
    println!("bottom row: four styled marks — colour, size, weight and thickness");
    println!("should all differ, and the labels should still sit on their rules.");
}
