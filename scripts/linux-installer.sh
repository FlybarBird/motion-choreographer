#!/bin/sh
# PTZ Programmer @VERSION@ for Linux (@ARCH@): the installer for the @KIND@.
# scripts/package.sh fills in the three names above and appends the files, as a
# tar.gz, after the last line of this script.
set -eu

KIND="@KIND@"
VERSION="@VERSION@"
ARCH="@ARCH@"
PREFIX="${HOME}/.local"
SERVER=ask
ACTION=install
TOOLS="ptz-server ptz-probe ptz-bringup"

say() { printf '==> %s\n' "$*"; }
fail() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

usage() {
  if [ "$KIND" = app ]; then
    cat <<HELP
PTZ Programmer $VERSION: installs the desktop app, and the show server if you want it here.

  sh $(basename "$0") [--with-server | --no-server] [--prefix DIR]
  sh $(basename "$0") --uninstall [--with-server] [--prefix DIR]

  --with-server   install the show server and its tools too, without asking
  --no-server     install the app only: for a station that connects to a server elsewhere
  --prefix DIR    install under DIR instead of ~/.local
  --uninstall     remove the app; with --with-server, the server and tools as well

The show data (~/PTZProgrammer) and the app's settings are never touched.
HELP
  else
    cat <<HELP
PTZ Programmer $VERSION: installs the show server and its tools (ptz-server, ptz-probe, ptz-bringup).

  sh $(basename "$0") [--prefix DIR]
  sh $(basename "$0") --uninstall [--prefix DIR]

  --prefix DIR    install under DIR instead of ~/.local
  --uninstall     remove the server and tools

The show data (~/PTZProgrammer) is never touched.
HELP
  fi
}

while [ $# -gt 0 ]; do
  case "$1" in
    --prefix)
      [ $# -ge 2 ] || fail "--prefix needs a folder"
      PREFIX="$2"
      shift
      ;;
    --prefix=*) PREFIX="${1#--prefix=}" ;;
    --with-server) SERVER=yes ;;
    --no-server) SERVER=no ;;
    --uninstall) ACTION=uninstall ;;
    --help | -h)
      usage
      exit 0
      ;;
    *) fail "unknown option $1. See --help" ;;
  esac
  shift
done
[ -n "$PREFIX" ] || fail "--prefix needs a folder"
case "$PREFIX" in
  /*) ;;
  *) PREFIX="$(pwd)/$PREFIX" ;;
esac
if [ "$KIND" = server ]; then
  [ "$SERVER" = ask ] || fail "this installer holds the server only. See --help"
  SERVER=yes
fi

BIN="$PREFIX/bin"
SHARE="$PREFIX/share/ptz-programmer"
DESKTOP="$PREFIX/share/applications/ptz-programmer.desktop"

# Each part keeps a list of what it installed, so that removing it removes exactly that.
remove_part() {
  list="$SHARE/installed-$1.txt"
  [ -f "$list" ] || return 1
  while IFS= read -r file; do
    rm -f "$file"
  done <"$list"
  rm -f "$list"
  return 0
}

if [ "$ACTION" = uninstall ]; then
  removed=""
  if [ "$KIND" = app ] && remove_part app; then removed="the app"; fi
  if [ "$SERVER" = yes ] && remove_part server; then removed="${removed:+$removed and }the show server and tools"; fi
  if [ ! -f "$SHARE/installed-app.txt" ] && [ ! -f "$SHARE/installed-server.txt" ]; then
    rm -rf "$SHARE"
  fi
  [ -n "$removed" ] || fail "nothing of PTZ Programmer to remove was found under $PREFIX"
  say "Removed $removed from $PREFIX"
  if [ "$KIND" = app ] && [ -f "$SHARE/installed-server.txt" ]; then
    say "Kept: the show server and tools. Add --with-server to remove them too"
  fi
  say "Kept: the show data (~/PTZProgrammer unless you chose another folder) and the app's settings"
  exit 0
fi

case "$(uname -m)" in
  x86_64 | amd64) machine=x86_64 ;;
  aarch64 | arm64) machine=arm64 ;;
  *) machine="$(uname -m)" ;;
esac
[ "$machine" = "$ARCH" ] || fail "this installer is for $ARCH computers; this one is $machine"

if [ "$SERVER" = ask ]; then
  SERVER=yes
  if [ -t 0 ]; then
    printf 'Install the show server on this computer too? It is needed on the machine that runs the show. [Y/n] '
    read -r answer || answer=""
    case "$answer" in
      n* | N*) SERVER=no ;;
    esac
  fi
fi

# Unpack everything first: a damaged file must not leave half an install behind.
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
start="$(awk '/^__FILES__$/ { print NR + 1; exit }' "$0")"
[ -n "$start" ] || fail "this installer is damaged: it holds no files"
tail -n "+$start" "$0" | tar -xzf - -C "$WORK" 2>/dev/null || fail "this installer is damaged. Copy or download it again"

mkdir -p "$BIN" "$SHARE/licenses"
cp "$WORK"/licenses/* "$SHARE/licenses/"

# put <part> <file> <destination>: copied beside the destination, then moved over it, which
# also works while the old program is running.
put() {
  cp "$2" "$3.new"
  chmod 755 "$3.new"
  mv -f "$3.new" "$3"
  printf '%s\n' "$3" >>"$SHARE/installed-$1.txt.new"
}

missing_libraries() {
  if command -v ldd >/dev/null 2>&1; then
    ldd "$1" 2>/dev/null | awk '/not found/ { print $1 }' | sort -u | tr '\n' ' '
  fi
}

if [ "$KIND" = app ]; then
  : >"$SHARE/installed-app.txt.new"
  put app "$WORK/app/bin/ptz-app" "$BIN/ptz-app"
  mkdir -p "$(dirname "$DESKTOP")"
  cat >"$DESKTOP" <<ENTRY
[Desktop Entry]
Type=Application
Name=PTZ Programmer
Comment=Timeline programming and playout for PTZ cameras
Exec="$BIN/ptz-app"
Terminal=false
Categories=AudioVideo;Video;
ENTRY
  printf '%s\n' "$DESKTOP" >>"$SHARE/installed-app.txt.new"
  mv -f "$SHARE/installed-app.txt.new" "$SHARE/installed-app.txt"
  say "Installed PTZ Programmer $VERSION: $BIN/ptz-app"
  missing="$(missing_libraries "$BIN/ptz-app")"
  [ -z "$missing" ] || say "ptz-app needs libraries this computer does not have: $missing"
fi

if [ "$SERVER" = yes ]; then
  if command -v pgrep >/dev/null 2>&1 && pgrep -x ptz-server >/dev/null 2>&1; then
    say "A ptz-server is running. It keeps running the version it was started from until you restart it"
  fi
  : >"$SHARE/installed-server.txt.new"
  for tool in $TOOLS; do
    put server "$WORK/server/bin/$tool" "$BIN/$tool"
  done
  mv -f "$SHARE/installed-server.txt.new" "$SHARE/installed-server.txt"
  say "Installed the show server and tools: $BIN/ptz-server, ptz-probe, ptz-bringup"
  if ! "$BIN/ptz-server" --version >/dev/null 2>&1; then
    missing="$(missing_libraries "$BIN/ptz-server")"
    say "ptz-server does not start on this computer.${missing:+ It needs libraries that are missing: $missing}"
  fi
else
  say "The show server was not installed. Run this again with --with-server to add it"
fi

# shellcheck disable=SC2016  # $PATH is meant to reach the user's shell profile unexpanded
case ":$PATH:" in
  *":$BIN:"*) ;;
  *) printf '\n%s is not on your PATH. Add this line to your shell profile:\n  export PATH="%s:$PATH"\n' "$BIN" "$BIN" ;;
esac

printf '\nNext:\n'
if [ "$SERVER" = yes ]; then
  printf '  ptz-server      start the show server (data in ~/PTZProgrammer, simulator rig to begin with)\n'
fi
if [ "$KIND" = app ]; then
  printf '  ptz-app         open the app, or choose PTZ Programmer in your applications menu\n'
  if [ "$SERVER" != yes ]; then
    printf '                  then enter the address of the show server on the connect screen\n'
  fi
fi
exit 0
# shellcheck disable=SC2317  # not a command: the files follow this line
__FILES__
