# 18 — QA test plan: agent vision (FITS and cube working areas)

**Build:** working tree on `main`, version 1.4.0, **two commits unpushed**
(`06952a0`, `221a022`). Build with `cargo build --release` and run
`./target/release/verbinal`. **Restart the app** — a running instance is the old
binary.

**What is new:** two MCP tools that let an AI agent SEE what the user is looking
at — `get_fits_image` and `get_cube_image` — plus the shared machinery under
every image the app hands to an agent, and two settings that bound it.

Everything below was run against the live app before it was written down. Where
a number is quoted, it is one that was measured, not one that is expected.

**Why this exists:** it is the first half of letting an agent draw a person's
attention to part of an image. That is why each capture returns a coordinate
transform alongside the pixels — see §6, which is the part most worth
scrutinising, because it is the part that later work depends on.

---

## Driving it

The app must be running. Everything except §7 can be checked over the socket.

```python
import socket, json, os, base64, time
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.connect("/run/user/%d/verbinal-mcp.sock" % os.getuid())
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
```

To look at a capture:

```python
r = tool("get_fits_image")
open("/tmp/cap.png", "wb").write(base64.b64decode(r["content"][0]["data"]))
```

---

## 1. The FITS working area

Open any FITS (`tool("open_fits_file", path="…")`), then:

```python
r = tool("get_fits_image")
print([c["type"] for c in r["content"]])          # ['image', 'text']
print(sorted(r["structuredContent"].keys()))
```

Expect a real **image content block** (`type: "image"`, `mimeType: "image/png"`)
— not base64 inside a text field, which is not an image to any client — and a
caption text block reading `FITS working area — <filename>`.

**The point of the tool is that it shows the CURRENT view.** Check that:

```python
a = tool("get_fits_image")["content"][0]["data"]
b = tool("get_fits_image")["content"][0]["data"]
assert a == b                       # same view, byte-identical
tool("set_fits_view", zoomPercent=400); time.sleep(1)
c = tool("get_fits_image")["content"][0]["data"]
assert c != a                       # the view changed, so the picture did
```

A capture that silently ignored the view state would pass neither, and would
reach an agent as a confident description of the wrong picture.

**Then look at one.** Pan, zoom, change the colormap, place a crosshair, start a
blink — each should appear in the capture, because the capture runs the same
drawing function the screen runs. If any of them does NOT appear, that is the
finding worth reporting: it means a second renderer has appeared somewhere.

## 2. The cube working area — and the difference from the export

This is the one to test hardest, because a tool that looks like it already
existed (`export_cube_figure`) does something else.

Open a cube, then capture both:

```python
r = tool("get_cube_image")
sc = r["structuredContent"]
open("/tmp/working_area.png","wb").write(base64.b64decode(r["content"][0]["data"]))
e = tool("export_cube_figure", width=sc["width"], height=sc["height"])
open("/tmp/export.png","wb").write(base64.b64decode(e["content"][0]["data"]))
```

**Open both files and compare them.** Measured on a synthetic cube:

| | `export_cube_figure` | `get_cube_image` |
| --- | --- | --- |
| Volume render | yes | yes |
| Wireframe box | **no** | yes |
| WCS axis captions (`RA`, `DEC`, `FREQUENCY Hz`) | **no** | yes |
| Coordinate tick labels | **no** | yes |
| Slice-plane marker | **no** | yes |
| Size | 166 KB | 224 KB |

The export is a bare blob on black. `export_cube_figure` is unchanged and is
still correct as an export — it is for composing into a document. The bug it
had, from an agent's point of view, was that it was the only thing on offer.

Then check the 2D mode: switch the viewer to the slice and capture again. The
slice draws its own overlay, so the axes box should NOT be composited over it.

## 3. Nothing open

```python
tool("get_fits_image")   # isError: True, "no FITS open"
tool("get_cube_image")   # isError: True, "no cube open"
```

Both measured. An error, not an empty image.

## 4. A viewer on a hidden tab

Open a cube, then switch the app to another page (Search, Storage) so the cube
tab is not the visible one, and capture:

```python
sc = tool("get_cube_image")["structuredContent"]
print(sc["width"], sc["height"], sc["viewportOnScreen"])
```

Expect a capture, **not** an error, with `viewportOnScreen: false`. A widget
that is not on screen has no allocation, so the size falls back to a default and
the flag says the aspect ratio did not come from the viewport.

Refusing here was the first behaviour and it was wrong: it would have meant an
agent could only look at whatever the user happened to be looking at, which is
the opposite of the point.

With the tab visible, expect `viewportOnScreen: true` and the size to match the
widget — measured 616×690 for the cube, 616×786 for the FITS canvas.

## 5. The size settings

**Settings ▸ Execution ▸ AI agent images**: two rows,
*Largest agent image (pixels)* (default 1024) and *Largest agent image (MB)*
(default 16).

Set the pixel limit to 256 and re-capture. Measured:

| tool | viewport | captured |
| --- | --- | --- |
| `get_cube_image` | 616×690 | 229×256, `scale` 0.372 |
| `get_fits_image` | 616×786 | 201×256, `scale` 0.326 |

Aspect ratio preserved, longest edge at the limit, `scale` reporting the
factor. Then check the other direction: a capture is **never enlarged** — set
the limit to 4096 and the capture should stay the viewport's size, not grow.

The limit exists because a 4000px capture costs an agent roughly sixteen times
the context of a 1000px one and tells it nothing more.

## 6. The coordinates — read this one carefully

Each capture carries, in `structuredContent`:

```
width, height              the returned raster
viewWidth, viewHeight      the viewport it came from
scale                      width / viewWidth
viewportOnScreen           whether that viewport was real
view                       the full get_fits_view / get_cube_view payload
imageMime, caption
```

**This is the half that later work depends on.** The goal is an agent that can
say "the source at the top-left of that image" and have the app ring it for the
user. That requires turning a position in the returned raster back into image or
sky coordinates, which needs `scale` and `view` together.

Worth checking that the arithmetic is consistent:

```python
sc = tool("get_fits_image")["structuredContent"]
assert abs(sc["scale"] - sc["width"] / sc["viewWidth"]) < 1e-9
v = sc["view"]        # centerX / centerY / zoomPercent, as get_fits_view reports
```

Measured on a 500×500 FITS in a 616×786 viewport at 100%: `centerX` 308,
`centerY` 393 — the viewport centre — with image pixel (0,0) at viewport (0,0),
so the image occupies the top-left and the rest is background. That looks odd in
a capture and **is correct**: it is what the viewer draws when the image is
smaller than the viewport. Do not report it as a crop.

## 7. The other two image tools still work

The plumbing under all four image sources was replaced, so the two that already
returned pictures need a regression pass:

- `get_cell_image` on a notebook cell with a figure → still image content, and
  now also carries `structuredContent`.
- `get_preview_image` with a `publisher_id` → still an image, still captioned
  `Preview of <id>`.

One behaviour deliberately changed: **the MIME type now follows the bytes.** Two
of the four families hard-coded `image/png` for whatever they were handed, so a
JPEG was announced as a PNG. If a preview that is genuinely a JPEG now reports
`image/jpeg`, that is the fix, not a regression.

---

## What is NOT covered

State plainly, so it is not re-reported:

- **Agents cannot draw on these images yet.** This is the step before that. The
  transform is returned so the next step is possible; nothing consumes it.
- **Animation.** Blinking two FITS tabs is a sequence; a capture is a still.
- **GL on a headless machine.** `get_cube_image` needs GL and will answer "cube
  could not be rendered (GL unavailable)" without it. `get_fits_image` is pure
  cairo and will not — worth knowing when the two disagree on the same box.
- **The colorbar and transfer-function panels** are not composited into the cube
  capture; the volume and its axes overlay are. Whether the colorbar should be
  in it is a design question, not a bug.
- **`export_cube_figure` still omits the overlay.** Unchanged on purpose.

## Automated checks

```
cargo test                                  # 1579 unit + 10 harness + 1 bridge
cargo run --example fits_capture_probe      # capture determinism + follows the view
cargo run --example notebook_layout_probe   # output heights in the real container
```

`fits_capture_probe` exits non-zero if a capture is blank, non-deterministic, or
does not change when the view does, and writes a PNG for a person to look at.

The cube composite has no headless check — it needs GL and a realized widget.
It is verified by probe and by eye, and guarded against silent removal by a test
that pins both the overlay call and the condition it is skipped under. That
guard was blind when first written (it scoped itself with `///` markers that the
comment stripper had already removed) and was fixed; if the composite work is
touched, re-run its mutation check rather than trusting the green tick.
