#!/usr/bin/env bash
# Produces the single `SHA256SUMS` asset a claude-fleet release publishes,
# covering EVERY asset on that release — not a hard-coded subset.
#
# WHAT CHANGED AND WHY. This script used to merge only the two
# `SHA256SUMS.<target>` files that scripts/package-linux-release.sh writes,
# and it validated them against a four-name whitelist built from a copy of
# release.yml's matrix. The result: a release carrying 11 assets shipped a
# 466-byte SHA256SUMS listing 4 of them, so no desktop bundle — the thing
# users actually download — had a published checksum at all, and the
# whitelist was a second copy of the matrix, free to drift from it. Now the
# authority is the release itself: scripts/download-release-assets.sh
# enumerates and fetches the assets by id, this script hashes what came back,
# and scripts/verify-release.sh (reading the one manifest,
# scripts/release-assets.sh) decides whether that set is complete. This
# script deliberately does NOT decide completeness — a partial release still
# gets a correct partial SHA256SUMS, and the gate stays in exactly one place.
#
# THE PER-TARGET FILES ARE NOW A CROSS-CHECK, NOT THE INPUT. Each
# `agent-hub-binaries` leg still computes `SHA256SUMS.<target>` locally, from
# the tarball it just built, before uploading it. Passing that directory here
# turns every overlapping digest into an assertion: the bytes GitHub serves
# must equal the bytes the runner built. A mismatch means a corrupted or
# clobbered upload and fails loudly (exit 1) rather than publishing a
# checksums file that certifies the wrong content. A name present in a
# per-target file but missing from the release fails the same way — an entry
# for an asset nobody can download is worse than no entry.
#
# Usage:
#   merge-sha256sums.sh <assets-dir> [per-target-sums-dir] >SHA256SUMS
#
# <assets-dir>            files downloaded from the release (one per asset)
# [per-target-sums-dir]   optional; any `SHA256SUMS.*` files in it are used
#                         as the cross-check described above. Genuinely
#                         optional: a path that does not exist, or exists and
#                         holds no `SHA256SUMS.*`, only skips the cross-check.
#                         release.yml passes this path unconditionally while
#                         the step that creates it (actions/download-artifact,
#                         continue-on-error) may never run — if both
#                         `agent-hub-binaries` legs fail before uploading
#                         their artifact there is no directory at all, and
#                         that must not cost the desktop bundles that DID
#                         build their SHA256SUMS.
#
# Pure and offline — no `gh`, no network, no knowledge of the release — so it
# runs against a fixture directory unchanged. Exit 0 with empty stdout means
# the assets directory is empty: nothing on the release to checksum yet,
# which is the caller's business, not a failure here.
set -euo pipefail

assets_dir="${1:?usage: merge-sha256sums.sh <assets-dir> [per-target-sums-dir] >SHA256SUMS}"
sums_dir="${2:-}"

if [ ! -d "$assets_dir" ]; then
  echo "merge-sha256sums.sh: $assets_dir is not a directory" >&2
  exit 1
fi

merged="$(mktemp)"
trap 'rm -f "$merged"' EXIT

# Sorted by name so the published file is byte-stable regardless of download
# order. Bare filenames, never `./name`: `sha256sum -c SHA256SUMS` expects a
# name it can open in the current directory. A plain glob rather than
# `find -printf`, which is GNU-only.
(
  cd "$assets_dir"
  shopt -s nullglob
  for f in *; do
    if [ -f "$f" ] && [ "$f" != "SHA256SUMS" ]; then
      printf '%s\n' "$f"
    fi
  done | LC_ALL=C sort | while IFS= read -r n; do
    sha256sum -- "$n"
  done
) >"$merged"

if [ ! -s "$merged" ]; then
  echo "merge-sha256sums.sh: no assets in $assets_dir — nothing to checksum" >&2
  exit 0
fi

count="$(wc -l <"$merged" | tr -d ' ')"

# A per-target directory that is not there at all is the same situation as
# one that is there and empty (see the usage note above): no cross-check is
# possible, but the assets still get checksummed. Only the assets directory
# is mandatory.
if [ -n "$sums_dir" ] && [ ! -d "$sums_dir" ]; then
  echo "merge-sha256sums.sh: $sums_dir does not exist — skipping the build-vs-release cross-check" >&2
  sums_dir=""
fi

# Cross-check against what the build legs computed locally, if provided.
if [ -n "$sums_dir" ]; then
  shopt -s nullglob
  per_target=("$sums_dir"/SHA256SUMS.*)
  shopt -u nullglob
  if [ "${#per_target[@]}" -eq 0 ]; then
    # Both legs failed before packaging, or their artifacts never arrived.
    # The desktop assets still deserve a checksums file; verify-release is
    # what notices that the tarballs are missing.
    echo "merge-sha256sums.sh: no per-target sums files in $sums_dir — skipping the build-vs-release cross-check" >&2
  else
    mismatches=0
    for f in $(printf '%s\n' "${per_target[@]}" | LC_ALL=C sort); do
      while read -r want name; do
        [ -n "${name:-}" ] || continue
        name="${name#\*}" # sha256sum's binary-mode marker
        name="${name#./}"
        got="$(awk -v n="$name" '$2 == n { print $1 }' "$merged" | head -n1)"
        if [ -z "$got" ]; then
          echo "merge-sha256sums.sh: $(basename "$f") lists '$name', which is not on the release" >&2
          mismatches=$((mismatches + 1))
        elif [ "$got" != "$want" ]; then
          echo "merge-sha256sums.sh: '$name' was built as $want but the release serves $got" >&2
          mismatches=$((mismatches + 1))
        fi
      done <"$f"
    done
    if [ "$mismatches" -ne 0 ]; then
      echo "merge-sha256sums.sh: $mismatches build-vs-release checksum disagreement(s); refusing to publish SHA256SUMS" >&2
      exit 1
    fi
    echo "merge-sha256sums.sh: build-vs-release cross-check passed for ${#per_target[@]} per-target file(s)" >&2
  fi
fi

echo "merge-sha256sums.sh: $count asset(s) checksummed from $assets_dir" >&2
cat "$merged"
