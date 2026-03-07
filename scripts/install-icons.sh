#!/bin/bash
# Install Verbinal icons into the system/user icon theme and desktop entry.
# Usage: ./scripts/install-icons.sh [--user]
#   --user  Install to ~/.local instead of /usr (no sudo needed)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

if [[ "${1:-}" == "--user" ]]; then
    PREFIX="$HOME/.local"
else
    PREFIX="/usr"
fi

ICON_DIR="$PREFIX/share/icons/hicolor"
APP_DIR="$PREFIX/share/applications"

for size in 16 24 32 48 64 128 256 512; do
    dest="$ICON_DIR/${size}x${size}/apps"
    mkdir -p "$dest"
    cp "$PROJECT_DIR/assets/icons/hicolor/${size}x${size}/apps/net.canfar.Verbinal.png" "$dest/net.canfar.Verbinal.png"
done

# Install scalable SVG
mkdir -p "$ICON_DIR/scalable/apps"
cp "$PROJECT_DIR/assets/icons/hicolor/scalable/apps/net.canfar.Verbinal.svg" "$ICON_DIR/scalable/apps/net.canfar.Verbinal.svg"

# Install desktop entry
mkdir -p "$APP_DIR"
cp "$PROJECT_DIR/data/net.canfar.Verbinal.desktop" "$APP_DIR/"

# Update icon cache
if command -v gtk-update-icon-cache &>/dev/null; then
    gtk-update-icon-cache -f -t "$ICON_DIR" 2>/dev/null || true
fi

echo "Icons and desktop entry installed to $PREFIX"
