# Changelog

All notable changes to Verbinal (the native Linux CANFAR Science Portal companion).

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
