//! Do marks reach an exported figure?
//!
//!     cargo run --example cube_export_marks_probe
//!
//! Reported from use: annotations were invisible in the export. They were
//! drawn on screen and in an agent's capture, but `render_figure` returned the
//! bare render, so a figure exported for a document carried none of the marks
//! someone had put there to say what to look at.
//!
//! The pieces are unit-tested — the compositing step, and where a voxel lands
//! on a plate — but the WIRING is what broke, and nothing that returns a plate
//! can be reached without GTK. So this drives the real `render_figure` and
//! compares the pixels.
//!
//! The 2D slice path only: the volume path needs a GL context, and a probe
//! that skips silently on a headless box is worse than one that never claimed
//! to cover it.
use verbinal::helpers::cube_wcs::CubeWcs;
use verbinal::models::annotation::{Anchor, Annotation, AnnotationKind, Author, Extent};
use verbinal::models::volume_data::VolumeData;
use verbinal::ui::cube_viewer::CubeViewer;

const NX: usize = 64;
const NY: usize = 64;
const NZ: usize = 8;

fn volume() -> VolumeData {
    // A smooth ramp: every pixel differs from its neighbours, so a mark drawn
    // anywhere changes pixels rather than blending into a flat field.
    let mut data = vec![0.0f32; NX * NY * NZ];
    for z in 0..NZ {
        for y in 0..NY {
            for x in 0..NX {
                data[z * NX * NY + y * NX + x] = (x + y) as f32 / (NX + NY) as f32;
            }
        }
    }
    VolumeData {
        nx: NX,
        ny: NY,
        nz: NZ,
        data,
        name: "Synthetic".into(),
        meta: None,
    }
}

fn mark_on(channel: f64) -> Annotation {
    Annotation::new(
        AnnotationKind::Circle,
        Anchor::Data {
            x: 32.0,
            y: 32.0,
            z: channel,
        },
        "look here",
        Author::User,
    )
    .with_extent(Extent::square(8.0))
}

fn main() {
    gtk4::init().expect("gtk init");
    let _ = libadwaita::init();

    let wcs = CubeWcs::from_header(&std::collections::HashMap::new());
    let viewer = CubeViewer::new(volume(), wcs, "probe".into());
    viewer.set_slice_mode(true);
    assert!(
        viewer.is_slice_mode(),
        "the probe needs the slice, not the volume"
    );

    let (w, h) = (256, 256);
    let bare = viewer
        .render_figure(w, h, false)
        .expect("the slice path needs no GL");

    // Same view, same size, one mark on the channel being shown.
    viewer.set_current_channel(4);
    viewer.set_annotations(vec![mark_on(4.0)]);
    let marked = viewer
        .render_figure(w, h, false)
        .expect("the slice path needs no GL");

    let mut failures = 0;
    if bare.len() != marked.len() {
        println!("FAIL: plate size changed with the marks");
        failures += 1;
    }
    let changed = bare
        .iter()
        .zip(marked.iter())
        .filter(|(a, b)| a != b)
        .count();
    if changed == 0 {
        println!("FAIL: the mark is not in the exported figure — nothing composited it");
        failures += 1;
    } else {
        println!("mark reached the export: {changed} bytes differ");
    }

    // A mark on ANOTHER channel must not appear on this plate: the plate is
    // one plane, and a figure that shows a mark from elsewhere on it makes a
    // claim about the data that is not true.
    viewer.set_annotations(vec![mark_on(0.0)]);
    let other = viewer
        .render_figure(w, h, false)
        .expect("the slice path needs no GL");
    if other != bare {
        println!("FAIL: a mark from channel 0 was drawn on the channel 4 plate");
        failures += 1;
    } else {
        println!("a mark from another channel stays off the plate");
    }

    // The mark must scale with the PLATE, not stay a fixed number of pixels:
    // a ring the same size on a 4096px plate as on a 256px one is a speck.
    //
    // Measured as the fraction of the plate the mark spans, not as a count of
    // changed pixels. A mark is a stroked outline with a deliberately fixed
    // hairline width, so its pixel COUNT grows with circumference rather than
    // with area — an area-shaped assertion here failed against correct code,
    // which is a good reason not to assert a proxy for the thing you mean.
    //
    // Unlabelled, so the fixed-size text does not stretch the box differently
    // at the two plate sizes and blunt the comparison.
    let unlabelled = {
        let mut m = mark_on(4.0);
        m.text = String::new();
        m
    };
    let span = |size: i32| -> f64 {
        viewer.set_annotations(Vec::new());
        let bare = viewer.render_figure(size, size, false).expect("plate");
        viewer.set_annotations(vec![unlabelled.clone()]);
        let marked = viewer.render_figure(size, size, false).expect("plate");
        let (mut lo, mut hi) = (i32::MAX, i32::MIN);
        for y in 0..size {
            for x in 0..size {
                let i = ((y * size + x) * 4) as usize;
                if bare[i..i + 4] != marked[i..i + 4] {
                    lo = lo.min(x);
                    hi = hi.max(x);
                }
            }
        }
        if hi < lo {
            return 0.0;
        }
        (hi - lo + 1) as f64 / size as f64
    };
    let small_span = span(256);
    let big_span = span(1024);
    if small_span <= 0.0 || (small_span - big_span).abs() > 0.02 {
        println!(
            "FAIL: the mark did not scale with the plate — it spans \
             {small_span:.3} of a 256px plate and {big_span:.3} of a 1024px one"
        );
        failures += 1;
    } else {
        println!(
            "mark scales with the plate: spans {small_span:.3} at 256px, \
             {big_span:.3} at 1024px"
        );
    }

    if failures > 0 {
        std::process::exit(1);
    }
    println!("\nmarks reach the exported figure");
}
