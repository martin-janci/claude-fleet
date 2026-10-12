#!/usr/bin/env bash
# cut-nightly.sh — cut a nightly release of a green `main` commit
# (update-channel design §5, slice S2b). Run by .github/workflows/nightly.yml.
#
#   scripts/cut-nightly.sh <sha> [--desktop] [--version-only]
#
# The version is the next patch after the newest stable tag, as a pre-release
# that sorts by the number of commits since that tag:
#
#   0.5.5-dev.17.g1a2b3c4            per push: the hub image and the tarballs
#   0.5.5-dev.17.desktop.g1a2b3c4    once a day: every leg, the desktop too
#
# (`scripts/release-assets.sh has-desktop` reads the difference back.) The
# release commit is scripts/release.sh's own — the version carriers, Cargo.lock
# and a CHANGELOG section — made on top of <sha> and NEVER pushed to a branch:
# only its tag is pushed, and release.yml / hub-image.yml are dispatched at it
# (a tag pushed with the workflow token starts no workflow by itself).
# update-channels.sh then lists it on `nightly` once it is published.
#
#   env: GH_TOKEN (dispatch) DRY_RUN=1 (commit and tag locally, push nothing)
#
# Prints the version on stdout. Idempotent: a version whose tag exists is
# printed and nothing else happens.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
cd "$here/.."

sha="${1:?usage: cut-nightly.sh <sha> [--desktop] [--version-only]}"
shift
desktop="" version_only=""
for a in "$@"; do
  case "$a" in
    --desktop) desktop=1 ;;
    --version-only) version_only=1 ;;
    *) echo "cut-nightly.sh: unknown argument '$a'" >&2; exit 2 ;;
  esac
done
case "$sha" in *[!0-9a-f]* | '') echo "cut-nightly.sh: not a commit sha: '$sha'" >&2; exit 2 ;; esac

git fetch -q --tags origin
full="$(git rev-parse --verify "$sha^{commit}")"
short="$(git rev-parse --short=7 "$full")"

# The newest STABLE tag reachable from the commit (an -rc or a -dev is not a base).
base_tag="$(git describe --tags --abbrev=0 --match 'v[0-9]*.[0-9]*.[0-9]*' --exclude '*-*' "$full")"
base="${base_tag#v}"
IFS=. read -r major minor patch <<<"$base"
n="$(git rev-list --count "$base_tag..$full")"
[ "$n" -gt 0 ] || { echo "cut-nightly.sh: $short is $base_tag itself; nothing to cut" >&2; exit 0; }
version="$major.$minor.$((patch + 1))-dev.$n.${desktop:+desktop.}g$short"

echo "$version"
[ -z "$version_only" ] || exit 0
if git rev-parse -q --verify "refs/tags/v$version" >/dev/null; then
  echo "cut-nightly.sh: v$version exists already" >&2
  exit 0
fi

# release.sh insists on a clean `main` that is origin's tip and CI-green. The
# workflow checked both for <sha> already; the local branch is named main only
# to satisfy it, and is never pushed.
git checkout -q -B main "$full"
RELEASE_ALLOW_BEHIND_ORIGIN=1 RELEASE_SKIP_CI_CHECK=1 \
  GIT_AUTHOR_NAME="github-actions[bot]" GIT_AUTHOR_EMAIL="41898282+github-actions[bot]@users.noreply.github.com" \
  GIT_COMMITTER_NAME="github-actions[bot]" GIT_COMMITTER_EMAIL="41898282+github-actions[bot]@users.noreply.github.com" \
  scripts/release.sh "$version" </dev/null >&2

if [ "${DRY_RUN:-0}" = 1 ]; then
  echo "cut-nightly.sh: DRY_RUN — tagged v$version locally, pushed nothing" >&2
  exit 0
fi
git push -q origin "refs/tags/v$version"
for wf in release.yml hub-image.yml; do
  gh workflow run "$wf" --ref "v$version" >&2
done
echo "cut-nightly.sh: pushed v$version and dispatched release.yml and hub-image.yml at it" >&2
