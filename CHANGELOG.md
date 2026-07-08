# Changelog

All notable changes to Verbinal (the native Linux CANFAR Science Portal companion).

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
