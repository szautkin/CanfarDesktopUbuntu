# 23 — Export the selected area of a FITS, with its marks, as PNG or PDF

Status: plan. Measured against the tree at `9c17f7c`.

**This replaces a first draft that was too big.** That version proposed porting
the cube's whole publication *plate* — title, caption, colorbar, WCS footer —
and splitting `cube_export::compose` to share it. None of that is needed for
what was actually asked, and the split was the one genuinely risky step in it:
1015 lines of working layout with no probe over its composed output.

The ask is narrower and better: **select part of the image, export what you see
there — marks included — as PNG or PDF.** Written down, most of the work turns
out to be already done, and three things are quietly wrong.

## What the ask needs that already exists

- **Annotations come free.** `annotation_render::draw` sits *outside* the
  `if chrome` guard in `draw_area_inner`, so every capture already contains the
  marks. Only the resize grips are chrome. Nothing to build.
- **PDF and PNG writing.** `helpers::pdf_writer::{write_png, write_pdf}` are
  already generic over `(width, height, rgba)`.
- **Format-conversion plumbing.** `cube_export::{rgba_to_surface,
  surface_to_rgba, draw_over_rgba}`.
- **A capture that scales rather than crops**, as of `9c17f7c`.

So the plate is not required, `cube_export::compose` is not touched, and the
risk named in the first draft disappears. What remains is a region, a dialog,
and three fixes.

## Three things that would ship wrong

These are the reason to write a plan rather than start typing.

### 1. The in-progress shape leaks into exports

`draw_area_inner` draws `pending_shape` — the rubber-band circle you are
dragging out — **unconditionally**, above the `if chrome` guard. An export
taken while a shape is being dragged would contain a half-made mark.

The cube's equivalent *is* guarded (`if chrome` around its preview), so the two
viewers already disagree. Move it under `chrome`.

### 2. Selection and edit highlighting leak into exports

`draw` is passed `selected_annotation` and `editing_annotation`, so a mark that
happens to be selected renders in white and one being edited renders in amber —
**in the exported figure**. A reader of that figure sees one ring in a different
colour from the others and has no way to know it means "this was clicked".

UI state must not survive into a deliverable. The capture and export paths pass
`None` for both; the screen keeps them. This is the same rule as the grips, and
it was half-applied.

### 3. Marks become hairlines at export scale

`style::STROKE` is a fixed 1.0 device pixels, deliberately not scaled with zoom
— correct on screen, where a thickening stroke turns a zoomed-out view into a
blot. But an export at 4× draws a 1px stroke on a raster four times larger, so
the marks come out four times finer than they look on screen, which is not what
anyone means by "export what I selected".

The stroke scales with the **output-to-view ratio** for a capture, not with the
zoom. On screen that ratio is 1 and nothing changes.

## The region

### What a selection is

**Image pixels**, `(x, y, width, height)`, held on the tab.

- Screen coordinates die on the next pan.
- Sky cannot express a selection on a frame with no WCS, and every FITS has
  pixels.
- Image pixels survive zoom, pan, rotation and a window resize, and are the
  space `probe_fits_pixel` and `annotate_fits` already speak.

On the tab rather than the canvas because a canvas is rebuilt on every HDU
change, and the selection should not be.

The sky equivalent is *reported* when there is a WCS — that is what lets an
agent cut the same region from another frame of the same field.

### Rendering a region, not cropping one

The naive version — capture the view, crop the raster — ties the export's
resolution to the window. A small selection at 25% zoom would export as a
handful of blurry pixels.

Instead, draw with a **substituted transform**: scale `= output_width /
region_width`, offset placing the region's origin at the raster origin. Save the
canvas transform, set it, draw, restore.

This is the payoff of having one drawing function: the image, the crosshair and
every mark are all projected through `self.transform`, so substituting it moves
all of them together and correctly, at any output size. A cropped raster cannot
do that, and a second renderer would drift.

### Interaction

Left-drag on this canvas is already pan, draw a mark, and move or resize one;
Shift-drag is pan-while-drawing; right-click places the crosshair. So selection
is a **mode with its own toggle**, mutually exclusive with drawing — enforced by
the toggles, not by the user remembering.

- drag draws a rubber band; release sets it
- a click with no drag clears it
- Escape clears and disarms
- `grab_at` still runs first: a press on a mark is a press on a mark. That
  lesson has been learned once already.

The selection rectangle draws under `chrome`, so it appears on screen and never
in the export. It is the tool for choosing the frame, not part of the picture.

**Not resizable in the first pass.** Resize needs the whole grab/intent
machinery again, and drawing a new box is what people do anyway.

## The dialog

`cube_export::show_cube_export`'s shape, without the plate:

| Control | Values |
| --- | --- |
| Area | Selection (default when one exists) · Whole view · Whole image |
| Format | PNG · PDF |
| Scale | 1× · 2× · 4× |
| Transparent background | off |

"Whole image" is not the same as zooming out: it exports the frame on its own
pixel grid, which is what a finder chart wants.

No title, caption, colorbar or footer. If a framed plate is wanted later it is a
separate presentation layer over the same capture, and can be shared with the
cube then — with a raster probe over `compose` first, which is what the earlier
draft should have said instead of proposing the split up front.

## The agent

`export_fits_figure`, mirroring `export_cube_figure`: `path` writes a file, no
path returns base64, plus `scale` / `transparent` / `format`.

`region` says what to cut:

| `region` | Means |
| --- | --- |
| `"view"` (default) | What is on screen — the picture `get_fits_image` returns |
| `"selection"` | What the user selected. **Fails clearly when there is none**, rather than quietly exporting the view |
| `"image"` | The whole frame, own pixel grid |
| `{x, y, width, height}` | An explicit box in image pixels |
| `{ra, dec, widthArcsec, heightArcsec}` | An explicit box on the sky; needs a WCS and says so when there is none |

`set_fits_view` gains `selection` (the same shapes, `null` clears) and
`get_fits_view` reports it with both boxes. An agent that can *set* the
selection can show a person what it is about to export, which is the difference
between handing over a picture and pointing at the screen.

## Order of work

1. **The three leaks** — preview under `chrome`, no selection/edit ink in a
   capture, stroke scaled by the output ratio. Independent of everything else,
   and they are bugs in what already ships: `get_fits_image` has them today.
2. **`capture_region_rgba`** on the canvas — the substituted transform, with the
   view-sized capture becoming the special case where the region is the view.
   Unit-testable through the existing stated-view entry point.
3. **The selection model** on the tab: set, clear, report, sky equivalent.
   No widget needed, so it is unit-testable.
4. **The export dialog**, Area: Whole view / Whole image only. Shippable — the
   viewer gains an export it has never had.
5. **The selection gesture** and its rectangle.
6. **Area: Selection**, and `export_fits_figure` + `selection` over MCP.

Step 1 is worth doing on its own whatever happens to the rest.

## What this does not cover

- **A FITS cutout** — a new FITS file of the selected pixels with a corrected
  CRPIX. Genuinely useful and a different feature: it is about the numbers, not
  the picture, and its correctness question is WCS arithmetic rather than
  layout.
- **The publication plate.** Deliberately dropped; see above.
- **Multiple selections.** One box. A second is what marks are for.
- **Selection on the cube.** The slice could take this model; the volume cannot,
  because a box on a projected volume is not a box in the data.

## How it gets verified

The pixel questions are the ones that shipped wrong last time, so they get a
probe rather than an assertion:

- a region export contains the marks that fall inside it, and none of the ones
  that do not;
- the same region at 1× and 4× is the same picture, by centre of mass and lit
  fraction — the measurement that caught the capture crop;
- a mark's stroke is the same *relative* weight at 1× and 4×;
- a selected or edited mark exports in the same ink as any other;
- a shape mid-drag does not appear at all.
