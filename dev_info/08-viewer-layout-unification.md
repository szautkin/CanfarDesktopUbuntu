# Unifying the FITS and Cube viewer layouts

_Researched 2026-08-12 · Verbinal 1.3.3 · Reference: CanfarDesktop 1.3.3 (`36ac1d8`)_

> **The short version.** The two viewers solve the same problem — an image, a set of display
> controls, some metadata — with two different layouts. The cube's is better, and it is better for a
> reason that generalises. Unifying is mostly re-parenting, because every handler in both viewers
> binds to a widget *variable*, not to where that widget sits.

## 1. What each viewer looks like today

| | **Cube viewer** | **FITS viewer** |
|---|---|---|
| Shape | `Paned` — image left, **docked control column** right (280 px, scrollable) | Vertical stack — **everything in a top toolbar**, panels as revealers |
| Controls | Grouped under `DISPLAY` / `VOLUME` headers, each labelled above its widget | 12 controls in one horizontal bar, two of them popovers |
| Metadata | `Info` expander in the column (name, dimensions, WCS, beam, …) | `Image Info` + searchable header, in a revealer beside the image |
| Extra panels | Transfer-function editor, live colorbar, channel scrubber | Saved-coordinates panel (bookmarks, go-to) in a second revealer |
| Tabs | `adw::TabView` + `adw::TabBar` | `gtk::Notebook` |
| Mode | `3D` / `Slice` linked toggle, top-centre | n/a |

Sizes: `fits_viewer.rs` 2,266 lines, `cube_viewer.rs` 1,538, plus `fits_tab.rs` 343,
`cube_slice_view.rs` 1,142, and the two FITS panels at 228 + 329.

## 2. Why the cube's layout is the better one

Not taste — three concrete properties:

1. **A column has room to label things.** Every cube control sits under a caption (`Colormap`,
   `Window low`, `Density`). The FITS toolbar has to communicate through icons and tooltips, which is
   exactly how eleven of its controls ended up invisible inside a "Display options" popover until
   this week.
2. **A column groups.** `DISPLAY` and `VOLUME` tell you what a control affects before you touch it.
   A horizontal bar can only separate with `|`.
3. **A column scales.** Adding a control to the cube costs one row. Adding one to the FITS toolbar
   costs horizontal space that does not exist on a laptop — which is the pressure that pushed
   controls into the popover in the first place.

## 3. The finding that decides it

**Our cube viewer already diverges from the reference, deliberately, and it is the layout you
prefer.**

- The reference's cube (`Views/CubeViewer/CubeViewerPage.xaml`) is a **floating dark HUD**: a
  control panel in a translucent `Border` pinned top-right *over* the render, an info panel
  bottom-left, a mode toggle floating top-centre.
- Ours is a **docked, resizable, scrollable column** in a `Paned` — a GNOME-idiomatic layout that
  keeps the controls out of the picture.

So the question is not "may we depart from the reference here?" — we already did, in the viewer you
like. The question is why we did it in only one of the two. **Parity of capability is the contract
that matters** (every affordance present, every tool reachable); parity of chrome across two
different toolkits was never achievable and has already been traded away once, to good effect.

The reference's FITS viewer is toolbar-based (`Views/FitsViewer/FitsTabHost.xaml`), so this plan
knowingly moves our FITS viewer's *chrome* away from it while keeping every affordance. That is the
one decision in this document that is yours rather than mine.

## 4. What can genuinely be shared

Re-parenting is cheap here for a specific reason worth stating: **every handler in both viewers binds
to the widget variable**, e.g. `viewer.header_btn.connect_toggled(…)`. Nothing depends on the
container. Moving a control from a toolbar into a column changes its `append` site and nothing else —
the MCP read-back (`sync_toolbar_to_tab`, `view_json`) and every existing test stay valid.

| Component | Today | After |
|---|---|---|
| `section_header()`, `labeled()` | private to `cube_viewer.rs` | `ui::viewer_shell`, used by both |
| Paned + scrollable column scaffold | inline in `cube_viewer.rs` | `ui::viewer_shell::ControlColumn` |
| Live colorbar (gradient + physical endpoints) | cube only, 55 lines | shared — the FITS viewer has a colormap and cut levels, so it applies unchanged |
| Coordinate chip | **already shared** (`ui::coord_chip`) | — |
| Info section | cube grid; FITS revealer | one `Info` section idiom, each filling its own rows |
| Tab strip | `TabView` vs `Notebook` | `adw::TabBar` in both (phase 4) |

Not shared, and correctly so: the cube's mode toggle, volume controls, transfer editor and channel
scrubber; the FITS viewer's extension selector, crosshair/bookmarks, north-up and zoom presets.

## 5. One improvement over copying the cube exactly

Use **`adw::OverlaySplitView`** rather than a bare `Paned` for the shared shell. It docks the column
on a wide window and overlays it on a narrow one, from a single toggle — which answers the objection
that a 280 px column squeezes the image on a laptop. libadwaita 0.7 is already a dependency and the
widget is present; the app uses `adw::ToolbarView` in five dialogs already, so the idiom is familiar
here.

## 6. Status

_Updated 2026-08-12, after the work._

| Phase | State |
|---|---|
| 1 — Extract the scaffold | **Done.** `ui::viewer_shell`; the cube switched in the same commit and looks unchanged. |
| 2 — The FITS viewer gets a column | **Done.** `DISPLAY` / `VIEW` / `CROSSHAIR` / `COMPARE`; the toolbar keeps Open and the status line. `sync_toolbar_to_tab` renamed. |
| 3 — Panels become sections | **Done.** Both panels are expanders in the column; one header panel for the viewer, refilled per tab; the expander is the MCP state. |
| 4 — One tab strip | **Open, deliberately.** The only phase that touches working tab logic rather than layout. |
| 5 — One guard for both | **Done.** Both viewers build from the shell, neither keeps a private copy of the helpers, and neither may open a popover from a control with no visible word. |

Found and fixed along the way, outside the plan: **Search here** was reachable only from inside the
saved-coordinates panel, which is closed by default — an action the reference offers from its
crosshair menu did not appear to exist. It is in the `CROSSHAIR` section now, running the panel's own
action rather than a copy of it.

Still worth doing when phase 4 lands: `adw::OverlaySplitView` instead of the plain `Paned`, so the
column overlays rather than squeezes on a narrow window.

## 7. Plan

Five phases, each shippable on its own, each leaving the app working.

### Phase 1 — Extract the scaffold (S)
Move `section_header`, `labeled`, the column/Paned assembly and the colorbar out of `cube_viewer.rs`
into a new `ui::viewer_shell`. **The cube switches to the extracted helpers in the same commit**, so
the extraction is proved by the viewer that already used them: if the cube still looks and behaves
the same, the scaffold is faithful. No visual change anywhere.

### Phase 2 — The FITS viewer gets a column (M)
Re-parent from the toolbar into `DISPLAY` / `IMAGE` / `COMPARE` sections: colormap, colorbar,
stretch, min/max cut, reset, north up, blink + target + fade, linked crosshair, synced zoom.

The toolbar keeps what is genuinely file- or view-scoped: **Open**, **zoom**, and the panel toggles.
The extension selector stays its own bar (it is per-tab, like the tab strip).

The toolbar-visibility guard written this week **will fail on this commit, by design** — it asserts
placement on the toolbar. It gets rewritten in the same commit to the rule that actually matters:
*every affordance the reference shows is visible without opening an unlabelled control*, wherever it
lives. `sync_toolbar_to_tab` is renamed `sync_controls_to_tab` at the same time, since the name would
otherwise describe a thing that no longer exists.

### Phase 3 — Panels become sections (M)
Fold the header/Image-Info panel and the saved-coordinates panel into the column as collapsible
sections, matching the cube's `Info` expander. Two revealers and their bespoke show/hide logic go
away; the column scrolls, which is what makes this possible. The MCP arguments `showHeaderPanel` and
`showBookmarksPanel` keep working — they drive the section expanders instead of the revealers, and
the read-back test that pairs setter with getter keeps them honest.

### Phase 4 — One tab strip (L, optional)
`gtk::Notebook` → `adw::TabView` + `adw::TabBar`, as the cube already uses. The FITS viewer gains
close buttons, reordering and the tab overview for free, and the two viewers stop looking like
different applications. Larger because tab lookup, the blink target list and `publish_fits_tabs` all
index pages; worth doing separately and last.

### Phase 5 — One guard for both (S)
A single test asserting that each viewer routes its controls through `viewer_shell` and that neither
hides an affordance behind an unlabelled popover — replacing the FITS-only guard with one that covers
whichever viewer grows a control next.

## 8. Risks, and what makes them small

| Risk | Why it is contained |
|---|---|
| `fits_viewer.rs` is the biggest UI file | The change is `append` sites only; handlers bind to variables. The 25 `connect_*` blocks are untouched. |
| MCP surface regressions | `get_fits_view` / `set_fits_view` read and write the same widget objects. The read-back test (everything settable is readable) already covers the pair. |
| A 280 px column on a small window | `OverlaySplitView` overlays instead of squeezing; the toggle is one button. |
| Blink's transient controls (stop, interval) | They move as a group into `COMPARE`; the stop path is already state-driven, not layout-driven. |
| Losing reference parity of chrome | Explicit, recorded here, and the same trade already made in the cube. Capability parity is unaffected — no affordance is removed, only relocated. |

## 9. Recommendation

Do **phases 1–3**. They deliver the whole of what you asked for: the FITS viewer gets the cube's
shape, the two stop diverging, and roughly 200 lines of duplicated scaffolding collapse into one
module.

Phase 4 is a real improvement but is the only phase that touches working tab logic; it should be its
own change, after 1–3 have been used for a while. Phase 5 is small and follows whenever 2 lands.
