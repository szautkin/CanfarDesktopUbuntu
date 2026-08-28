//! End-to-end test of the SHIPPED `data/kernel_harness.py` — the exact script
//! the app spawns to run a notebook cell.
//!
//! The harness is Python, so nothing in `cargo test` reached it before and its
//! behaviour was only ever checked by hand. That is how a cell mixing a magic
//! with Python came to fail outright, and how `_strip_harness_frames` shipped
//! testing `"__file__" in dir()` from inside a function — where `dir()` lists
//! locals — so it never stripped a single frame and every notebook traceback
//! carried this file's internals above the user's own line.
//!
//! This drives the real script over the real protocol: NDJSON in, NDJSON out,
//! terminated by the boundary sentinel.
//!
//! Cases needing a scientific library (matplotlib, PIL, astropy) SKIP when it
//! is not installed rather than fail, so the suite is honest on a bare CI image
//! — and each one prints that it skipped, because a silent skip is how a test
//! stops testing anything without anyone noticing.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};

use serde_json::{json, Value};

/// Terminates the outputs of one execution. Must match the harness constant.
const BOUNDARY: &str = "\u{4}__CANFAR_EXEC_BOUNDARY__\u{4}";

/// A running harness process, driven one cell at a time.
struct Harness {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
    count: i64,
}

impl Harness {
    fn start() -> Self {
        Self::start_with_path(None)
    }

    /// Start the harness, optionally prepending `pythonpath` to its import path.
    ///
    /// The override exists so a test can put a module in front of whatever the
    /// machine has installed — `PYTHONPATH` is searched before site-packages,
    /// so a stand-in wins either way and the test result does not depend on
    /// what happens to be on the build host.
    fn start_with_path(pythonpath: Option<&std::path::Path>) -> Self {
        // `-u`: unbuffered, or the boundary sits in a pipe buffer and the read
        // below blocks forever.
        let mut command = Command::new("python3");
        command
            .arg("-u")
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/data/kernel_harness.py"
            ))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if let Some(path) = pythonpath {
            command.env("PYTHONPATH", path);
        }
        let mut child = command.spawn().expect("spawn python3 kernel_harness.py");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Self {
            child,
            stdin,
            stdout,
            count: 0,
        }
    }

    /// Execute `code` and return every output up to the boundary.
    fn run(&mut self, code: &str) -> Vec<Value> {
        self.count += 1;
        let req = json!({"type": "execute", "code": code, "exec_count": self.count});
        writeln!(self.stdin, "{req}").expect("write request");
        self.stdin.flush().unwrap();

        let mut outputs = Vec::new();
        loop {
            let mut line = String::new();
            let read = self
                .stdout
                .read_line(&mut line)
                .expect("read harness output");
            assert!(read > 0, "harness closed its stdout mid-cell");
            if line.contains(BOUNDARY) {
                return outputs;
            }
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                outputs.push(serde_json::from_str(trimmed).unwrap_or_else(|e| {
                    panic!("harness emitted a non-JSON line {trimmed:?}: {e}")
                }));
            }
        }
    }

    /// Whether `module` is really installed, so a case can skip instead of fail.
    ///
    /// `PathFinder`, not `importlib.util.find_spec`: the latter walks
    /// `sys.meta_path`, where the harness installs its own `IPython` stand-in —
    /// so asking it "is IPython installed?" got back the harness's own answer,
    /// and BOTH tests that depend on IPython being absent skipped silently
    /// while reporting green. `PathFinder` searches `sys.path` only, which is
    /// the actual question: is this module on disk?
    fn has_module(&mut self, module: &str) -> bool {
        let outputs = self.run(&format!(
            "from importlib.machinery import PathFinder as _P\n\
             print('YES' if _P.find_spec('{module}') else 'NO')"
        ));
        outputs
            .iter()
            .any(|o| o["text"].as_str().is_some_and(|t| t.contains("YES")))
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = writeln!(self.stdin, r#"{{"type":"quit"}}"#);
        let _ = self.stdin.flush();
        let _ = self.child.wait();
    }
}

/// One output as `kind:detail`, so a whole cell is comparable in one line.
fn shape(output: &Value) -> String {
    match output["output_type"].as_str().unwrap_or("?") {
        "stream" => format!(
            "stream:{}={}",
            output["name"].as_str().unwrap_or("?"),
            output["text"].as_str().unwrap_or("")
        ),
        "error" => format!("error:{}", output["ename"].as_str().unwrap_or("?")),
        kind => {
            let mut keys: Vec<&str> = output["data"]
                .as_object()
                .map(|m| m.keys().map(String::as_str).collect())
                .unwrap_or_default();
            keys.sort_unstable();
            format!("{kind}:{}", keys.join(","))
        }
    }
}

fn shapes(outputs: &[Value]) -> Vec<String> {
    outputs.iter().map(shape).collect()
}

// ---------------------------------------------------------------------------
// The protocol shapes that already worked, and must keep working
// ---------------------------------------------------------------------------

#[test]
fn the_core_protocol_shapes_are_unchanged() {
    let mut h = Harness::start();

    assert_eq!(shapes(&h.run("print('hi')")), ["stream:stdout=hi\n"]);
    assert_eq!(shapes(&h.run("40 + 2")), ["execute_result:text/plain"]);
    // An assignment is not an expression, so it displays nothing.
    assert!(shapes(&h.run("x = 5")).is_empty());
    // ...and the namespace persists between cells.
    assert_eq!(shapes(&h.run("x")), ["execute_result:text/plain"]);
    assert_eq!(shapes(&h.run("None")), Vec::<String>::new());
    assert_eq!(shapes(&h.run("1/0")), ["error:ZeroDivisionError"]);
    assert_eq!(
        shapes(&h.run("print('p')\n1/0")),
        ["stream:stdout=p\n", "error:ZeroDivisionError"]
    );
    // The value of the LAST statement, not only of a single-expression cell.
    assert_eq!(
        shapes(&h.run("a = 1\nb = 2\na + b")),
        ["execute_result:text/plain"]
    );
}

// ---------------------------------------------------------------------------
// Magic and Python in one cell — order, and that both halves run
// ---------------------------------------------------------------------------

#[test]
fn a_cell_mixing_magic_and_python_runs_both_in_source_order() {
    let mut h = Harness::start();

    // This is the reported defect: `%pip --version` followed by Python was a
    // syntax error and NEITHER half ran.
    let out = shapes(&h.run("!echo shelled\nprint('python ran too')"));
    assert_eq!(
        out,
        ["stream:stdout=shelled\n", "stream:stdout=python ran too\n"]
    );

    // Order is source order in both directions. Hoisting the magic to the front
    // fixed the syntax error but printed `second` before `first`.
    assert_eq!(
        shapes(&h.run("print('first')\n!echo second")),
        ["stream:stdout=first\n", "stream:stdout=second\n"]
    );
    assert_eq!(
        shapes(&h.run("print('a')\n!echo b\nprint('c')")),
        [
            "stream:stdout=a\n",
            "stream:stdout=b\n",
            "stream:stdout=c\n"
        ]
    );

    // A raise stops the cell, as it does in Jupyter: the later magic must not
    // run just because it is handled outside the interpreter.
    assert_eq!(
        shapes(&h.run("1/0\n!echo should_not_run")),
        ["error:ZeroDivisionError"]
    );

    // Only the cell's FINAL statement produces a value — an expression split
    // off from the rest by a magic line is still mid-cell.
    //
    // The second case cannot distinguish `want_value` on the last segment from
    // `want_value` on every segment: measured, the two are identical, because
    // each code segment reassigns the result. It pins the BEHAVIOUR, which is
    // what a reader depends on, not the mechanism that currently delivers it.
    assert_eq!(
        shapes(&h.run("!echo m\n1+1")),
        ["stream:stdout=m\n", "execute_result:text/plain"]
    );
    assert_eq!(shapes(&h.run("1+1\n!echo m\nx = 3")), ["stream:stdout=m\n"]);

    // A cell that is nothing but magic still works.
    assert_eq!(shapes(&h.run("!echo alone")), ["stream:stdout=alone\n"]);
}

#[test]
fn a_traceback_starts_at_the_users_own_line() {
    let mut h = Harness::start();

    // Blanking magic lines rather than removing them keeps the line numbers, so
    // the traceback points where the user is looking.
    let outputs = h.run("!echo one\nx = 1\n!echo two\nraise ValueError('here')");
    let error = outputs
        .iter()
        .find(|o| o["output_type"] == "error")
        .expect("an error output");
    let tb = error["traceback"]
        .as_array()
        .expect("traceback lines")
        .iter()
        .map(|v| v.as_str().unwrap_or("").to_string())
        .collect::<Vec<_>>();

    assert!(
        tb.iter().any(|l| l.contains(r#"File "<cell>", line 4"#)),
        "the raise is on line 4: {tb:#?}"
    );
    assert!(
        !tb.iter().any(|l| l.contains("kernel_harness.py")),
        "the harness's own frames leaked into a user traceback: {tb:#?}"
    );

    // A SyntaxError arrives through `ast.parse`, so its machinery frames are
    // stdlib rather than harness — no filename check would have caught them.
    let outputs = h.run("def broken(:");
    let error = outputs
        .iter()
        .find(|o| o["output_type"] == "error")
        .expect("an error output");
    let tb: Vec<String> = error["traceback"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap_or("").to_string())
        .collect();
    assert_eq!(error["ename"], "SyntaxError");
    assert!(
        !tb.iter().any(|l| l.contains("ast.py")),
        "stdlib compile frames leaked into a syntax error: {tb:#?}"
    );

    // Frames BELOW the user's line are kept: a failure inside a library is how
    // they will find the problem.
    let outputs = h.run("import json\njson.loads('{oops')");
    let error = outputs
        .iter()
        .find(|o| o["output_type"] == "error")
        .unwrap();
    let tb: Vec<String> = error["traceback"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap_or("").to_string())
        .collect();
    assert!(
        tb.iter().any(|l| l.contains("json/decoder.py")),
        "library frames were stripped too: {tb:#?}"
    );
}

// ---------------------------------------------------------------------------
// The display protocol
// ---------------------------------------------------------------------------

#[test]
fn display_emits_one_bundle_per_object_in_order_with_print() {
    let mut h = Harness::start();

    // `display()` is injected into the namespace, so it works without IPython.
    let out = shapes(&h.run("display('one', 'two')"));
    assert_eq!(out, ["display_data:text/plain", "display_data:text/plain"]);

    // It emits immediately while `print` output is captured until the cell
    // ends, so without draining the buffer these arrive back-to-front.
    assert_eq!(
        shapes(&h.run("print('before')\ndisplay('mid')\nprint('after')")),
        [
            "stream:stdout=before\n",
            "display_data:text/plain",
            "stream:stdout=after\n"
        ]
    );
}

#[test]
fn an_object_is_asked_how_it_wants_to_be_shown() {
    let mut h = Harness::start();

    // A class defined in the cell itself: no dependency, and it pins the
    // protocol rather than any one library's use of it.
    let out = shapes(&h.run(
        "class Rich:\n\
         \x20   def _repr_html_(self): return '<b>rich</b>'\n\
         \x20   def __repr__(self): return 'Rich()'\n\
         Rich()",
    ));
    assert_eq!(out, ["execute_result:text/html,text/plain"]);

    // text/plain is ALWAYS present, so a client that renders nothing else can
    // still show something.
    let outputs = h.run("class P:\n    def _repr_png_(self): return b'\\x89PNG'\nP()");
    let data = &outputs[0]["data"];
    assert!(data["image/png"].is_string(), "png bundled: {data}");
    assert!(
        data["text/plain"].is_string(),
        "text/plain always present: {data}"
    );

    // A `_repr_*_` that raises must not take the whole output with it.
    let out = shapes(&h.run(
        "class Bad:\n\
         \x20   def _repr_html_(self): raise RuntimeError('boom')\n\
         \x20   def __repr__(self): return 'Bad()'\n\
         Bad()",
    ));
    assert_eq!(out, ["execute_result:text/plain"]);

    // One returning None is declining to be shown that way, not an error.
    let out = shapes(&h.run(
        "class Nope:\n\
         \x20   def _repr_html_(self): return None\n\
         Nope()",
    ));
    assert_eq!(out, ["execute_result:text/plain"]);
}

#[test]
fn the_scientific_stack_renders_richly_when_installed() {
    let mut h = Harness::start();

    if h.has_module("matplotlib") {
        let out = shapes(&h.run(
            "import matplotlib; matplotlib.use('Agg')\n\
             import matplotlib.pyplot as plt\n\
             plt.plot([1,2,3])",
        ));
        assert!(
            out.iter().any(|s| s.contains("image/png")),
            "a figure should produce a PNG: {out:?}"
        );
    } else {
        eprintln!("SKIPPED: matplotlib not installed");
    }

    if h.has_module("PIL") {
        // Before the display protocol this came back as `<PIL.Image.Image ...>`.
        let out = shapes(&h.run("from PIL import Image\nImage.new('RGB',(4,4),'red')"));
        assert!(
            out.iter().any(|s| s.contains("image/png")),
            "a PIL image has _repr_png_: {out:?}"
        );
    } else {
        eprintln!("SKIPPED: PIL not installed");
    }

    if h.has_module("astropy") {
        // The headline case: a table arrived as its repr with the HTML unread.
        let out = shapes(&h.run("from astropy.table import Table\nTable({'a':[1,2]})"));
        assert!(
            out.iter().any(|s| s.contains("text/html")),
            "an astropy Table has _repr_html_: {out:?}"
        );
    } else {
        eprintln!("SKIPPED: astropy not installed");
    }
}

// ---------------------------------------------------------------------------
// IPython.display
// ---------------------------------------------------------------------------

#[test]
fn ipython_display_imports_work_without_ipython_installed() {
    let mut h = Harness::start();
    if h.has_module("IPython") {
        eprintln!("SKIPPED: IPython is installed, so the stand-in path is not the one taken");
        return;
    }

    // The import line itself is the first line of a great many notebook cells.
    assert!(
        shapes(&h.run("from IPython.display import HTML, Image, Markdown, display")).is_empty()
    );

    // Each wrapper answers under its own MIME type.
    for (code, mime) in [
        ("HTML('<b>b</b>')", "text/html"),
        ("Markdown('# h')", "text/markdown"),
        ("Latex(r'\\alpha')", "text/latex"),
        ("SVG('<svg/>')", "image/svg+xml"),
        ("JSON({'a': 1})", "application/json"),
    ] {
        let cell = format!("from IPython.display import *\n{code}");
        let out = shapes(&h.run(&cell));
        assert!(
            out.iter().any(|s| s.contains(mime)),
            "{code} should produce {mime}: {out:?}"
        );
    }

    // `Image` sniffs the format from the bytes rather than trusting a caller.
    let out = shapes(
        &h.run("from IPython.display import Image\nImage(b'\\x89PNG\\r\\n\\x1a\\n' + b'rest')"),
    );
    assert!(out.iter().any(|s| s.contains("image/png")), "{out:?}");

    // A URL is refused rather than silently fetched: a cell that reaches the
    // network should say so, especially on someone else's cluster.
    let out = shapes(&h.run("from IPython.display import Image\nImage(url='http://x/y.png')"));
    assert_eq!(out, ["error:ValueError"]);

    // `clear_output` has nothing to clear, but a cell calling it must not fail.
    assert_eq!(
        shapes(&h.run("from IPython.display import clear_output\nclear_output()\nprint('ok')")),
        ["stream:stdout=ok\n"]
    );

    // The text/plain fallback is the source, not `<Markdown object>` — for the
    // types nothing renders yet, that fallback IS what the reader sees.
    let outputs = h.run("from IPython.display import Markdown\nMarkdown('# heading')");
    assert_eq!(outputs[0]["data"]["text/plain"], "# heading");
}

#[test]
fn an_installed_ipython_has_its_display_routed_here() {
    // A stand-in with the real library's out-of-a-kernel behaviour: `display()`
    // PRINTS a repr, because there is no kernel to publish to. This harness is
    // the kernel, so it must publish instead — and that branch is not otherwise
    // reachable on a machine without IPython, which includes CI.
    let dir = std::env::temp_dir().join(format!("verbinal-ipython-{}", std::process::id()));
    let package = dir.join("IPython");
    std::fs::create_dir_all(&package).expect("create stand-in package");
    std::fs::write(package.join("__init__.py"), "__version__ = '0-stand-in'\n").unwrap();
    std::fs::write(
        package.join("display.py"),
        "def display(*objs, **kw):\n\
         \x20   for o in objs:\n\
         \x20       print(repr(o))\n\
         \n\
         class HTML:\n\
         \x20   def __init__(self, data=None): self.data = data\n\
         \x20   def _repr_html_(self): return self.data\n\
         \x20   def __repr__(self): return '<IPython.core.display.HTML object>'\n",
    )
    .unwrap();

    let mut h = Harness::start_with_path(Some(&dir));
    assert!(h.has_module("IPython"), "the stand-in should be importable");

    // Published as display_data — NOT printed as a stream of its repr, which is
    // what the unpatched library does outside a kernel.
    let out =
        shapes(&h.run("from IPython.display import display, HTML\ndisplay(HTML('<b>x</b>'))"));
    assert_eq!(out, ["display_data:text/html,text/plain"]);

    // The library's own classes are left alone: they already carry the standard
    // `_repr_*_` methods, and rebinding them would be the rude version.
    let out = shapes(&h.run("from IPython.display import HTML\nHTML('<i>y</i>')"));
    assert_eq!(out, ["execute_result:text/html,text/plain"]);

    drop(h);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_shim_is_invisible_to_a_notebook_that_never_asks_for_it() {
    // Registering the stand-in at startup broke matplotlib outright: `pyplot`
    // reads `sys.modules.get("IPython")` to decide whether it is in a notebook,
    // found the stand-in, and called `get_ipython()` on it — so every figure
    // cell died with "module 'IPython' has no attribute 'get_ipython'".
    //
    // Two invariants came out of that, and this pins both.
    let mut h = Harness::start();
    if h.has_module("IPython") {
        eprintln!("SKIPPED: IPython is installed, so its absence cannot be observed");
        return;
    }

    // One: nothing appears in sys.modules until a cell actually imports it.
    let outputs = h.run("import sys\nprint('IPython' in sys.modules)");
    assert_eq!(
        shapes(&outputs),
        ["stream:stdout=False\n"],
        "the stand-in registered itself without being asked"
    );

    // Two: once it IS asked for, it answers a library's questions truthfully
    // rather than raising. `get_ipython()` is None because no shell is running.
    let outputs =
        h.run("from IPython.display import HTML\nimport IPython\nprint(IPython.get_ipython())");
    assert_eq!(shapes(&outputs), ["stream:stdout=None\n"]);

    // And matplotlib still works AFTER the shim has been imported, which is the
    // combination that actually failed.
    if h.has_module("matplotlib") {
        let out = shapes(&h.run(
            "import matplotlib; matplotlib.use('Agg')\n\
             import matplotlib.pyplot as plt\n\
             plt.plot([1,2,3])",
        ));
        assert!(
            out.iter().any(|s| s.contains("image/png")),
            "matplotlib broke once the IPython stand-in was present: {out:?}"
        );
    } else {
        eprintln!("SKIPPED: matplotlib not installed");
    }
}

#[test]
fn plt_show_draws_the_figure_instead_of_warning_that_it_cannot() {
    let mut h = Harness::start();
    if !h.has_module("matplotlib") {
        eprintln!("SKIPPED: matplotlib not installed");
        return;
    }

    let outputs = h.run(
        "import matplotlib; matplotlib.use('Agg')\n\
         import matplotlib.pyplot as plt\n\
         plt.plot([3,2,1])\n\
         plt.show()",
    );
    let out = shapes(&outputs);

    // The Agg backend cannot open a window, so the real `show()` warned
    // "FigureCanvasAgg is non-interactive, and thus cannot be shown" — and the
    // figure then appeared anyway at the end of the cell. The reader got a
    // picture AND a warning saying a picture could not be shown.
    assert!(
        !out.iter().any(|s| s.contains("non-interactive")),
        "plt.show() still warns that it cannot show: {out:?}"
    );
    // Exactly one figure: `show()` renders and closes, so the end-of-cell
    // sweep finds nothing left to draw twice.
    assert_eq!(
        out.iter().filter(|s| s.contains("image/png")).count(),
        1,
        "expected exactly one figure: {out:?}"
    );

    // And it lands where the call is, not after everything the cell printed.
    let out = shapes(&h.run(
        "import matplotlib.pyplot as plt\n\
         print('before')\n\
         plt.plot([1,2])\n\
         plt.show()\n\
         print('after')",
    ));
    let png_at = out.iter().position(|s| s.contains("image/png"));
    let after_at = out.iter().position(|s| s.contains("after"));
    assert!(
        matches!((png_at, after_at), (Some(p), Some(a)) if p < a),
        "the figure should precede what was printed after it: {out:?}"
    );
}
