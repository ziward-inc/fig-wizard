# Contributing to FigWizard

## Frontend stack

The UI (`src`) is a Vite + React + TypeScript app using [shadcn/ui](https://ui.shadcn.com)
components (on top of `@base-ui/react` primitives, not Radix — per this project's
`components.json`, from the `beXRTXBi` preset) and Tailwind v4. It talks to the Rust side via
`@tauri-apps/api` (`core`/`event`/`webview`) rather than the `window.__TAURI__` global —
`tauri.conf.json` sets `app.withGlobalTauri: false` accordingly. The UI font is
[SUITE Variable](https://github.com/sun-typeface/SUITE), vendored locally at
`public/fonts/SUITE-Variable.woff2` (not loaded from a CDN, so the app doesn't need
network access to render its own chrome) and wired in as `--font-sans` in `src/index.css`;
Geist Mono remains `--font-mono` for the monospace bits (file paths, etc).

Layout of `src`:

- `App.tsx` — top-level state (current PDF, output dir/format, job/progress state, results manifest) and the Tauri event-listener wiring; composes the section components below into the four numbered cards plus the results list.
- `components/app/` — the app's own feature components (`ModelBanner`, `PdfDropZone`, `FormatPicker`, `VerifySection`, `ExtractSection`, `ResultsGallery`, and `VerificationBadge`). Each corresponds 1:1 to a section/behavior of the original vanilla-JS UI.
- `components/ui/` — shadcn-generated primitives (button, card, dialog, etc.) — treat
  these as generated code; re-run `pnpm dlx shadcn@latest add <name>` from the repository root to
  add more or `--overwrite` to regenerate rather than hand-editing them.
- `lib/tauri-types.ts` — TypeScript mirrors of the Rust types in `src-tauri/src/commands.rs`
  / `src-tauri/src/pipeline/types.rs`. Note the casing split: pipeline events
  (`page-detected`, `object-exported`, etc.) are hand-built with `serde_json::json!` and
  use **camelCase** keys (`jobId`, `pageIndex`, ...), while `manifest.json` / command
  return values use serde's default **snake_case** (`page_index`, `has_caption`, ...).
  This is intentional on the Rust side, not a typo — keep it in mind when adding fields.
- `lib/tauri-commands.ts` — thin typed wrappers around `invoke("command_name", ...)` for
  every `#[tauri::command]` in `commands.rs`.
- `lib/format.ts` — small pure helpers (`pdfStem`, `dirName`, `formatBytes`).

Because `App.tsx` sets up the `listen()` calls once on mount, but needs the *current*
`currentJobId`/`currentPdf`/`currentOutputDir` inside those long-lived callbacks (to filter
events by job id, or to call `list_results` after `extraction-complete`), those three
pieces of state are mirrored into refs (`currentJobIdRef` etc.) that are updated in lockstep
with the corresponding `useState` setters, rather than read directly from the closed-over
state.

To add more shadcn components: `pnpm dlx shadcn@latest add <name>` from the repository root.

## Model / PDFium download

On first run, if the detection model / PDFium library aren't present yet, the app shows a
"Download model (~125MB)" banner. Clicking it downloads:

- The PP-DocLayoutV3 ONNX model + its `config.json` (label list) from
  `huggingface.co/alex-dinh/PP-DocLayoutV3-ONNX`.
- The macOS arm64 PDFium dylib from a `bblanchon/pdfium-binaries` GitHub release
  (`pdfium-mac-arm64.tgz`, only `lib/libpdfium.dylib` is kept).

Both land under the OS-managed app-data directory (not inside the app bundle), so a
packaged release doesn't need to ship ~130MB of model weights. During local development
(`cargo tauri dev` / debug builds only), the app also falls back to the gitignored copies
already checked out at `src-tauri/models/` and `src-tauri/binaries/pdfium/` if present, so
contributors who already have those don't need to re-download anything. That fallback path
is compiled out of release builds.

`download_model` streams the model, its config, and the PDFium archive straight to disk
with no SHA-256 check against a known hash.

## Building from source

Building requires CMake and a C++ toolchain, because JPEG XL support is provided by the
reference libjxl implementation, statically linked at build time through the permissively
licensed `gamut-jxl`/`gamut-jxl-sys` Rust wrapper (no Homebrew/`cjxl`/system-library
dependency at runtime). This doesn't affect `.dmg` or one-line-script installs — end users
get the already-built encoder inside `FigWizard.app`.

`cargo install figwizard` (by crate name, without `--path`) doesn't work and isn't planned:
`cargo publish --dry-run` fails because `tauri.conf.json`'s `frontendDist` points to
`"../dist"`, outside the `src-tauri/` crate root, so a published crate wouldn't include the
frontend build.

## Notarization

The published `.dmg`/`.app` is **ad-hoc signed only** (`codesign -dv` reports
`flags=0x20002(adhoc,linker-signed)`, `TeamIdentifier=not set`) — there is no Apple
Developer ID signature and it is not notarized.

Shipping a build that opens with no Gatekeeper warning would require:

1. A paid Apple Developer Program membership (Team ID).
2. A Developer ID Application signing certificate in the build machine's keychain.
3. Tauri's bundler configured to sign during `tauri build` — as of Tauri v2, generally via
   `APPLE_SIGNING_IDENTITY`, `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD` for signing,
   plus `APPLE_ID`/`APPLE_PASSWORD` (or an App Store Connect API key + `APPLE_TEAM_ID`) for
   notarization submission. Verify current variable names against the
   [Tauri v2 macOS signing docs](https://v2.tauri.app/distribute/sign/macos/) before
   relying on them.
4. `tauri build` submits to Apple's notary service automatically once signing is
   configured; add `--skip-stapling` on the very first run, since initial notarization
   "can take multiple hours."

## Releasing a new version

Run these commands from the repository root on a clean `main` branch. Choose a new,
unused patch/minor/major version and replace `0.2.7` below.

```sh
VERSION=0.2.7

perl -0pi -e 's/"version": "[^"]+"/"version": "'"$VERSION"'"/' package.json
perl -0pi -e 's/^version = "[^"]+"/version = "'"$VERSION"'"/m' src-tauri/Cargo.toml
perl -0pi -e 's/"version": "[^"]+"/"version": "'"$VERSION"'"/' src-tauri/tauri.conf.json
cargo check --manifest-path src-tauri/Cargo.toml

pnpm lint
pnpm typecheck
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
pnpm tauri build
```

Verify the generated artifact before publishing:

```sh
DMG="src-tauri/target/release/bundle/dmg/FigWizard_${VERSION}_aarch64.dmg"
MOUNT="$(mktemp -d /tmp/figwizard-release-XXXXXX)"
hdiutil attach -nobrowse -readonly -mountpoint "$MOUNT" "$DMG"
plutil -extract CFBundleShortVersionString raw "$MOUNT/FigWizard.app/Contents/Info.plist"
file "$MOUNT/FigWizard.app/Contents/MacOS/figwizard"
codesign -dv --verbose=4 "$MOUNT/FigWizard.app"
hdiutil detach "$MOUNT"
rmdir "$MOUNT"
shasum -a 256 "$DMG"
stat -f '%z' "$DMG"
```

Commit the exact source used to build the DMG, then push the tag and publish the asset:

```sh
git status --short
git diff --check
git ls-remote --tags origin "refs/tags/v${VERSION}"
git add package.json src-tauri/Cargo.lock src-tauri/Cargo.toml src-tauri/tauri.conf.json
git commit -m "Release v${VERSION}"
git push origin main
git tag "v${VERSION}"
git push origin "v${VERSION}"
gh release create "v${VERSION}" "$DMG" --verify-tag --latest --title "FigWizard v${VERSION}" --generate-notes
```
