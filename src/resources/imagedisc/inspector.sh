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
SYFT_OUT="$(mktemp)"
SYFT_ERR="$(mktemp)"
TRANSFORMER="$(mktemp --suffix=.py)"
cleanup() { rm -f "$SYFT_OUT" "$SYFT_ERR" "$TRANSFORMER" "$TMP"; }
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
}

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
    echo "syft missing; minimal manifest written"
    exit 0
fi

if ! command -v python3 >/dev/null 2>&1; then
    write_minimal "python3 not found in inspector image; cannot transform syft output"
    echo "python3 missing; minimal manifest written"
    exit 0
fi

# ---- Stage the python transformer to a real file. Avoids the
# `python3 - <<'PYEOF'` + pipe collision that fed python its own
# source as stdin (and silently 0-byte'd every manifest).
cat > "$TRANSFORMER" <<'PYEOF'
import json, sys, os, time

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

manifest = {
    "schemaVersion": 3,
    "imageID": target,
    "contentHash": "sha256:syft",
    "capturedAt": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
    "osFamily": (distro.get("name") or "unknown").lower(),
    "osVersion": distro.get("version") or "unknown",
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
"$SYFT" "registry:$TARGET_IMAGE" -o syft-json >"$SYFT_OUT" 2>"$SYFT_ERR" || syft_rc=$?

if [ "$syft_rc" -ne 0 ]; then
    # Truncate stderr to ~400 chars so the manifest stays small.
    snippet="$(head -c 400 "$SYFT_ERR" | tr '\n' ' ' | tr -d '\r')"
    write_minimal "syft failed (rc=$syft_rc): $snippet"
    echo "syft failed (rc=$syft_rc); minimal manifest written"
    exit 0
fi

# Run the transformer â€” stdin = syft's JSON, stdout = manifest.
py_rc=0
python3 "$TRANSFORMER" < "$SYFT_OUT" > "$TMP" 2>>"$SYFT_ERR" || py_rc=$?

if [ "$py_rc" -ne 0 ] || [ ! -s "$TMP" ]; then
    snippet="$(head -c 400 "$SYFT_ERR" | tr '\n' ' ' | tr -d '\r')"
    write_minimal "transformer failed (rc=$py_rc): $snippet"
    echo "transformer failed (rc=$py_rc); minimal manifest written"
    exit 0
fi

mv "$TMP" "$OUT"
echo "ok: $OUT"
