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

    // Does the canvas ANNOUNCE a change? The list is built from that signal,
    // so if it does not fire, marks land on the image and never in the panel —
    // which looks like an empty list that fills in as soon as you click a mark,
    // because clicking goes through a different callback.
    {
        use std::rc::Rc as R;
        use verbinal::ui::fits_canvas::FitsCanvas;
        let canvas = FitsCanvas::new(64, 64, vec![0u8; 64 * 64 * 4], Default::default(), None);
        let fired = R::new(Cell::new(0usize));
        {
            let fired = fired.clone();
            canvas.set_on_annotations_changed(move || fired.set(fired.get() + 1));
        }
        canvas.set_annotations(marks(2));
        println!(
            "annotations-changed fired {} time(s) for one set",
            fired.get()
        );
        if fired.get() == 0 {
            println!("  !! the canvas did not announce the change — the list cannot follow");
            failures += 1;
        }
        // And the marks really are there to be read back.
        if canvas.annotations().len() != 2 {
            println!("  !! the canvas did not keep the marks");
            failures += 1;
        }
    }

    // Select and deselect, which has been wrong three different ways: a click
    // that selected then instantly deselected itself, a deselect that landed on
    // the first row, and a select that needed two clicks. All three came from
    // GtkListBox's own selection; the section owns it now, so it can be asked.
    {
        use verbinal::ui::item_list_section::{ItemListSection, ListItem, RowActions, SectionSpec};
        let section = ItemListSection::new(SectionSpec {
            actions: RowActions::EDIT_AND_DELETE,
            filter_placeholder: Some("filter"),
            empty_message: "nothing",
            selectable: true,
        });
        let items: Vec<ListItem> = (0..4)
            .map(|i| ListItem {
                id: format!("id-{i}"),
                title: format!("row {i}"),
                subtitle: "sub".into(),
            })
            .collect();
        let reported = Rc::new(std::cell::RefCell::new(Vec::<String>::new()));
        {
            let reported = reported.clone();
            section.set_on_select(move |id| reported.borrow_mut().push(id.to_string()));
        }
        section.set_items(&items, None, None);

        section.click_row("id-2");
        println!("after one click: {:?}", section.selected());
        if section.selected().as_deref() != Some("id-2") {
            println!("  !! one click did not select — it used to need two");
            failures += 1;
        }

        section.click_row("id-2");
        println!("after a second click on it: {:?}", section.selected());
        match section.selected() {
            None => {}
            Some(other) => {
                println!("  !! deselecting landed on {other} instead of clearing");
                failures += 1;
            }
        }

        // A rebuild must not invent a selection.
        section.set_items(&items, None, None);
        if section.selected().is_some() {
            println!("  !! a rebuild selected something on its own");
            failures += 1;
        }
        let seen = reported.borrow().clone();
        println!("reported: {seen:?}");
        if seen != vec!["id-2".to_string(), String::new()] {
            println!("  !! the section reported {seen:?}, not one select then one clear");
            failures += 1;
        }
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
