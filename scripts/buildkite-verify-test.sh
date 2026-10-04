#!/usr/bin/env bash
# Tests for scripts/buildkite-verify.sh against a fake Buildkite REST API on
# loopback (python3's http.server): the build it creates, polling to a final
# state, the exit code, a failed job's log with Buildkite's escape codes
# stripped, and the auth header. Needs bash, git, curl and python3; builds
# nothing and takes a few seconds. PASS/FAIL lines, exit 1 on any failure.
# check() evals its condition, so conditions are single-quoted on purpose and
# the variables they name are read there.
# shellcheck disable=SC2016,SC2034
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP_BASE="/tmp/claude-$(id -u)"
[ -d "$TMP_BASE" ] || TMP_BASE=/tmp
ROOT="$(mktemp -d -p "$TMP_BASE" bkv.XXXXXX)"
PASS=0; FAIL=0
pass() { PASS=$((PASS + 1)); echo "PASS: $*"; }
fail() { FAIL=$((FAIL + 1)); echo "FAIL: $*" >&2; }
# check <name> <condition, as a shell string>: on failure, shows the output.
check() {
  if eval "$2"; then pass "$1"; else fail "$1"; sed 's/^/  | /' "$ROOT/out" >&2; fi
}

cat > "$ROOT/fake.py" <<'PY'
import json, sys, os
from http.server import BaseHTTPRequestHandler, HTTPServer
root = sys.argv[1]
polls = {"n": 0}
class H(BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def _auth(self):
        if self.headers.get("Authorization") != "Bearer good-token":
            self.send_response(401); self.end_headers(); self.wfile.write(b'{"message":"Unauthorized"}'); return False
        return True
    def _json(self, obj, code=200):
        b = json.dumps(obj).encode()
        self.send_response(code); self.send_header("Content-Type", "application/json"); self.end_headers(); self.wfile.write(b)
    def do_POST(self):
        if not self._auth(): return
        body = self.rfile.read(int(self.headers.get("Content-Length", 0)))
        open(os.path.join(root, "created.json"), "wb").write(body)
        open(os.path.join(root, "path.txt"), "w").write(self.path)
        self._json({"number": 7, "state": "scheduled", "web_url": "https://buildkite.example/b/7"}, 201)
    def do_PUT(self):
        if not self._auth(): return
        open(os.path.join(root, "cancelled"), "w").write(self.path)
        self._json({"number": 7, "state": "canceling"})
    def do_GET(self):
        if not self._auth(): return
        final = open(os.path.join(root, "scenario")).read().strip()
        if self.path.endswith("/log"):
            self.send_response(200); self.send_header("Content-Type", "text/plain"); self.end_headers()
            self.wfile.write(b"\x1b_bk;t=1700000000000\x07compiling\n\x1b[31merror[E0308]: mismatched types\x1b[0m\n")
            return
        polls["n"] += 1
        state = ["scheduled", "running"][polls["n"] - 1] if polls["n"] <= 2 else final
        jobs = [{"id": "job-1", "type": "script", "name": ":rust: verify full", "web_url": "https://buildkite.example/b/7#job-1",
                 "state": state if state in ("passed", "failed") else "running"},
                {"id": "wait-1", "type": "waiter"}]
        self._json({"number": 7, "state": state, "web_url": "https://buildkite.example/b/7", "jobs": jobs})
srv = HTTPServer(("127.0.0.1", 0), H)
open(os.path.join(root, "port"), "w").write(str(srv.server_address[1]))
srv.serve_forever()
PY

start_fake() {
  rm -f "$ROOT/port" "$ROOT/created.json" "$ROOT/cancelled"
  echo "$1" > "$ROOT/scenario"
  python3 "$ROOT/fake.py" "$ROOT" & FAKE=$!
  for _ in $(seq 50); do [ -s "$ROOT/port" ] && break; sleep 0.1; done
  PORT="$(cat "$ROOT/port")"
}
stop_fake() { kill "$FAKE" 2>/dev/null; wait "$FAKE" 2>/dev/null; }
run() {
  ( cd "$REPO" && BUILDKITE_API_URL="http://127.0.0.1:$PORT" BUILDKITE_ORG=acme BUILDKITE_PIPELINE=claude-fleet \
    BUILDKITE_POLL_SECS=0 BUILDKITE_VERIFY_ASSUME_PUSHED=1 BUILDKITE_API_TOKEN="${TOKEN:-good-token}" \
    "$REPO/scripts/buildkite-verify.sh" --branch bk-test "$@" ) > "$ROOT/out" 2>&1
}

head_sha="$(git -C "$REPO" rev-parse HEAD)"

start_fake passed
run; rc=$?
check "a passed build exits 0" '[ $rc = 0 ]'
check "the build is for HEAD" 'grep -q "\"commit\": \"$head_sha\"" "$ROOT/created.json"'
check "the branch is the one passed" 'grep -q "\"branch\": \"bk-test\"" "$ROOT/created.json"'
check "branch filters are ignored" 'grep -q "\"ignore_pipeline_branch_filters\": true" "$ROOT/created.json"'
check "org and pipeline in the URL" 'grep -qx /organizations/acme/pipelines/claude-fleet/builds "$ROOT/path.txt"'
check "it reports each state and the result" 'grep -q running "$ROOT/out" && grep -q "build #7 passed" "$ROOT/out"'
stop_fake

start_fake failed
run; rc=$?
check "a failed build exits 1" '[ $rc = 1 ]'
check "the failed job's log, escape codes stripped" 'grep -qxF "error[E0308]: mismatched types" "$ROOT/out"'
check "only script jobs are listed" '! grep -q wait-1 "$ROOT/out"'
stop_fake

start_fake passed
TOKEN=wrong run; rc=$?
check "a rejected token fails with the HTTP status" '[ $rc != 0 ] && grep -q "HTTP 401" "$ROOT/out"'
stop_fake

start_fake passed
run --no-wait; rc=$?
check "--no-wait prints the URL and returns" '[ $rc = 0 ] && grep -q https://buildkite.example/b/7 "$ROOT/out" && ! grep -q passed "$ROOT/out"'
stop_fake

( cd "$REPO" && env -u BUILDKITE_API_TOKEN BUILDKITE_ORG=acme "$REPO/scripts/buildkite-verify.sh" ) > "$ROOT/out" 2>&1; rc=$?
check "a missing token is named" '[ $rc != 0 ] && grep -q BUILDKITE_API_TOKEN "$ROOT/out"'

( cd "$REPO" && BUILDKITE_API_TOKEN=x BUILDKITE_ORG=acme BUILDKITE_VERIFY_ASSUME_PUSHED=1 \
  "$REPO/scripts/buildkite-verify.sh" --branch bk-test --commit no-such-ref ) > "$ROOT/out" 2>&1; rc=$?
check "an unknown --commit is refused before any request" '[ $rc != 0 ] && ! grep -q "build #" "$ROOT/out"'

rm -rf "$ROOT"
echo "buildkite-verify-test: $PASS passed, $FAIL failed"
[ "$FAIL" = 0 ]
