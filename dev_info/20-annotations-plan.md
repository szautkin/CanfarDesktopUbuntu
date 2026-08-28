# 20 — Annotations: drawing on the FITS and cube viewers

Status: plan. Measured against the tree at v1.4.0 + the agent-vision work;
nothing here is implemented.

The ask: a user — and an agent — can draw over both viewers. Rectangles and
circles, text, and a fine leader line leaving a shape at an acute angle with the
text sitting on a short rule at its end. Blueprint schematics, not marker pen.
Plus MCP tools, a panel to review what has been drawn, and French.

This is the half the vision work was groundwork for. `get_fits_image` and
`get_cube_image` let an agent SEE a viewer; this lets it point.

## What already exists to build on

Measured, not assumed:

| Need | What is there |
| --- | --- |
| One drawing, screen and capture | `FitsCanvas::draw_working_area` and `CubeViewer::draw_axes_overlay`, both extracted from their closures — an annotation layer drawn there appears on screen AND in every capture, from one place |
| FITS image→screen | `FitsCanvas::image_to_screen_point(px, py)` — public, honours pan, zoom and rotation |
| Cube data→screen | `cube_axes::project(&vp, [x,y,z], w, h) -> Option<(f32,f32)>` — already culls behind the near plane |
| A place to put coordinates | Captures already return `view` + `scale`, so an agent can turn a point it sees into one it can name |
| Persistence precedent | `helpers::fits_bookmarks` — JSON under the data dir, load-or-empty, id from a timestamp |
| Panel precedent | `fits_coords_panel.rs`, `fits_header_panel.rs` — collapsible, sectioned |
| Translation | `tr_en!` + the `(en, fr)` table, with `every_localized_string_has_a_french_form` failing the build on a miss |

Almost nothing here is new machinery. The work is a model, a renderer, two
projections, and a surface.

## The design

### One renderer, two projections

The trap is a FITS annotation layer and a cube annotation layer. They would
start identical and drift — one gains dashed strokes, the other a font change —
and the difference would show up as an agent describing a picture that no longer
looks like the other viewer.

So the geometry is projected per viewer and everything else is shared:

```rust
/// Where an annotation is pinned, in the viewer's OWN coordinates.
pub enum Anchor {
    /// FITS: image pixels. Survives pan, zoom and rotation.
    ImagePixel { x: f64, y: f64 },
    /// FITS with WCS: sky. Survives reopening the file, and points at the same
    /// place in a DIFFERENT image of the same field.
    Sky { ra_deg: f64, dec_deg: f64 },
    /// Cube: voxel space.
    Data { x: f64, y: f64, z: f64 },
}

/// A viewer that can place an anchor on its own canvas.
///
/// The only thing the renderer needs from a viewer, and the only thing the two
/// implementations differ by. `None` means "not visible right now" — behind the
/// cube's near plane, or outside the FITS viewport.
pub trait AnnotationSurface {
    fn project(&self, anchor: &Anchor) -> Option<(f64, f64)>;
}
```

`helpers::annotation_render::draw(&[Annotation], &dyn AnnotationSurface, cr, w, h)`
is then one function, and adding a third viewer is one `impl`.

**Screen pixels are never stored.** An annotation pinned to the screen would
slide off its subject the moment anyone panned — which is the bug that makes
annotation features feel broken, and it is invisible until someone zooms.

### Reuse: the label primitive already exists, in one of four styles

On-canvas text is drawn in four places today and no two agree:

| Where | Style |
| --- | --- |
| `cube_viewer` axis captions | centred on the anchor via `text_extents`, **clamped to the panel**, and a 1px dark shadow under a light glyph |
| `cube_slice_view` | plain text, no shadow |
| `cube_export` | plain text — fine, it draws on a controlled plate |
| `fits_canvas` | **none at all** — the FITS canvas draws no text today |

The cube's caption is the one that works, and it works for a reason worth
keeping: annotations sit over arbitrary data, and thin light text over bright
nebulosity is unreadable. The shadow is what makes a label legible on any
background, the clamp is what stops a label leaving the canvas when its anchor
is near an edge, and both are the sort of thing a second implementation gets
wrong and nobody notices until a screenshot.

So: **extract it**, as `helpers::annotation_style::draw_label(cr, at, text,
style)` — and have the existing axis captions call it too. Then the blueprint
text style is one decision, the axes and the annotations cannot drift, and the
FITS viewer gets a label renderer without anyone writing a second one.

`cube_export` and `cube_slice_view` keep their own text: they draw on
backgrounds they control, and forcing a shadow on them would be reuse for its
own sake.

### Reuse: the popover is for INPUT, never for output

The 2D viewer has a `coord_label` readout and GTK popovers — the right things to
reuse for *entering* an annotation's text: a small popover with an entry,
anchored at the click, shared by both viewers so text is captured the same way
in each.

But the annotation's text must be **drawn with cairo, not laid out as a widget**,
and this is not a style preference. A capture replays the viewer's cairo draw
function into an off-screen surface; a GTK label floating over the canvas is not
in that function, so it would appear on screen and be **absent from every
capture**. The user would see labelled shapes and the agent would see unlabelled
ones, with nothing reporting a problem — the exact divergence "one drawing, two
destinations" exists to prevent.

Popover to collect the text; cairo to draw it. The rule is short enough to state
in the code, and it should be.

### The blueprint look

One style module, so "blueprint" is a decision made once:

- **Strokes** hairline — 1px at scale 1, and *not* scaled with zoom, or a
  zoomed-out view turns into a blot.
- **Palette** cyan-white on the viewer's dark ground; a warm second colour for
  the selected annotation only.
- **Text** monospace, small, no background fill — sitting ON the rule, the way a
  drawing labels a dimension.
- **Corners** square. Circles are true circles in SCREEN space, so they read as
  drawn rather than as a projected ellipse.

Theme-aware: the viewers have a dark ground today, but the palette lives in one
place so a light ground is a change to that place.

### The callout — the part with real geometry

The requested shape is one thing, not three: a shape, a leader line leaving it at
an **acute** angle, and a short horizontal rule at the end carrying the text.

```
        ┌──────────┐
        │          │
        └────┐─────┘
              ╲                 ← leader, fixed acute angle from the shape edge
               ╲
                ╲______________  ← rule; text sits on it
                  NGC 5194 core
```

The rules worth writing down, because each is a way it can look wrong:

- The leader leaves the shape's **edge**, not its centre, and not its corner.
- The angle is fixed (30°/45°) and the same for every callout on a canvas —
  varying angles is what makes an annotated figure look untidy.
- The rule's length is the text's width, so text never overhangs it.
- The leader flips to whichever side has room, so a callout near the right edge
  points left. This is the fiddly one and it needs a probe with the text at the
  four corners.
- Text is drawn at a fixed size in SCREEN space: it is a label, not part of the
  image.

### The volume view, which is not the flat case with a third number

Anchoring to voxels and projecting with `cube_axes::project` gets an annotation
onto the rotating volume. Four things beyond that decide whether it feels right,
and one of them the first draft of this plan skipped entirely.

**Picking a depth from a 2D click.** This is the gap. A click on the volume is a
RAY, not a point — every voxel along it projects to the same pixel, and nothing
in the viewer picks between them today (`cube_volume_gl` ray-marches on the GPU;
there is no CPU pick). Three ways to choose:

| | |
| --- | --- |
| The slice plane's channel | The plane is already drawn in the volume as the cyan quad, and the scrubber that moves it is right there. The user places on a surface they can SEE. |
| Peak intensity along the ray | Clever, and surprising: the mark lands somewhere the user did not click, at a depth they cannot predict. |
| The volume's centre | Predictable and almost never what was meant. |

Take the slice plane — `CubeViewer::current_channel()` is already public — and
say so in the UI: while the draw mode is on, the plane marker is emphasised, so
it reads as "you are drawing on this plane". A user who wants another depth
scrubs to it first, which is the same gesture they already use.

An **agent is unaffected**: `annotate_cube` takes an explicit `{x, y, z}`, so
the ambiguity is the mouse's problem alone.

**Extent is in data units; text is not.** A circle marking a region of a cube is
a sphere, and a sphere seen from off-axis is an ellipse — so shapes project and
may render as ellipses, which is correct rather than a bug to fix. Strokes and
text stay screen-space: a hairline stays a hairline and a label stays readable
at any camera distance. Two different rules in one renderer, and the reason each
one is what it is belongs in the code.

**A callout's anchor is 3D; its label is 2D.** The leader starts at the projected
anchor and the rule sits at a screen-space offset from it, or the text would
shear and shrink as the camera moved. The offset is stored with the annotation,
so a label the user dragged clear of the volume stays clear of it — until the
camera rotates far enough that it overlaps again, which is a limitation to state
rather than solve.

**Depth, honestly.** The volume is semi-transparent, so there is no true
occlusion to respect — an annotation "inside" the data has no well-defined
in-front or behind. Annotations therefore draw with the axes overlay, over
everything. Two rules keep that from being confusing:

- An anchor behind the near plane must VANISH. `project` already returns `None`
  there, and the renderer must honour it — an unchecked projection puts the mark
  at a mirrored position on the far side of the canvas, which looks like a
  placed annotation and is not.
- A depth cue earns its place: annotations further from the camera drawn
  slightly dimmer. Cheap, and it is what makes several marks in one volume
  legible as being at different depths.

**Scrubbing does not hide them.** An annotation placed at channel 40 stays
visible when the user moves to channel 12: it is a mark in a 3D space, not a
property of a channel. (The 2D slice view is the opposite case, and is why that
mode is still deferred below.)

### The model, and where it lives

`models::annotation` — shared by both viewers, the MCP tools, and the panel.

```rust
pub struct Annotation {
    pub id: String,
    pub kind: AnnotationKind,   // Rect | Circle | Callout | Text | Line
    pub anchor: Anchor,
    pub size: Option<Extent>,   // radius, or half-width/half-height, in anchor units
    pub text: String,
    pub created_by: Author,     // User | Agent — an agent's marks are labelled
    pub created_at: String,
}
```

`created_by` matters. An agent drawing on a user's screen without saying so is
the kind of thing that erodes trust in the whole feature; the panel shows which
were the agent's, and the style gives them a subtly different accent.

**Persistence**: JSON beside the bookmarks, keyed by the file (FITS) or cube
path, so annotations come back with the image. `helpers::fits_bookmarks` is the
shape to copy — load-or-empty, never fail a viewer because a file is corrupt.

### MCP tools

Named for what an agent does, and taking coordinates it can actually produce —
which is why the captures return the transform:

| Tool | Notes |
| --- | --- |
| `annotate_fits` | rect/circle/callout/text at an image pixel or sky position |
| `annotate_cube` | the same in voxel space |
| `list_annotations` | both viewers, so an agent can see what it and the user have drawn |
| `remove_annotation` | by id |
| `clear_annotations` | one viewer, with a confirm — it destroys the user's work too |

Verb class: these are **writes**. `clear_annotations` is destructive (it can
delete what the user drew, which no undo brings back across a restart) and so
never auto-applies; the rest are non-destructive.

They dispatch through the existing viewer bridge, so the family files gain arms
and no new plumbing.

### The UI

- **Toolbar**: a draw-mode toggle, then rect / circle / callout / text. Escape
  leaves the mode. Drawing is drag-to-size for shapes; a callout is click the
  subject, drag the label where it should sit.
- **Panel**: a collapsible section in the FITS side panel (and the cube's
  equivalent) listing each annotation — its text, its kind, who made it, and
  Go-to / Edit / Delete. The list is the review surface asked for, and it is
  also the only way to reach an annotation whose subject is off-screen.
- **Selection**: click one on the canvas or in the list; both highlight.

### French

Every new string goes through `tr_en!` and gains a French form in the same
commit — the build fails otherwise, which is how the last four features stayed
translated. Terms worth agreeing once: annotation, légende (callout), forme,
repère (leader), calque (layer).

## Order of work

Each step is separately verifiable; the risky geometry comes after the plumbing
so a failure is visibly a geometry failure.

1. **`models::annotation`** — types, ids, serde. Pure; unit-tested for the
   anchor round-trip and for rejecting a NaN coordinate, which would otherwise
   reach cairo and draw nothing with no error.
1b. **`helpers::annotation_style::draw_label`** — lift the cube's caption
   drawing out of its closure, with its shadow, centring and panel clamp, and
   point the existing axis captions at it. A refactor with a behavioural claim
   (the captions look identical), so it goes first and is verified by capturing
   a cube before and after.
2. **`helpers::annotation_render`** — the renderer + the `AnnotationSurface`
   trait, against a fake surface. Every geometry rule above becomes a test:
   leader leaves the edge, rule matches text width, callout flips near an edge.
3. **Persistence** — mirrors `fits_bookmarks`, including its "corrupt file is an
   empty list, not a crash" behaviour.
4. **FITS: draw + hit-test.** `draw_working_area` gains one call. Verified with
   the existing capture probe: an annotated view differs from a clean one, and
   two captures of the same annotated view are identical.
5. **Cube volume: draw, project, pick.** Through `cube_axes::project`, with the
   slice plane as the depth for a click and data-unit extents. Two new risks
   here and neither exists in the flat case: an annotation behind the camera
   drawing at a mirrored position, and a shape whose extent was treated as
   screen pixels — which looks right at one camera distance and at no other.
   The rotation invariant is the test that matters: rotate the camera and every
   mark must stay on its voxel.
6. **MCP tools**, with the category, the alias entry and the French.
7. **UI: toolbar and panel.**
8. **The captures**: nothing to do, and that is the point — annotations appear in
   `get_fits_image` and `get_cube_image` because they are drawn by the function
   those already call.

## How it gets verified

- **Geometry by eye, once, deliberately.** A probe that renders one of each kind
  at the four corners and the centre, writes a PNG, and is looked at. Callout
  flipping cannot be checked any other way.
- **The invariant that matters**: an annotation stays on its subject. Capture,
  pan, capture again, and the anchor's projected position must move with the
  image, not with the window. That is the bug this design exists to prevent, so
  it gets a test rather than a hope.
- **Cube culling**: an annotation behind the near plane must vanish, not appear
  mirrored.
- **Mutation-test each geometry rule** — remove the edge-intersection, fix the
  flip to one side, unscale the stroke. Each must fail a test.
- **An agent's mark is labelled** as an agent's, in the panel and the payload.

## What this does not cover

- **Undo.** Delete is via the panel; there is no history stack. Worth saying
  before someone assumes Ctrl+Z.
- **Freehand.** Shapes and callouts only — a blueprint look and a marker pen are
  different products.
- **Rich text.** One line, one font, one size. A label that needs a paragraph is
  a note, and notes already have a home in the research pages.
- **Annotations in the export.** `export_cube_figure` is an export, and whether a
  figure carries the annotations is a separate decision from whether the working
  area does.
- **Sharing.** Annotations are local, beside the bookmarks. Putting them in a
  research bundle is a later question.
- **Cube annotations in 2D slice mode.** The VOLUME view is covered above; the
  slice is the open question. A voxel mark is visible from every camera angle,
  but a slice shows one channel — so a mark at channel 40 seen while scrubbed to
  channel 12 is either wrong to draw, or should be drawn faintly as "nearby".
  Worth deciding before step 5 rather than discovering during it; the volume
  work does not depend on the answer.
