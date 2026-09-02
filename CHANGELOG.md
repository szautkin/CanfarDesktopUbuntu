# Changelog

All notable changes to Verbinal (the native Linux CANFAR Science Portal companion).

## [Unreleased]

### The workflows run on a Node that is still supported

- All four actions — `checkout`, `cache`, `upload-artifact` and
  `action-gh-release` — declared `using: node20`, whose runtime is past
  end-of-life on GitHub's runners. A deprecated runtime is a warning on every
  run first and a failure on a morning nobody chose second.
- Bumped to the majors that declare `node24`: checkout v4 → v7, cache v4 → v6,
  upload-artifact v4 → v7, action-gh-release v2 → v3. Each `action.yml` was read
  at both the old and the new tag rather than inferred from the version number.

## [1.4.2] - 2026-09-01

A release about being told the truth. An agent asking for something impossible
was answered "done"; the ADQL checker shipped last week had never once run,
because its schema fetch panicked the main thread on the way in; the About
window's Runtime Info named the app's own version as the Rust version and gave
no version at all for GTK. Underneath, three pieces of code that existed twice
now exist once — including a whole streaming download whose copy had quietly
missed out on cancellation.

### The compiler is pinned, so the gates mean what they say

- The README promised that `cargo clippy --all-targets -- -D warnings` is "the
  exact command CI runs, so a green local run means a green CI run". The command
  was exact; the compiler was not. CI took `dtolnay/rust-toolchain@stable` — the
  day's stable — and a developer had whatever stable they last updated to.
- Rust 1.98 added `clippy::chunks_exact_to_as_chunks`, and three call sites that
  had been in the tree for months went red in CI while staying green on a 1.97
  desktop. No amount of care locally would have found it: a lint that does not
  exist on your machine cannot fire on your machine.
- `rust-toolchain.toml` now pins the version, both workflows install that
  version, and a test fails if the three copies drift. The three sites use
  `as_chunks::<4>()`, and `rust-version` records the 1.88 floor the code needs.

### What the app does, described as it actually is

- **The feature list was written when there were seven of them.** It ran the
  Portal's own minutiae (Auto-Refresh, Recent Launches) alongside whole
  applications (FITS Viewer, Archive Search) as thirteen equal bullets, and left
  out Research, Workflows, the AI Guide, finding an image by package, figure
  export and the ADQL editor entirely. It is now written by area, the way the
  app presents itself on its Home page.
- **The AppStream description** — what a software centre shows — gained the same
  areas. It had described two viewers and an agent, which was a third of the app.
- Counts that came from one screenshot ("368 images", "17106 entries") are
  described by their scale instead, because the numbers change and the README
  does not.

### The ADQL checker had never run

- The schema fetch that feeds it was awaited on the GLib main context, where
  there is no tokio reactor, so it **panicked the main thread the moment the
  Search page was built** — "there is no reactor running". The schema therefore
  never arrived, `cached()` stayed empty, and the checker, which says nothing
  when it has nothing to check against, said nothing. A check that silently
  never runs is the worst way for one to fail: from the outside it is
  indistinguishable from a query with no problems.
- The fetch now goes through the tokio runtime like every other request. The
  editor underlines the offending words, and Execute greys out, from a cold
  start — which is what the previous release said it did.
- `validate_adql_query` and `execute_adql_query` were unaffected: they fetch the
  schema themselves, on the runtime, which is why the tool worked while the
  editor did not.

### About points at the project, and says what it is running on

- **About and Help open [verbinal.com](https://verbinal.com)**, the project's own
  page, rather than `canfar.net` — that is the SERVICE this app talks to, and the
  two answer different questions. About also carries a "Report an Issue" link to
  the tracker and a support address, so a report arrives with a version beside it
  instead of "the latest one".
- **Runtime Info names the versions a bug report needs.** It said "Rust
  {app version}" — the app's own version wearing the toolchain's label — and
  "GTK4 + libadwaita" with no version at all, so the one section meant to help
  someone report a bug named nothing that varies between the machines where a bug
  appears. It now reads the real GTK and libadwaita versions at run time.
- The project's addresses live in one place with a test that the packaging, the
  metainfo and the README all name the same ones; nothing compiles those three,
  so a moved page would otherwise be found by a person landing on a 404.

### One implementation of a download, and of a sky coordinate

No behaviour was meant to change here, but one did, for the better.

- **The Search page had its own private copy of the whole streaming download** —
  191 lines, doc comments and all, forked from `services::transfer` and since
  drifted: the original grew cancellation support and the copy did not. Saving an
  observation to Research now runs the same code every other transfer does.
- **Four sky-coordinate formatters existed twice**, in `cube_axes` and again in
  `cube_export`, whose copy was justified in a comment as being there "so the
  footer ranges match the live axis captions verbatim" — the one guarantee that
  copying them cannot give. They now live once, in `helpers::sexagesimal` beside
  their long-form siblings, with a test that no file grows a private copy again.
- The two readers that decide what counts as a number now share one definition,
  so a tool that validates with one and applies with the other cannot disagree.

### An agent can see what a dropdown offers, and a bad value is refused

- **`rowsPerPageOptions`** reports the Rows/page menu — 25, 50, 100, 250, 500 —
  beside the size in force. Any whole number from 1 to 1000 is still accepted;
  the menu is the menu, not the limit.
- **Each column reports its own display `units`**, so a caller picks one instead
  of guessing and reading the refusal. RA offers hms/degrees, Int. Time offers
  seconds through days, the spectral columns offer fourteen.
- **A value that is not a whole number is refused rather than ignored.**
  `{"rowsPerPage": "abc"}` used to report success and change nothing, because
  the reader answered "absent" for both missing and unreadable. `"42"` is still
  accepted — agents send that routinely — but `12.5`, `null` and `"abc"` are
  now refused, saying what was wanted and what arrived.

### A hand-written query keeps the columns it selected

- `SELECT target_name, collection` came back as **`collection` alone** — the
  target silently dropped — while `SELECT target_name, type` came back with
  both. Adding a recognised column to a query made the others disappear. The
  column-visibility preference is about the SEARCH FORM's own result shape, and
  applying it to an arbitrary query filtered out every column it had never
  heard of. A result that is not the form's shape now shows everything it
  returned.
- **`set_search_results_view` reports `rowColumns`.** It answers without rows —
  a hundred of them on every column toggle is a lot of wire for a confirmation —
  and it used to drop the header list with them, so revealing a column and then
  checking the reply looked like nothing had happened.

### `validate_adql_query` — check a query without spending one

- The same check the editor and `execute_adql_query` run, offered on its own, so
  an agent composing a query can ask whether it is acceptable instead of finding
  out by running it. It returns each problem with the text it objects to and,
  where there is one obvious answer, what to write instead. It does not touch
  the ADQL editor: checking a draft should not replace what someone has open.

### An ADQL query is checked against the service's own schema before it is sent

- **The mistake that prompted it**: `FROM caom2.Observation JOIN caom2.Plane ON
  Plane.obsID=Observation.obsID` is refused by CADC with "Column [obsID] is
  ambiguous", because a bare table name is only a valid qualifier while the
  column it names belongs to one of the joined tables. Writing `AS p` or
  `caom2.Plane.obsID` both work.
- The offending words are underlined in the editor, the reason and the fix are
  shown beside the title, and Execute is greyed until the query is one the
  service would accept. The same check runs on `execute_adql_query`, so an
  agent's query is refused with the reason instead of spending a round trip.
- **It only reports what it is sure of.** A subquery, a function, a table the
  schema has not been fetched for — each is left alone, because a false positive
  here disables Execute on a query that would have worked.

### Several rows at once, and a metadata sheet you can read

- **Multiple selection.** Click picks one row, Ctrl-click adds or removes one,
  Shift-click takes the range — and neither modifier opens the dialog, because
  adding a fourth row to a comparison should not put a window over the three
  being read. `selectRow` takes one index, an array of them, or null, and
  `get_search_results` reports `selectedRows`.
- **The row dialog's metadata is in two columns**, with one heading above both
  so they start at the same height. A row has 41 columns and the dialog shows
  every non-empty one, which was a list to scroll rather than a sheet to read.

### A results row can be highlighted, and its dialog opened, from MCP

- **`selectRow`** highlights one row of the filtered results and pages to it, so
  "this one" means something to the person looking at the window. Clicking a row
  highlights it too, and `get_search_results` reports `selectedRow`.
- **`show_search_row_detail`** opens the detail dialog for the highlighted row —
  every column of it, the same window a click gives.
- **The Rows/page dropdown follows the model.** Setting the page size over MCP
  changed the model and not the control, so it read "Rows/page: 100" above a bar
  counting "31-40 of 60". The size is set in one place now, and that place moves
  the dropdown.

### A stack buffer overflow in the FITS error path

- **Opening a FITS file that produced a long cfitsio error crashed the app.**
  `ffgmsg` writes up to 81 bytes into the buffer it is handed, and both call
  sites gave it a 31-byte stack array — so any message longer than thirty
  characters wrote up to fifty bytes past the end of it. "failed to find or open
  the following file: (ffopen)" is one, which is why opening a path that did not
  exist ended in `double free or corruption (out)`.
- **Two cfitsio decodes at once read each other's errors.** cfitsio keeps its
  messages in a process-global stack and the cube loader decodes on a worker
  thread, so opening four cubes in a row reported the missing file's message for
  the malformed one and vice versa. One lock now lets one caller in at a time.
- **A failure left messages behind for the next one to find.** cfitsio queues
  several per failure and only one was read, so opening a text file left cards of
  it on the stack and the next error reported them: `Cannot open FITS file:
  [package] name = "verbinal"`. The stack is cleared when the lock is taken.
- **`open_cube` reports the load, not the request.** It answered `opened: true`
  the moment the decode was handed to a worker, so a file that was not a cube was
  reported as open while a toast said otherwise. The same bug `open_fits_file`
  had, and the same fix.

### open_cube takes an observation id, like open_fits_file

- `open_fits_file` accepted a path OR the id of a downloaded observation;
  `open_cube` took a path and nothing else. So an agent could open a download as
  an image but had to resolve the id itself to open the same file as a cube —
  and the observation detail page offers "Open in Cube Viewer" to a person. Both
  now go through one resolver, so "open this thing" means the same in either
  viewer, down to the two failure messages.

### View, Save and More are the first three columns

- The three things you can DO to a search result were the LAST three cells in
  the row, past a dozen data columns — 41 with every column shown — so they were
  off the right-hand edge of the table and only reachable by scrolling there.
  They are first now, with their headings, which is where a table puts what a
  row is for.

### Two things in the results table an agent could not reach

- **`show_observation_detail`** opens one observation's CAOM2 detail page in the
  window, the way a results row's Details button does. Every other control in
  that table already had a tool, and each row's three buttons had one for the
  DATA behind them — so an agent could read an observation's metadata but not
  put it in front of the person it was explaining it to.
- **`get_search_results` takes `allColumns`.** It returned the dozen columns the
  grid is showing; a row has 41. The only way to see the rest was to change the
  grid's column visibility, which is a change the person watching would see.
  This is the same data the row-detail dialog shows when a row is clicked.

### Steering the results table is about three times faster

- **Every cell in the results grid stored its own tooltip**, plus three action
  buttons a row — roughly 1,800 of them on a hundred-row page, rebuilt on every
  column toggle, sort and page turn. `set_tooltip_text` costs about 1.2 ms a
  widget, which was most of the time each of those took. The grid answers for
  its own cells through one handler now: hiding a column went from 0.90 s to
  0.35 s, and the main thread is blocked for a third of what it was. The full
  value of an elided cell is still on hover.

### Two ways the results tools made an agent guess

- **A column key is matched whatever case it is written in.** The keys are
  cleaned lower-case names, but an agent reads "Filter" and "Instrument" off the
  heading strip, and being refused for the case taught it nothing it could not
  have guessed. The canonical key is what gets stored, so a filter written as
  `TargetName` matches the same rows as `targetname` instead of matching none.
- **A rejected display unit names the units that column takes.** It used to say
  only that the unit was wrong, so `deg`, then `sexagesimal`, then giving up was
  three round trips to learn something the app already knew. It now reads:
  `"deg" is not a display unit for column "ra(j20000)"; it takes ["hms",
  "degrees"] (or "" to reset)`.
- `get_search_results` now names its pagination fields in its own description —
  `currentPage` (0-based), `totalPages`, `rowsPerPage`, `pageStatus` — because
  looking for `page` and `pageCount` and finding neither reads as a bug.

### Execute in the ADQL Editor looked like it did nothing

- **The spinner and the status line were in the Search Form's action bar**, and
  the Search Form is the one tab you are not on when it matters. Pressing
  Execute in the ADQL Editor set "Searching…" and started a spinner on a tab you
  could not see, then sat silent for however long CADC took — indistinguishable
  from a button that does nothing. They are in the page header now, beside the
  title, visible from all three tabs.
- **Both buttons that start a query grey out while one is running**, so a second
  press cannot queue a second query against a service that answers in tens of
  seconds.
- A failed query's message is now visible from whichever tab you ran it on.

### The agent's arrival and departure, heard and seen

- **Two short sounds**, one when an agent starts calling tools and a different
  one when it goes quiet — rising for a start, falling for a finish, about a
  third of a second each. On by default, with a switch in Preferences under
  Appearance. Nothing else in the application makes a sound.
- **The three dots travel** while an agent is working and sit still when it is
  not, so a glance answers the question without reading the words. They follow
  the desktop's animation setting, and the frame clock rather than a timer, so
  they cost nothing while idle.

### The agent says what it is doing, and a reset stopped taking fourteen seconds

- **`reset_search_form` timed out.** It took about fourteen seconds and grew
  from there, because clearing the form tore down and rebuilt every row in all
  seven Additional Constraints columns. The values in those columns are
  computed once, when the data train loads, and never change — only which are
  available and which are ticked. Updated in place, the same call takes 0.0 to
  0.2 seconds and stays there.
- That also fixes the jump: with nothing torn down, a click no longer sends
  every column back to its first row.
- **The agent indicator is always on screen**, beside the service health, as
  `agent idle` or `agent working…` with the same robot icon the AI Guide uses.
  It used to appear only while an agent was working, which is indistinguishable
  from its not existing: nothing told you it was a thing to look at, so a tool
  call that did nothing visible looked like nothing had happened.
- **The Search panel's collapse threshold had 17 px of margin**, and the app
  logged `AdwOverlaySplitView exceeds AdwBreakpointBin width: requested 933 px,
  920 available` when a resize landed in the gap. It has about 70 now, and the
  probe fails below 40.

### Additional Constraints jerked sideways, and Home kept asking a signed-in user to sign in

- **Clicking a facet checkbox scrolled its column to the right.** A value like
  `Infrared|Optical|UV|EUV|X-ray|Gamma-ray` gave a row a 257 px MINIMUM in a
  100 px column — `CheckButton::with_label` has no label to ellipsize, so the
  row could not shrink. The column scrolled horizontally, and clicking a
  checkbox focused it, so GTK scrolled sideways to reveal the rest of the row.
  The rows ellipsize now, with the full value on a tooltip, and the columns
  scroll vertically only.
- **And every column jumped back to its first row** on each click, because
  toggling one facet rebuilds all seven. The scroll positions are kept across
  the rebuild.
- **"Log in with your CADC credentials to get started" stayed on the landing
  page after signing in.** It was a plain label that nothing told about the
  sign-in state, sitting under the tiles while the sidebar above it showed the
  account name. It is driven by the same lockers that unlock the tiles now.

### Five things that were cut off, blank, or silently refused

- **The window drew 267 px past its own bottom edge.** `AdwViewStack` is
  homogeneous by default, so every page was allocated the size of the tallest
  one: content 945 px tall inside a 678 px window. The visible results were on
  pages that had nothing to do with the tall one — the Search form's Search
  button was off the bottom at any window under about 1000 logical px, the last
  navigation rows were unreachable, and the CANFAR Images card ran past the
  frame. Pages size themselves now.
- **A tool description containing `<user>` rendered as nothing.** Rows treat
  their title and subtitle as Pango markup, so `vos:<user>/workflows/` was an
  unclosed element: GTK logged a warning per row per rebuild and drew no text.
  Every row that shows a name, a path, a URL or an error — thirteen of them, in
  six files — says it is text now, and so does the Search page's error banner,
  which shows whatever a TAP service sent back.
- **The AI Guide lost its icon** in the sidebar and on the home page. The icon
  search path for a source tree had been put behind `debug_assertions`, which
  silently dropped it from `cargo build --release`. It is derived from the
  running executable now, so it works in either profile and embeds nothing in a
  shipped binary.
- **The AI Guide's tiles were one per row** at a quarter-screen window while
  the home page's were three. Both build their grid from one place now, and a
  tile states a width so the grid stays a grid.
- **`navigate_to("portal")` answered "navigated" and showed Home.** The alias
  outlived the split that gave the Portal a page of its own.

### Panels keep their width, and the account is where you look first

- **The Search page was clipping its own right panel.** At the window the app
  opens at it needed 844 px and had 797, so GTK drew Recent Searches 47 px past
  the window edge: every row's edit and delete buttons were outside the window,
  "Clear All" was cut mid-word, and a saved coordinate wrapped mid-token across
  three lines. Widen the window and the same panel took half of every extra
  pixel instead — 175 px at 1200 and 618 at 2000 — because one label inside it
  set `hexpand`, which GTK propagates upward. It is a panel now: it states its
  width, holds it, and steps aside into an overlay rather than off the edge.
- **Both viewers' controls were absent at the window the app opens at.**
  Colormap, stretch, cut levels, the channel scrubber and the marks panel were
  reachable only through a toggle, and then only as an overlay over the picture.
  Two causes: a collapse threshold measured for a different machine, and a
  picture that claimed an 800 px minimum it did not need. They dock now, and the
  picture takes the squeeze.
- **The Research and Workflows lists** were truncating the names they exist to
  show — `110.9 …`, `Archival imaging reconnaissance (CFHT Mega…` — beside a
  pane displaying an empty "select something" placeholder. The list keeps its
  width; the pane gives it up.
- **The account moved to the top of the sidebar**, under the header, with
  service health and agent activity beside it. It was the last thing in the
  sidebar and the first thing anyone checks. The display name is no longer
  printed twice.

### Marks can be styled, and they survive being exported

- **Colour, size, weight and thickness for a mark**, from a Style row in the
  Marks section that both viewers already mount. The controls act on the
  selected mark when there is one, and on what the next mark will look like
  otherwise. The default is remembered, read when a mark is CREATED and copied
  into it — never at draw time, so changing it leaves every mark already drawn
  alone. An agent's marks keep their own green: the setting means "what I
  draw".
- **Marks kept their screen size in an export.** At 4x the picture was
  re-rendered at four times the resolution and the plate's own title, caption
  and colorbar scaled with it, while a 2px ring stayed 2px and a 12px label
  stayed a 15x10px smudge — so the annotations were the one thing in the figure
  that shrank, and they became unreadable at exactly the resolution someone
  chose for publication. Stroke, label, leader and rule now follow the
  rendering, in both directions: an agent's downscaled capture thins them to
  match too.
- The two inks are written from their 8-bit channels now. A colour is stored,
  shown and sent as `#rrggbb`, so a constant with more precision than that could
  not come back from its own storage. Same colour on screen, to the byte.

## [1.4.1] - 2026-08-30

An imaging release. You can now draw on a FITS image or a cube, an AI agent can
see what you are looking at and point at part of it, and a wrong sky coordinate
that had been there the whole time is fixed.

Patch by instruction, not by content: this adds tools and features that would
normally be a minor. The version scheme is patch-only until that changes.

### Sky coordinates were wrong on modern FITS

- **A JWST image was 40 to 90 arcseconds out.** `parse_wcs` read the CD matrix
  and the older CDELT+CROTA2 form, and a JWST i2d header has neither — it has
  PC + CDELT. The scale stayed right and the rotation silently became zero, so
  the error was nil at the reference pixel and grew with distance from it. The
  standard's `CDi_j = CDELTi * PCi_j` is read now, with CD still winning when
  both are present.
- **Then a one-pixel offset underneath it.** Checking the fix against astropy
  left a residual that was CONSTANT with distance — the signature of a
  convention error, not a projection one. `pixel_to_sky` speaks the FITS
  convention (1-based, pixel centres, because that is what CRPIX is stated in)
  and the canvas was feeding it corner-origin display coordinates while the MCP
  tools fed it 0-based array indices. The other two conventions have names now,
  each paired with its inverse so crosshair and mark sync still cancel exactly.
- The astropy tests were passing at a tolerance eleven pixels wide, which is
  what hid this. Tightened, and extended to the far corner of the frame where a
  rotation error is largest.

### Marks

Draw on an image or a cube, say what you are pointing at, and have it still be
there tomorrow.

- Circles and boxes on the FITS canvas, the cube's 3D volume, and its 2D slice,
  through one renderer — a shape cannot start looking different depending on
  where you see it. Blueprint styling, with a fine leader at a fixed angle
  carrying the label.
- Click to place, drag to size, click to open, drag to move, drag a grip to
  resize. The shape follows the pointer while you draw it, and it is the shape
  you will get: the picker is asked at draw time, not remembered.
- **A cube mark lives on a channel.** It is drawn on the slice showing that
  channel and not on any other, because it is not at that position on any
  other. The list carries the rest, and picking one takes you to its channel.
- Marks persist with the file and appear in exported figures. Editing grips do
  not: an exported figure shows what is marked, not the controls for adjusting
  it.
- **MCP:** `annotate_fits`, `annotate_cube`, `update_annotation`,
  `select_annotation`, `remove_annotation`, `clear_annotations`, and the two
  listings. An agent can draw, label, move, resize, restyle, highlight and
  delete a mark, and read back its size in the units the tools take.

### An agent can see the working area

- `get_fits_image` and `get_cube_image` return what is on screen — the current
  zoom, pan, colormap, crosshair and marks — rather than a fresh render of the
  file. Each carries the transform needed to turn a position in the picture
  back into a sky or voxel coordinate, which is what makes pointing possible.
- Two settings bound the cost: the largest agent image in pixels and in MB. A
  4000px capture costs an agent roughly sixteen times the context of a 1000px
  one and tells it nothing more.

### Opening a FITS

- **It opens showing the whole frame.** An 11471x4593 mosaic opened at 100% and
  showed about 5% of its width. It now fits the viewport on the tighter axis,
  centred, and never enlarges — a 64x64 thumbnail stays at 100%. It happens
  once, on the first frame with a real size, and any zoom you choose cancels it.
- **The empty state offers the last files opened**, on the same component and
  the same store as the cube's, which had this already.

### The cube viewer

- **The slice opens at the size the volume shows it.** The volume frames the box
  so it never clips while orbiting, which is further out than fitting a plane to
  the widget, so everything changed size when you switched modes. The default is
  measured from both views rather than picked. The slice can also be zoomed out
  past fit now, which it could not before.
- Marks in the exported figure, which returned the bare render.
- `close_cube_tab`; `get_cube_view` says which cube it describes and reports the
  slice's own zoom and pan; the Info panel's object, telescope, instrument and
  value range reach an agent.
- **`cubeTabs[].path` was not a path** — it was the display name, so an agent
  could not reopen what it listed.

### MCP

- **A map of the tool surface.** `list_apps`, `describe_app`, `search_tools` and
  `man(tool)`, so an agent can find the dozen tools it needs without carrying
  all 134.
- **CAOM2 column guidance.** Agents were guessing column names from a sentence
  in a tool description and getting 400s back; the schema is described now.
- Query-path fixes from the m51 QA run, a bounded VOSpace listing, and clearer
  errors where an agent was most likely to be wrong.

### The sidebar

- Header info, saved coordinates and marks are one list component: same height,
  same filter, same selection behaviour, per-section row actions. Selecting and
  deselecting work on one click, which they did not.
- Bookmarks select like marks, and their button searches the archive at that
  position.

### Fixed

- Segfault when choosing an extension from the HDU dropdown.
- Stack overflow when placing a mark.
- The zoom dropdown said 100% while the image was at 28%.
- Sync zoom used a stale scale.
- The image surface is built once, not once per frame.
- A mark with no radius was invisible, on both viewers, for different reasons.
- With the pencil armed, pressing an existing mark made another one on top
  instead of grabbing it.

## [1.4.0] - 2026-08-27

A notebook release. The subsystem could not show what a cell produced, and two
of the files it offered to open were destroyed by saving them. Both are fixed,
along with the agent-facing side: a figure can now be fetched as an image, a
cell can be run with a deadline, and a reply says what it contains.

Minor rather than patch: it adds a tool, a file format, a setting and several
renderers. Nothing existing changes shape.

### Notebook output

- **Only two of the four MIME types were ever drawn.** `OutputData` has
  modelled `text/html` and `image/jpeg` since the parser was written, and the
  renderer asked "is there a PNG?", then "is there text?", and stopped. An
  `astropy.table.Table` — the output an astronomer looks at most — arrived as
  its `repr()` with the HTML sitting unread in the same bundle. The choice is
  now one function with an exhaustive match, so a MIME type that is modelled
  and not rendered does not compile.
- **The harness asked one question of every object.** "Are you a matplotlib
  Figure?" — so a PIL image came back as `<PIL.Image.Image ...>`. It asks the
  object now, through `_repr_html_`, `_repr_png_`, `_repr_jpeg_`, `_repr_svg_`,
  `_repr_markdown_`, `_repr_latex_` and `_repr_json_`, and a method that raises
  costs the object only that one representation.
- **Images rendered as blank cells even once the data arrived.** A
  `GtkPicture` with `can_shrink` reports a MINIMUM height of zero, and the
  notebook packs cells into a `GtkListBox`, which allocates rows their minimum;
  every figure was built correctly, handed real pixels, and drawn in one pixel
  of height. Labels survived because a label's minimum is its text. Output
  images state their size now, derived from the texture — which also stops a
  140x90 thumbnail being upscaled and blurred across a fixed 400px.
- `image/svg+xml`, `text/markdown`, `application/json` and `text/latex` are
  rendered (LaTeX as its source; there is no LaTeX renderer yet). Each
  previously fell through to `text/plain`, which for an object without a
  `__repr__` is a memory address.
- **`display()` works, with or without IPython.** With the library installed,
  its `display()` is routed through the harness: publishing display data is the
  job of the kernel around it, and outside one the library prints a repr to
  stdout. Without it, `from IPython.display import HTML, Image, Markdown` still
  works — built on demand, so a notebook that never mentions IPython never sees
  it. An earlier version registered the stand-in at startup and broke every
  matplotlib cell, because `pyplot` reads that module to decide whether it is
  in a notebook.
- **`plt.show()` warned that it could not show anything**, and the figure then
  appeared anyway at the end of the cell. It renders on the spot now, as
  `%matplotlib inline` does.

### Notebook execution

- **A cell mixing a magic with Python failed outright.** `%pip --version`
  followed by `print(...)` was a syntax error in which neither half ran.
  Segments now execute in source order, keeping their line numbers.
- **Tracebacks showed the harness's internals.** The frame stripper tested
  `"__file__" in dir()` from inside a function, where `dir()` lists locals and
  `__file__` is a module global — always false, so nothing was ever stripped.
  A traceback starts at the user's own line; frames BELOW it, inside numpy or
  the stdlib, are kept, because that is the user's stack.

### Files

- **Saving a `.py` or `.md` replaced it with nbformat JSON.** The Open dialog
  offers both, so the way to lose a file was to use a feature. Each format is
  written back as itself, byte for byte when nothing changed.
- **A `.py` or `.md` opened as ONE cell** holding the whole file. They are
  split on the `# %%` markers jupytext, VS Code, Spyder and PyCharm share, and
  on fenced python blocks. A script with no markers is still one cell.
- `.txt` and `.log` open as notes you can add a code cell under. `.html`,
  `.pdf` and other export formats are refused by name with the reason —
  converting a notebook to HTML is one way — instead of being reported as
  invalid notebook JSON.
- **The only limit on reading a file was a cap on the number of CELLS**,
  reached long after the bytes are in memory. There is a size limit now,
  default 64 MB, adjustable in the notebook settings.

### Agents

- **`get_cell_image`** returns a cell's figure as real MCP image content. An
  agent could see `hasImage: true` and `<Figure size 640x480>` and had no way
  to reach the pixels. `get_cell_output` still does not carry them: inlining
  base64 into every read spends a caller's context on data most calls never
  wanted.
- **`run_cell` takes a `timeout`.** It was unbounded, so a cell that looped
  held the call open until the client gave up, and the only escape was
  `interrupt_kernel`. On expiry the reply says `timedOut` and `running`; the
  cell is not cancelled.
- **`run_all_cells` waited for nothing.** It returned the instant the sweep was
  spawned, so a caller reading outputs immediately saw none, with nothing in the
  reply to say why. It waits, like `run_cell`.
- **Replies carry `structuredContent`** as well as the JSON text they always
  had, so a client no longer parses a document out of a string field.
- **`get_cell_output` reports `richTypes`** — every MIME an output carries —
  and reads answer `cellType` as well as `type`, which is the name the write
  tools take.
- **`open_notebook` reported success for files it could not open**, answering
  with whatever tab happened to be open. It propagates the failure now.
- `list_notebooks` entries carry `kind` and `exists`. A `.md` in that list was
  never a stray file — this editor opens Markdown as a notebook.

### Interface

- **Kernel status was one English sentence doing three jobs**: the text on
  screen, the colour of the dot (found by searching it for the word "idle"),
  and the `state` field over MCP (found by searching it again). Two consumers
  read it by substring, so it could not be translated — except for the one place
  that set the first label through the translator, which is why a French desktop
  showed "Noyau : non démarré" and then switched to English. The state is a
  value now: a stable keyword for machines, English for the API, translated for
  the window.
- **A markdown cell rendered nothing if any line confused the converter.** Four
  independent replace passes paired the underscore in `snake_case` with the one
  in `proposal_id` — across a code span — and Pango refuses malformed markup
  outright, so one line blanked a 12 KB document. Inline markdown is one
  left-to-right pass now: code spans are literal, `_` is emphasis only at a word
  boundary, and a cell whose markup is ever rejected shows its source rather
  than nothing.

## [1.3.7] - 2026-08-22

A bug-fix release from a sixth QA session. Two entries below are corrections to
fixes that shipped in 1.3.6 and did not hold, and one is a fault that was never
ours; the notes say which.

### Downloads

- **A download that produced nothing counted as a success.** `caom2ops/pkg`
  answers HTTP 200 with an empty body and no content type for a publisher id it
  cannot resolve — measured anonymously — so the status check passed, a
  zero-byte file landed in the research library, and the job reported
  "Downloaded obs-….fits (0 bytes)" as succeeded. Zero bytes is refused now, the
  empty file is removed, and the message distinguishes a bad id from an empty
  artifact. CADC MIRRORS JWST, so a real publisher id is
  `ivo://cadc.nrc.ca/mirror/JWST?<observationID>/<productID>` and the message
  shows that shape.
- **DataLink faults were skipped in silence.** A row carrying `error_message`
  was dropped without a word, so a response that was entirely faults —
  `UsageFault: invalid ID` — parsed to an empty file list and read as "resolved,
  nothing here". A response with no rows and at least one fault is now an error
  carrying the fault text. A fault beside real rows is still not fatal.
- **A JWST plane downloaded its index file, not its image.** `#this` marks a
  science product and most collections publish one; JWST publishes several, and
  the 46 MB `_i2d.fits` is rarely first. Four of six planes sampled fetched a
  four-kilobyte `_asn.json` instead.
- **Every record in the library was anonymous.** ra, dec, target, instrument,
  filter and calibration level were empty for all eight records on the test
  machine — those fields were only ever filled by SAVING a search result, so
  anything fetched by publisher id arrived with a file and nothing else. A
  download now asks the archive: one indexed query on `publisherID`, 0.26s,
  returning the columns the search grid shows. Only empty fields are written.

### Health checks

**Three services reported a 4xx beside a tick.** The label was not the fault —
the probe was. It sent `GET` to the WORKING endpoints, and a bare GET on a TAP
sync endpoint is a malformed request that a healthy service answers 400;
`whoami` answers 401 to an anonymous caller. Both were then called healthy
because the host had replied.

Every IVOA service publishes `/availability` for this question, and all four of
CADC's answer 200 with `<vosi:available>true`. The probe reads that document
rather than the status line, so a service announcing planned downtime is
reported as down with its own note. Each service also carries `requiresAuth`,
and the summary carries `usableCount` beside `healthyCount`: a signed-out
session can have every service healthy and none of them usable.

### FITS viewer

**Switching HDU destroyed the viewer.** Four faults on one path — the tab was
unregistered and never re-registered, the published HDU numbers were off by one
against the numbers the same tool accepts, a failed switch reported success, and
the reply described the tab the caller had just left.

**A 720x360 image showed "64x64 pixels".** The status line is viewer-wide and
nothing refreshed it when the selection changed, so it kept describing the tab
you left — and `get_fits_view` reported that stale text.

**FITS tabs can be closed.** `close_active_tab` is an app-level stub that has
never been wired to a module; it answered `closed: false` for every call with no
reason, and the documented switch-then-close sequence could not work because
switching moves the viewer's focus and not the app's.

### For agents

- **`describe_tap_schema`** reads the archive's own `TAP_SCHEMA`: 21 tables with
  descriptions and column counts, or one table's columns with datatype,
  description, unit and UCD, plus the declared joins. An agent writing ADQL had
  two table names and one join, both from a sentence in a tool description, and
  had to guess the rest — `caom2.Plane` alone has 78 columns. Fetched once and
  cached; measured at 1.0s cold, 1.7µs warm.
- **Unknown arguments are refused by name.** Every schema said
  `additionalProperties: false` and nothing enforced it, which produced three
  separate misdiagnoses in one QA session.
- **`get_proposal_state` accepts either spelling of the id.** Queueing answers
  `proposalId` while the lifecycle tools took only `id`, so passing back the key
  you were handed produced `{"id": "", "state": "unknown"}` — the same answer as
  for a proposal that never existed.
- **Pending proposals survive a restart**, rehydrated under their original ids.
- **Sessions report `cpuInUse` and `memoryInUse`.** Skaha leaves the REQUESTED
  figures empty for notebook sessions while still reporting usage, and the
  payload carried only the empty half.
- **`get_downloaded_observation` reports `localPath` and `fileExists`.**
- **A slow notebook cell says it is running** instead of "UI busy".

### Interface

- **The agent icon is a robot**, as it is on Windows and macOS, instead of a
  laboratory flask.
- **CANFAR image manifests are checked on request**, not on every sign-in. The
  sync walked every manifest in the user's ARC space at 400ms a file, toasting
  its way through, on the screen where they were trying to start work. It is a
  "Check images" button now.
- **The AI Guide sorts every tool into a real category** — twenty-seven were in
  "Other", including every search tool — and its headings appear in French,
  which they were translated for all along.

### Not fixed, and why

**VOSpace `contentType` is null** because CADC does not publish the property. A
live, anonymously readable node carries `creator`, `date`, `ispublic`, `length`
and `quota`, and nothing else; a container has no MIME type to report. The
parser reads `#contenttype` and always has. No value is guessed from the
filename, because that would put an invention in a field labelled as the
archive's.

## [1.3.6] - 2026-08-21

A bug-fix release driven by two QA sessions against the live service. Several
of these were not what they looked like, and the notes say which.

### The FITS viewer destroyed itself on an HDU switch

Four faults on one path. Switching to a valid image HDU answered `tabCount: 0`,
the HDU it had just left, and `isError: false` — and every call after it said
"no FITS open" while the page was still on screen.

- A switch rebuilds the page: close the old, insert its replacement. The close
  handler retains the tab OUT of the registry, because it cannot tell a
  replacement from a close, so the re-registration below indexed a slot that no
  longer existed and silently did nothing.
- `HduInfo::index` is the 1-based CFITSIO number and `set_fits_view` passes it
  straight to cfitsio, but `get_fits_view` published a 0-based counter instead
  of the field the model already carried. An agent reading `hdus[1].index` and
  passing it back selected the PRIMARY — and on a primary with no image data,
  that is the "CFITSIO status 301" the report saw.
- A failed switch wrote its error into a status label and reported success.
- The reply described the pre-switch tab, as did the crosshair and centre
  operations after it.

### run_code could never have worked

Every execution failed on a doubled path,
`/home/<user>/<user>/.verbinal/exec/inbox`, and every result read 404 and
looked like "not ready yet" rather than a wrong address. The contract built
`<username>/.verbinal/…` exactly as the reference does; the difference is that
the reference's storage layer roots at `/home/` and ours at `/home/<username>/`.
The username-rooted builders are gone rather than fixed — their only remaining
caller was a mistake, and removing them means the compiler refuses the original
line.

### Tools that answered without doing anything

- **Unknown arguments were accepted and ignored.** Every schema says
  `additionalProperties: false` and nothing enforced it. That silence produced
  three separate misreadings in one session: `get_job_status` with an
  `executionId` returned the whole job list as if called with `{}`,
  `set_fits_view` dropped a `tabIndex`, and `create_analysis_notebook` was
  reported as ignoring a `title` it never had.
- **`get_proposal_state` could not find a pending proposal.** It could; it never
  saw the id — queueing answers `proposalId` and the lifecycle tools accepted
  only `id`, so passing back the key you were just handed produced
  `{"id": "", "state": "unknown"}`, identical to a proposal that never existed.
- **`close_active_tab` refused silently.** It is an app-level stub that has
  never been wired to a module. FITS tabs have `close_fits_tab` now.
- **`list_sessions` ignored a `kind` filter** it never had.

### Every generated notebook died on its first run

A Rust `\n\` line continuation strips the newline AND the next line's leading
whitespace, so a template that reads as correctly indented Python was emitted
flush against column 0. Thirteen lines across three stubs. Generated cells are
now compiled by the interpreter in a test, because the literal is exactly what
looked right.

### Pending proposals survive a restart

An app restart destroyed the queue in silence: seven proposals awaiting human
review vanished, and one the user had already approved was voided. The queue is
journalled and rehydrated under its original ids. Resolved proposals are
deliberately not restored — a tombstone stops an id being applied twice and
expires on a TTL.

### VizieR

Two of four mirrors did not exist: `tap.cds.unistra.fr` and
`tapvizier.esac.esa.int` are NXDOMAIN, and they were the first and third
entries, so every cone search opened with two certain failures. The chain then
stopped on a 404 — the one status that least indicates a query problem, since
it means the path is not on that host. Only 400 and 403 are definitive now.
The mirror list is a setting with the shipped list as its default, because
these hostnames have moved before and a constant in the binary left nobody a
way to route around it.

`vizier_cone_search` also takes `columns`. A Gaia DR3 cone is ~230 columns per
row and 500 rows is ~760 KB, past what an agent can hold; omitting `columns`
still sends `SELECT TOP n *`, byte-identical to the reference.

### Agents

- **A slow notebook cell said "UI busy".** `run_cell` awaited the whole
  execution inside the bridge's 30s budget, so any cell slower than that lost by
  construction — about a window that was not blocked. It reports `running: true`
  with whatever output has landed. Timing out the await would have cancelled the
  cell, which is worse than the wrong error.
- **`get_downloaded_observation` reports `localPath` and `fileExists`.** The
  files live under a managed directory no agent could guess, and only the
  basename was exposed — so a session went hunting through Downloads, Documents
  and home for a file it had just fetched.
- **Clients are told when the tool list changes**, once per burst rather than
  once per edit.
- **An apply that failed is no longer recorded as a rejection.** A proposal that
  reached CANFAR and came back 400 was indistinguishable from one a policy
  refused.

### Internal

One `write_atomic` instead of three. Two of them differed in a way that
mattered: one derived its temp path with `with_extension("json.tmp")` — the
same name for every writer — so two saves in flight could rename each other's
half-written file into place.

## [1.3.5] - 2026-08-21

A bug-fix release, and one of the bugs was invisible from inside the app: an
AI client could not use Verbinal at all, and every check we ran said the server
was healthy.

### An agent could not connect, and nothing said why

`check_notebook_dependencies` advertised `inputSchema: {"type": "string"}`. Tool
arguments are a named map, so the schema has to describe an object; this one was
built from the schema for its own `notebook` PROPERTY, passed where the whole
schema goes. Every other notebook tool wraps it correctly.

The tool itself worked — it dispatched, it answered, its tests passed. Only the
catalogue was wrong, and a client that validates tools before registering them
rejects the entry; some reject the entire list over one bad member. That
presents as "this server has no tools" rather than "one tool is malformed",
which is why the connector kept reconnecting with no error logged anywhere.
Every advertised schema is now checked: object type, `properties` a map, and
nothing `required` that the tool never declares.

### Tools that said yes and did nothing

- **`open_fits_file` answered `opened: true` unconditionally.** It dispatched a
  fire-and-forget GTK action, so a path that does not exist, an id that does not
  resolve and a file that will not parse all reported success.
- **`open_cube` did the same**, and `export_search_results` went further: given
  a `vos:` destination it created a LOCAL directory named `vos:`, wrote the CSV
  into it, and returned `"exported": true` with the path the caller recognised.
  Six tools took a local path without checking it — including
  `download_vospace_file`, where the remote `path` and the local `local_path`
  sit side by side with nothing saying which is which. The remedy in the error
  message named `upload_vospace_file`, which has never been a tool.
- **Long applies timed out.** A 332 MB download returned "Request timed out"
  while the transfer carried on unseen. Slow applies now run as background jobs
  and answer immediately with a job id.
- **One slow command starved every other.** The bridge awaited each handler
  inline, so a ninety-second notebook cell made the FITS viewer and ADQL both
  answer "UI busy". Commands now run concurrently on the GTK context.
- **Clients are told when the tool list changes.** Guide tools are user-editable
  and read live, so a name an agent cached at connect could stop existing
  mid-session with no way to find out.

### Notebooks

- **A phantom `SyntaxError` headed every cell traceback.** The harness decided
  how to run a cell by catching rather than asking, so the real error raised
  inside an `except SyntaxError` block and Python chained the two.
- **Missing packages can be installed** from the notebook and by an agent. On
  Ubuntu the system Python is externally managed and `pip install --user` is
  refused by design; the refusal was being thrown away with pip's stderr.
  Overriding it is offered only after pip itself says so, and only on request.

### Dialogs

- **Content was cut off at the right edge.** One suffix label held a full
  sentence, and a `GtkLabel` neither wraps nor ellipsizes by default — its
  minimum width is its text, which propagates up and made the preferences
  dialog demand 784px inside 720px.
- **Buttons went past the bottom edge.** A dialog with no scroller grows to fit
  its content: asked for 560px, measured 2034px, carrying its own action row
  below the bottom of the display. There is one dialog shell now — content
  scrolls, actions do not — and the modal widths are four named roles rather
  than the thirteen numbers they had been.

### Settings

- **Every field shows an example of what belongs in it.** A configuration read
  as complete while `run_code` stayed off, because the full image reference had
  been typed into "Registry repository (project)" and nothing said what a
  project looks like. A repository carrying a tag is now accepted as the
  reference it is, and a readiness row reports what `run_code` will launch.

### For agents

- **The ADQL dialect is documented** where the query is written: CADC TAP is
  ADQL 2.0 with no UDFs, `SELECT TOP n` rather than `LIMIT`,
  `lower(col) LIKE lower(...)` rather than `ILIKE`, and geometry predicates
  compared with `= 1`. Each rule was checked against the live service.

## [1.3.4] - 2026-08-20

A bug-fix release. Everything below was found by using the app against the
live service, and every one was confirmed by measurement or by a live request
rather than by reasoning about the code.

### Image inspection worked for nobody

Four separate defects stacked on the same feature, each hiding the next.

- **The probe wrote its manifest where the app never looks.** Both scripts
  publish to `~/.verbinal/manifests/` and echo only `ok: <path>`. This port
  recovers the manifest from the job's *stdout* and then deletes the job, so
  every inspection reported "job produced no manifest JSON in its logs". Worse,
  every error branch writes a stub manifest whose `probeNotes` says exactly what
  went wrong — to the same unread file. Both scripts now print the manifest on
  every path that publishes one, with status on stderr.
- **The script never reached the container.** Skaha reads a single `args`
  value, and the launch sent one form field per argument — so `bash -c <script>`
  arrived as `bash -c` and died with *"option requires an argument"*. The
  reference uploads the script and passes its path; that half of the port had
  never been finished. The same bug meant a user's `python run.py --fast` ran
  `python run.py`, silently.
- **`mktemp --suffix=` is GNU.** The inspector image is Alpine, so mktemp is
  BusyBox's: it printed usage, the assignment came back empty, and the next line
  died on an empty filename. Both scripts are POSIX now, checked by a deny-list
  and by actually running them under a BusyBox-only PATH.
- **One failed poll killed a healthy job.** A 500 from the CADC identity service
  behind `/ac/search` aborted probes that were running fine, and blamed the job.

### Failures you can now read

- **Batch Jobs kept nothing.** CANFAR reaps finished headless jobs and the
  discovery coordinator deletes its own probes within seconds, so the Completed
  and Failed tiles were structurally always zero. The last 50 finished jobs are
  remembered with the reason each failure failed — captured while the job still
  existed, since its logs die with it — and the tiles count them.
- **"Job ended in failed state: Failed"** was a status word, not a reason. The
  job's logs and events are now read *before* it is deleted.
- **The Batch Jobs modal showed only the tile you clicked**; its other three
  tabs were always empty.

### Search

- **Filters accept boolean expressions**: `!`, `&`, `|` and parentheses over
  CADC's own condition syntax (`a..b`, `>=`, `=`, substring). `!tess & !apass`
  was previously read as one literal string, matched nothing, and — negated —
  kept every row. The syntax is documented in a **?** popover beside the filter
  buttons, not only in a tooltip.
- **A new search no longer inherits the last one's filters.**
- **Paging a large result set took nine seconds.** The column-width helper
  measures a heading with Pango, and the row loop called it once per *cell* —
  1,500 text-shaping runs per page turn. Memoised. Rows also built the
  detail-modal payload for all 41 columns up front, for a modal that usually
  never opens.

### Images and the Portal

- **"Use this image" did nothing** from the images card or from the
  find-by-package dialog opened from it: it activated an app action that was
  never registered, and GIO logs that and carries on.
- **Manifests are shared through your VOSpace.** Every probe has always
  published one there; nothing read it. Signing in now pulls them in the
  background, paced, so a second machine or a reinstall costs no jobs.
- **The find-by-package modal rendered 4,652 checkboxes per keystroke** — a real
  catalogue facets to that many values. Capped, with ticked values always kept.
- **Home is the landing page after sign-in**, not the Portal.
- The Portal's primary column (sessions, launch form) gets two thirds of the
  width instead of half.
- **Session Templates is gone.** Recent Launches is a strict superset of what it
  stored, records automatically, and its list actually refreshed.

## [1.3.3] - 2026-08-14

Parity with the CanfarDesktop 1.3.2 + 1.3.3 (Windows) generation. 1.3.2 was
reliability work; 1.3.3 was MCP full-UI coverage. Both are in.

### Fixed after the parity sweep

Using the app — and driving it with an AI agent — turned up defects the tests
could not see. Each was confirmed against the live service or by reading what
the app actually wrote, not by reasoning about it.

- **The search form could build ADQL the archive refuses.** `GETDATE()` is
  T-SQL and CADC answers *"Function [GETDATE] is not found in TapSchema"*, so
  **Public data only** had never worked — in this app or in the Windows one it
  was ported from. Two more of the same kind: `Plane.dataRelease` is a
  TIMESTAMP and was compared against an MJD number (a 500), and `INTERVAL`
  bounds must be doubles, so any observation-date range 400'd. CADC declares
  ADQL 2.0 with twelve geometry functions and no UDFs; a guard now rejects any
  call outside that set, which is what would have caught `GETDATE` on day one.
- **A folder you created was invisible.** The listing cache had no way to say
  "this is now wrong", so after a create the browser redisplayed the listing
  from before it — and creating the folder again failed as already existing,
  which it was. Delete, rename, share and upload had it too, and Refresh could
  hand back a cached listing. Separately, the node URI in the create request
  omitted the `home/<user>` root the URL carries, which the service rightly
  refused as invalid.
- **Transfers ran blind.** Storage held whole files in memory — a 5 GB cube
  needed 5 GB of RAM, showed nothing until it finished, and could not be
  stopped. Uploads and downloads now stream with a progress strip and a cancel
  button, a download lands in `.tmp` and is renamed only when complete, and a
  failed upload deletes what it left rather than leaving a truncated FITS that
  looks whole.
- **An agent's work was recorded as yours.** The router stamped the
  originating client into the proposal store and then applied a copy taken
  before the stamp, so nothing an agent created ever earned its badge.
- **`run_search` reported success on failure**, hiding the service's
  explanation in an on-screen banner; `run_cell` returned before the cell had
  run. Both now answer with what happened, errors included.
- **Notebooks were not quite notebooks.** Every markdown cell carried
  `outputs` and `execution_count`, and `kernelspec`/`language_info` were
  written as `null` — all three make a file the nbformat schema rejects. Saved
  notebooks now declare the Python 3 kernel they ran under. Save As left the
  tab reading "Untitled" and saved whatever name was typed, so a notebook
  saved as `analysis` had no extension and the Open dialog would not show it.
  Clicking into a cell left no caret until you typed.
- **89 dead-code warnings were suppressed**, and reading them found five real
  defects: the app view snapshot never reported who was signed in, the
  proposal budget's tests covered a copy of the code the router did not run,
  and two reference features had shipped with their halves never called.
  The shipping build now lints with `dead_code` on.

### Added
- **Full MCP tool parity** — the surface now advertises the reference's **137
  tool names**, enforced by a test that diffs the live manifest against them.
  Search gained its 15-tool family over the view-state bridge, so an agent
  drives the same widgets a person does; the FITS and Cube viewers gained every
  control their panels expose.
- **Research bundle: `includeFiles`** — the export can now carry the downloaded
  data files themselves, under `research/files/`. They are STREAMED by a new
  ZIP64-capable writer, so a multi-gigabyte cube is fine; a bundle that needs no
  ZIP64 record still carries none, and opens in any reader.
- **Headless replicas, properly** — one launch per replica, each named `job-N`
  and told which one it is via `REPLICA_ID` / `REPLICA_COUNT`. The launch form's
  Replicas control previously asked for N jobs and produced one.
- **Search**: a per-row preview popover, a pinned results header, per-field
  spectral units (14 of them, not 4), live column filters, date presets that
  fill the visible date field, and a row-limit notice when the answer was
  truncated.
- **Research / CAOM2**: citation fields (proposal id / PI / title / data
  release) in the export the README already told users to cite, a junk-quality
  chip on the plane header, "View on CADC" in the detail header, an inline
  progress bar for artifact downloads, and Sign-in (not Retry) on the
  proprietary-data panel.
- **Workflows**: the VOSpace tier end to end — list, fetch, publish — plus
  editor validation, copy-prompt, clickable tool chips and a current-step
  highlight.

### Fixed
- **Silent wrongness in the query builder**: one-sided spectral coverage asked
  containment where the archive wants overlap (a search above 500 nm excluded a
  400–900 nm observation); an inline unit (`500nm`) dropped the constraint
  entirely; an observation ID matched as a substring.
- **Units and choices decoded by position** — six dropdowns decoded a selection
  against a second list, and MCP enum arguments fell back to entry 0, so
  `timeUnit: "weeks"` searched in SECONDS. An unadvertised value is refused now.
- **Session launch wire format**: flexible allocation omits `cores`/`ram`
  instead of sending zero, an agent's unsized launch gets the reference's 2
  cores / 8 GB rather than 1 and 1, and an unnamed agent session is
  distinguishable rather than "notebook".
- **`get_session`** reads the session's own URL, so a finished headless job is
  no longer reported as never having existed.
- **`fits` is a default feature** — following the README used to produce a
  binary that could not open a FITS file.
- Clearing the research archive now removes its notes; export options
  (`includeNotes`, `includeSearchHistory`, `maxRec`) are read by the names the
  schemas advertise rather than silently ignored.
- **French was only ever the part the reference had.** 621 user-visible strings
  reached a person without French: 52 literals never wrapped at all (whole
  screens — the AI-connect wizard, image discovery, the template manager — and
  fourteen of those already had French sitting unused in the catalog), 81 built
  with `format!`, which cannot consult a catalog, and 488 that were wrapped,
  looked up, and missed, because the catalog is generated from the reference's
  resource files and every screen Verbinal grew past it had no entry. All are
  translated; `HAND_PAIRS` carries 684 pairs, brands and technical tokens
  included, mapped to themselves on purpose.
- Plural forms no longer decide English morphology at the call site — the
  research export said `quer{y|ies}` through an argument no translation can
  undo. `tr_plural!` picks between two whole templates, so each language states
  its own plural.
- The notebook's cells carry the reference's prompt again; a new notebook opened
  on two silent boxes that said nothing about which one was Python.

### Changed
- Test suite: 841 → **1,212**, including invariant tests that walk the live tool
  manifest — every advertised argument is read by something, everything settable
  is readable, and every payload key an applier reads is one a proposer writes —
  and four that walk the source for strings a person reads: nothing reaches a
  label or a toast without the catalog, every localized string has a French
  form, every template has a French pair, and every `tr_fmt!` passes one
  argument per placeholder (which the runtime tolerates on purpose, so nothing
  else could see a dropped one).
- CI lints every target (`--all-targets`) and builds the way the README
  documents; README and CONTRIBUTING quote the gate's exact commands.

## [1.3.1] - 2026-07-07

One-to-one parity with the CanfarDesktop 1.3.0 + 1.3.1 (Windows) generation.

### Added
- **Workflows** — research-protocol module: a `.workflow.md` markdown-checklist
  format with byte-preserving check-off, built-in Canada-first templates, local
  working copies, and a master-detail page with step deep-links.
- **Cube Viewer** — 3D spectral-cube module: a GPU volume ray-marcher
  (`gtk::GLArea` + GLSL 330) with camera orbit/zoom/auto-orbit, colormaps and an
  opacity transfer-function editor, plus a GL-free slice mode (channel scrubber,
  waveform, spectrum probe) that is always available as a fallback, and PNG/PDF
  figure export.
- **AI Assistant (MCP)** — an MCP server lets an AI agent (Claude Desktop /
  Claude Code CLI) drive Verbinal over a private per-user UNIX socket
  (`verbinal mcp` bridge). Read tools + a propose-then-approve write pipeline,
  a guided connect wizard (Claude config merge + self-test), and an **AI Guide**
  for tuning how the agent sees each tool.
- **Editable service endpoints** in Settings for all eight CANFAR/CADC hosts,
  with live-apply and a **Test connections** self-test.
- **FITS Image Info panel**, **projection-aware WCS with SIP distortion**
  (TAN/SIN/STG/ZEA + legacy approximate), **Search Here** (crosshair → search
  form), and mouse-wheel zoom-toward-cursor + Ctrl/Shift pan.
- **VOSpace folder sharing** — make a node public or share it with a group
  (with the 1.3.1 container-tail `setNode` fix).
- **French localization** — a compile-time EN/FR catalog (1271 keys) with a
  Language setting and reverse-lookup string wrapping.

### Changed
- Config now survives upgrades: `AppConfig` carries `#[serde(default)]` so a new
  field can never silently reset existing settings; Reset restores endpoints only.
- North-up rotation uses the `atan2(-Cd1_2, Cd2_2)` convention.

### Fixed
- A mid-session 401 now leads back to sign-in (silent re-auth with a cooldown,
  else a persistent sign-in banner) instead of being swallowed.
- FITS crosshairs hide when they fall outside the image; Go To is bounds-checked.
- Search-at-position lands on the search form, not a stale results tab.

## [1.2.0] and earlier

See git history — Portal, Search, Storage, Notebook, Research, and the initial
FITS viewer.
