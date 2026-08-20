//! What measuring a column heading costs, and what memoising it saves.
//!
//!     cargo run --example cell_measure_probe
//!
//! `column_width_for` builds a throwaway Label and asks Pango to shape its
//! text. The results table called it once per CELL — a hundred rows times
//! fifteen columns — and a page turn took nine and a half seconds.
use gtk4::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::time::Instant;

const HEADINGS: [&str; 15] = [
    "Collection",
    "RA (J2000.0)",
    "Dec. (J2000.0)",
    "Target Name",
    "Start Date",
    "Instrument",
    "Filter",
    "Cal. Lev.",
    "Obs. Type",
    "Proposal ID",
    "PI Name",
    "Obs. ID",
    "View",
    "Save",
    "More",
];
const ROWS: usize = 100;

fn measure(heading: &str) -> i32 {
    let probe = gtk4::Label::new(Some(heading));
    let (_, natural, _, _) = probe.measure(gtk4::Orientation::Horizontal, -1);
    natural + 34
}

fn main() {
    gtk4::init().expect("gtk init");

    let started = Instant::now();
    let mut sink = 0i32;
    for _ in 0..ROWS {
        for h in HEADINGS {
            sink += measure(h);
        }
    }
    let per_cell = started.elapsed();

    let cache: RefCell<HashMap<&str, i32>> = RefCell::new(HashMap::new());
    let started = Instant::now();
    for _ in 0..ROWS {
        for h in HEADINGS {
            let hit = cache.borrow().get(h).copied();
            sink += match hit {
                Some(w) => w,
                None => {
                    let w = measure(h);
                    cache.borrow_mut().insert(h, w);
                    w
                }
            };
        }
    }
    let memoised = started.elapsed();

    println!("one page render, {ROWS} rows x {} columns:", HEADINGS.len());
    println!("  measuring per cell   {per_cell:?}");
    println!("  memoised per column  {memoised:?}");
    println!(
        "  ({}x faster, sink {sink})",
        per_cell.as_nanos() / memoised.as_nanos().max(1)
    );

    // And the widgets themselves, built as the row loop builds them.
    let panel = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    let started = Instant::now();
    for _ in 0..ROWS {
        let row_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 0);
        for heading in HEADINGS {
            let label = gtk4::Label::new(Some(heading));
            label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
            label.set_max_width_chars(1);
            label.set_halign(gtk4::Align::Fill);
            label.set_xalign(0.0);
            label.set_size_request(100, -1);
            label.set_hexpand(false);
            label.set_margin_end(6);
            row_box.append(&label);
        }
        let row_btn = gtk4::Button::new();
        row_btn.set_child(Some(&row_box));
        row_btn.add_css_class("flat");
        panel.append(&row_btn);
    }
    println!();
    println!("  building the widgets {:?}", started.elapsed());
    println!("  (a page render is the two added together)");
}
