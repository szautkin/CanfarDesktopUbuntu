//! Does rebuilding the marks list stay quiet?
//!
//!     cargo run --example annotations_panel_probe
//!
//! Repopulating the list selects a row, and selecting a row tells the viewer,
//! which refreshes the panel, which repopulates... The first version also
//! connected the selection handler INSIDE the rebuild, so every pass added
//! another one and all of them fired. Placing a mark ended in
//! `fatal runtime error: stack overflow`.
//!
//! Needs a GTK init for the widgets, so it is a probe rather than a unit test.
use std::cell::Cell;
use std::rc::Rc;
use verbinal::models::annotation::{Anchor, Annotation, AnnotationKind, Author};
use verbinal::ui::annotations_panel::AnnotationsPanel;

fn marks(n: usize) -> Vec<Annotation> {
    (0..n)
        .map(|i| {
            Annotation::new(
                AnnotationKind::Circle,
                Anchor::ImagePixel {
                    x: i as f64,
                    y: i as f64,
                },
                format!("mark {i}"),
                Author::User,
            )
        })
        .collect()
}

fn main() {
    gtk4::init().expect("gtk init");
    let panel = AnnotationsPanel::new();

    let calls = Rc::new(Cell::new(0usize));
    {
        let calls = calls.clone();
        panel.set_on_select(move |_| calls.set(calls.get() + 1));
    }

    let list = marks(5);
    let selected = list[2].id.clone();

    // The rebuild selects a row. It must not report that as the user's doing.
    panel.set_annotations(&list, Some(&selected));
    let after_one = calls.get();
    println!("callbacks after one rebuild with a selection: {after_one}");

    // And repeating it must not multiply anything.
    for _ in 0..50 {
        panel.set_annotations(&list, Some(&selected));
    }
    let after_many = calls.get();
    println!("callbacks after fifty more: {after_many}");

    let mut failures = 0;
    if after_one != 0 {
        println!("  !! a rebuild reported its own selection — this is the loop");
        failures += 1;
    }
    if after_many != after_one {
        println!(
            "  !! repeated rebuilds fired {} extra callbacks — handlers are accumulating",
            after_many - after_one
        );
        failures += 1;
    }

    // An empty list is a valid state and must not panic.
    panel.set_annotations(&[], None);
    println!("empty list rebuilt cleanly");

    if failures > 0 {
        println!("{failures} check(s) failed.");
        std::process::exit(1);
    }
    println!("rebuilding the list is silent");
}
