#!/usr/bin/env bash
# Assert that ci.yml has actually passed on one specific commit.
#
# `scripts/release.sh` calls this before it tags: a tag is the one irreversible
# step of a release (pushing it builds and — since T10 — PUBLISHES), and until
# now nothing stopped a release being cut from a commit whose CI was red or had
# never run at all.
#
# WHAT "GREEN" MEANS HERE, and why each case is a refusal rather than a pass.
# The question is asked about a SHA, never about a branch: ci.yml runs on
# `push: branches: [main]` and on `pull_request`, so every commit that lands on
# main has its own run keyed to its own sha, and "main is green" would happily
# answer with some other commit's run.
#
#   no run at all for this sha   -> REFUSE. This is the state a just-pushed (or
#                                   never-pushed) HEAD is in, and it is exactly
#                                   the state a check that cannot tell it from
#                                   "green" would wave through. Wait for CI, or
#                                   push the commit first.
#   newest run not completed     -> REFUSE (queued / in_progress / waiting /
#                                   requested / pending). The answer is not
#                                   stable yet; asking again in a minute is
#                                   free, un-cutting a release is not.
#   newest run completed, but    -> REFUSE, naming the conclusion: failure,
#   conclusion != success           cancelled, timed_out, action_required,
#                                   neutral, stale — and `skipped`, which is a
#                                   run that decided not to test anything and
#                                   is not evidence of anything either.
#   newest run succeeded         -> PASS.
#
# "Newest" is by creation time, so a re-run that fixed a flake counts and an
# older green run cannot outvote a newer red one. A run still in flight beats
# an older green run too — deliberately: something is testing this sha right
# now and its verdict is the one that matters.
#
# WHAT THIS CANNOT COVER, and nothing here should imply otherwise: release.sh
# checks the PRE-release HEAD. It then creates `chore(release): vX.Y.Z` on top
# of it and tags THAT commit, which by construction no CI run has ever seen.
# The release commit only touches version carriers, the lockfile and the
# CHANGELOG, and release.yml's own `version-consistency` job re-checks exactly
# those on the tag — that is the cover for it, not this script.
#
# Usage: scripts/check-ci-green.sh [<sha>]        (default: HEAD)
# Env:   REPO      owner/repo (default: derived from `origin`)
#        WORKFLOW  workflow file name (default: ci.yml)
# Needs `gh` authenticated (read-only: one API GET).
# Exit 0 green, 1 not green, 2 usage / tooling problem (no gh, no network, no
# repo) — a tooling problem is never reported as green.
set -euo pipefail

self="$(basename "$0")"
die() { echo "$self: $1" >&2; exit "${2:-1}"; }

[ $# -le 1 ] || die "usage: $self [<sha>]" 2

command -v gh >/dev/null 2>&1 || die "gh is not installed — install it, or set RELEASE_SKIP_CI_CHECK=1 to cut the release without this check" 2
command -v git >/dev/null 2>&1 || die "git is not installed" 2

SHA="${1:-$(git rev-parse HEAD)}"
case "$SHA" in
  *[!0-9a-fA-F]* | '') die "not a commit sha: '$SHA'" 2 ;;
esac

WORKFLOW="${WORKFLOW:-ci.yml}"
case "$WORKFLOW" in
  *[!0-9A-Za-z._-]* | '') die "not a workflow file name: '$WORKFLOW'" 2 ;;
esac

if [ -z "${REPO:-}" ]; then
  # `owner/repo` out of the origin remote, ssh or https, with or without .git.
  origin="$(git config --get remote.origin.url || true)"
  [ -n "$origin" ] || die "no 'origin' remote and REPO is unset" 2
  REPO="$(printf '%s\n' "$origin" | sed -E 's#^git@[^:]+:##; s#^[a-z]+://[^/]+/##; s#\.git$##')"
fi
case "$REPO" in
  */*) ;;
  *) die "REPO is not owner/repo: '$REPO'" 2 ;;
esac

# One GET. `head_sha` is filtered server-side, so this is exact even when the
# commit is thousands of runs back, which a `gh run list --limit N` scan is not.
#
# Newest first: the API orders by created_at desc, but sort explicitly rather
# than trust that, and carry the fields the messages below need. `gh api --jq`
# rather than a `| jq` pipe, so this needs no jq on the machine cutting the
# release (gh embeds one).
if ! newest="$(gh api "repos/$REPO/actions/workflows/$WORKFLOW/runs?head_sha=$SHA&per_page=100" \
  --jq '[.workflow_runs[]? | {status, conclusion, event, created_at, html_url}]
        | sort_by(.created_at) | reverse | .[0] // empty
        | [.status, (.conclusion // "none"), .event, .created_at, .html_url] | @tsv' 2>&1)"; then
  printf '%s\n' "$newest" >&2
  die "could not ask GitHub about $WORKFLOW runs for $SHA (auth? network? no such workflow?) — this is NOT a pass" 2
fi

if [ -z "$newest" ]; then
  die "no $WORKFLOW run exists for commit $SHA in $REPO — CI has never tested this commit
  (is it pushed? has the run been started yet? has it been garbage-collected?)
  Wait for the run, or set RELEASE_SKIP_CI_CHECK=1 to cut the release without this check."
fi

# No field may be empty: tab is an IFS *whitespace* character, so bash collapses
# a run of them and an empty field would silently shift every field after it
# (which is why `conclusion` above is `// "none"`, not `// ""`).
IFS=$'\t' read -r status conclusion event created url <<<"$newest"

if [ "$status" != "completed" ]; then
  die "the newest $WORKFLOW run for $SHA is '$status', not finished — no verdict yet
  $url (started $created, event $event)
  Wait for it to finish and re-run."
fi

if [ "$conclusion" != "success" ]; then
  die "the newest $WORKFLOW run for $SHA concluded '$conclusion'
  $url (started $created, event $event)
  Fix it and let CI pass on the commit you are about to release."
fi

echo "$self: $WORKFLOW is green on $SHA ($url)"
