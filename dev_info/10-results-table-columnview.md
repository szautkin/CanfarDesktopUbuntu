# The results table, done properly

_Planned 2026-08-19 · supersedes stage 1 of `09-ui-consistency-and-results-table.md`_

> **The short version.** Stage 1 stopped the drift and made the table unreadable: everything
> is "…". Two causes — one a bug I introduced, one a design that was always going to end here.
> `gtk::ColumnView` removes both, because a column owns its header and its cells instead of two
> sides being kept in step by hand.

## 1. What the screenshot shows, and why

**Every header is "…" and so are the narrowable cells.** That is my bug. `cell_label` clamps a
label's natural width to one character so the *cell* decides the width — correct for a label
that IS the cell, wrong for a label INSIDE a pinned button. The button hands its child the
child's natural width, which is now one character. Plain-label columns (RA, Dec, Start Date)
render fine in the screenshot; every button-backed column — the header buttons, and the
narrowable cells (collection, Target Name, Instrument) — collapsed. One property, two lines.

**Even repaired, the headers would not fit.** `column_width` hands out 60–110 px, and
"Dec. (J2000.0)" needs about 120. The old table hid this by letting each side take the width it
wanted — which is exactly what made the columns drift. Fixed widths chosen in advance cannot
be right for 41 columns of unknown content; the width has to come from the content and from the
user, not from a match arm.

**A guard passed through all of it.** The one I wrote asserts the two sides pin to the same
number. They do. A table where every cell is "…" satisfies it perfectly. I measured alignment
and shipped illegibility.

## 2. Why ColumnView is the answer

One `ColumnViewColumn` owns its header *and* its cell factory, so alignment stops being
something two code paths must agree about — it is not expressible for them to disagree.
Confirmed present in gtk4 0.9 with `v4_12`:

| We need | The API |
|---|---|
| Full headers, sized to their text | `set_title` — GTK measures the header |
| The user can widen a column | `set_resizable(true)` |
| A starting width | `set_fixed_width` (+ `connect_fixed_width_notify` to follow it) |
| Sorting | `set_sorter` + `ColumnView::sorter` driving a `SortListModel` |
| The Columns dialog | `set_visible` per column — no re-render |
| Only visible cells built | `SignalListItemFactory` |

Today the page builds 41 × 100 widgets on every render, sort, filter keystroke and page turn.
A factory builds what is on screen.

And the model needs no GObject subclass: `glib::BoxedAnyObject` carries a `SearchResultRow`
into a `gio::ListStore` directly. That was the main thing that made this look expensive; it is
not there.

## 3. Plan

### Step 0 — Stop shipping "…" (S, today)
Repair the child-stretch bug so the interim table is readable: a label inside a pinned button
fills it. Not the fix, just not leaving it worse than before the sweep.

### Step 1 — The model (M)
`SearchResults.rows` → `gio::ListStore` of `BoxedAnyObject`. Sorting and filtering move to
`SortListModel` / `FilterListModel`, which is where ~120 lines of hand-rolled sort/filter go
away. Pagination stays for now: `get_search_results` and `set_search_results_view` report and
set it, and that contract is not worth breaking in the same change. (With virtualisation it
becomes optional — worth retiring later, on purpose, not as a side effect.)

### Step 2 — Columns and cells (M)
One column per CSV header: title from `display_name`, initial width from `column_width`,
resizable, sorter, visibility from the existing `column_is_visible`. A factory per column
builds a `Label`, or a `Button` for the narrowable ones, and binds it on demand.

### Step 3 — The actions column (S)
The three buttons become one final column titled **Actions**, built by a factory. The headings
stop being something to remember to append.

### Step 4 — The filter row (M) — **a decision for you**

**(a) Keep the row, bind it to the columns.** One entry per column above the view, each
following its column's `fixed-width`. Familiar; the entries stay visible and fast to reach.
Costs a binding per column and re-derives on resize.

**(b) Move filters into the header menu.** `set_header_menu` puts sort ascending / descending /
*Filter…* behind a click on the heading. Removes the filter row, its alignment problem, and a
whole row of screen height; the GNOME-idiomatic shape. Costs discoverability — a filter is one
click away rather than always on screen.

I lean to **(a)** because per-column filters are used constantly here and hiding them behind a
menu trades a lot of speed for tidiness. Worth your call, since it changes how the page feels.

### Step 5 — Guards that check legibility, not just geometry (S)
The pinning guards go. In their place, the properties that would have caught this screenshot:
every column has a non-empty title; no cell sets `max_width_chars(1)`; the actions column
exists; and the Columns dialog drives `set_visible` rather than a rebuild.

## 4. What this costs and what it risks

~600 lines of rendering in `search_page/mod.rs` are replaced. The MCP surface
(`get_search_results`, `set_search_results_view`, `export_search_results`) reads the same
column/filter/sort/page state and must keep answering identically — its tests are the
regression net and stay untouched.

| Risk | Why it is contained |
|---|---|
| A rewrite of the busiest page | The model, columns, cells and actions land as separate commits, each with the page working. |
| MCP contract drift | The snapshot tests already assert the shape; they run unchanged throughout. |
| ColumnView is new to this codebase | One page adopts it; if it goes badly the two-panel table is one revert away. |
