//! Does the markup we generate for a markdown cell actually parse?
//!
//!     cargo run --example markdown_parse_probe -- <file.md>
//!
//! A `gtk::Label` given malformed markup renders NOTHING — Pango aborts the
//! parse and the cell is blank. So the question is not what the converter
//! produces but whether Pango accepts it.
use verbinal::ui::notebook_cell::markdown_to_pango;

fn parses(markup: &str) -> Result<(), String> {
    // `parse_markup` is exactly what `Label::set_markup` runs.
    gtk4::pango::parse_markup(markup, '\0')
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn main() {
    gtk4::init().expect("gtk init");
    let path = std::env::args().nth(1).expect("a markdown file");
    let text = std::fs::read_to_string(&path).expect("read");

    let markup = markdown_to_pango(&text);
    match parses(&markup) {
        Ok(()) => println!("whole file: markup parses"),
        Err(e) => println!("whole file: REJECTED — {e}\n"),
    }

    // Narrow it to the lines that break, so the construct is named rather than
    // guessed at.
    let mut bad = 0;
    for (i, line) in text.lines().enumerate() {
        let m = markdown_to_pango(line);
        if let Err(e) = parses(&m) {
            bad += 1;
            if bad <= 6 {
                println!("line {:>4}: {}", i + 1, line.trim());
                println!("           -> {m}");
                println!("           !! {e}\n");
            }
        }
    }
    println!("{bad} line(s) produce markup Pango refuses.");
}
