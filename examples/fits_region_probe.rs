//! Is the exported area the area that was dragged?
//!
//!     cargo run --example fits_region_probe
//!
//! Reported from use: an exported PDF covered more of the sky than the box
//! drawn on screen. This drives the same `compose_region` the Save button
//! does, on an image whose every pixel says where it is, and checks the
//! corners of the output against the corners of the region.
use verbinal::helpers::image_bytes::surface_to_rgba;
use verbinal::ui::fits_canvas::{FitsCanvas, ViewRegion};

const N: usize = 240;

/// An image whose red channel encodes x and green encodes y, so any pixel of
/// the export says which image pixel it came from.
fn coded() -> Vec<u8> {
    let mut rgba = vec![0u8; N * N * 4];
    for y in 0..N {
        for x in 0..N {
            let i = (y * N + x) * 4;
            rgba[i] = x as u8;
            rgba[i + 1] = y as u8;
            rgba[i + 2] = 128;
            rgba[i + 3] = 255;
        }
    }
    rgba
}

fn main() {
    gtk4::init().expect("gtk init");
    let mut failures = 0;

    let c = FitsCanvas::new(
        N,
        N,
        coded(),
        std::rc::Rc::new(std::cell::RefCell::new(Default::default())),
        None,
    );
    c.cancel_fit(); // scale 1, offset 0: view pixel == image pixel

    // A region that is NOT at the origin and NOT square, because a region at
    // the origin makes the offset term a no-op and a square one hides an
    // axis swap.
    let region = ViewRegion {
        x: 40.0,
        y: 90.0,
        width: 120.0,
        height: 60.0,
    };

    for scale in [1, 2, 4] {
        let Some(mut surf) =
            verbinal::ui::fits_export::compose_region(&c, N as i32, N as i32, region, scale, false)
        else {
            println!("  !! scale {scale}: nothing composed");
            failures += 1;
            continue;
        };
        let (w, h, rgba) = surface_to_rgba(&mut surf);
        let at = |x: i32, y: i32| -> (u8, u8) {
            let i = ((y * w + x) * 4) as usize;
            (rgba[i], rgba[i + 1])
        };
        // Half a source pixel in, so sampling lands inside the first and last
        // source pixels rather than on their boundary.
        let inset = scale / 2;
        let (tl_x, tl_y) = at(inset, inset);
        let (br_x, br_y) = at(w - 1 - inset, h - 1 - inset);

        let want_tl = (region.x as u8, region.y as u8);
        let want_br = (
            (region.x + region.width - 1.0) as u8,
            (region.y + region.height - 1.0) as u8,
        );
        let near = |a: u8, b: u8| (a as i32 - b as i32).abs() <= 1;

        let ok = near(tl_x, want_tl.0)
            && near(tl_y, want_tl.1)
            && near(br_x, want_br.0)
            && near(br_y, want_br.1);
        if ok {
            println!(
                "scale {scale}: {w}x{h} covers image ({tl_x},{tl_y})..({br_x},{br_y}) — the region asked for"
            );
        } else {
            println!(
                "  !! scale {scale}: {w}x{h} covers ({tl_x},{tl_y})..({br_x},{br_y}), \
                 expected ({},{})..({},{})",
                want_tl.0, want_tl.1, want_br.0, want_br.1
            );
            failures += 1;
        }
    }

    if failures > 0 {
        println!("{failures} check(s) failed.");
        std::process::exit(1);
    }
    println!("\nthe exported area is the area that was asked for");
}
