#!/usr/bin/env bash
# Stark Rune release package.
#
# Produces:
#   dist/StarkRune-<version>-windows-x86_64/          per-user install tree
#   dist/StarkRune-<version>-windows-x86_64.zip      manual/offline install
#   dist/StarkRune-<version>-x64.zip                 updater payload (app only)
#   dist/StarkRune-<version>-x64.zip.sha256
#   dist/StarkRune-<version>-x64.zip.minisig         when MINISIGN_SECRET_KEY is set
#   dist/StarkRune-Setup-<version>.exe               NSIS bootstrap when makensis exists
#
# The updater extracts the payload into app/<version>/ and leaves the launcher
# and current.json untouched. The full tree is for first install/repair.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
DESKTOP="$ROOT/desktop-rune"
VERSION="$(grep '^version' "$DESKTOP/Cargo.toml" | head -1 | cut -d'"' -f2)"
ARCH="windows-x86_64"
DIST="$DESKTOP/dist"
STAGE="$DIST/StarkRune-$VERSION-$ARCH"
APP="$STAGE/app/$VERSION"
PAYLOAD="$DIST/StarkRune-$VERSION-x64.zip"
ROOT_ZIP="$DIST/StarkRune-$VERSION-$ARCH.zip"
TARGET_DIR="${CARGO_TARGET_DIR:-$DESKTOP/target}"

if [ "${1:-}" != "--stage-only" ]; then
  cargo build --manifest-path "$DESKTOP/Cargo.toml" \
    -p stark-rune-app -p stark-launcher \
    --features stark-rune-app/rune-real --release
  cargo build --manifest-path "$ROOT/backend-rs/Cargo.toml" -p stark-server --release
fi

SERVER_BIN="${STARK_SERVER_BIN:-}"
if [ -z "$SERVER_BIN" ]; then
  for candidate in \
    "$ROOT/backend-rs/target/release/stark-server.exe" \
    "$TARGET_DIR/release/stark-server.exe" \
    "D:/cargo-target/stark-server/release/stark-server.exe" \
    "D:/cargo-target/stark/release/stark-server.exe"; do
    if [ -f "$candidate" ]; then SERVER_BIN="$candidate"; break; fi
  done
fi
[ -n "$SERVER_BIN" ] && [ -f "$SERVER_BIN" ] || {
  echo "stark-server.exe not found; set STARK_SERVER_BIN or build backend-rs" >&2
  exit 1
}

APP_BIN="$TARGET_DIR/release/stark-rune.exe"
LAUNCHER_BIN="$TARGET_DIR/release/stark-launcher.exe"
[ -f "$APP_BIN" ] || { echo "missing $APP_BIN" >&2; exit 1; }
[ -f "$LAUNCHER_BIN" ] || { echo "missing $LAUNCHER_BIN" >&2; exit 1; }

rm -rf "$STAGE" "$PAYLOAD" "$ROOT_ZIP"
mkdir -p "$APP"

cp "$LAUNCHER_BIN" "$STAGE/StarkRune.exe"
cp "$APP_BIN" "$APP/stark-rune.exe"
cp "$SERVER_BIN" "$APP/stark-server.exe"
printf '{\n  "version": "%s",\n  "previous": null\n}\n' "$VERSION" > "$STAGE/current.json"

# Shared runtime data stays outside versioned app folders.
for dll in "$ROOT/backend-rs/onnxruntime.dll" "${STARK_ONNXRUNTIME_DLL:-}"; do
  if [ -n "$dll" ] && [ -f "$dll" ]; then
    cp "$dll" "$APP/onnxruntime.dll"
    break
  fi
done
MODELS="${STARK_MODELS_DIR:-$ROOT/backend-rs/models}"
if [ -d "$MODELS" ]; then
  mkdir -p "$STAGE/models"
  cp -R "$MODELS/." "$STAGE/models/"
fi
if [ -n "${STARK_UPDATE_PUBKEY_FILE:-}" ] && [ -f "$STARK_UPDATE_PUBKEY_FILE" ]; then
  cp "$STARK_UPDATE_PUBKEY_FILE" "$STAGE/update.pub"
fi
mkdir -p "$STAGE/licenses"
for license in POCKET_TTS_LICENSE THIRD_PARTY_VOICE.md; do
  if [ -f "$ROOT/backend-rs/crates/stark-voice/$license" ]; then
    cp "$ROOT/backend-rs/crates/stark-voice/$license" "$STAGE/licenses/$license"
  fi
done
"$APP/stark-rune.exe" --version > "$APP/VERSION.txt"
"$STAGE/StarkRune.exe" --version > "$STAGE/LAUNCHER_VERSION.txt"
cat > "$STAGE/README.txt" <<EOF
Stark Rune $VERSION ($ARCH)
Run StarkRune.exe. The launcher starts the bundled backend and keeps the
current UI/backend pair under app/; updates are silent and roll back on failure.
Data lives in %LOCALAPPDATA%\\StarkRune\\data (override with STARK_HARNESS_ROOT).
EOF

# Payload: only the version directory contents. updater.rs extracts this into
# app/<version>, so the launcher's current.json and shared models never move.
(cd "$APP" && powershell -NoProfile -Command "Compress-Archive -Path '*' -DestinationPath '$(cygpath -w "$PAYLOAD")' -Force")
(cd "$DIST" && sha256sum "$(basename "$PAYLOAD")" > "$(basename "$PAYLOAD").sha256")
if [ -n "${MINISIGN_SECRET_KEY:-}" ]; then
  minisign -S -s "$MINISIGN_SECRET_KEY" -m "$PAYLOAD" -x "$PAYLOAD.minisig"
fi

# Full tree for first install or repair.
(cd "$DIST" && powershell -NoProfile -Command "Compress-Archive -Path '$(cygpath -w "$STAGE")' -DestinationPath '$(cygpath -w "$ROOT_ZIP")' -Force")
(cd "$DIST" && sha256sum "$(basename "$ROOT_ZIP")" > "$(basename "$ROOT_ZIP").sha256")

if command -v makensis >/dev/null 2>&1; then
  makensis -V2 \
    -DSTAGE="$(cygpath -w "$STAGE")" \
    -DVERSION="$VERSION" \
    -DOUTFILE="$(cygpath -w "$DIST/StarkRune-Setup-$VERSION.exe")" \
    "$DESKTOP/packaging/StarkRune.nsi"
else
  echo "makensis not found: skipped StarkRune-Setup-$VERSION.exe" >&2
fi

echo "--- payload ---"
ls -lh "$PAYLOAD" "$PAYLOAD.sha256"
echo "--- install tree ---"
ls -lh "$STAGE/StarkRune.exe" "$STAGE/current.json" "$APP/stark-rune.exe" "$APP/stark-server.exe"
echo "--- smoke ---"
"$APP/stark-rune.exe" --version
"$STAGE/StarkRune.exe" --version
echo "PACKAGE_OK"
