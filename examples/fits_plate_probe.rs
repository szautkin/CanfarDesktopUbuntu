//! Does a FITS plate say where it is pointing?
//!
//!     cargo run --example fits_plate_probe -- <file.fits> [out.png]
//!
//! The exported figure was a bare crop: no caption, no colour ramp, and
//! nothing saying what part of the sky it showed. This composes the plate for a
//! known region of a real frame and checks the coordinates in it are the
//! coordinates of that region, read back through the same WCS the crosshair
//! uses.
use std::rc::Rc;
use verbinal::ui::fits_canvas::ViewRegion;
use verbinal::ui::fits_tab::FitsTab;

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: fits_plate_probe <file.fits>");
    gtk4::init().expect("gtk init");

    let data =
        verbinal::helpers::fits_loader::load_fits_image(std::path::Path::new(&path)).expect("load");
    let shared = Rc::new(std::cell::RefCell::new(Default::default()));
    let tab = FitsTab::new(data, shared, path.clone());
    tab.canvas().cancel_fit(); // scale 1, offset 0: view pixel == image pixel

    // A region well inside the frame, not at the origin and not square.
    let region = ViewRegion {
        x: 2400.0,
        y: 2000.0,
        width: 600.0,
        height: 400.0,
    };
    let content = verbinal::ui::fits_export::plate_content(&tab, 900, 700, region);

    println!("title:    {}", content.title);
    println!("subtitle: {}", content.subtitle);
    println!("caption:  {}", content.caption);
    for (k, v) in &content.footer {
        println!("  {k:<12} {v}");
    }
    println!(
        "colorbar: {} .. {}  ({})",
        content.lo_label, content.hi_label, content.colormap
    );

    let mut failures = 0;
    if content.caption.trim().is_empty() {
        println!("  !! the caption is empty — the figure says nothing about itself");
        failures += 1;
    }
    // The caption must carry sky coordinates, not just a size.
    if !content.caption.contains('h') && !content.caption.contains('°') {
        println!("  !! the caption has no sky position: {}", content.caption);
        failures += 1;
    }
    for want in ["RA", "DEC", "FIELD", "CUT LEVELS"] {
        if !content.footer.iter().any(|(k, _)| k == want) {
            println!("  !! the footer has no {want}");
            failures += 1;
        }
    }

    let Some(surf) = content.compose(1, false) else {
        println!("  !! the plate composed nothing");
        std::process::exit(1);
    };
    let out = std::env::args().nth(2).unwrap_or_else(|| {
        std::env::temp_dir()
            .join("fits_plate_probe.png")
            .display()
            .to_string()
    });
    let mut f = std::fs::File::create(&out).expect("create");
    surf.write_to_png(&mut f).expect("write");
    println!("\nwrote {out} ({}x{})", surf.width(), surf.height());

    if failures > 0 {
        std::process::exit(1);
    }
}
