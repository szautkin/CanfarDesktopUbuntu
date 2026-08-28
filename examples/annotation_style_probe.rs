//! What the annotations actually look like.
//!
//!     cargo run --example annotation_style_probe
//!
//! Writes a PNG of every kind, including callouts at the four corners where the
//! leader has to flip. The geometry rules are unit-tested; whether the result
//! reads as blueprint schematics is not a thing a test can answer, and the
//! flipping in particular has to be looked at once.
use verbinal::helpers::annotation_render::{draw, AnnotationSurface};
use verbinal::models::annotation::{Anchor, Annotation, AnnotationKind, Author, Extent};

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

fn main() {
    let (w, h) = (900, 560);
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
    ];
    let selected = anns[1].id.clone();
    draw(&anns, &Flat, Some(&selected), &cr, w as f64, h as f64);
    drop(cr);

    let out = std::env::temp_dir().join("annotation_style_probe.png");
    let mut file = std::fs::File::create(&out).expect("create");
    surface.write_to_png(&mut file).expect("write");
    println!("wrote {}", out.display());
    println!("look for: leaders leaving the EDGE, all at the same angle, every");
    println!("callout's rule and text on the canvas, the circle selected in amber,");
    println!("and the agent's two marks in their own green.");
}
