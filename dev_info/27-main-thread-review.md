# 27 — What blocks the main thread

Status: measured, with one fix applied. Numbers from the running app at
`4cd65bb`, on the 5120x2880 display at scale 2.

## How it was measured

GTK is single-threaded. Every widget, every draw, every click and every
`spawn_future_local` share one thread, and nothing reports when that thread is
busy: the window stops repainting and starts again later. A blocking read, a
parse, a few thousand widgets — none of them raise an error.

So `helpers::main_thread_watch` measures it directly: a timer that should fire
every 100 ms, and a note of how late it actually was. Off unless
`VERBINAL_WATCH_MAIN_THREAD` is set, because a stall detector that always runs
is one more thing on the thread it is watching.

```
VERBINAL_WATCH_MAIN_THREAD=1 target/release/verbinal
[main-thread] watching; anything over 250 ms will be reported
[main-thread] blocked for 80507 ms (worst so far 80507 ms)
```

That 80-second line is what "sometimes it hangs" was.

## The finding

**A tooltip on every facet row.** The Additional Constraints panel builds one
checkbox per value, and CADC's data train has 5,443 of them — 5,005 in the
Filter column alone. Last commit gave each row a tooltip, so its ellipsized
value could still be read in full. `set_tooltip_text` installs the tooltip
machinery per widget, and at that scale it dominates everything:

| 5,005 rows | build |
| --- | --- |
| without a per-row tooltip | **87–121 ms** |
| with one | **2,376–2,434 ms** |

A 25x penalty, on the thread that draws the window, mine, and one commit old.

The column answers for its own rows now — one `query-tooltip` handler that asks
which row is under the pointer. Asked a handful of times a session instead of
5,443 times a load, and the full value is still there on hover.

## Where the app stands now

Same watchdog, same build, one action at a time with quiet gaps between:

| | main thread blocked |
| --- | --- |
| 20 s idle | **0** |
| startup | 2 stalls, ~1.1 s each |
| navigating any page | none |
| `get_search_constraints` (builds all 5,443 rows) | none |
| `reset_search_form`, `set_search_constraints` | none |
| opening a 1433x1413 FITS file | **none** |
| opening a spectral cube | **none** |
| a search matching 10,000 rows | 890 ms |

The two heavy file paths cost nothing because they already do the right thing:
`AppServices::spawn` hands the work to the tokio runtime and awaits the result
on the main thread, so only the handover is on it.

## What is left, and what it is worth

### 1. Rendering a page of results — 890 ms, now ~350 ms

**Update.** The same per-widget tooltip that cost the facet panel 25x cost the
results grid too: every cell carried one, plus three action buttons a row — a
hundred rows by fifteen columns is around 1,800 stored tooltips rebuilt on
every column toggle, sort and page turn. The grid answers for its own cells
through one `query-tooltip` handler now, and a column toggle went from 0.90 s
to 0.35 s.

What remains below is still true of the ~350 ms.



Results are paginated at 100 rows, so this is 100 rows times about fifteen
columns of widgets, not 10,000. It is the largest remaining stall and the one a
person is most likely to meet, because it lands right after a search they were
waiting for anyway.

Worth doing something about; not urgent. The shape of the fix is the same one
the facet panel wants (below): a view that builds what is visible instead of
what exists.

### 2. Startup — two stalls of about 1.1 s

Every page is constructed at startup, whether or not it is ever opened. That is
also why the app is usable the instant you click a sidebar row, so it is a
trade rather than a defect. If it were worth changing, the change is to build a
page the first time it is shown.

### 3. Five thousand checkboxes is a UI problem, not only a speed one

At 100 ms the Filter column is no longer slow, but nobody scrolls a 5,005-item
checkbox list in a 120 px tall box. The panel needs a way to search within a
column before its longest column is usable at all.

`GtkListView` with a model and a factory is the GTK4 answer to both: it recycles
widgets and builds only what is on screen, so the cost stops depending on how
many values CADC happens to have. It is a real rewrite of the facet columns —
selection and the cascade both have to survive widget recycling — so it is a
plan, not a patch.

### 4. Small main-thread I/O, noted and left

- `sound::enabled()` reads the settings file on each agent transition, which is
  twice per agent session and a few tens of microseconds. Left alone: a cache
  would need invalidating, and the invalidation is more code than the read.
- `load_data_train` reads and deserialises a 2.8 MB cache file synchronously —
  measured at **55 ms to read and parse, 45 ms to index**. Under the 250 ms
  threshold, so the watchdog does not see it, but it is real and it is on the
  main thread. The fix is one `services.spawn`, and it is worth doing next time
  that function is opened.

## On the two things the review was asked about

**The agent idle/working indicator does not need a thread.** It polls once a
second, and the poll is a mutex around a single `Instant::elapsed` — held for
microseconds, and the only other writer is the MCP router recording a call. The
animation is a frame-clock callback that runs only while the dots are moving.
Neither showed up in any measurement.

**The search page is already off-thread where it matters.** The TAP query, the
data-train fetch and the resolver all go through `AppServices::spawn`. What is
left on the main thread is drawing, which cannot go anywhere else — GTK widgets
belong to the thread that made them. So "put the search on a thread" is not the
lever; "build fewer widgets" is.

## What holds it

- `helpers::main_thread_watch` stays, behind its environment variable. Three
  unit tests cover the arithmetic — a punctual tick, a stall, and a tick that
  arrives fractionally early, because `Duration` subtraction panics rather than
  going negative.
- The tooltip regression has no test, deliberately: what would be asserted is a
  timing, and a timing assertion in CI is a flaky test. The number lives here,
  and `panel_width_probe`-style measurement is the tool if it comes back.
