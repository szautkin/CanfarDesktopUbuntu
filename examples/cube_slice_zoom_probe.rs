//! Does the slice show the data at the same size the volume does?
//!
//!     cargo run --example cube_slice_zoom_probe
//!
//! Reported from use: switching from the volume to the slice enlarged
//! everything, marks included. It was not the marks — a mark is stored in
//! voxels and is the same fraction of the cube in both views. It was the
//! FRAMING: the volume keeps the whole box on screen at every orbit angle,
//! which sits further out than fitting a plane to the widget, and the slice
//! had no way to sit at the same distance. Its zoom floor was fit itself, so
//! the wheel would not pull it back either.
//!
//! Both numbers come from live widgets, so no unit test can compare them.
use gtk4::prelude::*;
use verbinal::helpers::cube_wcs::CubeWcs;
use verbinal::models::volume_data::VolumeData;
use verbinal::ui::cube_viewer::CubeViewer;

fn viewer(nx: usize, ny: usize, nz: usize) -> std::rc::Rc<CubeViewer> {
    let vol = VolumeData {
        nx,
        ny,
        nz,
        data: vec![0.5; nx * ny * nz],
        name: "probe".into(),
        meta: None,
    };
    let wcs = CubeWcs::from_header(&std::collections::HashMap::new());
    CubeViewer::new(vol, wcs, "probe".into())
}

fn main() {
    gtk4::init().expect("gtk init");
    let _ = libadwaita::init();
    let mut failures = 0;

    for (nx, ny, nz) in [(64usize, 64usize, 8usize), (256, 256, 32), (512, 128, 16)] {
        let v = viewer(nx, ny, nz);
        v.set_slice_mode(true);
        let (zoom, in_volume, on_slice) = v.probe_scales();

        // The whole point: a voxel is worth the same on screen in both views,
        // so nothing changes size when you switch.
        let ratio = on_slice / in_volume.max(1e-9);
        if (ratio - 1.0).abs() > 0.02 {
            println!(
                "FAIL {nx}x{ny}x{nz}: a voxel is {in_volume:.3}px in the volume and \
                 {on_slice:.3}px on the slice ({ratio:.2}x) — switching modes resizes \
                 everything"
            );
            failures += 1;
        } else {
            println!("{nx}x{ny}x{nz}: voxel {in_volume:.3}px in both (slice zoom {zoom:.3})");
        }

        // Fit-to-widget would be a zoom of 1.0. Matching the volume means
        // sitting further out than that, and a default of exactly 1.0 is the
        // tell that the match did nothing.
        if zoom >= 0.99 {
            println!("FAIL {nx}x{ny}x{nz}: the slice defaulted to fit, so no match happened");
            failures += 1;
        }
    }

    // The wheel must be able to pull further out than the default, which the
    // old floor of 1.0 (fit) forbade outright.
    //
    // Realized, because zooming needs a real allocation to hold a point under
    // the cursor — without one the wheel is a no-op and the check would pass
    // for the wrong reason.
    let v = viewer(64, 64, 8);
    let win = gtk4::Window::new();
    win.set_default_size(616, 690);
    win.set_child(Some(v.widget()));
    win.present();
    while gtk4::glib::MainContext::default().iteration(false) {}
    v.set_slice_mode(true);
    while gtk4::glib::MainContext::default().iteration(false) {}
    let (start, ..) = v.probe_scales();
    for _ in 0..8 {
        v.probe_scroll_slice(0.8);
    }
    let (out, ..) = v.probe_scales();
    if (out - start).abs() < 1e-9 {
        // GTK does not allocate widgets without a real display, and zooming
        // needs an allocation to hold a point under the cursor. Said plainly
        // rather than passed off as a pass: the RANGE is covered by
        // `slice_zoom_tests::the_wheel_can_pull_back_past_fit`, which needs no
        // widget at all.
        println!("zoom-out check skipped — no allocation in a headless probe");
    } else if out >= start {
        println!("FAIL: the slice will not zoom out past {start:.3}");
        failures += 1;
    } else {
        println!("the slice zooms out: {start:.3} -> {out:.3}");
    }

    // Not checked here: that a zoom the user chose survives a mode switch.
    // Without an allocation the wheel above did nothing, so the check would
    // compare a value against itself and pass whatever the code did — which is
    // exactly what it did until a mutation showed it up. The decision is
    // `slice_zoom_tests::a_zoom_the_user_chose_is_left_alone`, which needs no
    // widget.

    if failures > 0 {
        std::process::exit(1);
    }
    println!("\nthe two views show the data at the same size");
}
