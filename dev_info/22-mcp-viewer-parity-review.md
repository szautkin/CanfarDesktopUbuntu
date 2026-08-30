# 22 — MCP review: can an agent do everything the viewer panels can?

Every control in the FITS viewer's and the cube viewer's panels, checked
against the MCP tools, field by field. Measured against the tree and the tool
schemas, not against memory.

**Verdict: the FITS viewer was already complete. The cube had six gaps, five of
them real, and all are now closed.** The one non-gap is worth knowing about too.

The method matters more than the list: every gap below was found by walking the
UI and asking "which tool does this?", not by reading the tool list and asking
"does this look complete?" The second question has an answer for every gap that
was there.

---

## FITS viewer — complete

| Panel control | Tool | |
| --- | --- | --- |
| Open FITS | `open_fits_file` | ✅ |
| Colormap, stretch | `set_fits_view.colormap` / `.stretch` | ✅ |
| Black / white point sliders | `.minCut` / `.maxCut` | ✅ |
| Reset | `.reset` | ✅ |
| Zoom box and preset list | `.zoomPercent` | ✅ |
| Pan | `.centerX` / `.centerY` | ✅ |
| North up | `.northUp` | ✅ |
| Extension selector | `.hdu` | ✅ |
| Crosshair placement / clear | `.crosshairX`/`.crosshairY`, `.clearCrosshair` | ✅ |
| Header & image info section | `.showHeaderPanel`, `get_fits_header`, `get_fits_wcs` | ✅ |
| Saved coordinates | `list/save/delete_fits_bookmark`, `fits_goto_coordinate` | ✅ |
| Marks | `annotate_fits`, `update_annotation`, `select_annotation`, `remove_annotation`, `clear_annotations`, `list_fits_annotations` | ✅ |
| Blink, "vs…" target, fade speed | `blink_fits_tabs` (`action`, `withTabIndex`, `intervalMs`) | ✅ |
| Link crosshair, Sync zoom | `.linkedCrosshair`, `.syncZoom` | ✅ |
| Tabs | `switch_fits_tab`, `close_fits_tab` | ✅ |
| Pixel readout | `probe_fits_pixel` | ✅ |
| What the user is looking at | `get_fits_image` | ✅ |

**Two controls have no tool, on purpose.** *Copy RA/Dec* puts text on a
clipboard an agent cannot read and does not need — `get_fits_view` already
reports `crosshairRa`/`crosshairDec`. *Search here* hands the crosshair to the
Search page, which has its own tools; duplicating it here would be a second way
to do one thing.

The drawing controls (pencil, shape picker) have no tool either, and should
not: they arm a *mouse*, and an agent says what it wants with
`annotate_fits(kind, …)` directly.

---

## Cube viewer — six findings

### 1. `cubeTabs[].path` was not a path — **bug**

`publish_cube_tabs` published the display **name**; `open_tabs_payload` reports
those strings under `path`, and that is what an agent reopens a tab with. So it
got a bare filename with no directory.

The FITS side has always published `source_file()`. This is the same shape as
the annotation target that was read out of a payload with no `path` key and
silently became `""` — a field whose *name* promises more than its contents.
One-line fix, because the viewer now holds its own path.

### 2. `get_cube_view` never said which cube — **gap**

It described the camera precisely — azimuth, elevation, dolly, steps, spectral
scale, colormap, window — and never named the file. With two cubes open the
payloads were indistinguishable, and nothing in one could reopen it.

Now carries `name`, `path`, `tabIndex`, `tabCount`, matching what the FITS
payload has always had.

### 3. The 2-D slice's zoom and pan — **gap**

The slice has its own zoom, pan and reset. None appeared in `set_cube_view` or
`get_cube_view`, because the camera fields describe the *volume* and nobody had
noticed the other view has a view too.

This became more visible when the slice stopped defaulting to fit: it now opens
at a measured zoom (~0.47) matched to the volume, so "the slice's zoom" is a
real number an agent may need to read or set. `sliceZoom`, `slicePanX/Y`,
`resetSlice`.

### 4. No `close_cube_tab` — **gap**

A cube could be opened and switched to and never closed. An agent working
through a list of cubes piled up tabs it had no way to clear, each holding a
decoded volume in memory. `close_fits_tab` had existed for a while; the cube was
not revisited when it was added.

Recorded in `VERBINAL_FIRST` with the reason, as the parity rule requires.

### 5. The Info panel reached no one — **gap**

Object, telescope, instrument, unit, value range, median: all on screen, none in
any tool. An agent could describe a cube's camera angle to three decimals and
not say what it was pointed at.

Now under `metadata`, with the **native** dimensions beside the in-RAM ones — a
large cube is decimated to load, and a voxel coordinate means nothing until you
know which grid it is on.

### 6. No cube header tool — **not a gap**

`get_fits_header` and `get_fits_wcs` take a path and read the file directly,
with no viewer involved. A cube is a FITS file, so they already work on one.
This was discoverability, not capability; `open_cube` now says so rather than a
third tool being written.

---

## What the repo's own guards caught

Three fired while closing these, and each was right:

- **`advertised_names_match_the_reference`** refused `close_cube_tab` until the
  reason for going beyond the Windows app was written down.
- **`every_tool_lands_in_a_real_category`** refused it until it was filed.
- **`every_settable_control_can_be_read_back`** refused the slice controls. I
  had reported them nested under a tidy `slice: { zoom, panX, panY }` while the
  setter took `sliceZoom`. The guard's point is exact: **a control an agent sets
  by one name and reads by another is one it cannot reliably put back.** They
  are flat and symmetric now.

That last one is the argument for keeping such guards even when they feel
pedantic. It caught a real asymmetry in a payload I had just written and
believed was better.

---

## Deliberate non-goals

Stated so they are not re-reported:

- **Channel playback.** The scrubber's play button animates; a tool returns a
  still. An agent steps `channel` instead.
- **Clipboard actions.** Nothing an agent can read.
- **Drawing-mode arming.** The pencil and shape picker arm a pointer. An agent
  passes `kind` to `annotate_cube` / `annotate_fits`.
- **Edit mode (grips, the label field).** Pure UI state for a human hand. An
  agent moves, resizes and renames with `update_annotation`.
