#!/usr/bin/env bash
# THE release gate. Asserts that the GitHub Release this workflow run just
# built actually contains everything a claude-fleet release promises, and
# that every byte of it is checksummed.
#
# Three assertions, all against the release's REAL assets (enumerated by
# numeric id — a draft release is not resolvable by tag, see
# scripts/upload-release-asset.sh), never against what a build job claims it
# uploaded:
#
#   1. every asset named by `scripts/release-assets.sh assets <version>` is
#      present — a failed or skipped leg is caught here, by its missing
#      output, whether or not the leg itself turned the run red;
#   2. nothing else is present — an asset under an unexpected name means the
#      naming contract drifted (that is exactly how the unversioned
#      `claude-fleet_x64.app.tar.gz` survived six releases), or a stale asset
#      from an earlier run of a renamed leg is still attached;
#   3. `SHA256SUMS` exists and has a line for every other asset, and names no
#      asset that is not on the release.
#
# WHY THIS JOB IS NOT `continue-on-error` WHEN THE BUILD LEGS ARE. Per-leg
# isolation is deliberate and stays: one arch failing must not cancel or
# delay another, so `agent-hub-binaries` and the checksums job keep
# `continue-on-error: true` and a failure there does not turn the run red by
# itself. That isolation is about BUILD legs; it must not extend to the
# RELEASE. This job is the one place that judges the release as a whole, it
# carries no `continue-on-error`, and it fails the run when the release is
# incomplete — so "green run" stops meaning "every leg that felt like
# running, ran" and starts meaning "this release is complete".
#
# Usage: REPO=owner/repo RELEASE_ID=123456 TAG=v0.3.0 verify-release.sh
# Needs `gh` authenticated via GH_TOKEN/GITHUB_TOKEN. Run from the repo root.
# Exit 0 when the release is complete, 1 otherwise (every problem is
# collected and printed before exiting, so one run reports all of them).
set -euo pipefail

: "${REPO:?REPO is required, e.g. owner/repo}"
: "${RELEASE_ID:?RELEASE_ID is required — the numeric release id, never a tag}"
: "${TAG:?TAG is required, e.g. v0.3.0}"

here="$(cd "$(dirname "$0")" && pwd)"
version="${TAG#v}"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

"$here/release-assets.sh" assets "$version" | LC_ALL=C sort >"$work/expected"
gh api --paginate "repos/$REPO/releases/$RELEASE_ID/assets" --jq '.[] | [.id, .name] | @tsv' >"$work/listing"
awk -F'\t' '{ print $2 }' "$work/listing" | LC_ALL=C sort >"$work/actual"

echo "verify-release.sh: release $RELEASE_ID ($TAG) carries $(wc -l <"$work/actual" | tr -d ' ') asset(s);" \
     "$(wc -l <"$work/expected" | tr -d ' ') expected" >&2

problems=0

report() {
  # `::error::` on stdout, because Actions only parses workflow commands out
  # of stdout — same reasoning as upload-release-asset.sh's annotation.
  echo "::error title=incomplete release::$1"
  problems=$((problems + 1))
}

missing="$(comm -23 "$work/expected" "$work/actual")"
if [ -n "$missing" ]; then
  while IFS= read -r m; do
    [ -n "$m" ] && report "$TAG is missing the asset '$m'"
  done <<<"$missing"
fi

unexpected="$(comm -13 "$work/expected" "$work/actual")"
if [ -n "$unexpected" ]; then
  while IFS= read -r u; do
    [ -n "$u" ] && report "$TAG carries '$u', which scripts/release-assets.sh does not declare for version $version"
  done <<<"$unexpected"
fi

# SHA256SUMS coverage. Fetched by id like every other asset; absent means the
# checksums job never got to upload it, which is itself a failure of the
# release, not a reason to skip the check.
sums_id="$(awk -F'\t' '$2 == "SHA256SUMS" { print $1; exit }' "$work/listing")"
if [ -z "$sums_id" ]; then
  report "$TAG has no SHA256SUMS asset, so none of its downloads can be verified"
else
  gh api -H "Accept: application/octet-stream" "repos/$REPO/releases/assets/$sums_id" >"$work/SHA256SUMS"
  awk '{ sub(/^\*/, "", $2); sub(/^\.\//, "", $2); print $2 }' "$work/SHA256SUMS" \
    | LC_ALL=C sort >"$work/listed"
  # SHA256SUMS cannot contain its own digest, so it is the one asset exempt
  # from needing a line.
  grep -v '^SHA256SUMS$' "$work/actual" >"$work/needs_sum" || true

  unchecksummed="$(comm -23 "$work/needs_sum" "$work/listed")"
  if [ -n "$unchecksummed" ]; then
    while IFS= read -r a; do
      [ -n "$a" ] && report "'$a' is published with no SHA256SUMS line"
    done <<<"$unchecksummed"
  fi

  phantom="$(comm -13 "$work/needs_sum" "$work/listed")"
  if [ -n "$phantom" ]; then
    while IFS= read -r p; do
      [ -n "$p" ] && report "SHA256SUMS lists '$p', which is not an asset of $TAG"
    done <<<"$phantom"
  fi
fi

if [ "$problems" -ne 0 ]; then
  echo "verify-release.sh: $TAG is NOT a complete release — $problems problem(s) above." >&2
  echo "verify-release.sh: fix the failing leg and re-run this workflow from the same tag;" \
       "every asset is reproducible from it, so a re-run is always a complete fix." >&2
  exit 1
fi

echo "verify-release.sh: $TAG is complete — all $(wc -l <"$work/expected" | tr -d ' ') expected assets present and checksummed." >&2
