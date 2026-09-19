#!/usr/bin/env bash
# Uploads one file to a GitHub release BY NUMERIC RELEASE ID, never by tag:
# `GET /repos/{owner}/{repo}/releases/tags/{tag}` (what a tag-based lookup,
# including `gh release upload <tag>`, resolves through) does not return
# draft releases. release.yml's `create-release` job already hands every
# other job its release's numeric id for exactly this reason (see
# `tauri-action`'s `releaseId:` input on the desktop `build` job) — this
# script keeps that same rule for the assets this task adds, going straight
# at the uploads API instead.
#
# Deletes any existing asset of the same name first, so a re-run of the
# workflow from the same tag replaces assets exactly like `gh release
# upload --clobber` did before this script replaced it (release.yml's own
# top-of-file comment already documents that re-run behaviour).
#
# Usage: REPO=owner/repo RELEASE_ID=123456 upload-release-asset.sh <file> [asset-name]
# (asset-name defaults to the file's own basename). Needs `gh` authenticated
# via GH_TOKEN (or GITHUB_TOKEN), as every other step in this workflow does.
set -euo pipefail

: "${REPO:?REPO is required, e.g. owner/repo}"
: "${RELEASE_ID:?RELEASE_ID is required — the numeric release id, never a tag}"
file="${1:?usage: REPO=... RELEASE_ID=... upload-release-asset.sh <file> [asset-name]}"
name="${2:-$(basename "$file")}"

if ! [[ "$name" =~ ^[A-Za-z0-9._-]+$ ]]; then
  echo "upload-release-asset.sh: refusing an asset name with anything outside [A-Za-z0-9._-]: $name" >&2
  exit 1
fi
if [ ! -f "$file" ]; then
  echo "upload-release-asset.sh: $file not found" >&2
  exit 1
fi
if ! [[ "$RELEASE_ID" =~ ^[0-9]+$ ]]; then
  echo "upload-release-asset.sh: RELEASE_ID must be numeric, got: $RELEASE_ID" >&2
  exit 1
fi

existing_id="$(gh api "repos/$REPO/releases/$RELEASE_ID/assets" --jq ".[] | select(.name == \"$name\") | .id" | head -n1)"
if [ -n "$existing_id" ]; then
  echo "upload-release-asset.sh: replacing existing asset $name (id $existing_id)" >&2
  gh api --method DELETE "repos/$REPO/releases/assets/$existing_id"
fi

gh api --method POST \
  -H "Content-Type: application/octet-stream" \
  "https://uploads.github.com/repos/$REPO/releases/$RELEASE_ID/assets?name=$name" \
  --input "$file" >/dev/null

echo "upload-release-asset.sh: uploaded $name" >&2
