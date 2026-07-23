#!/usr/bin/env bash
# FigWizard installer: downloads the latest GitHub release's .dmg and
# installs FigWizard.app into /Applications.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/ziward-inc/fig-wizard/main/install.sh | bash
#   wget -qO- https://raw.githubusercontent.com/ziward-inc/fig-wizard/main/install.sh | bash
set -euo pipefail

REPO="ziward-inc/fig-wizard"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "error: FigWizard only supports macOS (Apple Silicon, macOS 15+)." >&2
  exit 1
fi
if [[ "$(uname -m)" != "arm64" ]]; then
  echo "error: FigWizard only ships an Apple Silicon (arm64) build." >&2
  exit 1
fi

api_get() { # $1 = URL
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL -H "Accept: application/vnd.github+json" "$1"
  else
    wget -qO- --header="Accept: application/vnd.github+json" "$1"
  fi
}

download_asset() { # $1 = download URL, $2 = output path
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL -L -o "$2" "$1"
  else
    wget -qO "$2" "$1"
  fi
}

# Pulls one field off the (single) .dmg asset in a /releases/latest response,
# or the top-level tag_name when $2 is "tag_name". jq is preferred; python3
# (always present on macOS) is the fallback so this doesn't gain a hard
# dependency beyond what the OS already ships.
release_field() { # $1 = release JSON, $2 = field name
  if command -v jq >/dev/null 2>&1; then
    if [[ "$2" == "tag_name" ]]; then
      printf '%s' "$1" | jq -r '.tag_name // empty'
    else
      printf '%s' "$1" | jq -r --arg f "$2" '[.assets[] | select(.name | endswith(".dmg"))][0][$f] // empty'
    fi
  else
    FIELD="$2" python3 -c '
import json, os, sys
data = json.load(sys.stdin)
field = os.environ["FIELD"]
if field == "tag_name":
    print(data.get("tag_name", ""))
else:
    for a in data.get("assets", []):
        if a["name"].endswith(".dmg"):
            print(a[field])
            break
' <<<"$1"
  fi
}

echo "==> Looking up the latest FigWizard release..."
RELEASE_JSON="$(api_get "https://api.github.com/repos/$REPO/releases/latest")"

TAG="$(release_field "$RELEASE_JSON" tag_name)"
ASSET_URL="$(release_field "$RELEASE_JSON" browser_download_url)"
ASSET_NAME="$(release_field "$RELEASE_JSON" name)"

if [[ -z "$ASSET_URL" || -z "$ASSET_NAME" ]]; then
  echo "error: couldn't find a .dmg asset on the latest release. Response was:" >&2
  printf '%s\n' "$RELEASE_JSON" >&2
  exit 1
fi

echo "==> Found $ASSET_NAME ($TAG)"

WORKDIR=""
MOUNT_POINT=""
cleanup() {
  if [[ -n "$MOUNT_POINT" && -d "$MOUNT_POINT" ]]; then
    hdiutil detach "$MOUNT_POINT" -quiet >/dev/null 2>&1 || true
  fi
  if [[ -n "$WORKDIR" && -d "$WORKDIR" ]]; then
    rm -rf "$WORKDIR"
  fi
}
trap cleanup EXIT

WORKDIR="$(mktemp -d)"
DMG_PATH="$WORKDIR/$ASSET_NAME"

echo "==> Downloading $ASSET_NAME..."
download_asset "$ASSET_URL" "$DMG_PATH"

echo "==> Mounting disk image..."
MOUNT_POINT="$(mktemp -d)"
hdiutil attach "$DMG_PATH" -nobrowse -quiet -mountpoint "$MOUNT_POINT"

APP_PATH="$(find "$MOUNT_POINT" -maxdepth 1 -iname '*.app' | head -n1)"
if [[ -z "$APP_PATH" ]]; then
  echo "error: no .app bundle found inside $ASSET_NAME." >&2
  exit 1
fi
APP_BASENAME="$(basename "$APP_PATH")"

echo "==> Installing $APP_BASENAME to /Applications..."
rm -rf "/Applications/$APP_BASENAME"
cp -R "$APP_PATH" "/Applications/$APP_BASENAME"

hdiutil detach "$MOUNT_POINT" -quiet
MOUNT_POINT=""

# This build is ad-hoc signed only, not notarized (see README.md's
# "Notarization status") - strip any quarantine attribute defensively so
# Gatekeeper doesn't block the first launch. curl/wget downloads typically
# don't get the com.apple.quarantine xattr in the first place (only
# GUI download managers set it), but this is a no-op if it's already absent.
xattr -cr "/Applications/$APP_BASENAME"

echo ""
echo "FigWizard ($TAG) installed to /Applications/$APP_BASENAME"
echo "Launch it from Spotlight/Launchpad, or: open \"/Applications/$APP_BASENAME\""
