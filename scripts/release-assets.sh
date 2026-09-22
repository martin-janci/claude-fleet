#!/usr/bin/env bash
# THE release asset manifest: the one place that says what a claude-fleet
# release consists of — which build legs run, on which runner, for which
# target, and exactly which asset filenames each leg is expected to put on
# the GitHub Release.
#
# Everything downstream reads this file instead of keeping its own copy:
#
#   .github/workflows/release.yml   `plan` job -> `matrix <kind>` drives the
#                                   `build` and `agent-hub-binaries` matrices
#                                   (`strategy: matrix: ${{ fromJSON(...) }}`)
#   scripts/package-linux-release.sh  asserts the tarballs it just wrote are
#                                   exactly `assets <version> bins-<target>`
#   scripts/rename-updater-asset.sh renames tauri-action's unversioned
#                                   `.app.tar.gz` to the name declared here
#   scripts/verify-release.sh       asserts the release carries exactly
#                                   `assets <version>` and nothing else
#
# That single-source rule is the point of the file. Before this existed, the
# expected names lived in three places that drifted: release.yml's matrix,
# merge-sha256sums.sh's four-name whitelist, and docs/RELEASING.md's table.
# If you add a build leg or rename an asset, change the LEGS table below and
# nothing else — and if you find yourself typing an asset filename into a
# workflow or another script, that is the bug.
#
# Usage:
#   release-assets.sh legs                     # every leg id, one per line
#   release-assets.sh assets <version> [leg]   # expected asset names (all, or one leg's)
#   release-assets.sh matrix <kind>            # GitHub Actions matrix JSON for a kind
#   release-assets.sh runner <leg>             # the runner label for one leg
#   release-assets.sh --help
#
# <version> is the tag with its leading `v` stripped (0.3.0, not v0.3.0).
# Exit 0 on success, 1 on a bad leg/kind/version, 2 on a usage error.
set -euo pipefail

self="$(basename "$0")"

# leg | kind | runner | rust target | build args | asset names ({v} = version)
#
# `kind` groups legs into the workflow matrices: `desktop` is tauri-action,
# `bins` is the fleet-agent/fleet-hub tarball packaging, `checksums` is the
# single aggregate job that has no matrix but does produce an asset.
#
# Runner choices are explained in release.yml (22.04 for the bins legs so the
# binaries only need glibc 2.35+). The macOS `.app.tar.gz` names here are the
# VERSIONED names the assets end up with — tauri-action itself uploads them
# unversioned (`claude-fleet_aarch64.app.tar.gz`) and
# scripts/rename-updater-asset.sh renames them to these afterwards.
read_legs() {
  cat <<'LEGS'
desktop-aarch64-apple-darwin|desktop|macos-latest|aarch64-apple-darwin|--target aarch64-apple-darwin|claude-fleet_{v}_aarch64.dmg claude-fleet_{v}_aarch64.app.tar.gz
desktop-x86_64-apple-darwin|desktop|macos-latest|x86_64-apple-darwin|--target x86_64-apple-darwin|claude-fleet_{v}_x64.dmg claude-fleet_{v}_x64.app.tar.gz
desktop-x86_64-linux|desktop|ubuntu-24.04||--bundles appimage,deb|claude-fleet_{v}_amd64.deb claude-fleet_{v}_amd64.AppImage
bins-x86_64-unknown-linux-gnu|bins|ubuntu-22.04|x86_64-unknown-linux-gnu||fleet-agent-{v}-x86_64-unknown-linux-gnu.tar.gz fleet-hub-{v}-x86_64-unknown-linux-gnu.tar.gz
bins-aarch64-unknown-linux-gnu|bins|ubuntu-22.04-arm|aarch64-unknown-linux-gnu||fleet-agent-{v}-aarch64-unknown-linux-gnu.tar.gz fleet-hub-{v}-aarch64-unknown-linux-gnu.tar.gz
checksums|checksums|ubuntu-24.04|||SHA256SUMS
LEGS
}

usage() {
  sed -n '2,/^set -euo/p' "$0" | sed 's/^# \{0,1\}//; $d'
}

die() {
  echo "$self: $1" >&2
  exit "${2:-1}"
}

# A release version, not a tag: the caller strips the `v`. Refused rather than
# silently interpolated, so a stray `v` or a shell metacharacter can never end
# up inside an asset filename that later gets passed to the uploads API.
check_version() {
  case "$1" in
    *[!0-9A-Za-z.+-]* | '') die "not a version: '$1' (expected 0.3.0, without the leading 'v')" ;;
  esac
  if ! echo "$1" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.-]+)?$'; then
    die "not a semver version: '$1' (expected X.Y.Z, optionally -rc.1)"
  fi
}

cmd_legs() {
  read_legs | cut -d'|' -f1
}

cmd_runner() {
  local want="${1:?}" leg kind runner target args assets
  while IFS='|' read -r leg kind runner target args assets; do
    [ "$leg" = "$want" ] || continue
    echo "$runner"
    return 0
  done < <(read_legs)
  die "no such leg: '$want' (see: $self legs)"
}

# Every expected asset name for this version, sorted and deduplicated. With a
# leg id, only that leg's — which is how package-linux-release.sh and
# rename-updater-asset.sh learn the names they are supposed to produce.
#
# The sort is done here rather than left to each caller so the contract in
# that first sentence is one the code actually keeps: callers diff this list
# against a sorted listing of what a release carries, and two of them were
# already re-sorting it by hand. Collected into a variable first so an
# unknown leg still `die`s with its own exit status instead of that status
# being swallowed by a pipeline.
cmd_assets() {
  local version="${1:?}" want="${2:-}" found=0 leg kind runner target args assets a
  local out=""
  check_version "$version"
  while IFS='|' read -r leg kind runner target args assets; do
    if [ -n "$want" ] && [ "$leg" != "$want" ]; then
      continue
    fi
    found=1
    for a in $assets; do
      # Only {v} is substituted, and only by a version that passed
      # check_version above.
      out+="${a//\{v\}/$version}"$'\n'
    done
  done < <(read_legs)
  if [ -n "$want" ] && [ "$found" -eq 0 ]; then
    die "no such leg: '$want' (see: $self legs)"
  fi
  [ -n "$out" ] || return 0
  printf '%s' "$out" | LC_ALL=C sort -u
}

# `{"include":[…]}` — the whole `strategy.matrix` value, so release.yml can
# write `matrix: ${{ fromJSON(needs.plan.outputs.<kind>) }}` verbatim. Only
# the fields the workflow actually reads are emitted; every value comes from
# the table above, which contains no JSON metacharacters (asserted below), so
# this hand-rolled encoder needs no escaping.
cmd_matrix() {
  local want="${1:?}" first=1 found=0 leg kind runner target args assets
  case "$want" in
    desktop | bins) ;;
    *) die "no such matrix kind: '$want' (expected: desktop, bins)" ;;
  esac
  printf '{"include":['
  while IFS='|' read -r leg kind runner target args assets; do
    [ "$kind" = "$want" ] || continue
    case "$leg$runner$target$args" in
      *[\"\\]*) die "leg '$leg' contains a JSON metacharacter; the encoder here cannot escape it" ;;
    esac
    found=1
    [ "$first" -eq 1 ] || printf ','
    first=0
    printf '{"leg":"%s","os":"%s","target":"%s","args":"%s"}' \
      "$leg" "$runner" "$target" "$args"
  done < <(read_legs)
  printf ']}\n'
  [ "$found" -eq 1 ] || die "matrix kind '$want' matched no leg"
}

case "${1:-}" in
  --help | -h | help) usage ;;
  legs) cmd_legs ;;
  runner) shift; [ $# -eq 1 ] || die "usage: $self runner <leg>" 2; cmd_runner "$1" ;;
  assets) shift; [ $# -ge 1 ] && [ $# -le 2 ] || die "usage: $self assets <version> [leg]" 2; cmd_assets "$@" ;;
  matrix) shift; [ $# -eq 1 ] || die "usage: $self matrix <kind>" 2; cmd_matrix "$1" ;;
  *) usage >&2; exit 2 ;;
esac
