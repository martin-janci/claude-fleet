#!/usr/bin/env bash
# release-manifest.sh — write, sign and upload a release's update manifest
# (update-channel design §4, slice S2). Called by release.yml's `manifest`
# job after the build legs, before `checksums`, so SHA256SUMS covers it.
#
#   env: REPO RELEASE_ID TAG GIT_SHA BUILD_ID GH_TOKEN RELEASE_SIGNING_KEY
#        ASSETS_DIR (every asset on the release, downloaded by id)
#        FLEET_RELEASE (the fleet-release binary)
#        HUB_IMAGE_WAIT_SECS (default 1800) DRY_RUN=1 (write + sign, no upload)
#
# What it takes from where, and why:
# - the protocol windows from the RELEASED fleet-hub binary itself
#   (`fleet-hub compat`), unpacked from this release's x86_64 tarball — so
#   the manifest states what the shipped build speaks, not what a source
#   file said;
# - the desktop's window from src-tauri's MIN_/MAX_HUB_CONTRACT at this tag
#   (the desktop is not runnable here), the same lines fleet-mobile's drift
#   test reads;
# - the hub image digest from the release body, where hub-image.yml's
#   record-digest writes it. That workflow runs beside this one, so this
#   waits for it; without it the manifest still carries the hub tarballs.
#
# The manifest is required from the version `scripts/release-assets.sh`
# names for the `manifest` leg. For an older version this does nothing; for
# a newer one a missing RELEASE_SIGNING_KEY fails, and verify-release then
# reports the missing asset: a release is never published with an update
# manifest nobody can verify.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/.." && pwd)"
: "${TAG:?TAG is required, e.g. v0.4.1}"
: "${ASSETS_DIR:?ASSETS_DIR is required}"
: "${FLEET_RELEASE:?FLEET_RELEASE is required (the fleet-release binary)}"
version="${TAG#v}"

if [ -z "$("$here/release-assets.sh" assets "$version" manifest)" ]; then
  echo "release-manifest.sh: $TAG predates the update manifest; nothing to do." >&2
  exit 0
fi
if [ -z "${RELEASE_SIGNING_KEY:-}" ]; then
  echo "::error title=no release signing key::RELEASE_SIGNING_KEY is not set, so $TAG gets no signed update manifest and verify-release will fail. Run scripts/release-key.sh once (docs/RELEASING.md → Update manifest and channels), then re-run this workflow."
  exit 1
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
umask 077

# The public keys every build trusts, from fleet-update's keys.rs.
"$here/release-pubkeys.sh" >"$work/pubkeys"
if [ ! -s "$work/pubkeys" ]; then
  echo "::error title=no compiled-in release key::crates/fleet-update/src/keys.rs lists no release key at $TAG, so nothing signed now could ever be verified. Add the public key scripts/release-key.sh printed, then re-tag."
  exit 1
fi

# 1. The hub's windows, from the shipped binary.
tarball="$ASSETS_DIR/fleet-hub-$version-x86_64-unknown-linux-gnu.tar.gz"
[ -f "$tarball" ] || { echo "::error::$TAG has no $(basename "$tarball") to read the hub's protocol windows from"; exit 1; }
mkdir -p "$work/hub"
tar -xzf "$tarball" -C "$work/hub"
hub_bin="$(find "$work/hub" -type f -name fleet-hub | head -n1)"
[ -n "$hub_bin" ] || { echo "::error::no fleet-hub binary inside $(basename "$tarball")"; exit 1; }
chmod +x "$hub_bin"
"$hub_bin" compat >"$work/compat.json"

# 2. The desktop's window, from this tag's source.
contract="$root/src-tauri/src/backend/contract.rs"
dmin="$(sed -n 's/^pub const MIN_HUB_CONTRACT: u32 = \([0-9][0-9]*\);.*/\1/p' "$contract")"
dmax="$(sed -n 's/^pub const MAX_HUB_CONTRACT: u32 = \([0-9][0-9]*\);.*/\1/p' "$contract")"
if [ -z "$dmin" ] || [ -z "$dmax" ]; then
  echo "::error::could not read MIN_/MAX_HUB_CONTRACT from src-tauri/src/backend/contract.rs"
  exit 1
fi

# 3. The hub image digest, once hub-image.yml has recorded it.
hub_image=""
if [ -n "${RELEASE_ID:-}" ] && [ -n "${REPO:-}" ]; then
  waited=0
  limit="${HUB_IMAGE_WAIT_SECS:-1800}"
  while :; do
    body="$(gh api "repos/$REPO/releases/$RELEASE_ID" --jq '.body // ""')"
    line="$(printf '%s\n' "$body" | sed -n '/<!-- fleet-hub-image -->/,/<!-- \/fleet-hub-image -->/p' | grep -E 'digest `sha256:[0-9a-f]{64}`' | head -n1 || true)"
    if [ -n "$line" ]; then
      image="$(printf '%s' "$line" | sed -nE 's/^`([^`]+):[^`:]+` .*/\1/p')"
      digest="$(printf '%s' "$line" | grep -oE 'sha256:[0-9a-f]{64}' | head -n1)"
      [ -n "$image" ] && [ -n "$digest" ] && hub_image="$image@$digest"
      break
    fi
    if [ "$waited" -ge "$limit" ]; then
      echo "::warning title=no hub image digest::hub-image.yml recorded no digest on $TAG within ${limit}s; the manifest carries the hub tarballs but no container image, so fleet-updater cannot update to $version."
      break
    fi
    sleep 30
    waited=$((waited + 30))
  done
fi

# 4. Write, sign, and check the signature against the compiled-in key.
"$FLEET_RELEASE" manifest \
  --version "$version" \
  --commit "${GIT_SHA:-unknown}" \
  --build-id "${BUILD_ID:-local}" \
  --assets "$ASSETS_DIR" \
  --assets-base "https://github.com/${REPO:-martin-janci/claude-fleet}/releases/download/$TAG/" \
  --notes-url "https://github.com/${REPO:-martin-janci/claude-fleet}/releases/tag/$TAG" \
  --compat "$work/compat.json" \
  --desktop-accepts "$dmin,$dmax" \
  --hub-image "$hub_image" \
  --out release-manifest.json

printf '%s\n' "$RELEASE_SIGNING_KEY" >"$work/key"
minisign -S -s "$work/key" -m release-manifest.json -x release-manifest.json.minisig \
  -t "claude-fleet $TAG release manifest" >/dev/null
"$here/release-verify-sig.sh" "$work/pubkeys" release-manifest.json release-manifest.json.minisig manifest

if [ "${DRY_RUN:-0}" = 1 ]; then
  echo "release-manifest.sh: DRY_RUN — wrote and signed release-manifest.json, uploaded nothing." >&2
  exit 0
fi
"$here/upload-release-asset.sh" release-manifest.json
"$here/upload-release-asset.sh" release-manifest.json.minisig
echo "release-manifest.sh: $TAG carries a signed release-manifest.json." >&2
