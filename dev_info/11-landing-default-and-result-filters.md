# Landing by default, and filters that behave like CADC's

_Planned 2026-08-19 · researched against the CADC Advanced Search page and both reference apps_
_Section 3 revised after reading the CADC sources directly — the first draft was wrong about
operators, see "Correction" below._

## 1. Home should be the default screen

**Today:** signing in calls `navigate_to_dashboard`, which navigates to the Portal. So the tiles
appear only until you authenticate, and never again unless you click Home.

**The reference:** `ApplyMode(AppMode.Landing)` is reached from exactly one place — `GoHome()` —
and login does not call it. The Portal is reached deliberately, through its tile or
`NavigateByKey("portal")`. The macOS app has `.landing` as its own mode for the same reason.

**Plan (S).** Signing in *builds* the Portal — so it is ready and its data is loading — but does
not navigate to it. The user stays where they are, which after start-up is Home.

A guard: signing in must not change the visible page.

## 2. A new search inherits the last one's filters

**Today:** `run_query` replaces `results_store` and resets the page, and never touches
`column_filters`. The next result set is silently narrowed by filters typed against the previous
one. `render_results_page` even refills the boxes from that map, so the state is visible — but
only if you look up at a row of filter fields you have stopped noticing.

**This is a parity gap, not a judgement call.** `SearchViewModel.ExecuteAdqlAsync` is the single
place Windows assigns `Results`, and the very next line is `ResetFiltersAndSort()`. We ported the
method and dropped the call.

**Plan (S).** Two separate things, both wanted:

* **A new query clears the column filters and the sort.** They describe rows that no longer exist.
  This is the bug; the button below is the feature.
* **A visible "Clear filters" button**, beside the existing *Apply filters to ADQL*, shown on the
  same condition (any filter set). CADC's own form carries a **Reset** (`.reset-query-form`) next
  to its Search for the same reason. `set_search_results_view` already accepts `clearAll`, so the
  agent-facing half exists — this gives the person at the screen the same control.

## 3. Operators in the column filters

### Correction

The first draft of this plan said *"CADC has no AND / OR / NOT within a filter box."* **That was
wrong.** CADC has a negation operator, `!`, and it is documented in the tooltip CADC puts on every
filter input. I had read `valueFilters()` — which parses the comparison operators — and stopped
there. The negation is one level up, in `searchFilter()`, which strips a leading `!` *before*
calling `valueFilters` and inverts the result. Reading one function and generalising to "CADC does
not have this" is the same mistake as a guard that passes because it tests the wrong property.

### What CADC actually accepts, per filter box

From `cadc.votv.js` — `searchFilter()` (line 1539) and `valueFilters()` (line 681), the code the
Advanced Search results grid actually runs:

| Input | Meaning |
|---|---|
| `!…` | **negate** — prefix to any of the rows below, stripped first and the verdict inverted |
| `a..b` | between a and b, inclusive |
| `> v` `>= v` `< v` `<= v` | comparison |
| `= v` | exact match, case-insensitive |
| anything else | contains, case-insensitive |

Because `!` is stripped before the rest is parsed, it composes with every row: `!>5`, `!2..8`,
`!=HST`, `!raw` are all valid and all mean what you would expect.

Across columns the combination is **AND** — `searchFilter` returns `false` on the first column
whose filter rejects the row.

Three behaviours worth copying exactly:

* **Numeric when both sides are numbers, lexical otherwise** — so `> 2020` works on a date column
  and `> m` works on a target name, each doing the sensible thing. The string comparison
  upper-cases both sides, so it is case-insensitive.
* **A numeric filter excludes empty and `NaN` cells.** Asking for `> 5` should not keep rows that
  have no value at all.
* **An operator with no value filters nothing**, so a half-typed `>` does not blank the table.

**We already match the last row of that table:** VOTV escapes the filter before building its
regex, so bare text is a case-insensitive substring — exactly what `filter_rows` does today. The
operators and the negation are the gap.

### The tooltip

We do not have to invent the wording. CADC sets `title` on each filter input, chosen by column
type (`cadc.votv.js:932`):

> **Number:** `Number: 10 or >=10 or 10..20 for a range , ! to negate`
> **String:** `String: Substring match , ! to negate matches`

and on the "Filter:" label: `Enter values into the boxes to further filter results.`

Using CADC's own two strings means a user who knows the web form finds the same words here, and it
keeps the tooltip honest — it can only drift from the parser if we change the parser. We know the
column's datatype at header-build time, so picking between the two is free.

### Plan (M)

1. `result_filter::matches(cell, filter)` implementing the table above, unit-tested against the
   VOTV rules including negation, the empty/NaN case and the bare-operator case. `filter_rows`
   calls it; the all-columns-AND behaviour is unchanged.
2. Reuse `helpers::range_parser` where it already parses `..`, `>`, `>=`, `<`, `<=` for the search
   *form* — so the app has one syntax rather than a form dialect and a table dialect.
3. Each filter entry gets the matching CADC tooltip, keyed off the column's numeric-ness, and a
   short placeholder (`Filter…`) with the tooltip carrying the detail.
4. `filters_to_where` has to learn the same grammar or refuse it. Today it turns any filter into
   `= n` or `LIKE '%…%'`, so `!raw` would become `LIKE '%!raw%'` and silently mean the opposite of
   what the grid shows. One parser feeding both is the fix — the operator becomes `NOT LIKE`,
   `BETWEEN`, `<`, `>=`, and so on.
5. The MCP `setFilters` argument documents the same vocabulary, since an agent typing `> 2020`
   should get what a person typing it gets.

Note that this goes **beyond** the Windows reference, deliberately and at your request:
`ResultFilter.Filter` there is `Contains(…, OrdinalIgnoreCase)` and nothing else. CADC's web UI is
the richer of the two, and it is the one users compare us against.

## 4. The search *form* has a different, richer syntax

Worth recording separately, because it is a second dialect and our form tooltips are thinner than
CADC's. From `/static/js/search/json/tooltips_en.json` — the 65-entry file `app.js` loads into the
field popovers:

* **Text fields** (`Observation.observationID`, `proposal.id`, `proposal.title`, `proposal.pi`):
  case-insensitive, with **wildcards automatically appended to both ends**. An explicit `*` is
  supported mid-string — the observationID examples include `idt802*`, `acsis*20081109*`,
  `20081109-*`, `M08BU04-*`.
* **`proposal.keywords`: comma-delimited values, meaning OR** — "Multiple strings can be comma
  delimited", example `Dynamics, Emission lines`. This is the OR the first draft said did not
  exist; it lives on the form, not the results table.
* **Dates** (`Plane.dataRelease`, `Plane.time.bounds.samples`): single value, `a..b` range, or
  `<`/`>`, in JD, MJD or ISO8601, with partial ISO dates allowed (`2013`, `2013-02-28`,
  `2013-03-01..2013-09-01`, `< 2013-02-22T9:00:00`).
* **Numeric with units** (`time.exposure`, `energy.bounds.samples`, `position.sampleSize`):
  `a..b` or `<`/`>`, with a unit suffix on either end — `0.5..1m`, `800..1200nm`,
  `32000A..1300GHz`, `0.02..0.05arcmin`.
* **Position** (`Plane.position.bounds`): name or coordinates, optional radius with unit, optional
  frame (`ICRS`/`J2000`/`B1950`/`FK4`/`FK5`/`GAL`), and `..` ranges on RA/Dec.

We implement much of this already in `range_parser` and the resolver. The gap is that CADC *tells*
the user, in a popover per field, and we mostly do not. Folding these strings into our field
tooltips is a separate, cheap task — filed here rather than done inline, since it touches every
form field.

## 5. Order

1 and 2 are small and independent — landing default, clear-on-new-search, Clear button. 3 is the
substantial one and lands behind them. 4 is a follow-up.

## 6. Risks

| Risk | Why it is contained |
|---|---|
| Changing filter semantics surprises someone mid-session | Bare text is unchanged; only inputs that start with an operator or `!` behave differently, and those do nothing useful today. |
| A literal `!` or `>` someone wants to search for | Same trade CADC makes. The tooltip says so, and no CAOM2 column we show contains either character in normal data. |
| `>` on a text column | VOTV compares case-insensitively as strings, and so will we — tested. |
| Grid and ADQL disagreeing about a filter | Item 4 of the plan exists precisely to stop that: one parser, both consumers. |
| Clearing filters on a new search loses a deliberate narrowing | The narrowing was against rows that no longer exist, and Windows already clears. The Clear button makes the state visible either way. |
