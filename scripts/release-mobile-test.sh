#!/usr/bin/env bash
# Tests for scripts/release-mobile.sh against a fake `gh`: the contract guard
# (fleet-mobile's MIN/MAX_HUB_CONTRACT must cover the hub revision the
# claude-fleet tag ships) and that a refused run creates no tag. bash only,
# about a second. PASS/FAIL lines, exit 1 on any failure.
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
S="$REPO/scripts/release-mobile.sh"
PASS=0; FAIL=0

t="$(mktemp -d)"
trap 'rm -rf "$t"' EXIT
HEAD=1111111111111111111111111111111111111111
FLEET_SHA=2222222222222222222222222222222222222222

# The fake answers only what release-mobile.sh asks, and serves each contract
# file ONLY at the ref the guard must read it at (the claude-fleet tag, the
# fleet-mobile main head) — reading it anywhere else is a test failure.
mkdir -p "$t/bin"
cat >"$t/bin/gh" <<EOF
#!/usr/bin/env bash
args="\$*"
case "\$args" in
  *"-X POST"*"repos/martin-janci/fleet-mobile/git/refs"*) echo "\$args" >>"$t/posted" ;;
  *"repos/martin-janci/claude-fleet/git/ref/tags/v1.2.3"*) echo $FLEET_SHA ;;
  *"repos/martin-janci/fleet-mobile/git/ref/tags/"*) echo '{"message":"Not Found"}'; exit 1 ;;
  *"repos/martin-janci/fleet-mobile/commits/main"*) echo $HEAD ;;
  "run list"*"--commit $HEAD"*) echo "completed success" ;;
  *"repos/martin-janci/claude-fleet/contents/crates/fleet-core/src/wire_contract.rs?ref=v1.2.3"*)
    cat "$t/wire_contract.rs" ;;
  *"repos/martin-janci/fleet-mobile/contents/shared/src/commonMain/kotlin/dev/claudefleet/mobile/net/HubContract.kt?ref=$HEAD"*)
    cat "$t/HubContract.kt" ;;
  *) echo "fake gh: unexpected: \$args" >&2; exit 1 ;;
esac
EOF
chmod +x "$t/bin/gh"

hub_rev() { printf '//! doc\npub const CONTRACT_REVISION: u32 = %s;\n' "$1" >"$t/wire_contract.rs"; }
mobile_range() {
  printf 'package x\n/** doc */\nconst val MIN_HUB_CONTRACT: Int = %s\nconst val MAX_HUB_CONTRACT: Int = %s\n' \
    "$1" "$2" >"$t/HubContract.kt"
}

# expect <name> <want exit: 0|nonzero> <want tag created: yes|no> <output must contain>
expect() {
  local name=$1 want_rc=$2 want_tag=$3 want_out=$4 out rc tagged=no
  rm -f "$t/posted"
  out="$(PATH="$t/bin:$PATH" bash "$S" 1.2.3 2>&1)"; rc=$?
  [[ -s "$t/posted" ]] && tagged=yes
  local ok=1
  if [[ $want_rc == 0 ]]; then [[ $rc == 0 ]] || ok=0; else [[ $rc != 0 ]] || ok=0; fi
  [[ $tagged == "$want_tag" ]] || ok=0
  [[ "$out" == *"$want_out"* ]] || ok=0
  if [[ $ok == 1 ]]; then
    PASS=$((PASS + 1)); echo "PASS: $name"
  else
    FAIL=$((FAIL + 1))
    printf 'FAIL: %s (rc=%s, tag created=%s)\n--- want output containing\n%s\n--- got\n%s\n' \
      "$name" "$rc" "$tagged" "$want_out" "$out" >&2
  fi
}

hub_rev 8; mobile_range 0 8
expect "a phone range covering the hub revision tags main" 0 yes "Created v1.2.3"

hub_rev 8; mobile_range 0 7
expect "a phone whose MAX is below the hub revision is refused" nonzero no \
  "accepts hub contracts 0..7, but claude-fleet v1.2.3 ships hub contract 8"

hub_rev 2; mobile_range 3 8
expect "a hub revision below the phone's MIN is refused" nonzero no \
  "accepts hub contracts 3..8, but claude-fleet v1.2.3 ships hub contract 2"

hub_rev 8; printf 'package x\nconst val MIN_HUB_CONTRACT: Int = 0\n' >"$t/HubContract.kt"
expect "an unreadable phone MAX is refused, not trusted" nonzero no "could not read MIN/MAX_HUB_CONTRACT"

printf '//! no revision here\n' >"$t/wire_contract.rs"; mobile_range 0 8
expect "an unreadable hub revision is refused, not trusted" nonzero no "could not read CONTRACT_REVISION"

hub_rev 8; mobile_range 0 8
rm -f "$t/posted"
out="$(PATH="$t/bin:$PATH" RELEASE_DRY_RUN=1 bash "$S" 1.2.3 2>&1)"; rc=$?
if [[ $rc == 0 && ! -s "$t/posted" && "$out" == *"Dry run"* && "$out" == *"hub contract 8"* ]]; then
  PASS=$((PASS + 1)); echo "PASS: a dry run checks the contract and creates nothing"
else
  FAIL=$((FAIL + 1)); printf 'FAIL: dry run (rc=%s)\n%s\n' "$rc" "$out" >&2
fi

echo "release-mobile-test: $PASS passed, $FAIL failed"
[[ $FAIL == 0 ]]
