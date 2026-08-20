# The image-inspection cache: what exists, what is missing, what to do

_Planned 2026-08-20 · read against `JsonManifestStore`, `ImageDiscoveryCoordinator`, both probe
scripts, and the Windows and macOS references._

## Short answers

| Question | Answer |
|---|---|
| Is there a cache of inspected images? | Yes, **local only** — one JSON per image under `<data_dir>/ImageManifests/`. |
| Is it in VOSpace too? | The **probe scripts already write one there**. Nothing in this app has ever read it. Both references do. |
| Is it keyed by the image's hash? | **No** — by the image id string. Neither reference keys by hash either. The manifest carries a `contentHash`, but our parser drops it, and it cannot be a key (see §4). |

The gap that matters is the second row, and it is larger than it looks — see §2.

## 1. What the local cache is today

`JsonManifestStore` writes `<data_dir>/ImageManifests/<sanitized-id>.json`, one file per image,
holding the whole `LastOutcome`: either a parsed manifest or a typed failure (category, message,
job id) with a timestamp. It is mirrored in memory, hydrated lazily, written atomically.

`discover_image(image_id, force)`:

* a cached **success** short-circuits — no job is launched;
* a cached **failure** does not — every call re-probes;
* `force` skips the check entirely.

**There is no expiry.** Once an image is inspected successfully it is never inspected again unless
someone presses Inspect. For an immutable tag that is correct and cheap. For `:latest`, or for a
`1.0` that was re-pushed, the app will serve a manifest describing an image that no longer exists,
indefinitely and silently. That is the real question behind "should we key by sha", and §4 is
about it.

## 2. There is already a VOSpace copy, and we throw it away

Both scripts end by writing `$HOME/.verbinal/manifests/<safe-id>.json`. On CANFAR `$HOME` is
`/arc/home/<user>` — arc storage. So every probe that has ever run has left a manifest in the
user's VOSpace, and this app has never once looked.

Both references look:

* **Windows** — `FetchManifestIfPresentAsync` runs *before* launching a job (`if (!force)`), and
  again after a poll failure, described in its own comment as "the job may have written the
  manifest just after our last poll".
* **macOS** — the same pre-launch check, plus a *grace task*: after the foreground timeout gives
  up, a detached task keeps re-checking VOSpace every 30 s for up to 10 minutes, so a probe that
  outran its budget still populates the cache.

Both validate what they find: parse, reject if `imageID` does not match the image asked for
("wrote-for-another-image guard"), reject stub manifests.

What this buys, none of which the local cache can:

* **A probe that finished after we stopped watching still counts.** Today a timeout is a total
  loss: we mark a failure, delete the job, and the manifest the job wrote sits unread.
* **A second machine, a reinstall, or a colleague's earlier probe costs zero jobs.** The local
  cache is per-install; VOSpace is per-user and durable.
* **It is the only durable copy.** We recover the manifest from the job's stdout and then delete
  the job. Once that call returns, the logs are gone. The file is not.

This is the single highest-value missing piece and it needs no new storage, no new format, and no
new writes — only a read of something already being written.

### A trap to fix first

The reader must derive the filename exactly as the writer did, and today two sanitisers disagree:

| | maps to `_` |
|---|---|
| `manifest_store::sanitize_image_id` (Rust) | `/ \ : @ ? * < > \| "` and whitespace/control |
| `SAFE_ID` (both scripts) | `/ : ? * < > \| " \` |

They agree on every ordinary id and diverge on `@` — exactly the character in a digest-pinned
reference (`image@sha256:…`), which is what §4 would introduce. One definition, used by the Rust
reader and asserted against the shell's `tr` set by a guard.

## 3. `contentHash` is computed, then dropped

`probe.sh` computes a real SHA-256 over stable marker files (`/etc/os-release`,
`/var/lib/dpkg/status`, …). The Windows model keeps it: `ImageManifest.ContentHash`, defaulting to
`"sha256:none"`. **Our `ImageManifest` has no such field**, so `manifest_parser` discards it.

Two consequences:

* A re-inspection cannot say "nothing changed" — which is the honest and cheap answer most of the
  time, and the one that makes a Refresh button feel safe to press.
* `inspector.sh` writes the literal string `"sha256:syft"` — a marker, not a hash. So even with
  the field restored the two paths would not produce comparable values. It should hash syft's own
  output.

## 4. Why the cache is not keyed by a hash, and what would actually help

`contentHash` cannot be a cache key: it is computed *inside* the image, by the probe. To learn it
you must run the job the cache exists to avoid. It is a change-detector, not a key.

The only hash available *before* probing is the **registry digest** — Harbor answers
`HEAD /v2/<project>/<repo>/manifests/<tag>` with a `Docker-Content-Digest` header, cheaply and
without pulling anything. That is what makes a re-pushed tag detectable.

We do not talk to the registry at all today: `RawImage` is `{ id, types }` from Skaha, and there is
no registry client. Adding one means a new auth surface (the same Harbor credentials the launch
already carries) and a new dependency on the registry being reachable.

So the choice for staleness is:

| Option | Correct for a re-pushed tag | Cost |
|---|---|---|
| **A. Age-based TTL** | No — only by luck | Trivial. Wrong for immutable tags: re-probes what cannot have changed. |
| **B. Registry digest** | Yes | A Harbor client, its auth, and its failure modes. |
| **C. Show the age; let the user decide** | n/a | Nothing. Already have `force`. |

**Recommendation: C now, B later, never A.** Displaying "inspected 3 weeks ago" next to a Refresh
turns an invisible staleness problem into a visible one for the price of a label, and it is honest
in a way a TTL is not. B is the real fix and deserves its own change, with the digest stored
alongside the manifest and compared on open.

## 5. Plan

**Phase 1 (S) — read the VOSpace manifest.** The whole of §2.

1. One shared image-id sanitiser, with a guard that it matches the scripts' `tr` set.
2. `fetch_manifest_if_present(image_id)` — download, parse, reject on id mismatch or stub, exactly
   as both references do.
3. Call it before launching when not forced; short-circuit on a hit.
4. Call it again when polling fails or times out, before recording the failure.
5. Guards: the pre-launch check must run before the launch; a mismatched or stub manifest must be
   rejected rather than cached.

**Phase 2 (S) — keep `contentHash`.** Add `content_hash` to `ImageManifest` (default
`sha256:none`, as the reference), parse it, show it in the detail pane, and make `inspector.sh`
hash syft's output instead of writing a marker. Then a re-inspection can report "unchanged".

**Phase 3 (S) — make age visible.** "Inspected {time_ago}" on the image row and in the detail
pane, beside the existing Inspect/Refresh. `time_ago` and the timestamp already exist; only the
label is missing.

**Phase 4 (M, separate) — the grace task.** macOS's detached re-check of VOSpace after the
foreground timeout. Worth doing once Phase 1 exists, since it is the same fetch on a timer, and it
converts the most expensive failure mode (a slow probe, marked failed, job deleted) into a success.

**Phase 5 (L, its own decision) — registry digests.** Option B above.

## 6. Risks

| Risk | Why it is contained |
|---|---|
| A stale VOSpace manifest is trusted | Same guards both references use: id must match, stubs rejected. Phase 3 makes its age visible; `force` always re-probes. |
| The reader and the scripts disagree on a filename | Phase 1 item 1 exists for exactly this, and it is guarded rather than assumed. |
| A VOSpace read on every Inspect adds latency | It replaces a headless job. A download that fails simply falls through to launching, as both references do. |
| Reading someone else's manifest | The path is under the user's own home; there is no cross-user read. Sharing between users is not in scope. |
