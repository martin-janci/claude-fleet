#!/usr/bin/env bash
# Downloads every asset currently attached to a GitHub Release into a
# directory, BY NUMERIC ASSET ID.
#
# This is the "enumerated from the release itself" half of SHA256SUMS: the
# checksums job no longer hashes only the four tarballs its own matrix legs
# happened to build, it hashes whatever the release actually carries —
# desktop bundles uploaded by tauri-action included. Nothing here knows or
# cares what the assets are called; scripts/verify-release.sh is what asserts
# the set is the expected one.
#
# By id, not by tag and not by name, for the same reason as
# scripts/upload-release-asset.sh: a DRAFT release is not resolvable through
# the "get release by tag" endpoint, and `gh release download` goes through
# exactly that. `GET /repos/{owner}/{repo}/releases/assets/{id}` with
# `Accept: application/octet-stream` returns the bytes of a draft's asset.
#
# `SHA256SUMS` itself is skipped: it is an output of the job that calls this,
# and a checksums file cannot list its own digest.
#
# Usage: REPO=owner/repo RELEASE_ID=123456 download-release-assets.sh <outdir>
# Writes <outdir>/<asset name> for each asset and prints the names on stdout.
# Exit 0 with no output means the release has no assets yet.
set -euo pipefail

: "${REPO:?REPO is required, e.g. owner/repo}"
: "${RELEASE_ID:?RELEASE_ID is required — the numeric release id, never a tag}"
out="${1:?usage: REPO=... RELEASE_ID=... download-release-assets.sh <outdir>}"

if ! [[ "$RELEASE_ID" =~ ^[0-9]+$ ]]; then
  echo "download-release-assets.sh: RELEASE_ID must be numeric, got: $RELEASE_ID" >&2
  exit 1
fi

mkdir -p "$out"

listing="$(mktemp)"
trap 'rm -f "$listing"' EXIT
gh api --paginate "repos/$REPO/releases/$RELEASE_ID/assets" \
  --jq '.[] | [.id, .name] | @tsv' >"$listing"

count=0
while IFS=$'\t' read -r id name; do
  [ -n "${id:-}" ] || continue
  # The same charset upload-release-asset.sh enforces on the way in. An
  # asset uploaded by some other means with a `/` or a leading `-` in its
  # name must not be turned into a path here.
  if ! [[ "$name" =~ ^[A-Za-z0-9._-]+$ ]] || [[ "$name" == -* ]] || [ "$name" = "." ] || [ "$name" = ".." ]; then
    echo "download-release-assets.sh: refusing an asset name outside [A-Za-z0-9._-]: $name" >&2
    exit 1
  fi
  if [ "$name" = "SHA256SUMS" ]; then
    continue
  fi
  gh api -H "Accept: application/octet-stream" "repos/$REPO/releases/assets/$id" >"$out/$name"
  echo "$name"
  count=$((count + 1))
done <"$listing"

echo "download-release-assets.sh: downloaded $count asset(s) from release $RELEASE_ID into $out" >&2
