//! Notebook cell widgets for Verbinal.
//!
//! Provides [`CodeCellWidget`] (with Python syntax highlighting and output
//! rendering) and [`MarkdownCellWidget`] (with preview/edit toggle), wrapped
//! in the [`CellWidget`] enum.

use crate::models::notebook_document::{CellOutput, OutputData};
use base64::Engine;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::{self as gtk};
use std::rc::Rc;

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

    /// Render a single MIME-bundle output (image → picture, else plain text).
    fn render_output_data(&self, data: &OutputData) {
        // Prefer image/png
        if let Some(png_b64) = &data.image_png {
            if let Some(picture) = decode_png_picture(png_b64) {
                picture.set_halign(gtk::Align::Start);
                picture.set_can_shrink(true);
                picture.set_size_request(400, -1);
                self.output_box.append(&picture);
                return;
            }
        }

        // Fallback: text/plain
        if let Some(text) = data.plain_text() {
            let label = gtk::Label::new(Some(&text));
            label.set_halign(gtk::Align::Start);
            label.set_selectable(true);
            label.set_wrap(true);
            label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
            label.add_css_class("monospace");
            label.add_css_class("caption");
            self.output_box.append(&label);
        }
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
        self.preview_label.set_markup(&markdown_to_pango(text));
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
        self.preview_label.set_markup(&markdown_to_pango(&text));
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
// Image decoding helper
// ---------------------------------------------------------------------------

/// Decode a base64-encoded PNG string and produce a `gtk::Picture`.
///
/// Returns `None` if decoding or texture creation fails.
fn decode_png_picture(b64: &str) -> Option<gtk::Picture> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .ok()?;
    let gbytes = glib::Bytes::from(&bytes);
    let texture = gtk::gdk::Texture::from_bytes(&gbytes).ok()?;
    let picture = gtk::Picture::for_paintable(&texture);
    Some(picture)
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
fn apply_inline_markdown(line: &str) -> String {
    let escaped = escape_pango(line);

    // Order matters: apply bold before italic so **b** beats *i*.
    let s = apply_span(&escaped, "**", "**", "<b>", "</b>");
    let s = apply_span(&s, "*", "*", "<i>", "</i>");
    let s = apply_span(&s, "_", "_", "<i>", "</i>");
    apply_span(&s, "`", "`", "<tt>", "</tt>")
}

/// Replace `open_delim … close_delim` with `open_tag … close_tag` in `s`.
fn apply_span(
    s: &str,
    open_delim: &str,
    close_delim: &str,
    open_tag: &str,
    close_tag: &str,
) -> String {
    let mut result = String::new();
    let mut rest = s;
    while let Some(start) = rest.find(open_delim) {
        result.push_str(&rest[..start]);
        let after_open = &rest[start + open_delim.len()..];
        if let Some(end) = after_open.find(close_delim) {
            result.push_str(open_tag);
            result.push_str(&after_open[..end]);
            result.push_str(close_tag);
            rest = &after_open[end + close_delim.len()..];
        } else {
            // No closing delimiter — emit literally and stop.
            result.push_str(open_delim);
            rest = after_open;
        }
    }
    result.push_str(rest);
    result
}

/// Escape characters that are special in Pango markup (XML).
fn escape_pango(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
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
