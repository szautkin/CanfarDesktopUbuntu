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

    let out = std::env::temp_dir().join("fits_capture_probe.png");
    std::fs::write(&out, &first).expect("write");
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
