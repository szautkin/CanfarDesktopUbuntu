# 23 — Drag a region of a FITS, get a figure

Status: plan. Measured against the tree at `9c17f7c`.

**Third draft.** The first proposed porting the cube's publication plate and
splitting `cube_export::compose` to share it — answering "the same as the cube
viewer" literally instead of answering the ask. The second dropped the plate but
kept a *persistent selection*: a box you set, that lives on the tab, that a
dialog later asks about.

The interaction as actually wanted is simpler than either:

> A dashed-square button on the panel arms **select area** mode. You drag, and
> **the release is the capture** — a dialog opens there and then with the same
> options the cube's export has.

That removes a whole model from the plan. There is no selection to store, no
selection to report, and no "Area" dropdown in the dialog: the area is the thing
you just dragged. What is left is a gesture, a transform, a dialog, and three
bugs.

## The flow

1. **`edit-select-symbolic`** — Adwaita's dashed marquee square — as a toggle in
   the panel, beside the drawing pencil. Adwaita rather than Yaru's
   `image-crop-symbolic`: Yaru is Ubuntu's, and the app is meant to hold up on
   other distributions.
2. Armed, the pointer is a crosshair and left-drag draws a rubber band.
3. **Release captures.** The dialog opens with the region already taken.
4. Cancel discards it; the mode stays armed, because someone who cancelled is
   about to drag again.
5. Escape or the toggle leaves the mode — the same way the pencil behaves, so
   there is one rule for both.

Select and draw are mutually exclusive, enforced by the toggles rather than by
the user remembering.

`grab_at` still runs first: a press on an existing mark is a press on that mark.
That lesson has been learned once already, when the pencil stole every press.

## The dialog

`cube_export::show_cube_export`'s shape — an `adw::Window`, transient for the
root, an `adw::PreferencesGroup` of rows, Cancel and Save:

| Row | Values |
| --- | --- |
| Scale | 1× · 2× · 4× |
| Transparent background | off |
| Format | PNG · PDF |

No Area row. No title, caption, colorbar or footer — that is the plate, and the
plate is not what was asked for. Should a framed figure be wanted later it is a
presentation layer over the same capture, shareable with the cube then, and it
needs a raster probe over `compose` before that refactor is safe.

## Rendering the region

### Not a crop

Capturing the view and cropping the raster ties the export's resolution to the
window: a small region at 25% zoom would export as a handful of blurry pixels,
and the 4× control would do nothing.

Instead **substitute the transform** — save the canvas transform, set one that
puts the region's origin at the raster origin at the chosen scale, draw, restore.
The image, the crosshair and every mark all project through `self.transform`, so
substituting it moves them together and correctly at any output size. This is
the payoff of having a single drawing function, and what a second renderer would
throw away.

### Two ways to build that transform, and the difference matters

**A dragged region is in SCREEN space.** So the substituted transform is the
current one, translated by the region's origin and scaled. That preserves
rotation, zoom and pan exactly — "what you see is what you get", including a
north-up rotated frame, where a screen-aligned drag is *not* a rectangle in
image pixels.

**An agent's region is in IMAGE PIXELS** (or on the sky), and must work whatever
the view happens to be showing. There the transform is built from scratch for
that image box.

One mechanism, two constructors. Worth writing down because the rotated case is
where a single "convert the drag to image pixels" shortcut quietly produces the
wrong rectangle.

## Three bugs to fix first

These affect `get_fits_image` today, independent of any export.

### 1. The in-progress shape leaks into captures

`draw_area_inner` draws `pending_shape` — the rubber band you are dragging out —
**above** the `if chrome` guard, so a capture taken mid-drag contains a half-made
mark. The cube's equivalent *is* guarded, so the two viewers already disagree.

### 2. Selection and edit highlighting leak into captures

`draw` is handed `selected_annotation` and `editing_annotation`, so a mark that
happens to be clicked renders white and one being edited renders amber — in the
exported figure. A reader sees one ring in a different colour from the rest and
no way to know it means "this was selected". UI state must not survive into a
deliverable: the same rule as the grips, half-applied.

### 3. Marks become hairlines at scale — **wrong, measured**

This draft claimed `style::STROKE`'s fixed 1.0 would leave marks four times
finer in a 4× export. It does not. Cairo's line width is in **user space**, and
`draw_scaled_into` applies `cr.scale()`, so strokes follow the export like
everything else.

Measured on a ring at four output sizes, by the run length where a scanline
crosses it:

| output | 1× | 2× | 4× | 8× |
| --- | --- | --- | --- | --- |
| stroke | 2 px | 2 px | 4 px | 8 px |

The 1× and 2× both read 2 because antialiasing spreads a 1 px stroke over two
partially-covered pixels; from 2× on, the doubling is exact. Total ink grows
12.4× between 1× and 4×, against 4× for a true hairline and 16× for a stroke
that scales in both width and length.

No work, and the claim is struck rather than left standing. Writing a plan is
worth it partly for the items it deletes.

Annotations otherwise need no work either: `annotation_render::draw` sits
outside the `chrome` guard, so every capture already contains the marks.

## The agent

`export_fits_figure`, mirroring `export_cube_figure`: `path` writes a file, no
path returns base64, plus `scale` / `transparent` / `format`.

| `region` | Means |
| --- | --- |
| `"view"` (default) | What is on screen — the picture `get_fits_image` returns |
| `"image"` | The whole frame on its own pixel grid |
| `{x, y, width, height}` | An explicit box in image pixels |
| `{ra, dec, widthArcsec, heightArcsec}` | A box on the sky; needs a WCS and says so when there is none |

No `"selection"` and nothing added to `set_fits_view`: with the release-captures
flow there is no stored selection for either to refer to. An agent that wants to
point at a region draws a **rect mark**, which it can already do, and which is
visible to the user in a way a transient rubber band is not.

## Order of work

1. ~~**The three bugs.**~~ **Two bugs** — the third was measured away. Done at
   `9c17f7c`+: the preview is under `chrome`, and selection/edit ink no longer
   reaches a capture. Both were wrong in what shipped.
2. **`capture_region_rgba`** — the substituted transform, with the whole-view
   capture as the case where the region is the view. Testable through the
   stated-view entry point that already exists.
3. **The export dialog**, driven by the whole view. Shippable on its own: the
   viewer gains an export it has never had.
4. **The select-area toggle and gesture**, with the rubber band drawn under
   `chrome` so it never lands in the picture.
5. **Release opens the dialog** with the dragged region.
6. **`export_fits_figure`** with its four region forms.

## What this does not cover

- **A FITS cutout** — a new FITS of the selected pixels with a corrected CRPIX.
  Useful, and a different feature: it is about the numbers rather than the
  picture, and its correctness question is WCS arithmetic.
- **The publication plate.** Deliberately dropped.
- **A persistent selection.** The release is the capture; there is nothing to
  keep.
- **Selection on the cube.** Its slice could take this; its volume cannot,
  because a box on a projected volume is not a box in the data.

## How it gets verified

The pixel questions are the ones that shipped wrong last time, so they get a
probe rather than an assertion:

- a region export contains the marks inside it and none of those outside;
- the same region at 1× and 4× is the same picture — by centre of mass and lit
  fraction, the measurement that caught the capture crop;
- a selected or edited mark exports in the ordinary ink;
- a shape mid-drag does not appear at all;
- on a north-up rotated frame, a screen-aligned region exports what was on
  screen — the case a convert-to-image-pixels shortcut gets wrong.
