#!/usr/bin/env bash
# Put the desktop entry and icons where the shell looks, for a build from
# source.
#
# Without this the app runs perfectly and shows a generic icon: on Wayland the
# shell finds a window's icon by matching its app_id to an INSTALLED desktop
# entry, and a binary in target/ has none. Nothing in the application can
# stand in for that — see the note in src/lib.rs.
#
# Installs to ~/.local/share, so it needs no root and touches nothing the
# package manager owns. `--uninstall` takes it all back out again.
set -euo pipefail

APP_ID="net.canfar.Verbinal"
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DATA="${XDG_DATA_HOME:-$HOME/.local/share}"

uninstall() {
  rm -f "$DATA/applications/$APP_ID.desktop"
  rm -f "$DATA/metainfo/$APP_ID.metainfo.xml"
  find "$DATA/icons/hicolor" -name "$APP_ID.png" -delete 2>/dev/null || true
  find "$DATA/icons/hicolor" -name "$APP_ID.svg" -delete 2>/dev/null || true
  echo "removed $APP_ID from $DATA"
}

if [[ "${1:-}" == "--uninstall" ]]; then
  uninstall
else
  install -d "$DATA/applications"
  install -m 644 "$REPO/data/$APP_ID.desktop" "$DATA/applications/"
  install -d "$DATA/metainfo"
  install -m 644 "$REPO/data/$APP_ID.metainfo.xml" "$DATA/metainfo/"

  # Every size the package ships, from the same tree the package reads.
  shopt -s nullglob
  for src in "$REPO"/assets/icons/hicolor/*/apps/"$APP_ID".{png,svg}; do
    rel="${src#"$REPO"/assets/icons/}"
    install -d "$DATA/icons/${rel%/*}"
    install -m 644 "$src" "$DATA/icons/$rel"
  done
  echo "installed $APP_ID into $DATA"
fi

# The caches are what the shell actually reads; without refreshing them the
# files are on disk and still invisible.
update-desktop-database "$DATA/applications" 2>/dev/null || true
gtk4-update-icon-cache -qtf "$DATA/icons/hicolor" 2>/dev/null ||
  gtk-update-icon-cache -qtf "$DATA/icons/hicolor" 2>/dev/null || true

echo "log out and back in if the shell still shows the old icon"
