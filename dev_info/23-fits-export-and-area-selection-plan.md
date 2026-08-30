# 23 — Exporting a FITS: a figure, and only the part you meant

Status: plan. Measured against the tree at `9c17f7c`; nothing here is
implemented.

Three asks, and they are not the same feature:

1. The FITS viewer should **export a figure** the way the cube viewer does.
2. You should be able to **select an area** and export only that.
3. An agent should be able to do both, and **cut a specific area** without a
   person drawing the box.

The first is a presentation job, the second is a framing job, and keeping them
separate is most of the design.

## What exists

| Piece | Where | Reusable? |
| --- | --- | --- |
| Export dialog — format PNG/PDF, scale 1/2/4, transparent background | `cube_export::show_cube_export` | **Yes**, with the cube-specific spec lifted out |
| `write_png` / `write_pdf` | `helpers::pdf_writer` | **Yes**, already generic over `(w, h, rgba)` |
| `rgba_to_surface` / `surface_to_rgba` / `draw_over_rgba` | `cube_export` | **Yes**, already the shared pixel-format layer |
| Plate composition — title, caption, frame, colorbar, footer grid | `cube_export::PlateSpec::compose` | **Structure yes, content no** |
| `PlateOverlay` — `view_proj`, `nz`, `spectral_scale`, `CubeMetadata` | `cube_export` | **No.** Wholly cube-specific |
| A size-parameterised capture | `PlateSpec.capture: Rc<dyn Fn(i32,i32) -> Option<Vec<u8>>>` | **Yes** — this is already the right seam |
| FITS capture at an arbitrary size | `FitsCanvas::capture_png_from_view` | **Yes**, and it now scales rather than crops |

The `capture` closure is the important discovery: the plate already takes "give
me RGBA at this size" as an abstraction. A FITS viewer can satisfy it today.

**The FITS viewer has no export at all.** Not a stripped-down one — none.

## Part 1 — the figure

### Split the plate

`PlateSpec::compose` lays out title, subtitle, frame, caption, colorbar and a
footer metadata grid. That layout is not about cubes. Two things in it are:

- the **overlay** painted over the frame (wireframe box, axis captions)
- the **footer columns** (dims / RA-DEC-SPECTRAL / NaN% / mode)

So `compose` takes two injected pieces instead of a `PlateOverlay`:

```rust
/// Painted over the frame, in frame coordinates.
type FramePainter = Rc<dyn Fn(&cairo::Context, f64, f64, f64, f64)>;
/// Footer key/value columns, already resolved to strings.
type FooterColumns = Vec<(String, Vec<String>)>;
```

The cube passes what it passes today. The FITS viewer passes a painter that
draws nothing (its overlay is already in the capture — crosshair, marks — since
the canvas draws them) and footer columns of its own.

**Do not** generalise `PlateOverlay` itself. It has `view_proj`, `nz` and
`spectral_scale` in it; a shape that fits both would have half its fields unused
on either side, which is a struct pretending to be an interface.

### The FITS footer

What an astronomer needs to read a figure back:

| Column | From |
| --- | --- |
| Dimensions + HDU | `width`/`height`, `hduName` |
| Sky centre | `crosshairRa/Dec` or the frame centre through the WCS |
| Field of view + pixel scale | `pixelScaleArcsec` × dimensions |
| Cut levels + stretch + colormap | `minCut`/`maxCut` with BUNIT, `stretch`, `colormap` |
| Object / instrument / filter | `OBJECT`, `INSTRUME`, `FILTER` from the header |

The cut levels matter more here than on a cube: a FITS figure is meaningless
without saying what black and white were, and the panel now knows both the
percentile and the data value.

### The capture

`FitsCanvas::capture_rgba(view_w, view_h, w, h)`, beside the PNG one and
sharing `draw_scaled_into`. The PNG entry point becomes a thin wrapper, so the
plate and `get_fits_image` cannot drift into different pictures — which is the
same rule that already binds the screen and the capture.

## Part 2 — selecting an area

### The interaction

Left-drag is taken three times over on the FITS canvas: pan, draw a mark, and
move or resize one. Shift-drag is pan-while-drawing. Right-click places the
crosshair.

So selection is a **mode**, armed by a toggle, exactly as drawing is — and the
two are mutually exclusive, which the toggles should enforce rather than the
user remembering. Arming it:

- drag draws a rubber band; release sets the selection
- a click with no drag clears it
- Escape clears it and disarms
- the existing press-order rule (`grab_at`) stays first: a press on a mark is
  still a press on a mark, because that lesson has been learned once already

### What a selection IS

**Image pixels**, stored as `(x, y, width, height)` in the image's own
coordinates — not screen, and not sky.

- Screen dies the moment you pan or zoom.
- Sky cannot express a selection on an image with no WCS, and every FITS has
  pixels.
- Image pixels survive zoom, pan, rotation and a window resize, and are what
  `probe_fits_pixel` and `annotate_fits` already speak.

The **sky equivalent is reported alongside** when there is a WCS, because that
is what makes a selection meaningful across two images of the same field — and
it is what an agent asking for "the same region on the other frame" needs.

Held on the tab, not the canvas: it belongs to the image, survives a tab switch,
and a canvas is rebuilt on every HDU change.

### Drawing it

A rubber band while dragging, then a persistent rectangle with corner marks.
Reuse `annotation_render::draw_handles` for the corners if the selection becomes
resizable — but **not in the first pass**. Resizable selections need the whole
grab/intent machinery again, and the cheap version (draw a new one) is what
people do anyway.

Draw it in `draw_area_inner` under `chrome`, so it appears on screen and **not**
in `get_fits_image` or an export. A selection is a tool for choosing a frame,
not part of the picture.

### What "export the selection" means

The selection changes **what is captured**, not how it is presented. So the
dialog gains one control:

- **Area**: `Whole view` / `Selection` / `Whole image`
- and the existing **Format**, **Scale**, **Transparent** are unchanged

`Whole image` is worth having and is not the same as zooming out: it exports the
frame at its own pixel grid, which is what you want for a finder chart.

Presentation stays orthogonal: **Plate** (framed, with the footer) or **Plain
image** (the pixels, nothing else). The cube only offers a plate; a plain crop
is most of the point of selecting an area, so the FITS viewer offers both and
the cube can gain the choice later.

## Part 3 — the agent

### `export_fits_figure`

Mirrors `export_cube_figure` — `path` writes a file, no path returns base64 —
with the same `width`/`height`/`scale`/`transparent`/`format`.

### The region

Four ways to say it, because an agent has four different amounts of knowledge:

| `region` | Means |
| --- | --- |
| `"view"` (default) | What is on screen. The same picture `get_fits_image` returns. |
| `"image"` | The whole frame at its own pixel grid. |
| `"selection"` | What the user selected. **Fails clearly if nothing is selected** rather than silently exporting the view. |
| an object | An explicit box |

An explicit box in either space:

```json
{ "x": 2600, "y": 2200, "width": 400, "height": 400 }
{ "ra": 202.4694, "dec": 47.1959, "widthArcsec": 30, "heightArcsec": 30 }
```

Sky is the one that lets an agent cut the same region from two frames of the
same field, which is the thing this is for. It needs a WCS and says so when
there is none.

### Setting the selection

`set_fits_view` gains `selection` — the same shapes as above, plus `null` to
clear. An agent can then select and let the user see what it chose, which is the
point of a shared workspace: it is the difference between an agent handing over
a picture and an agent pointing at the screen.

`get_fits_view` reports `selection` with both the pixel box and its sky
equivalent, so a settable control can be read back — the guard will insist
anyway.

## Order of work

1. **`capture_rgba`** on the canvas, sharing `draw_scaled_into`. Small, and the
   plate cannot be built without it.
2. **Split `compose`** onto the painter + footer columns; the cube keeps working
   and its tests keep passing. No new behaviour yet.
3. **The FITS plate and dialog**, with `Area: Whole view` only. Shippable here:
   the viewer gains an export it has never had.
4. **The selection model** on the tab — set, clear, report, with the sky
   equivalent. Unit-testable without a widget.
5. **The selection gesture** and its rectangle.
6. **`Area: Selection / Whole image`** in the dialog.
7. **`export_fits_figure`** and `selection` on `set_fits_view` / `get_fits_view`.

Steps 1–3 stand alone and are worth shipping on their own.

## What this does not cover

- **A FITS cutout.** Exporting *data* — a new FITS file with the selected pixels
  and a corrected WCS — is a genuinely useful and completely different feature:
  it is about the numbers, not the picture, and its correctness question is
  CRPIX arithmetic rather than layout. Worth doing, separately.
- **Multiple selections.** One box. A second one is a marks problem, and marks
  already exist.
- **Selection on the cube.** The plan above is FITS-only; the cube's slice could
  take the same model, but its volume cannot — a box on a projected volume is
  not a box in the data.
- **Print / page setup.** PDF here is a single page sized to the plate, as the
  cube's already is.

## The risk worth naming

The plate work touches `cube_export`, which is 1015 lines of layout that
currently works and has no probe over its composed output — only unit tests on
the scale and format lists. Splitting `compose` is a refactor of working
functionality with no pixel-level guard underneath it.

So step 2 gets one first: a probe that composes a cube plate before and after
the split and compares the rasters. Without it the refactor is unfalsifiable,
and "the cube export still looks right" is not something the test suite can
currently tell anyone.
