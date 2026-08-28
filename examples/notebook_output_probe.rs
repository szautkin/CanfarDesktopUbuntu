//! What a notebook cell actually BUILDS for each kind of output.
//!
//!     cargo run --example notebook_output_probe
//!
//! `OutputData` has modelled `text/html` and `image/jpeg` since the parser was
//! written, and the renderer showed neither: it asked "is there a PNG?", then
//! "is there text?", and stopped. So an `astropy.table.Table` — the output an
//! astronomer looks at most — arrived as its `repr()` with the HTML sitting
//! unread in the same bundle.
//!
//! The lesson that produced this probe is that unit tests on `OutputData` would
//! not have caught it. The model was right the whole time; the call site never
//! asked. So this drives the real `CodeCellWidget` with real captured bundles
//! and walks the widget tree it produces — the thing a user would actually see,
//! without needing a kernel or a window.
//!
//! That whole bug class is now closed structurally as well: the widget matches
//! `Representation` exhaustively, so a MIME type added to `richest` will not
//! compile until it has somewhere to go. This probe answers the question the
//! compiler cannot — whether the branch builds the right thing.
//!
//! It does NOT answer whether the user can SEE it, and that distinction has
//! already cost one round: this probe reported a `GtkPicture` for every image
//! while a reader was looking at blank cells, because the picture was built
//! correctly and then allocated one pixel of height. It lays widgets out in a
//! plain box with room to spare, which hands out natural sizes and hides the
//! problem entirely. `examples/notebook_layout_probe.rs` uses the container the
//! app actually uses and asks what each output was ALLOCATED. Run both.
use gtk4::prelude::*;
use verbinal::models::notebook_document::CellOutput;
use verbinal::ui::notebook_cell::CodeCellWidget;

/// Captured from `astropy.table.Table({'name':…,'ra':…})._repr_html_()`.
const ASTROPY_HTML: &str = r#"<div><i>Table length=2</i>
<table id="table1" class="table-striped">
<thead><tr><th>name</th><th>ra</th></tr></thead>
<thead><tr><th>str3</th><th>float64</th></tr></thead>
<tr><td>M31</td><td>10.68</td></tr>
<tr><td>M51</td><td>202.5</td></tr>
</table></div>"#;

/// A 1×1 PNG, and the same pixel as a JPEG.
const PNG_1PX: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";
const JPEG_1PX: &str = "/9j/4AAQSkZJRgABAQEAYABgAAD/2wBDAAgGBgcGBQgHBwcJCQgKDBQNDAsLDBkSEw8UHRofHh0aHBwgJC4nICIsIxwcKDcpLDAxNDQ0Hyc5PTgyPC4zNDL/wAALCAABAAEBAREA/8QAFAABAAAAAAAAAAAAAAAAAAAACf/EABQQAQAAAAAAAAAAAAAAAAAAAAD/2gAIAQEAAD8AKp//2Q==";

/// One probe case: a bundle, and what a reader should end up looking at.
struct Case {
    what: &'static str,
    bundle: &'static str,
    expect: &'static str,
}

const CASES: &[Case] = &[
    Case {
        what: "astropy Table (text/html)",
        bundle: r#"{"output_type":"display_data","data":{"text/html":"HTML","text/plain":"<Table length=2>"},"metadata":{}}"#,
        expect: "a table, not <Table length=2>",
    },
    Case {
        what: "matplotlib figure (image/png)",
        bundle: r#"{"output_type":"display_data","data":{"image/png":"PNG","text/plain":"<Figure size 640x480>"},"metadata":{}}"#,
        expect: "a picture",
    },
    Case {
        what: "PIL image (image/jpeg only)",
        bundle: r#"{"output_type":"display_data","data":{"image/jpeg":"JPEG","text/plain":"<PIL.Image.Image>"},"metadata":{}}"#,
        expect: "a picture",
    },
    Case {
        what: "print() (stream)",
        bundle: r#"{"output_type":"stream","name":"stdout","text":"hello\n"}"#,
        expect: "one label",
    },
    Case {
        what: "plain repr (text/plain)",
        bundle: r#"{"output_type":"execute_result","execution_count":1,"data":{"text/plain":"42"},"metadata":{}}"#,
        expect: "one label",
    },
    Case {
        what: "svg (image/svg+xml)",
        bundle: r#"{"output_type":"display_data","data":{"image/svg+xml":"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"16\" height=\"16\"><rect width=\"16\" height=\"16\" fill=\"red\"/></svg>","text/plain":"<S object at 0x7f>"},"metadata":{}}"#,
        expect: "a picture, not an object address",
    },
    Case {
        what: "markdown (text/markdown)",
        // `r##"…"##`: the markdown heading contains `"#`, which closes a
        // single-hash raw string mid-JSON.
        bundle: r##"{"output_type":"display_data","data":{"text/markdown":"# Heading","text/plain":"<M object at 0x7f>"},"metadata":{}}"##,
        expect: "rendered markdown, not an object address",
    },
    Case {
        what: "latex (text/latex)",
        bundle: r#"{"output_type":"display_data","data":{"text/latex":"$\\alpha$","text/plain":"<L object at 0x7f>"},"metadata":{}}"#,
        expect: "the source (no renderer yet), not an object address",
    },
    Case {
        what: "json (application/json)",
        bundle: r#"{"output_type":"display_data","data":{"application/json":{"a":1,"b":[2,3]},"text/plain":"<J object at 0x7f>"},"metadata":{}}"#,
        expect: "pretty-printed json, not an object address",
    },
    Case {
        what: "corrupt image bytes",
        bundle: r#"{"output_type":"display_data","data":{"image/png":"bm90IGFuIGltYWdl","text/plain":"<Figure>"},"metadata":{}}"#,
        expect: "falls back to the text, never blank",
    },
];

/// Every widget in the output area, as `Type[detail]`, outermost first.
fn describe(widget: &gtk4::Widget) -> Vec<String> {
    let mut out = Vec::new();
    let mut child = widget.first_child();
    while let Some(w) = child {
        let kind = w.type_().name().to_string();
        let detail = if let Some(label) = w.downcast_ref::<gtk4::Label>() {
            let text = label.text();
            let text = text.replace('\n', "\\n");
            format!("\"{}\"", &text[..text.len().min(40)])
        } else if let Some(grid) = w.downcast_ref::<gtk4::Grid>() {
            let cells: Vec<String> = {
                let mut v = Vec::new();
                let mut c = grid.first_child();
                while let Some(cell) = c {
                    if let Some(l) = cell.downcast_ref::<gtk4::Label>() {
                        v.push(l.text().to_string());
                    }
                    c = cell.next_sibling();
                }
                v
            };
            format!("{} cells: {:?}", cells.len(), cells)
        } else if w.downcast_ref::<gtk4::Picture>().is_some() {
            "image".to_string()
        } else {
            String::new()
        };

        if w.downcast_ref::<gtk4::ScrolledWindow>().is_some() || w.first_child().is_some() {
            let inner = describe(&w);
            if !inner.is_empty() {
                out.push(format!("{kind} → {}", inner.join(" + ")));
                child = w.next_sibling();
                continue;
            }
        }
        out.push(if detail.is_empty() {
            kind
        } else {
            format!("{kind}[{detail}]")
        });
        child = w.next_sibling();
    }
    out
}

/// The output area of a freshly built cell.
fn output_area(cell: &CodeCellWidget) -> gtk4::Widget {
    // The outputs hang off the cell's own box; find the container that the
    // outputs were appended to by walking to the last child that has children.
    fn deepest_with_children(w: &gtk4::Widget) -> Option<gtk4::Widget> {
        let mut child = w.last_child();
        while let Some(c) = child {
            if c.first_child().is_some() {
                return Some(c);
            }
            child = c.prev_sibling();
        }
        None
    }
    let root: gtk4::Widget = cell.widget().clone().upcast();
    deepest_with_children(&root).unwrap_or(root)
}

/// Run the REAL harness and render what it actually emits.
///
/// The cases above use captured bundles, which proves the renderer but assumes
/// the harness still produces that shape. This closes the loop: python runs,
/// astropy builds a table, and the bundle that comes back is the one the widget
/// is handed — no transcription in between.
///
/// Prints and returns rather than failing when python or astropy is missing;
/// the captured cases already cover the renderer on their own.
fn live_end_to_end() {
    println!("Live: astropy through the real harness\n");
    let cell_code =
        "from astropy.table import Table\nTable({'name':['M31','M51'],'ra':[10.68,202.5]})";
    let script = format!(
        "import json,subprocess,sys\n\
         B='\\x04__CANFAR_EXEC_BOUNDARY__\\x04'\n\
         try:\n\
        \x20 import astropy\n\
         except ImportError:\n\
        \x20 print('SKIP astropy not installed'); sys.exit(0)\n\
         p=subprocess.Popen([sys.executable,'-u','data/kernel_harness.py'],\n\
        \x20 stdin=subprocess.PIPE,stdout=subprocess.PIPE,text=True,bufsize=1)\n\
         p.stdin.write(json.dumps({{'type':'execute','code':{code},'exec_count':1}})+chr(10))\n\
         p.stdin.flush()\n\
         outs=[]\n\
         while True:\n\
        \x20 l=p.stdout.readline()\n\
        \x20 if not l or B in l: break\n\
        \x20 if l.strip(): outs.append(json.loads(l))\n\
         p.stdin.write('{{\"type\":\"quit\"}}'+chr(10)); p.stdin.flush(); p.wait(timeout=10)\n\
         print(json.dumps(outs[0]) if outs else 'SKIP no output')\n",
        code = serde_json::to_string(cell_code).unwrap()
    );

    let out = match std::process::Command::new("python3")
        .arg("-c")
        .arg(&script)
        .output()
    {
        Ok(o) => o,
        Err(e) => {
            println!("  SKIP: python3 not runnable: {e}\n");
            return;
        }
    };
    let stdout = String::from_utf8_lossy(&out.stdout);
    let line = stdout.trim();
    if line.starts_with("SKIP") || line.is_empty() {
        println!("  {line}\n");
        return;
    }

    match serde_json::from_str::<CellOutput>(line) {
        Ok(output) => {
            let cell = CodeCellWidget::new();
            cell.set_outputs(std::slice::from_ref(&output));
            for part in describe(&output_area(&cell)) {
                println!("  {part}");
            }
            println!();
        }
        Err(e) => println!("  the harness bundle did not parse as a CellOutput: {e}\n"),
    }
}

fn main() {
    gtk4::init().expect("gtk init");

    let mut failures = 0;
    println!("\nWhat a notebook cell builds for each output bundle\n");
    for case in CASES {
        let json = case
            .bundle
            .replace(
                "HTML",
                &ASTROPY_HTML.replace('"', "\\\"").replace('\n', "\\n"),
            )
            .replace("PNG", PNG_1PX)
            .replace("JPEG", JPEG_1PX);
        let output: CellOutput = match serde_json::from_str(&json) {
            Ok(o) => o,
            Err(e) => {
                println!("  {:32} PARSE FAILED: {e}", case.what);
                failures += 1;
                continue;
            }
        };

        let cell = CodeCellWidget::new();
        cell.set_outputs(std::slice::from_ref(&output));
        let tree = describe(&output_area(&cell));

        println!(
            "  {:32} {}",
            case.what,
            tree.join(&format!("\n{}", " ".repeat(36)))
        );
        println!("  {:32} expected: {}\n", "", case.expect);
        if tree.is_empty() {
            println!("  {:32} ^^ NOTHING RENDERED\n", "");
            failures += 1;
        }
    }

    live_end_to_end();

    if failures > 0 {
        println!("{failures} case(s) rendered nothing.");
        std::process::exit(1);
    }
}
