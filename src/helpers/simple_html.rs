//! Enough HTML to render what a notebook actually produces.
//!
//! `text/html` is the representation the scientific stack reaches for first:
//! `astropy.table.Table`, `pandas.DataFrame`, and anything else with a
//! `_repr_html_`. Until now the notebook parsed that MIME type and then ignored
//! it, falling back to `text/plain` — so a table arrived as its `repr()`, which
//! is the output an astronomer looks at most.
//!
//! Not a browser and not trying to be. A notebook's HTML output is a table, or
//! a few lines of formatted text around one. Everything here is driven by what
//! the real thing emits — see the tests, which use captured
//! `astropy` and `pandas` output rather than invented markup.
//!
//! The split is deliberate: this module turns HTML into a small block list with
//! no GTK in it at all, so the parsing can be tested without a display. The
//! widget half only has to walk the blocks.

/// One renderable piece of an HTML output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HtmlBlock {
    /// A table. `headers` is empty when the table has no `<th>` row.
    Table {
        headers: Vec<String>,
        rows: Vec<Vec<String>>,
    },
    /// Everything else, already converted to Pango markup and escaped.
    ///
    /// Never empty: an empty run is dropped rather than emitted, so a caller
    /// can render every block it is given.
    Markup(String),
}

/// Split `html` into tables and the markup between them.
pub fn to_blocks(html: &str) -> Vec<HtmlBlock> {
    let mut blocks = Vec::new();
    let mut rest = html;

    while let Some(start) = find_ci(rest, "<table") {
        push_markup(&mut blocks, &rest[..start]);
        let after_open = &rest[start..];
        match find_ci(after_open, "</table>") {
            Some(end) => {
                let table = &after_open[..end + "</table>".len()];
                if let Some(block) = parse_table(table) {
                    blocks.push(block);
                }
                rest = &after_open[end + "</table>".len()..];
            }
            // An unclosed table: render what is left as text rather than
            // dropping the remainder of the output on the floor.
            None => {
                push_markup(&mut blocks, after_open);
                return blocks;
            }
        }
    }
    push_markup(&mut blocks, rest);
    blocks
}

/// Convert a run of non-table HTML to Pango markup, if it says anything.
fn push_markup(blocks: &mut Vec<HtmlBlock>, html: &str) {
    let markup = to_markup(html);
    if !markup.trim().is_empty() {
        blocks.push(HtmlBlock::Markup(markup));
    }
}

/// One table's header and body rows.
///
/// astropy emits TWO `<thead>` rows — the column names and their dtypes — so
/// the first is taken as the header and any later one becomes an ordinary row.
/// Losing the dtype row would hide real information; promoting it to a second
/// header would misrepresent it.
fn parse_table(table: &str) -> Option<HtmlBlock> {
    let mut headers: Vec<String> = Vec::new();
    let mut rows: Vec<Vec<String>> = Vec::new();

    for row_html in tags(table, "tr") {
        let cells_th: Vec<String> = tags(&row_html, "th").map(|c| text_of(&c)).collect();
        let cells_td: Vec<String> = tags(&row_html, "td").map(|c| text_of(&c)).collect();

        if !cells_td.is_empty() {
            rows.push(cells_td);
        } else if !cells_th.is_empty() {
            if headers.is_empty() {
                headers = cells_th;
            } else {
                rows.push(cells_th);
            }
        }
    }

    if headers.is_empty() && rows.is_empty() {
        return None;
    }
    Some(HtmlBlock::Table { headers, rows })
}

/// The inner HTML of every `<name>…</name>` in `html`, in order.
fn tags<'a>(html: &'a str, name: &'a str) -> impl Iterator<Item = String> + 'a {
    let open = format!("<{name}");
    let close = format!("</{name}>");
    let mut rest = html;
    std::iter::from_fn(move || {
        let start = find_ci(rest, &open)?;
        // Past the opening tag's own `>`, so attributes are skipped.
        let body_at = start + rest[start..].find('>')? + 1;
        let end = find_ci(&rest[body_at..], &close)?;
        let inner = rest[body_at..body_at + end].to_string();
        rest = &rest[body_at + end + close.len()..];
        Some(inner)
    })
}

/// Plain text of a cell: tags removed, entities decoded, whitespace collapsed.
fn text_of(html: &str) -> String {
    let stripped = strip_tags(html);
    decode_entities(&stripped)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Convert a small tag set to Pango markup; drop the rest.
///
/// Pango has no tables, no lists and no links-with-href, so those either become
/// a [`HtmlBlock::Table`] above or lose their decoration here. Text is escaped
/// FIRST and tags substituted after, so a `<` in the content cannot become
/// markup.
fn to_markup(html: &str) -> String {
    // Block-ish tags become line breaks so paragraphs do not run together.
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(at) = rest.find('<') {
        out.push_str(&escape_pango(&decode_entities(&rest[..at])));
        let after_lt = &rest[at + 1..];
        match rest[at..].find('>') {
            Some(close) if opens_a_tag(after_lt) => {
                out.push_str(markup_for_tag(&rest[at + 1..at + close]));
                rest = &rest[at + close + 1..];
            }
            // Content, not a tag: `x < y` and a stray unclosed `<` both land
            // here. Already escaped, so it is pushed rather than re-escaped.
            _ => {
                out.push_str("&lt;");
                rest = after_lt;
            }
        }
    }
    out.push_str(&escape_pango(&decode_entities(rest)));
    collapse_blank_lines(&out)
}

/// The markup a tag opens or closes, or a line break, or nothing.
fn markup_for_tag(tag: &str) -> &'static str {
    let name = tag
        .trim_start_matches('/')
        .split(|c: char| c.is_whitespace() || c == '/')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    let closing = tag.starts_with('/');
    match (name.as_str(), closing) {
        ("b" | "strong", false) => "<b>",
        ("b" | "strong", true) => "</b>",
        ("i" | "em", false) => "<i>",
        ("i" | "em", true) => "</i>",
        ("code" | "tt" | "pre", false) => "<tt>",
        ("code" | "tt" | "pre", true) => "</tt>",
        ("h1" | "h2" | "h3" | "h4" | "h5" | "h6", false) => "<b>",
        ("h1" | "h2" | "h3" | "h4" | "h5" | "h6", true) => "</b>\n",
        ("br", _) => "\n",
        ("p" | "div" | "li" | "tr", true) => "\n",
        _ => "",
    }
}

/// Squeeze runs of blank lines left behind by dropped tags.
fn collapse_blank_lines(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut blank_run = 0;
    for line in s.lines() {
        if line.trim().is_empty() {
            blank_run += 1;
            if blank_run > 1 {
                continue;
            }
        } else {
            blank_run = 0;
        }
        out.push_str(line);
        out.push('\n');
    }
    out.trim_end().to_string()
}

/// `str::find`, but blind to case.
///
/// Tag names are case-insensitive in HTML and plenty of real output is written
/// `<TABLE>` or `<Br>`. `needle` is always an ASCII tag fragment from this
/// module, so ASCII-lowercasing the haystack is enough to match it.
///
/// `to_ascii_lowercase` leaves non-ASCII bytes untouched, so the lowered copy
/// has the same byte length as the original and an offset found in one indexes
/// the other — which every slice in this module depends on.
fn find_ci(haystack: &str, needle: &str) -> Option<usize> {
    debug_assert!(needle.is_ascii(), "needle must be ASCII: {needle:?}");
    haystack.to_ascii_lowercase().find(needle)
}

fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(at) = rest.find('<') {
        out.push_str(&rest[..at]);
        match rest[at..].find('>') {
            Some(close) if opens_a_tag(&rest[at + 1..]) => rest = &rest[at + close + 1..],
            _ => {
                out.push('<');
                rest = &rest[at + 1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Whether the `<` immediately before `after_lt` opens a tag, or is content.
///
/// A bare `<` is legal text and appears constantly in real output — `3 < 4`, a
/// printed comparison, a diff. Treating every `<` as a tag opener made
/// `3 < 4 && 5 > 2` render as `3  2`, silently eating everything up to the next
/// `>`. HTML5 draws the line the same way: a `<` only starts a tag when a name
/// follows it.
fn opens_a_tag(after_lt: &str) -> bool {
    let mut chars = after_lt.chars();
    match chars.next() {
        // `</p>` — a closing tag still needs a name.
        Some('/') => chars.next().is_some_and(|c| c.is_ascii_alphabetic()),
        // `<!-- -->`, `<!DOCTYPE>`, `<?xml?>`: not rendered, but they are markup
        // and skipping to the `>` is the right thing to do with them.
        Some('!' | '?') => true,
        Some(c) => c.is_ascii_alphabetic(),
        None => false,
    }
}

/// The handful of entities that appear in real notebook output.
fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&nbsp;", " ")
        // Last: an `&amp;lt;` must not become `<`.
        .replace("&amp;", "&")
}

/// Escape text for Pango markup.
///
/// The one definition. `notebook_cell` had its own with the same name and a
/// different body — it escaped `"` as well — while importing this module, so
/// which one a reader was looking at depended on where they were standing.
/// `"` is kept, since it is the superset and is required inside an attribute
/// value; in text content the two render identically.
pub fn escape_pango(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact HTML `astropy.table.Table._repr_html_` produces, captured
    /// 2026-08-23 from astropy 8.0.1.
    const ASTROPY_TABLE: &str = r#"<div><i>Table length=2</i>
<table id="table135145463169728" class="table-striped table-bordered table-condensed">
<thead><tr><th>name</th><th>ra</th></tr></thead>
<thead><tr><th>str3</th><th>float64</th></tr></thead>
<tr><td>M31</td><td>10.68</td></tr>
<tr><td>M51</td><td>202.5</td></tr>
</table></div>"#;

    #[test]
    fn an_astropy_table_becomes_a_table() {
        let blocks = to_blocks(ASTROPY_TABLE);

        // The caption before the table survives as text.
        assert!(
            matches!(&blocks[0], HtmlBlock::Markup(m) if m.contains("Table length=2")),
            "{blocks:?}"
        );

        let HtmlBlock::Table { headers, rows } = &blocks[1] else {
            panic!("expected a table, got {:?}", blocks[1]);
        };
        assert_eq!(headers, &["name".to_string(), "ra".to_string()]);
        // astropy emits a SECOND thead with the dtypes. It is data, not a
        // header — dropping it would hide what the columns are.
        assert_eq!(rows[0], vec!["str3".to_string(), "float64".to_string()]);
        assert_eq!(rows[1], vec!["M31".to_string(), "10.68".to_string()]);
        assert_eq!(rows[2], vec!["M51".to_string(), "202.5".to_string()]);
    }

    /// A pandas-shaped table, with `<tbody>` and an index column.
    #[test]
    fn a_pandas_table_becomes_a_table() {
        let html = r#"<table border="1" class="dataframe">
  <thead><tr style="text-align: right;"><th></th><th>a</th><th>b</th></tr></thead>
  <tbody>
    <tr><th>0</th><td>1</td><td>3</td></tr>
    <tr><th>1</th><td>2</td><td>4</td></tr>
  </tbody>
</table>"#;
        let blocks = to_blocks(html);
        let HtmlBlock::Table { headers, rows } = &blocks[0] else {
            panic!("expected a table");
        };
        assert_eq!(headers, &["".to_string(), "a".to_string(), "b".to_string()]);
        // A row mixing `<th>` (the index) with `<td>` keeps its data cells.
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], vec!["1".to_string(), "3".to_string()]);
    }

    /// Text around a table is not lost.
    #[test]
    fn markup_before_and_after_a_table_is_kept() {
        let blocks = to_blocks("<p>before</p><table><tr><td>x</td></tr></table><p>after</p>");
        assert_eq!(blocks.len(), 3);
        assert!(matches!(&blocks[0], HtmlBlock::Markup(m) if m.contains("before")));
        assert!(matches!(&blocks[1], HtmlBlock::Table { .. }));
        assert!(matches!(&blocks[2], HtmlBlock::Markup(m) if m.contains("after")));
    }

    #[test]
    fn inline_formatting_becomes_pango() {
        let blocks = to_blocks("<b>bold</b> and <i>italic</i> and <code>code</code>");
        let HtmlBlock::Markup(m) = &blocks[0] else {
            panic!("expected markup");
        };
        assert!(m.contains("<b>bold</b>"), "{m}");
        assert!(m.contains("<i>italic</i>"), "{m}");
        assert!(m.contains("<tt>code</tt>"), "{m}");
    }

    /// Content that looks like markup is escaped, not obeyed.
    ///
    /// This text reaches a `set_markup` call. A `<` that survived as itself
    /// would either be swallowed as an unknown tag or abort the parse and blank
    /// the output — and the content here comes from whatever the user ran.
    #[test]
    fn content_cannot_smuggle_in_markup() {
        let blocks = to_blocks("<p>a &lt; b &amp;&amp; c &gt; d</p>");
        let HtmlBlock::Markup(m) = &blocks[0] else {
            panic!("expected markup");
        };
        assert!(m.contains("a &lt; b"), "{m}");
        assert!(
            !m.contains("a < b"),
            "an unescaped < reached the markup: {m}"
        );

        // And a raw span tag in the content does not become a Pango span.
        let blocks = to_blocks("<p>&lt;span foreground='red'&gt;x&lt;/span&gt;</p>");
        let HtmlBlock::Markup(m) = &blocks[0] else {
            panic!("expected markup");
        };
        assert!(!m.contains("<span"), "a span survived into the markup: {m}");
    }

    /// Escaping covers text that is not between two tags.
    ///
    /// `to_markup` has three exits — before a tag, after the last one, and a
    /// stray `<` with no `>` after it. Only the first was covered, and the
    /// other two carry the same text: tag-free output goes entirely through the
    /// trailing exit. A raw `<` reaching `set_markup` aborts Pango's parse and
    /// blanks the whole output, so every exit has to escape.
    #[test]
    fn content_is_escaped_at_every_exit() {
        // Trailing: after the last closing tag.
        let blocks = to_blocks("<p>ok</p>then 3 &lt; 4");
        let joined = format!("{blocks:?}");
        assert!(joined.contains("3 &lt; 4"), "{joined}");

        // No tags at all — the whole string takes the trailing exit.
        let blocks = to_blocks("3 < 4 && 5 > 2");
        let HtmlBlock::Markup(m) = &blocks[0] else {
            panic!("expected markup");
        };
        assert_eq!(m, "3 &lt; 4 &amp;&amp; 5 &gt; 2");

        // A `<` that never closes is content, not a tag.
        let blocks = to_blocks("<p>x</p>a < b");
        let joined = format!("{blocks:?}");
        assert!(joined.contains("a &lt; b"), "{joined}");
        assert!(!joined.contains("a < b"), "unescaped < survived: {joined}");
    }

    /// An unclosed table still shows its text.
    #[test]
    fn an_unclosed_table_is_not_swallowed() {
        let blocks = to_blocks("<p>head</p><table><tr><td>orphan");
        assert!(!blocks.is_empty());
        let joined = format!("{blocks:?}");
        assert!(joined.contains("head"), "{joined}");
        assert!(joined.contains("orphan"), "{joined}");
    }

    /// Plain text with no tags at all is still rendered.
    #[test]
    fn plain_text_survives() {
        let blocks = to_blocks("just words");
        assert_eq!(blocks, vec![HtmlBlock::Markup("just words".to_string())]);
    }

    /// Nothing renderable yields nothing to render.
    #[test]
    fn empty_html_yields_no_blocks() {
        assert!(to_blocks("").is_empty());
        assert!(to_blocks("   \n  ").is_empty());
        assert!(to_blocks("<div></div>").is_empty());
    }
}
