//! Notebook cell widgets for Verbinal.
//!
//! Provides [`CodeCellWidget`] (with Python syntax highlighting and output
//! rendering) and [`MarkdownCellWidget`] (with preview/edit toggle), wrapped
//! in the [`CellWidget`] enum.

use crate::helpers::simple_html::{self, escape_pango, HtmlBlock};
use crate::models::notebook_document::{CellOutput, OutputData, Representation};
use base64::Engine;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use std::rc::Rc;

/// Width an output image is asked to occupy, in px.
///
/// A request rather than a cap: pictures shrink below it, and a figure narrower
/// than this is not stretched.
const OUTPUT_IMAGE_WIDTH: i32 = 400;

/// Gaps between the cells of a rendered HTML table, in px.
///
/// Columns get the wider gap: rows are already separated by the line height,
/// while adjacent columns would otherwise run into each other.
const HTML_TABLE_ROW_SPACING: u32 = 2;
const HTML_TABLE_COLUMN_SPACING: u32 = 16;

// ---------------------------------------------------------------------------
// Keyword / syntax sets for lightweight Python highlighting
// ---------------------------------------------------------------------------

static PYTHON_KEYWORDS: &[&str] = &[
    "False", "None", "True", "and", "as", "assert", "async", "await", "break", "class", "continue",
    "def", "del", "elif", "else", "except", "finally", "for", "from", "global", "if", "import",
    "in", "is", "lambda", "nonlocal", "not", "or", "pass", "raise", "return", "try", "while",
    "with", "yield",
];

// ---------------------------------------------------------------------------
// Placeholder text
// ---------------------------------------------------------------------------

/// Wrap a `TextView` so it shows `hint` while its buffer is empty.
///
/// `gtk::Entry` has `placeholder-text`; `TextView` has nothing, which is why a
/// new notebook opened on two blank boxes that said nothing about what goes in
/// them or which one was Python. The reference prompts in both cell types, so
/// both get this — one function, because two hand-rolled placeholders would
/// drift in padding the moment either cell's margins changed.
///
/// The label sits in an overlay and is click-through (`can_target(false)`), so
/// the first click still lands in the text view underneath it.
fn with_placeholder(text_view: &gtk::TextView, hint: &'static str) -> gtk::Overlay {
    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(text_view));

    let label = gtk::Label::new(Some(crate::tr_en!(hint)));
    label.add_css_class("dim-label");
    label.set_can_target(false);
    label.set_halign(gtk::Align::Start);
    label.set_valign(gtk::Align::Start);
    // Match the text view's own insets so the hint sits exactly where the first
    // character will appear.
    label.set_margin_start(text_view.left_margin());
    label.set_margin_top(text_view.top_margin());
    overlay.add_overlay(&label);

    let buffer = text_view.buffer();
    let sync = {
        let label = label.clone();
        move |buf: &gtk::TextBuffer| label.set_visible(buf.char_count() == 0)
    };
    sync(&buffer);
    buffer.connect_changed(sync);
    overlay
}

// ---------------------------------------------------------------------------
// CodeCellWidget
// ---------------------------------------------------------------------------

/// A notebook code cell: execution counter on the left, editable source on
/// the right, with rendered outputs below the source.
pub struct CodeCellWidget {
    /// The root widget – a horizontal box.
    pub widget: gtk::Box,
    text_view: gtk::TextView,
    output_box: gtk::Box,
    exec_label: gtk::Label,
    run_button: gtk::Button,
}

impl CodeCellWidget {
    /// Build a new, empty code cell.
    pub fn new() -> Self {
        // ── root: left gutter + right content ───────────────────────────────
        let widget = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        widget.add_css_class("notebook-cell");

        // ── left gutter: exec counter + run button ───────────────────────────
        let gutter = gtk::Box::new(gtk::Orientation::Vertical, 2);
        gutter.set_width_request(56);
        gutter.set_valign(gtk::Align::Start);
        gutter.set_margin_top(6);
        gutter.set_margin_end(6);

        let exec_label = gtk::Label::new(Some("[ ]"));
        exec_label.set_halign(gtk::Align::Center);
        exec_label.add_css_class("monospace");
        exec_label.add_css_class("dim-label");
        exec_label.add_css_class("caption");

        let run_button = gtk::Button::from_icon_name("media-playback-start-symbolic");
        run_button.add_css_class("flat");
        run_button.add_css_class("circular");
        run_button.set_tooltip_text(Some(crate::tr_en!("Run cell (Ctrl+Enter)")));
        run_button.set_halign(gtk::Align::Center);
        run_button.set_icon_name("media-playback-start-symbolic");

        gutter.append(&run_button);
        gutter.append(&exec_label);
        widget.append(&gutter);

        // ── right side: source editor + outputs ─────────────────────────────
        let right = gtk::Box::new(gtk::Orientation::Vertical, 0);
        right.set_hexpand(true);

        // Source text view
        let text_view = gtk::TextView::new();
        text_view.set_monospace(true);
        text_view.set_wrap_mode(gtk::WrapMode::Word);
        text_view.set_top_margin(6);
        text_view.set_bottom_margin(6);
        text_view.set_left_margin(8);
        text_view.set_right_margin(8);
        text_view.add_css_class("code-cell-source");

        // Set up syntax highlighting tags in the buffer
        let buffer = text_view.buffer();
        setup_syntax_tags(&buffer);

        // Debounced syntax highlighting on change
        {
            let tv = text_view.clone();
            buffer.connect_changed(move |buf| {
                apply_syntax_highlighting(buf, &tv);
            });
        }

        right.append(&with_placeholder(&text_view, "Type Python code here…"));

        // Output area
        let output_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
        output_box.set_margin_start(8);
        output_box.set_margin_top(4);
        output_box.set_margin_bottom(4);
        right.append(&output_box);

        widget.append(&right);

        // Bottom separator
        widget.set_margin_bottom(2);

        CodeCellWidget {
            widget,
            text_view,
            output_box,
            exec_label,
            run_button,
        }
    }

    /// Return the root widget reference.
    pub fn widget(&self) -> &gtk::Box {
        &self.widget
    }

    /// Replace the source text.
    pub fn set_source(&self, text: &str) {
        self.text_view.buffer().set_text(text);
    }

    /// Read the current source text from the buffer.
    pub fn get_source(&self) -> String {
        let buf = self.text_view.buffer();
        let (start, end) = buf.bounds();
        buf.text(&start, &end, true).to_string()
    }

    /// Update the execution counter label: `[N]`.
    pub fn set_execution_count(&self, n: u32) {
        self.exec_label.set_label(&format!("[{}]", n));
    }

    /// Reset the execution counter label to `[ ]` (no count).
    pub fn clear_execution_count(&self) {
        self.exec_label.set_label("[ ]");
    }

    /// Show `[*]` while the cell is executing, or restore to `[ ]` / `[N]`.
    pub fn set_executing(&self, executing: bool) {
        if executing {
            self.exec_label.set_label("[*]");
            self.run_button.set_sensitive(false);
        } else {
            self.run_button.set_sensitive(true);
        }
    }

    /// Render `outputs` into the output area, replacing any previous content.
    /// Replace the cell's rendered outputs.
    pub fn set_outputs(&self, outputs: &[CellOutput]) {
        self.clear_outputs();
        for output in outputs {
            self.append_output(output);
        }
    }

    /// Remove every rendered output.
    pub fn clear_outputs(&self) {
        while let Some(child) = self.output_box.first_child() {
            self.output_box.remove(&child);
        }
        self.output_box.set_visible(false);
    }

    /// Render ONE output and append it.
    ///
    /// Split out of `set_outputs` so a long-running cell can show each line as
    /// the kernel produces it, instead of leaving the user staring at nothing
    /// until the whole cell finishes.
    pub fn append_output(&self, output: &CellOutput) {
        {
            match output {
                CellOutput::Stream { name, text } => {
                    let label = gtk::Label::new(Some(&text.joined()));
                    label.set_halign(gtk::Align::Start);
                    label.set_selectable(true);
                    label.set_wrap(true);
                    label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
                    label.add_css_class("monospace");
                    label.add_css_class("caption");
                    if name == "stderr" {
                        label.add_css_class("error");
                    }
                    self.output_box.append(&label);
                }

                CellOutput::ExecuteResult { data, .. } | CellOutput::DisplayData { data, .. } => {
                    self.render_output_data(data);
                }

                CellOutput::Error {
                    ename,
                    evalue,
                    traceback,
                } => {
                    let mut text = format!("{}: {}\n", ename, evalue);
                    text.push_str(&traceback.join("\n"));

                    let label = gtk::Label::new(Some(&text));
                    label.set_halign(gtk::Align::Start);
                    label.set_selectable(true);
                    label.set_wrap(true);
                    label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
                    label.add_css_class("monospace");
                    label.add_css_class("caption");
                    label.add_css_class("error");
                    self.output_box.append(&label);
                }
            }
        }

        self.output_box.set_visible(true);
    }

    /// Render a single MIME-bundle output as its richest representation.
    ///
    /// [`OutputData::richest`] makes the choice; this only builds the widget
    /// for it. A representation that turns out to be undisplayable — corrupt
    /// image bytes, HTML that reduces to nothing — falls back to `text/plain`
    /// rather than leaving the cell blank, because the bundle always carries it
    /// and a wrong-looking output beats a missing one.
    fn render_output_data(&self, data: &OutputData) {
        // Exhaustive on purpose: a MIME type added to `richest` must be given
        // somewhere to go here, and the compiler is what enforces it. The two
        // that shipped modelled-but-unrendered got there through a chain of
        // `if let`s that simply ran out.
        let rendered = match data.richest() {
            Some(Representation::Image(b64)) => self.append_image(b64),
            Some(Representation::Svg(svg)) => self.append_svg(&svg),
            Some(Representation::Html(html)) => self.append_html(&html),
            Some(Representation::Markdown(md)) => {
                self.output_box
                    .append(&html_markup_label(&markdown_to_pango(&md)));
                true
            }
            // Both are shown as their source, in monospace. There is no LaTeX
            // renderer yet (see B5 in dev_info/13), and pretty-printed JSON is
            // already the readable form. Both beat what `text/plain` falls back
            // to for these objects, which is `<__main__.X object at 0x…>`.
            Some(Representation::Latex(source)) | Some(Representation::Json(source)) => {
                self.append_text(&source);
                true
            }
            Some(Representation::Text(text)) => {
                self.append_text(&text);
                true
            }
            None => true,
        };
        if !rendered {
            if let Some(text) = data.plain_text() {
                self.append_text(&text);
            }
        }
    }

    /// Append a decoded image; `false` if the bytes were not an image.
    fn append_image(&self, b64: &str) -> bool {
        let Some(texture) = decode_image_texture(b64) else {
            return false;
        };
        self.output_box.append(&output_picture(&texture));
        true
    }

    /// Append an SVG picture; `false` if it could not be rasterised.
    ///
    /// SVG support comes from librsvg being present as a loader, which is a
    /// separate package and not guaranteed on every desktop. A `false` here
    /// sends the caller to `text/plain`, so a machine without it degrades to
    /// text rather than to a blank output.
    fn append_svg(&self, svg: &str) -> bool {
        let bytes = glib::Bytes::from(svg.as_bytes());
        let Ok(texture) = gtk::gdk::Texture::from_bytes(&bytes) else {
            return false;
        };
        self.output_box.append(&output_picture(&texture));
        true
    }

    /// Append rendered HTML; `false` if it held nothing renderable.
    fn append_html(&self, html: &str) -> bool {
        let blocks = simple_html::to_blocks(html);
        if blocks.is_empty() {
            return false;
        }
        for block in blocks {
            match block {
                HtmlBlock::Markup(markup) => self.output_box.append(&html_markup_label(&markup)),
                HtmlBlock::Table { headers, rows } => {
                    self.output_box.append(&html_table(&headers, &rows));
                }
            }
        }
        true
    }

    /// Append one run of plain output text.
    fn append_text(&self, text: &str) {
        let label = gtk::Label::new(Some(text));
        label.set_halign(gtk::Align::Start);
        label.set_selectable(true);
        label.set_wrap(true);
        label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        label.add_css_class("monospace");
        label.add_css_class("caption");
        self.output_box.append(&label);
    }

    /// Highlight or un-highlight the active-cell frame.
    pub fn set_active(&self, active: bool) {
        if active {
            self.widget.add_css_class("notebook-cell-active");
        } else {
            self.widget.remove_css_class("notebook-cell-active");
        }
        // The caret is NOT set from here. GTK already draws it only in the
        // view that has focus, so gating it on "is this the active cell" was a
        // second opinion about the same thing — and when the two disagreed the
        // focused cell had no caret at all: you clicked into a cell, saw
        // nothing, and only typing made it appear.
    }

    /// Expose the run button so [`NotebookPage`] can connect a callback.
    pub fn run_button(&self) -> &gtk::Button {
        &self.run_button
    }

    /// Expose the text view so [`NotebookPage`] can attach key controllers.
    pub fn text_view(&self) -> &gtk::TextView {
        &self.text_view
    }
}

// ---------------------------------------------------------------------------
// MarkdownCellWidget
// ---------------------------------------------------------------------------

/// A notebook markdown cell.
///
/// In *preview* mode, displays rendered markup via a `Label`.
/// Double-click (or Enter in command mode) switches to *edit* mode, where the
/// raw text is editable.  Escape returns to preview.
///
/// The `edit_mode` flag is held in an `Rc<Cell<bool>>` so that the
/// double-click gesture closure can share it without requiring `unsafe`.
pub struct MarkdownCellWidget {
    /// Root widget.
    pub widget: gtk::Box,
    stack: gtk::Stack,
    text_view: gtk::TextView,
    preview_label: gtk::Label,
    /// Whether the cell is currently showing the editor rather than the preview.
    edit_mode: Rc<std::cell::Cell<bool>>,
}

impl MarkdownCellWidget {
    /// Build a new, empty markdown cell.
    pub fn new() -> Self {
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        widget.add_css_class("notebook-cell");
        widget.set_margin_start(62); // align with code cell content
        widget.set_margin_top(4);
        widget.set_margin_bottom(4);

        let stack = gtk::Stack::new();
        stack.set_transition_type(gtk::StackTransitionType::None);
        // Size to the page being SHOWN, not to the tallest page.
        //
        // A `GtkStack` is homogeneous by default: it asks every child for its
        // size and requests the largest, so the hidden edit-mode `TextView`
        // — which holds the whole document and whose minimum height is its
        // content — set the height of a cell that was displaying a rendered
        // preview. Opening a 227-line markdown file filled the console with
        //
        //     GtkOverlay exceeds AdwApplicationWindow height:
        //     requested 875 px, 800 px available
        //
        // once per frame, because the invisible editor was asking for room the
        // window did not have.

        // ── edit mode ───────────────────────────────────────────────────────
        let text_view = gtk::TextView::new();
        text_view.set_wrap_mode(gtk::WrapMode::Word);
        text_view.set_top_margin(6);
        text_view.set_bottom_margin(6);
        text_view.set_left_margin(8);
        text_view.set_right_margin(8);

        // ── preview mode ────────────────────────────────────────────────────
        let preview_label = gtk::Label::new(None);
        preview_label.set_halign(gtk::Align::Start);
        preview_label.set_wrap(true);
        preview_label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        preview_label.set_use_markup(true);
        preview_label.set_selectable(false);
        preview_label.set_margin_start(8);
        preview_label.set_margin_top(6);
        preview_label.set_margin_bottom(6);

        stack.add_named(&preview_label, Some("preview"));
        stack.add_named(
            &with_placeholder(&text_view, "Type markdown here…"),
            Some("edit"),
        );
        stack.set_visible_child_name("preview");

        widget.append(&stack);

        // Shared edit-mode flag: both the struct and the gesture closure hold
        // a clone of this `Rc<Cell<bool>>`.
        let edit_mode = Rc::new(std::cell::Cell::new(false));

        // Double-click on preview → enter edit mode
        let gesture = gtk::GestureClick::new();
        gesture.set_button(gtk::gdk::BUTTON_PRIMARY);
        {
            let stack_clone = stack.clone();
            let tv = text_view.clone();
            let edit_mode_clone = edit_mode.clone();
            gesture.connect_pressed(move |g, n_press, _, _| {
                if n_press >= 2 {
                    stack_clone.set_visible_child_name("edit");
                    edit_mode_clone.set(true);
                    tv.grab_focus();
                    g.set_state(gtk::EventSequenceState::Claimed);
                }
            });
        }
        preview_label.add_controller(gesture);

        MarkdownCellWidget {
            widget,
            stack,
            text_view,
            preview_label,
            edit_mode,
        }
    }

    /// Return the root widget.
    pub fn widget(&self) -> &gtk::Box {
        &self.widget
    }

    /// Replace the source text and re-render the preview.
    pub fn set_source(&self, text: &str) {
        self.text_view.buffer().set_text(text);
        self.render_preview(text);
    }

    /// Show `source` as rendered markdown, or as itself if that is impossible.
    ///
    /// Pango refuses malformed markup outright and a `Label` given it renders
    /// NOTHING — so a single bad line blanked an entire 12 KB document, and the
    /// only sign was a warning on the console. The converter that caused it is
    /// fixed, but a markdown cell showing its source is a formatting problem
    /// while a markdown cell showing nothing is a lost document, and only one
    /// of those is worth risking on the next edge case.
    fn render_preview(&self, source: &str) {
        let markup = markdown_to_pango(source);
        self.preview_label.set_markup(&markup);
        if self.preview_label.text().is_empty() && !source.trim().is_empty() {
            self.preview_label.set_text(source);
        }
    }

    /// Read current source text from the buffer.
    pub fn get_source(&self) -> String {
        let buf = self.text_view.buffer();
        let (start, end) = buf.bounds();
        buf.text(&start, &end, true).to_string()
    }

    /// Switch into edit mode (shows the `TextView`).
    pub fn enter_edit_mode(&self) {
        self.edit_mode.set(true);
        self.stack.set_visible_child_name("edit");
        self.text_view.grab_focus();
    }

    /// Commit edits and switch to preview mode.
    pub fn enter_preview_mode(&self) {
        let text = self.get_source();
        self.render_preview(&text);
        self.edit_mode.set(false);
        self.stack.set_visible_child_name("preview");
    }

    /// Highlight or un-highlight the active-cell frame.
    pub fn set_active(&self, active: bool) {
        if active {
            self.widget.add_css_class("notebook-cell-active");
        } else {
            self.widget.remove_css_class("notebook-cell-active");
        }
    }

    /// Expose the text view for key-controller attachment.
    pub fn text_view(&self) -> &gtk::TextView {
        &self.text_view
    }
}

// ---------------------------------------------------------------------------
// Combined enum
// ---------------------------------------------------------------------------

/// Wraps either a [`CodeCellWidget`] or a [`MarkdownCellWidget`].
pub enum CellWidget {
    Code(CodeCellWidget),
    Markdown(MarkdownCellWidget),
}

impl CellWidget {
    /// Return the root `gtk::Box` of whichever variant this is.
    pub fn widget(&self) -> &gtk::Box {
        match self {
            CellWidget::Code(c) => c.widget(),
            CellWidget::Markdown(m) => m.widget(),
        }
    }

    /// Read the current source text.
    pub fn get_source(&self) -> String {
        match self {
            CellWidget::Code(c) => c.get_source(),
            CellWidget::Markdown(m) => m.get_source(),
        }
    }

    /// Mark or unmark as the active cell.
    pub fn set_active(&self, active: bool) {
        match self {
            CellWidget::Code(c) => c.set_active(active),
            CellWidget::Markdown(m) => m.set_active(active),
        }
    }

    /// The editable text view, whichever kind of cell this is.
    ///
    /// The page needs it to answer one question: *where is the keyboard
    /// pointing?* A markdown cell in preview mode still owns its view — it is
    /// simply not the visible stack child, so it cannot hold focus, which is
    /// the right answer rather than a special case.
    pub fn text_view(&self) -> &gtk::TextView {
        match self {
            CellWidget::Code(c) => c.text_view(),
            CellWidget::Markdown(m) => m.text_view(),
        }
    }

    /// Put the keyboard in this cell.
    pub fn focus_editor(&self) {
        if let CellWidget::Markdown(m) = self {
            m.enter_edit_mode();
        }
        self.text_view().grab_focus();
    }
}

// ---------------------------------------------------------------------------
// Syntax highlighting helpers
// ---------------------------------------------------------------------------

/// Create the named `TextTag`s used for Python syntax highlighting.
fn setup_syntax_tags(buffer: &gtk::TextBuffer) {
    let table = buffer.tag_table();

    let keyword_tag = gtk::TextTag::new(Some("kw"));
    keyword_tag.set_foreground(Some("#3584e4")); // blue
    table.add(&keyword_tag);

    let string_tag = gtk::TextTag::new(Some("str"));
    string_tag.set_foreground(Some("#2ec27e")); // green
    table.add(&string_tag);

    let comment_tag = gtk::TextTag::new(Some("cmt"));
    comment_tag.set_foreground(Some("#9a9996")); // gray
    table.add(&comment_tag);

    let number_tag = gtk::TextTag::new(Some("num"));
    number_tag.set_foreground(Some("#e5a50a")); // orange
    table.add(&number_tag);
}

/// Apply lightweight Python syntax highlighting to the whole buffer.
///
/// Called from `buffer.connect_changed`.  Runs entirely on the GLib main
/// thread — no threading concerns.
fn apply_syntax_highlighting(buffer: &gtk::TextBuffer, _tv: &gtk::TextView) {
    // Remove all existing tags first
    let (start, end) = buffer.bounds();
    buffer.remove_all_tags(&start, &end);

    let text = buffer.text(&start, &end, true).to_string();

    let mut offset = 0usize; // byte offset into `text`

    for line in text.lines() {
        let line_start = offset;
        let trimmed = line.trim_start();

        // Comment: everything from '#' to end-of-line
        if let Some(hash_pos) = line.find('#') {
            // Make sure the '#' isn't inside a string (simplified: skip if
            // there's a string delimiter before it — good enough for a
            // lightweight highlighter).
            let before_hash = &line[..hash_pos];
            if !before_hash.contains('"') && !before_hash.contains('\'') {
                let cmt_byte_start = line_start + hash_pos;
                let cmt_byte_end = line_start + line.len();
                tag_range(buffer, "cmt", &text, cmt_byte_start, cmt_byte_end);
                // Still highlight keywords before the '#' below; comment
                // highlighting takes precedence for the rest.
            }
        }

        // Skip comment-only lines for keyword/number/string scanning
        if trimmed.starts_with('#') {
            let cmt_byte_start = line_start + (line.len() - trimmed.len());
            let cmt_byte_end = line_start + line.len();
            tag_range(buffer, "cmt", &text, cmt_byte_start, cmt_byte_end);
            offset += line.len() + 1; // +1 for '\n'
            continue;
        }

        // Keywords: word-boundary scan
        for kw in PYTHON_KEYWORDS {
            let mut search_from = 0usize;
            while let Some(pos) = line[search_from..].find(kw) {
                let abs = search_from + pos;
                let after = abs + kw.len();
                // Ensure word boundaries
                let before_ok = abs == 0
                    || !line.as_bytes()[abs - 1].is_ascii_alphanumeric()
                        && line.as_bytes()[abs - 1] != b'_';
                let after_ok = after >= line.len()
                    || !line.as_bytes()[after].is_ascii_alphanumeric()
                        && line.as_bytes()[after] != b'_';
                if before_ok && after_ok {
                    tag_range(buffer, "kw", &text, line_start + abs, line_start + after);
                }
                search_from = abs + 1;
                if search_from >= line.len() {
                    break;
                }
            }
        }

        // String literals: single or double quoted (single-line only)
        highlight_strings(buffer, &text, line, line_start);

        // Numbers
        highlight_numbers(buffer, &text, line, line_start);

        offset += line.len() + 1; // +1 for '\n'
    }
}

/// Apply `tag_name` to the byte range `[byte_start, byte_end)` within `text`.
fn tag_range(
    buffer: &gtk::TextBuffer,
    tag_name: &str,
    text: &str,
    byte_start: usize,
    byte_end: usize,
) {
    if byte_start >= byte_end || byte_end > text.len() {
        return;
    }
    // Convert byte offsets to char offsets (GTK iterators use char offsets).
    let char_start = text[..byte_start].chars().count() as i32;
    let char_end = text[..byte_end].chars().count() as i32;
    let iter_start = buffer.iter_at_offset(char_start);
    let iter_end = buffer.iter_at_offset(char_end);
    buffer.apply_tag_by_name(tag_name, &iter_start, &iter_end);
}

/// Highlight quoted string literals on a single `line`.
fn highlight_strings(buffer: &gtk::TextBuffer, full_text: &str, line: &str, line_start: usize) {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'"' || b == b'\'' {
            let quote = b;
            let start = i;
            i += 1;
            // Find closing quote
            while i < bytes.len() {
                if bytes[i] == b'\\' {
                    i += 2; // skip escaped char
                    continue;
                }
                if bytes[i] == quote {
                    i += 1;
                    break;
                }
                i += 1;
            }
            tag_range(buffer, "str", full_text, line_start + start, line_start + i);
        } else {
            i += 1;
        }
    }
}

/// Highlight numeric literals on a single `line`.
fn highlight_numbers(buffer: &gtk::TextBuffer, full_text: &str, line: &str, line_start: usize) {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            // Check it's not part of an identifier
            if i > 0 && (bytes[i - 1].is_ascii_alphabetic() || bytes[i - 1] == b'_') {
                i += 1;
                continue;
            }
            let start = i;
            while i < bytes.len()
                && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'.' || bytes[i] == b'_')
            {
                i += 1;
            }
            tag_range(buffer, "num", full_text, line_start + start, line_start + i);
        } else {
            i += 1;
        }
    }
}

// ---------------------------------------------------------------------------
// HTML output rendering
// ---------------------------------------------------------------------------

/// A label for one run of non-table HTML.
///
/// The markup comes from [`simple_html`], which escapes the content it wraps.
/// Pango aborts its parse on malformed markup and leaves the label EMPTY, so a
/// rejected string falls back to showing itself — an output that reads oddly is
/// recoverable, one that silently vanishes is not.
fn html_markup_label(markup: &str) -> gtk::Label {
    let label = gtk::Label::new(None);
    label.set_halign(gtk::Align::Start);
    label.set_xalign(0.0);
    label.set_selectable(true);
    label.set_wrap(true);
    label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    label.add_css_class("caption");
    label.set_markup(markup);
    if label.text().is_empty() && !markup.is_empty() {
        label.set_text(markup);
    }
    label
}

/// A grid for one HTML table.
///
/// Wrapped in a horizontal `ScrolledWindow`: a table is as wide as its columns
/// need, and a wide one used to force the whole notebook to scroll sideways.
fn html_table(headers: &[String], rows: &[Vec<String>]) -> gtk::Widget {
    let grid = gtk::Grid::new();
    grid.set_row_spacing(HTML_TABLE_ROW_SPACING);
    grid.set_column_spacing(HTML_TABLE_COLUMN_SPACING);
    grid.set_halign(gtk::Align::Start);
    grid.add_css_class("notebook-html-table");

    let mut row_index = 0;
    if !headers.is_empty() {
        for (column, text) in headers.iter().enumerate() {
            let cell = html_table_cell(text);
            cell.add_css_class("heading");
            grid.attach(&cell, column as i32, row_index, 1, 1);
        }
        row_index += 1;
    }
    for row in rows {
        for (column, text) in row.iter().enumerate() {
            grid.attach(&html_table_cell(text), column as i32, row_index, 1, 1);
        }
        row_index += 1;
    }

    let scroller = gtk::ScrolledWindow::new();
    scroller.set_child(Some(&grid));
    scroller.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Never);
    scroller.set_halign(gtk::Align::Fill);
    // Without this the scroller asks for zero height and the table is invisible.
    scroller.set_propagate_natural_height(true);
    scroller.upcast()
}

/// One table cell.
fn html_table_cell(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_halign(gtk::Align::Start);
    label.set_xalign(0.0);
    label.set_selectable(true);
    label.add_css_class("monospace");
    label.add_css_class("caption");
    label
}

// ---------------------------------------------------------------------------
// Image decoding helper
// ---------------------------------------------------------------------------

/// Decode a base64-encoded image into a texture.
///
/// Format-agnostic on purpose: `Texture::from_bytes` sniffs PNG, JPEG and the
/// rest from the bytes themselves, so `image/jpeg` — modelled since the parser
/// was written and never rendered — needs nothing here beyond being asked for.
///
/// Returns `None` if decoding or texture creation fails.
fn decode_image_texture(b64: &str) -> Option<gtk::gdk::Texture> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .ok()?;
    let gbytes = glib::Bytes::from(&bytes);
    gtk::gdk::Texture::from_bytes(&gbytes).ok()
}

/// A `Picture` for an output image, sized so a container cannot collapse it.
///
/// The size request is the whole point. A `GtkPicture` with `can_shrink` set
/// reports a MINIMUM height of zero — it will happily be drawn in no space at
/// all — and the notebook packs cells into a `GtkListBox`, which allocates its
/// rows their minimum. Every image output was therefore built correctly,
/// handed real texture data, and then given a height of one or two pixels: a
/// blank cell where a figure should be. Labels survived the same treatment
/// because a label's minimum height is its text.
///
/// So the height is stated rather than left to be negotiated. Deriving it from
/// the texture also stops small images being upscaled to fill a fixed width,
/// which is what a notebook does elsewhere and what the reference does: a
/// 140x90 thumbnail now renders at 140x90 instead of blurred across 400px.
fn output_picture(texture: &gtk::gdk::Texture) -> gtk::Picture {
    let picture = gtk::Picture::for_paintable(texture);
    picture.set_halign(gtk::Align::Start);
    // Still allowed to shrink, so a narrow window scales the image down rather
    // than forcing the notebook to scroll sideways. The request below is what
    // keeps "can shrink" from meaning "can vanish".
    picture.set_can_shrink(true);
    let (width, height) = output_image_size(texture.width(), texture.height());
    picture.set_size_request(width, height);
    picture
}

/// The on-screen size for an image of `width` x `height`.
///
/// Downscale to fit [`OUTPUT_IMAGE_WIDTH`], never upscale, and keep the aspect
/// ratio. Split out because it is the arithmetic worth testing, and testing it
/// needs no display.
fn output_image_size(width: i32, height: i32) -> (i32, i32) {
    if width <= 0 || height <= 0 {
        // A texture with no size cannot be scaled to one. Fall back to the
        // nominal width and let GTK do what it can.
        return (OUTPUT_IMAGE_WIDTH, -1);
    }
    if width <= OUTPUT_IMAGE_WIDTH {
        return (width, height);
    }
    let scaled = (f64::from(height) * f64::from(OUTPUT_IMAGE_WIDTH) / f64::from(width)).round();
    (OUTPUT_IMAGE_WIDTH, (scaled as i32).max(1))
}

// ---------------------------------------------------------------------------
// Markdown → Pango markup
// ---------------------------------------------------------------------------

/// Convert a small subset of Markdown to Pango markup understood by `gtk::Label`.
///
/// Supported:
/// - `# Heading` → `<span size="xx-large"><b>…</b></span>`
/// - `## Heading` → `<span size="x-large"><b>…</b></span>`
/// - `### Heading` → `<span size="large"><b>…</b></span>`
/// - `**bold**` → `<b>bold</b>`
/// - `*italic*` or `_italic_` → `<i>italic</i>`
/// - `` `code` `` → `<tt>code</tt>`
/// - Plain text is XML-escaped.
pub fn markdown_to_pango(md: &str) -> String {
    let mut out = String::new();

    for line in md.lines() {
        // Headings
        if let Some(rest) = line.strip_prefix("### ") {
            out.push_str(&format!(
                "<span size=\"large\"><b>{}</b></span>\n",
                escape_pango(rest)
            ));
            continue;
        }
        if let Some(rest) = line.strip_prefix("## ") {
            out.push_str(&format!(
                "<span size=\"x-large\"><b>{}</b></span>\n",
                escape_pango(rest)
            ));
            continue;
        }
        if let Some(rest) = line.strip_prefix("# ") {
            out.push_str(&format!(
                "<span size=\"xx-large\"><b>{}</b></span>\n",
                escape_pango(rest)
            ));
            continue;
        }

        // Inline spans (bold, italic, code) — process left-to-right
        let processed = apply_inline_markdown(line);
        out.push_str(&processed);
        out.push('\n');
    }

    // Trim trailing newline
    if out.ends_with('\n') {
        out.pop();
    }

    out
}

/// Apply inline Markdown (bold / italic / code) to a single line.
///
/// ONE left-to-right pass, because four independent passes could not stay out
/// of each other's way. `**`, `*`, `_` and `` ` `` were each replaced across the
/// whole line in turn, so a line of snake_case identifiers inside code spans —
///
/// ```text
/// - Column names are snake_case CAOM paths (`proposal_id`, `target_name`,
/// ```
///
/// paired the underscore in `snake_case` with the one in `proposal_id`, opening
/// an italic span that closed INSIDE a later `<tt>`:
///
/// ```text
/// - Column names are snake<i>case CAOM paths (<tt>proposal</i>id</tt>, …
/// ```
///
/// Pango refuses malformed markup outright and a `Label` given it renders
/// NOTHING, so one such line blanked the entire cell. A 12 KB manual displayed
/// as an empty box.
///
/// Two rules from CommonMark fix it by construction, and both are what a reader
/// already expects:
///
///  * A code span is literal. No emphasis is looked for inside one, so an
///    identifier in backticks cannot open anything.
///  * `_` is emphasis only at a word boundary. `proposal_id` is a name, not
///    italic text — this is exactly why CommonMark treats intraword `_`
///    differently from `*`.
///
/// Anything unterminated stays literal, so the output is always balanced.
fn apply_inline_markdown(line: &str) -> String {
    let mut out = String::new();
    let mut rest = line;
    let mut previous: Option<char> = None;

    while !rest.is_empty() {
        // Code first: its contents are text, whatever they contain.
        if let Some(after) = rest.strip_prefix('`') {
            if let Some(end) = after.find('`') {
                out.push_str("<tt>");
                out.push_str(&escape_pango(&after[..end]));
                out.push_str("</tt>");
                rest = &after[end + 1..];
                previous = Some('`');
                continue;
            }
        }
        if let Some(after) = rest.strip_prefix("**") {
            if let Some(end) = after.find("**").filter(|e| is_emphasis(&after[..*e])) {
                out.push_str("<b>");
                out.push_str(&apply_inline_markdown(&after[..end]));
                out.push_str("</b>");
                rest = &after[end + 2..];
                previous = Some('*');
                continue;
            }
        }
        if let Some(after) = rest.strip_prefix('*') {
            if let Some(end) = after.find('*').filter(|e| is_emphasis(&after[..*e])) {
                out.push_str("<i>");
                out.push_str(&apply_inline_markdown(&after[..end]));
                out.push_str("</i>");
                rest = &after[end + 1..];
                previous = Some('*');
                continue;
            }
        }
        if let Some(after) = rest.strip_prefix('_') {
            if underscore_is_emphasis(previous, after) {
                if let Some(end) = closing_underscore(after).filter(|e| is_emphasis(&after[..*e])) {
                    out.push_str("<i>");
                    out.push_str(&apply_inline_markdown(&after[..end]));
                    out.push_str("</i>");
                    rest = &after[end + 1..];
                    previous = Some('_');
                    continue;
                }
            }
        }

        let ch = rest.chars().next().expect("rest is non-empty");
        out.push_str(&escape_pango(&ch.to_string()));
        rest = &rest[ch.len_utf8()..];
        previous = Some(ch);
    }
    out
}

/// Whether `content` between two delimiters is really emphasis.
///
/// Empty or space-padded content is not. `a ** b` has no closing `**`, so it
/// falls to the single-`*` rule, which without this matched the very next `*`
/// and produced an empty `<i></i>` — swallowing the stars a reader typed. The
/// same rule is why `2 * 3 * 4` stays arithmetic rather than becoming italic.
fn is_emphasis(content: &str) -> bool {
    !content.is_empty()
        && !content.starts_with(char::is_whitespace)
        && !content.ends_with(char::is_whitespace)
}

/// Whether an `_` at this position opens emphasis rather than being part of a word.
///
/// `proposal_id` and `energy_bandpassName` are identifiers, and a scientist's
/// notes are full of them. CommonMark draws the line at word boundaries for
/// exactly this reason: `_` is a word character in code, `*` is not.
fn underscore_is_emphasis(previous: Option<char>, after: &str) -> bool {
    let preceded_by_word = previous.is_some_and(|c| c.is_alphanumeric() || c == '_');
    let followed_by_space = after.chars().next().is_none_or(char::is_whitespace);
    !preceded_by_word && !followed_by_space
}

/// The closing `_` of an emphasis span, if the line has one.
///
/// It must also sit at a word boundary, or `_a_b_c` would close on the `_`
/// inside a name and leave the rest mismatched.
fn closing_underscore(after: &str) -> Option<usize> {
    let bytes = after.as_bytes();
    for (i, ch) in after.char_indices() {
        if ch != '_' {
            continue;
        }
        let follows_word = i > 0
            && after[..i]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric() || c == '_');
        let next_is_word = bytes
            .get(i + 1)
            .is_some_and(|b| (*b as char).is_alphanumeric() || *b == b'_');
        if follows_word && !next_is_word {
            return Some(i);
        }
    }
    None
}

impl Default for MarkdownCellWidget {
    fn default() -> Self {
        Self::new()
    }
}

impl Default for CodeCellWidget {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod caret_tests {
    //! Clicking into a cell showed no caret; it appeared only once you typed.
    //!
    //! `set_active` drove `set_cursor_visible(active)`, so the caret depended on
    //! an INDEX the page maintained rather than on where the keyboard actually
    //! was. GTK already draws a caret only in the focused view, so the extra
    //! gate could only ever disagree — and when it did, the cell you had just
    //! clicked into looked dead.

    const SOURCE: &str = include_str!("notebook_cell.rs");

    #[test]
    fn nothing_hides_the_caret_of_a_focused_cell() {
        let code = crate::testing::code(SOURCE);
        assert!(
            !code.contains("set_cursor_visible("),
            "the caret is being toggled by hand again; GTK ties it to focus, and \
             a second opinion about that leaves a clicked-in cell with no cursor"
        );
    }
}

#[cfg(test)]
mod output_layout_tests {
    //! Sizing for image outputs.
    //!
    //! Guards a bug that reached a user: every image output rendered as a
    //! blank cell while text in the same cell rendered fine. See
    //! `examples/notebook_layout_probe.rs` for the half of it that needs a
    //! display — this file covers the arithmetic, which does not.
    use super::*;

    /// An output image gets a real height, so a container cannot collapse it.
    ///
    /// This is the arithmetic behind a bug that reached a user: every image
    /// output rendered as a blank cell. `GtkPicture` with `can_shrink` reports
    /// a minimum height of ZERO, and the notebook packs cells into a
    /// `GtkListBox`, which allocates rows their minimum — so the picture was
    /// built, handed real pixels, and drawn in one pixel of height.
    #[test]
    fn an_output_image_is_never_asked_to_be_zero_high() {
        // The reported case: a 140x90 PIL image, and a 640x480 figure.
        for (w, h) in [(140, 90), (640, 480), (1, 1), (4000, 30)] {
            let (rw, rh) = output_image_size(w, h);
            assert!(
                rh > 0,
                "{w}x{h} asked for a height of {rh}; the row will collapse"
            );
            assert!(rw > 0, "{w}x{h} asked for a width of {rw}");
        }
    }

    #[test]
    fn a_small_image_is_shown_at_its_own_size_not_stretched() {
        // The old code requested a fixed 400px width for everything, so a
        // thumbnail was blown up and blurred. Jupyter shows it at its own size.
        assert_eq!(output_image_size(140, 90), (140, 90));
        assert_eq!(
            output_image_size(OUTPUT_IMAGE_WIDTH, 100),
            (OUTPUT_IMAGE_WIDTH, 100)
        );
    }

    #[test]
    fn a_large_image_is_scaled_down_keeping_its_shape() {
        // 640x480 is the matplotlib default.
        let (w, h) = output_image_size(640, 480);
        assert_eq!(w, OUTPUT_IMAGE_WIDTH);
        assert_eq!(
            h, 300,
            "4:3 scaled to {OUTPUT_IMAGE_WIDTH} wide should be 300 high"
        );

        // An extreme aspect ratio still gets at least one pixel rather than
        // rounding down to the collapse this whole test exists to prevent.
        let (_, h) = output_image_size(40_000, 30);
        assert!(h >= 1, "height rounded away to {h}");
    }

    #[test]
    fn a_texture_with_no_size_does_not_divide_by_zero() {
        assert_eq!(output_image_size(0, 0), (OUTPUT_IMAGE_WIDTH, -1));
        assert_eq!(output_image_size(-1, 10), (OUTPUT_IMAGE_WIDTH, -1));
    }
}

#[cfg(test)]
mod markdown_markup_tests {
    //! The markup a markdown cell renders must be markup Pango accepts.
    //!
    //! Pango refuses malformed markup outright and a `Label` given it renders
    //! NOTHING. So a converter bug is not a formatting bug here — it is a blank
    //! cell, and the only sign is a warning on a console nobody is reading.
    use super::*;

    /// Every line must produce balanced markup.
    ///
    /// Checked by counting tags rather than by parsing, so the test needs no
    /// display and runs in CI. `examples/markdown_parse_probe.rs` does the real
    /// Pango parse over a whole file.
    fn well_formed(markup: &str) -> bool {
        let mut stack: Vec<&str> = Vec::new();
        let mut rest = markup;
        while let Some(at) = rest.find('<') {
            let Some(close) = rest[at..].find('>') else {
                return false;
            };
            let tag = &rest[at + 1..at + close];
            if let Some(name) = tag.strip_prefix('/') {
                if stack.pop() != Some(name) {
                    return false;
                }
            } else if !tag.ends_with('/') {
                let name = tag.split_whitespace().next().unwrap_or(tag);
                stack.push(match name {
                    "b" => "b",
                    "i" => "i",
                    "tt" => "tt",
                    "span" => "span",
                    other => other,
                });
            }
            rest = &rest[at + close + 1..];
        }
        stack.is_empty()
    }

    /// The line that blanked a 12 KB manual.
    ///
    /// Four independent replace passes paired the `_` in `snake_case` with the
    /// one in `proposal_id`, opening an italic span that closed inside a later
    /// `<tt>`. Pango rejected the document and the cell rendered empty.
    #[test]
    fn snake_case_identifiers_in_code_spans_do_not_break_the_markup() {
        let line = "- Column names are snake_case CAOM paths (`proposal_id`, `target_name`,";
        let markup = apply_inline_markdown(line);
        assert!(well_formed(&markup), "unbalanced: {markup}");
        assert!(
            !markup.contains("<i>"),
            "an identifier was italicised: {markup}"
        );
        assert!(markup.contains("<tt>proposal_id</tt>"), "{markup}");

        let line = "`energy_bandpassName`). When unsure: `describe_tap_schema {\"table\":\"x\"}`.";
        let markup = apply_inline_markdown(line);
        assert!(well_formed(&markup), "unbalanced: {markup}");
        assert!(markup.contains("<tt>energy_bandpassName</tt>"), "{markup}");

        // An identifier OUTSIDE a code span must not open emphasis either.
        // This case isolates the opening rule: the closing `_` here sits at a
        // word end, so it would be accepted — only the check on the OPENER
        // keeps `my_var_` from becoming `my<i>var</i>`.
        let markup = apply_inline_markdown("use my_var_ here");
        assert!(
            !markup.contains("<i>"),
            "an identifier opened emphasis: {markup}"
        );
        assert!(well_formed(&markup));
    }

    /// A code span is literal: nothing inside it is formatting.
    #[test]
    fn a_code_span_is_not_searched_for_emphasis() {
        let markup = apply_inline_markdown("`a *b* c`");
        assert_eq!(markup, "<tt>a *b* c</tt>");
        // ...and its content is still escaped.
        let markup = apply_inline_markdown("`a < b & c`");
        assert!(
            markup.contains("&lt;") && markup.contains("&amp;"),
            "{markup}"
        );
        assert!(well_formed(&markup));
    }

    /// The formatting that should work, still works.
    #[test]
    fn bold_italic_and_code_still_render() {
        assert_eq!(apply_inline_markdown("**b**"), "<b>b</b>");
        assert_eq!(apply_inline_markdown("*i*"), "<i>i</i>");
        assert_eq!(apply_inline_markdown("_i_"), "<i>i</i>");
        assert_eq!(apply_inline_markdown("`c`"), "<tt>c</tt>");
        // Bold beats italic on the same run.
        assert_eq!(
            apply_inline_markdown("**b** and *i*"),
            "<b>b</b> and <i>i</i>"
        );
    }

    /// An unterminated delimiter is text, not an open tag.
    #[test]
    fn an_unclosed_delimiter_stays_literal() {
        for line in ["a * b", "a ** b", "a ` b", "a _ b", "*", "`", "__"] {
            let markup = apply_inline_markdown(line);
            assert!(well_formed(&markup), "{line:?} -> {markup}");
            assert!(
                !markup.contains('<'),
                "{line:?} produced a tag from an unclosed delimiter: {markup}"
            );
        }
    }

    /// Content that looks like markup is escaped, not obeyed.
    #[test]
    fn content_cannot_smuggle_in_markup() {
        let markup = apply_inline_markdown("<span foreground='red'>x</span> & y");
        assert!(!markup.contains("<span"), "{markup}");
        assert!(markup.contains("&lt;span"), "{markup}");
        assert!(markup.contains("&amp;"), "{markup}");
        assert!(well_formed(&markup));
    }

    /// Whole documents, including the shapes that appear in real notes.
    #[test]
    fn a_document_of_awkward_lines_stays_well_formed() {
        let doc = "# Heading\n\
                   Some `snake_case_name` and a *word* and **bold**.\n\
                   - `a_b`, `c_d`, `e_f`\n\
                   Mixed `code with *stars*` and _real emphasis_.\n\
                   An unmatched ` backtick and an unmatched * star.\n\
                   file_name_with_many_underscores.txt\n\
                   <not a tag> & an ampersand\n";
        let markup = markdown_to_pango(doc);
        assert!(well_formed(&markup), "unbalanced markup:\n{markup}");
        assert!(markup.contains("<tt>snake_case_name</tt>"), "{markup}");
        assert!(
            markup.contains("file_name_with_many_underscores.txt"),
            "a filename was mangled: {markup}"
        );
    }
}
