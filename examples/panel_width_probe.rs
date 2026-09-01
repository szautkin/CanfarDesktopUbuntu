//! What every side panel needs, and what it actually gets.
//!
//!     cargo run --example panel_width_probe
//!
//! `gtk::init()` fails on a spawned thread and libtest runs every test in one,
//! so no `cargo test` can answer a layout question. That gap is how the Search
//! page shipped clipping its own right panel: at the app's default window width
//! the panel is allocated less than the minimum it asks for, so its rows' edit
//! and delete buttons fall outside the window, with nothing reporting a problem.
//!
//! This builds the REAL pages — not a mimicry of them, which would only measure
//! the mimicry — measures each one, allocates it at the widths that matter, and
//! walks the tree reporting what each top-level child was given against what it
//! asked for.
use gtk4::prelude::*;
use gtk4::{self as gtk};
use libadwaita as adw;
use std::sync::Arc;

/// Logical widths worth asking about.
///
/// 1200 is the app's own `default_width`, and on a 2x display it is also very
/// nearly a quarter of a 5K screen — the case this probe exists for. 1400 is
/// where the viewers' control column currently starts to dock. 1600 is a
/// comfortable half-screen.
const WIDTHS: &[i32] = &[1200, 1400, 1600];

/// What the shell takes before a page sees anything.
///
/// Measured on the running app: a window the window manager calls 1200 logical
/// px wide gives its top-level page 797. The difference is the navigation
/// sidebar, pinned to its 280 maximum at every width a page is usable at, plus
/// the client-side shadow the WM counts as part of the frame.
///
/// It matters because it is the gap between "the page fits in 920" — which it
/// does — and "the page fits in the window you actually opened", which it does
/// not.
const SHELL_CHROME: i32 = 403;

/// A page whose minimum needs more than this is not usable at a quarter screen.
///
/// 1200 logical is the app's own `default_width`, and on a 2x display it is
/// also very nearly a quarter of a 5K panel.
const BUDGET_WINDOW: i32 = 1200;

fn nat(w: &impl IsA<gtk::Widget>) -> (i32, i32) {
    let (min, natural, _, _) = w.as_ref().measure(gtk::Orientation::Horizontal, -1);
    (min, natural)
}

/// One line about a widget: what it asked for, and what it was given.
///
/// A child allocated less than its own minimum is the failure this probe is
/// for. GTK does not report it — it simply draws past the edge, which is why
/// the symptom was a button outside the window rather than a warning.
fn verdict(cmin: i32, cnat: i32, got: i32) -> String {
    if got == 0 {
        "not allocated".to_string()
    } else if got < cmin {
        format!("CLIPPED — {} px short of its own minimum", cmin - got)
    } else if cnat > got {
        format!("squeezed {} below natural", cnat - got)
    } else {
        "ok".to_string()
    }
}

/// Whether this widget, or anything under it, asks to expand horizontally.
///
/// GTK propagates expansion UPWARD, so a panel that never sets `hexpand` still
/// grows without limit if one label inside it does. That is not visible at the
/// panel's own call site, and it is why the Search page's right panel takes
/// half of every pixel a wider window brings.
fn expands(w: &gtk::Widget) -> bool {
    w.compute_expand(gtk::Orientation::Horizontal)
}

/// The widest a row in `root` would be if nothing ellipsized.
///
/// An ellipsizing label reports its minimum AS its natural width — that is what
/// ellipsizing means — so a list that truncates every row measures as though it
/// fits perfectly. Turning it off is the only way to ask what the content
/// actually wants, which is the number a list panel's width should come from.
///
/// Destructive, so it runs last: the labels stay un-ellipsized afterwards.
fn width_without_truncation(root: &gtk::Widget) -> (i32, i32, usize) {
    let mut widths: Vec<i32> = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(w) = stack.pop() {
        if let Ok(label) = w.clone().downcast::<gtk::Label>() {
            if label.ellipsize() != gtk4::pango::EllipsizeMode::None {
                label.set_ellipsize(gtk4::pango::EllipsizeMode::None);
            }
        }
        let mut child = w.first_child();
        while let Some(c) = child {
            child = c.next_sibling();
            stack.push(c);
        }
    }
    // Re-measure the rows, not the panel: the panel is inside a scroller that
    // will happily report a small minimum however wide its content is.
    let mut stack = vec![root.clone()];
    while let Some(w) = stack.pop() {
        if w.type_().name() == "GtkListBoxRow" || w.type_().name() == "AdwActionRow" {
            widths.push(nat(&w).1);
        }
        let mut child = w.first_child();
        while let Some(c) = child {
            child = c.next_sibling();
            stack.push(c);
        }
    }
    // The median, not only the maximum: a panel sized for its longest row is
    // sized for its rarest one, and one 641 px workflow title is not a reason
    // to give every list 641 px.
    widths.sort_unstable();
    let n = widths.len();
    if n == 0 {
        return (0, 0, 0);
    }
    (widths[n / 2], widths[n - 1], n)
}

/// Widgets that state a width AND expand — the contradiction this is all about.
///
/// A `set_size_request` says "I am this wide". Expanding says "give me
/// everything spare". A widget doing both is a panel that will be clipped when
/// the window is narrow and unbounded when it is wide, which is one missing
/// decision showing up as two different bugs.
///
/// Expansion is checked with `compute_expand`, not with the widget's own
/// `hexpand` flag, because GTK propagates it UPWARD: the Search panel never
/// sets `hexpand`, and one label inside it does.
fn contradictions(root: gtk::Widget) -> Vec<gtk::Widget> {
    let mut found = Vec::new();
    let mut stack = vec![root];
    while let Some(w) = stack.pop() {
        // Only things the code CALLED panels. Every guess from the outside — "a
        // width request between 200 and 400" — flagged a shrink floor, a
        // thumbnail or a hidden placeholder as well.
        if w.has_css_class(verbinal::ui::panel::MARKER) && expands(&w) {
            found.push(w.clone());
        }
        let mut child = w.first_child();
        while let Some(c) = child {
            child = c.next_sibling();
            stack.push(c);
        }
    }
    found
}

/// Walk into a widget one level, printing what each child asked for and got.
///
/// Deep enough to reach the panel and shallow enough to read: the split that
/// decides a page's layout is always its top-level box, or the `GtkPaned` that
/// box holds.
fn walk(w: &gtk::Widget, indent: &str, depth: usize) {
    let mut child = w.first_child();
    let mut i = 0;
    while let Some(c) = child {
        let (cmin, cnat) = nat(&c);
        println!(
            "{indent}[{i}] {:<20} min {cmin:>4}  nat {cnat:>5}  got {:>4}  {}{}",
            c.type_().name(),
            c.width(),
            if expands(&c) {
                "expands  "
            } else {
                "fixed    "
            },
            verdict(cmin, cnat, c.width())
        );
        // Into a Paned, because that is where a list-and-detail page decides
        // how much the list gets, and the decision is invisible from outside.
        if depth > 0 && c.type_().name() == "GtkPaned" {
            walk(&c, &format!("{indent}    "), depth - 1);
        }
        child = c.next_sibling();
        i += 1;
    }
}

/// Every top-level child of `page`, with what it asks for and what it got.
fn report(name: &str, page: &impl IsA<gtk::Widget>, page_w: i32) {
    let page = page.as_ref();
    let (min, natural) = nat(page);
    let fits = if min <= page_w {
        format!("fits, {} px of slack", page_w - min)
    } else {
        format!("TOO NARROW by {} px", min - page_w)
    };
    println!("  {name}: min {min}, natural {natural}, has {page_w} — {fits}");

    walk(page, "      ", 1);
}

/// Lay `page` out at `page_w` x 800 without putting a window on anyone's screen.
///
/// `allocate` directly rather than through a presented window: a probe that
/// flashed nine windows across the desktop is a probe nobody runs twice, and
/// the layout managers do not need a surface to divide a width.
fn lay_out(page: &impl IsA<gtk::Widget>, page_w: i32) {
    let page = page.as_ref();
    page.measure(gtk::Orientation::Horizontal, -1);
    page.measure(gtk::Orientation::Vertical, page_w);
    page.allocate(page_w, 800, -1, None);
}

fn main() {
    adw::init().expect("adw init");
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let (services, _rx) = verbinal::state::AppServices::new(rt.handle().clone());
    let app = adw::Application::builder()
        .application_id("net.canfar.Verbinal.probe")
        .build();
    let window = adw::ApplicationWindow::builder().application(&app).build();

    let pages: Vec<(&str, gtk::Widget)> = vec![
        (
            "search",
            verbinal::ui::search_page::SearchPage::new(Arc::clone(&services), window.clone())
                .widget()
                .clone()
                .upcast(),
        ),
        (
            "research",
            verbinal::ui::research_page::ResearchPage::new(Arc::clone(&services))
                .widget()
                .clone()
                .upcast(),
        ),
        (
            "workflows",
            verbinal::ui::workflows_page::WorkflowsPage::new(Arc::clone(&services))
                .widget()
                .clone()
                .upcast(),
        ),
    ];

    for width in WIDTHS {
        let page_w = width - SHELL_CHROME;
        println!("\nwindow {width} logical  →  page gets {page_w}");
        for (name, page) in &pages {
            lay_out(page, page_w);
            report(name, page, page_w);
        }
    }

    // ── The verdict ─────────────────────────────────────────────────────────
    //
    // Two questions, and a page has to answer both. "Does it fit?" is about the
    // window someone actually opens. "Does the panel stay put?" is about the
    // one they drag wider, where a panel that expands takes half of every pixel
    // and the picture, list or form it sits beside gets the other half.
    println!("\n── at the {BUDGET_WINDOW} logical window the app opens at ──");
    let budget = BUDGET_WINDOW - SHELL_CHROME;
    let mut failures = 0;
    for (name, page) in &pages {
        lay_out(page, budget);
        let (min, _) = nat(page);
        if min > budget {
            println!(
                "  {name}: needs {min}, has {budget} — CLIPS by {}",
                min - budget
            );
            failures += 1;
        } else {
            println!("  {name}: needs {min}, has {budget} — fits");
        }
    }

    println!("\n── panels that state a width and expand anyway ──");
    for (name, page) in &pages {
        lay_out(page, 2400);
        for w in contradictions(page.clone()) {
            println!(
                "  {name}: {} asks for {} px and took {} on a 2400 page",
                w.type_().name(),
                w.width_request(),
                w.width()
            );
            failures += 1;
        }
    }

    println!("\n── what a list row wants, with truncation switched off ──");
    for (name, page) in &pages {
        lay_out(page, 2400);
        let (median, widest, n) = width_without_truncation(page);
        if n > 0 {
            println!("  {name}: {n} rows — median wants {median} px, widest {widest} px");
        }
    }

    if failures == 0 {
        println!("\nevery page fits its window, and every panel keeps its width");
    } else {
        println!("\n{failures} panel(s) to fix");
        std::process::exit(1);
    }
}
