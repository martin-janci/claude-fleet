#!/usr/bin/env bash
# Resolves which platforms' digests actually made it out of hub-image.yml's
# `build` matrix, and prints (on stdout) the exact argv for `docker buildx
# imagetools create` to combine them into the release's real tags — or
# fails if the amd64 leg's digest is missing, since amd64 is the one
# platform that must never silently drop out.
#
# Layout expected under <digests-dir>, exactly as
# `actions/download-artifact@v4` lays it out for `pattern: digests-*` with
# NO `merge-multiple` (each matrix leg's artifact keeps its own
# subdirectory instead of being flattened together — flattening would make
# the two platforms' opaque-hex-named digest files indistinguishable):
#   <digests-dir>/digests-amd64/<digest-hex>   (exactly one file)
#   <digests-dir>/digests-arm64/<digest-hex>   (exactly one file, optional)
#
# Rule: amd64 present -> publish (multi-arch if arm64 is also present,
# amd64-only otherwise, so one platform's failure never blocks the other's
# release); amd64 missing -> fail (exit 1), publish nothing. An amd64-only
# publish is a SUCCESSFUL run (exit 0) here and in the workflow — it does
# NOT make the job or the run fail or show red; the caller is responsible
# for surfacing the degradation visibly (a `::warning::` annotation and a
# $GITHUB_STEP_SUMMARY line), which is exactly why the first line of stdout
# below is a machine-readable `STATUS=` marker rather than leaving the
# caller to guess from stderr prose.
#
# Usage: IMAGE=ghcr.io/owner/fleet-hub TAGS=$'tag1\ntag2' \
#          merge-hub-digests.sh <digests-dir>
# Stdout: line 1 is `STATUS=both` or `STATUS=amd64-only`; every line after
# it is one argv element for `docker buildx imagetools create`.
set -euo pipefail

: "${IMAGE:?IMAGE is required, e.g. ghcr.io/owner/fleet-hub}"
: "${TAGS:?TAGS is required, one tag per line}"
dir="${1:?usage: IMAGE=... TAGS=... merge-hub-digests.sh <digests-dir>}"

# Prints $1's one digest-hex filename on stdout, or nothing if that
# subdirectory is absent or empty. Fails (return, not exit — this runs
# inside a command substitution, where `exit` would only end the subshell
# and go unnoticed by the caller) if it holds more than one file: that
# would mean a single leg somehow exported two digests, which is corruption
# worth stopping for rather than silently picking one.
one_digest() {
  local d="$dir/$1"
  [ -d "$d" ] || return 0
  shopt -s nullglob
  local files=("$d"/*)
  shopt -u nullglob
  [ "${#files[@]}" -eq 0 ] && return 0
  if [ "${#files[@]}" -gt 1 ]; then
    echo "merge-hub-digests.sh: $d has more than one digest file: ${files[*]}" >&2
    return 2
  fi
  basename "${files[0]}"
}

if ! amd64="$(one_digest digests-amd64)"; then
  exit 1
fi
if ! arm64="$(one_digest digests-arm64)"; then
  exit 1
fi

if [ -z "$amd64" ]; then
  echo "merge-hub-digests.sh: no amd64 digest — the amd64 leg did not succeed; publishing nothing" >&2
  exit 1
fi

if [ -n "$arm64" ]; then
  echo "merge-hub-digests.sh: amd64 + arm64 both present — publishing a multi-arch manifest" >&2
  echo "STATUS=both"
else
  echo "merge-hub-digests.sh: only amd64 present — publishing an amd64-only manifest (arm64 did not succeed)" >&2
  echo "STATUS=amd64-only"
fi

while IFS= read -r t; do
  [ -n "$t" ] || continue
  echo "-t"
  echo "$t"
done <<<"$TAGS"

echo "${IMAGE}@sha256:${amd64}"
if [ -n "$arm64" ]; then
  echo "${IMAGE}@sha256:${arm64}"
fi
