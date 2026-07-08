#!/usr/bin/env bash
# verbinal-image-probe v1
# Runs inside a Skaha container; emits a JSON manifest of installed
# software to $HOME/.verbinal/manifests/<sanitized-image-id>.json
#
# Required env: IMAGE_ID  (full registry-qualified id from Skaha)
set -u

USER_HOME="${HOME:-/arc/home/$(whoami)}"
: "${IMAGE_ID:?IMAGE_ID env var must be set by the launcher}"

OUT_DIR="$USER_HOME/.verbinal/manifests"
mkdir -p "$OUT_DIR"

# Filesystem-safe form of the image id: replace / : ? * < > | " with _
SAFE_ID=$(printf '%s' "$IMAGE_ID" | tr '/:?*<>|"\\' '_')
OUT="$OUT_DIR/$SAFE_ID.json"
TMP="$OUT.partial"

# Stage raw outputs in a tempdir; python below assembles JSON.
STAGE=$(mktemp -d)
cleanup() { rm -rf "$STAGE"; }
trap cleanup EXIT

uname -srm > "$STAGE/kernel" 2>/dev/null || true
[ -r /etc/os-release ] && cp /etc/os-release "$STAGE/os-release" || true

# Python 3 interpreter version. `python3 -V` writes the
# version to stderr or stdout depending on Python's version
# (pre-3.4: stderr; 3.4+: stdout) â€” capture both. We parse
# this in the aggregator into `pythonVersion`, the field
# the 2026-05-15 QA report explicitly named ("a
# cirada/cutout_core_interactive:latest image shipping
# Python 3.6.9 cost one job and ~10 minutes â€” surfacing
# the version pre-launch lets agents pick a compatible
# image up-front").
if command -v python3 >/dev/null 2>&1; then
    python3 -V > "$STAGE/python-version.txt" 2>&1 || true
fi

# Interactive shells on PATH / /bin. Used by agents to
# decide whether `cmd: "bash"` is viable. Test the common
# ones in order of falling popularity; minimal images may
# only ship `/bin/sh`. Each line is one shell name; the
# aggregator dedupes and sorts.
: > "$STAGE/shells.txt"
for sh in bash zsh sh dash fish ksh; do
    if [ -x "/bin/$sh" ] || [ -x "/usr/bin/$sh" ] || command -v "$sh" >/dev/null 2>&1; then
        echo "$sh" >> "$STAGE/shells.txt"
    fi
done

# System package managers
if command -v dpkg-query >/dev/null 2>&1; then
    dpkg-query -W -f='${Package}|${Version}\n' > "$STAGE/dpkg" 2>/dev/null || true
fi
if command -v rpm >/dev/null 2>&1; then
    rpm -qa --qf '%{NAME}|%{VERSION}-%{RELEASE}\n' > "$STAGE/rpm" 2>/dev/null || true
fi
if command -v apk >/dev/null 2>&1; then
    # apk info -v emits "name-1.2.3-r0"; convert to name|version
    apk info -v 2>/dev/null \
      | sed -E 's/^([^[:space:]]+)-([0-9].*)$/\1|\2/' \
      > "$STAGE/apk" || true
fi

# Conda environments
CONDA=""
for c in /opt/conda/bin/conda /usr/local/bin/conda /usr/bin/conda conda; do
    if command -v "$c" >/dev/null 2>&1; then CONDA="$c"; break; fi
done
if [ -n "$CONDA" ]; then
    # `conda env list --json` is well-defined; fall back to text otherwise.
    "$CONDA" env list --json 2>/dev/null > "$STAGE/conda-envs.json" || true
    if [ -s "$STAGE/conda-envs.json" ]; then
        # Per-env package list. Each env writes to conda-pkgs-<safe-name>.txt
        # using the same name|version format the python assembler expects.
        python3 -c '
import json, sys, os, subprocess
d = json.load(open(os.path.join(os.environ["STAGE"], "conda-envs.json")))
for env_path in d.get("envs", []):
    # env name = basename, except the root which we label "base"
    name = os.path.basename(env_path)
    if env_path.endswith("/conda") or env_path == "/opt/conda":
        name = "base"
    try:
        r = subprocess.run([os.environ["CONDA"], "list", "--prefix", env_path, "--export"],
                           capture_output=True, text=True, timeout=60)
        lines = []
        for ln in r.stdout.splitlines():
            ln = ln.strip()
            if not ln or ln.startswith("#"): continue
            # `--export` format is name=version=build; keep name=version
            parts = ln.split("=")
            if len(parts) >= 2:
                lines.append(f"{parts[0]}|{parts[1]}")
        with open(os.path.join(os.environ["STAGE"], f"conda-pkgs-{name}.txt"), "w") as f:
            f.write("\n".join(lines))
        with open(os.path.join(os.environ["STAGE"], f"conda-meta-{name}.txt"), "w") as f:
            f.write(env_path)
    except Exception:
        pass
' 2>/dev/null || true
    fi
fi

# System python (if no conda env labeled "base", otherwise covered by conda block)
if [ -z "$CONDA" ]; then
    for py in python3 python; do
        if command -v "$py" >/dev/null 2>&1; then
            "$py" -m pip list --format=freeze 2>/dev/null \
              | sed -E 's/^([^=]+)==(.*)$/\1|\2/' \
              > "$STAGE/python-system.txt" || true
            break
        fi
    done
fi

# R packages
if command -v Rscript >/dev/null 2>&1; then
    Rscript -e 'inv <- installed.packages()[, c("Package","Version")]; \
                if (length(inv) > 0) cat(apply(matrix(inv, ncol=2, byrow=FALSE), 1, \
                function(x) paste(x[1], x[2], sep="|")), sep="\n")' \
            2>/dev/null > "$STAGE/r.txt" || true
fi

# Behavioural capability flags. One line per detected
# capability written to a stage file the Python aggregator
# picks up. Each test is best-effort: silent failure is fine,
# the absence of the key in the manifest's `capabilities` is
# the signal "not detected". Tests are deliberately strict â€”
# we want false negatives over false positives so an agent
# that filters on `fitsio` can trust the hit.
: > "$STAGE/capabilities.txt"
command -v python3 >/dev/null 2>&1 && echo "python3" >> "$STAGE/capabilities.txt"
command -v conda   >/dev/null 2>&1 && echo "conda"   >> "$STAGE/capabilities.txt"
command -v Rscript >/dev/null 2>&1 && echo "rscript" >> "$STAGE/capabilities.txt"
command -v nvidia-smi >/dev/null 2>&1 && \
    nvidia-smi --version >/dev/null 2>&1 && echo "gpu" >> "$STAGE/capabilities.txt"
if command -v python3 >/dev/null 2>&1; then
    python3 -c "import fitsio" 2>/dev/null && echo "fitsio" >> "$STAGE/capabilities.txt"
    python3 -c "from photutils.psf import IterativePSFPhotometry" 2>/dev/null \
        && echo "photutils-iterative-psf" >> "$STAGE/capabilities.txt"
fi

# Content fingerprint over stable marker files
HASH_INPUT=""
for f in /etc/os-release /var/lib/dpkg/status /etc/redhat-release /etc/alpine-release; do
    [ -r "$f" ] && HASH_INPUT="$HASH_INPUT $f"
done
if [ -n "$HASH_INPUT" ] && command -v sha256sum >/dev/null 2>&1; then
    CONTENT_HASH="sha256:$(cat $HASH_INPUT 2>/dev/null | sha256sum | awk '{print $1}')"
elif [ -n "$HASH_INPUT" ] && command -v shasum >/dev/null 2>&1; then
    CONTENT_HASH="sha256:$(cat $HASH_INPUT 2>/dev/null | shasum -a 256 | awk '{print $1}')"
else
    CONTENT_HASH="sha256:none"
fi

export STAGE CONDA IMAGE_ID CONTENT_HASH

# Assemble JSON. If python3 is missing the entire probe loses; emit
# a minimal error manifest so the polling client sees a result, not
# a hung job.
if ! command -v python3 >/dev/null 2>&1; then
    cat > "$TMP" <<MINIMAL
{"schemaVersion":3,"imageID":"$IMAGE_ID","contentHash":"$CONTENT_HASH","capturedAt":"$(date -u +%Y-%m-%dT%H:%M:%SZ)","osFamily":"unknown","osVersion":"unknown","osRelease":"unknown","kernel":"unknown","dpkgPackages":[],"rpmPackages":[],"apkPackages":[],"pythonPackages":[],"rPackages":[],"condaEnvs":[],"capabilities":[],"pythonVersion":"unknown","shells":[],"probeNotes":"python3 not found in image"}
MINIMAL
    mv "$TMP" "$OUT"
    exit 0
fi

python3 - <<'PYEOF' > "$TMP"
import os, json, glob, time

stage = os.environ["STAGE"]
image_id = os.environ["IMAGE_ID"]
content_hash = os.environ.get("CONTENT_HASH", "sha256:none")

def read_pkgs(path):
    try:
        out = []
        for ln in open(path):
            ln = ln.strip()
            if not ln or "|" not in ln: continue
            name, _, version = ln.partition("|")
            out.append({"name": name, "version": version})
        return out
    except FileNotFoundError:
        return []

def parse_os_release():
    try:
        data = {}
        for ln in open(os.path.join(stage, "os-release")):
            k, _, v = ln.strip().partition("=")
            data[k] = v.strip('"\'')
        return (
            data.get("ID", "unknown"),
            data.get("VERSION_ID", "unknown"),
            data.get("PRETTY_NAME", "unknown"),
        )
    except FileNotFoundError:
        return "unknown", "unknown", "unknown"

def python_version():
    # `python3 -V` output looks like "Python 3.11.6" or
    # (on Python 2 fallback) "Python 2.7.18". Strip the
    # leading "Python " and trailing whitespace.
    try:
        raw = open(os.path.join(stage, "python-version.txt")).read().strip()
        if raw.startswith("Python "):
            return raw[len("Python "):].strip() or "unknown"
        return raw or "unknown"
    except FileNotFoundError:
        return "unknown"

def shells_list():
    try:
        seen = []
        for ln in open(os.path.join(stage, "shells.txt")):
            name = ln.strip()
            if name and name not in seen:
                seen.append(name)
        return sorted(seen)
    except FileNotFoundError:
        return []

def kernel():
    try:
        return open(os.path.join(stage, "kernel")).read().strip() or "unknown"
    except FileNotFoundError:
        return "unknown"

def conda_envs():
    envs = []
    for meta in glob.glob(os.path.join(stage, "conda-meta-*.txt")):
        name = os.path.basename(meta)[len("conda-meta-"):-4]
        prefix = open(meta).read().strip()
        pkgs_file = os.path.join(stage, f"conda-pkgs-{name}.txt")
        envs.append({
            "name": name, "prefix": prefix,
            "packages": [
                {"name": p["name"], "version": p["version"], "source": "conda", "env": name}
                for p in read_pkgs(pkgs_file)
            ]
        })
    return envs

def python_system():
    # Only used when no conda found.
    return [
        {"name": p["name"], "version": p["version"], "source": "pip", "env": ""}
        for p in read_pkgs(os.path.join(stage, "python-system.txt"))
    ]

os_family, os_version, os_release = parse_os_release()

# Flatten conda envs into pythonPackages so the UI can search uniformly,
# but keep condaEnvs[] populated for env-aware filters.
envs = conda_envs()
flat_python = python_system()
for env in envs:
    flat_python.extend(env["packages"])

notes = None
if not envs and not flat_python and not read_pkgs(os.path.join(stage, "dpkg")) \
   and not read_pkgs(os.path.join(stage, "rpm")) \
   and not read_pkgs(os.path.join(stage, "apk")):
    notes = "image lacks dpkg/rpm/apk and pip â€” minimal manifest"

# Capability markers â€” one per line, in order of detection.
# Empty file or missing file (older probes) â†’ empty list.
capabilities = []
cap_path = os.path.join(stage, "capabilities.txt")
if os.path.exists(cap_path):
    with open(cap_path) as f:
        capabilities = [line.strip() for line in f if line.strip()]

manifest = {
    "schemaVersion": 3,
    "imageID": image_id,
    "contentHash": content_hash,
    "capturedAt": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
    "osFamily": os_family,
    "osVersion": os_version,
    "osRelease": os_release,
    "kernel": kernel(),
    "dpkgPackages": read_pkgs(os.path.join(stage, "dpkg")),
    "rpmPackages":  read_pkgs(os.path.join(stage, "rpm")),
    "apkPackages":  read_pkgs(os.path.join(stage, "apk")),
    "pythonPackages": flat_python,
    "rPackages":    read_pkgs(os.path.join(stage, "r.txt")),
    "condaEnvs":    envs,
    "capabilities": capabilities,
    "pythonVersion": python_version(),
    "shells":       shells_list(),
}
if notes is not None:
    manifest["probeNotes"] = notes

print(json.dumps(manifest, separators=(",", ":")))
PYEOF

# Atomic publish
mv "$TMP" "$OUT"
echo "ok: $OUT"
