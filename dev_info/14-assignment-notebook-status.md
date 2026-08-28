# 14 — Development Team Assignment: notebook workstreams, verified

Covers the notebook items of the 2026-08-21 assignment: **VC-1 … VC-5**
(Workstream 1) and **VC-9, VC-10** (Workstream 3). The catalog, FITS-viewer and
MCP-transport workstreams are untouched — see "Not looked at" at the end.

Every finding below was re-tested against the current tree before being acted
on, through the real harness and the real widget, per the assignment's global
definition of done. Where the report's diagnosis and the measurement disagree,
the measurement is recorded.

## Status

| ID | Sev | Assignment says | Verified state |
| --- | --- | --- | --- |
| VC-1 | P1 | `image/png`/`image/jpeg` not rendered | **Done** |
| VC-2 | P1 | generic `text/html` not rendered | **Done** (one caveat below) |
| VC-3 | P2 | need a branch per MIME type | **Done** |
| VC-4 | P3 | add `richTypes[]` | **Done** |
| VC-5 | P3 | `plt.show()` warns noisily | **Done** |
| VC-9 | P2 | dependency tools answer "no such tool" | **Already shipped** — in HEAD, with a guard |
| VC-10 | P2 | `run_cell` blocks the MCP UI | **Already shipped** — in HEAD |

## The assignment's root-cause line needs one correction

> `richest()` now parses image/html correctly (harness OK), but the widget only
> paints `text/plain` and the astropy-Table grid.

The widget was not the only half missing. Measured against 1.3.7, the **harness**
produced no `image/png` for a PIL image and no `text/html` for an astropy Table
either — both came back as `text/plain` only, because the harness asked one
question ("is this a matplotlib Figure?") instead of asking the object. So VC-1
and VC-2 each needed a fix on both sides. See `dev_info/13` for that work.

## VC-1 — images render (and the round it took to actually work)

The first fix was reported as done on the strength of a probe that walked the
widget tree and found a `GtkPicture` for every image. A screenshot came back
with blank cells anyway. The probe was not lying and the picture was not
missing — it was **allocated one pixel of height**.

`GtkPicture` with `can_shrink` set reports a MINIMUM height of zero: it will
consent to being drawn in no space at all. The notebook packs cells into a
`GtkListBox`, which allocates its rows their minimum. So every image output was
decoded, textured, appended — and then squeezed to nothing. Labels survived
because a label's minimum height is its text, and the astropy table survived
because it sits in a `ScrolledWindow` with `propagate_natural_height`. That is
why HTML looked fixed and images did not.

The probe missed it by laying widgets out in a plain `GtkBox` with room to
spare, which hands out NATURAL sizes. Every unit test had the same blind spot.
The bug is only visible in the container the app actually uses.

The fix states the size instead of leaving it to be negotiated:
`output_image_size` derives a width and height from the texture, capped at
`OUTPUT_IMAGE_WIDTH`, and that becomes the picture's size request. A side
effect worth having: small images are no longer upscaled to a fixed 400px, so
a 140x90 thumbnail renders at 140x90 rather than blurred across the cell.

Measured in the app's own container, before and after:

| output | before | after |
| --- | --- | --- |
| 140x90 PIL image | 21px | 90px |
| 640x480 figure | 21px | 300px |
| 120x60 SVG | 21px | 60px |
| html table (control) | 34px | 34px |
| stream text (control) | 31px | 31px |

`examples/notebook_layout_probe.rs` is that measurement, kept: it builds the
app's real `ScrolledWindow`/`ListBox`/`ListBoxRow` stack and **exits non-zero**
if any output gets less than 24px. Verified both ways — it exits 1 against the
old sizing and 0 against the new.

### What the data path looked like all along

Worth recording, because it was ruled out one layer at a time and none of it
was at fault: the harness emitted the PNG, the reader parsed it, the document
stored it, and `get_cell_output` on the LIVE app reported
`richTypes=['image/jpeg','image/png','text/plain']` for the PIL cell and
`['image/png','text/plain']` for the figure. The bytes were always there. Only
the last step — how much room the picture was given — was wrong.

## VC-1 — what renders now

Both output paths, verified live:

| cell | widget builds |
| --- | --- |
| `Image.new('RGB',(8,8),'red')` (execute_result) | `GtkPicture` |
| `display(Image.new(...))` (display_data) | `GtkPicture` |
| `plt.plot([1,2,3])` | `GtkPicture` |

`image/jpeg` works with no format-specific code: `Texture::from_bytes` sniffs
the format, so a JPEG-only bundle (a PIL image with `_repr_jpeg_` and no PNG)
paints. Corrupt bytes fall back to `text/plain` rather than leaving the cell
blank.

## VC-2 — generic HTML renders

A class with a custom `_repr_html_` returning `<b>bold</b> and <i>it</i>`
renders as formatted text, not as `<obj>`. An `astropy.table.Table` renders as a
real `GtkGrid` — verified end to end, python through to widget tree.

**Caveat, stated rather than glossed:** `pandas` is not installed on this
machine, so the DataFrame half of the acceptance criterion is **not live
verified**. What is verified: the harness half is generic (it asks for
`_repr_html_`, which is what a DataFrame offers — proven with a custom class),
and the widget half is unit-tested against real `DataFrame.to_html` markup,
including the `<tbody>` and index-column shape pandas emits and astropy does
not. A live check needs pandas installed; the system Python is externally
managed (PEP 668), so that is a deliberate decision rather than something to do
silently.

## VC-3 — a branch per MIME type

Each of these previously fell through to `text/plain`, which for an object
without a `__repr__` is a memory address — `<__main__.M object at 0x7f…>`:

| MIME | branch |
| --- | --- |
| `image/svg+xml` | rasterised to a picture. Needs librsvg; if it is absent the load fails and the output falls back to text rather than going blank |
| `text/markdown` | the same renderer markdown CELLS use |
| `text/latex` | **documented fallback** — the source, in monospace. There is no LaTeX renderer yet (B5 in `dev_info/13`) |
| `application/json` | pretty-printed, in monospace |

The bug class is now closed structurally, not just fixed: the widget matches
`Representation` **exhaustively**, so a MIME type added to `richest()` will not
compile until it has somewhere to go. `image/jpeg` and `text/html` reached
production modelled-but-unrendered through a chain of `if let`s that just ran
out; that cannot happen again.

## VC-4 — `richTypes[]`

`get_cell_output` and `run_cell` now report `richTypes` — every MIME the output
carries, sorted. Present on **every** output type and empty where there is
nothing rich, so a caller never has to branch on output type before reading it.
The tool description lists the types an agent should expect.

## VC-5 — `plt.show()`

Reproduced exactly:

```
<cell>:3: UserWarning: FigureCanvasAgg is non-interactive, and thus cannot be shown
```

The figure then appeared anyway at the end of the cell, so the reader got a
picture *and* a warning saying a picture could not be shown.

Suppressing the warning would have left `show()` a no-op that lies by omission.
Instead `plt.show()` now renders the open figures on the spot and closes them —
what `%matplotlib inline` does. The warning has nothing left to warn about, the
figure appears where the call is rather than at the end of the cell, and it
appears exactly once.

## VC-9 and VC-10 — already shipped

Both are fixed in `HEAD`, before this assignment was written.

**VC-9.** The host implemented the handlers as `check_dependencies` /
`install_dependencies` while the catalogue advertised the `_notebook_` spelling,
so the two halves never met. The host ops were renamed to match, and
`every_advertised_notebook_tool_is_dispatchable` now guards the seam — scoped to
the `dispatch` function, because a first version that searched the whole file
matched each tool's own declaration in `descriptors()` and passed with both
tools still unwired.

**VC-10.** `run_cell` detaches the execution and waits `RUN_CELL_WAIT` — derived
as two thirds of the bridge's own timeout rather than written down a second
time. If the cell is still going it returns `running: true` with the outputs so
far and tells the caller to poll `get_cell_output`. The execution is never
cancelled, which dropping the future to time it out would have done.

## How each is guarded

`tests/kernel_harness.rs` (10 tests) drives the real python script over the real
protocol; `examples/notebook_output_probe.rs` walks the real widget tree for
each bundle and then runs the live harness end to end. Model-level tests cover
`richest()` and `mime_types()`.

Every guard was mutation-tested — the code broken deliberately, the test
confirmed to fail. That caught two things reading would not have:

- The `plt.show()` fix initially had **no test at all**, only a manual probe.
- Both tests requiring IPython to be *absent* were **skipping silently**: their
  check used `importlib.util.find_spec`, which walks `sys.meta_path` — where
  the harness had just installed its own IPython stand-in. The test asked "is
  IPython installed?" and got the harness's own answer back.

## Not looked at

Workstreams 2, 4 and 5 — VC-6/7/8 (VizieR), VC-11/12 (FITS open + download
paths), VC-13/14/15 (bridge stability, tool-name churn) — and the VC-QA
regression suite for the already-fixed items.
