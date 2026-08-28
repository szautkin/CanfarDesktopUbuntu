# 17 — Letting an agent see the WORKING AREA of the cube and FITS viewers

Status: **steps 1–4 implemented** (shared PNG encoder, the `agent_image` seam,
the four drifted copies deleted, and `get_fits_image`). Step 5 — the cube
viewer's composited working area — is next. The rest of this document is the
plan as written; "Order of work" marks what is done.

The tools also exist to enable the step after: an agent DRAWING on these images
to point a person at part of one. That is why a capture returns the transform
alongside the pixels rather than the pixels alone — see "An image an agent
cannot place is half a tool" — and why the drawing was extracted rather than
duplicated, so an annotation layer will appear on screen and in captures from
one place.

The ask: MCP tools so an agent can see the working area of the images in the
cube viewer and the FITS viewer — what the user is looking at *right now*, not
an export and not a thumbnail. The agent and the person should be able to
discuss the same picture.

That is a sharper requirement than "return an image", and the difference is
where the work is.

## What was measured

### The FITS viewer cannot be seen at all

Twelve tools: set the view, probe a pixel, read the header, blink two tabs, go
to a coordinate. None returns pixels. An agent can steer the viewer and has no
idea what it produced.

`get_fits_view` already reports the *numbers* of the working area — zoom,
viewport centre, stretch, colormap, black/white cut levels, North-Up, WCS
presence, crosshair sky position. Everything except the view itself.

### The cube viewer returns an image that is NOT the working area

`export_cube_figure` exists and returns a PNG. It calls `render_figure`, which
renders the **GL volume alone**. The working area is a composite:

| Layer | Widget | In `export_cube_figure`? |
| --- | --- | --- |
| Volume | `cube_volume_gl.rs` — `GLArea` | yes |
| Axes overlay | `cube_viewer.rs` — `DrawingArea`, drawn over it | **no** |
| Colorbar | `DrawingArea` | no |
| Transfer function | `DrawingArea` | no |

So an agent asking to see the cube gets the volume stripped of the axes the user
is reading it by — and no error, because nothing is wrong from the code's point
of view. This is the finding that most changes the shape of the work: the
existing tool is not a starting point, it is a second thing.

### The capture path differs per viewer, and one of them is easy

- **FITS** draws with **cairo** into a `DrawingArea` (`fits_canvas.rs:488`,
  `set_draw_func(|_, cr, w, h| …)`). A cairo draw function can be replayed into
  an off-screen `ImageSurface` of the same size — pixel-exact, including every
  overlay the same function paints, CPU-only, no GL, works headless.
- **Cube** needs GL for the volume and cairo for the overlay, then composited in
  the right order at the right scale. HiDPI has already caused one alignment
  bug between exactly these two layers (`fix(cube): align GL volume with axes
  overlay on HiDPI`), which is a warning about how this can be subtly wrong.

## The design

### The principle: one drawing, two destinations

The trap here is writing a second renderer for the agent's benefit. It would
start correct and drift — the screen path would gain an overlay, a colormap
change, a HiDPI fix, and the capture would quietly show something else. Nobody
would notice, because the only witness is an agent describing a picture nobody
else looked at.

So: each viewer's drawing becomes a function of `(cairo context, width, height)`
that does not know whether it is drawing to a screen or a file. The screen path
and the capture path call the same function. A change to how the viewer looks is
a change to how the agent sees it, by construction.

```rust
// fits_canvas.rs — extracted from the closure in set_draw_func.
pub fn draw_working_area(state: &CanvasState, cr: &cairo::Context, w: i32, h: i32);

// set_draw_func becomes a one-liner over it, and capture becomes:
let surface = ImageSurface::create(Format::ARgb32, w, h)?;
draw_working_area(&state, &Context::new(&surface)?, w, h);
```

For the cube, the same idea with the composite spelled out: render the GL layer
to RGBA, wrap it as a cairo source, then run the *same* overlay draw function
over it that the screen uses. The scale factor is an argument, not an ambient
value, because that is what the HiDPI bug was.

### The seam everything shares

`imageBase64` → `ToolResult::Image` is currently written **four times**, and has
already drifted:

| File | MIME | Caption |
| --- | --- | --- |
| `mcp/tools/notebook.rs` | reads `imageMime` | none |
| `mcp/tools/cube.rs` | hard-coded `image/png` | none |
| `mcp/tools/fits.rs` | hard-coded `image/png`, and its own doc says *"unused by the current FITS ops"* | none |
| `mcp/tools/research.rs` | real MIME from the fetch | `Preview of {pid}` |

Adding two more sources this way gives six copies. Instead:

- **`helpers::png`** — `encode_png_bytes` moves here from `ui/cube_tab_host.rs`,
  where it is private and where the FITS work would otherwise copy it.
- **`mcp::agent_image`** — the one place that turns pixels into a tool result:
  bounds the size, downscales, sets the MIME from the bytes, attaches the
  caption. `promote(value, limits)` becomes the only reader of `imageBase64`,
  and the four copies are deleted.

Tool families keep their own tools and their own arguments; they gain no
knowledge of encoding or budgets. SRP at the surface, DRY underneath. The test
that this is honest: adding a seventh image source touches no shared file.

### An image an agent cannot place is half a tool

A picture with no coordinates is not much use to an astronomer's agent. Each
capture returns the picture **and** the view state that produced it — for FITS
that is exactly what `get_fits_view` already reports, so the tool composes the
two rather than inventing a second vocabulary:

```
{ "imageBase64": …, "imageMime": "image/png",
  "view": { … the get_fits_view payload … },
  "capturedAt": …, "width": …, "height": … }
```

The caption says which viewer and which tab, because an agent holding four
images needs to tell them apart.

### Limits, as settings

Per the standing rule — named defaults the user can change:

| Setting | Default | Why |
| --- | --- | --- |
| `agent_image_max_dimension` | 1024 px | What a vision model uses. A 4000px capture costs ~16× the context for no more understanding. |
| `agent_image_max_bytes` | 16 MB | The budget `get_preview_image` already proved reasonable, now applied to all sources. |

Downscaling belongs in the shared layer or it will exist nowhere.
`get_preview_image` keeps today's behaviour as a floor: this must not make a
working tool start refusing what it fetches now.

## Order of work

1. ~~**`helpers::png`**~~ Done — moved out of `ui::cube_tab_host`, and given
   the tests it never had (round-trip colour, premultiplied alpha, refusal of
   impossible sizes and short buffers).
2. ~~**`mcp::agent_image`**~~ Done. The MIME now follows the BYTES, so a JPEG
   is no longer announced as a PNG; the size budget that only `get_preview_image`
   had applies everywhere; `fit_within` scales down and never up.
3. ~~**Delete the four copies.**~~ Done — the compiler named all four when
   `ToolResult::Image` gained its payload field. Verified live: `get_cell_image`
   still returns image content, and now carries its coordinates as
   `structuredContent`.
4. ~~**`get_fits_image`**~~ Done. `draw_working_area` is extracted from the
   closure and `set_draw_func` calls it, so screen and capture cannot diverge.
   Verified live against a real FITS: two captures of one view are
   byte-identical, a capture after `set_fits_view zoomPercent=400` differs, and
   the geometry was checked by eye against a synthetic ramp — no flip, no crop,
   correct quadrant.
5. **`get_cube_image`** — the working area, composited: GL volume + axes overlay
   + colorbar, at the current scale factor. `export_cube_figure` stays as it is
   (it is an export, and the reference owns the name); the new tool is what
   "show me the cube" means.
6. **Captions and view state** on all four sources, including the two that
   already return images.

## How each step is verified

- **Steps 1–3** are refactors: same results before and after, checked live.
- **Step 4 is checkable by machine.** Two renders of the same view must be
  identical, and a render after `set_fits_view` must differ — that catches a
  capture that silently ignores the view state, which is the failure that would
  otherwise reach an agent as a confident description of the wrong picture.
- **Step 5 needs an eye, once.** A composite can be plausible and wrong:
  overlay offset by the scale factor, drawn under instead of over, colorbar
  missing. The probe should write the PNG to disk so a person can look at it,
  and the HiDPI case should be one of the ones looked at.
- **A capture must equal what is on screen.** Where the widget is realized, the
  strongest check is to snapshot it via `WidgetPaintable` and compare against
  the off-screen render. If they differ, the "one drawing, two destinations"
  rule has been broken somewhere and the plan's main defence is gone.
- **Mutation-test every guard**: remove the downscale, hard-code the MIME, drop
  the byte budget, skip the overlay in the composite. Each must fail a test.

## What this does not cover

- **Animation.** Blinking two FITS tabs is a sequence; an agent gets stills.
- **GL on a headless machine.** `export_cube_figure` already answers "cube
  figure could not be rendered (GL unavailable)", so `get_cube_image` will
  inherit that; `get_fits_image` will not, because it is pure cairo. Worth
  knowing when the two behave differently on the same box.
- **Interactive round-trips.** Seeing an image and asking to zoom stays two
  calls.
