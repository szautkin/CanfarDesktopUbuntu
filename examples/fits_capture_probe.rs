//! Does the FITS capture show what the screen shows?
//!
//!     cargo run --example fits_capture_probe -- [file.fits]
//!
//! Exits non-zero if a capture is blank, or does not follow the view.
//!
//! `get_fits_image` renders through `FitsCanvas::draw_working_area` — the same
//! function `set_draw_func` runs — so the capture and the screen cannot drift
//! apart without the screen changing too. That is the design; this checks the
//! properties that would break if someone later "optimised" the capture into a
//! second renderer:
//!
//!  * the same view captured twice is byte-identical, and
//!  * a capture after the view changes is different.
//!
//! A capture that silently ignored the view state would pass neither, and would
//! otherwise reach an agent as a confident description of the wrong picture.
use verbinal::ui::fits_canvas::FitsCanvas;

/// A synthetic image, so the probe runs with no data files present.
fn synthetic_rgba(width: usize, height: usize) -> Vec<u8> {
    let mut rgba = vec![0u8; width * height * 4];
    for y in 0..height {
        for x in 0..width {
            let i = (y * width + x) * 4;
            // A diagonal ramp with a bright square, so a wrong crop or a
            // flipped axis is visible rather than plausible.
            let v = ((x + y) * 255 / (width + height)) as u8;
            let bright =
                (width / 4..width / 2).contains(&x) && (height / 4..height / 2).contains(&y);
            let value = if bright { 255 } else { v };
            rgba[i] = value;
            rgba[i + 1] = value;
            rgba[i + 2] = value;
            rgba[i + 3] = 255;
        }
    }
    rgba
}

fn sha(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())[..12].to_string()
}

fn main() {
    gtk4::init().expect("gtk init");

    let (w, h) = (256usize, 192usize);
    let canvas = FitsCanvas::new(w, h, synthetic_rgba(w, h), Default::default(), None);

    let mut failures = 0;
    let (cw, ch) = (400, 300);

    let first = canvas.capture_png(cw, ch).expect("capture");
    let again = canvas.capture_png(cw, ch).expect("capture");
    println!("first  {} bytes, sha {}", first.len(), sha(&first));
    println!("again  {} bytes, sha {}", again.len(), sha(&again));

    if first != again {
        println!("  !! the same view captured twice differs — the capture is not deterministic");
        failures += 1;
    }
    // A blank capture is the failure mode that looks like success: a PNG of the
    // right size holding nothing.
    if first.len() < 1000 {
        println!(
            "  !! the capture is suspiciously small ({} bytes) — probably blank",
            first.len()
        );
        failures += 1;
    }

    // Change the view. The capture must follow it.
    canvas.set_zoom(4.0);
    let zoomed = canvas.capture_png(cw, ch).expect("capture");
    println!("zoomed {} bytes, sha {}", zoomed.len(), sha(&zoomed));
    if zoomed == first {
        println!(
            "  !! the capture did not change when the view did — it is ignoring the view state"
        );
        failures += 1;
    }

    // A size that cannot be drawn is refused, not allocated.
    if canvas.capture_png(0, 300).is_ok() || canvas.capture_png(400, -1).is_ok() {
        println!("  !! an impossible capture size was accepted");
        failures += 1;
    }

    // ── Annotations ─────────────────────────────────────────────────────────
    //
    // The invariant the whole anchor design exists for: a mark is pinned to an
    // IMAGE pixel, so when the view moves the mark moves with the image. A mark
    // stored in screen pixels passes every other test and fails this one, and
    // in the app it shows up as annotations sliding off their subjects.
    use verbinal::models::annotation::{Anchor, Annotation, AnnotationKind, Author, Extent};
    canvas.set_zoom(1.0);
    let mut ring = Annotation::new(
        AnnotationKind::Circle,
        Anchor::ImagePixel { x: 128.0, y: 96.0 },
        "subject",
        Author::Agent,
    );
    ring.extent = Some(Extent::square(20.0));
    canvas.set_annotations(vec![ring]);

    let annotated = canvas.capture_png(cw, ch).expect("capture");
    println!(
        "annotated {} bytes, sha {}",
        annotated.len(),
        sha(&annotated)
    );
    if annotated == first {
        println!("  !! the annotation did not appear in the capture");
        failures += 1;
    }

    // Zoom holds the view CENTRE now, so a mark AT the centre correctly stays
    // put — this probe used to check the middle pixel and began reporting a
    // failure the moment that fix landed. An off-centre pixel is the one that
    // must travel with the image.
    let off_centre = (40.0, 30.0);
    let before = canvas.image_to_screen_point(off_centre.0, off_centre.1);
    canvas.set_zoom(2.0);
    let after = canvas.image_to_screen_point(off_centre.0, off_centre.1);
    let travelled = ((before.0 - after.0).powi(2) + (before.1 - after.1).powi(2)).sqrt();
    println!("off-centre anchor travelled {travelled:.1}px on a 2x zoom");
    if travelled < 5.0 {
        println!(
            "  !! it barely moved ({before:?} -> {after:?}) — pinned to the window, not the data"
        );
        failures += 1;
    }

    // The other half of that rule — the view centre must NOT move — needs a
    // REALIZED widget: `set_zoom` anchors on the allocated viewport, and an
    // unrealized drawing area reports no size, so the two disagree here and
    // nowhere else. It is checked against the running app instead, where the
    // centre held at 100%, 200% and 400%.
    let (vw, vh) = canvas.view_size();
    if vw > 0 && vh > 0 {
        let centre_px = canvas.screen_to_image_point_public(vw as f64 / 2.0, vh as f64 / 2.0);
        canvas.set_zoom(4.0);
        let centre_now = canvas.screen_to_image_point_public(vw as f64 / 2.0, vh as f64 / 2.0);
        if (centre_px.0 - centre_now.0).abs() > 2.0 || (centre_px.1 - centre_now.1).abs() > 2.0 {
            println!("  !! zooming moved the view centre ({centre_px:?} -> {centre_now:?})");
            failures += 1;
        } else {
            println!("view centre held across a 4x zoom");
        }
    } else {
        println!("view centre check skipped — no allocation in a headless probe");
    }
    canvas.set_zoom(1.0);

    // ── Hit-testing ─────────────────────────────────────────────────────────
    //
    // `annotation_at` used `?` inside its loop, so ONE mark that could not be
    // placed on this canvas abandoned the whole search and every other mark
    // became unclickable. A cube's voxel anchor is exactly such a mark.
    let mut clickable = Annotation::new(
        AnnotationKind::Circle,
        Anchor::ImagePixel { x: 128.0, y: 96.0 },
        "clickable",
        Author::User,
    );
    clickable.extent = Some(Extent::square(20.0));
    let unplaceable = Annotation::new(
        AnnotationKind::Circle,
        // Voxel space means nothing on a FITS canvas.
        Anchor::Data {
            x: 1.0,
            y: 1.0,
            z: 1.0,
        },
        "not here",
        Author::Agent,
    );
    // The unplaceable one LAST, so the reversed search meets it first.
    canvas.set_annotations(vec![clickable.clone(), unplaceable]);
    let (hx, hy) = canvas.image_to_screen_point(128.0, 96.0);
    match canvas.annotation_at(hx, hy) {
        Some(id) if id == clickable.id => println!("hit test found the clickable mark"),
        other => {
            println!("  !! hit test returned {other:?} — one unplaceable mark hid the rest");
            failures += 1;
        }
    }

    // ── A smaller capture is the same picture, not a corner of it ──────────
    //
    // The view transform is in absolute screen pixels, so drawing it into a
    // smaller raster used to clip rather than shrink: a capture asked for at
    // 1024 from a 1400px canvas returned the top-left 1024px and reported
    // `scale: 0.73`. The default limit IS 1024, so any maximised window was
    // handing an agent a crop labelled as a faithful downscale — the exact
    // failure the whole feature exists to avoid.
    //
    // Measured through the centre of mass of the lit pixels: under a true
    // scale it stays in the middle of the frame at every size, and the lit
    // area keeps the same FRACTION of the raster.
    {
        let (iw, ih) = (200usize, 200usize);
        let mut rgba = vec![0u8; iw * ih * 4];
        for y in 80..120 {
            for x in 80..120 {
                let i = (y * iw + x) * 4;
                rgba[i..i + 4].copy_from_slice(&[255, 255, 255, 255]);
            }
        }
        let c = FitsCanvas::new(
            iw,
            ih,
            rgba,
            std::rc::Rc::new(std::cell::RefCell::new(Default::default())),
            None,
        );
        c.cancel_fit();
        for (cw, ch) in [(400, 400), (200, 200), (100, 100)] {
            let png = c.capture_png_from_view(200, 200, cw, ch).expect("capture");
            let mut surf =
                gtk4::cairo::ImageSurface::create_from_png(&mut png.as_slice()).expect("decode");
            let stride = surf.stride() as usize;
            surf.flush();
            let data = surf.data().expect("pixels");
            let (mut lit, mut sx, mut sy) = (0usize, 0f64, 0f64);
            for y in 0..ch as usize {
                for x in 0..cw as usize {
                    // Premultiplied BGRA; red is byte 2.
                    if data[y * stride + x * 4 + 2] > 128 {
                        lit += 1;
                        sx += x as f64;
                        sy += y as f64;
                    }
                }
            }
            let frac = lit as f64 / (cw * ch) as f64;
            let (mx, my) = (
                sx / lit.max(1) as f64 / cw as f64,
                sy / lit.max(1) as f64 / ch as f64,
            );
            // The square is 40x40 in a 200x200 view: 4% of it, dead centre.
            let ok =
                (frac - 0.04).abs() < 0.005 && (mx - 0.5).abs() < 0.02 && (my - 0.5).abs() < 0.02;
            if ok {
                println!(
                    "capture {cw}x{ch}: {:.1}% lit, centred at ({mx:.2}, {my:.2})",
                    100.0 * frac
                );
            } else {
                println!(
                    "  !! capture {cw}x{ch} is a CROP, not a scale: {:.1}% lit at ({mx:.2}, {my:.2}) \
                     — expected 4.0% at (0.50, 0.50)",
                    100.0 * frac
                );
                failures += 1;
            }
        }
    }

    // ── A capture is a picture, not a screenshot of a UI state ─────────────
    //
    // Marks belong in a capture; which one you happen to have CLICKED does
    // not. A selected ring draws white and an edited one amber, and in an
    // exported figure those say nothing to a reader except that one mark is
    // inexplicably a different colour. Same rule as the grips, which was
    // applied and these two were not.
    {
        let (iw, ih) = (120usize, 120usize);
        let mk = || {
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
            m.extent = Some(Extent::square(30.0));
            let id = m.id.clone();
            c.set_annotations(vec![m]);
            (c, id)
        };

        let (plain, _) = mk();
        let reference = plain
            .capture_png_from_view(120, 120, 120, 120)
            .expect("capture");

        let (selected, id) = mk();
        selected.set_selected_annotation(Some(id.clone()));
        let with_selection = selected
            .capture_png_from_view(120, 120, 120, 120)
            .expect("capture");
        if with_selection == reference {
            println!("a selected mark exports in the ordinary ink");
        } else {
            println!("  !! selection highlighting leaked into the capture");
            failures += 1;
        }

        let (editing, id) = mk();
        editing.set_selected_annotation(Some(id.clone()));
        editing.set_editing_annotation(Some(id));
        let with_editing = editing
            .capture_png_from_view(120, 120, 120, 120)
            .expect("capture");
        if with_editing == reference {
            println!("an edited mark exports in the ordinary ink, without grips");
        } else {
            println!("  !! edit highlighting or grips leaked into the capture");
            failures += 1;
        }
    }

    let out = std::env::temp_dir().join("fits_capture_probe.png");
    std::fs::write(&out, &annotated).expect("write");
    println!(
        "\nwrote {} — look at it once; a render can be plausible and wrong",
        out.display()
    );

    if failures > 0 {
        println!("{failures} check(s) failed.");
        std::process::exit(1);
    }
    println!("capture follows the view");
}
