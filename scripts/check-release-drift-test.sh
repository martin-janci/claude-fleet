#!/usr/bin/env bash
# Tests for scripts/check-release-drift.sh against a fake `gh` and `curl`:
# what the draft check concludes from zero drafts, and which releases the
# completeness check picks. The script runs from a copy beside a stub
# verify-release.sh (which records the tags it is asked about) and a stub
# release.sh (the image pin's path). bash only, about a second.
# PASS/FAIL lines, exit 1 on any failure.
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PASS=0; FAIL=0

t="$(mktemp -d)"
trap 'rm -rf "$t"' EXIT
mkdir -p "$t/bin" "$t/tree/scripts"
cp "$REPO/scripts/check-release-drift.sh" "$t/tree/scripts/"
S="$t/tree/scripts/check-release-drift.sh"

cat >"$t/tree/scripts/verify-release.sh" <<EOF
#!/usr/bin/env bash
echo "\$TAG" >>"$t/verified"
case " \${FAKE_INCOMPLETE:-} " in *" \$TAG "*) echo "::error title=incomplete::\$TAG lacks an asset"; exit 1 ;; esac
exit 0
EOF
cat >"$t/tree/scripts/release.sh" <<'EOF'
#!/usr/bin/env bash
echo compose.yml
EOF
chmod +x "$t/tree/scripts/"*.sh
echo '    image: ghcr.io/x/fleet-hub:1.0.0' >"$t/tree/compose.yml"

# The fake answers the three calls the script makes. The releases listing is
# served as the TSV the script's --jq would have produced:
#   id, tag, draft, prerelease, created_at, stale
cat >"$t/bin/gh" <<EOF
#!/usr/bin/env bash
args="\$*"
case "\$args" in
  *"repos/x/y/tags?"*) cat "$t/tags" ;;
  *"repos/x/y/releases?"*) cat "$t/releases" ;;
  "api repos/x/y --jq"*) echo probed >>"$t/probed"; echo "\${FAKE_PROBE:-false}" ;;
  *) echo "fake gh: unexpected: \$args" >&2; exit 1 ;;
esac
EOF
cat >"$t/bin/curl" <<'EOF'
#!/usr/bin/env bash
case "$*" in *"ghcr.io/token"*) echo '{"token":"t"}' ;; *) printf 200 ;; esac
EOF
chmod +x "$t/bin/gh" "$t/bin/curl"

ok() { PASS=$((PASS + 1)); echo "PASS: $1"; }
bad() { FAIL=$((FAIL + 1)); echo "FAIL: $1"; [ -z "${2:-}" ] || sed 's/^/    /' "$2"; }

# run <name> "<script args>" [VAR=value ...]: runs the script, leaving its report in $t/out,
# its exit code in $code and the tags it verified in $t/verified.
run() {
  local name="$1" args="$2"; shift 2
  : >"$t/verified"; rm -f "$t/probed"
  # shellcheck disable=SC2086  # $args is a list of words
  env PATH="$t/bin:$PATH" REPO=x/y IGNORE_FILE="$t/none" "$@" bash "$S" $args >"$t/out" 2>"$t/err"
  code=$?
  cat "$t/err" >>"$t/out"
  CASE="$name"
}
expect_code() { if [ "$code" = "$1" ]; then ok "$CASE: exit $1"; else bad "$CASE: exit $code, expected $1" "$t/out"; fi; }
expect_out() { if grep -qF -- "$1" "$t/out"; then ok "$CASE: says '$1'"; else bad "$CASE: does not say '$1'" "$t/out"; fi; }
refute_out() { if grep -qF -- "$1" "$t/out"; then bad "$CASE: says '$1'" "$t/out"; else ok "$CASE: does not say '$1'"; fi; }
expect_verified() {
  local want="$1" got
  got="$(tr '\n' ' ' <"$t/verified" | sed 's/ $//')"
  if [ "$got" = "$want" ]; then ok "$CASE: verified [$want]"; else bad "$CASE: verified [$got], expected [$want]"; fi
}

row() { printf '%s\t%s\t%s\t%s\t2026-01-01T00:00:00Z\t%s\n' "$1" "$2" "$3" "$4" "$5"; }

# --- zero drafts, under an Actions token --------------------------------------
printf 'v1.0.0\nv0.9.0\n' >"$t/tags"
{ row 1 v1.0.0 false false true; row 2 v0.9.0 false false true; } >"$t/releases"

run "no drafts, DRIFT_SEES_DRAFTS=1" "" DRIFT_SEES_DRAFTS=1 FAKE_PROBE=false
expect_code 0
expect_out "No drift"
refute_out "Drafts: not checked"
if [ -e "$t/probed" ]; then bad "$CASE: probed the token's permissions anyway"; else ok "$CASE: no permission probe"; fi

run "no drafts, a read-only user token" "" FAKE_PROBE=false
expect_code 1
expect_out "Drafts: not checked"

run "no drafts, a user token with push access" "" FAKE_PROBE=true
expect_code 0

# --- which releases are verified ----------------------------------------------
# Dev releases sort above every stable one and outnumber --verify-tags: the
# stable ones must still be verified, with only the newest published dev.
: >"$t/tags"; : >"$t/releases"
i=10
for n in 1 2 3 4 5 6; do
  echo "v1.0.1-dev.$n.gabcdef0" >>"$t/tags"
  row "$i" "v1.0.1-dev.$n.gabcdef0" false true true >>"$t/releases"; i=$((i + 1))
done
for v in v1.0.0 v0.9.0 v0.8.0 v0.7.0-rc.1; do
  echo "$v" >>"$t/tags"; row "$i" "$v" false false true >>"$t/releases"; i=$((i + 1))
done
# A dev draft still being built (not stale), and one nobody published (stale).
echo "v1.0.1-dev.7.gabcdef0" >>"$t/tags"; row 30 v1.0.1-dev.7.gabcdef0 true true false >>"$t/releases"
# A stable draft inside its build window.
echo "v1.1.0" >>"$t/tags"; row 31 v1.1.0 true false false >>"$t/releases"

run "dev releases outnumber the limit" "--verify-tags 3 --verify-dev-tags 2"
expect_verified "v1.0.0 v0.9.0 v0.8.0 v1.0.1-dev.6.gabcdef0 v1.0.1-dev.5.gabcdef0"
expect_code 0

run "an incomplete stable release behind many dev ones" "--verify-tags 3" FAKE_INCOMPLETE=v0.9.0
expect_code 1
expect_out "v0.9.0 lacks an asset"

run "a draft still being built is not called incomplete" "--verify-tags 3" FAKE_INCOMPLETE="v1.1.0 v1.0.1-dev.7.gabcdef0"
expect_code 0
refute_out "Incomplete releases"

# The same stable draft once it is past the cutoff: reported as stale, and verified.
sed -i.bak $'s/^31\tv1.1.0\ttrue\tfalse\t\\(.*\\)\tfalse$/31\tv1.1.0\ttrue\tfalse\t\\1\ttrue/' "$t/releases"
run "a stale stable draft is verified" "--verify-tags 3" FAKE_INCOMPLETE=v1.1.0
expect_code 1
expect_out "Drafts older than"
expect_out "v1.1.0 lacks an asset"

echo "check-release-drift-test: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
