#!/usr/bin/env bash
# Local mirror of .github/workflows/ci.yml. Runs the same steps, in the same
# order, so a green run here should mean a green run in CI:
#
#   version job:   scripts/check-version-consistency.sh — the six version
#                  carriers, their Cargo.lock entries and fleet-core's
#                  allowlisted 0.1.0; then a smoke test of
#                  scripts/release-assets.sh, the manifest release.yml builds
#                  its matrices from. Both run in every mode: they take
#                  seconds and cost nothing, and between them they are what
#                  keeps a hand-edited carrier or a broken asset manifest
#                  from reaching a tag.
#   rust job:      cargo fmt --all --check
#                  cargo clippy --workspace --all-targets -- -D warnings
#                  cargo test --workspace
#                  cargo deny check
#                  cargo build -p fleet-hub --locked   (mirrors the hub-headless CI job)
#                  cargo build -p fleet-agent --locked (same job; the agent must
#                  build with neither the Tauri libs nor fleet-core)
#                  On a box without the Tauri system libs (no gtk+-3.0 via
#                  pkg-config), --rust-only instead runs a headless subset:
#                  fmt, clippy/test/build scoped to fleet-core, fleet-hub,
#                  fleet-proto and fleet-agent, and cargo deny check.
#   frontend job:  pnpm install --frozen-lockfile
#                  pnpm run check
#                  pnpm run test
#                  pnpm run build
#                  pnpm audit --audit-level=high
#   hub-e2e:       scripts/hub-e2e.sh, opt-in via --hub-e2e (mirrors the
#                  hub-headless CI job's e2e step; see that script's own
#                  header). Skipped with a message if tmux is missing.
#                  NOT part of the default run: it drives real fleet-hub /
#                  fleet-agent processes and, for one of its three hubs,
#                  manages this machine's own tmux server directly, which a
#                  developer may already have a real hub or session on.
#
# Usage:
#   scripts/ci-local.sh                 # everything (rust first, then frontend)
#   scripts/ci-local.sh --rust-only
#   scripts/ci-local.sh --frontend-only
#   scripts/ci-local.sh --hub-e2e       # also run scripts/hub-e2e.sh (opt-in; see above)
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
for arg in "$@"; do
  case "$arg" in
    --rust-only) RUN_FRONTEND=0 ;;
    --frontend-only) RUN_RUST=0 ;;
    --hub-e2e) RUN_HUB_E2E=1 ;;
    -h|--help)
      sed -n '2,/^set -euo/p' "${BASH_SOURCE[0]}" | sed '$d' | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *)
      echo "ci-local: unknown argument '$arg' (expected --rust-only, --frontend-only or --hub-e2e)" >&2
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
  if ! pkg-config --exists gtk+-3.0 2>/dev/null; then
    echo "ci-local: no Tauri system libs (gtk+-3.0); running the headless subset only" >&2
    step cargo fmt --all --check
    step cargo clippy -p fleet-core -p fleet-hub -p fleet-proto -p fleet-agent --all-targets -- -D warnings
    step cargo test -p fleet-core -p fleet-hub -p fleet-proto -p fleet-agent
    step cargo deny check
    step cargo build -p fleet-hub --locked
    step cargo build -p fleet-agent --locked
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
  BIN="$bin" ABIN="$abin" step bash scripts/hub-e2e.sh
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
  step "${PNPM[@]}" audit --audit-level=high
}

# Always, and first: ci.yml's version-consistency job. Cheap, and a mismatch
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

[[ "$RUN_RUST" == 1 ]] && run_rust
[[ "$RUN_FRONTEND" == 1 ]] && run_frontend
[[ "$RUN_HUB_E2E" == 1 ]] && run_hub_e2e

printf '\n\033[1;32mci-local: all selected checks passed\033[0m\n'
