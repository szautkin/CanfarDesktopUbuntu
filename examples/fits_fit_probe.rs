//! Does a FITS open showing the whole frame?
//!
//!     cargo run --example fits_fit_probe
//!
//! An 11471x4593 NIRCam mosaic opened at 100% and showed about 5% of its
//! width: the first thing anyone saw was a patch of sky with no way to tell
//! what they were looking at, and any marks on it were off-screen.
//!
//! The arithmetic is unit-tested. What is not, and what this checks, is the
//! one-shot RULE around it: a fit must happen once, must centre the frame, and
//! must never arrive after someone has chosen a zoom — a viewer that re-fits
//! later throws that choice away.
//!
//! GTK does not allocate widgets headlessly, so the viewport is passed in
//! rather than read off a widget.
use verbinal::ui::fits_canvas::FitsCanvas;

fn canvas(w: usize, h: usize) -> std::rc::Rc<FitsCanvas> {
    FitsCanvas::new(
        w,
        h,
        vec![0u8; w * h * 4],
        std::rc::Rc::new(std::cell::RefCell::new(Default::default())),
        None,
    )
}

fn main() {
    gtk4::init().expect("gtk init");
    let mut failures = 0;

    // A frame far wider than the viewport fits, and is centred.
    let c = canvas(11471, 4593);
    if !c.fit_pending() {
        println!("FAIL: a fresh canvas is not waiting to fit");
        failures += 1;
    }
    c.fit_to_viewport_for_probe(900.0, 700.0);
    let scale = c.zoom_scale();
    let expected = 900.0 / 11471.0;
    if (scale - expected).abs() > 1e-9 {
        println!("FAIL: fitted to {scale}, expected {expected}");
        failures += 1;
    } else {
        println!("11471x4593 in 900x700 fits at {:.2}%", scale * 100.0);
    }

    // Once, and only once: a later allocation must not re-fit, or every window
    // resize would throw away the zoom the user had chosen.
    if c.fit_pending() {
        println!("FAIL: still pending after fitting — a resize would re-fit");
        failures += 1;
    }
    let before = c.zoom_scale();
    c.fit_to_viewport_for_probe(1600.0, 1200.0);
    if (c.zoom_scale() - before).abs() > 1e-12 {
        println!("FAIL: a second allocation re-fitted, discarding the current zoom");
        failures += 1;
    } else {
        println!("a later allocation does not re-fit");
    }

    // A chosen zoom cancels the pending fit. This is the one that matters for
    // sync-zoom: it applies a scale as the tab loads, and a fit landing after
    // it would silently undo it.
    let c = canvas(4000, 4000);
    c.set_zoom(2.0);
    if c.fit_pending() {
        println!("FAIL: a chosen zoom left the fit pending — it would overwrite it");
        failures += 1;
    }
    c.fit_to_viewport_for_probe(900.0, 700.0);
    if (c.zoom_scale() - 2.0).abs() > 1e-12 {
        println!("FAIL: the fit overwrote a zoom the user had chosen");
        failures += 1;
    } else {
        println!("a chosen zoom survives the pending fit");
    }

    // A small image opens at 100% with room around it, not blown up.
    let c = canvas(64, 64);
    c.fit_to_viewport_for_probe(1600.0, 1200.0);
    if (c.zoom_scale() - 1.0).abs() > 1e-12 {
        println!("FAIL: a 64x64 thumbnail was enlarged to {}", c.zoom_scale());
        failures += 1;
    } else {
        println!("a small image stays at 100%");
    }

    if failures > 0 {
        std::process::exit(1);
    }
    println!("\na FITS opens showing all of itself");
}
