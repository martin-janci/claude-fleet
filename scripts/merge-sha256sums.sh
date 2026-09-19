#!/usr/bin/env bash
# Merges the per-target SHA256SUMS.<target> files that
# scripts/package-linux-release.sh writes (one per `agent-hub-binaries`
# matrix leg, downloaded as separate workflow artifacts) into the single
# SHA256SUMS that release.yml's `agent-hub-checksums` job attaches to the
# release.
#
# Never trusts an input file's own claims blindly: every asset name in
# every input file must be one of the FOUR names this release can possibly
# produce — fleet-agent/fleet-hub, each for both release targets, at this
# tag's version — anything else fails loudly (exit 1) rather than
# publishing a checksums file that names something wrong. A leg that never
# ran, or failed before writing its sums file, is fine on its own: this
# script only needs SOME input files to exist, not all of them, and reports
# on stderr which of the four expected assets are present or missing
# without treating "fewer than four" as an error — the caller may still get
# a correct PARTIAL SHA256SUMS when one leg failed.
#
# Usage: TAG=v0.3.0 merge-sha256sums.sh <dir-of-SHA256SUMS.*-files> >SHA256SUMS
# Exit 0 with empty stdout means "no input files at all — nothing to merge".
set -euo pipefail

: "${TAG:?TAG is required, e.g. v0.3.0}"
dir="${1:?usage: TAG=vX.Y.Z merge-sha256sums.sh <dir-of-per-target-sums-files>}"

version="${TAG#v}"
# Keep in sync with release.yml's agent-hub-binaries matrix targets.
targets="aarch64-unknown-linux-gnu x86_64-unknown-linux-gnu"
bins="fleet-agent fleet-hub"

expected="$(mktemp)"
merged="$(mktemp)"
trap 'rm -f "$expected" "$merged"' EXIT

for t in $targets; do
  for b in $bins; do
    echo "${b}-${version}-${t}.tar.gz"
  done
done | sort >"$expected"

shopt -s nullglob
files=("$dir"/SHA256SUMS.*)
shopt -u nullglob
if [ "${#files[@]}" -eq 0 ]; then
  echo "merge-sha256sums.sh: no per-target sums files in $dir — nothing to merge" >&2
  exit 0
fi

# Stable order: sorted by the per-target filename (SHA256SUMS.aarch64-... <
# SHA256SUMS.x86_64-...), so the merge is deterministic regardless of which
# matrix leg's artifact download finished first. Every emitted line is
# normalised to a BARE filename (never `./name`): package-linux-release.sh
# already writes bare names, but a foreign or older input file might not,
# and the requirement here is bare names in what gets published, not just
# in this script's own internal comparisons.
for f in $(printf '%s\n' "${files[@]}" | sort); do
  sed -E 's#^([0-9a-f]+  )\./#\1#' "$f" >>"$merged"
done

names="$(awk '{print $2}' "$merged" | sort)"

# A name appearing more than once — whether from one leg's own file or
# across both — is corruption distinct from "not one of the four": call it
# out by name rather than folding it into the unexpected-name check below,
# which (comparing multisets with `comm`) would otherwise misreport a
# duplicate of a perfectly valid name as "outside the four".
duplicates="$(echo "$names" | uniq -d)"
if [ -n "$duplicates" ]; then
  while IFS= read -r d; do
    [ -n "$d" ] && echo "merge-sha256sums.sh: duplicate entry for $d" >&2
  done <<<"$duplicates"
  exit 1
fi

unexpected="$(comm -23 <(echo "$names") "$expected" || true)"
if [ -n "$unexpected" ]; then
  echo "merge-sha256sums.sh: SHA256SUMS names an asset outside the four this tag can produce:" >&2
  echo "$unexpected" >&2
  exit 1
fi

missing="$(comm -13 <(echo "$names") "$expected" || true)"
if [ -n "$missing" ]; then
  echo "merge-sha256sums.sh: partial — a leg likely failed; missing:" >&2
  echo "$missing" >&2
else
  echo "merge-sha256sums.sh: complete — all four expected assets present" >&2
fi

cat "$merged"
