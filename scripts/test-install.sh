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
for bin in ptz-app ptz-server ptz-probe ptz-bringup ptz-panel; do
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

APP="$WORK/Applications/Motion Choreographer.app"
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
for bin in ptz-server ptz-probe ptz-bringup ptz-panel; do
  check test -L "$WORK/bin/$bin"
  check test "$("$WORK/bin/$bin")" = "$bin"
done
check grep -q 'Installed' "$WORK/second.log"
check grep -q 'is not on your PATH' "$WORK/second.log"

check test -f "$APP/Contents/Resources/licenses/LICENSE"
check test -f "$APP/Contents/Resources/licenses/THIRD-PARTY-NOTICES.md"
check test -f "$APP/Contents/Resources/licenses/Inter-LICENSE.txt"

# Without Rust the script must stop with the install command, not a stack of errors.
# The destinations are given so that a machine with cargo in /usr/bin installs nowhere real.
if PATH="/usr/bin:/bin" PTZ_TARGET=install-test PTZ_SKIP_BUILD=1 PTZ_SKIP_CHECKS=1 PTZ_SKIP_SIGN=1 \
  PTZ_APP_DEST="$WORK/norust/Applications" PTZ_BIN_DEST="$WORK/norust/bin" \
  bash "$ROOT/scripts/macos-install.sh" >"$WORK/norust.log" 2>&1; then
  if ! PATH="/usr/bin:/bin" command -v cargo >/dev/null 2>&1; then
    echo "install test failed: the script succeeded without cargo" >&2
    exit 1
  fi
else
  check grep -q 'rustup.rs' "$WORK/norust.log"
fi

# A Rust older than the manifest asks for is named, with the command that updates it.
mkdir -p "$WORK/oldrust"
printf '#!/bin/sh\necho "rustc 1.80.0 (old 2024-01-01)"\n' >"$WORK/oldrust/rustc"
printf '#!/bin/sh\nexit 0\n' >"$WORK/oldrust/cargo"
printf '#!/bin/sh\necho Darwin\n' >"$WORK/oldrust/uname"
printf '#!/bin/sh\nexit 0\n' >"$WORK/oldrust/xcode-select"
chmod +x "$WORK/oldrust"/*
if PATH="$WORK/oldrust:$PATH" PTZ_TARGET=install-test PTZ_SKIP_BUILD=1 PTZ_SKIP_SIGN=1 \
  PTZ_APP_DEST="$WORK/oldrust/Applications" PTZ_BIN_DEST="$WORK/oldrust/bin" \
  bash "$ROOT/scripts/macos-install.sh" >"$WORK/oldrust.log" 2>&1; then
  echo "install test failed: the script accepted Rust 1.80" >&2
  exit 1
fi
check grep -q 'Rust 1.80 is too old' "$WORK/oldrust.log"
check grep -q 'rustup update stable' "$WORK/oldrust.log"

# Uninstall removes the app and the links and nothing else.
touch "$WORK/bin/keep-me"
PTZ_SKIP_CHECKS=1 PTZ_APP_DEST="$WORK/Applications" PTZ_BIN_DEST="$WORK/bin" \
  bash "$ROOT/scripts/macos-install.sh" --uninstall >"$WORK/uninstall.log"
check test ! -e "$APP"
for bin in ptz-server ptz-probe ptz-bringup ptz-panel; do
  check test ! -e "$WORK/bin/$bin"
done
check test -f "$WORK/bin/keep-me"
check grep -q 'Kept: the show data' "$WORK/uninstall.log"

echo "install script test passed"
