#!/usr/bin/env bash
# Tests for scripts/verify.sh's plan: which checks a set of changed files
# selects. Runs only `verify.sh --dry-run --files …`, so it needs bash and
# git, builds nothing and takes about a second. PASS/FAIL lines, exit 1 on
# any failure.
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
V="$REPO/scripts/verify.sh"
PASS=0; FAIL=0

# expect <name> <level> <expected plan, one line per check> -- <files...>
expect() {
  local name=$1 level=$2 want=$3; shift 4
  local got
  got="$(VERIFY_HEADLESS="${HEADLESS:-0}" "$V" "$level" --dry-run --files "$@" 2>&1)"
  if [[ "$got" == "$want" ]]; then
    PASS=$((PASS + 1)); echo "PASS: $name"
  else
    FAIL=$((FAIL + 1))
    printf 'FAIL: %s\n--- want\n%s\n--- got\n%s\n' "$name" "$want" "$got" >&2
  fi
}

# expect_line <name> <a line the plan must contain> -- <files...>: for plans
# that follow repo content (which modules read a file), so a new reader does
# not fail this test.
expect_line() {
  local name=$1 want=$2; shift 3
  local got
  got="$(VERIFY_HEADLESS=0 "$V" quick --dry-run --files "$@" 2>&1)"
  if grep -qxF -- "$want" <<< "$got" || grep -qF -- "$want " <<< "$got"; then
    PASS=$((PASS + 1)); echo "PASS: $name"
  else
    FAIL=$((FAIL + 1))
    printf 'FAIL: %s\n--- want a line with\n%s\n--- got\n%s\n' "$name" "$want" "$got" >&2
  fi
}

LINT='verify quick: cargo fmt --all --check
verify quick: cargo fleet-lint'

expect "a module file → its tests" quick "$LINT
verify quick: cargo fleet-test -- service::work::view" -- \
  crates/fleet-core/src/service/work/view.rs

expect "mod.rs and a data file → their directory's modules" quick "$LINT
verify quick: cargo fleet-test -- store store::migrations" -- \
  crates/fleet-core/src/store/mod.rs crates/fleet-core/src/store/migrations/094_changesets.sql

expect "a fixture under testdata → the module above it" quick "$LINT
verify quick: cargo fleet-test -- service" -- \
  crates/fleet-core/src/service/testdata/decide/x.json

expect "the desktop crate" quick "$LINT
verify quick: cargo fleet-test -- backend::tests_routing" -- \
  src-tauri/src/backend/tests_routing.rs

expect "a crate root → every unit test" quick "$LINT
verify quick: cargo fleet-test" -- \
  crates/fleet-core/src/lib.rs

expect "Cargo.lock → every unit test" quick "$LINT
verify quick: cargo fleet-test" -- \
  Cargo.lock

expect "a file outside src/ in a crate → every unit test" quick "$LINT
verify quick: cargo fleet-test" -- \
  crates/fleet-core/pages/usage.json

expect "an integration test file → that test target" quick "$LINT
verify quick: cargo test --workspace --test frames" -- \
  crates/fleet-proto/tests/frames.rs

expect "a helper under tests/ → every test target" quick "$LINT
verify quick: cargo test --workspace" -- \
  crates/fleet-proto/tests/common/mod.rs

expect_line "a .ts mirror → the Rust tests that read it" "verify quick: cargo fleet-test -- events" -- \
  src/lib/events.ts
expect_line "a .ts mirror → vitest related" "verify quick: pnpm exec vitest related --run src/lib/events.ts" -- \
  src/lib/events.ts
expect_line "a guide a Rust test reads" "verify quick: cargo fleet-test -- service::settings" -- \
  docs/updates.md

expect "a package file → the whole frontend suite" quick "verify quick: pnpm install --frozen-lockfile
verify quick: pnpm run check
verify quick: pnpm run test" -- \
  package.json

expect "files no check covers are named" quick "verify quick: nothing to check for this change.
verify quick: not covered here (run 'scripts/verify.sh full'): .github/workflows/ci.yml scripts/ag-test.sh" -- \
  .github/workflows/ci.yml scripts/ag-test.sh

HEADLESS=1 expect "headless Linux → the crates that build without Tauri" quick "verify quick: cargo fmt --all --check
verify quick: cargo clippy -p fleet-core -p fleet-hub -p fleet-proto -p fleet-agent -p fleet-update --all-targets -- -D warnings
verify quick: cargo test -p fleet-core -p fleet-hub -p fleet-proto -p fleet-agent -p fleet-update --lib --bins -- service::health" -- \
  crates/fleet-core/src/service/health.rs

expect "full, Rust only" full "verify full: scripts/ci-local.sh --rust-only" -- \
  crates/fleet-core/src/service/health.rs
expect "full, frontend only" full "verify full: scripts/ci-local.sh --frontend-only" -- \
  src/lib/components/Foo.svelte
expect "full, both" full "verify full: scripts/ci-local.sh " -- \
  src/lib/events.ts crates/fleet-core/src/events.rs

expect "remote → the Buildkite client" remote "verify remote: scripts/buildkite-verify.sh" -- \
  crates/fleet-core/src/service/health.rs

echo "verify-test: $PASS passed, $FAIL failed"
[[ $FAIL == 0 ]]
