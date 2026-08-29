//! What does one redraw of a large FITS cost?
//!
//!     cargo run --release --example fits_draw_cost_probe
//!
//! `rgba_to_surface` premultiplies and channel-swaps every pixel, and it used
//! to run on every draw. On a JWST-sized frame that is tens of millions of
//! pixels per repaint — so a popover taking a keystroke, or a pointer moving,
//! paid for the whole image again. This measures the first draw against the
//! ones after it.
use std::time::Instant;
use verbinal::ui::fits_canvas::FitsCanvas;

fn main() {
    gtk4::init().expect("gtk init");

    // A NIRCam i2d SCI frame, near enough.
    let (w, h) = (11471usize, 4593usize);
    println!("image {w}x{h} = {:.1} Mpx", (w * h) as f64 / 1e6);
    let rgba = vec![128u8; w * h * 4];

    let canvas = FitsCanvas::new(w, h, rgba, Default::default(), None);
    let (cw, ch) = (1600, 1000);

    let first = Instant::now();
    canvas.capture_png(cw, ch).expect("capture");
    let first = first.elapsed();

    let mut rest = std::time::Duration::ZERO;
    for _ in 0..5 {
        let t = Instant::now();
        canvas.capture_png(cw, ch).expect("capture");
        rest += t.elapsed();
    }
    let rest = rest / 5;

    println!(
        "first draw   {:>8.1} ms  (builds the surface)",
        first.as_secs_f64() * 1000.0
    );
    println!(
        "later draws  {:>8.1} ms  (reuses it)",
        rest.as_secs_f64() * 1000.0
    );

    // A repaint that costs as much as the first one is a repaint that is
    // rebuilding the image, which is what made typing feel laggy.
    if rest.as_secs_f64() > first.as_secs_f64() * 0.6 {
        println!(
            "  !! later draws cost nearly as much as the first — the surface is \
             being rebuilt every frame"
        );
        std::process::exit(1);
    }
    println!("\nthe surface is built once, not per frame");
}
