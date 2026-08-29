# 21 — Opening a FITS: fit the image, and offer the last ones

Status: plan. Measured against the tree after the annotation work; nothing here
is implemented.

Two asks, both about the moment a file opens or is about to:

1. An image should open showing **all of it** — 100%, or less when it does not
   fit — so the frame and any marks on it are visible at once.
2. The empty view should offer a **scrollable list of recently viewed files**.

They are unrelated in code and related in intent: the first thing you see should
be useful.

## What is there already

| Need | What exists |
| --- | --- |
| Recents storage | `services::recent_cubes_service` — JSON under the data dir, `path` + `name` + `opened_at`, newest first. Written for the cube viewer. |
| Recents UI | `cube_tab_host` has a `recents_section` + `recents_list` in its empty state, and rows that open a cube. |
| Empty state | The FITS viewer already switches `content_stack` to "empty" when the last tab closes. |
| Zoom mechanics | `set_zoom` holds the view centre; `viewport_size()` falls back to the requested size before allocation. |
| A list component | `ui::item_list_section` — filter, fixed height, per-row actions, selection. |

So the FITS viewer needs neither a storage format nor a list widget invented for
it. What it needs is to use both, and to decide one number.

## Part 1 — fit on open

### The number

`scale = min(1.0, viewport / image)`, on the tighter axis.

**Never above 1.0.** A 64×64 thumbnail blown up to fill a 1600px viewport is a
wall of fat pixels, and the user asked for "100% or less". Small images open at
100% with space around them, which is what they look like.

### The hard part is WHEN, not what

The viewport is not known when the file loads. A `DrawingArea` reports 0×0 until
it is allocated, and `viewport_size()` falls back to the REQUESTED size — a
number that has nothing to do with the window the user has. Fitting at load time
would fit to a guess.

So: fit on the first allocation after a load, once.

- `connect_map` or a one-shot `connect_resize` on the drawing area, guarded by a
  "needs fit" flag the loader sets.
- The flag is cleared the first time it fires, so **resizing the window later
  never re-fits**. That matters: a viewer that re-fits on resize throws away the
  zoom the user chose every time they drag the window edge.

### What it must not disturb

- **A user's zoom.** Only a fresh load sets the flag.
- **Sync zoom.** `sync_zoom_enabled` matches an angular scale across tabs; when
  it is on, that is the user asking for something more specific than "fit", and
  it wins.
- **An HDU switch.** `switch_hdu` rebuilds the tab. Extensions of one file are
  the same size, so re-fitting is invisible — except for the 3-D `CON` HDU,
  which is not. Fit on HDU switch too, and it will look like nothing happened
  in the common case.

### Verification

- The maths is a pure function — `fit_scale(image, viewport)` — and gets unit
  tests: wide image in a tall viewport and the reverse, an image smaller than
  the viewport staying at 1.0, a zero viewport not dividing by zero.
- The capture probe gains a case: after a load into a known viewport, the whole
  image is inside the canvas.
- By eye once, on the 11471×4593 NIRCam frame, which is where the current
  behaviour is worst: it opens showing about 5% of the frame's width.

## Part 2 — recents in the empty view

### Storage: generalise, do not copy

`recent_cubes_service` is the right shape and the wrong name. Two options:

| | |
| --- | --- |
| **Copy it as `recent_fits_service`** | Fastest, and leaves two files that will drift — the cube one already has the "cap the list at N" and "move an existing entry to the top" rules that the FITS one would have to re-derive. |
| **Generalise to `recent_files_service`, keyed by kind** | One implementation, two files on disk (`recent_cubes.json`, `recent_fits.json`) so neither viewer's list is polluted by the other's. |

Take the second. The cube's file name and behaviour are preserved, so nothing a
user has is lost, and `list_recent_cubes` keeps working unchanged.

### The list

`ui::item_list_section` with `RowActions::DELETE` — a recents list wants
"forget this one" and nothing else — a filter, and selection off: clicking a
recent opens it, which is an action, not a choice you can un-make.

That is a third caller for the component and the first outside a sidebar. If it
does not fit an empty-state panel, that is worth knowing now rather than after a
fourth section is written against it.

Rows: **name** as title, **path** as subtitle, and the path is what the row
carries as its id — a filename alone is ambiguous across directories, and two
`i2d.fits` from different observations are exactly what an astronomer has.

### What it must handle

- **A file that has moved or been deleted.** Opening it fails; the row should
  say so and offer to forget it rather than failing silently each time.
- **An empty list**, on a first run: the section's own empty message.
- **Long paths.** The subtitle wraps today; a middle-ellipsised path reads
  better for `/home/…/observations/obs-532a…/jw01783-o003_t009_nircam….fits`.

## Order of work

1. **`fit_scale`** — pure, unit-tested.
2. **Fit on first allocation**, with the flag. Verified with the probe and by
   eye on the NIRCam frame.
3. **Generalise the recents service**, cube behaviour unchanged, tests for the
   cap and the move-to-top.
4. **Record a FITS open** — one call at the point a tab is created, which is the
   one place a file becomes "viewed".
5. **The empty-state list**, on `item_list_section`.
6. **The missing-file case**, which is the part that will actually happen.

## What this does not cover

- **Remembering per-file view state** — reopening a file will fit it again
  rather than restoring where you were. Worth wanting; a different feature.
- **Recents across viewers.** A cube and a FITS stay in separate lists; a single
  "recently opened" across the app is a product decision, not a refactor.
- **Thumbnails** in the recents list. They would need a render per entry at
  startup, and the marks work has just shown what a 52-megapixel conversion
  costs.
