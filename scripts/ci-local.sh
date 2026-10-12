#!/usr/bin/env bash
# Local mirror of .github/workflows/ci.yml. Runs the same steps, in the same
# order, so a green run here should mean a green run in CI:
#
#   migrations:    scripts/check-migration-numbers.sh --fetch — every
#                  migration this tree adds is numbered above origin/main's
#                  highest, and no two share a number (main took a branch's
#                  number four times in one week); then its own test,
#                  scripts/check-migration-numbers-test.sh and
#                  scripts/renumber-migrations-test.sh. Every mode.
#   version job:   scripts/check-version-consistency.sh — the six version
#                  carriers, their Cargo.lock entries and fleet-core's
#                  allowlisted 0.1.0; then a smoke test of
#                  scripts/release-assets.sh, the manifest release.yml builds
#                  its matrices from. Both run in every mode: they take
#                  seconds and cost nothing, and between them they are what
#                  keeps a hand-edited carrier or a broken asset manifest
#                  from reaching a tag.
#   ag job:        scripts/ag-test.sh — the agent launcher (tools/ag) against
#                  fake claude/codex binaries. CI also runs this under macOS's
#                  stock /bin/bash 3.2 and shellchecks it on Linux; this
#                  script only runs the bash test, on whatever bash is first
#                  on PATH. Then scripts/voice-arecord-test.sh (tools/voice/
#                  arecord against fake servers), and its shellcheck when
#                  shellcheck is installed.
#   rust job:      cargo fmt --all --check
#                  cargo clippy --workspace --all-targets -- -D warnings
#                  cargo test --workspace
#                  cargo deny check
#                  cargo build -p fleet-hub --locked   (mirrors the hub-headless CI job)
#                  cargo build -p fleet-agent --locked (same job; the agent must
#                  build with neither the Tauri libs nor fleet-core)
#                  scripts/release-update-scripts-test.sh (same job; skipped
#                  without minisign)
#                  On Linux without the Tauri system libs (no gtk+-3.0 via
#                  pkg-config; macOS and Windows need none), it runs a
#                  headless subset instead:
#                  fmt, clippy/test/build scoped to fleet-core, fleet-hub,
#                  fleet-proto and fleet-agent, and cargo deny check.
#   frontend job:  pnpm install --frozen-lockfile
#                  pnpm run check
#                  pnpm run test
#                  pnpm run build
#                  pnpm audit --audit-level=high (CI_LOCAL_AUDIT=warn reports
#                  a failure and goes on; the pre-commit hook sets it, so an
#                  advisory already on main does not block a commit — CI and
#                  a plain run of this script still fail on it)
#   hub-e2e:       scripts/hub-e2e.sh, opt-in via --hub-e2e (mirrors the
#                  hub-headless CI job's e2e step; see that script's own
#                  header), work-graph leg included: like CI, it builds a
#                  hub with the test-only `e2e` feature into
#                  $CARGO_TARGET_DIR/e2e and passes it as WBIN. Skipped with a message if
#                  tmux is missing.
#                  NOT part of the default run: it drives real fleet-hub /
#                  fleet-agent processes and, for one of its three hubs,
#                  manages this machine's own tmux server directly, which a
#                  developer may already have a real hub or session on.
#   updater-e2e:   scripts/updater-e2e.sh, opt-in via --updater-e2e: fleet-updater
#                  against this machine's Docker daemon and a local registry
#                  (a good image, one that crashes on start, one that migrates
#                  and never gets ready). Skipped with a message without a
#                  reachable Docker daemon or minisign.
#
# Usage:
#   scripts/ci-local.sh                 # everything (rust first, then frontend)
#   scripts/ci-local.sh --rust-only
#   scripts/ci-local.sh --frontend-only
#   scripts/ci-local.sh --hub-e2e       # also run scripts/hub-e2e.sh (opt-in; see above)
#   scripts/ci-local.sh --updater-e2e   # also run scripts/updater-e2e.sh (opt-in; see above)
#
# Opt in to a fast subset of this before each commit (fmt + clippy for rust
# changes, the full frontend job for frontend changes; see .githooks/pre-commit):
#
#   git config core.hooksPath .githooks
#
# cargo test and cargo deny are intentionally left to this script (or a
# pre-push hook), not the pre-commit hook.
#
# Requirements: the Rust toolchain from rust-toolchain.toml (rustfmt + clippy),
# cargo-deny (`cargo install cargo-deny --locked`), Node >= 20 (.node-version)
# and pnpm 10 (package.json "packageManager"). If the pnpm on PATH is older
# than 10 the script falls back to `corepack pnpm`, then to `npx -y pnpm@10`.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

RUN_RUST=1
RUN_FRONTEND=1
RUN_HUB_E2E=0
RUN_UPDATER_E2E=0
for arg in "$@"; do
  case "$arg" in
    --rust-only) RUN_FRONTEND=0 ;;
    --frontend-only) RUN_RUST=0 ;;
    --hub-e2e) RUN_HUB_E2E=1 ;;
    --updater-e2e) RUN_UPDATER_E2E=1 ;;
    -h|--help)
      sed -n '2,/^set -euo/p' "${BASH_SOURCE[0]}" | sed '$d' | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *)
      echo "ci-local: unknown argument '$arg' (expected --rust-only, --frontend-only, --hub-e2e or --updater-e2e)" >&2
      exit 2
      ;;
  esac
done

step() {
  printf '\n\033[1;34m==> %s\033[0m\n' "$*"
  "$@"
}

need() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "ci-local: '$1' not found. $2" >&2
    exit 1
  fi
}

# --- pnpm resolution -------------------------------------------------------
# CI (pnpm/action-setup) reads the pinned version from "packageManager" in
# package.json. Locally, prefer a pnpm >= 10 on PATH; otherwise let corepack
# resolve the same pin. Note that on pnpm 9 even `pnpm --version` errors in
# this repo ("packages field missing or empty", pnpm-workspace.yaml uses the
# pnpm 10 `allowBuilds` key), so `major` stays empty and we fall through to
# corepack.
resolve_pnpm() {
  local major=""
  if command -v pnpm >/dev/null 2>&1; then
    major="$(pnpm --version 2>/dev/null | cut -d. -f1 || true)"
  fi
  if [[ -n "$major" && "$major" -ge 10 ]]; then
    PNPM=(pnpm)
  elif command -v corepack >/dev/null 2>&1; then
    PNPM=(corepack pnpm)
  elif command -v npx >/dev/null 2>&1; then
    PNPM=(npx -y pnpm@10)
  else
    echo "ci-local: need pnpm >= 10 (or corepack / npx to bootstrap it)" >&2
    exit 1
  fi
}

# --- rust job --------------------------------------------------------------
run_rust() {
  need cargo "Install Rust via rustup: https://rustup.rs"
  if ! cargo deny --version >/dev/null 2>&1; then
    echo "ci-local: cargo-deny is not installed; run: cargo install cargo-deny --locked" >&2
    exit 1
  fi
  if [[ "$(uname -s)" == Linux ]] && ! pkg-config --exists gtk+-3.0 2>/dev/null; then
    echo "ci-local: no Tauri system libs (gtk+-3.0); running the headless subset only" >&2
    step cargo fmt --all --check
    step cargo clippy -p fleet-core -p fleet-hub -p fleet-proto -p fleet-agent -p fleet-update --all-targets -- -D warnings
    step cargo test -p fleet-core -p fleet-hub -p fleet-proto -p fleet-agent -p fleet-update
    step cargo deny check
    step cargo build -p fleet-hub --locked
    step cargo build -p fleet-agent --locked
    run_update_scripts
    return
  fi
  step cargo fmt --all --check
  step cargo clippy --workspace --all-targets -- -D warnings
  step cargo test --workspace
  # No --config: cargo-deny finds ./deny.toml from the repo root on its own.
  step cargo deny check
  # Mirrors the hub-headless CI job (no Tauri libs needed).
  step cargo build -p fleet-hub --locked
  step cargo build -p fleet-agent --locked
  run_update_scripts
}

# The update-publishing scripts (hub-headless's step of the same name).
run_update_scripts() {
  if ! command -v minisign >/dev/null 2>&1; then
    echo "ci-local: minisign not found; skipping scripts/release-update-scripts-test.sh" >&2
    return
  fi
  step cargo build -p fleet-update --bin fleet-release --locked
  step env FLEET_RELEASE=target/debug/fleet-release FLEET_HUB=target/debug/fleet-hub \
    bash scripts/release-update-scripts-test.sh
}

# --- hub end-to-end script (opt-in: --hub-e2e) ------------------------------
run_hub_e2e() {
  if ! command -v tmux >/dev/null 2>&1; then
    echo "ci-local: tmux not found; skipping --hub-e2e (the macOS runners/dev machines don't have it)" >&2
    return
  fi
  local target_dir="${CARGO_TARGET_DIR:-$ROOT/target}"
  local bin="$target_dir/debug/fleet-hub" abin="$target_dir/debug/fleet-agent"
  if [[ ! -x "$bin" || ! -x "$abin" ]]; then
    echo "ci-local: fleet-hub/fleet-agent not built at $target_dir/debug; run without --frontend-only, or build them first: cargo build -p fleet-hub -p fleet-agent --locked" >&2
    exit 1
  fi
  # The work-graph leg needs a hub built with the test-only `e2e` feature
  # (the fake-tracker override). Its own target dir, so the plain
  # target/debug/fleet-hub that CI mirrors is never replaced by it.
  step cargo build -p fleet-hub --features e2e --locked --target-dir "$target_dir/e2e"
  BIN="$bin" ABIN="$abin" WBIN="$target_dir/e2e/debug/fleet-hub" step bash scripts/hub-e2e.sh
}

# --- updater end-to-end script (opt-in: --updater-e2e) ----------------------
run_updater_e2e() {
  if ! docker info >/dev/null 2>&1 || ! command -v minisign >/dev/null 2>&1; then
    echo "ci-local: no reachable Docker daemon or no minisign; skipping --updater-e2e" >&2
    return
  fi
  step bash scripts/updater-e2e.sh
}

# --- frontend job ----------------------------------------------------------
run_frontend() {
  need node "Install Node >= 20 (see .node-version)"
  resolve_pnpm
  echo "ci-local: using pnpm via '${PNPM[*]}' ($("${PNPM[@]}" --version))"
  step "${PNPM[@]}" install --frozen-lockfile
  step "${PNPM[@]}" run check
  step "${PNPM[@]}" run test
  step "${PNPM[@]}" run build
  if [[ "${CI_LOCAL_AUDIT:-}" == warn ]]; then
    if ! step "${PNPM[@]}" audit --audit-level=high; then
      printf '\n\033[1;33mci-local: pnpm audit found advisories (not blocking: CI_LOCAL_AUDIT=warn; CI fails on them)\033[0m\n' >&2
    fi
  else
    step "${PNPM[@]}" audit --audit-level=high
  fi
}

# Always, and first: no migration this tree adds collides with main's numbers.
# Fetches origin's main so the comparison is against today's main, not the
# clone's last fetch (best-effort: offline it checks the local ref).
step scripts/check-migration-numbers.sh --fetch
step bash scripts/check-migration-numbers-test.sh
step bash scripts/renumber-migrations-test.sh

# ci.yml's version-consistency job. Cheap, and a mismatch
# here is what turns a release tag into three different advertised versions.
step scripts/check-version-consistency.sh

# The second step of that same CI job: the release asset manifest still
# parses and still declares a non-empty matrix for both kinds. Nothing else
# runs scripts/release-assets.sh until a tag is pushed, by which point a typo
# in it is expensive.
release_assets_smoke() {
  local v leg
  v="$(node -p 'require("./package.json").version')"
  scripts/release-assets.sh assets "$v" >/dev/null
  for leg in $(scripts/release-assets.sh legs); do
    scripts/release-assets.sh assets "$v" "$leg" >/dev/null
  done
  scripts/release-assets.sh matrix desktop >/dev/null
  scripts/release-assets.sh matrix bins >/dev/null
}
step release_assets_smoke

# The hub deploy scripts (backup.sh / upgrade.sh) against a fake docker:
# bash + sqlite3 only, a few seconds.
step bash scripts/hub-deploy-scripts-test.sh

# scripts/verify.sh's plan (which checks a set of changed files selects),
# dry runs only: about a second.
step bash scripts/verify-test.sh

# scripts/buildkite-verify.sh against a fake Buildkite API: a few seconds.
step bash scripts/buildkite-verify-test.sh

# scripts/release-mobile.sh's contract guard against a fake gh: a second.
step bash scripts/release-mobile-test.sh

# tools/ag (the agent launcher) against fake claude/codex binaries. bash +
# a few seconds; see .github/workflows/ci.yml's ag job for the matching
# macOS-under-bash-3.2 leg CI runs that this local step doesn't.
step bash scripts/ag-test.sh

# tools/voice/arecord (the /voice recorder stand-in) against fake servers,
# and its shellcheck when shellcheck is installed (CI's ag job runs both on
# Linux). bash + python3, a few seconds.
step bash scripts/voice-arecord-test.sh
if command -v shellcheck >/dev/null; then
  step shellcheck -s bash tools/voice/arecord scripts/voice-arecord-test.sh
else
  echo "ci-local: shellcheck not installed; skipping the voice arecord shellcheck"
fi

[[ "$RUN_RUST" == 1 ]] && run_rust
[[ "$RUN_FRONTEND" == 1 ]] && run_frontend
[[ "$RUN_HUB_E2E" == 1 ]] && run_hub_e2e
[[ "$RUN_UPDATER_E2E" == 1 ]] && run_updater_e2e

printf '\n\033[1;32mci-local: all selected checks passed\033[0m\n'
