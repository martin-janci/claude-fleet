#!/usr/bin/env bash
# Print one version's CHANGELOG.md section verbatim — the release body.
#
# `scripts/release.sh` writes every section in the same shape
# (`## [X.Y.Z] - YYYY-MM-DD`, then `### Added` / `### Changed` / `### Fixed` /
# `### Documentation`), so the notes that end up on the GitHub Release are the
# ones that were reviewed in the release commit, not a second copy typed into
# the workflow. `.github/workflows/release.yml` (`create-release`) is the only
# caller; it FAILS the release when this script exits non-zero, because a tag
# whose version has no CHANGELOG section is a release with no notes.
#
# Usage: scripts/changelog-section.sh <X.Y.Z[-rc.N]> [path/to/CHANGELOG.md]
# Prints the section BODY (the `## [...]` heading itself is left out: the
# release already carries the version in its title), with leading and trailing
# blank lines trimmed and everything in between untouched.
# Exit 0 with the section on stdout, 1 when there is no such section or it is
# empty, 2 on a usage error.
set -euo pipefail

self="$(basename "$0")"
die() { echo "$self: $1" >&2; exit "${2:-1}"; }

[ $# -ge 1 ] && [ $# -le 2 ] || die "usage: $self <X.Y.Z[-rc.N]> [CHANGELOG.md]" 2
version="$1"
file="${2:-CHANGELOG.md}"

# Same shape scripts/release.sh accepts, so a version that can be released can
# always be looked up here. Refused rather than pasted into the awk program
# below as data it might be able to steer.
case "$version" in
  *[!0-9A-Za-z.-]* | '') die "not a version: '$version' (expected 0.3.0 or 0.3.0-rc.1, without the leading 'v')" 2 ;;
esac
echo "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$' \
  || die "not a semver version: '$version' (expected X.Y.Z, optionally -rc.1)" 2

[ -f "$file" ] || die "no such file: $file"

# The section runs from its own heading to whichever comes first: the next
# `## ` heading, or the block of `[X.Y.Z]: <url>` link references at the foot
# of the file (the last section would otherwise swallow all of them).
# The heading is matched as a literal string, never as a regex — `0.3.0` as a
# pattern would also match a hypothetical `0x3y0` section.
section="$(awk -v want="## [$version]" '
  !inside { if (substr($0, 1, length(want)) == want) { inside = 1 } ; next }
  /^## /              { exit }
  /^\[[^]]+\]: /      { exit }
  { print }
' "$file")"

# Leading blank lines (there is always one right after the heading); trailing
# ones are already gone, command substitution strips them.
section="$(printf '%s\n' "$section" | sed '/./,$!d')"

[ -n "$section" ] || die "CHANGELOG.md has no '## [$version]' section, or it is empty — a release must not ship without notes"

printf '%s\n' "$section"
