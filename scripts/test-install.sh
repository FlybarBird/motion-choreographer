#!/usr/bin/env bash
# Run scripts/macos-install.sh end to end on any machine, with the build,
# platform checks and signing stubbed, and check what it installed.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK" "$ROOT/target/install-test"' EXIT

# Stand-in release binaries, each printing its own name.
RELEASE="$ROOT/target/install-test/release"
mkdir -p "$RELEASE"
for bin in ptz-app ptz-server ptz-probe ptz-bringup; do
  printf '#!/bin/sh\necho %s\n' "$bin" >"$RELEASE/$bin"
  chmod +x "$RELEASE/$bin"
done

run() {
  PTZ_TARGET=install-test PTZ_SKIP_BUILD=1 PTZ_SKIP_CHECKS=1 PTZ_SKIP_SIGN=1 \
    PTZ_APP_DEST="$WORK/Applications" PTZ_BIN_DEST="$WORK/bin" \
    bash "$ROOT/scripts/macos-install.sh"
}

run >"$WORK/first.log"
# A second run over an existing install must also succeed.
run >"$WORK/second.log"

APP="$WORK/Applications/PTZ Programmer.app"
check() {
  if ! "$@"; then
    printf 'install test failed: %s\n' "$*" >&2
    exit 1
  fi
}
check test -x "$APP/Contents/MacOS/ptz-app"
check test -f "$APP/Contents/Info.plist"
check grep -q '<string>ptz-app</string>' "$APP/Contents/Info.plist"
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -n 1)"
check grep -q "<string>$VERSION</string>" "$APP/Contents/Info.plist"
for bin in ptz-server ptz-probe ptz-bringup; do
  check test -L "$WORK/bin/$bin"
  check test "$("$WORK/bin/$bin")" = "$bin"
done
check grep -q 'Installed' "$WORK/second.log"
check grep -q 'is not on your PATH' "$WORK/second.log"

# Without Rust the script must stop with the install command, not a stack of errors.
if PATH="/usr/bin:/bin" PTZ_SKIP_CHECKS=1 bash "$ROOT/scripts/macos-install.sh" >"$WORK/norust.log" 2>&1; then
  if ! PATH="/usr/bin:/bin" command -v cargo >/dev/null 2>&1; then
    echo "install test failed: the script succeeded without cargo" >&2
    exit 1
  fi
else
  check grep -q 'rustup.rs' "$WORK/norust.log"
fi

echo "install script test passed"
