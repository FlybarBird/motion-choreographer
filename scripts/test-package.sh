#!/usr/bin/env bash
# Run scripts/package.sh on any machine with the build stubbed, and check the
# installers it writes. The Linux installers are run for real, into a scratch
# folder. The Mac ones cannot be built off a Mac, so pkgbuild, productbuild and
# codesign are replaced by stand-ins that record what they were asked to do.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK" "$ROOT/target/package-test"' EXIT

check() {
  if ! "$@"; then
    printf 'package test failed: %s\n' "$*" >&2
    exit 1
  fi
}
refuse() {
  if "$@" >"$WORK/refused.log" 2>&1; then
    printf 'package test failed: this should have been refused: %s\n' "$*" >&2
    exit 1
  fi
}

# Stand-in release binaries, each printing its own name.
RELEASE="$ROOT/target/package-test/release"
mkdir -p "$RELEASE"
for bin in ptz-app ptz-server ptz-probe ptz-bringup ptz-panel; do
  printf '#!/bin/sh\necho %s\n' "$bin" >"$RELEASE/$bin"
  chmod +x "$RELEASE/$bin"
done
package() {
  PTZ_TARGET=package-test PTZ_SKIP_BUILD=1 PTZ_DIST="$WORK/dist-$1" PTZ_PACKAGE_OS="$1" bash "$ROOT/scripts/package.sh"
}
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -n 1)"
case "$(uname -m)" in
  x86_64 | amd64) ARCH=x86_64 ;;
  aarch64 | arm64) ARCH=arm64 ;;
  *) ARCH="$(uname -m)" ;;
esac

# ---------------------------------------------------------------- Linux

# The target name stands in for a triple, so the installers are named for this machine by hand.
package linux >"$WORK/linux.log"
mv "$WORK/dist-linux/ptz-programmer-$VERSION-linux-$(uname -m).run" "$WORK/app.run"
mv "$WORK/dist-linux/ptz-server-$VERSION-linux-$(uname -m).run" "$WORK/server.run"
check test "$(find "$WORK/dist-linux" -type f | wc -l)" -eq 0
sed -i.bak "s/^ARCH=\"$(uname -m)\"\$/ARCH=\"$ARCH\"/" "$WORK/app.run" "$WORK/server.run"
check test -x "$WORK/app.run"
check test -x "$WORK/server.run"

tools_in() {
  test -x "$1/bin/ptz-server" && test -x "$1/bin/ptz-probe" && test -x "$1/bin/ptz-bringup" && test -x "$1/bin/ptz-panel"
}
no_tools_in() {
  test ! -e "$1/bin/ptz-server" && test ! -e "$1/bin/ptz-probe" && test ! -e "$1/bin/ptz-bringup" && test ! -e "$1/bin/ptz-panel"
}

# The app alone: for a station that connects to a server elsewhere.
P="$WORK/app only"
sh "$WORK/app.run" --no-server --prefix "$P" >"$WORK/app-only.log"
check test "$("$P/bin/ptz-app")" = ptz-app
check no_tools_in "$P"
check grep -qF "Exec=\"$P/bin/ptz-app\"" "$P/share/applications/ptz-programmer.desktop"
check test -f "$P/share/ptz-programmer/licenses/LICENSE"
check test -f "$P/share/ptz-programmer/licenses/THIRD-PARTY-NOTICES.md"
check grep -q 'The show server was not installed' "$WORK/app-only.log"
check grep -q 'enter the address of the show server' "$WORK/app-only.log"

# Asked for, the server goes in beside it; a second run over the first also works.
sh "$WORK/app.run" --with-server --prefix "$P" >"$WORK/app-server.log"
check test "$("$P/bin/ptz-app")" = ptz-app
check tools_in "$P"
check test "$("$P/bin/ptz-server")" = ptz-server

# With nobody to ask, the app installer installs both.
P2="$WORK/unattended"
sh "$WORK/app.run" --prefix "$P2" </dev/null >"$WORK/unattended.log"
check test -x "$P2/bin/ptz-app"
check tools_in "$P2"

# Removing the app leaves the server, and anything else in the folder, alone.
touch "$P2/bin/keep-me"
sh "$WORK/app.run" --uninstall --prefix "$P2" >"$WORK/uninstall-app.log"
check test ! -e "$P2/bin/ptz-app"
check test ! -e "$P2/share/applications/ptz-programmer.desktop"
check tools_in "$P2"
check test -f "$P2/share/ptz-programmer/licenses/LICENSE"
check grep -q 'Kept: the show server' "$WORK/uninstall-app.log"
sh "$WORK/server.run" --uninstall --prefix "$P2" >"$WORK/uninstall-server.log"
check no_tools_in "$P2"
check test ! -e "$P2/share/ptz-programmer"
check test -f "$P2/bin/keep-me"
refuse sh "$WORK/server.run" --uninstall --prefix "$P2"
check grep -q 'nothing of PTZ Programmer to remove' "$WORK/refused.log"

# --uninstall --with-server takes both away in one go.
sh "$WORK/app.run" --uninstall --with-server --prefix "$P" >"$WORK/uninstall-both.log"
check test ! -e "$P/bin/ptz-app"
check no_tools_in "$P"
check test ! -e "$P/share/ptz-programmer"

# The server installer holds the server and nothing of the app.
P3="$WORK/server only"
sh "$WORK/server.run" --prefix "$P3" >"$WORK/server-only.log"
check tools_in "$P3"
check test ! -e "$P3/bin/ptz-app"
check test ! -e "$P3/share/applications"
check test -f "$P3/share/ptz-programmer/licenses/LICENSE"
refuse sh "$WORK/server.run" --no-server --prefix "$WORK/nowhere"
check test ! -e "$WORK/nowhere"
check test "$(tar -tzf <(tail -n "+$(awk '/^__FILES__$/ { print NR + 1; exit }' "$WORK/server.run")" "$WORK/server.run") | grep -c ptz-app)" -eq 0

# A damaged installer, an unknown option and the wrong kind of computer install nothing.
head -c "$(($(wc -c <"$WORK/app.run") - 200))" "$WORK/app.run" >"$WORK/cut.run"
refuse sh "$WORK/cut.run" --with-server --prefix "$WORK/nowhere"
check grep -q 'damaged' "$WORK/refused.log"
refuse sh "$WORK/app.run" --frobnicate --prefix "$WORK/nowhere"
sed 's/^ARCH=".*"$/ARCH="sparc"/' "$WORK/app.run" >"$WORK/sparc.run"
refuse sh "$WORK/sparc.run" --with-server --prefix "$WORK/nowhere"
check grep -q 'is for sparc computers' "$WORK/refused.log"
check test ! -e "$WORK/nowhere"
sh "$WORK/app.run" --help >"$WORK/help.txt"
check grep -q -- '--no-server' "$WORK/help.txt"

# ---------------------------------------------------------------- macOS, with stand-in tools

STUBS="$WORK/stubs"
CALLS="$WORK/calls"
mkdir -p "$STUBS" "$CALLS"
cat >"$STUBS/pkgbuild" <<'STUB'
#!/bin/sh
# Records the arguments, keeps a copy of what would be packed, and writes the output file.
log="$CALLS_DIR/pkgbuild-$(basename "$(eval "echo \${$#}")" .pkg)"
printf '%s\n' "$@" >"$log.args"
while [ $# -gt 1 ]; do
  case "$1" in
    --root) cp -R "$2" "$log.root" ;;
    --component-plist) cp "$2" "$log.component.plist" ;;
    --scripts) cp -R "$2" "$log.scripts" ;;
  esac
  shift
done
: >"$1"
STUB
cat >"$STUBS/productbuild" <<'STUB'
#!/bin/sh
log="$CALLS_DIR/productbuild-$(basename "$(eval "echo \${$#}")" .pkg)"
printf '%s\n' "$@" >"$log.args"
while [ $# -gt 1 ]; do
  case "$1" in
    --distribution) cp "$2" "$log.distribution.xml" ;;
    --package-path) ls "$2" >"$log.packages" ;;
  esac
  shift
done
echo product >"$1"
STUB
cat >"$STUBS/pkgutil" <<'STUB'
#!/bin/sh
# --expand <installer> <folder>: lays out what the stand-in productbuild was given.
[ "$1" = --expand ] || exit 2
log="$CALLS_DIR/productbuild-$(basename "$2" .pkg)"
mkdir -p "$3"
cp "$log.distribution.xml" "$3/Distribution"
grep -o '>[a-z]*\.pkg<' "$log.distribution.xml" | tr -d '<>' | while read -r part; do mkdir -p "$3/$part"; done
STUB
cat >"$STUBS/codesign" <<'STUB'
#!/bin/sh
printf '%s\n' "$*" >>"$CALLS_DIR/codesign.args"
STUB
chmod +x "$STUBS"/*
cp "$STUBS/pkgutil" "$STUBS/pkgutil.good"

CALLS_DIR="$CALLS" PATH="$STUBS:$PATH" package macos >"$WORK/macos.log"
APP_PKG="PTZ-Programmer-$VERSION-macos-$(uname -m)"
SERVER_PKG="PTZ-Server-$VERSION-macos-$(uname -m)"
check test -f "$WORK/dist-macos/$APP_PKG.pkg"
check test -f "$WORK/dist-macos/$SERVER_PKG.pkg"

# The app goes to /Applications, there and nowhere else, and carries no server.
check grep -qx -- '/Applications' "$CALLS/pkgbuild-app.args"
check grep -qx -- 'com.ptzprogrammer.app' "$CALLS/pkgbuild-app.args"
BUNDLE="$CALLS/pkgbuild-app.root/PTZ Programmer.app"
check test "$("$BUNDLE/Contents/MacOS/ptz-app")" = ptz-app
check test "$(find "$BUNDLE/Contents/MacOS" -type f | wc -l)" -eq 1
check test -f "$BUNDLE/Contents/Resources/licenses/LICENSE"
check grep -q "<string>$VERSION</string>" "$BUNDLE/Contents/Info.plist"
check grep -q 'NSMicrophoneUsageDescription' "$BUNDLE/Contents/Info.plist"
grep -A1 'BundleIsRelocatable' "$CALLS/pkgbuild-app.component.plist" >"$WORK/relocatable.txt"
check grep -q '<false/>' "$WORK/relocatable.txt"

# The server and tools go to their own folder, which a script puts on PATH.
check grep -qx -- '/usr/local/ptz-programmer' "$CALLS/pkgbuild-server.args"
check grep -qx -- 'com.ptzprogrammer.server' "$CALLS/pkgbuild-server.args"
for bin in ptz-server ptz-probe ptz-bringup ptz-panel; do
  check test "$("$CALLS/pkgbuild-server.root/bin/$bin")" = "$bin"
done
check test ! -e "$CALLS/pkgbuild-server.root/bin/ptz-app"
check test -f "$CALLS/pkgbuild-server.root/licenses/LICENSE"
check test -x "$CALLS/pkgbuild-server.scripts/postinstall"
check grep -q '/usr/local/ptz-programmer/bin. > /etc/paths.d/ptz-programmer' "$CALLS/pkgbuild-server.scripts/postinstall"
check sh -n "$CALLS/pkgbuild-server.scripts/postinstall"

# The app and each tool are signed, ad hoc unless an identity is given.
check test "$(grep -c -- '--sign -' "$CALLS/codesign.args")" -eq 5

# The app installer offers the server as a choice that can be cleared; the app itself cannot be.
APP_DIST="$CALLS/productbuild-$APP_PKG.distribution.xml"
SERVER_DIST="$CALLS/productbuild-$SERVER_PKG.distribution.xml"
if command -v xmllint >/dev/null 2>&1; then
  check xmllint --noout "$APP_DIST" "$SERVER_DIST" "$BUNDLE/Contents/Info.plist" "$CALLS/pkgbuild-app.component.plist"
fi
check grep -q 'customize="always"' "$APP_DIST"
check grep -q '<choice id="app" .*start_selected="true" enabled="false">' "$APP_DIST"
check grep -q '<choice id="server" .*start_selected="true">' "$APP_DIST"
check test "$(grep -c '<choice id="server" .*enabled="false"' "$APP_DIST")" -eq 0
# "selected" would keep ticking the server again after the user cleared it.
check test "$(grep -c ' selected=' "$APP_DIST")" -eq 0
check grep -q 'enable_anywhere="false" enable_currentUserHome="false" enable_localSystem="true"' "$APP_DIST"
check grep -q "hostArchitectures=" "$APP_DIST"
check grep -q '>app.pkg</pkg-ref>' "$APP_DIST"
check grep -q '>server.pkg</pkg-ref>' "$APP_DIST"
check grep -qx 'app.pkg' "$CALLS/productbuild-$APP_PKG.packages"
check grep -qx 'server.pkg' "$CALLS/productbuild-$APP_PKG.packages"
# The server installer holds the server and nothing of the app.
check test "$(grep -c 'app.pkg' "$SERVER_DIST")" -eq 0
check grep -q '<choice id="server" .*enabled="false">' "$SERVER_DIST"
check test "$(grep -c -- '--sign' "$CALLS/productbuild-$APP_PKG.args")" -eq 0
check grep -q 'not signed with a Developer ID' "$WORK/macos.log"

# An installer that comes out without one of its parts stops the build.
cat >"$STUBS/pkgutil" <<'STUB'
#!/bin/sh
mkdir -p "$3/app.pkg"
: >"$3/Distribution"
STUB
refuse env CALLS_DIR="$CALLS" PATH="$STUBS:$PATH" PTZ_TARGET=package-test PTZ_SKIP_BUILD=1 PTZ_DIST="$WORK/dist-broken" \
  PTZ_PACKAGE_OS=macos bash "$ROOT/scripts/package.sh"
check grep -q 'is missing its server part' "$WORK/refused.log"
mv "$STUBS/pkgutil.good" "$STUBS/pkgutil"

# With an installer identity both packages are signed with it.
rm -rf "${CALLS:?}"/*
CALLS_DIR="$CALLS" PATH="$STUBS:$PATH" PTZ_INSTALLER_SIGN="Developer ID Installer: Example" PTZ_APP_SIGN="Developer ID Application: Example" \
  package macos >"$WORK/macos-signed.log"
check grep -qx 'Developer ID Installer: Example' "$CALLS/productbuild-$APP_PKG.args"
check grep -qx 'Developer ID Installer: Example' "$CALLS/productbuild-$SERVER_PKG.args"
check test "$(grep -c -- '--sign Developer ID Application: Example' "$CALLS/codesign.args")" -eq 5

# Off a Mac, without the stand-ins, it says where to run it instead of failing half-way.
if ! command -v pkgbuild >/dev/null 2>&1; then
  refuse package macos
  check grep -q 'run this on a Mac' "$WORK/refused.log"
fi

echo "package script test passed"
