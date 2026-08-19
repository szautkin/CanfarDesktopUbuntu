# UI consistency, and the results table

_Planned 2026-08-19 · Verbinal 1.3.3 · evidence: three screenshots + a source survey_

> **The short version.** The table misaligns for a reason the existing guard cannot see: both
> sides ask `column_width()` for the same number, but that number is a *minimum*, and the
> geometry is decided by each cell's *natural* width. Spacing is 522 hand-written numbers in
> 14 different values with no scale and no name. Both are the same defect in different
> clothes: a rule that lives in the call sites instead of in one place.

## 1. What the screenshots show

| Screenshot | Symptom |
|---|---|
| Search results | Header labels drift right of the values they name; the error compounds across the row. Three trailing action cells (preview, save, ⋮) have no header above them. |
| Launch Session | Every row invents its own idiom: four label-left/control-right rows with four different control widths, then a Session Name row with the label *above* the value. |
| Connect an AI agent | Back/Done clipped at the window's bottom edge; ~700 px of dead space above them. |

## 2. Why the table misaligns

The header and the rows live in **two separate scrollers**, kept in step by a shared horizontal
adjustment. Both size their cells with

```rust
set_size_request(column_width(&col.key), -1)
```

`set_size_request` sets a **minimum**. Inside a horizontally-scrolling viewport there is no
pressure to shrink anything, so every cell gets its **natural** width instead — and the two
sides have different naturals:

* a header cell is a vertical box holding a sort button **and a `gtk::Entry`** (the per-column
  filter), whose natural width is far wider than a label's;
* a data cell is a label or a flat button whose natural width is its text.

So each column is as wide as whichever side wanted more, the two sides disagree column by
column, and the offsets add up left to right. That is exactly the drift in the screenshot:
column 2 is nearly right, column 4 is 150 px out.

**The existing guard passes on all of this.** It asserts that both sides call `column_width`
with the same key — which they do. It checks the number that is asked for, not the number that
is used. A guard that only ever sees `set_size_request` cannot see a natural width.

### The headerless columns

Data rows append three action cells *after* the loop over visible columns — preview,
Save to Research, and the ⋮ details menu. The header row loops over the visible columns and
stops, so nothing is emitted for those three: no label, and no reserved width.

## 3. Why the spacing is uneven

522 spacing calls across the UI files, in 14 distinct values:

| value | uses | | value | uses |
|---|---|---|---|---|
| 12 | 293 | | 18 | 11 |
| 6 | 79 | | 10 | 11 |
| 8 | 45 | | 48 | 8 |
| 4 | 24 | | 2 / 3 / 32 / 1 / 0 | 12 |
| 24 | 20 | | 16 | 16 |

Every one is written at the point of use. There is one named spacing constant in the whole UI
(`RESULT_COLUMN_GAP`), and `style.css` carries 14 padding/margin rules against 170 lines that
are otherwise colour. Nothing says what 12 *means*, so a new widget picks a number that looks
right beside its neighbour — which is how 10, 18, 3 and 1 got in.

## 4. Plan

Four stages, each shippable, in the order the screenshots put them.

### Stage 1 — The results table lines up (S/M)

Make `column_width(key)` **authoritative** rather than advisory: every widget in a column is
clamped to it, so its natural width cannot exceed it.

* header label: `set_max_width_chars` derived from the column width, `EllipsizeMode::End`
* filter entry: `set_width_chars` derived from the same width, `set_hexpand(false)`
* data cells (label and narrowable button): already width-requested; add the same cap
* every cell: `set_hexpand(false)`

Then add the three action columns to the header — `column_width` gains `"actions.preview"`,
`"actions.save"`, `"actions.details"` — so the header and the rows end at the same place and
the labels sit over the buttons they name.

**The guard gets rewritten to the rule that actually matters:** for every column, the header
side and the row side must clamp to the same width, and no cell may be allowed to expand.
Verified by re-introducing an unclamped cell.

### Stage 2 — One spacing vocabulary (M)

A `ui::space` module naming spacings by **role**, not size:

```rust
pub const PAGE: i32 = 12;      // a page's outer margin
pub const CARD: i32 = 12;      // inside a card / boxed list
pub const SECTION: i32 = 24;   // between sections
pub const ROW: i32 = 6;        // between rows in a group
pub const CONTROL: i32 = 8;    // between a label and its control
pub const ICON: i32 = 4;       // between an icon and its text
```

The values are today's dominant ones, so most call sites change name without changing pixels —
the diff is large but the rendering is nearly unchanged, which is what makes it reviewable. The
off-scale values (1, 2, 3, 10, 18) get folded into the nearest role, which is where the visible
improvement comes from.

A guard, in the shape of the localization ones: **no integer literal in `set_margin_*`,
`set_spacing`, `set_row_spacing` or `set_column_spacing` outside `ui::space`.**

### Stage 3 — Shared row and dialog builders (M)

The Launch Session screenshot is four rows built four ways. `ui::forms` gains:

* `labeled_row(label, control)` — label left, control right, one control width for the group
* `value_row(caption, value, actions)` — the Session Name shape, used deliberately
* `dialog_shell(title, content, actions)` — content margins, action-bar padding, and
  **size-to-content**, which is what the wizard's clipped Back/Done button needs

`viewer_shell` already proved this works for the two image viewers; this is the same move for
forms and dialogs.

### Stage 4 — The wizard's clipped actions (S)

Its own bug, not a spacing one: the dialog is ~1135 px tall for ~200 px of content, and its
action bar is pushed past the bottom edge. Fix with `dialog_shell` from stage 3 plus a height
that follows the content.

## 5. What this does not do

* **No move to `gtk::ColumnView`.** It would make alignment structural rather than maintained,
  and would virtualise rows (we currently build 41 × 100 widgets per page render). But its
  header cannot easily carry a per-column filter entry, and the filters are a real feature. It
  is the right five-year answer and the wrong one this week; noted here so the next person does
  not have to rediscover it.
* **No visual redesign.** Colour, typography and iconography are unchanged. This is about
  alignment and rhythm only.

## 6. Risks

| Risk | Why it is contained |
|---|---|
| A 522-site sweep is a big diff | Values are unchanged for the dominant roles, so the rendering diff is small; the guard makes the sweep verifiable rather than trusted. |
| Clamping cells could truncate values a user needs | Every clamped cell keeps its full text in a tooltip, and the column widths are unchanged from today's. |
| The table guard could pass while the UI still misaligns — again | That is the failure being fixed. The new guard asserts the clamp, not the request, and is verified by re-introducing an unclamped cell. |
