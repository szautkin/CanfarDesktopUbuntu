# Portal QA Runbook

Hands-on regression checks for the Portal work in this cycle: notification timing, the
CANFAR Images card, the registry browser, the image detail view, and six new MCP tools.

Figures below are measured against the live CANFAR platform and a real manifest cache, so a
check that disagrees is a finding, not a rounding difference.

| Figure | Value |
| --- | ---: |
| Images returned by `/v1/image` | 365 |
| Launchable — what the card shows | 288 |
| `desktop-app`-only, hidden | 77 |
| Projects in filter row 2 | 21 |
| Manifests cached | 219 |
| Distinct packages known | 6,511 |
| Median packages per image | 624 |
| Unit tests passing | 1,964 |

**Build state:** uncommitted working tree — 49 files changed, 4,858 insertions, 801
deletions, plus 14 new untracked files, on top of `33b39b0`.

---

## 0. Before you start

Two of these matter more than they look: the app is single-instance by application id, so a
stale copy will silently answer your MCP calls instead of the new build.

| # | Do | Expect |
| --- | --- | --- |
| 0.1 | Fully quit any running Verbinal, then start the freshly built binary. | Exactly one `verbinal` process. A second instance never starts and never warns. |
| 0.2 | Sign in, then `ls /run/user/$(id -u)/verbinal-mcp.sock` | Socket present. Without it every check in §7 fails for the wrong reason. |
| 0.3 | Note whether a Harbor CLI secret is saved (Settings → Image Discovery). | Decides whether §3 exercises the saved-credential path or the typed one. Both must work. |

---

## 1. Notification timing

Notifications were only ever a side effect of a poll, so the interval **was** the delay — up
to 45 s for jobs, 15 s for sessions. The cadence now backs off from 5 s to a per-surface
ceiling, and drops to 45 s when nothing is in flight. The risk in this change is the inverse
of the original bug: too much polling, or a poller that multiplied.

| # | Do | Expect |
| --- | --- | --- |
| 1.1 | Launch an interactive session; watch the session strip countdown. | Counts down from 5 s, then 8 s — never above 8 s while anything is pending. |
| 1.2 | Time the gap between the card turning Running and the desktop notification. | Under 8 s. Previously up to 15 s. |
| 1.3 | With nothing pending, leave the Portal idle. | Countdown hidden, but polling continues silently at 45 s. It no longer stops entirely. |
| 1.4 | **Regression** — leave the Portal open 10+ min, then check the Batch Jobs countdown and CPU. | One poller, not several. An erratically jumping countdown means `start_polling`'s guard failed and pollers accumulated. |
| 1.5 | Submit a headless job; watch the Batch Jobs card. | Counts update within ~2 s of submission — the card is refreshed on launch, not at its next scheduled poll. |
| 1.6 | Let that job finish; time the completion notification. | Within 20 s. Previously up to 45 s — and a job that started *and* finished inside one window raised nothing at all. |
| 1.7 | **Regression** — disconnect the network for a minute with the Portal open. | No repeating toast. "Sessions unreachable — cached list" fires only for a refresh *you* asked for; the poller reports through the health tracker instead. |
| 1.8 | **Regression** — watch the session strip and Batch Jobs spinners while idle. | No spinner flashing on its own. Background polls take the quiet path. |
| 1.9 | Trigger several toasts quickly (add/remove two images in the registry browser). | Each replaces the last; no queue of stale messages arriving seconds late. An identical repeat is dropped rather than resetting the clock. |
| 1.10 | Raise a toast with a button (or one that never times out), then trigger a background toast. | The button toast is **not** displaced — it keeps the screen and the newcomer queues behind it. |

---

## 2. CANFAR Images — scope and filters

The card is now scoped to what the launch form can actually offer, and carries a second
filter row for projects. The subtle behaviour is what happens when the two rows disagree.

| # | Do | Expect |
| --- | --- | --- |
| 2.1 | Read the count badge beside "CANFAR Images". | **288**, not 365. The 77 `desktop-app`-only images — every CASA tag back to `3.4.0` — are gone. |
| 2.2 | Look for `images.canfar.net/casa-4/casa:4.2.0`. | Absent. No launch tab offers it, and Inspect on it would spend a probe job for nothing. |
| 2.3 | Check which filter is selected on first load. | **All** — the first button in row 1. *Changed default:* it used to open filtered to one type. |
| 2.4 | With All selected, count row 2. | 21 projects plus All. Alphabetical, and stable between refreshes. |
| 2.5 | Select type **CARTA**; watch row 2. | Narrows to 2 projects. Only projects that still have an image appear — never a button that selects nothing. |
| 2.6 | **Regression** — select project `uvickbos`, then type **CARTA** (that project has no CARTA image). | Project resets to All and CARTA images show. An empty list means `surviving_project` failed, and the cause would be a pressed button in a row that just rebuilt without it. |
| 2.7 | **Regression** — click rapidly between type and project buttons, 15–20 times. | No freeze, no crash. Both rows previously held a `RefCell` borrow across widget rebuilds; a handler reaching the same cell aborts the process rather than panicking. |
| 2.8 | Pick a type *and* a project; read the caption above the list. | "Discovered X of Y" counts rows matching **both** filters, not just the type. |
| 2.9 | Narrow until one project remains. | Row 2 hides itself. "All \| srcnet" is two buttons showing the same list. |
| 2.10 | Press Inspect on an image. | Button becomes a spinner, insensitive, tooltip "Working…". A task with its stage appears in the status bar. |

---

## 3. Registry browser

The second door: images the platform does not publish. The rule that must never break is
that it only talks to the registry when asked — no timer, no search-as-you-type.

| # | Do | Expect |
| --- | --- | --- |
| 3.1 | Open **Add image from registry** from the card header. | Host prefilled (`images.canfar.net`), credentials collapsed, search focused. **No network call yet.** |
| 3.2 | Type `astroml` slowly; do not press Enter or Search. | Nothing happens. Searching on keystroke would enumerate a shared Harbor instance. |
| 3.3 | Press Search (or Enter). | ~16 results incl. `skaha/astroml-cuda`, `skaha/astroml-notebook`, each with session types from registry labels. Spinner on the button; task in the status bar. |
| 3.4 | Clear the field and press Search. | "Type something to search for." — refused, not treated as "everything". |
| 3.5 | Press **Add** on a result. | Button flips to Remove in place without the row shifting sideways. "Your images (1)" appears. Toast confirms. |
| 3.6 | Close the browser; look at the CANFAR Images list. | The added image is there, with a trash button the platform's own images do not have. Count badge +1. |
| 3.7 | Open the launch form's Standard tab; look for it under its type. | Present. One merged catalogue feeds the card, the package search and the launch form. |
| 3.8 | Add an image whose labels name no session type; view the card with All selected. | Visible under All. This is why All exists — a bar that can only select a type has nowhere to show it. |
| 3.9 | Restart the app; re-open the card. | Added images survived, from `~/.local/share/verbinal/user_images.json`. |
| 3.10 | Enter a wrong username/secret under Credentials and search. | "The registry rejected these credentials. Use your Harbor CLI secret, not your CADC password." Your saved secret is untouched — confirm in Settings. |
| 3.11 | Remove the image via the card's trash button. | Row disappears, count drops, toast confirms. Gone from the launch form too. |

---

## 4. Image detail view

Opened by clicking an image row — and only from there. A median image holds 624 packages and
the largest section 960, so these checks are mostly about it staying fast.

| # | Do | Expect |
| --- | --- | --- |
| 4.1 | Click an *inspected* image row (not on a button). | Detail modal opens instantly. Buttons still work independently — clicking Inspect must not open this. |
| 4.2 | Read the header and Environment group. | Image id (selectable), package total, "Inspected N days ago". OS shows the full release string, e.g. `Ubuntu 22.04.4 LTS`. Capabilities and conda envs when present. |
| 4.3 | Look for a Kernel row. | Absent on most images — the probe records `unknown (static layer scan)`, and a row that only ever says that is noise. |
| 4.4 | Check which package sections exist. | Python first, then System · apt / rpm. No "R (0)" or "apk (0)" — both are empty in all 219 manifests. |
| 4.5 | Expand **System · apt** on a large image (~960 packages). | Opens without a stall. 200 chips, then "+760 more — search to narrow". Chips build on expand, not at open. |
| 4.6 | Type `NUMPY` in the filter, in capitals. | Matches. Case is folded inside the search; an earlier version silently returned nothing for a capitalised term. |
| 4.7 | Type a term matching nothing, e.g. `zzzz`. | "No package matches that." All sections hidden rather than each showing zero. |
| 4.8 | Type quickly with a section expanded. | No stutter. Names are pre-lowercased once, not re-folded per keystroke. |
| 4.9 | Open the detail view for an image that **failed** inspection (red icon). | Failure summary, the probe job id, and the job's own output behind a collapsed row. Not a blank dialog. |
| 4.10 | Open the detail view for a never-inspected image (grey icon). | "Not inspected yet" with a pointer to Inspect. It must **not** start a probe on its own. |
| 4.11 | **Regression** — open **Find images by package…** and click rows inside it. | The detail modal never opens from inside that dialog; it shows the manifest inline in an expander. A modal raised from a modal is what froze this app before. |

---

## 5. Portal layout and launch

| # | Do | Expect |
| --- | --- | --- |
| 5.1 | At a wide window, check the three rows. | Platform load / Storage / Batch jobs; Active Sessions full width; CANFAR Images 2⁄3 with Recent Launches 1⁄3. Filter bars scroll horizontally rather than widening the grid. |
| 5.2 | **Regression** — drag the window narrow, past ~1000 px. | Cards stack into one column. No clipping of the right-hand column, and no `GtkOverlay exceeds AdwBreakpointBin width` warning on stderr. |
| 5.3 | Hover the floating launch button, then click it. | Hover reveals Standard / Advanced / Headless; clicking one opens the modal on that form. |
| 5.4 | Press **Launch session** in the Active Sessions card header. | Opens the same modal on the Standard tab. This is the labelled route for anyone who never notices the floating button. |
| 5.5 | **Regression** — open and close the launch modal ten times. | No crash. This previously aborted with `RefCell already borrowed` at close. |
| 5.6 | **Regression** — launch a session; watch where the confirmation appears. | Modal closes; confirmation is in front of the main window, not behind it. |
| 5.7 | Press "Use this image" on an added registry image with no session-type labels. | Launch modal opens on **Advanced** with the full reference prefilled — that tab takes a URI and carries the registry credentials. |
| 5.8 | Click the play button on a Recent Launches item. | Immediate feedback on the clicked button, and a session request actually starts. |

---

## 6. Status bar

| # | Do | Expect |
| --- | --- | --- |
| 6.1 | With nothing running, read the collapsed bar. | "Idle", no moving dots, no "0 failed" badge. |
| 6.2 | Start an Inspect and a registry search together, then expand. | Both listed with stage and elapsed time, newest first. Count reads "2 tasks running". |
| 6.3 | Let something fail; read the collapsed bar. | A red failure count appears. Expanding shows the reason collapsed, full text one click away. |
| 6.4 | Press "Clear finished". | Succeeded and failed entries go; anything still running stays. |

---

## 7. MCP tools

Six new tools. The two discovery ones exist because an agent asked *"which image for spectra
on M51"* would previously search `spectroscopy`, get zero hits **and** zero near-misses, and
report that no image does it — while nine images carry `specutils`.

| # | Do | Expect |
| --- | --- | --- |
| 7.1 | List tools; confirm all six. | `search_packages`, `describe_image`, `search_image_registry`, `list_my_images`, `add_registry_image`, `remove_registry_image`. Total 168. |
| 7.2 | `search_packages` with term `spec`. | Prefix matches lead: `spectres`, `specutils`, `spectral-cube`, `specviz`, `specreduce` — **above** `jsonschema-specifications` (71 images) and `fsspec` (56). A package in 71 images distinguishes nothing. |
| 7.3 | `search_packages` with no term. | The commonest packages overall, and `totalKnown: 6511`. Not an error. |
| 7.4 | `describe_image` on `images.canfar.net/crispasa/mufasa:latest` with `packages: "spec"`. | Ubuntu 22.04.4 LTS, 1398 packages, capabilities `python3, conda`, matches incl. `pyspeckit`, `spectral-cube`, `specutils`. |
| 7.5 | `describe_image` on a failed image, and on a never-inspected one. | `discovered: false` with failure + job id; `discovered: null` with a pointer to `discover_image_packages`. Neither is an error. |
| 7.6 | `search_image_registry` with term `astroml`. | ~16 images with types from labels and an `alreadyAdded` flag per result. |
| 7.7 | `search_image_registry` with a blank term. | Error "term is required" — never a whole-registry enumeration. |
| 7.8 | `add_registry_image`, then `list_my_images`, then `remove_registry_image`. | Add queues a non-destructive proposal; remove queues a **destructive** one needing review. Both reflect in the UI without a restart. |

### Full spectra walkthrough

```jsonc
search_packages           { "term": "spectr", "limit": 6 }
find_images_with_packages { "packages": ["specutils", "astropy"] }   // → 9 images
describe_image            { "image": "images.canfar.net/crispasa/mufasa:latest",
                            "packages": "spec" }
describe_image            { "image": "images.canfar.net/gemini/iraf:0.3",
                            "packages": "spec" }
```

The comparison those two `describe_image` calls support:

```
mufasa:latest   Ubuntu 22.04 · 1398 pkgs · pyspeckit + spectral-cube + specutils + conda
iraf:0.3        Ubuntu 24.04 ·  972 pkgs · specutils only
```

---

## 8. Known limits

Things this plan deliberately does not claim, so nobody chases them as bugs.

- **Tag bulk remains.** 288 images sit behind 174 repositories; `casa-6/casa` alone
  contributes 24 rows. Collapsing tags was not done, because the launch form does not
  collapse them either.
- **Nested repository names are untested against live data.** The double-encoding path has
  unit coverage, but this registry has no repository with more than one slash (0 of 244
  sampled).
- **No science-term mapping.** Nothing translates "spectra" to `specutils`; the agent's own
  knowledge does that, and `search_packages` confirms which guesses exist here.
- **Notification timing is bounded, not instant.** Worst case is 8 s (sessions) and 20 s
  (jobs) — better than the previous 15 s and 45 s, and never worse.
- **Cadence rules are unit-tested; wall-clock behaviour is not.** §1 is the only place that
  gets verified.
