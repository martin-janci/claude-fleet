#!/usr/bin/env bash
# Gives the macOS `.app.tar.gz` bundle the version it is missing from its
# filename.
#
# WHY THIS EXISTS. `tauri-apps/tauri-action` builds and uploads the macOS
# updater bundle itself, and names it `<productName>_<arch>.app.tar.gz` —
# `claude-fleet_aarch64.app.tar.gz`, `claude-fleet_x64.app.tar.gz`. No
# version. Every release since v0.2.21 therefore published byte-different
# files under those two identical names, so a downloaded
# `claude-fleet_x64.app.tar.gz` is untraceable to the release it came from
# and two of them cannot even coexist in a downloads folder. The `.dmg`,
# `.deb` and `.AppImage` that the same action produces all carry the version;
# only the updater bundles do not, because that name is hard-coded by the
# bundler, not derived from the config.
#
# WHY A RENAME RATHER THAN A DIFFERENT UPLOAD. The action's upload path is
# not configurable per-artifact, and replacing it wholesale (building with
# `cargo tauri build` and uploading by hand) would mean re-implementing its
# bundle discovery for three platforms to fix two filenames. GitHub's
# releases API can rename an asset in place — `PATCH
# /repos/{owner}/{repo}/releases/assets/{asset_id}` with a new `name` — which
# leaves tauri-action's build and upload completely untouched and changes
# only the name the asset is published under. The download URL derives from
# the name, so the published URL carries the version too.
#
# The new name is NOT spelled out here: it comes from
# `scripts/release-assets.sh assets <version> <leg>`, the single asset
# manifest that also drives the build matrix and `verify-release`. The name
# tauri-action used is derived from that same string by deleting the version
# segment, so there is still exactly one place that knows what these assets
# are called.
#
# Idempotent: a re-run of the workflow from the same tag re-uploads the
# unversioned name (tauri-action clobbers by name), and this renames it
# again, replacing the previously renamed asset.
#
# Usage: REPO=owner/repo RELEASE_ID=123 TAG=v0.3.0 LEG=desktop-x86_64-apple-darwin \
#          rename-updater-asset.sh
# Needs `gh` authenticated via GH_TOKEN/GITHUB_TOKEN. Run from the repo root.
set -euo pipefail

: "${REPO:?REPO is required, e.g. owner/repo}"
: "${RELEASE_ID:?RELEASE_ID is required — the numeric release id, never a tag}"
: "${TAG:?TAG is required, e.g. v0.3.0}"
: "${LEG:?LEG is required — a leg id from scripts/release-assets.sh legs}"

here="$(cd "$(dirname "$0")" && pwd)"
version="${TAG#v}"

# Exactly one `.app.tar.gz` per macOS leg, by construction of the manifest.
# The manifest lookup runs on its own line, not inside the pipeline, so an
# unknown LEG fails here (set -e) instead of being swallowed by the `|| true`
# that only covers "this leg has no .app.tar.gz".
leg_assets="$("$here/release-assets.sh" assets "$version" "$LEG")"
desired="$(printf '%s\n' "$leg_assets" | grep '\.app\.tar\.gz$' || true)"
if [ -z "$desired" ]; then
  echo "rename-updater-asset.sh: leg '$LEG' declares no .app.tar.gz asset — nothing to rename" >&2
  exit 0
fi
if [ "$(printf '%s\n' "$desired" | wc -l)" -ne 1 ]; then
  echo "rename-updater-asset.sh: leg '$LEG' declares more than one .app.tar.gz asset:" >&2
  printf '%s\n' "$desired" >&2
  exit 1
fi

# claude-fleet_0.3.0_x64.app.tar.gz -> claude-fleet_x64.app.tar.gz, i.e. the
# name tauri-action actually uploaded. Derived, never a second literal.
current="${desired/_${version}_/_}"
if [ "$current" = "$desired" ]; then
  echo "rename-updater-asset.sh: '$desired' does not contain the version segment '_${version}_';" \
       "the manifest's naming scheme changed — update this script with it" >&2
  exit 1
fi

asset_id() {
  gh api --paginate "repos/$REPO/releases/$RELEASE_ID/assets" \
    --jq ".[] | select(.name == \"$1\") | .id" | head -n1
}

current_id="$(asset_id "$current")"
desired_id="$(asset_id "$desired")"

if [ -z "$current_id" ]; then
  if [ -n "$desired_id" ]; then
    echo "rename-updater-asset.sh: already renamed — '$desired' is on the release and '$current' is not" >&2
    exit 0
  fi
  echo "rename-updater-asset.sh: tauri-action did not upload '$current' to release $RELEASE_ID." >&2
  echo "rename-updater-asset.sh: assets currently on the release:" >&2
  gh api --paginate "repos/$REPO/releases/$RELEASE_ID/assets" --jq '.[].name' >&2
  exit 1
fi

# A previous run's renamed asset would make the PATCH collide (GitHub refuses
# a duplicate asset name with 422); drop it, the fresh build supersedes it.
if [ -n "$desired_id" ]; then
  echo "rename-updater-asset.sh: replacing the previously renamed '$desired' (id $desired_id)" >&2
  gh api --method DELETE "repos/$REPO/releases/assets/$desired_id"
fi

gh api --method PATCH "repos/$REPO/releases/assets/$current_id" -f "name=$desired" >/dev/null
echo "rename-updater-asset.sh: renamed '$current' -> '$desired' (asset id $current_id)" >&2
