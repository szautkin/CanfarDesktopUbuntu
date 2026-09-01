//! Does 1x / 2x / 4x enlarge the marks along with the picture?
//!
//!     cargo run --example export_scale_probe
//!
//! It did not, and nothing caught it: an export at 4x re-rendered the image at
//! four times the resolution and scaled the plate's own title, caption and
//! colorbar with it, while every mark kept its screen numbers — a 2px ring
//! stayed 2px and a 12px label stayed a 15x10px smudge. The annotations were
//! the one thing in the figure that shrank, and they became unreadable at
//! exactly the resolution someone had chosen for publication.
//!
//! This drives the same `compose_region` the Save button does, at all three
//! scales the picker offers, and measures the ink. A unit test cannot: the
//! export path needs a real `FitsCanvas`, and constructing one needs GTK.
//!
//! It writes the plates too. Look at them once — the numbers can agree and the
//! figure still be wrong.
use std::rc::Rc;
use verbinal::helpers::image_bytes::surface_to_rgba;
use verbinal::models::annotation::{Anchor, Annotation, AnnotationKind, Author, Extent, MarkStyle};
use verbinal::ui::figure_plate::PlateContent;
use verbinal::ui::fits_canvas::{DrawOpts, FitsCanvas, ViewRegion};

const N: usize = 240;
/// The mark's own numbers, in screen pixels.
const STROKE: f64 = 2.0;
const FONT: f64 = 12.0;

/// A checkerboard, so an upscale is visible as blocks rather than as nothing.
fn checks() -> Vec<u8> {
    let mut rgba = vec![0u8; N * N * 4];
    for y in 0..N {
        for x in 0..N {
            let i = (y * N + x) * 4;
            let v = if ((x / 8) + (y / 8)) % 2 == 0 {
                60
            } else {
                140
            };
            rgba[i] = v;
            rgba[i + 1] = v;
            rgba[i + 2] = v;
            rgba[i + 3] = 255;
        }
    }
    rgba
}

/// `visible_image` false leaves the frame fully transparent, so the only ink in
/// the export is the mark itself and a bounding box means what it says.
fn canvas(kind: AnnotationKind, text: &str, visible_image: bool) -> Rc<FitsCanvas> {
    let c = FitsCanvas::new(
        N,
        N,
        if visible_image {
            checks()
        } else {
            vec![0u8; N * N * 4]
        },
        Rc::new(std::cell::RefCell::new(Default::default())),
        None,
    );
    // scale 1, offset 0: one view pixel is one image pixel, so a 4x export is
    // exactly four times the raster and nothing else has moved.
    c.cancel_fit();
    let mut mark = Annotation::new(
        kind,
        Anchor::ImagePixel { x: 90.0, y: 150.0 },
        text,
        Author::User,
    );
    if kind.needs_extent() {
        mark.extent = Some(Extent::square(24.0));
    }
    mark.style = Some(MarkStyle {
        stroke: STROKE,
        font_size: FONT,
        ..MarkStyle::default()
    });
    c.set_annotations(vec![mark]);
    c
}

/// The ink bounding box of everything drawn on an otherwise flat export.
fn ink_box(c: &Rc<FitsCanvas>, scale: i32) -> (i32, i32) {
    let region = ViewRegion {
        x: 0.0,
        y: 0.0,
        width: N as f64,
        height: N as f64,
    };
    let mut surf =
        verbinal::ui::fits_export::compose_region(c, N as i32, N as i32, region, scale, true)
            .expect("composed");
    let (w, h, rgba) = surface_to_rgba(&mut surf);
    let (mut t, mut b, mut l, mut r) = (h, 0, w, 0);
    for y in 0..h {
        for x in 0..w {
            if rgba[((y * w + x) * 4 + 3) as usize] > 40 {
                t = t.min(y);
                b = b.max(y);
                l = l.min(x);
                r = r.max(x);
            }
        }
    }
    if b >= t {
        (r - l + 1, b - t + 1)
    } else {
        (0, 0)
    }
}

fn main() {
    gtk4::init().expect("gtk init");
    let mut failures = 0;

    // ── The label, on a canvas holding nothing but text ─────────────────────
    let text_only = canvas(AnnotationKind::Text, "AB", false);
    let (w1, h1) = ink_box(&text_only, 1);
    if w1 == 0 || h1 == 0 {
        println!("  !! nothing drew at 1x");
        failures += 1;
    }
    for scale in [2, 4] {
        let (w, h) = ink_box(&text_only, scale);
        let (want_w, want_h) = (w1 * scale, h1 * scale);
        // Within a pixel per step: glyph hinting rounds, it does not drift.
        let ok = (w - want_w).abs() <= scale && (h - want_h).abs() <= scale;
        println!(
            "label at {scale}x: {w}x{h}px, wanted about {want_w}x{want_h} — {}",
            if ok {
                "scales with the figure"
            } else {
                "DID NOT SCALE"
            }
        );
        if !ok {
            failures += 1;
        }
    }

    // ── The outline, measured as a run of ink across the ring ───────────────
    let ringed = canvas(AnnotationKind::Circle, "", false);
    for scale in [1, 2, 4] {
        let region = ViewRegion {
            x: 0.0,
            y: 0.0,
            width: N as f64,
            height: N as f64,
        };
        let mut surf = verbinal::ui::fits_export::compose_region(
            &ringed, N as i32, N as i32, region, scale, true,
        )
        .expect("composed");
        let (w, _h, rgba) = surface_to_rgba(&mut surf);
        // Across the row through the ring's centre: the first run of ink is the
        // left arc, and its width is the stroke.
        let cy = 150 * scale;
        let mut run = 0;
        for x in 0..w {
            let lit = rgba[((cy * w + x) * 4 + 3) as usize] > 40;
            if lit {
                run += 1;
            } else if run > 0 {
                break;
            }
        }
        let want = STROKE as i32 * scale;
        let ok = (run - want).abs() <= 1;
        println!(
            "ring at {scale}x: {run}px of stroke, wanted {want} — {}",
            if ok {
                "scales with the figure"
            } else {
                "DID NOT SCALE"
            }
        );
        if !ok {
            failures += 1;
        }
    }

    // ── And the whole plate, to look at ─────────────────────────────────────
    let shown = canvas(AnnotationKind::Circle, "NGC 5194", true);
    let region = ViewRegion {
        x: 0.0,
        y: 0.0,
        width: N as f64,
        height: N as f64,
    };
    let content = PlateContent {
        capture: Rc::new(move |w, h| {
            let mut s = shown
                .capture_region_surface(N as i32, N as i32, region, w, h, DrawOpts::export(false))
                .ok()?;
            let (_, _, rgba) = surface_to_rgba(&mut s);
            Some(rgba)
        }),
        title: "export_scale".into(),
        subtitle: "FITS image".into(),
        caption: "the marks should read the same at every scale".into(),
        colormap: "grayscale".into(),
        ramp: [(0, 0, 0); 256],
        lo_label: "0.0000".into(),
        hi_label: "1.0000".into(),
        date: "—".into(),
        footer: vec![("DIMENSIONS".into(), "240×240".into())],
        overlay: None,
    };
    for scale in [1, 4] {
        let out = std::env::temp_dir().join(format!("export_scale_{scale}x.png"));
        let mut f = std::fs::File::create(&out).expect("create");
        let surf = content.compose(scale, false).expect("plate");
        surf.write_to_png(&mut f).expect("write");
        println!(
            "wrote {} ({}x{})",
            out.display(),
            surf.width(),
            surf.height()
        );
    }

    if failures == 0 {
        println!("\nthe marks scale with the figure at every export scale");
    } else {
        println!("\n{failures} measurement(s) failed");
        std::process::exit(1);
    }
}
