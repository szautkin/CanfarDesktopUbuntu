//! Does GTK actually find the agent icon by name?
//!
//! An icon file that exists is not an icon GTK can look up: symbolic icons live
//! in a directory the theme index has to declare, and a name GTK cannot resolve
//! falls back to a broken-image glyph without any error.
//!
//!     cargo run --example icon_render_probe
use gtk4::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

fn main() {
    let app = adw::Application::builder()
        .application_id("net.canfar.verbinal.iconprobe")
        .build();

    app.connect_activate(|app| {
        // The same registration the app does at startup.
        let display = gtk4::gdk::Display::default().expect("display");
        let theme = gtk4::IconTheme::for_display(&display);
        theme.add_search_path(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("assets")
                .join("icons"),
        );

        // `has_icon` is not enough, and this probe passed once while every load
        // failed with "Unrecognized image file format": the file was found and
        // could not be decoded, so the UI showed nothing and nothing errored.
        // Resolving the name and DECODING the bytes are two separate claims.
        let mut broken = Vec::new();
        for name in ["verbinal-agent-symbolic", "net.canfar.Verbinal"] {
            let found = theme.has_icon(name);
            let icon = theme.lookup_icon(
                name,
                &[],
                16,
                1,
                gtk4::TextDirection::None,
                gtk4::IconLookupFlags::empty(),
            );
            // Decode it the way the widget will.
            let loads = icon
                .file()
                .map(|f| gtk4::gdk::Texture::from_file(&f).is_ok())
                .unwrap_or(false);
            println!("{name:<28} resolves: {found}   loads: {loads}");
            if !found || !loads {
                broken.push(name);
            }
        }
        let missing = broken;

        // And that the flask is gone from the agent surfaces.
        let sources = [
            include_str!("../src/ui/agent_badge.rs"),
            include_str!("../src/ui/ai_guide_page.rs"),
            include_str!("../src/ui/main_window.rs"),
        ];
        let flask = sources
            .iter()
            .filter(|s| s.contains("applications-science-symbolic"))
            .count();
        println!("files still using the flask: {flask}");

        let win = adw::ApplicationWindow::builder().application(app).build();
        win.set_content(Some(&gtk4::Image::from_icon_name(
            "verbinal-agent-symbolic",
        )));
        win.present();

        gtk4::glib::timeout_add_local_once(std::time::Duration::from_millis(300), move || {
            let ok = missing.is_empty() && flask == 0;
            println!(
                "\n{}",
                if ok {
                    "PASS: GTK resolves the agent icon and no agent surface uses the flask"
                } else {
                    "FAIL"
                }
            );
            std::process::exit(if ok { 0 } else { 1 });
        });
    });

    app.run_with_args::<&str>(&[]);
}
