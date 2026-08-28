# 15 — Notebook dev tasks NB-1 … NB-8, and how close the notebook is to Jupyter

Covers the 2026-08-27 notebook task list. Every item was reproduced against the
running app over its MCP socket before being changed, and re-verified against a
rebuilt one afterwards.

## Status

| ID | Sev | Verified state |
| --- | --- | --- |
| NB-1 | Blocker | **Done** — new `get_cell_image` returns real MCP image content |
| NB-2 | Blocker | **Done** — `run_cell` takes `timeout`; measured 2.05s for `timeout: 2` |
| NB-3 | High | **Done** — `run_all_cells` now waits, like `run_cell` |
| NB-4 | Medium | **Done** — `structuredContent` on every structured reply |
| NB-5 | Low | **Done** — the message names the fix |
| NB-6 | Low | **Done** — no orphan docs; one real naming split fixed |
| NB-7 | Low | **Done, differently** — see below; the `.md` was not a bug |
| NB-8 | Low | **Done** — and the cause was worth more than the symptom |

## NB-1 — pixels

`get_cell_output` describes an image and does not carry it. That is deliberate
and stays: a base64 PNG inlined into every read spends a caller's context on
data most calls never wanted, and a client that cannot display an image gains
nothing from receiving one. What was missing is a way to *ask*.

`get_cell_image(cell, output?)` returns an MCP **image content block** — not
base64 hidden in a text field, which is not an image to any client. Verified
live: a matplotlib figure came back as `type=image, mime=image/png` with 29 484
base64 characters, a PIL image with 1 036. The `mimeType` is the type actually
found, so a JPEG-only bundle is not announced as a PNG.

A cell with no image answers with the way to find one:

> cell 3 has no image output. `get_cell_output` reports `richTypes` for every
> output — an image is one whose list contains image/png or image/jpeg.

## NB-2 / NB-3 — waiting

`run_cell` takes an optional `timeout` in seconds. Measured: `timeout: 2` on a
`time.sleep(60)` cell returned in **2.05s** with `timedOut: true`,
`running: true`, `waitedSeconds: 2.0`, and the kernel still queryable (`busy`).
The cell is never cancelled — dropping the future to time it out would kill work
that is running perfectly well.

The value is clamped to the transport's own budget. Above that the reply would
be cut off by the bridge and reported as `UI busy`, which tells a caller nothing
about their cell, so an over-long request is answered with the honest maximum
rather than refused — and `waitedSeconds` says what was actually used.

`run_all_cells` now waits the same way and takes the same argument, which was
option (a) of NB-3. `running: false` means every output is final. Measured: a
two-cell sweep returned in 0.30s with outputs already populated, where before it
returned in 0.00s with `outputCount: 0`.

## NB-4 — structured content

Structured replies now carry `structuredContent` **and** the JSON text that has
always been in `content`. Both, because the MCP specification asks servers that
send one to keep sending the other — it is all an older client understands.
Verified live: `get_notebook` returns `structuredContent` with `cellCount`,
`cells`, `kernelState` and the rest as values.

## NB-5 — saving an Untitled notebook

`save_notebook` has always accepted `path`. The error never said so:

- before: `save failed: No file path set`
- after: `save failed: this notebook has never been saved, so there is no file
  to write to; call save_notebook again with a` `path` `(a .ipynb file) to
  choose one`

## NB-6 — tool surface

`set_notebook_session`, `get_notebook_metadata` and `get_cell_outputs` are
referenced by nothing in this repository — no orphan documentation to fix.

One real drift did turn up: `add_cell` and `change_cell_type` take **`cellType`**
while every read answered **`type`**. One concept, two names, so a caller had to
know both to round-trip a cell. Reads now emit both; `type` stays because
something may already read it.

## NB-7 — `list_notebooks` scope

The `.md` entry is **not** a stray file. This editor opens `.ipynb`, `.py` and
`.md` as notebooks, and has offered all three in its Open dialog for some time —
filtering them out would have hidden something the app can do.

So the entries say what they are: `kind` is `"notebook"`, `"python"`,
`"markdown"` or `"other"`, plus `exists`, since a recents list outlives its
files.

## NB-8 — locale

The symptom was `create_notebook` answering `"Noyau : non démarré"` and
`get_kernel_state` answering `"Kernel: idle"` in the same session.

The cause was that one English sentence was doing three jobs: the text on
screen, the colour of the kernel dot (found by searching the sentence for the
word "idle"), and the `state` field over MCP (found by searching it again).
Two of the three consumers read it by substring match, so the string could not
be translated without silently reclassifying every kernel as "unknown". It was
therefore left in English — except for the one place that set the FIRST label
through the translator. Half-translated was the only stable point that
arrangement had.

`models::kernel_status::KernelStatus` makes the state a value and derives the
strings from it:

- `keyword()` — `dead` / `starting` / `idle` / `busy` / `error`. Never
  translated. The dot and the MCP `state` field.
- `api_text()` — English, always. A reply whose language followed the operator's
  desktop is a reply a program cannot rely on.
- `label()` — translated, for the window.

Both string-parsers are deleted. A test switches the app to French and asserts
the label changes while `api_text` and `keyword` do not — asserting it in an
English test process would have proved nothing.

---

# How close is the notebook to Jupyter?

## What was measured

The app already opened three formats. What it did with two of them was the
problem, and the second finding is worse than anything on the task list.

**A `.py` or `.md` arrived as ONE cell** holding the whole file. A 500-line
script was a single block that could only be run end to end — and its own cell
structure, sitting right there in the `# %%` comments, was ignored.

**Saving destroyed the file.** `save_notebook` wrote nbformat JSON to whatever
path it was handed. Open `analysis.py`, press Ctrl+S, and the script was
replaced by a JSON document. The same for a `.md`, which might be a document
someone opened only to read. Since the Open dialog offers `*.py` and `*.md`, the
way to lose a file was to use a feature. Reproduced, then fixed.

## What changed

`helpers::notebook_formats` decides the format from the path once, and each
format can both read cells and write them back:

| format | cells found | saved as |
| --- | --- | --- |
| `.ipynb` | nbformat cells | nbformat JSON, unchanged |
| `.py` | split on `# %%`, `# %% [markdown]`, `# %% [raw]` | percent-format Python |
| `.md` | fenced ```` ```python ```` blocks become code cells | Markdown |
| `.txt` `.log` | one cell; nothing is parsed out of it | plain text |
| `.html` `.pdf` `.docx` … | refused by name, with the reason | never written |

`# %%` is the convention jupytext, VS Code, Spyder and PyCharm share, so a
script a scientist already has opens as the cells they already wrote. A file
with no markers is still one cell, which is right for an ordinary script.

Only Python fences become code cells. A ```` ```bash ```` block in a document is
illustration, and turning it into a runnable cell would offer to execute text
nobody meant to run.

Round-trips are **byte-exact** — verified live: opening `live.py` through the
app and saving it left the file identical on disk. These files live in git, and
a save that appended even a blank line would produce a diff every time a
notebook was opened and closed, which teaches people to stop reading diffs.

## `.txt` and `.html` — and a worse bug found by asking

Checking what those two did today turned up something bigger than either.

**`open_notebook` reported success for a file it could not open.** `load_from_path`
returned nothing — it raised a toast and gave up — so the MCP op then called
`current_page()`, which answers with whatever tab is ALREADY open, and reported
that tab's state. Measured: asking for `notes.txt` came back `isError: false`
with a completely different notebook's cells, and an agent had no way to tell.
Same shape as VC-11 in `dev_info/14` (`open_fits_file` answering `opened: true`
when nothing loaded). `load_from_path` now returns `Result`, as the FITS
viewer's equivalent already did, and the op propagates it.

**`.txt` — supported.** Notes, instrument logs and READMEs sit beside the data,
and opening one to add a code cell under it is what a notebook is for. It loads
as a single markdown cell — nothing is parsed out of it, because a `.txt` has no
cell convention and splitting on blank lines would cut prose the author wrote
whole. Saving writes text back; a code cell added to it gets a `# %%` marker so
reopening keeps it. Verified live: opens as one cell, saves, file byte-identical.

**`.html` — refused, deliberately.** It is what a notebook is converted TO. The
conversion is one way: cells, outputs and order are not recoverable from it, and
no tool reads it back — jupytext, which is what makes `.py` and `.md` openable
anywhere, does not support it either. Opening it would have produced a single
cell of HTML source, which is not what anyone double-clicking a report wants.

What it does now is name the reason. Before, `.html` fell through to the
notebook reader and produced *"invalid notebook JSON in report.html"* — the
parser's disappointment, not the user's problem. Verified live:

> report.html is an exported document, not a notebook. Converting a notebook to
> HTML is one way — the cells, their outputs and their order are not recoverable
> from it. Open the .ipynb it was made from.

The same applies to `.pdf`, `.docx`, `.odt`, `.rtf`, `.tex`, which get the
general form listing what CAN be opened.

**A size limit came with `.txt`.** The loader reads a file into memory whole
before anything can inspect it, and the only limit was on the number of CELLS —
which a text file reaches long after the bytes are in RAM. In an astronomy
folder the file most likely to be enormous is exactly the kind now openable: a
source catalogue saved as `.txt` beside its notes. `read_within_limit` asks the
filesystem for the size first, and the ceiling is a real setting —
**Largest file to open (MB)**, default 64, clamped to 1…4096 — so a workstation
can raise it.

## What is still missing, honestly

Ranked by what it would actually buy:

1. **One kernel language.** Python only. This is the real distance from Jupyter,
   and it is much larger than any file extension: `.jl`, `.R` and `.qmd` are
   only worth opening if something can run them.
2. **`.Rmd` / `.qmd`** (R Markdown, Quarto — both common in academia). Now
   nearly free: both are Markdown with a YAML header, so they need front-matter
   handling and a fence-language decision, not a new reader. Worth doing when
   someone asks for them.
3. **Cell metadata** — `tags`, `collapsed`, `scrolled` are round-tripped but not
   acted on (B6 in `dev_info/13`).
4. **LaTeX rendering**, **ipywidgets**, **output truncation** — see
   `dev_info/13`, unchanged.

Data formats — FITS, cubes, VOTable — are not the notebook's job here; the app
has viewers for those. Adding a `.csv` reader to the *notebook* would duplicate
what `pandas.read_csv` in a cell already does better.
