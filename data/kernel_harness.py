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

import base64
import io
import json
import subprocess
import sys
import traceback as tb_mod
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


# ── helpers ───────────────────────────────────────────────────────────────────

def _emit(obj: dict) -> None:
    """Write one JSON line to stdout and flush immediately."""
    sys.stdout.write(json.dumps(obj, ensure_ascii=False) + "\n")
    sys.stdout.flush()


def _emit_boundary() -> None:
    sys.stdout.write(BOUNDARY + "\n")
    sys.stdout.flush()


def _emit_stream(name: str, text: str) -> None:
    if text:
        _emit({"output_type": "stream", "name": name, "text": text})


def _emit_error(ename: str, evalue: str, traceback_lines: list[str]) -> None:
    _emit({
        "output_type": "error",
        "ename": ename,
        "evalue": evalue,
        "traceback": traceback_lines,
    })


def _strip_harness_frames(frames: list[str]) -> list[str]:
    """Remove frames that originate inside this harness script."""
    harness = __file__ if "__file__" in dir() else "<harness>"
    cleaned: list[str] = []
    skip_next = False
    for frame in frames:
        if harness in frame and "kernel_harness.py" in frame:
            skip_next = True
            continue
        if skip_next and frame.startswith("    "):
            skip_next = False
            continue
        skip_next = False
        cleaned.append(frame)
    return cleaned if cleaned else frames


def _format_traceback(exc_type, exc_value, exc_tb) -> tuple[str, str, list[str]]:
    """Return (ename, evalue, traceback_lines) with harness frames removed."""
    ename = exc_type.__name__ if exc_type else "Exception"
    evalue = str(exc_value)
    raw_frames = tb_mod.format_exception(exc_type, exc_value, exc_tb)
    # Split the single formatted string into individual frame lines.
    all_lines: list[str] = []
    for chunk in raw_frames:
        all_lines.extend(chunk.rstrip("\n").split("\n"))
    cleaned = _strip_harness_frames(all_lines)
    return ename, evalue, cleaned


def _render_matplotlib_figures(exec_count: int) -> None:
    """Render any open matplotlib figures to base64 PNG and emit as display_data."""
    if not _HAS_MATPLOTLIB:
        return
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


def _preprocess_magic(code: str) -> str | None:
    """
    Check whether the cell is a single magic/shell line.

    Returns None if the code should be compiled normally, or handles the
    magic and returns the sentinel string "MAGIC_HANDLED".
    """
    stripped = code.strip()
    if stripped.startswith("%pip "):
        handled = _handle_magic_pip(stripped[5:], 0)
        return "MAGIC_HANDLED" if handled else None
    if stripped.startswith("!"):
        _handle_magic_shell(stripped[1:])
        return "MAGIC_HANDLED"
    return None


# ── execution core ────────────────────────────────────────────────────────────

def _execute_cell(code: str, exec_count: int) -> None:
    """
    Execute one cell of code and emit all resulting outputs.
    Always ends by calling _render_matplotlib_figures.
    Never raises.
    """
    # 1. Check for magic commands (single-line only).
    magic_result = _preprocess_magic(code)
    if magic_result == "MAGIC_HANDLED":
        _render_matplotlib_figures(exec_count)
        return

    # 2. Redirect stdout / stderr.
    old_stdout, old_stderr = sys.stdout, sys.stderr
    captured_out = io.StringIO()
    captured_err = io.StringIO()
    sys.stdout = captured_out
    sys.stderr = captured_err

    result_value = None
    exc_info = None

    try:
        # 3a. Try eval mode first (returns a value for the last expression).
        try:
            code_obj = compile(code, "<cell>", "eval")
            result_value = eval(code_obj, _NS)  # noqa: S307
        except SyntaxError:
            # 3b. Fall back to exec mode.
            code_obj = compile(code, "<cell>", "exec")
            exec(code_obj, _NS)  # noqa: S102
    except KeyboardInterrupt:
        exc_info = sys.exc_info()
    except Exception:  # noqa: BLE001
        exc_info = sys.exc_info()
    finally:
        sys.stdout = old_stdout
        sys.stderr = old_stderr

    # 4. Emit captured stream output.
    out_text = captured_out.getvalue()
    err_text = captured_err.getvalue()
    _emit_stream("stdout", out_text)
    _emit_stream("stderr", err_text)

    # 5. Emit execute_result for non-None return values.
    if exc_info is None and result_value is not None:
        try:
            plain = repr(result_value)
        except Exception:  # noqa: BLE001
            plain = "<repr failed>"
        _emit({
            "output_type": "execute_result",
            "execution_count": exec_count,
            "data": {"text/plain": plain},
            "metadata": {},
        })

    # 6. Emit error if an exception was raised.
    if exc_info is not None:
        etype, evalue, etb = exc_info
        ename, evalue_str, traceback_lines = _format_traceback(etype, evalue, etb)
        _emit_error(ename, evalue_str, traceback_lines)

    # 7. Render any matplotlib figures produced during execution.
    _render_matplotlib_figures(exec_count)


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
