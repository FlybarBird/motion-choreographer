#!/usr/bin/env bash
# Produce a clean source tarball in /opt/cursor/artifacts (or ./dist).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT_DIR="${ARTIFACT_DIR:-/opt/cursor/artifacts}"
mkdir -p "$OUT_DIR"

STAGING="$(mktemp -d)"
NAME="ptz-director-0.2.0-src"
DEST="$STAGING/$NAME"
mkdir -p "$DEST"

# Copy source without build artifacts / git / caches / databases
tar -C "$ROOT" \
  --exclude='.git' \
  --exclude='target' \
  --exclude='dist' \
  --exclude='.DS_Store' \
  --exclude='*.db' \
  --exclude='*.sqlite' \
  --exclude='README.seed.md' \
  --exclude='.cursor' \
  -cf - . | tar -C "$DEST" -xf -

# Ensure fixture wav exists
if [[ ! -f "$DEST/fixtures/test-click-12_4s.wav" ]]; then
  python3 "$DEST/scripts/generate-test-wav.py"
fi

TAR="$OUT_DIR/$NAME.tar.gz"
tar -C "$STAGING" -czf "$TAR" "$NAME"
rm -rf "$STAGING"

if command -v sha256sum >/dev/null; then
  sha256sum "$TAR" | tee "$OUT_DIR/$NAME.tar.gz.sha256"
elif command -v shasum >/dev/null; then
  shasum -a 256 "$TAR" | tee "$OUT_DIR/$NAME.tar.gz.sha256"
fi

echo "Artifact: $TAR"
ls -lh "$TAR"
