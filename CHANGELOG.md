# Changelog

All notable changes to Verbinal (the native Linux CANFAR Science Portal companion).

## [1.3.3] - 2026-08-12

Parity with the CanfarDesktop 1.3.2 + 1.3.3 (Windows) generation. 1.3.2 was
reliability work; 1.3.3 was MCP full-UI coverage. Both are in.

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
- Test suite: 841 → **1,172**, including invariant tests that walk the live tool
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
