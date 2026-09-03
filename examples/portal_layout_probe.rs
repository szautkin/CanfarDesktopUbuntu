//! What each Portal card demands, measured on the REAL widgets.
//!
//! `gtk::init()` fails on a spawned thread and libtest runs every test on one,
//! so no `cargo test` can measure a layout — the guards in `dashboard.rs` can
//! only check the placement TABLES. This measures the actual cards.
//!
//! Run: `cargo run --features fits --example portal_layout_probe`
//!
//! The number that matters is each card's MINIMUM width. The Portal grid is
//! column-homogeneous, so every column is as wide as the widest column needs,
//! and a card that cannot shrink drags the whole grid past the window — where a
//! scroller with `hscrollbar_policy(Never)` clips it instead of scrolling.
//!
//! IMPORTANT: the cards are measured EMPTY. Nothing here signs in, so no
//! images, sessions or quota have loaded, and a card that grows with its
//! content measures smaller here than it will in the app. That is exactly how
//! the first run of this probe under-reported: it put the grid minimum at
//! 978px, while the real CANFAR Images card — carrying one filter button per
//! session type — pushed it past 1122px and clipped the Portal.
//!
//! So the last section below measures the known content-driven offender
//! directly, as a worst case, rather than pretending an empty card is the
//! answer.

use gtk4::prelude::*;
use gtk4::{self as gtk};
use verbinal::state::AppServices;
use verbinal::ui::batch_jobs_view::BatchJobsView;
use verbinal::ui::canfar_images::CanfarImagesView;
use verbinal::ui::platform_load::PlatformLoadView;
use verbinal::ui::recent_launches::RecentLaunchesView;
use verbinal::ui::session_list::SessionListView;
use verbinal::ui::storage_quota::StorageQuotaView;

/// Portal widths to report against, after the shell's sidebar and margins.
const PORTAL_WIDTHS: [i32; 4] = [500, 800, 1100, 1550];

/// The Portal grid is three homogeneous columns.
const COLUMNS: i32 = 3;

fn main() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let handle = rt.handle().clone();

    let app = gtk4::Application::builder()
        .application_id("net.canfar.Verbinal.PortalProbe")
        .build();

    app.connect_activate(move |app| {
        let (services, _rx) = AppServices::new(handle.clone());
        let window = gtk::Window::builder()
            .application(app)
            .default_width(1600)
            .default_height(900)
            .build();

        let session_list = SessionListView::new(services.clone());
        let storage = StorageQuotaView::new(services.clone());
        let batch = BatchJobsView::new(services.clone());
        let recents = RecentLaunchesView::new(services.clone());
        let load = PlatformLoadView::new(services.clone());
        let images = CanfarImagesView::new(services.clone());

        // Same spans as `dashboard::WIDE`.
        let cards: [(&str, &gtk::Box, i32); 6] = [
            ("Platform load", load.widget(), 1),
            ("Storage", storage.widget(), 1),
            ("Batch jobs", batch.widget(), 1),
            ("Active sessions", session_list.widget(), COLUMNS),
            ("CANFAR images", images.widget(), 2),
            ("Recent launches", recents.widget(), 1),
        ];

        // They must be realized inside a window to measure honestly.
        let holder = gtk::Box::new(gtk::Orientation::Vertical, 0);
        for (_, w, _) in cards.iter() {
            holder.append(*w);
        }
        window.set_child(Some(&holder));
        window.present();

        let owned: Vec<(String, gtk::Box, i32)> = cards
            .iter()
            .map(|(n, w, s)| (n.to_string(), (*w).clone(), *s))
            .collect();

        after(300, move || {
            println!("Portal card minimum widths (column-homogeneous, {COLUMNS} columns)\n");
            let mut worst_column = 0;
            for (name, w, span) in &owned {
                let (min, nat, _, _) = w.measure(gtk::Orientation::Horizontal, -1);
                // A card spanning N columns forces each column to min/N.
                let per_column = (min + span - 1) / span;
                worst_column = worst_column.max(per_column);
                println!(
                    "  {name:<17} span {span}  minimum {min:>5}px  natural {nat:>5}px  \
                     -> needs {per_column:>4}px per column"
                );
            }
            let grid_min = worst_column * COLUMNS;
            println!("\n  widest column demand : {worst_column}px");
            println!("  => grid minimum      : {grid_min}px\n");

            for portal in PORTAL_WIDTHS {
                let verdict = if grid_min <= portal {
                    "fits".to_string()
                } else {
                    format!("CLIPPED — {}px past the viewport", grid_min - portal)
                };
                println!("  Portal {portal:>5}px  ->  {verdict}");
            }
            println!(
                "\nA scroller with hscrollbar_policy(Never) does not scroll the overflow, \
                 it clips it."
            );

            // The content-driven worst case: the CANFAR Images filter bar, one
            // linked toggle per session type. Measured on its own because the
            // card above is empty here and will not show it.
            let bar = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            bar.add_css_class("linked");
            let mut group: Option<gtk::ToggleButton> = None;
            for label in [
                "Notebook",
                "Desktop",
                "Carta",
                "Contributed",
                "Firefly",
                "Headless",
                "Desktop-app",
            ] {
                let b = gtk::ToggleButton::with_label(label);
                match &group {
                    Some(first) => b.set_group(Some(first)),
                    None => group = Some(b.clone()),
                }
                bar.append(&b);
            }
            let holder = gtk::Box::new(gtk::Orientation::Vertical, 0);
            holder.append(&bar);
            let probe_win = gtk::Window::new();
            probe_win.set_child(Some(&holder));
            probe_win.present();
            let (bar_min, _, _, _) = bar.measure(gtk::Orientation::Horizontal, -1);
            println!(
                "\n  session-type filter bar, 7 buttons: {bar_min}px\n  \
                 unscrolled that is the CANFAR Images card's floor, and the card \
                 spans 2 of {COLUMNS} columns\n  -> it would demand \
                 {}px of grid on its own",
                ((bar_min + 1) / 2) * COLUMNS
            );
            probe_win.close();
            std::process::exit(0);
        });
    });

    app.run_with_args::<&str>(&[]);
}

fn after(ms: u64, f: impl FnOnce() + 'static) {
    let cell = std::cell::RefCell::new(Some(f));
    gtk4::glib::timeout_add_local(std::time::Duration::from_millis(ms), move || {
        if let Some(f) = cell.borrow_mut().take() {
            f();
        }
        gtk4::glib::ControlFlow::Break
    });
}
