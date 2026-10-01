#!/usr/bin/env bash
# Release sqlite_spect: bump the pubspec version, prepend a CHANGELOG entry,
# commit, tag, and push. CI (.github/workflows/release.yml) builds everything
# and publishes to GitHub Releases + pub.dev.
#
# Usage (from anywhere in the repo):
#   ./release.sh 0.2.0
#   ./release.sh 0.2.0 "What's new in one line"
set -euo pipefail

VERSION="${1:-}"
NOTES="${2:-}"

die() { echo "release: $*" >&2; exit 1; }

[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "usage: ./release.sh <X.Y.Z> [release-note]"

cd "$(git rev-parse --show-toplevel)"

# --- guards -----------------------------------------------------------------
[[ -z "$(git status --porcelain --untracked-files=no)" ]] \
  || die "tracked files are modified — commit or stash first"
BRANCH="$(git branch --show-current)"
[[ "$BRANCH" == "main" ]] || die "on branch '$BRANCH' — releases go out from main"
git fetch -q origin
[[ "$(git rev-parse HEAD)" == "$(git rev-parse origin/main)" ]] \
  || die "main is out of sync with origin/main — pull/push first"
git rev-parse -q --verify "refs/tags/v$VERSION" >/dev/null \
  && die "tag v$VERSION already exists locally"
git ls-remote --exit-code --tags origin "refs/tags/v$VERSION" >/dev/null 2>&1 \
  && die "tag v$VERSION already exists on origin"

PUBSPEC="dart/sqlite_spect/pubspec.yaml"
CHANGELOG="dart/sqlite_spect/CHANGELOG.md"
CURRENT="$(grep -E '^version:' "$PUBSPEC" | head -1 | awk '{print $2}')"
[[ "$CURRENT" == "$VERSION" ]] && die "pubspec is already at $VERSION"

echo "Releasing sqlite_spect $CURRENT -> $VERSION"
read -r -p "Continue? [y/N] " REPLY
case "$REPLY" in
  y|Y) ;;
  *) die "aborted" ;;
esac

# --- bump -------------------------------------------------------------------
sed -i.bak "s/^version: .*/version: $VERSION/" "$PUBSPEC" && rm -f "$PUBSPEC.bak"
NOTE_LINE="${NOTES:-See the [GitHub release](https://github.com/farmery/sqlite_spect/releases/tag/v$VERSION) for details.}"
printf '## %s\n\n* %s\n\n' "$VERSION" "$NOTE_LINE" \
  | cat - "$CHANGELOG" > "$CHANGELOG.new" && mv "$CHANGELOG.new" "$CHANGELOG"

git add "$PUBSPEC" "$CHANGELOG"
git commit -q -m "chore: release v$VERSION"
git tag -a "v$VERSION" -m "sqlite_spect v$VERSION"
git push origin main "v$VERSION"

echo
echo "Released v$VERSION — CI is building now:"
echo "  https://github.com/farmery/sqlite_spect/actions"
echo "It will create the GitHub release (CLI binaries + SHA256SUMS) and"
echo "publish $VERSION to pub.dev automatically."
