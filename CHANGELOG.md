# Changelog

All notable changes to Verbinal (the native Linux CANFAR Science Portal companion).

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
