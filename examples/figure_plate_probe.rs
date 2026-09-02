//! Does the publication plate still lay out the way it did?
//!
//!     cargo run --example figure_plate_probe [out.png]
//!
//! The plate is 300 lines of hand-placed typography that no unit test can see,
//! and it was extracted from the cube viewer so the FITS viewer could share it.
//! A refactor of working layout with nothing underneath it is unfalsifiable, so
//! this composes a known plate and checks the parts are where they belong:
//! a header band, a framed picture, a caption, a colour ramp and a footer.
//!
//! It writes the PNG too. Look at it once — a plate can be plausible and wrong.
use std::rc::Rc;
use verbinal::ui::figure_plate::{FramePainter, PlateContent};

fn main() {
    gtk4::init().expect("gtk init");
    let mut failures = 0;

    // A picture that is pure red, so it is trivially distinguishable from the
    // plate's dark furniture.
    let capture: Rc<dyn Fn(i32, i32) -> Option<Vec<u8>>> = Rc::new(|w, h| {
        let mut v = vec![0u8; (w * h * 4) as usize];
        for px in v.as_chunks_mut::<4>().0 {
            *px = [220, 30, 30, 255];
        }
        Some(v)
    });

    // A painter that fills the frame's top-left corner green, to prove the
    // overlay is called and positioned in frame coordinates.
    let painter: FramePainter = Rc::new(|cr, fx, fy, fw, fh| {
        cr.set_source_rgb(0.0, 0.9, 0.0);
        cr.rectangle(fx, fy, fw * 0.1, fh * 0.1);
        let _ = cr.fill();
    });

    let content = PlateContent {
        capture,
        title: "Probe Plate".into(),
        subtitle: "a subtitle".into(),
        caption: "a caption line".into(),
        colormap: "Inferno".into(),
        // A ramp the probe supplies itself, which is the point of the plate not
        // owning one: red rising to white.
        ramp: {
            let mut r = [(0u8, 0u8, 0u8); 256];
            for (i, e) in r.iter_mut().enumerate() {
                *e = (255, i as u8, i as u8);
            }
            r
        },
        lo_label: "1.0e-3".into(),
        hi_label: "0.5".into(),
        date: "2026-01-01".into(),
        footer: vec![
            ("DIMENSIONS".into(), "60x40".into()),
            ("RA".into(), "10h 00m .. 10h 05m".into()),
        ],
        overlay: Some(painter),
    };

    let Some(mut surf) = content.compose(1, false) else {
        println!("  !! the plate composed nothing at all");
        std::process::exit(1);
    };
    let (w, h) = (surf.width(), surf.height());
    let (_, _, rgba) = verbinal::helpers::image_bytes::surface_to_rgba(&mut surf);
    let at = |x: i32, y: i32| {
        let i = ((y * w + x) * 4) as usize;
        (rgba[i], rgba[i + 1], rgba[i + 2])
    };
    let redish = |p: (u8, u8, u8)| p.0 > 150 && p.1 < 90 && p.2 < 90;
    let greenish = |p: (u8, u8, u8)| p.1 > 150 && p.0 < 90;
    let darkish = |p: (u8, u8, u8)| p.0 < 60 && p.1 < 60 && p.2 < 60;

    // The picture occupies the middle band; the header above and the footer
    // below are plate, not picture.
    let mid = at(w / 2, h / 2);
    if !redish(mid) {
        println!("  !! the picture is not in the middle of the plate: {mid:?}");
        failures += 1;
    }
    let header = at(w / 2, 6);
    if !darkish(header) {
        println!("  !! the header band is not plate-dark: {header:?}");
        failures += 1;
    }
    let footer = at(w / 2, h - 6);
    if !darkish(footer) {
        println!("  !! the footer band is not plate-dark: {footer:?}");
        failures += 1;
    }
    // The overlay painter ran, in frame coordinates.
    let over = at(w / 2 - 200, h / 2 - 150);
    let corner_found = (0..h)
        .step_by(4)
        .any(|y| (0..w).step_by(4).any(|x| greenish(at(x, y))));
    if !corner_found {
        println!("  !! the frame painter never ran, or painted nothing ({over:?})");
        failures += 1;
    }

    let out = std::env::args().nth(1).unwrap_or_else(|| {
        std::env::temp_dir()
            .join("figure_plate_probe.png")
            .display()
            .to_string()
    });
    let mut f = std::fs::File::create(&out).expect("create");
    surf.write_to_png(&mut f).expect("write");
    println!("plate {w}x{h}: picture centred, header and footer present, overlay ran");
    println!("wrote {out} — look at it once; a plate can be plausible and wrong");

    if failures > 0 {
        std::process::exit(1);
    }
}
