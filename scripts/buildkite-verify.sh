#!/usr/bin/env bash
# Runs the full verification of the pushed HEAD on the persistent Buildkite
# builder (docs/buildkite.md) and waits for it: the remote form of
# `scripts/verify.sh full`, for an agent whose own machine should not pay the
# full suite. Also reached as `scripts/verify.sh remote`.
#
#   scripts/buildkite-verify.sh                # build HEAD, wait, exit 0 iff passed
#   scripts/buildkite-verify.sh --no-wait      # start the build, print its URL
#   scripts/buildkite-verify.sh --timeout 3600 # give up waiting after N s (default 3600)
#   scripts/buildkite-verify.sh --branch B --commit C
#                                              # build commit C (default HEAD) as branch B
#                                              # (default: the current branch; needed
#                                              # on a detached HEAD)
#
# The builder checks out the commit from GitHub, so HEAD must be pushed to
# origin under the current branch; uncommitted changes are not part of the
# build (a warning says so). On a failure it prints the end of each failed
# job's log; Ctrl-C cancels the build.
#
# Environment:
#   BUILDKITE_API_TOKEN   required; scopes read_builds, write_builds,
#                         read_build_logs
#   BUILDKITE_ORG         required; the organisation slug
#   BUILDKITE_PIPELINE    the pipeline slug (default: claude-fleet)
#   BUILDKITE_API_URL     default https://api.buildkite.com/v2
#   BUILDKITE_POLL_SECS   default 10
#   BUILDKITE_LOG_LINES   lines of a failed job's log to print (default 150)
#   BUILDKITE_VERIFY_ASSUME_PUSHED=1
#                         skip the "is the commit on origin" check
#                         (scripts/buildkite-verify-test.sh)
set -euo pipefail

usage() { sed -n '2,/^set -euo/p' "$0" | sed -e '$d' -e 's/^# \{0,1\}//'; }

wait_build=1
timeout=3600
branch=""
commit=HEAD
while [[ $# -gt 0 ]]; do
  case "$1" in
    --no-wait) wait_build=0; shift ;;
    --timeout) timeout=${2:?--timeout needs seconds}; shift 2 ;;
    --branch) branch=${2:?--branch needs a name}; shift 2 ;;
    --commit) commit=${2:?--commit needs a ref}; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "buildkite-verify: unknown argument: $1 (see --help)" >&2; exit 2 ;;
  esac
done

: "${BUILDKITE_API_TOKEN:?buildkite-verify: set BUILDKITE_API_TOKEN (docs/buildkite.md)}"
: "${BUILDKITE_ORG:?buildkite-verify: set BUILDKITE_ORG to the organisation slug (docs/buildkite.md)}"
pipeline=${BUILDKITE_PIPELINE:-claude-fleet}
api=${BUILDKITE_API_URL:-https://api.buildkite.com/v2}
poll=${BUILDKITE_POLL_SECS:-10}
log_lines=${BUILDKITE_LOG_LINES:-150}
command -v curl >/dev/null || { echo "buildkite-verify: needs curl" >&2; exit 2; }
command -v python3 >/dev/null || { echo "buildkite-verify: needs python3 (JSON)" >&2; exit 2; }

ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"
sha="$(git rev-parse --verify "$commit^{commit}")"
if [[ -z "$branch" ]]; then
  branch="$(git rev-parse --abbrev-ref HEAD)"
  [[ "$branch" == HEAD ]] && { echo "buildkite-verify: detached HEAD; check out a branch or pass --branch NAME" >&2; exit 2; }
fi
subject="$(git log -1 --format=%s "$sha")"

if [[ "${BUILDKITE_VERIFY_ASSUME_PUSHED:-}" != 1 ]]; then
  remote_sha="$(git ls-remote origin "refs/heads/$branch" 2>/dev/null | cut -f1)"
  if [[ "$remote_sha" != "$sha" ]]; then
    echo "buildkite-verify: origin/$branch is not at ${sha:0:12} (${remote_sha:-missing}); the builder builds what GitHub has." >&2
    echo "buildkite-verify: push first: git push -u origin $branch" >&2
    exit 2
  fi
fi
if [[ -n "$(git status --porcelain)" ]]; then
  echo "buildkite-verify: warning: uncommitted changes are not part of the remote build" >&2
fi

base="$api/organizations/$BUILDKITE_ORG/pipelines/$pipeline/builds"

# api METHOD URL [JSON BODY] -> response body on stdout; fails on HTTP >= 400.
api_call() {
  local method=$1 url=$2 body=${3:-} out code
  out="$(mktemp)"
  if [[ -n "$body" ]]; then
    code="$(curl -sS -o "$out" -w '%{http_code}' -X "$method" \
      -H "Authorization: Bearer $BUILDKITE_API_TOKEN" -H 'Content-Type: application/json' \
      --data "$body" "$url")" || { rm -f "$out"; return 1; }
  else
    code="$(curl -sS -o "$out" -w '%{http_code}' -X "$method" \
      -H "Authorization: Bearer $BUILDKITE_API_TOKEN" -H 'Accept: application/json' "$url")" || { rm -f "$out"; return 1; }
  fi
  if [[ "$code" -ge 400 ]]; then
    echo "buildkite-verify: $method $url -> HTTP $code: $(head -c 300 "$out")" >&2
    rm -f "$out"
    return 1
  fi
  cat "$out"
  rm -f "$out"
}

# json EXPR < body: evaluates a Python expression over the parsed body `b`.
json() { python3 -c 'import json,sys; b=json.load(sys.stdin); r=eval(sys.argv[1]); print("" if r is None else r)' "$1"; }

body="$(python3 -c 'import json,sys; print(json.dumps({"commit":sys.argv[1],"branch":sys.argv[2],"message":sys.argv[3],"ignore_pipeline_branch_filters":True}))' "$sha" "$branch" "$subject")"
created="$(api_call POST "$base" "$body")"
number="$(json 'b["number"]' <<< "$created")"
web_url="$(json 'b.get("web_url")' <<< "$created")"
echo "buildkite-verify: build #$number for $branch @ ${sha:0:12}: $web_url"
[[ $wait_build == 0 ]] && exit 0

cancel() {
  echo
  echo "buildkite-verify: interrupted; cancelling build #$number" >&2
  api_call PUT "$base/$number/cancel" >/dev/null || true
  exit 130
}
trap cancel INT TERM

start=$(date +%s)
last=""
errors=0
while :; do
  # A blip in the network should not end the wait: the build keeps running.
  if ! build="$(api_call GET "$base/$number")"; then
    errors=$((errors + 1))
    if (( errors >= 5 )); then
      echo "buildkite-verify: 5 failed polls in a row; the build continues: $web_url" >&2
      exit 3
    fi
    sleep "$poll"
    continue
  fi
  errors=0
  state="$(json 'b["state"]' <<< "$build")"
  if [[ "$state" != "$last" ]]; then
    echo "buildkite-verify: $(( $(date +%s) - start ))s  $state"
    last=$state
  fi
  case "$state" in
    passed|failed|canceled|skipped|not_run|blocked) break ;;
  esac
  if (( $(date +%s) - start > timeout )); then
    echo "buildkite-verify: still $state after ${timeout}s; leaving it running: $web_url" >&2
    exit 3
  fi
  sleep "$poll"
done
trap - INT TERM

# One line per script job: id, state, label, URL.
jobs="$(mktemp)"
json '"\n".join("%s\t%s\t%s\t%s" % (j.get("id",""), j.get("state",""), j.get("name") or j.get("label") or "", j.get("web_url") or "") for j in b.get("jobs", []) if j.get("type") == "script")' <<< "$build" > "$jobs" || true
while IFS=$'\t' read -r jid jstate jname jurl; do
  [[ -z "$jid" ]] && continue
  echo "buildkite-verify: job $jstate: $jname"
  if [[ "$jstate" != passed ]]; then
    echo "----- last $log_lines lines of '$jname' ($jurl)"
    # Strip ANSI colour and Buildkite's timestamp markers (ESC _bk;t=… BEL).
    curl -sS -H "Authorization: Bearer $BUILDKITE_API_TOKEN" -H 'Accept: text/plain' \
      "$base/$number/jobs/$jid/log" \
      | sed -e $'s/\x1b_bk;t=[0-9]*\x07//g' -e $'s/\x1b\\[[0-9;]*[A-Za-z]//g' \
      | tail -n "$log_lines" || echo "(log unavailable)"
    echo "-----"
  fi
done < "$jobs"
rm -f "$jobs"

echo "buildkite-verify: build #$number $state in $(( $(date +%s) - start ))s: $web_url"
[[ "$state" == passed ]]
