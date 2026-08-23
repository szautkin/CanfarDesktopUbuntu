# 13 — Notebook alignment: Jupyter, and the Windows app

Status: plan. Nothing here is implemented yet.

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

1. **A3** — mixed magic/Python cells. A defect, small, self-contained.
2. **B1 + A1 together** — the display protocol in the harness, and an HTML
   renderer for what it produces. This is the "images and tables work now"
   change, and the two halves are not useful apart.
3. **B2** — `display()` / `IPython.display` shim.
4. **A2** — markdown headings, lists, code blocks, rules.
5. **B3** — the remaining MIME types, starting with `image/jpeg`, which is
   already modelled and one line from working.
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
