# 24 — Styling a mark: colour, weight, size, thickness

Status: **done**. Steps 1–4 landed in `9de3b29` and `43eb1a4`; steps 5 and 6
are below the line at the end of this file, with what changed from the plan.

The ask: font size, font weight and colour for a mark's label, and thickness for
its outline.

The whole reason this is a small job is that **there is one renderer**.
`annotation_render::draw` serves the FITS canvas, the cube's volume, the cube's
slice, every agent capture and both exports. Style added there is style
everywhere, and nothing else has to be told.

## What is there now

`annotation_render::style` is a module of constants:

| | | |
| --- | --- | --- |
| `STROKE` | 1.0 | Hairline, deliberately not scaled with zoom |
| `SELECTED_STROKE` | 2.0 | |
| `FONT_SIZE` | 11.0 | |
| `INK` | cold white-cyan | The drawing ink |
| `AGENT_INK` | green | An agent's marks |
| `EDITING_INK` | amber | The mark being edited |
| `SELECTED_INK` | white | The mark picked out |
| `ALPHA` | 0.92 | |

`ink_for(mark, selected, editing)` picks between them: editing wins, then
selected, then author.

**Two consumers live outside the renderer**, and both matter here:

- `fits_canvas::label_bounds` computes a label's clickable box from
  `style::FONT_SIZE`. Per-mark sizes make that constant wrong, and the symptom
  is that clicking the words on a restyled mark stops opening it — which reads
  as the click being broken, not the metrics.
- `fits_canvas` also estimates text width from the same constant.

## The model

One struct, defaulting to exactly today's look:

```rust
pub struct MarkStyle {
    pub colour: (f64, f64, f64),
    pub font_size: f64,
    pub bold: bool,
    pub stroke: f64,
}
```

on `Annotation` as `#[serde(default)] pub style: MarkStyle`.

`serde(default)` is what makes every mark already on disk keep the look it has.
That is not a nicety: marks persist per file, and a release that silently
restyled everything anyone had drawn would be a bug with no error message.

**Per-mark, not global-only.** A mark has to carry its own style because it
persists, it travels over MCP, and it ends up in an exported figure that must
look the same when reopened. A global default is a *second* thing (below), not
the storage.

### Units: device pixels, not data units

`stroke` and `font_size` stay in device pixels, unscaled by zoom — the existing
comment on `STROKE` gives the reason and it is still right: a stroke that
thickens as you zoom out turns the view into a blot.

Exports still scale correctly, because cairo's line width and font size are in
user space and the capture scales the context. Measured on the current code: a
1 px stroke exports at 1/2/4/8 px for 1×/2×/4×/8×.

## The one behaviour change, stated

Today `AGENT_INK` is applied at DRAW time: an agent's mark is green because it
is re-derived on every frame. With a per-mark colour, the author's ink becomes
the colour a mark is CREATED with, and after that it is the mark's own.

That is the better behaviour — you can recolour an agent's mark, which you
cannot do now — but it is a change: recolouring an agent's mark to match your
own then makes it indistinguishable in the picture. The panel still shows the
author, which is where that question belongs.

Selection and editing ink stay as they are: computed at draw time, overriding
the mark's colour **on screen only**. They are already excluded from captures
and exports, and a custom colour must not resurrect them there.

## The defaults

A second, smaller thing: what a NEW mark gets.

Stored in `settings_service` beside the other display preferences, read when a
mark is created, and copied into the mark. Not consulted at draw time — a
setting that restyles existing marks when changed is the same silent-rewrite
problem as above.

## The UI

In `MarksSection`, which both viewers already mount, so the cube gets this for
nothing. That is the DRY payoff and it is worth not spending: no per-viewer
controls, no second panel.

A **Style** row inside the Marks section: colour button, size spin, bold
toggle, thickness spin. Four controls, and they act on:

- the **selected mark**, if one is selected — immediate, visible, and how every
  drawing application behaves;
- otherwise the **defaults** for the next mark.

One control set for both jobs, rather than a separate "preferences for new
marks" screen that nobody would find.

## MCP

`annotate_fits` / `annotate_cube` and `update_annotation` gain the same four,
optional; `list_*_annotations` reports them. Colour as a `#rrggbb` string — an
agent writes hex, not a float triple, and it round-trips through JSON without
precision arguments.

`update_annotation` mattering here is the point: an agent asked to "make the
NGC 5194 mark red and thicker" can, without deleting and redrawing, which would
change the id it has already quoted.

## Order of work

1. **`MarkStyle` + `serde(default)`**, and `style::` constants expressed as
   `MarkStyle::default()` so there is one set of numbers rather than two.
2. **The renderer takes it**, and `ink_for` becomes "state overrides, else the
   mark's own colour".
3. **`label_bounds` and the width estimate** use the mark's size. Without this,
   step 2 quietly breaks clicking a label.
4. **MCP**, on all four tools.
5. **The Style row** in `MarksSection`.
6. **Defaults in settings.**

Steps 1–4 are shippable without any UI: an agent could style marks before a
person could.

## What this does not cover

- **A font family.** One face, monospace, chosen so numbers and coordinates
  line up. A per-mark family is a way to make a figure look untidy.
- **Per-mark leader angle.** Deliberately fixed: the existing comment says
  varying angles are what make an annotated figure look untidy, and that is
  still true.
- **Dash patterns.** The selection rectangle uses one; marks are solid. Worth
  wanting, not worth widening this.
- **Styling by kind or by author as a rule** — "all agent marks green". That is
  a defaults question, and the defaults above already answer the useful half.

## How it gets verified

- **Old marks are unchanged.** A stored annotation with no `style` key loads
  with today's numbers — the round trip through `annotation_store`, tested
  against a JSON fixture rather than a struct, because the fixture is what is
  actually on disk.
- **A restyled label is still clickable.** `label_bounds` follows the mark's
  font size; a 22 px label has a hit box about twice the height of an 11 px one.
- **Colour and thickness reach the export**, and `probe`-measured: a mark drawn
  at stroke 3 lays down more ink than one at stroke 1, in the exported raster.
- **Selection ink still does not.** The existing capture probe already requires
  a selected mark to export identically to an unselected one; a custom colour
  must not change that.
- **Both viewers**, since the renderer is shared: the cube probes cover the
  volume and the slice without new work.

---

## What was built (steps 5 and 6)

### The two things the plan did not say

**One value, not two.** The plan had the picker and the style as separate
questions. They are asked at the same moment, for the same reason — either can
change while drawing is armed — so they became `PendingMark { kind, style }`,
and `set_preview_kind_source` became `set_pending_mark_source`. The preview and
the mark it lands as are now made from ONE read of the controls, so they cannot
disagree about either half. That removed a field and a setter per surface
rather than adding one.

**The inks are 8-bit now.** `USER_INK` was `(0.62, 0.85, 1.0)`, which does not
survive its own storage: a colour is written as `#rrggbb` in the settings file,
in a colour button and over MCP, so the constant came back as
`(0.6196…, 0.8509…, 1.0)` and a fresh install's default was not equal to
itself. They are written `rgb8(158, 217, 255)` now — the same colour on screen,
to the byte, and the round trip is exact.

### Where things live

| | |
| --- | --- |
| `AppConfig.mark_{colour,font_size,bold,stroke}` | What a NEW mark looks like |
| `MarkStyle::from_settings` / `store_in` | The one bridge between the two |
| `settings_service::{default_mark_style, remember_mark_style}` | Read and write it |
| `MarksSection::{pending, style, show_style_for, set_on_style_changed}` | The row |

The section owns the CONTROLS and knows nothing about tabs or canvases; each
viewer decides the POLICY, which is the one thing that genuinely differs:
a control move restyles the selected mark if there is one, and otherwise
becomes the default for the next.

An agent's marks are excluded from the default deliberately: the setting means
"what I draw", and green is how a mark is known to be an agent's before anyone
opens the list.

### The invariant, and the test that holds it

Nothing reads the stored default at draw time. A draw path that consulted it
would restyle marks people had already drawn — and already exported — the
moment the colour button moved, silently.

`nothing_but_the_style_row_reads_the_stored_default` walks the tree and fails
on any caller of `default_mark_style` outside the one place a new mark is made
from it. Mutation-tested: adding a call inside the FITS canvas's draw function
fails it, and deleting the cube's `set_on_style_changed` fails
`both_viewers_connect_the_style_row`.

### Measured, not assumed

- `a_thicker_outline_lays_down_more_ink` — stroke 4 covers more pixels than
  stroke 1 in a real raster. With the renderer mutated back to the constant:
  "stroke 4 covered 419 pixels and stroke 1 covered 419".
- `a_marks_colour_reaches_the_raster` — a red mark comes out red in BGRA.
- `selection_still_overrides_a_custom_colour` — a red mark still goes white
  when picked out, so selection does not become invisible on exactly the marks
  someone cared enough to restyle.
- `a_bad_settings_file_still_draws_a_visible_mark` — the file is JSON people
  edit; an unparseable colour or a zero stroke cannot produce a mark that is
  drawn and invisible.
- `annotation_style_probe` grew a styled row, because whether four numbers a
  person picked still read as one set of annotations is not a thing a test can
  answer.
