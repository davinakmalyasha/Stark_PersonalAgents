#!/usr/bin/env bash
# Stark Rune portable package (Windows x86_64): stage the release binary with
# version metadata, zip it, hash it, then smoke-test the staged copy.
# No Inno/WiX on this machine — a signed installer comes later; the zip is
# byte-verifiable today.
#
#   desktop-rune/packaging/package.sh [--smoke-only]
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
DESKTOP="$ROOT/desktop-rune"
TARGET_DIR="${CARGO_TARGET_DIR:-$DESKTOP/target}"
VERSION="$(grep '^version' "$DESKTOP/Cargo.toml" | head -1 | cut -d'"' -f2)"
ARCH="windows-x86_64"
STAGE="$DESKTOP/dist/stark-rune-$VERSION-$ARCH"
ZIP="$DESKTOP/dist/stark-rune-$VERSION-$ARCH.zip"

if [ "${1:-}" != "--smoke-only" ]; then
  cargo build --manifest-path "$DESKTOP/Cargo.toml" -p stark-rune-app --features rune-real --release
  rm -rf "$STAGE" "$ZIP"
  mkdir -p "$STAGE"
  cp "$TARGET_DIR/release/stark-rune.exe" "$STAGE/"
  "$TARGET_DIR/release/stark-rune.exe" --version > "$STAGE/VERSION.txt"
  cat > "$STAGE/README.txt" <<EOF
Stark Rune $VERSION ($ARCH) — native Rust desktop UI (branch rune-rewrite).
Run stark-rune.exe directly; it opens the Map/Chat shell on 1280x900.
The UI talks to stark-server on 127.0.0.1:8124 (run it separately).
Headless checks: stark-rune.exe --headless --dump-tree tree.txt
EOF
  powershell -NoProfile -Command "Compress-Archive -Path '$(cygpath -w "$STAGE")' -DestinationPath '$(cygpath -w "$ZIP")' -Force"
  (cd "$DESKTOP/dist" && sha256sum "stark-rune-$VERSION-$ARCH.zip" > SHA256SUMS.txt)
fi

echo "--- package contents ---"
powershell -NoProfile -Command "(Get-ChildItem '$(cygpath -w "$STAGE")' | Format-Table Name, Length -AutoSize | Out-String -Width 80).Trim()"
echo "--- smoke test (staged copy) ---"
SMOKE="$(mktemp -d)"
powershell -NoProfile -Command "Expand-Archive -Path '$(cygpath -w "$ZIP")' -DestinationPath '$(cygpath -w "$SMOKE")' -Force"
"$SMOKE/stark-rune-$VERSION-$ARCH/stark-rune.exe" --version
"$SMOKE/stark-rune-$VERSION-$ARCH/stark-rune.exe" --headless --dump-tree "$SMOKE/tree.txt" > /dev/null
head -2 "$SMOKE/tree.txt"
rm -rf "$SMOKE"
echo "SMOKE_OK"
