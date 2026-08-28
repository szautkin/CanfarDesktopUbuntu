"""
CANFAR Verbinal Kernel Harness
==============================
A lightweight Python execution engine that communicates over stdin/stdout
using a line-delimited JSON protocol.

Protocol (stdin):
  {"type": "execute", "code": "...", "exec_count": N}
  {"type": "quit"}

Protocol (stdout):
  Zero or more output JSON lines, then one boundary sentinel line.
  Boundary: \x04__CANFAR_EXEC_BOUNDARY__\x04

Output line shapes:
  {"output_type": "stream",         "name": "stdout"|"stderr", "text": "..."}
  {"output_type": "execute_result", "execution_count": N,       "data": {"text/plain": "..."}}
  {"output_type": "display_data",   "data": {"image/png": "base64...", "text/plain": "..."}}
  {"output_type": "error",          "ename": "...", "evalue": "...", "traceback": ["..."]}
"""

from __future__ import annotations

import ast
import base64
import importlib.util
import io
import json
import subprocess
import sys
import traceback as tb_mod
import types
from typing import Any

# ── boundary sentinel ─────────────────────────────────────────────────────────
BOUNDARY = "\x04__CANFAR_EXEC_BOUNDARY__\x04"

# ── matplotlib setup (optional) ───────────────────────────────────────────────
try:
    import matplotlib
    matplotlib.use("Agg")
    import matplotlib.pyplot as plt
    _HAS_MATPLOTLIB = True
except ImportError:
    _HAS_MATPLOTLIB = False

# ── persistent user namespace ─────────────────────────────────────────────────
_NS: dict[str, Any] = {
    "__name__": "__main__",
    "__doc__": None,
    "__builtins__": __builtins__,
}
# `display` is injected below, once it is defined — notebooks are written in
# terms of it and it is the only way to show more than one thing from a cell.


# ── helpers ───────────────────────────────────────────────────────────────────

# The real stdout, captured before anything can redirect it.
#
# `_execute_cell` points `sys.stdout` at a StringIO for the duration of a cell,
# so that user `print()` output can be reported as a `stream`. Anything the
# harness emitted DURING that window went into the same buffer: `display()`
# wrote its JSON there and it came back to the client as a stream output whose
# text was a serialised display_data message. The protocol has to leave by a
# door user code cannot move.
_PROTOCOL_OUT = sys.stdout


def _emit(obj: dict) -> None:
    """Write one JSON line to the protocol stream and flush immediately."""
    _PROTOCOL_OUT.write(json.dumps(obj, ensure_ascii=False) + "\n")
    _PROTOCOL_OUT.flush()


def _emit_boundary() -> None:
    _PROTOCOL_OUT.write(BOUNDARY + "\n")
    _PROTOCOL_OUT.flush()


def _emit_stream(name: str, text: str) -> None:
    if text:
        _emit({"output_type": "stream", "name": name, "text": text})


# The buffers the running cell is printing into, or None between cells.
_CAPTURING: tuple[io.StringIO, io.StringIO] | None = None


def _flush_captured() -> None:
    """Emit what the running cell has printed so far, and clear the buffers.

    Captured output is normally emitted once, after the cell finishes. That is
    fine while the cell's only other outputs come afterwards too — but
    `display()` emits the moment it is called, so

        print("before"); display(table); print("after")

    would arrive as the table first and both prints after it. Draining the
    buffer at each display point puts the outputs back in the order they
    happened, which is what Jupyter shows and what a reader assumes.
    """
    if _CAPTURING is None:
        return
    for name, buf in (("stdout", _CAPTURING[0]), ("stderr", _CAPTURING[1])):
        text = buf.getvalue()
        if text:
            _emit_stream(name, text)
            buf.seek(0)
            buf.truncate(0)


def _emit_error(ename: str, evalue: str, traceback_lines: list[str]) -> None:
    _emit({
        "output_type": "error",
        "ename": ename,
        "evalue": evalue,
        "traceback": traceback_lines,
    })


# The filename user code is compiled under. Every frame above the first one
# bearing it belongs to the machinery that got there.
CELL_FILENAME = "<cell>"


def _strip_machinery_frames(frames: list[str]) -> list[str]:
    """Start a traceback at the user's own code.

    Two kinds of noise sat above it. This harness's own frames — the stripper
    for those asked `"__file__" in dir()` from INSIDE a function, where `dir()`
    lists locals and `__file__` is a module global, so the test was always false
    and nothing was ever stripped. And, for a SyntaxError, the stdlib frames
    from `ast.parse`, which no harness-name check would ever have caught:

        File "/usr/lib/python3.12/ast.py", line 52, in parse
          return compile(source, filename, mode, flags,
                 ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^

    Both are the same thing — frames from before the user's code started — so
    both go by the same rule, which needs no list of filenames to keep current.
    Frames BELOW the first cell frame are kept: a failure inside numpy is the
    user's stack and its frames are how they will find it.

    A traceback with no cell frame at all is left alone. That is the harness
    itself having failed, and then every frame is the interesting one.
    """
    first_cell = next(
        (i for i, f in enumerate(frames)
         if f.lstrip().startswith(f'File "{CELL_FILENAME}"')),
        None,
    )
    if first_cell is None:
        return frames
    header = frames[:1] if frames[0].startswith("Traceback") else []
    return header + frames[first_cell:]


def _format_traceback(exc_type, exc_value, exc_tb) -> tuple[str, str, list[str]]:
    """Return (ename, evalue, traceback_lines) with harness frames removed."""
    ename = exc_type.__name__ if exc_type else "Exception"
    evalue = str(exc_value)
    raw_frames = tb_mod.format_exception(exc_type, exc_value, exc_tb)
    # Split the single formatted string into individual frame lines.
    all_lines: list[str] = []
    for chunk in raw_frames:
        all_lines.extend(chunk.rstrip("\n").split("\n"))
    cleaned = _strip_machinery_frames(all_lines)
    return ename, evalue, cleaned


# ── the display protocol ──────────────────────────────────────────────────────
#
# A Jupyter kernel does not ask "is this a matplotlib figure?". It asks the
# OBJECT how it would like to be shown, through a set of methods the scientific
# stack has implemented for years: `_repr_html_` on an astropy Table and a
# pandas DataFrame, `_repr_png_` on a PIL image, `_repr_svg_`, `_repr_latex_`,
# `_repr_markdown_`, `_repr_json_`.
#
# This harness used to ask the one question, so every one of those objects came
# back as `repr()` text: a table printed as its repr, an image printed as
# `<PIL.Image.Image ...>`. Asking the object instead fixes all of them at once,
# and any library that follows the same convention in future.

# method name → MIME type it produces. Ordered richest-first, which is the order
# a renderer should prefer them in.
_REPR_METHODS = (
    ("_repr_html_", "text/html"),
    ("_repr_markdown_", "text/markdown"),
    ("_repr_svg_", "image/svg+xml"),
    ("_repr_png_", "image/png"),
    ("_repr_jpeg_", "image/jpeg"),
    ("_repr_latex_", "text/latex"),
    ("_repr_json_", "application/json"),
)

# Which of those carry binary data that must reach the client base64-encoded.
_BINARY_MIMES = ("image/png", "image/jpeg")


def _mime_bundle(obj) -> dict:
    """
    Every representation `obj` offers, as a MIME bundle.

    Always includes `text/plain`, because a client that understands none of the
    richer types still has something to show. A method that raises is skipped:
    one broken `_repr_html_` must not cost the object its image as well.
    """
    bundle = {}
    for method_name, mime in _REPR_METHODS:
        method = getattr(obj, method_name, None)
        if not callable(method):
            continue
        try:
            value = method()
        except Exception:  # noqa: BLE001
            continue
        if value is None:
            continue
        if mime in _BINARY_MIMES and isinstance(value, (bytes, bytearray)):
            value = base64.b64encode(bytes(value)).decode("ascii")
        elif not isinstance(value, str):
            # `_repr_json_` may hand back a dict; everything else should be text.
            if mime != "application/json":
                value = str(value)
        bundle[mime] = value

    try:
        bundle["text/plain"] = repr(obj)
    except Exception:  # noqa: BLE001
        bundle["text/plain"] = "<repr failed>"
    return bundle


def _display(*objects) -> None:
    """
    `display(obj)` — emit an object's representations without returning it.

    Notebooks and tutorials are written in terms of this, and it is the only way
    to show more than one thing from a single cell. It is injected into the
    user's namespace rather than imported, so it works whether or not IPython
    is installed.
    """
    # Anything printed before this call belongs before this call's output.
    _flush_captured()
    for obj in objects:
        _emit({
            "output_type": "display_data",
            "data": _mime_bundle(obj),
            "metadata": {},
        })


# Available to user code as `display(...)`, without needing IPython installed.
_NS["display"] = _display


# ── IPython.display ───────────────────────────────────────────────────────────
#
# Tutorials and notebooks are written as `from IPython.display import HTML,
# Image, Markdown`, and that import is the first line of a great many cells. It
# is worth supporting whether or not IPython is installed, and there are two
# quite different situations to handle.
#
# Every one of these classes is the same shape — hold a value, offer it under
# one MIME type — so they are generated from a table rather than written out
# seven times. `_mime_bundle` already knows what to do with each method.

# Constructor name → the `_repr_*_` it answers to.
_DISPLAY_CLASSES = {
    "HTML": "_repr_html_",
    "Markdown": "_repr_markdown_",
    "Latex": "_repr_latex_",
    "SVG": "_repr_svg_",
    "JSON": "_repr_json_",
}


def _make_display_class(name: str, repr_method: str):
    """A one-MIME wrapper class, as `IPython.display` defines it."""

    def __init__(self, data=None):  # noqa: N807
        self.data = data

    def _repr(self):
        return self.data

    def __repr__(self):  # noqa: N807
        # IPython answers `<IPython.core.display.HTML object>` here. That is a
        # poor fallback and there is no reason to copy it: `text/plain` is what
        # a client shows when it cannot render the rich type, and right now
        # markdown, latex, svg and json are exactly that. The source is
        # readable; the object marker tells the reader nothing at all.
        return self.data if isinstance(self.data, str) else f"<{name} object>"

    return type(name, (), {
        "__init__": __init__,
        repr_method: _repr,
        "__repr__": __repr__,
        "__doc__": f"Display `data` as {repr_method[6:-1]}.",
    })


class _ShimImage:
    """`Image(...)` — bytes, or a path to read them from.

    `url=` is deliberately unsupported rather than quietly fetched: a cell that
    reaches the network should say so, and a silent download is a surprise in a
    notebook running against someone else's cluster.
    """

    def __init__(self, data=None, filename=None, url=None, format=None):  # noqa: A002
        if url is not None and data is None and filename is None:
            raise ValueError(
                "Image(url=...) is not supported here; download the bytes "
                "first, e.g. with requests, and pass data=..."
            )
        if filename is not None and data is None:
            with open(filename, "rb") as handle:
                data = handle.read()
        self.data = data if isinstance(data, (bytes, bytearray)) else None
        self._text = None if self.data is not None else data
        self.format = (format or _sniff_image_format(self.data) or "png").lower()

    def _repr_png_(self):
        return self.data if self.format == "png" else None

    def _repr_jpeg_(self):
        return self.data if self.format in ("jpeg", "jpg") else None

    def __repr__(self):
        return f"<Image {self.format} {len(self.data or b'')} bytes>"


def _sniff_image_format(data):
    """The format of `data` from its first bytes, or None."""
    if not isinstance(data, (bytes, bytearray)):
        return None
    if data[:8] == b"\x89PNG\r\n\x1a\n":
        return "png"
    if data[:2] == b"\xff\xd8":
        return "jpeg"
    return None


def _build_shim_module(name: str):
    """The stand-in `IPython` package, or its `display` submodule."""
    module = types.ModuleType(name)
    if name == "IPython":
        module.__path__ = []  # marks it a package, so the submodule import works
        module.version_info = (0, 0, 0, "verbinal-stand-in")
        module.__version__ = "0.0.0-verbinal-stand-in"
        # The truthful answer: no IPython shell is running.
        #
        # Libraries probe this to decide whether they are in a notebook —
        # matplotlib's `install_repl_displayhook` calls it the moment
        # `pyplot` is imported. An earlier version of this shim omitted it and
        # every matplotlib cell died with
        # "module 'IPython' has no attribute 'get_ipython'". A stand-in that
        # answers questions wrongly is worse than no stand-in.
        module.get_ipython = lambda: None
        return module

    for class_name, repr_method in _DISPLAY_CLASSES.items():
        setattr(module, class_name, _make_display_class(class_name, repr_method))
    module.Image = _ShimImage
    module.display = _display
    # A no-op: there is no live output area to clear, and a cell calling it
    # should not fail over housekeeping.
    module.clear_output = lambda *args, **kwargs: None
    module.__all__ = [*_DISPLAY_CLASSES, "Image", "display", "clear_output"]
    return module


class _IPythonShimLoader:
    """Builds the stand-in module the finder below was asked for."""

    def create_module(self, spec):
        return _build_shim_module(spec.name)

    def exec_module(self, module):
        """Nothing to execute: `create_module` returned it fully built."""


class _IPythonShimFinder:
    """Supplies `IPython.display` on demand, and only if it is really absent.

    Appended to the END of `sys.meta_path`, so a genuinely installed IPython is
    found first and this is never consulted.

    On demand matters. The first version registered the stand-in in
    `sys.modules` at startup, whether or not the notebook mentioned IPython.
    Libraries treat the presence of that module as "we are in a notebook" —
    matplotlib reached straight for `get_ipython()` — so every cell in the app
    was running against a lie about its own environment. Built only when a cell
    actually writes the import, a notebook that never mentions IPython sees no
    trace of it.
    """

    _PROVIDES = ("IPython", "IPython.display")

    def find_spec(self, fullname, path=None, target=None):
        if fullname not in self._PROVIDES:
            return None
        return importlib.util.spec_from_loader(fullname, _IPythonShimLoader())


class _PatchingLoader:
    """Runs the real loader, then points the module's `display` at us."""

    def __init__(self, inner):
        self._inner = inner

    def create_module(self, spec):
        return self._inner.create_module(spec)

    def exec_module(self, module):
        self._inner.exec_module(module)
        module.display = _display


class _IPythonDisplayRouter:
    """Routes a REAL `IPython.display.display` through this harness.

    The library's `display()` publishes to whatever kernel is running. None is
    — this harness IS the kernel — so out of the box it falls back to printing a
    repr to stdout, and `display(table)` shows the user the same text they were
    unhappy with in the first place. Rebinding it is not rudeness toward a
    library; publishing display data is precisely the job of the kernel around
    it. Its `HTML`/`Image` classes are left alone, since they already carry the
    standard `_repr_*_` methods that `_mime_bundle` reads.

    Done at IMPORT time, from the front of `sys.meta_path`. Patching at the
    start of each cell was too late by one statement: a cell whose own first
    line is `from IPython.display import display` has already bound the
    unpatched function by the time the next cell runs.

    Nothing is imported at startup — this only acts when a cell asks — so a
    notebook that never mentions IPython pays neither the import nor the
    presence of `IPython` in `sys.modules`.
    """

    def __init__(self):
        self._resolving = False

    def find_spec(self, fullname, path=None, target=None):
        if fullname != "IPython.display" or self._resolving:
            return None
        # Re-enter the normal machinery to find the real module; the flag stops
        # this finder from answering its own question.
        self._resolving = True
        try:
            spec = importlib.util.find_spec(fullname)
        except Exception:  # noqa: BLE001
            spec = None
        finally:
            self._resolving = False
        if spec is None or spec.loader is None:
            return None  # not installed — the stand-in finder answers instead
        spec.loader = _PatchingLoader(spec.loader)
        return spec


# Order matters: the router goes first so a real IPython is found and patched,
# and the stand-in goes last so it only answers when there is nothing to find.
sys.meta_path.insert(0, _IPythonDisplayRouter())
sys.meta_path.append(_IPythonShimFinder())


def _render_matplotlib_figures() -> None:
    """Emit every open matplotlib figure as a PNG, and close it.

    Called at the end of each cell, and by `plt.show()` — which is what makes
    the two behave alike. The `exec_count` this used to take was never read;
    it is gone rather than threaded through a third caller.
    """
    if not _HAS_MATPLOTLIB:
        return
    # Called mid-cell by `show()`, where anything already printed belongs
    # first. At end of cell the buffers are gone and this does nothing.
    _flush_captured()
    try:
        fig_nums = plt.get_fignums()
        for num in fig_nums:
            fig = plt.figure(num)
            buf = io.BytesIO()
            fig.savefig(buf, format="png", bbox_inches="tight", dpi=100)
            buf.seek(0)
            png_b64 = base64.b64encode(buf.read()).decode("ascii")
            buf.close()
            # Plain-text fallback representation
            w, h = fig.get_size_inches()
            plain = f"<Figure size {w*fig.dpi:.0f}x{h*fig.dpi:.0f} with {len(fig.axes)} Axes>"
            _emit({
                "output_type": "display_data",
                "data": {
                    "image/png": png_b64,
                    "text/plain": plain,
                },
                "metadata": {"image/png": {"width": int(w * fig.dpi), "height": int(h * fig.dpi)}},
            })
            plt.close(fig)
    except Exception:  # noqa: BLE001
        # Never let figure rendering crash the loop.
        pass


def _inline_show(*args, **kwargs) -> None:
    """`plt.show()` — draw the open figures here, now.

    The Agg backend cannot open a window, so the real `show()` warns

        UserWarning: FigureCanvasAgg is non-interactive, and thus cannot be shown

    and does nothing. The figure then appeared anyway at the end of the cell,
    because that is when figures are collected — so the reader got a picture
    AND a warning telling them a picture could not be shown.

    Suppressing the warning would leave `show()` a no-op that lies by omission.
    Rendering on the spot is what `%matplotlib inline` does, it puts the figure
    where the call is rather than at the end of the cell, and the warning has
    nothing left to warn about.
    """
    _render_matplotlib_figures()


if _HAS_MATPLOTLIB:
    plt.show = _inline_show


def _handle_magic_pip(rest: str, exec_count: int) -> bool:
    """Handle %pip install ... lines. Returns True if handled."""
    rest = rest.strip()
    if not rest.lower().startswith("install"):
        return False
    try:
        result = subprocess.check_output(
            [sys.executable, "-m", "pip"] + rest.split(),
            stderr=subprocess.STDOUT,
            text=True,
        )
        _emit_stream("stdout", result)
    except subprocess.CalledProcessError as exc:
        _emit_stream("stderr", exc.output or str(exc))
    return True


def _handle_magic_shell(command: str) -> None:
    """Handle !shell command lines."""
    try:
        proc = subprocess.Popen(
            command,
            shell=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        out, err = proc.communicate(timeout=120)
        if out:
            _emit_stream("stdout", out)
        if err:
            _emit_stream("stderr", err)
        if proc.returncode != 0:
            _emit_stream("stderr", f"[Exit code {proc.returncode}]\n")
    except subprocess.TimeoutExpired:
        proc.kill()
        _emit_stream("stderr", "Shell command timed out after 120 seconds.\n")
    except Exception as exc:  # noqa: BLE001
        _emit_stream("stderr", f"Shell error: {exc}\n")


# Line prefixes handled outside the interpreter. `%` lines are IPython magics,
# `!` lines are shell. Everything else is Python.
MAGIC_PREFIXES = ("!", "%pip", "%conda", "%matplotlib")


def _is_magic_line(line: str) -> bool:
    """Whether this line is handled outside the interpreter."""
    stripped = line.strip()
    return any(
        stripped == p or stripped.startswith(p + " ") or stripped.startswith(p + "\t")
        for p in MAGIC_PREFIXES
    ) or stripped.startswith("!")


def _run_magic_line(line: str, exec_count: int) -> None:
    """Run one magic or shell line.

    Emits straight to the protocol, so anything the cell printed before this
    line has to go out first or the two arrive back-to-front.
    """
    _flush_captured()
    stripped = line.strip()
    if stripped.startswith("!"):
        _handle_magic_shell(stripped[1:])
        return
    if stripped.startswith("%pip"):
        if not _handle_magic_pip(stripped[len("%pip"):], exec_count):
            # Not an install — run it as pip anyway so `%pip --version` and
            # `%pip list` behave, rather than reaching `compile()` where a `%`
            # is a syntax error.
            _handle_magic_shell(f"{sys.executable} -m pip {stripped[len('%pip'):].strip()}")
        return
    if stripped.startswith("%conda"):
        _handle_magic_shell(f"conda {stripped[len('%conda'):].strip()}")
        return
    if stripped.startswith("%matplotlib"):
        # The backend is already Agg and figures are captured after every cell,
        # so `%matplotlib inline` is the behaviour you get regardless. Accept it
        # silently instead of failing: notebooks open with this line.
        return
    _emit_stream("stderr", f"Unsupported magic: {stripped}\n")


def _split_cell(code: str) -> list[tuple[str, str]]:
    """
    The cell as `("magic", line)` and `("code", text)` segments, in the order
    they appear in the source.

    Code segments are padded with the blank lines that precede them rather than
    being closed up, so every statement keeps its original line number and a
    traceback still points at the line the user is looking at.

    Two defects live here, both about order.

    The cell used to be tested as a WHOLE for a leading magic, so

        %pip --version
        print('and this')

    matched `%pip`, sent "--version\nprint('and this')" to pip, and when that
    was refused fell through to `compile()` — where `%` is a syntax error. The
    cell failed with "invalid syntax" and neither half ran.

    Hoisting every magic to the front fixed that but introduced the second: a
    cell of `print('first')` then `!echo second` printed `second` first, because
    all the magic ran before any of the Python. Segments run in source order,
    which is what the reference does and what a reader expects.
    """
    lines = code.replace("\r\n", "\n").replace("\r", "\n").split("\n")
    segments: list[tuple[str, str]] = []
    run: list[str] = []
    run_start = 0

    def flush_run() -> None:
        if run and "\n".join(run).strip():
            segments.append(("code", "\n" * run_start + "\n".join(run)))
        run.clear()

    for i, line in enumerate(lines):
        if _is_magic_line(line):
            flush_run()
            segments.append(("magic", line))
            run_start = i + 1
        else:
            if not run:
                run_start = i
            run.append(line)
    flush_run()
    return segments


# ── execution core ────────────────────────────────────────────────────────────

def _exec_code(code: str, want_value: bool):
    """
    Execute one run of Python from a cell; return its value, or None.

    `want_value` is what makes the last line of a notebook cell print itself.
    Only the cell's final code segment gets it: an earlier segment's trailing
    expression is mid-cell, and Jupyter shows nothing for those either.

    This used to `compile(code, "<cell>", "eval")` and fall back to exec inside
    `except SyntaxError`. Two things were wrong with that. The real error then
    raised INSIDE the except block, so Python chained it and every traceback for
    a cell starting with `import` or an assignment was headed by a phantom
    "SyntaxError: invalid syntax" that had nothing to do with the problem. And
    it only ever displayed a value for a cell that was a single bare expression,
    so `x = f()` then `x` showed nothing. Asking `ast` fixes both.
    """
    tree = ast.parse(code, CELL_FILENAME, "exec")
    last = tree.body[-1] if tree.body else None
    if want_value and isinstance(last, ast.Expr):
        head = ast.Module(body=tree.body[:-1], type_ignores=[])
        exec(compile(head, CELL_FILENAME, "exec"), _NS)  # noqa: S102
        tail = ast.Expression(last.value)
        return eval(compile(tail, CELL_FILENAME, "eval"), _NS)  # noqa: S307
    exec(compile(tree, CELL_FILENAME, "exec"), _NS)  # noqa: S102
    return None



def _execute_cell(code: str, exec_count: int) -> None:
    """
    Execute one cell of code and emit all resulting outputs.
    Always ends by calling _render_matplotlib_figures.
    Never raises.
    """
    # 1. Magic, shell and Python run in the order they were written.
    segments = _split_cell(code)
    code_at = [i for i, (kind, _) in enumerate(segments) if kind == "code"]
    if not code_at:
        # The cell was nothing but magic; there is no Python to redirect for.
        for _, line in segments:
            _run_magic_line(line, exec_count)
        _render_matplotlib_figures()
        return
    # Only the cell's final statement produces a value, so only the last code
    # segment is evaluated for one.
    last_code = code_at[-1]

    # 2. Redirect stdout / stderr.
    global _CAPTURING
    old_stdout, old_stderr = sys.stdout, sys.stderr
    captured_out = io.StringIO()
    captured_err = io.StringIO()
    sys.stdout = captured_out
    sys.stderr = captured_err
    _CAPTURING = (captured_out, captured_err)

    result_value = None
    exc_info = None

    try:
        # 3. Run the segments in source order. A raise stops the cell here, as
        #    it does in Jupyter — later segments do not run.
        for i, (kind, text) in enumerate(segments):
            if kind == "magic":
                _run_magic_line(text, exec_count)
                continue
            result_value = _exec_code(text, want_value=(i == last_code))
    except KeyboardInterrupt:
        exc_info = sys.exc_info()
    except Exception:  # noqa: BLE001
        exc_info = sys.exc_info()
    finally:
        sys.stdout = old_stdout
        sys.stderr = old_stderr
        _CAPTURING = None

    # 4. Emit captured stream output.
    out_text = captured_out.getvalue()
    err_text = captured_err.getvalue()
    _emit_stream("stdout", out_text)
    _emit_stream("stderr", err_text)

    # 5. Emit execute_result for non-None return values.
    if exc_info is None and result_value is not None:
        # Everything the object offers, not just its repr.
        _emit({
            "output_type": "execute_result",
            "execution_count": exec_count,
            "data": _mime_bundle(result_value),
            "metadata": {},
        })

    # 6. Emit error if an exception was raised.
    if exc_info is not None:
        etype, evalue, etb = exc_info
        ename, evalue_str, traceback_lines = _format_traceback(etype, evalue, etb)
        _emit_error(ename, evalue_str, traceback_lines)

    # 7. Render any matplotlib figures produced during execution.
    _render_matplotlib_figures()


# ── main loop ─────────────────────────────────────────────────────────────────

def main() -> None:
    # Ensure stdout is line-buffered so the Rust side sees output promptly.
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(line_buffering=True)  # type: ignore[attr-defined]

    while True:
        try:
            raw = sys.stdin.readline()
        except KeyboardInterrupt:
            # SIGINT during readline: ignore and continue waiting.
            continue

        if not raw:
            # EOF on stdin — the parent process closed the pipe.
            break

        raw = raw.strip()
        if not raw:
            continue

        try:
            request = json.loads(raw)
        except json.JSONDecodeError as exc:
            # Malformed request: emit an error output and the boundary so the
            # Rust side does not hang waiting for the sentinel.
            _emit_error(
                "JSONDecodeError",
                f"Malformed harness request: {exc}",
                [f"Raw input: {raw!r}"],
            )
            _emit_boundary()
            continue

        req_type = request.get("type", "")

        if req_type == "quit":
            break

        if req_type == "execute":
            code = request.get("code", "")
            exec_count = int(request.get("exec_count", 0))
            try:
                _execute_cell(code, exec_count)
            except KeyboardInterrupt:
                # Catch any KeyboardInterrupt that escapes _execute_cell
                # (should not happen, but belt-and-suspenders).
                _emit_error(
                    "KeyboardInterrupt",
                    "Execution interrupted",
                    ["KeyboardInterrupt"],
                )
            except Exception:  # noqa: BLE001
                etype, evalue, etb = sys.exc_info()
                ename, evalue_str, lines = _format_traceback(etype, evalue, etb)
                _emit_error(ename, evalue_str, lines)
            _emit_boundary()
        else:
            _emit_error(
                "ProtocolError",
                f"Unknown request type: {req_type!r}",
                [],
            )
            _emit_boundary()


if __name__ == "__main__":
    main()
