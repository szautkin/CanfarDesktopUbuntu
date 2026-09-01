# 26 — Panels that keep their width, and an account block that is not last

Status: plan. Every number below was **measured on the running app**
(`target/release/verbinal` at `960362b`), not read off the source.

## How it was measured

The display is 5120x2880 with a GDK scale factor of 2, so the app works in
**2560x1440 logical pixels**. A quarter of the screen is therefore
**1280x720 logical**, and the app's own default (`default_width(1200)`,
`default_height(800)`) opens at **1200x800 logical** — almost exactly the
quarter-screen case.

The app was driven through its own MCP socket (`navigate_to`, `open_fits_file`,
`open_cube`), resized with `wmctrl`, captured with `import`, and the captures
were profiled column by column to find panel boundaries. That is worth stating
because it is the only way to answer a layout question here: `gtk::init()` fails
on a spawned thread, so `cargo test` cannot measure an allocation, which is the
same gap `examples/layout_probe.rs` exists to cover.

## What is actually wrong

### 1. The Search page is CLIPPED, not squeezed

At the default 1200 logical width the Recent Searches panel is allocated about
**175 logical px against the 260 it asks for** — below its own stated minimum.
The consequences are all off the right edge of the window:

- the card's right border is not drawn at all; it runs into the window edge
- every row's **edit and delete buttons are entirely outside the window**
- "Clear All" is cut mid-word
- a saved coordinate target wraps mid-token across three lines —
  `13:29:5121,+47:12:145` renders as `13:29:51 / 21,+47:1 / 2:145`

And the same panel, at wider windows:

| window (logical) | 1200 | 1400 | 1600 | 1800 | 2000 |
| --- | --- | --- | --- | --- | --- |
| Recent Searches | **175** | 318 | 418 | 518 | 618 |

It takes **half of every extra pixel**, without limit. A panel that is 175 wide
when it needs 260 and 618 wide when it needs 260 is not two bugs; it is one
missing policy, seen from both ends.

The cause is worth naming because it is invisible at the call site:
`sidebar_scroll` never sets `hexpand`, but `recent_title.set_hexpand(true)` sits
**inside** it, and GTK propagates expansion upward from any descendant. One
label makes a fixed-role panel behave like a resizable pane.

### 2. A squeezed list beside an empty placeholder

Research and Workflows both give the detail pane 100% of any extra width, which
is right. What is wrong is where they start:

- **Research**: size badges truncate to `540.0 …`, `110.9 …`, `1….` while the
  right pane holds "Select an observation" and nothing else.
- **Workflows**: titles truncate to `Archival imaging reconnaissance (CFHT
  Mega…` beside an empty "Select a workflow" placeholder about **425 logical**
  wide at the default size.

This is exactly the case the ask describes: the panel carries the information
and is starved, and the pane that is starving it is showing nothing.

### 3. Both viewers' controls are simply absent at this size

The control column is **hidden below a window width of about 1400 logical**, and
docks at about 300 logical above it. At the default 1200 the FITS and cube
controls — colormap, stretch, cut levels, channel scrubber, marks — are not on
screen at all. They are reachable through the header toggle, and then they
**overlay** the image rather than dock beside it.

`viewer_shell::COLLAPSE_WIDTH_SP = 900` was chosen so "the image keeps roughly
two thirds of a small laptop's width". On this display it fires at 1400, because
`sp` is scaled and the nav sidebar's 280 comes off the top first. The intent was
right and the threshold is wrong for the machine it is being used on.

### 4. The window's stated minimum is not its real one

`width_request(480)` / `height_request(360)`; the window will not go below
**602x482 logical**, and reports the same floor on every page. `adw::ViewStack`
is homogeneous by default, so the widest page sets the minimum for all ten. The
declared numbers describe nothing.

### 5. Five separate ideas of how wide a panel is

| | |
| --- | --- |
| `viewer_shell::COLUMN_WIDTH` | 280 |
| `NavigationSplitView` | min 220, max 280 |
| `Paned::set_position` (file panel) | 280 |
| Search `sidebar_scroll` | 260 |
| Research / Workflows lists | whatever `hexpand` leaves |

Four numbers that are nearly the same and one that is not a number at all.

## The design

The ask — *"always show the proper width of the panels and squeeze the viewport
instead"* — is the right rule and it is not, by itself, enough. It answers
failures 1 and 2 completely. It does not answer what happens when the viewport
has also run out, which is when the clipping in failure 1 actually happens.

So: **one policy, three clauses, applied to every page that has a side panel.**

1. **A panel states its width and holds it.** `hexpand(false)`, a width request,
   and — because expansion propagates upward — nothing inside it may set
   `hexpand(true)`. The content pane is the only child that expands.
2. **The content pane takes the squeeze**, down to its own honest minimum.
3. **Below a breakpoint the panel stops taking space** and becomes an overlay
   the user opens, rather than being clipped.

Clause 3 is not new work: `viewer_shell` already implements exactly this with
`adw::OverlaySplitView` in an `adw::BreakpointBin`, and both viewers already use
it. The plan is to make the other pages use the same thing rather than each
inventing an answer — which is also why the fix for failure 3 is a threshold
change and not a redesign.

### The numbers, in one place

A `ui::metrics` module (or `viewer_shell`, extended — it already owns
`COLUMN_WIDTH` for the same reason):

```rust
/// A docked side panel. One number, or the app has several ideas of how wide
/// "a panel" is.
pub const PANEL_WIDTH: i32 = 280;
/// Below this, a panel overlays instead of docking.
pub const PANEL_COLLAPSE_SP: f64 = ...;
```

`PANEL_COLLAPSE_SP` has to be **measured, not chosen**: the current 900 fires at
1400 logical on this display, which is why the controls vanish at the default
window size. The right value is whatever puts the collapse just below the width
at which the content pane reaches its own minimum, and that is a probe result.

### What each page gets

| page | today | change |
| --- | --- | --- |
| Search | clipped at 175, unbounded when wide | clause 1 (drop the inner `hexpand`, pin 280) + clause 3 |
| Research | list truncates beside an empty pane | clause 1, at a width that fits a size badge |
| Workflows | titles truncate beside an empty pane | clause 1, at a width that fits a title |
| FITS / cube | controls absent below 1400 | re-measure `COLLAPSE_WIDTH_SP` |
| Storage (file panel) | `Paned` at 280, not shrinkable | already correct; adopt the shared constant |
| AI Guide | tiles collapse to one column | out of scope — see below |

## The second ask: the account block belongs at the top

Today the sidebar's `ToolbarView` carries a **bottom** bar holding, in order:
agent activity, service health, the Login / account button, and a status label.

Three things are wrong with that, and only one of them is the position:

- **It is last in reading order and first in importance.** "Am I signed in?" and
  "is the service up?" are what a person checks before doing anything, and they
  are at the far end of a 10-item list.
- **The name is printed twice.** The account button's label is the display name,
  and `status_label` immediately under it says `Welcome, {display name}`. One of
  those is redundant, and it is the sentence.
- **It competes with the nav list for height.** The footer is pinned and the
  list scrolls, so on a short window the footer takes space from the only part
  of the sidebar that has more to show.

Proposal: identity moves to the **top of the sidebar, under the header** — the
GNOME pattern (Fractal, Software, Console all put the account there). The
service-health control and the agent-activity indicator stay a compact strip
with it, since all three answer "what is the state of my connection to CANFAR".
`Welcome, {name}` is deleted rather than moved; the button already says the
name, and the status label goes back to being what it is useful for, which is
transient state ("Checking authentication…", "Session expired").

That empties the bottom bar. Removing it gives the nav list the height back.

## What this does not cover

- **Redesigning any page.** Every change above is about who gets the width, not
  what is in it.
- **The AI Guide's single-column tiles.** A `FlowBox` that folds to one column
  is behaving correctly for the width it has; making the tiles narrower is a
  content decision, not a layout policy.
- **The window's declared minimum.** Making `width_request(480)` true would mean
  every page fitting in 480 logical px, which is a mobile layout and a different
  project. Worth correcting the numbers so they describe reality, no more.
- **The cube's overlapping mark labels** seen in the captures. That is the
  auto-layout item deferred from the QA report, not a panel width.
- **Text scaling.** `sp` already accounts for it and the breakpoints use it; the
  fixed pixel widths do not, which is a real inconsistency but a separate one.

## Order of work

1. **Measure, and write the probe first.** Extend `examples/layout_probe.rs` to
   report, per panel, its minimum width, its natural width, and what it is
   allocated at 1200 / 1400 / 1600 logical. Without this the collapse threshold
   is another guess, and the last two guesses are the bug being fixed.
2. **One constant module**, and every panel width read from it.
3. **Clause 1 on Search** — the clipping is the only failure here that loses
   function rather than polish, and the inner `hexpand(true)` is a one-line
   cause with a one-line fix.
4. **Clause 1 on Research and Workflows.**
5. **Clause 3**: the shared collapse behaviour, at the measured threshold, on
   the pages from 3 and 4; and the same threshold applied to `viewer_shell`.
6. **The account block moves up**, and `Welcome, {name}` is deleted.

Steps 1-3 are worth doing even if nothing else here is.

## How it gets verified

The honest constraint first: **no `cargo test` can measure a widget
allocation**, because `gtk::init()` fails off the main thread. So the split is:

- **A probe measures.** `layout_probe` prints each panel's minimum, natural and
  allocated width at three window widths, and fails when a panel is allocated
  less than its own minimum — which is precisely the Search bug, expressed as a
  number rather than as a screenshot.
- **A test guards what is expressible in source.** No widget inside a panel sets
  `hexpand(true)`; every panel width comes from the shared constant; the
  breakpoint threshold appears once. These are the three things that regress
  silently, and all three are greppable.
- **A screenshot review, once.** The clipped card and the mid-token wrap are
  things a number will not describe. The captures in this plan are the "before";
  the same six pages at 1200 logical are the "after".

The specific claims to re-measure after the change:

| claim | today |
| --- | --- |
| Search panel is allocated at least its minimum at 1200 logical | 175 of 260 |
| Search panel does not grow past its stated width | 618 at 2000 |
| No row control is outside the window at 1200 logical | edit + delete are |
| Viewer controls are on screen at the default window size | absent below 1400 |
