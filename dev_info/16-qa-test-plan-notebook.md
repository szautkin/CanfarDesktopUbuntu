# 16 — QA test plan: notebook subsystem

**Build:** working tree on `parity/canfardesktop-full-sweep`, version 1.3.7,
**uncommitted**. Build with `cargo build --release` and run
`./target/release/verbinal`. **Restart the app** — an already-running instance
is the old binary.

**Covers:** the notebook rich-output report (VC-1…VC-5), the notebook task list
(NB-1…NB-8), and the file-format work. Background and root causes are in
`dev_info/13`, `14` and `15`; this file is only how to test it.

Two things it is worth knowing before starting:

- Every item below was reproduced on the old build and re-verified on the new
  one. Where a report's diagnosis turned out to be wrong, this says so.
- Three of the fixes are for bugs **not** on either list, found while checking
  the ones that were. They are marked **[extra]** and are the ones most worth
  an independent look, because nobody asked for them.

---

## How to drive it without a GUI

Most of this can be checked over the app's MCP socket, which is faster and
exact. The app must be running.

```python
import socket, json
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.connect("/run/user/%d/verbinal-mcp.sock" % __import__("os").getuid())
f = s.makefile("rw", encoding="utf-8", newline="\n")
n = [0]

def call(method, params=None):
    n[0] += 1; i = n[0]
    m = {"jsonrpc": "2.0", "id": i, "method": method}
    if params is not None: m["params"] = params
    f.write(json.dumps(m) + "\n"); f.flush()
    while True:
        line = f.readline()
        if not line: return None
        r = json.loads(line)
        if r.get("id") == i: return r

def tool(name, **args):
    return call("tools/call", {"name": name, "arguments": args})["result"]

call("initialize", {"protocolVersion": "2024-11-05", "capabilities": {},
                    "clientInfo": {"name": "qa", "version": "1"}})
f.write(json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"}) + "\n"); f.flush()

tool("create_notebook")
```

`tool(...)` returns the raw result. `result["structuredContent"]` is the parsed
object (that is NB-4); `result["isError"]` says whether it failed.

---

## 1. Rich output in the notebook window

Needs the GUI. Open a notebook, paste each cell, run it, **look at the cell**.

| # | Cell | Expect |
| --- | --- | --- |
| 1.1 | `from astropy.table import Table`<br>`Table({'name':['M31','M33'],'ra':[10.68,23.46]})` | A **table** with a border and column headings — not `<Table length=2>` |
| 1.2 | `from PIL import Image`<br>`Image.new('RGB',(140,90),'red')` | A **red picture**, about 140×90 — its own size, not stretched |
| 1.3 | `import matplotlib.pyplot as plt`<br>`plt.plot([1,2,3])`<br>`plt.show()` | A **plot**, and **no** `FigureCanvasAgg is non-interactive` warning |
| 1.4 | `class H:`<br>`    def _repr_html_(self): return "<b>bold</b> and <i>it</i>"`<br>`H()` | **bold** and *italic* text |
| 1.5 | `class S:`<br>`    def _repr_svg_(self): return '<svg xmlns="http://www.w3.org/2000/svg" width="60" height="40"><rect width="60" height="40" fill="teal"/></svg>'`<br>`S()` | A teal rectangle |
| 1.6 | `class M:`<br>`    def _repr_markdown_(self): return "# Heading"`<br>`M()` | A rendered heading — **not** `<__main__.M object at 0x…>` |
| 1.7 | `class L:`<br>`    def _repr_latex_(self): return r"$\alpha$"`<br>`L()` | The source `$\alpha$` in monospace. **Documented limit** — there is no LaTeX renderer yet |
| 1.8 | `display('one', 'two')` | Two separate outputs |
| 1.9 | `print('before')`<br>`display('mid')`<br>`print('after')` | Three outputs **in that order** |
| 1.10 | `!echo shelled`<br>`print('python ran too')` | Both lines, `shelled` **first**. This used to be a syntax error where neither ran |
| 1.11 | `print('first')`<br>`!echo second` | `first` then `second` — source order both ways |
| 1.12 | `1/0` | Traceback starts at `File "<cell>", line 1`. **No `kernel_harness.py` frames** |
| 1.13 | `def broken(:` | A SyntaxError with **no `ast.py` frames** above it |
| 1.14 | `import json`<br>`json.loads('{oops')` | Library frames (`json/decoder.py`) **kept** — those are the user's stack |

**1.2 and 1.3 are the regression to watch.** They were reported fixed once and
were not: the picture was built correctly and then allocated **one pixel of
height**, so the cell looked blank. Anything that changes notebook layout can
bring that back. There is a probe for it:

```
cargo run --example notebook_layout_probe    # exits non-zero if an output collapses
cargo run --example notebook_output_probe    # what each output builds, + a live harness run
```

---

## 2. NB-1 — cell images over the API

```python
tool("edit_cell", index=0, source="import matplotlib.pyplot as plt\nplt.plot([1,2,3])\nplt.show()")
tool("run_cell", index=0, timeout=30)
r = tool("get_cell_image", cell=0)
print(r["content"][0]["type"], r["content"][0]["mimeType"], len(r["content"][0]["data"]))
```

- Expect `image image/png` and a few thousand characters of data.
- It is a real **MCP image content block**, not base64 in a text field.
- `get_cell_output` still does **not** carry the bytes. That is deliberate —
  inlining them into every read would spend a caller's context on pixels it did
  not ask for. `richTypes` tells you an image is there; this fetches it.
- On a cell with no image, expect `isError: true` and a message naming
  `richTypes` as the way to find one.
- `output=N` picks among several images in one cell.

## 3. NB-2 — `run_cell` timeout

```python
tool("edit_cell", index=0, source="import time; time.sleep(60)")
import time; t = time.time()
r = tool("run_cell", index=0, timeout=2)["structuredContent"]
print(round(time.time() - t, 2), r["timedOut"], r["running"], r["waitedSeconds"])
print(tool("get_kernel_state")["structuredContent"]["state"])
tool("interrupt_kernel")
```

Expect ≈2s, `True True 2.0`, then `busy`. **The cell keeps running** — the
timeout is a decision to stop waiting, not to cancel work. `interrupt_kernel`
is what stops it.

Ask for a very large timeout and it is capped at the transport budget;
`waitedSeconds` reports what was actually used rather than what you asked for.

## 4. NB-3 — `run_all_cells` waits

```python
tool("clear_cell_outputs")
r = tool("run_all_cells")["structuredContent"]
print(r["running"], r["cellsWithErrors"])
print([c["outputCount"] for c in tool("get_notebook")["structuredContent"]["cells"]])
```

`running: False` means every output is final — read them immediately, no
polling. Before, this returned in 0.00s with `outputCount: 0`.

It takes the same `timeout`; if it expires you get `running: True` and the
instruction to poll `get_kernel_state` until `idle`.

## 5. NB-4 — structured content

Every structured reply now has `result["structuredContent"]` as a real object,
**and** the JSON text in `content[0].text` as before. Old clients keep working;
the spec asks for both.

## 6. NB-5 — saving an untitled notebook

`tool("create_notebook")` then `tool("save_notebook")` → the error names the fix:
*"…call save_notebook again with a `path` (a .ipynb file) to choose one"*.
`save_notebook(path=...)` has always worked; the message never said so.

## 7. NB-6 / NB-7 — tool surface and listing

- `set_notebook_session`, `get_notebook_metadata`, `get_cell_outputs` do not
  exist and are referenced by nothing. Not orphan docs.
- Reads now return **both** `type` and `cellType`; writes take `cellType`. One
  concept had two names.
- `list_notebooks` entries carry `kind` (`notebook` / `python` / `markdown` /
  `other`) and `exists`. **The `.md` entry was not a bug** — this editor really
  does open Markdown as a notebook, so it is labelled rather than hidden.

## 8. NB-8 — locale

```python
print(tool("get_kernel_state")["structuredContent"]["statusText"])
print(tool("create_notebook")["structuredContent"]["kernelStatusText"])
```

Both English, in any desktop language. `state` / `kernelState` stay
`dead|starting|idle|busy|error` — that is what to branch on.

**In the window** the status line should be translated: on a French desktop
(`LANG=fr_FR.UTF-8`) expect `Noyau : inactif`, `Noyau : occupé`. The GUI is
localised, the API is not — a reply that changed language with the operator's
locale is one a program cannot rely on.

Worth checking the **kernel dot** still tracks state in French. It used to be
coloured by searching the status sentence for the English word "idle", so
translating the sentence would have broken it.

---

## 9. File formats

| File | Opens as | Saves as |
| --- | --- | --- |
| `.ipynb` | nbformat cells | nbformat JSON — unchanged |
| `.py` | split on `# %%`, `# %% [markdown]`, `# %% [raw]` | percent-format Python |
| `.md` | fenced ```` ```python ```` → code cells, prose → markdown | Markdown |
| `.txt` `.log` | one cell, nothing parsed out of it | plain text |
| `.html` `.pdf` `.docx` `.odt` `.rtf` `.tex` | **refused**, with the reason | never written |

**9.1 [extra] — the data-loss test. Do this one first.**

```bash
printf '# %%%% [markdown]\n# Notes\n\n# %%%%\nimport numpy as np\n\n# %%%%\n2 + 2\n' > /tmp/qa.py
cp /tmp/qa.py /tmp/qa_before.py
```

Open `/tmp/qa.py` in the notebook, confirm **3 cells**, save, then:

```bash
diff /tmp/qa.py /tmp/qa_before.py && echo "IDENTICAL"
head -1 /tmp/qa.py          # must be "# %% [markdown]", NOT "{"
```

On the old build this replaced the script with nbformat JSON. Since the Open
dialog offers `*.py` and `*.md`, the way to destroy a file was to use a feature.
Repeat with a `.md` and a `.txt`.

**9.2 — `.html` is refused with a reason.** Opening one gives *"…is an exported
document, not a notebook. Converting a notebook to HTML is one way… Open the
.ipynb it was made from."* Previously it produced *"invalid notebook JSON in
report.html"*. Refusing is deliberate: HTML export is one-way and no tool reads
it back.

**9.3 — the size limit.** Settings ▸ Execution ▸ **Largest file to open (MB)**,
default 64. A file over it is refused by name and size with the setting named.
Try `head -c 100000000 /dev/urandom | base64 > /tmp/big.txt`. The limit exists
because a `.txt` in an astronomy folder is as likely to be a catalogue dump as
a page of notes.

## 10. [extra] `open_notebook` no longer lies

```python
r = tool("open_notebook", path="/tmp/qa_does_not_exist.txt")
print(r["isError"], r["content"][0]["text"][:120])
```

Expect `True` and a real message. **On the old build this returned
`isError: false` describing a completely different notebook** — whichever tab
happened to be open — because the load failure was dropped and the op reported
`current_page()`. Same shape as VC-11 (`open_fits_file` answering `opened: true`
when nothing loaded). Worth re-checking the FITS path for the same pattern.

---

## What is NOT fixed

State plainly, so nobody re-reports it:

- **LaTeX is not rendered** — `text/latex` shows its source (1.7). A renderer is
  its own piece of work.
- **ipywidgets** are not supported at all.
- **Long outputs are not truncated** — a cell printing a million lines builds a
  million widgets.
- **Cell metadata** (`tags`, `collapsed`, `scrolled`) round-trips but nothing
  acts on it.
- **Markdown cells** render bold, italic and code only — headings, lists and
  code blocks in *markdown cells* are still plain (this is separate from 1.6,
  which is markdown OUTPUT and does render).
- **Python only.** No Julia or R kernel, which is the real distance from
  Jupyter — and why `.jl`, `.R` and `.qmd` are not openable: nothing could run
  them. `.Rmd`/`.qmd` would be cheap to add if that changes.
- **pandas is not installed on the dev machine**, so the DataFrame case in 1.1
  was verified from unit tests against real `DataFrame.to_html` markup, not
  live. Worth an explicit check on a machine that has it.

## Automated checks

```
cargo test                                   # 1543 unit + 10 harness + 1 bridge
cargo run --example notebook_layout_probe    # output heights in the real container
cargo run --example notebook_output_probe    # what each MIME type builds
cargo run --example css_check                # stylesheet property errors
```

`tests/kernel_harness.rs` drives the real `data/kernel_harness.py` over the real
protocol — nothing in `cargo test` reached the Python side before, which is how
two of the defects in section 1 shipped. Cases needing matplotlib, PIL or
astropy skip loudly when those are absent rather than passing quietly.
