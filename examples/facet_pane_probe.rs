//! What the image-discovery dialog's filter pane demands as a minimum.
//!
//! GTK must be initialised on the MAIN thread and libtest runs every test in a
//! spawned one, so no `cargo test` can answer a layout question.
//!
//!     cargo run --example facet_pane_probe
//!
//! The pane is set to 380px. A child whose MINIMUM exceeds that cannot be
//! allocated its own size, and the content overflows the modal's left edge —
//! which is what "Active filters" rendering as "ve filters" looks like.
use gtk4::prelude::*;

/// The longest facet label a real catalogue produces.
const WORST: &str = "22.04.5 LTS (Jammy Jellyfish)  ·  0";

fn minimum_of(build: fn() -> gtk4::Widget) -> i32 {
    // As the dialog nests it: label/check inside a Box inside a ScrolledWindow
    // whose horizontal policy is Never — which passes the child's minimum
    // straight through rather than absorbing it.
    let column = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
    column.append(&build());
    let scroll = gtk4::ScrolledWindow::new();
    scroll.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);
    scroll.set_child(Some(&column));

    let window = gtk4::Window::new();
    window.set_child(Some(&scroll));
    let (min, _, _, _) = scroll.measure(gtk4::Orientation::Horizontal, -1);
    min
}

fn plain() -> gtk4::Widget {
    gtk4::CheckButton::with_label(WORST).upcast()
}

fn ellipsized() -> gtk4::Widget {
    let label = gtk4::Label::new(Some(WORST));
    label.set_ellipsize(gtk4::pango::EllipsizeMode::End);
    label.set_max_width_chars(26);
    label.set_halign(gtk4::Align::Start);
    label.set_xalign(0.0);
    let check = gtk4::CheckButton::new();
    check.set_child(Some(&label));
    check.upcast()
}

/// A right-pane row as the dialog builds it: an ExpanderRow carrying two
/// buttons in its suffix.
fn image_row() -> gtk4::Widget {
    use libadwaita::prelude::*;
    let row = libadwaita::ExpanderRow::builder()
        .title("casa-6/casa:6.1.1-15-pipeline")
        .subtitle("382 packages · Jul 2")
        .build();
    let use_btn = gtk4::Button::with_label("Use this image");
    use_btn.add_css_class("suggested-action");
    let rediscover = gtk4::Button::with_label("Rediscover");
    row.add_suffix(&use_btn);
    row.add_suffix(&rediscover);
    let list = gtk4::ListBox::new();
    list.add_css_class("boxed-list");
    list.append(&row);
    list.upcast()
}

/// The left pane as the dialog builds it, with realistic content.
fn left_pane(ellipsize: bool) -> (i32, i32) {
    let left = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
    left.set_margin_start(12);
    left.set_margin_end(12);

    let search = gtk4::SearchEntry::new();
    search.set_placeholder_text(Some("Filter packages…"));
    left.append(&search);

    let header = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
    let title = gtk4::Label::new(Some("Active filters"));
    title.set_hexpand(true);
    title.set_halign(gtk4::Align::Start);
    header.append(&title);
    header.append(&gtk4::Button::with_label("Clear all"));
    left.append(&header);

    // A chip, as `chips_box` builds them.
    let chips = gtk4::FlowBox::new();
    chips.set_selection_mode(gtk4::SelectionMode::None);
    chips.set_max_children_per_line(20);
    for text in [
        "OS family: centos",
        "OS version: 22.04.5 LTS (Jammy Jellyfish)",
    ] {
        let chip = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
        chip.append(&gtk4::Label::new(Some(text)));
        chip.append(&gtk4::Button::from_icon_name("window-close-symbolic"));
        chips.append(&chip);
    }
    left.append(&chips);

    let column = gtk4::Box::new(gtk4::Orientation::Vertical, 4);
    column.append(&if ellipsize { ellipsized() } else { plain() });
    let scroll = gtk4::ScrolledWindow::new();
    scroll.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);
    scroll.set_child(Some(&column));
    left.append(&scroll);

    let window = gtk4::Window::new();
    window.set_child(Some(&left));
    let (min, natural, _, _) = left.measure(gtk4::Orientation::Horizontal, -1);
    (min, natural)
}

fn main() {
    gtk4::init().expect("gtk init");
    println!("the dialog opens at 1040px wide, divider at 380px\n");
    println!(
        "LEFT  CheckButton::with_label   -> minimum {}px",
        minimum_of(plain)
    );
    println!(
        "LEFT  ellipsized label as child -> minimum {}px",
        minimum_of(ellipsized)
    );
    println!(
        "RIGHT one image row             -> minimum {}px",
        minimum_of(image_row)
    );
    println!();
    println!(
        "so the pane pair demands at least {}px before this change, {}px after",
        minimum_of(plain) + minimum_of(image_row),
        minimum_of(ellipsized) + minimum_of(image_row),
    );
    println!();
    println!("WHOLE LEFT PANE (divider sits at 380px)");
    println!(
        "  before -> minimum {}px, natural {}px",
        left_pane(false).0,
        left_pane(false).1
    );
    println!(
        "  after  -> minimum {}px, natural {}px",
        left_pane(true).0,
        left_pane(true).1
    );
    println!();
    println!();
    println!(
        "GtkPaned defaults shrink-start-child to {} — with that on, GTK may \n\
         allocate the start child LESS than its minimum and clip it, rather \n\
         than moving the divider.",
        gtk4::Paned::new(gtk4::Orientation::Horizontal).shrinks_start_child()
    );
}
