//! Who owns a press on the FITS canvas?
//!
//!     cargo run --example fits_gesture_probe
//!
//! Three gestures want the left mouse button: pan, draw a mark, and select an
//! area to export. The first time two of them wanted it, the pan drag claimed
//! the sequence first and marks could not be placed at all — the arbitration
//! is now one function, and this is what keeps it honest.
//!
//! It needs a real canvas because the answer depends on whether a mark is under
//! the pointer, so it is a probe rather than a unit test.
use verbinal::models::annotation::{Anchor, Annotation, AnnotationKind, Author, Extent};
use verbinal::ui::fits_canvas::FitsCanvas;

fn main() {
    gtk4::init().expect("gtk init");
    let mut failures = 0;

    let (iw, ih) = (200usize, 200usize);
    let c = FitsCanvas::new(
        iw,
        ih,
        vec![0u8; iw * ih * 4],
        std::rc::Rc::new(std::cell::RefCell::new(Default::default())),
        None,
    );
    c.cancel_fit();
    let mut m = Annotation::new(
        AnnotationKind::Circle,
        Anchor::ImagePixel { x: 60.0, y: 60.0 },
        "",
        Author::User,
    );
    m.extent = Some(Extent::square(20.0));
    c.set_annotations(vec![m]);

    // (label, on a mark?, shift?, draw armed?, select armed?, expected owner)
    let cases: &[(&str, f64, f64, bool, bool, bool, &str)] = &[
        (
            "idle, empty space",
            150.0,
            150.0,
            false,
            false,
            false,
            "canvas",
        ),
        ("idle, on a mark", 60.0, 60.0, false, false, false, "canvas"),
        (
            "drawing, empty space",
            150.0,
            150.0,
            false,
            true,
            false,
            "drawing",
        ),
        // Draw stands aside on a mark, so a mark can still be picked up rather
        // than another being stacked on top of it.
        (
            "drawing, on a mark",
            60.0,
            60.0,
            false,
            true,
            false,
            "canvas",
        ),
        // Shift always means "move the image".
        (
            "drawing, shifted",
            150.0,
            150.0,
            true,
            true,
            false,
            "canvas",
        ),
        // Select-area owns everything while armed, marks included: the region
        // you want almost always starts on top of something interesting.
        (
            "selecting, empty space",
            150.0,
            150.0,
            false,
            false,
            true,
            "selecting",
        ),
        (
            "selecting, on a mark",
            60.0,
            60.0,
            false,
            false,
            true,
            "selecting",
        ),
        (
            "selecting, shifted",
            150.0,
            150.0,
            true,
            false,
            true,
            "canvas",
        ),
        ("both armed", 150.0, 150.0, false, true, true, "selecting"),
    ];

    for (label, x, y, shifted, drawing, selecting, want) in cases {
        if *drawing {
            c.set_on_left_click(|_, _, _| {});
        } else {
            c.clear_on_left_click();
        }
        c.set_selecting(*selecting);
        let got = c.press_owner_name(*x, *y, *shifted);
        if got == *want {
            println!("{label}: {got}");
        } else {
            println!("  !! {label}: {got}, expected {want}");
            failures += 1;
        }
    }
    c.set_selecting(false);
    c.clear_on_left_click();

    if failures > 0 {
        println!("{failures} check(s) failed.");
        std::process::exit(1);
    }
    println!("\nevery press has exactly one owner");
}
