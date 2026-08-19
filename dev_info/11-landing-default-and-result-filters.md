# Landing by default, and filters that behave like CADC's

_Planned 2026-08-19 · researched against the CADC Advanced Search page and both reference apps_

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

**Plan (S).** Two separate things, both wanted:

* **A new query clears the column filters.** They describe rows that no longer exist. This is the
  bug; the button below is the feature.
* **A visible "Clear filters" button**, beside the existing *Apply filters to ADQL*, shown on the
  same condition (any filter set). CADC's own form carries a **Reset** next to its Search for the
  same reason. `set_search_results_view` already accepts `clearAll`, so the agent-facing half
  exists — this gives the person at the screen the same control.

## 3. Operators in the column filters

**Researched from the source CADC actually runs** — `cadc.votv.js`, the VOTable viewer behind the
Advanced Search results table. Per column it accepts:

| Input | Meaning |
|---|---|
| `a..b` | between a and b, inclusive |
| `> v` `>= v` `< v` `<= v` | comparison |
| `= v` | exact match, case-insensitive |
| anything else | contains, case-insensitive |

with three behaviours worth copying exactly:

* **Numeric when both sides are numbers, lexical otherwise** — so `> 2020` works on a date column
  and `> m` works on a target name, each doing the sensible thing.
* **A numeric filter excludes empty and `NaN` cells.** Asking for `> 5` should not keep rows that
  have no value at all.
* **An operator with no value filters nothing**, so a half-typed `>` does not blank the table.

**We already match the last row of that table:** VOTV escapes the filter before building its
regex, so bare text is a case-insensitive substring — exactly what `filter_rows` does today. The
operators are the gap.

**On "logical operands":** CADC has no AND / OR / NOT *within* a filter box. Combination across
columns is AND — every column's filter must pass. I would match that rather than invent a boolean
syntax, and say so in the tooltip. If you want OR within one column (`a, b` meaning either), that
is a deliberate step beyond the reference and worth deciding on its own; it is cheap to add to the
parser once the operators are there.

**Plan (M).**

1. `result_filter::matches(cell, filter)` implementing the table above, unit-tested against the
   VOTV rules including the empty/NaN and bare-operator cases. `filter_rows` calls it; the
   all-columns-AND behaviour is unchanged.
2. Reuse `helpers::range_parser` where it already parses `..`, `>`, `>=`, `<`, `<=` for the search
   *form* — so the app has one syntax rather than a form dialect and a table dialect.
3. Each filter entry gets a tooltip naming the syntax, and a placeholder that stays short:
   `Filter…` with the tooltip carrying the detail.
4. The MCP `setFilters` argument documents the same vocabulary, since an agent typing `> 2020`
   should get what a person typing it gets.

## 4. Order

1 and 2 are small and independent — landing default, clear-on-new-search, Clear button. 3 is the
substantial one and lands behind them.

## 5. Risks

| Risk | Why it is contained |
|---|---|
| Changing filter semantics surprises someone mid-session | Bare text is unchanged; only inputs that start with an operator behave differently, and those do nothing useful today. |
| `>` on a text column | VOTV compares case-insensitively as strings, and so will we — tested. |
| Clearing filters on a new search loses a deliberate narrowing | The narrowing was against rows that no longer exist. The Clear button makes the state visible either way. |
