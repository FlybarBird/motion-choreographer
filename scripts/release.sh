#!/usr/bin/env bash
# Publish a new version of Motion Choreographer: every installed copy then offers it as an update.
#
#   scripts/release.sh <version> ["what is new"]
#   scripts/release.sh 0.1.1 "A scroll bar along the bottom of the timeline."
#
# It sets the version, builds the installers, signs a description of them, commits and tags the
# version, pushes it, and puts the installers on GitHub as a release. Run it on the Mac that holds
# the release keys, on main, with nothing uncommitted.
#
# What it needs:
#   ~/.ptz-release/signing-key.pem   signs the release (made by: node updates/tools/release-tool.mjs keys)
#   ~/.ptz-release/update-key        what the apps send to the update service
#   updates/service-url              the address of the update service (see "Updates" in README.md)
#   gh                               signed in to GitHub with write access to the repository
#
# Environment (all optional):
#   PTZ_RELEASE_HOME   where the keys are (default ~/.ptz-release)
#   PTZ_RELEASE_REPO   the GitHub repository (default: the one in updates/wrangler.jsonc)
#   PTZ_REMOTES        the git remotes to push to (default "origin github")
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

say() { printf '==> %s\n' "$*"; }
fail() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

VERSION="${1:-}"
NOTES="${2:-}"
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || fail "give the new version as three numbers, like 0.1.1"
[ "$(uname -s)" = Darwin ] || fail "releases are made on a Mac: the update is a Mac installer"

KEYS="${PTZ_RELEASE_HOME:-$HOME/.ptz-release}"
REPO="${PTZ_RELEASE_REPO:-$(sed -n 's/.*"GITHUB_REPO": *"\(.*\)".*/\1/p' updates/wrangler.jsonc | head -n 1)}"
REMOTES="${PTZ_REMOTES:-origin github}"
TAG="v$VERSION"

# 1. Everything that could stop it, before anything is changed.
for tool in cargo node gh git; do
  command -v "$tool" >/dev/null 2>&1 || fail "$tool is missing"
done
[ -s "$KEYS/signing-key.pem" ] && [ -s "$KEYS/update-key" ] ||
  fail "the release keys are not in $KEYS. They are made once, with: node updates/tools/release-tool.mjs keys"
[ -s updates/service-url ] || fail "updates/service-url is missing: the update service has not been set up (see "Updates" in README.md)"
SERVICE="$(tr -d '[:space:]' <updates/service-url)"
[[ "$SERVICE" =~ ^https://[a-z0-9.-]+$ ]] || fail "updates/service-url should be one https address with no path"
[ -n "$REPO" ] || fail "could not read the GitHub repository from updates/wrangler.jsonc"

[ "$(git rev-parse --abbrev-ref HEAD)" = main ] || fail "releases are made from main"
[ -z "$(git status --porcelain)" ] || fail "there are uncommitted changes. Commit them or put them aside first"
if git rev-parse -q --verify "refs/tags/$TAG" >/dev/null; then fail "version $VERSION has been released already"; fi
for remote in $REMOTES; do
  git remote get-url "$remote" >/dev/null 2>&1 || fail "there is no git remote called $remote"
  git fetch --quiet "$remote" main
  git merge-base --is-ancestor FETCH_HEAD HEAD || fail "$remote has commits that are not here. Pull them first"
done
# Asked of the repository itself: "gh auth status" also fails for a second account that is signed out.
gh repo view "$REPO" --json name >/dev/null 2>&1 ||
  fail "gh cannot reach $REPO on GitHub. Sign in with: gh auth login"
if gh release view "$TAG" --repo "$REPO" >/dev/null 2>&1; then fail "GitHub already has a release $TAG"; fi

NOW="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)"
[ "$(printf '%s\n%s\n' "$NOW" "$VERSION" | sort -t. -k1,1n -k2,2n -k3,3n | tail -n 1)" = "$VERSION" ] && [ "$NOW" != "$VERSION" ] ||
  fail "this is version $NOW, so the next one has to be later than that"
[ -n "$NOTES" ] || NOTES="Version $VERSION."

# The service has to be there, or the copies that are built could never update.
ANSWER="$(printf 'header = "x-ptz-key: %s"\n' "$(cat "$KEYS/update-key")" |
  curl --silent --max-time 20 --config - --output /dev/null --write-out '%{http_code}' "$SERVICE/v1/latest/macos-arm64" || true)"
case "$ANSWER" in
  200 | 404) ;;
  401) fail "the update service refused the update key. Set it again (see "Updates" in README.md)" ;;
  000 | "") fail "the update service at $SERVICE could not be reached" ;;
  *) fail "the update service answered $ANSWER. Run this for its reason: (cd updates && npx wrangler tail)" ;;
esac

# 2. Set the version, and put it back if anything fails before it is committed.
undo() {
  git checkout --quiet -- Cargo.toml Cargo.lock
  printf 'error: nothing was released. The version is %s again\n' "$NOW" >&2
}
trap undo ERR
say "Version $NOW becomes $VERSION"
sed -i '' "1,/^version = \".*\"/s/^version = \".*\"/version = \"$VERSION\"/" Cargo.toml
cargo update --workspace --offline --quiet

# 3. Build the installers. The address and the key are built into the app, which is what makes it
# a copy that updates itself.
OUT="$(mktemp -d)"
PTZ_UPDATE_URL="$SERVICE" PTZ_UPDATE_KEY="$(cat "$KEYS/update-key")" PTZ_DIST="$OUT" bash scripts/package.sh
APP_PKG="$(ls "$OUT"/Motion-Choreographer-"$VERSION"-macos-*.pkg)"
SERVER_PKG="$(ls "$OUT"/PTZ-Server-"$VERSION"-macos-*.pkg)"
PLATFORM="$(basename "$APP_PKG" .pkg | sed "s/^Motion-Choreographer-$VERSION-//")"

# 4. Describe the release and sign the description.
printf '%s\n' "$NOTES" >"$OUT/notes.txt"
PTZ_RELEASE_HOME="$KEYS" node updates/tools/release-tool.mjs describe "$VERSION" "$PLATFORM" "$APP_PKG" "$OUT/notes.txt" "$OUT"

# 5. Commit, tag and push. From here on the version stays set.
git add Cargo.toml Cargo.lock
git commit --quiet -m "Version $VERSION"
trap - ERR
git tag -a "$TAG" -m "Version $VERSION"
for remote in $REMOTES; do
  say "Pushing to $remote"
  git push --quiet "$remote" main "$TAG"
done

# 6. Publish. Once this is done every installed copy offers the update.
say "Publishing $TAG on GitHub"
gh release create "$TAG" --repo "$REPO" --title "Motion Choreographer $VERSION" --notes-file "$OUT/notes.txt" --latest \
  "$APP_PKG" "$SERVER_PKG" "$OUT/update-$PLATFORM.json" "$OUT/update-$PLATFORM.json.sig" >/dev/null ||
  fail "version $VERSION is committed, tagged and pushed, but GitHub did not take the release. Publish it by hand from $OUT"

mkdir -p dist
cp "$APP_PKG" "$SERVER_PKG" dist/
rm -rf "$OUT"
say "Version $VERSION is published. Copies of the app offer it the next time they look, within six hours, or at once from View, Updates."
