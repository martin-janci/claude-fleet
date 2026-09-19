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
# NOT ATOMIC: this only matters on a re-run over an asset that already
# exists (the ordinary "delete, then upload" case) — a first-time upload of
# a name that isn't already on the release has nothing to delete and can't
# hit this. If the DELETE succeeds but the POST that follows it fails (a
# dropped connection, a bad token, GitHub having a bad moment), the release
# is left with NO asset of that name at all until the workflow is re-run.
# The design is kept anyway: `gh release upload --clobber` had the exact
# same window, and this script only fails loudly instead of silently — an
# `::error::` naming the asset (see below), and a non-zero exit that fails
# this step and its job for real. Both call sites' jobs carry
# `continue-on-error: true`, so that failure shows as the job's own
# "failed but allowed" status, not necessarily a red overall workflow run
# — check job status and the `::error::` log line, not just the run's own
# green checkmark. Every asset here is reproducible from the same tag, so
# a re-run is always a complete fix, never a partial one. See
# docs/RELEASING.md's "what to check after a release" for the re-run
# instruction.
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
deleted_existing=0
if [ -n "$existing_id" ]; then
  echo "upload-release-asset.sh: replacing existing asset $name (id $existing_id)" >&2
  gh api --method DELETE "repos/$REPO/releases/assets/$existing_id"
  deleted_existing=1
fi

if ! gh api --method POST \
  -H "Content-Type: application/octet-stream" \
  "https://uploads.github.com/repos/$REPO/releases/$RELEASE_ID/assets?name=$name" \
  --input "$file" >/dev/null; then
  if [ "$deleted_existing" = 1 ]; then
    echo "::error title=release asset missing::$name was deleted to be replaced, but the re-upload failed — the release has NO $name asset until this job is re-run"
  fi
  exit 1
fi

echo "upload-release-asset.sh: uploaded $name" >&2
