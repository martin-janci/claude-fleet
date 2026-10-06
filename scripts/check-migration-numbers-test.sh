#!/usr/bin/env bash
# Tests for scripts/check-migration-numbers.sh against a throwaway repo whose
# refs/remotes/origin/main plays main. bash + git, about a second. PASS/FAIL
# lines, exit 1 on any failure.
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
C="$REPO/scripts/check-migration-numbers.sh"
PASS=0; FAIL=0
T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT

D=crates/fleet-core/migrations
g() { git -C "$T/r" -c user.email=t@t -c user.name=t -c commit.gpgsign=false "$@"; }

# A fresh repo: main has 001 and 002; the branch `work` starts there.
mkdir -p "$T/r/$D"
g init -q -b work
touch "$T/r/$D/001_init.sql" "$T/r/$D/002_people.sql"
g add -A && g commit -qm main
g update-ref refs/remotes/origin/main HEAD

# main moves on: it gains <file>, the branch does not see it.
main_adds() {
  g checkout -q --detach origin/main
  touch "$T/r/$D/$1"; g add -A; g commit -qm "main: $1"
  g update-ref refs/remotes/origin/main HEAD
  g checkout -q work
}

# expect <name> <exit code> <text the output must contain> -- <script args...>
expect() {
  local name=$1 want=$2 text=$3; shift 4
  local out code
  out="$(cd "$T/r" && bash "$C" "$@" 2>&1)"; code=$?
  if [[ $code == "$want" && "$out" == *"$text"* ]]; then
    PASS=$((PASS + 1)); echo "PASS: $name"
  else
    FAIL=$((FAIL + 1))
    printf 'FAIL: %s (want exit %s with "%s")\n--- got exit %s\n%s\n' "$name" "$want" "$text" "$code" "$out" >&2
  fi
}

expect "nothing added" 0 "ok (0 added; origin/main's highest is 002)" --

touch "$T/r/$D/003_tasks.sql"
expect "a number above main's passes (working tree, untracked)" 0 "ok (1 added" --

g add -A && g commit -qm "branch: 003"
main_adds 003_assets.sql
expect "main took the number: fails" 1 "003_tasks.sql: number 3 is not above origin/main's highest, 003" --
expect "the hint names the next free number" 1 "from 004" --
expect "--ref checks a commit's tree" 1 "number 3 is not above" -- --ref HEAD

g mv "$D/003_tasks.sql" "$D/004_tasks.sql" && g commit -qm renumber
expect "renumbered above main: passes" 0 "ok (1 added" -- --ref HEAD

touch "$T/r/$D/004_other.sql"
expect "two files share a number" 1 "number 004 is used by more than one file" --
rm -f "$T/r/$D/004_other.sql"

touch "$T/r/$D/5_bad.sql"
expect "a misnamed file fails" 1 "5_bad.sql: not named NNN_<topic>.sql" --
rm -f "$T/r/$D/5_bad.sql"

expect "a base that does not resolve skips the comparison" 0 "base not checked" -- --base refs/remotes/nowhere/main
touch "$T/r/$D/004_other.sql"
expect "...but not the shared-number check" 1 "number 004 is used by more than one file" -- --base refs/remotes/nowhere/main
rm -f "$T/r/$D/004_other.sql"
expect "an unknown argument" 2 "unknown argument" -- --bogus

echo
echo "check-migration-numbers-test: $PASS passed, $FAIL failed"
[[ $FAIL == 0 ]]
