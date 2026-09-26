#!/usr/bin/env bash
# Tag fleet-mobile with the claude-fleet release it ships alongside.
# Usage: scripts/release-mobile.sh <X.Y.Z>
# Env:   RELEASE_DRY_RUN=1   resolve and check everything, create nothing
#        FLEET_MOBILE_REPO   owner/name of the phone app (default below)
# Needs: gh (logged in with push rights to fleet-mobile). See docs/RELEASING.md.
#
# The tag `vX.Y.Z` on fleet-mobile's `main` head is what starts that repo's
# own release.yml: test, build, sign with the release key, publish a GitHub
# Release with the APK. Nothing in claude-fleet builds the phone app; this
# script only decides WHICH phone commit a claude-fleet release goes out
# with, under the same version, so the pair is one release.
#
# It refuses, rather than tagging something it cannot vouch for, when:
#   - claude-fleet has no tag vX.Y.Z on GitHub yet (push it first — the
#     phone release follows the fleet one, never leads it);
#   - fleet-mobile's CI has not passed on its `main` head;
#   - fleet-mobile already has vX.Y.Z at a DIFFERENT commit. The same
#     commit is a no-op, so a re-run after a network hiccup is safe.
#
# The ref is created through the API under the caller's own gh token, not a
# workflow's GITHUB_TOKEN — a tag pushed with the latter would not trigger
# fleet-mobile's release workflow.
set -euo pipefail

FLEET_REPO="martin-janci/claude-fleet"
MOBILE_REPO="${FLEET_MOBILE_REPO:-martin-janci/fleet-mobile}"

die() { echo "release-mobile.sh: $*" >&2; exit 1; }

[[ $# -eq 1 ]] || die "usage: scripts/release-mobile.sh <X.Y.Z>"
NEW="$1"
[[ "$NEW" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "'$NEW' is not a plain semver X.Y.Z"
TAG="v$NEW"

# The sha a tag ref points at, or "" when there is no such tag. On a 404
# `gh api` still prints the error body to stdout and exits non-zero, so the
# output only counts when it is a sha; anything else is "no tag". (An
# annotated tag yields its tag object's sha — fine for "does it exist", and
# this script only ever creates lightweight tags, which yield the commit.)
ref_sha() {
  local out
  out="$(gh api "repos/$1/git/ref/tags/$2" --jq '.object.sha' 2>/dev/null || true)"
  [[ "$out" =~ ^[0-9a-f]{40}$ ]] && echo "$out" || true
}

[[ -n "$(ref_sha "$FLEET_REPO" "$TAG")" ]] \
  || die "$FLEET_REPO has no tag $TAG — push the claude-fleet release first"

HEAD_SHA="$(gh api "repos/$MOBILE_REPO/commits/main" --jq '.sha')"
[[ "$HEAD_SHA" =~ ^[0-9a-f]{40}$ ]] || die "could not resolve $MOBILE_REPO main (got '$HEAD_SHA')"

EXISTING="$(ref_sha "$MOBILE_REPO" "$TAG")"
if [[ -n "$EXISTING" ]]; then
  [[ "$EXISTING" == "$HEAD_SHA" ]] \
    || die "$MOBILE_REPO already has $TAG at ${EXISTING:0:12}, not main (${HEAD_SHA:0:12}) — resolve by hand"
  echo "$MOBILE_REPO $TAG already points at main ${HEAD_SHA:0:12}; nothing to do"
  exit 0
fi

# The newest CI run for exactly this commit — an older green run on another
# commit says nothing about this one.
CI="$(gh run list -R "$MOBILE_REPO" --workflow ci.yml --commit "$HEAD_SHA" -L 1 \
  --json status,conclusion --jq '.[0] | "\(.status) \(.conclusion)"' 2>/dev/null || true)"
[[ "$CI" == "completed success" ]] \
  || die "$MOBILE_REPO CI on main ${HEAD_SHA:0:12} is '${CI:-no run}', not 'completed success' — wait for it or fix it"

echo "$MOBILE_REPO main ${HEAD_SHA:0:12}: CI green"
if [[ -n "${RELEASE_DRY_RUN:-}" ]]; then
  echo "Dry run: would create $TAG on $MOBILE_REPO at ${HEAD_SHA:0:12}"; exit 0
fi

gh api -X POST "repos/$MOBILE_REPO/git/refs" -f ref="refs/tags/$TAG" -f sha="$HEAD_SHA" >/dev/null
echo "Created $TAG on $MOBILE_REPO at ${HEAD_SHA:0:12}; its release workflow builds the signed APK:"
echo "  gh run list -R $MOBILE_REPO --workflow release.yml -L 1"
echo "  https://github.com/$MOBILE_REPO/releases/tag/$TAG"
