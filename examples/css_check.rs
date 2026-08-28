//! Does the app stylesheet parse without complaint?
//!
//!     cargo run --example css_check
//!
//! A bad property is not a build error — GTK drops that ONE declaration and
//! carries on, so a rule can silently do nothing while the app looks fine
//! everywhere else.
//!
//! What this does NOT catch, verified by injecting one: an undefined
//! `@named-color`. GTK4 resolves those when the style is computed against a
//! widget, not when the sheet is parsed, so no parsing error is ever emitted.
//! The defence against that is to use names the sheet already uses elsewhere.
fn main() {
    gtk4::init().expect("gtk init");
    let provider = gtk4::CssProvider::new();
    let errors = std::rc::Rc::new(std::cell::RefCell::new(Vec::<String>::new()));
    let sink = errors.clone();
    provider.connect_parsing_error(move |_, section, err| {
        sink.borrow_mut()
            .push(format!("{}: {err}", section.to_str()));
    });
    provider.load_from_string(include_str!("../src/style.css"));
    let errors = errors.borrow();
    if errors.is_empty() {
        println!("style.css parses clean (property errors only — see the note above)");
    } else {
        for e in errors.iter() {
            println!("CSS ERROR {e}");
        }
        std::process::exit(1);
    }
}
