//! Do notebook outputs get any ROOM, in the container the app actually uses?
//!
//!     cargo run --example notebook_layout_probe
//!
//! Exits non-zero if an output is allocated too little height to be seen.
//!
//! This exists because `notebook_output_probe` passed while a user was looking
//! at blank cells. That probe walks the widget tree and confirms a `GtkPicture`
//! was built — which it was, with correct texture data. What it could not see
//! is that the picture was then drawn in ONE PIXEL of height.
//!
//! `GtkPicture` with `can_shrink` reports a minimum height of zero, and the
//! notebook packs its cells into a `GtkListBox`, which allocates rows their
//! minimum. Labels survived because a label's minimum height is its text; every
//! image output collapsed. Measuring in a plain `GtkBox` — which is what the
//! other probe and every unit test did — hides this completely, because a box
//! with room to spare hands out natural heights.
//!
//! So this builds the app's real container: a `ScrolledWindow` with horizontal
//! policy `Never`, a `GtkListBox`, and one non-focusable `GtkListBoxRow` per
//! cell, exactly as `NotebookPage` does. Then it presents a window and reports
//! what each output was ALLOCATED, which is the only question that matters.
use gtk4::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use verbinal::models::notebook_document::CellOutput;
use verbinal::ui::notebook_cell::CodeCellWidget;

/// Below this, an output is not visible to a reader.
const MIN_VISIBLE_HEIGHT: i32 = 24;

/// A PNG of `width` x `height`, base64-encoded, built with PIL.
///
/// Generated rather than pasted so the aspect ratios are real and the sizes are
/// the ones from the report: a 140x90 thumbnail and a 640x480 figure.
fn png(width: u32, height: u32) -> Option<String> {
    let script = format!(
        "import base64, io\n\
         from PIL import Image, ImageDraw\n\
         im = Image.new('RGB',({width},{height}),(20,24,48))\n\
         ImageDraw.Draw(im).ellipse((4,4,{width}-4,{height}-4), fill=(240,190,70))\n\
         b = io.BytesIO(); im.save(b, format='PNG')\n\
         print(base64.b64encode(b.getvalue()).decode())"
    );
    let out = std::process::Command::new("python3")
        .arg("-c")
        .arg(script)
        .output()
        .ok()?;
    let b64 = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!b64.is_empty()).then_some(b64)
}

/// The outputs to lay out, as `(name, CellOutput)`.
fn cases() -> Vec<(String, Vec<CellOutput>)> {
    let mut out = Vec::new();

    if let Some(b64) = png(140, 90) {
        // Smaller than the nominal output width: must be shown at its own size,
        // not stretched, and certainly not collapsed.
        let json = format!(
            r#"{{"output_type":"execute_result","execution_count":1,
                 "data":{{"image/png":"{b64}","text/plain":"<PIL.Image.Image>"}},
                 "metadata":{{}}}}"#
        );
        out.push((
            "small image (140x90)".to_string(),
            vec![serde_json::from_str(&json).expect("bundle parses")],
        ));
    }
    if let Some(b64) = png(640, 480) {
        // The matplotlib default size: must be scaled down, keeping its shape.
        let json = format!(
            r#"{{"output_type":"display_data",
                 "data":{{"image/png":"{b64}","text/plain":"<Figure size 640x480>"}},
                 "metadata":{{}}}}"#
        );
        out.push((
            "figure-sized image (640x480)".to_string(),
            vec![serde_json::from_str(&json).expect("bundle parses")],
        ));
    }

    // SVG takes the same path, and would collapse the same way.
    let svg = r#"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"120\" height=\"60\"><rect width=\"120\" height=\"60\" fill=\"teal\"/></svg>"#;
    let json = format!(
        r#"{{"output_type":"display_data","data":{{"image/svg+xml":"{svg}","text/plain":"<S>"}},"metadata":{{}}}}"#
    );
    out.push((
        "svg (120x60)".to_string(),
        vec![serde_json::from_str(&json).expect("bundle parses")],
    ));

    // A table and a plain label, as the controls: these never collapsed, and if
    // they ever start to, the cause is shared and worth knowing about.
    let json = r#"{"output_type":"execute_result","execution_count":1,
                   "data":{"text/html":"<table><tr><th>a</th></tr><tr><td>1</td></tr></table>",
                           "text/plain":"<Table>"},"metadata":{}}"#;
    out.push((
        "html table (control)".to_string(),
        vec![serde_json::from_str(json).expect("bundle parses")],
    ));
    let json = r#"{"output_type":"stream","name":"stdout","text":"plain text\n"}"#;
    out.push((
        "stream text (control)".to_string(),
        vec![serde_json::from_str(json).expect("bundle parses")],
    ));

    out
}

/// The tallest output widget inside `cell`, and what it is.
fn tallest_output(cell: &gtk4::Widget) -> (String, i32) {
    fn walk(w: &gtk4::Widget, best: &mut (String, i32)) {
        let kind = w.type_().name().to_string();
        let interesting = matches!(
            kind.as_str(),
            "GtkPicture" | "GtkLabel" | "GtkGrid" | "GtkScrolledWindow"
        );
        // The cell's own source view is not an output.
        let is_source = kind == "GtkTextView";
        if interesting && !is_source && w.height() > best.1 {
            *best = (kind, w.height());
        }
        let mut child = w.first_child();
        while let Some(c) = child {
            walk(&c, best);
            child = c.next_sibling();
        }
    }
    let mut best = ("nothing".to_string(), 0);
    walk(cell, &mut best);
    best
}

fn main() {
    let app = gtk4::Application::builder()
        .application_id("net.canfar.Verbinal.NotebookLayoutProbe")
        .build();
    let failures = Rc::new(RefCell::new(0usize));
    let failures_out = failures.clone();

    app.connect_activate(move |app| {
        let window = gtk4::ApplicationWindow::new(app);
        window.set_default_size(900, 700);

        // The app's container, not a convenient one. See `NotebookPage::new`.
        let list = gtk4::ListBox::new();
        list.set_selection_mode(gtk4::SelectionMode::None);
        list.set_hexpand(true);

        let built: Rc<RefCell<Vec<(String, gtk4::Widget)>>> = Rc::new(RefCell::new(Vec::new()));
        for (name, outputs) in cases() {
            let cell = CodeCellWidget::new();
            cell.set_outputs(&outputs);
            let w: gtk4::Widget = cell.widget().clone().upcast();
            let row = gtk4::ListBoxRow::new();
            row.set_selectable(false);
            row.set_activatable(false);
            row.set_focusable(false);
            row.set_child(Some(&w));
            list.append(&row);
            built.borrow_mut().push((name, w));
            // The row owns the widget from here; the wrapper must not drop it.
            std::mem::forget(cell);
        }

        let scroller = gtk4::ScrolledWindow::new();
        scroller.set_vexpand(true);
        scroller.set_hexpand(true);
        scroller.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);
        scroller.set_child(Some(&list));
        window.set_child(Some(&scroller));
        window.present();

        let built = built.clone();
        let app = app.clone();
        let failures = failures.clone();
        // Let the layout settle before asking what anything was given.
        gtk4::glib::timeout_add_local_once(std::time::Duration::from_millis(600), move || {
            println!("\nOutput heights in the app's own container\n");
            for (name, w) in built.borrow().iter() {
                let (kind, height) = tallest_output(w);
                let verdict = if height < MIN_VISIBLE_HEIGHT {
                    *failures.borrow_mut() += 1;
                    "  <-- COLLAPSED, a reader sees nothing"
                } else {
                    ""
                };
                println!("  {name:30} {kind:18} {height:>4}px{verdict}");
            }
            app.quit();
        });
    });

    // No arguments: GTK would try to open them as files.
    let empty: [&str; 0] = [];
    app.run_with_args(&empty);

    let failures = *failures_out.borrow();
    if failures > 0 {
        println!("\n{failures} output(s) allocated less than {MIN_VISIBLE_HEIGHT}px.");
        std::process::exit(1);
    }
    println!("\nevery output has room to be seen");
}
