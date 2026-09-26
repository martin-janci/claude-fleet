#!/usr/bin/env bash
# Does every released tag still have a complete release behind it?
#
# release.yml guarantees a release is complete AT THE MOMENT IT IS BUILT. It
# cannot say anything about the days after: a release can be deleted by hand
# (v0.2.22, v0.2.24 and v0.2.25 were — their tags are still there, their
# releases are gone), a tag can be pushed whose workflow never ran (v0.2.27 was
# bumped without one), an asset can be removed from a published release, and a
# draft can sit unpublished for a week. Nothing watched for any of that. This
# script does, on a schedule (.github/workflows/release-drift.yml).
#
# It REPORTS, it does not write: a markdown report on stdout, a summary line on
# stderr, exit 1 when anything drifted. The workflow is what turns that into a
# single, updated-in-place GitHub issue.
#
# It owns no assertions of its own. Completeness is `scripts/verify-release.sh`
# — the same script release.yml's gate runs, against the same
# `scripts/release-assets.sh` manifest — invoked once per release. If the
# definition of "complete" changes, it changes in one place and this follows.
#
# The four checks:
#   1. a release-shaped tag (`vX.Y.Z`, optionally `-rc.N`) with no release
#      object at all;
#   2. a draft release older than --draft-max-age-hours (the habit T10 exists
#      to end: six drafts had accumulated, each one a version nobody can
#      download);
#   3. the newest --verify-tags releases are complete and fully checksummed
#      (bounded on purpose: this is the expensive check, two API calls plus a
#      download per release, and old releases do not spontaneously rot);
#   4. a release pointing at a tag that no longer exists.
#
# Usage: scripts/check-release-drift.sh [--verify-tags N] [--draft-max-age-hours H]
# Env:   REPO         owner/repo (default: derived from `origin`)
#        IGNORE_FILE  tags known to have no release, one per line, `#` comments
#                     (default: .github/release-drift-ignore)
# Needs `gh` authenticated. NOTE: draft releases are only visible to a token
# with push access; with a read-only token check 2 silently sees nothing, so
# the report says so instead of pretending the check ran.
# Exit 0 no drift, 1 drift found, 2 usage / tooling problem.
set -euo pipefail

self="$(basename "$0")"
here="$(cd "$(dirname "$0")" && pwd)"
die() { echo "$self: $1" >&2; exit "${2:-1}"; }

VERIFY_TAGS=10
DRAFT_MAX_AGE_HOURS=6
while [ $# -gt 0 ]; do
  case "$1" in
    --verify-tags) VERIFY_TAGS="${2:?--verify-tags needs a number}"; shift 2 ;;
    --draft-max-age-hours) DRAFT_MAX_AGE_HOURS="${2:?--draft-max-age-hours needs a number}"; shift 2 ;;
    -h | --help) sed -n '2,/^set -euo/p' "$0" | sed 's/^# \{0,1\}//; $d'; exit 0 ;;
    *) die "unknown argument: '$1'" 2 ;;
  esac
done
case "$VERIFY_TAGS" in *[!0-9]* | '') die "--verify-tags must be a number, got '$VERIFY_TAGS'" 2 ;; esac
case "$DRAFT_MAX_AGE_HOURS" in *[!0-9]* | '') die "--draft-max-age-hours must be a number, got '$DRAFT_MAX_AGE_HOURS'" 2 ;; esac

command -v gh >/dev/null 2>&1 || die "gh is not installed" 2

if [ -z "${REPO:-}" ]; then
  origin="$(git config --get remote.origin.url || true)"
  [ -n "$origin" ] || die "no 'origin' remote and REPO is unset" 2
  REPO="$(printf '%s\n' "$origin" | sed -E 's#^git@[^:]+:##; s#^[a-z]+://[^/]+/##; s#\.git$##')"
fi
case "$REPO" in */*) ;; *) die "REPO is not owner/repo: '$REPO'" 2 ;; esac
export REPO

IGNORE_FILE="${IGNORE_FILE:-.github/release-drift-ignore}"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# --- inputs -------------------------------------------------------------------

# Release-shaped tags only: `v0.2.0-phase2` and friends are refs that live in
# refs/tags without ever having been a release, and flagging them forever would
# train everyone to ignore this report.
gh api --paginate "repos/$REPO/tags?per_page=100" --jq '.[].name' \
  | grep -E '^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$' \
  | LC_ALL=C sort -u >"$work/tags" || true

# id, tag, draft, prerelease, created_at, stale(draft older than the cutoff).
# The age is computed by jq's `now` at fetch time rather than by `date`, whose
# arithmetic flags differ between GNU and BSD.
gh api --paginate "repos/$REPO/releases?per_page=100" --jq "
  .[] | [(.id|tostring), .tag_name, (.draft|tostring), (.prerelease|tostring), .created_at,
         ((now - (.created_at|fromdateiso8601)) > ($DRAFT_MAX_AGE_HOURS * 3600) | tostring)] | @tsv
" >"$work/releases"

# Can this token see drafts at all? Only push access lists them, so a read-only
# token makes check 2 vacuous — which must be said out loud, not passed off as
# "no stale drafts".
sees_drafts="$(gh api "repos/$REPO" --jq '.permissions.push // false' 2>/dev/null || echo unknown)"

if [ -f "$IGNORE_FILE" ]; then
  sed 's/#.*//' "$IGNORE_FILE" | tr -d '[:blank:]' | grep -v '^$' | LC_ALL=C sort -u >"$work/ignored" || true
fi
[ -f "$work/ignored" ] || : >"$work/ignored"

awk -F'\t' '{ print $2 }' "$work/releases" | LC_ALL=C sort -u >"$work/released_tags"

# --- 1. release-shaped tags with no release object ----------------------------
comm -23 "$work/tags" "$work/released_tags" | comm -23 - "$work/ignored" >"$work/no_release"

# --- 2. drafts older than the cutoff ------------------------------------------
awk -F'\t' '$3 == "true" && $6 == "true" { print $2 "\t" $5 }' "$work/releases" >"$work/stale_drafts"

# --- 4. releases whose tag is gone --------------------------------------------
comm -13 "$work/tags" "$work/released_tags" >"$work/dangling"

# --- 3. the newest N releases are complete ------------------------------------
# Newest by version, not by API order: `sort -V` keeps 0.2.9 below 0.2.10.
: >"$work/incomplete"
: >"$work/verified"
while IFS= read -r tag; do
  [ -n "$tag" ] || continue
  id="$(awk -F'\t' -v t="$tag" '$2 == t { print $1; exit }' "$work/releases")"
  [ -n "$id" ] || continue
  echo "$tag" >>"$work/verified"
  if out="$(RELEASE_ID="$id" TAG="$tag" "$here/verify-release.sh" 2>&1)"; then
    continue
  fi
  # verify-release.sh speaks in Actions annotations (`::error title=…::`),
  # which belong to a release run and not to this report — strip the prefix and
  # keep the sentence.
  # Tagged rows (`t` heading, `m` message), because verify-release.sh's own
  # sentences start with the tag name too and could not otherwise be told
  # apart from the heading when the report is rendered.
  {
    printf 't\t%s\n' "$tag"
    printf '%s\n' "$out" | sed -n 's/^::error title=[^:]*:://p' | sed 's/^/m\t/'
  } >>"$work/incomplete"
done < <(LC_ALL=C sort -rV "$work/released_tags" | grep -E '^v[0-9]' | head -n "$VERIFY_TAGS")

# --- report -------------------------------------------------------------------
# Deliberately free of anything that changes between two runs over the same
# state (no "generated at", no ages in hours): the workflow compares this body
# with the open issue's and leaves the issue untouched when they are equal, so
# an unchanged problem produces no notification.
problems=0
{
  echo "\`scripts/check-release-drift.sh\` on \`$REPO\`."
  echo
  echo "$(wc -l <"$work/tags" | tr -d ' ') release-shaped tags, $(wc -l <"$work/releases" | tr -d ' ') releases;" \
       "completeness verified on the newest $VERIFY_TAGS ($(wc -l <"$work/verified" | tr -d ' ') checked)."
  echo

  if [ -s "$work/no_release" ]; then
    problems=$((problems + $(wc -l <"$work/no_release")))
    echo "### Tags with no release"
    echo
    echo "A \`v*\` tag exists but no release object does — a release that was deleted by hand, or a tag whose \`release.yml\` run never happened. Nothing at that version is downloadable."
    echo
    while IFS= read -r t; do
      [ -n "$t" ] && echo "- \`$t\` — https://github.com/$REPO/releases/tag/$t"
    done <"$work/no_release"
    echo
  fi

  if [ "$sees_drafts" != "true" ]; then
    echo "### Drafts: not checked"
    echo
    echo "This token has no push access (\`repos/$REPO\`.\`permissions.push\` = \`$sees_drafts\`), and GitHub only lists draft releases to a token that has it. The stale-draft check did not run — it did not pass."
    echo
    problems=$((problems + 1))
  elif [ -s "$work/stale_drafts" ]; then
    problems=$((problems + $(wc -l <"$work/stale_drafts")))
    echo "### Drafts older than ${DRAFT_MAX_AGE_HOURS}h"
    echo
    echo "Since T10 a release publishes itself once \`verify-release\` passes, so a lingering draft means either a failed leg nobody went back to, or a release created before that change. Either way the version is invisible to users."
    echo
    while IFS=$'\t' read -r t created; do
      [ -n "$t" ] && echo "- \`$t\` — created \`$created\` — https://github.com/$REPO/releases/tag/$t"
    done <"$work/stale_drafts"
    echo
  fi

  if [ -s "$work/incomplete" ]; then
    problems=$((problems + $(grep -c '^t' "$work/incomplete" || true)))
    echo "### Incomplete releases"
    echo
    echo "\`scripts/verify-release.sh\` — the same gate \`release.yml\` runs — against the release as it stands today:"
    echo
    while IFS=$'\t' read -r kind line; do
      case "$kind" in
        t) echo; echo "- \`$line\` — https://github.com/$REPO/releases/tag/$line" ;;
        m) echo "  - $line" ;;
      esac
    done <"$work/incomplete"
    echo
  fi

  if [ -s "$work/dangling" ]; then
    problems=$((problems + $(wc -l <"$work/dangling")))
    echo "### Releases whose tag is gone"
    echo
    echo "The release still lists assets, but the tag it names no longer resolves, so nothing can be rebuilt from it."
    echo
    while IFS= read -r t; do
      [ -n "$t" ] && echo "- \`$t\`"
    done <"$work/dangling"
    echo
  fi

  if [ "$problems" -eq 0 ]; then
    echo "No drift: every release-shaped tag has a release, no draft is older than ${DRAFT_MAX_AGE_HOURS}h, and the newest $VERIFY_TAGS releases are complete and fully checksummed."
    echo
  fi

  echo "---"
  echo
  echo "A tag that is *meant* to have no release (a withdrawn version) belongs in \`$IGNORE_FILE\`, one tag per line, with the reason in a \`#\` comment — that is the only way to silence a line here, and it leaves the reason in the repository."
} >"$work/report"

cat "$work/report"

if [ "$problems" -ne 0 ]; then
  echo "$self: $problems drift problem(s) — see the report above." >&2
  exit 1
fi
echo "$self: no release drift." >&2
