# 13 — Notebook alignment: Jupyter, and the Windows app

Status: **A3, B1, A1 and the `image/jpeg` half of B3 are implemented.** The
rest is still plan. See "What has been done" below for what changed and how it
was checked; the tier descriptions are left as written so the remaining items
still read as they were assessed.

The prompt was "the Windows app can show images inside the notebook, ours
cannot". That turned out to be half right in an interesting way, so this starts
with what was measured rather than what was assumed.

## What was measured

Driving `data/kernel_harness.py` directly, one cell at a time, and reading the
MIME keys it emits:

| cell | our harness emits |
| --- | --- |
| `plt.plot([1,2,3])` | `text/plain`, **`image/png`** |
| `Image.new('RGB',(32,32),'red')` (PIL, has `_repr_png_`) | `text/plain` only |
| `Table({'a':[1,2]})` (astropy, has `_repr_html_`) | `text/plain` only |
| `print(...)` | `stream` |

And what each app's renderer dispatches on:

| | ours | CanfarDesktop |
| --- | --- | --- |
| `image/png` | yes | yes |
| `text/plain` | yes | yes |
| `text/html` | **no** — modelled, never rendered | **yes** (`SimpleHtmlRenderer`) |
| `image/jpeg` | no — modelled, never rendered | no |
| ANSI escapes | no | yes (`AnsiParser`) |

So images DO work — for matplotlib, which both harnesses special-case by
checking `isinstance(obj, Figure)`. Nothing else produces an image in either
app, because neither implements the display protocol that would let it. The
reference's `_capture_display_data` has exactly our limitation and returns
`None` for everything that is not a figure.

The real difference the prompt is pointing at is more likely `text/html`: an
`astropy.table.Table` or a `pandas.DataFrame` renders as a formatted table on
Windows and as its `repr()` here. For an astronomy notebook that is the output
people look at most.

## What has been done

Implemented, with the verification for each:

**A3 — a cell mixing a magic with Python.** `_split_cell` now splits a cell into
magic and code segments and runs them **in source order**, so both halves run
and their output arrives in the order it was written. The first attempt hoisted
every magic to the front, which fixed the syntax error and introduced a subtler
one — `print('first')` then `!echo second` printed `second` first. Code segments
are blank-padded rather than closed up so tracebacks keep their line numbers.

**B1 — the `_repr_*_` protocol.** `_mime_bundle` asks an object how it wants to
be shown (`_repr_html_`, `_repr_png_`, `_repr_jpeg_`, `_repr_svg_`,
`_repr_markdown_`, `_repr_latex_`, `_repr_json_`), always includes `text/plain`,
and ignores a method that raises. Measured before and after, on the same cells
as the table above:

| cell | was | now |
| --- | --- | --- |
| `Image.new('RGB',(32,32),'red')` | `text/plain` | `image/png`, `image/jpeg`, `text/plain` |
| `Table({'a':[1,2]})` | `text/plain` | **`text/html`**, `text/plain` |

**B2 — `display()` and `IPython.display`.** `display()` is injected into the
user's namespace, so it works with no IPython at all. It emits immediately while
`print` output is captured until the cell ends, so it drains the capture buffer
first — otherwise `print`/`display`/`print` arrived back-to-front.

`from IPython.display import HTML, Image, Markdown, Latex, SVG, JSON,
clear_output` now works in both directions:

- **IPython absent** — a stand-in is built on demand by a `sys.meta_path`
  finder. On demand is the whole point: the first version registered it at
  startup, and libraries read the presence of `IPython` in `sys.modules` as
  "we are in a notebook". matplotlib called `get_ipython()` on it and every
  figure cell died with `module 'IPython' has no attribute 'get_ipython'`. It is
  now created only when a cell actually writes the import, and when it is, it
  answers `get_ipython()` truthfully with `None`.
- **IPython installed** — its `display()` publishes to a running kernel, and
  none is; outside one it prints a repr, which is the text the user was
  unhappy with to begin with. A second finder patches it at import time so it
  publishes here instead. Its own `HTML`/`Image` classes are untouched: they
  already carry the `_repr_*_` methods `_mime_bundle` reads.

`Image(url=...)` is refused rather than fetched — a cell that reaches the
network should say so, especially on someone else's cluster.

**A1 — `text/html` is rendered.** `helpers::simple_html` turns HTML into tables
and Pango markup with no GTK in it, so the parsing is tested without a display;
`notebook_cell` builds a `GtkGrid` per table and a label per markup run.
`OutputData::richest` now picks the representation, replacing a fixed
"is there a PNG? is there text?" that left `text/html` and `image/jpeg` parsed
and ignored. It prefers images over HTML, which is deliberately the opposite of
nbconvert — nbconvert renders into a browser, and ours would show plotly's
interactive HTML as empty divs while dropping the static image that works.

**B3, in part — `image/jpeg`.** The decoder sniffs the format from the bytes, so
this needed only to be asked for.

### Defects found while doing it

Not in the original plan; found by driving the harness rather than reading it.

- **`_strip_harness_frames` had never stripped a single frame.** It asked
  `"__file__" in dir()` from inside a function, where `dir()` lists locals and
  `__file__` is a module global — always false, so the path was the literal
  `"<harness>"` and no frame ever matched. Every notebook traceback has been
  showing the harness's internals above the user's own line.
- **A SyntaxError carried stdlib frames** (`ast.py`, line 52, in parse) that no
  harness-filename check could ever have caught. Both are now one rule: a
  traceback starts at the first `<cell>` frame. Frames *below* it are kept — a
  failure inside numpy is the user's stack.
- **Python 3.11+ emits two continuation lines per frame** (source and `^^^^`
  carets); the filter skipped a fixed one, leaving orphaned carets.
- **A bare `<` in HTML ate everything up to the next `>`.** `3 < 4 && 5 > 2`
  rendered as `3  2`. A `<` only opens a tag when a name follows it.

### How it is guarded now

- `tests/kernel_harness.rs` — nine integration tests driving the real python
  script over the real protocol. Nothing in `cargo test` reached the harness
  before, which is how the two defects above shipped. Library-dependent cases
  skip loudly when the library is absent.
- `helpers::simple_html` unit tests, against captured `astropy` and `pandas`
  output rather than invented markup.
- `OutputData::richest` unit tests for each representation.
- `examples/notebook_output_probe.rs` — builds a real `CodeCellWidget`, feeds it
  captured bundles, and walks the widget tree; then runs the REAL harness and
  renders what it actually emits. Model-level tests would not have caught the
  original bug, because the model was right the whole time and the call site
  never asked.
- `testing::python_code` strips Python docstrings as well as `#` comments, after
  a harness guard found the defective call inside the docstring explaining why
  it had been removed.

Every guard above was mutation-tested: the code was broken deliberately and the
test confirmed to fail. That caught two things reading could not have:

- One survivor was traced to a mutation that is a **no-op** — `want_value`
  per-segment versus on the last segment produces identical output, because
  each code segment reassigns the result — and the test comment now says so
  rather than claiming a guard it does not have.
- Three survivors shared one cause: both tests that require IPython to be
  ABSENT were **skipping silently**. Their check used
  `importlib.util.find_spec`, which walks `sys.meta_path` — where the harness
  had just installed its own IPython stand-in. The test asked "is IPython
  installed?" and got back the harness's own answer. It now uses `PathFinder`,
  which searches `sys.path` only.

## Tier A — where the Windows app is ahead

These are parity gaps: the reference does it, we do not.

**A1. `text/html` outputs are never rendered.** `OutputData::text_html` is
parsed and then ignored; rendering falls through to `text/plain`. The reference
has `SimpleHtmlRenderer` — tables, bold, italic, code, pre, links, br, p,
headings, explicitly "not a full CommonMark parser, covers the 80% used in
Jupyter". Highest value of anything in this document: it is what makes a table
look like a table.

**A2. Markdown cells render three constructs.** `markdown_to_pango` handles
`**bold**`, `*italic*`, `` `code` `` and escapes the rest. The reference adds
headings, code blocks, lists and horizontal rules. A notebook's prose is mostly
headings and lists, so ours reads as a wall of text.

**A3. A cell mixing a magic with Python fails outright.** Measured:

```
%pip --version
print('python ran too')
```

→ `error: invalid syntax (<cell>, line 1)`. Neither part runs. `_preprocess_magic`
only recognises a cell whose ENTIRE content is one magic line, and only
`%pip install …` and `!shell`; anything else falls through to `compile()`,
where `%` is a syntax error. The reference processes lines top-to-bottom,
mixing magic and code, and recognises `%pip`, `%conda`, `%matplotlib` and `!`.

This one is a defect rather than a gap, and it is small. It should go first.

**A4. No ANSI handling.** Our tracebacks come from `traceback.format_exc()`,
which emits none, so this is currently invisible — but any library that colours
its own output (rich, colorama, pytest) will print escape codes as literal
garbage. Cheap to add once, and it stops being a question.

## Tier B — where both apps are behind Jupyter

Neither app does these. They are what "align with the official notebook
projects" means beyond parity.

**B1. The `_repr_*_` protocol.** This is the root cause of "images do not
show". A Jupyter kernel asks an object for its representations —
`_repr_html_`, `_repr_png_`, `_repr_svg_`, `_repr_latex_`, `_repr_markdown_`,
`_repr_json_`, `_ipython_display_` — and sends every one it gets as a MIME
bundle. Both harnesses instead ask one question: "is this a matplotlib
Figure?".

Implementing the protocol is perhaps thirty lines in the harness and fixes PIL
images, astropy tables and quicklooks, pandas, plotly's static output, and
anything else in the scientific stack, all at once. **Do this before anything
else in Tier B**, and pair it with A1 so the HTML it starts producing has
somewhere to go.

**B2. No `display()` and no `IPython.display`.** `display(obj)`,
`Image(...)`, `HTML(...)`, `Markdown(...)` are what notebooks and tutorials
tell people to write. A shim that emits the same MIME bundles is small once B1
exists, and does not require IPython to be installed.

**B3. Other MIME types.** `image/jpeg` (modelled, unrendered), `image/svg+xml`,
`text/latex`, `application/json`, `text/markdown`. Each is a small renderer
once the dispatch exists.

**B4. Markdown attachments.** `![fig](attachment:plot.png)` with the bytes in
the cell's `attachments` field is how a notebook embeds an image without an
external file. Neither app reads `attachments` at all. This is the other
legitimate reading of "show images inside the notebook".

**B5. LaTeX math in markdown.** `$…$` and `$$…$$`. Common in the notebooks
this app exists to open. Needs a renderer; Pango will not do it alone.

**B6. Cell metadata.** `tags`, `collapsed`, `scrolled`. Round-tripped in the
file today but not acted on, so a notebook saved elsewhere loses its intent
when reopened here.

**B7. Output limits.** A cell printing a million lines builds a million
widgets. Jupyter truncates with a "show more"; we do not.

**B8. ipywidgets.** `application/vnd.jupyter.widget-view+json` needs the comm
protocol and a widget runtime. Large, and out of scope unless someone asks —
noted so it is a decision rather than an oversight.

## Suggested order

Grouped by what each buys, cheapest first within each group.

1. ~~**A3** — mixed magic/Python cells.~~ Done.
2. ~~**B1 + A1 together**~~ Done. The two halves were not useful apart, and
   were built and verified together.
3. ~~**B2** — `display()` and the `IPython.display` shim.~~ Done.
4. **A2** — markdown headings, lists, code blocks, rules. Now the largest
   remaining gap against the reference.
5. **B3** — the remaining MIME types: `image/svg+xml`, `text/markdown`,
   `text/latex`, `application/json`. ~~`image/jpeg`~~ done.
6. **B4** — markdown attachments.
7. **A4** — ANSI, and **B7** — output truncation. Both are small hardening.
8. **B5** — LaTeX. Bigger; worth its own discussion.
9. **B6** — cell metadata.
10. **B8** — ipywidgets, only on request.

## How to verify each

The harness can be driven directly from a script — see the measurements above.
That is faster than the UI and gives exact MIME keys, so every item here should
be checked that way first and only then in a window. `examples/` already holds
probes built this way for the FITS viewer and the icon theme.

For the renderer half, a probe that constructs a `CodeCellWidget`, feeds it a
captured output bundle, and walks the resulting widget tree will show what a
user would see without needing a kernel at all.
