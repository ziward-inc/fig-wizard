#!/usr/bin/env bash
#
# Usage:
#   ./deploy.sh v0.2.8
#
# Bumps the version, builds and verifies the DMG, then commits, tags, pushes,
# and publishes a GitHub release. Run from a clean main branch.

set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "Usage: $0 vX.Y.Z" >&2
  exit 1
fi

TAG="$1"
VERSION="${TAG#v}"

if [[ ! "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "Version must look like vX.Y.Z (got: $TAG)" >&2
  exit 1
fi

cd "$(git rev-parse --show-toplevel)"

if [[ "$(git branch --show-current)" != "main" ]]; then
  echo "Must be on the main branch" >&2
  exit 1
fi

if [[ -n "$(git status --porcelain)" ]]; then
  echo "Working tree is not clean" >&2
  exit 1
fi

if git ls-remote --tags origin "refs/tags/${TAG}" | grep -q .; then
  echo "Tag ${TAG} already exists on origin" >&2
  exit 1
fi

perl -0pi -e 's/"version": "[^"]+"/"version": "'"$VERSION"'"/' package.json
perl -0pi -e 's/^version = "[^"]+"/version = "'"$VERSION"'"/m' src-tauri/Cargo.toml
perl -0pi -e 's/"version": "[^"]+"/"version": "'"$VERSION"'"/' src-tauri/tauri.conf.json
cargo check --manifest-path src-tauri/Cargo.toml

pnpm lint
pnpm typecheck
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
pnpm tauri build

DMG="src-tauri/target/release/bundle/dmg/FigWizard_${VERSION}_aarch64.dmg"
MOUNT="$(mktemp -d /tmp/figwizard-release-XXXXXX)"
hdiutil attach -nobrowse -readonly -mountpoint "$MOUNT" "$DMG"
BUNDLE_VERSION="$(plutil -extract CFBundleShortVersionString raw "$MOUNT/FigWizard.app/Contents/Info.plist")"
file "$MOUNT/FigWizard.app/Contents/MacOS/figwizard"
PDFIUM_LIB="$MOUNT/FigWizard.app/Contents/Frameworks/libpdfium.dylib"
if [[ ! -f "$PDFIUM_LIB" ]]; then
  echo "Bundled PDFium library is missing: $PDFIUM_LIB" >&2
  exit 1
fi
file "$PDFIUM_LIB"
codesign -dv --verbose=4 "$MOUNT/FigWizard.app"
hdiutil detach "$MOUNT"
rmdir "$MOUNT"

if [[ "$BUNDLE_VERSION" != "$VERSION" ]]; then
  echo "Bundle version ($BUNDLE_VERSION) does not match requested version ($VERSION)" >&2
  exit 1
fi

shasum -a 256 "$DMG"
stat -f '%z' "$DMG"

git diff --check
git add package.json src-tauri/Cargo.lock src-tauri/Cargo.toml src-tauri/tauri.conf.json
git commit -m "Release ${TAG}"
git push origin main
git tag "${TAG}"
git push origin "${TAG}"
gh release create "${TAG}" "$DMG" --verify-tag --latest --title "FigWizard ${TAG}" --generate-notes
