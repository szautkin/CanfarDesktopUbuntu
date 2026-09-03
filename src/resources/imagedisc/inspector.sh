#!/usr/bin/env bash
# verbinal-image-inspector v2
# Runs inside a known-good headless container to inspect a
# *different* target image whose own type doesn't allow
# headless launch (notebook/desktop/carta/firefly/contributed).
set -u
set -o pipefail

USER_HOME="${HOME:-/arc/home/$(whoami)}"
: "${TARGET_IMAGE:?TARGET_IMAGE env var must be set by the launcher}"

OUT_DIR="$USER_HOME/.verbinal/manifests"
mkdir -p "$OUT_DIR"

SAFE_ID=$(printf '%s' "$TARGET_IMAGE" | tr '/:?*<>|"\\' '_')
OUT="$OUT_DIR/$SAFE_ID.json"
TMP="$OUT.partial"
# Temp files, portably.
#
# `mktemp --suffix=` is a GNU coreutils extension. The inspector image is
# Alpine, where mktemp is BusyBox's and takes only [-dqtup] and a TEMPLATE — it
# printed a usage message, the assignment came back empty, and the very next
# `cat > "$TRANSFORMER"` died on an empty filename. An explicit TEMPLATE is
# understood by both, and python3 does not care what a script it is handed by
# path is called.
#
# This script runs inside whatever image the user points it at, so every
# utility it uses has to be the POSIX one, not the GNU one.
new_temp() {
    mktemp "${TMPDIR:-/tmp}/verbinal-$1-XXXXXX" 2>/dev/null
}
SYFT_OUT="$(new_temp syft-out)"
SYFT_ERR="$(new_temp syft-err)"
TRANSFORMER="$(new_temp transform)"

# ---- Where syft unpacks the target image.
#
# syft pulls EVERY LAYER of the target and unpacks it to disk before it can
# catalogue anything, so the scratch it needs is the size of the image. Left on
# the default /tmp that lands on the pod's overlay filesystem — measured at 54 GB
# free and shared with every other pod on the node — and the kubelet kills the
# container for exceeding its ephemeral storage.
#
# That kill comes from OUTSIDE, which is why it looks like nothing: none of the
# error branches below get to run, so there is no stub manifest, and Skaha
# reports neither logs nor a termination event. The caller is left with "job
# ended in failed state: Failed" and no way to learn more. Two images failed
# exactly that way on every attempt, unchanged by going from 1 GB to 8 GB of
# RAM — because memory was never the resource they were short of.
#
# Skaha mounts /scratch as real per-session disk (6.2 TB free where this was
# measured) and wipes it when the session ends.
# `-d /scratch` FIRST, and it is not a formality: `mkdir -p` would happily
# create /scratch itself on a host that has no such mount — as a plain directory
# in the container's own writable layer, which IS the pod's ephemeral storage.
# That is the exact resource this block exists to stay off, so without the test
# the "fix" quietly becomes the bug, and does it while looking like it worked.
SYFT_TMPDIR=""
if [ -d /scratch ] && mkdir -p "/scratch/verbinal-syft.$$" 2>/dev/null; then
    SYFT_TMPDIR="/scratch/verbinal-syft.$$"
fi
# Say which it picked. The fallback is silent by design — an inspector host
# without /scratch must still work — and a silent fallback is indistinguishable
# from the bug it was meant to fix: syft back on the pod's ephemeral storage,
# the container evicted, and no logs left to say so. This line is the only way
# to tell the two apart from the outside.
if [ -n "$SYFT_TMPDIR" ]; then
    echo "syft scratch: $SYFT_TMPDIR" >&2
else
    echo "syft scratch: ${TMPDIR:-/tmp} (no writable /scratch — syft will unpack onto the pod's ephemeral storage)" >&2
fi

cleanup() {
    rm -f "$SYFT_OUT" "$SYFT_ERR" "$TRANSFORMER" "$TMP"
    # An `if`, not `[ … ] && …`: a trailing test that fails is the trap's exit
    # status, and this trap runs on the success path too.
    if [ -n "$SYFT_TMPDIR" ]; then
        rm -rf "$SYFT_TMPDIR"
    fi
}
trap cleanup EXIT

# Helper: write a minimal manifest with a `probeNotes` field set
# to the supplied reason and atomically swap into place. Used by
# every error branch so the caller always sees structured data.
write_minimal() {
    local reason="$1"
    local now
    now="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    # Escape backslashes and double-quotes for JSON safety.
    reason="${reason//\\/\\\\}"
    reason="${reason//\"/\\\"}"
    cat > "$TMP" <<MINIMAL
{"schemaVersion":3,"imageID":"$TARGET_IMAGE","contentHash":"sha256:syft","capturedAt":"$now","osFamily":"unknown","osVersion":"unknown","osRelease":"unknown","kernel":"unknown","dpkgPackages":[],"rpmPackages":[],"apkPackages":[],"pythonPackages":[],"rPackages":[],"condaEnvs":[],"capabilities":[],"pythonVersion":"unknown","shells":[],"probeNotes":"$reason"}
MINIMAL
    mv "$TMP" "$OUT"
    # On stdout too, or the reason this probe gave up never leaves the
    # container: the app reads the manifest from the job logs, and every
    # `probeNotes` explanation was being written to a file and thrown away.
    cat "$OUT"
}

# A temp file we could not create is not survivable, and the failure mode is
# obscure: `> ""` reports "No such file or directory" against a line number,
# naming neither the variable nor the reason.
if [ -z "$SYFT_OUT" ] || [ -z "$SYFT_ERR" ] || [ -z "$TRANSFORMER" ]; then
    write_minimal "mktemp failed in the inspector image; cannot stage temporary files"
    echo "mktemp failed; minimal manifest written" >&2
    exit 0
fi

# ---- Install syft (binary, ~80MB) into ~/.local/bin if missing.
SYFT="$(command -v syft || true)"
if [ -z "$SYFT" ]; then
    mkdir -p "$USER_HOME/.local/bin"
    if command -v curl >/dev/null 2>&1; then
        curl -sSfL https://raw.githubusercontent.com/anchore/syft/main/install.sh \
          | sh -s -- -b "$USER_HOME/.local/bin" >&2 2>&1 || true
    elif command -v wget >/dev/null 2>&1; then
        wget -qO- https://raw.githubusercontent.com/anchore/syft/main/install.sh \
          | sh -s -- -b "$USER_HOME/.local/bin" >&2 2>&1 || true
    fi
    SYFT="$USER_HOME/.local/bin/syft"
fi
if [ ! -x "$SYFT" ]; then
    write_minimal "syft installation failed; inspector image lacks curl/wget or has no network egress"
    echo "syft missing; minimal manifest written" >&2
    exit 0
fi

if ! command -v python3 >/dev/null 2>&1; then
    write_minimal "python3 not found in inspector image; cannot transform syft output"
    echo "python3 missing; minimal manifest written" >&2
    exit 0
fi

# ---- Stage the python transformer to a real file. Avoids the
# `python3 - <<'PYEOF'` + pipe collision that fed python its own
# source as stdin (and silently 0-byte'd every manifest).
cat > "$TRANSFORMER" <<'PYEOF'
import json, sys, os, time, hashlib

target = os.environ["TARGET_IMAGE"]

try:
    raw = sys.stdin.read()
    if not raw.strip():
        print(json.dumps({
            "schemaVersion": 3, "imageID": target,
            "contentHash": "sha256:syft",
            "capturedAt": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
            "osFamily": "unknown", "osVersion": "unknown", "osRelease": "unknown",
            "kernel": "unknown",
            "dpkgPackages": [], "rpmPackages": [], "apkPackages": [],
            "pythonPackages": [], "rPackages": [], "condaEnvs": [],
            "capabilities": [],
            "pythonVersion": "unknown", "shells": [],
            "probeNotes": "syft produced no output"
        }, separators=(",", ":")))
        sys.exit(0)
    sbom = json.loads(raw)
except Exception as e:
    print(json.dumps({
        "schemaVersion": 3, "imageID": target,
        "contentHash": "sha256:syft",
        "capturedAt": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "osFamily": "unknown", "osVersion": "unknown", "osRelease": "unknown",
        "kernel": "unknown",
        "dpkgPackages": [], "rpmPackages": [], "apkPackages": [],
        "pythonPackages": [], "rPackages": [], "condaEnvs": [],
        "capabilities": [],
        "pythonVersion": "unknown", "shells": [],
        "probeNotes": f"syft output unreadable: {e}"
    }, separators=(",", ":")))
    sys.exit(0)

artifacts = sbom.get("artifacts", []) or []
distro = sbom.get("distro", {}) or {}

dpkg, rpm_pkgs, apk_pkgs, py_pkgs, r_pkgs = [], [], [], [], []
conda_env_packages = {}

def env_for(locations):
    for loc in locations:
        path = loc.get("path", "") or ""
        if "/conda/envs/" in path:
            return path.split("/conda/envs/")[1].split("/")[0]
        if path.startswith("/opt/conda/") or "/conda-meta/" in path:
            return "base"
    return ""

for a in artifacts:
    name = a.get("name") or ""
    version = a.get("version") or ""
    if not name:
        continue
    typ = (a.get("type") or "").lower()
    if typ in ("deb", "dpkg"):
        dpkg.append({"name": name, "version": version})
    elif typ == "rpm":
        rpm_pkgs.append({"name": name, "version": version})
    elif typ in ("apk", "alpine-apk"):
        apk_pkgs.append({"name": name, "version": version})
    elif typ in ("python", "wheel", "egg-info", "python-package"):
        env = env_for(a.get("locations", []) or [])
        py_pkgs.append({
            "name": name, "version": version,
            "source": "conda" if env else "pip",
            "env": env or "system"
        })
        if env:
            conda_env_packages.setdefault(env, []).append({
                "name": name, "version": version,
                "source": "conda", "env": env
            })
    elif typ in ("r-package", "r"):
        r_pkgs.append({"name": name, "version": version})

conda_envs = [
    {"name": env, "prefix": "/opt/conda" if env == "base" else f"/opt/conda/envs/{env}",
     "packages": pkgs}
    for env, pkgs in sorted(conda_env_packages.items())
]

notes = None
if not (dpkg or rpm_pkgs or apk_pkgs or py_pkgs or r_pkgs):
    notes = "syft scan returned no recognisable packages"

# Inspector-path capabilities are inferred from syft's
# package list â€” we can't run python imports against a
# non-running target. The detections below are
# version-aware where it matters (photutils-iterative-psf
# needs 1.13+), name-aware where it doesn't (fitsio's
# presence implies importability since it ships as a wheel
# with bundled cfitsio). Misses some behavioural truths
# (does python3 actually start? is the GPU runtime
# wired up?) that only an in-target probe can answer; the
# in-target probe path detects those when scheduled.
capabilities = []
py_names = {p["name"].lower() for p in py_pkgs}
if "fitsio" in py_names:
    capabilities.append("fitsio")
if "photutils" in py_names:
    ver = next((p["version"] for p in py_pkgs
                if p["name"].lower() == "photutils"), "")
    try:
        major, minor, *_ = (int(x) for x in ver.split(".")[:2])
        if (major, minor) >= (1, 13):
            capabilities.append("photutils-iterative-psf")
    except Exception:
        pass
if py_pkgs:
    capabilities.append("python3")
if conda_envs:
    capabilities.append("conda")
if r_pkgs:
    capabilities.append("rscript")

# Interpreter version. Syft surfaces the `python` /
# `python3` interpreter as a `binary` artifact in many
# images; we try both names and fall back to extracting
# from a pip's "python" library entry. "unknown" when none
# of these signals are present (e.g. images with conda
# envs but no system `python3` symlink).
python_version_str = "unknown"
for a in artifacts:
    name = (a.get("name") or "").lower()
    if name in ("python", "python3"):
        v = a.get("version") or ""
        if v:
            python_version_str = v
            break

# osRelease: prefer syft's prettyName (Ubuntu 22.04.3 LTS),
# fall back to composing name + version, then to "unknown".
pretty = distro.get("prettyName") or ""
if not pretty:
    name = distro.get("name") or ""
    version = distro.get("version") or ""
    if name and version:
        pretty = f"{name} {version}".strip()
os_release_str = pretty or "unknown"

# Shell list â€” best-effort name-match against the dpkg
# / rpm / apk catalogue. Inspector path doesn't see the
# filesystem, so we can't verify the binary actually
# exists, but the package presence is a reliable proxy
# for "installed at image-build time."
shell_names_observed = []
pkg_name_set = (
    {p["name"].lower() for p in dpkg}
    | {p["name"].lower() for p in rpm_pkgs}
    | {p["name"].lower() for p in apk_pkgs}
)
for shell in ("bash", "zsh", "sh", "dash", "fish", "ksh"):
    if shell in pkg_name_set:
        shell_names_observed.append(shell)
shells_list = sorted(set(shell_names_observed))

fingerprint = json.dumps(
    {
        "packages": sorted(
            f"{p.get('name','')}@{p.get('version','')}"
            for p in (dpkg + rpm_pkgs + apk_pkgs + py_pkgs + r_pkgs)
        ),
        "distro": [
            distro.get("id") or "",
            distro.get("versionID") or "",
        ],
    },
    separators=(",", ":"),
    sort_keys=True,
)
content_hash = "sha256:" + hashlib.sha256(fingerprint.encode("utf-8")).hexdigest()

manifest = {
    "schemaVersion": 3,
    "imageID": target,
    # A digest of what was FOUND, not of the scan that found it.
    #
    # This was the literal string "sha256:syft" — a marker, so a
    # re-inspection could never answer "did anything change?". Hashing
    # syft's raw output would not work either: it carries a timestamp and
    # a scan descriptor, so two scans of an unchanged image would disagree.
    # The sorted package set plus the distro is stable across runs and
    # changes exactly when the image does.
    #
    # Comparable within this path, not across paths: the in-container probe
    # hashes marker FILES. An image is inspected by the same strategy every
    # time (the strategy follows the image's declared types), so that is
    # enough for the question being asked.
    "contentHash": content_hash,
    "capturedAt": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
    # `id` and `versionID`, not `name` and `version`: syft mirrors os-release
    # field for field, and the in-container probe reads ID / VERSION_ID. Using
    # the other pair made the same image come out as "alpine linux" / "24.04.4
    # LTS (Noble Numbat)" from this path and "alpine" / "24.04" from the other,
    # so the discovery facets listed both and a filter on either missed half
    # the images.
    "osFamily": (distro.get("id") or distro.get("name") or "unknown").lower(),
    "osVersion": distro.get("versionID") or distro.get("version") or "unknown",
    "osRelease": os_release_str,
    "kernel": "unknown (static layer scan)",
    "dpkgPackages": dpkg,
    "rpmPackages": rpm_pkgs,
    "apkPackages": apk_pkgs,
    "pythonPackages": py_pkgs,
    "rPackages": r_pkgs,
    "condaEnvs": conda_envs,
    "capabilities": capabilities,
    "pythonVersion": python_version_str,
    "shells": shells_list,
}
if notes:
    manifest["probeNotes"] = notes

print(json.dumps(manifest, separators=(",", ":")))
PYEOF

# ---- Run syft against the target image. We capture stdout +
# stderr to disk so a failure produces a useful manifest rather
# than a silent 0-byte file. `set -o pipefail` makes syft
# failures abort the pipeline below.
syft_rc=0
# TMPDIR only for syft: the small temp files above are already staged, and this
# is the one command whose scratch is measured in gigabytes. An unavailable
# /scratch leaves SYFT_TMPDIR empty and the default applies, which is exactly
# the old behaviour.
if [ -n "$SYFT_TMPDIR" ]; then
    TMPDIR="$SYFT_TMPDIR" "$SYFT" "registry:$TARGET_IMAGE" -o syft-json \
        >"$SYFT_OUT" 2>"$SYFT_ERR" || syft_rc=$?
else
    "$SYFT" "registry:$TARGET_IMAGE" -o syft-json >"$SYFT_OUT" 2>"$SYFT_ERR" || syft_rc=$?
fi

if [ "$syft_rc" -ne 0 ]; then
    # Truncate stderr to ~400 chars so the manifest stays small.
    snippet="$(head -c 400 "$SYFT_ERR" | tr '\n' ' ' | tr -d '\r')"
    write_minimal "syft failed (rc=$syft_rc): $snippet"
    echo "syft failed (rc=$syft_rc); minimal manifest written" >&2
    exit 0
fi

# Run the transformer â€” stdin = syft's JSON, stdout = manifest.
py_rc=0
python3 "$TRANSFORMER" < "$SYFT_OUT" > "$TMP" 2>>"$SYFT_ERR" || py_rc=$?

if [ "$py_rc" -ne 0 ] || [ ! -s "$TMP" ]; then
    snippet="$(head -c 400 "$SYFT_ERR" | tr '\n' ' ' | tr -d '\r')"
    write_minimal "transformer failed (rc=$py_rc): $snippet"
    echo "transformer failed (rc=$py_rc); minimal manifest written" >&2
    exit 0
fi

# Atomic publish, then emit the manifest on STDOUT.
#
# The file is for anything running inside the container. The STDOUT copy is
# what the app actually reads: this Linux port recovers the manifest from the
# job's logs rather than round-tripping it through VOSpace, so a manifest that
# only ever lands in a file inside a container the app then deletes is a
# manifest nobody sees. Status goes to stderr so stdout stays parseable.
mv "$TMP" "$OUT"
echo "ok: $OUT" >&2
cat "$OUT"
