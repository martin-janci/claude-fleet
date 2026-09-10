#!/usr/bin/env bash
# Local mirror of .github/workflows/ci.yml. Runs the same steps, in the same
# order, so a green run here should mean a green run in CI:
#
#   rust job:      cargo fmt --check
#                  cargo clippy --all-targets -- -D warnings
#                  cargo test
#                  cargo deny check
#   frontend job:  pnpm install --frozen-lockfile
#                  pnpm run check
#                  pnpm run test
#                  pnpm run build
#                  pnpm audit --audit-level=high
#
# Usage:
#   scripts/ci-local.sh                 # everything (rust first, then frontend)
#   scripts/ci-local.sh --rust-only
#   scripts/ci-local.sh --frontend-only
#
# Opt in to running this automatically before each commit (see
# .githooks/pre-commit for what runs when):
#
#   git config core.hooksPath .githooks
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
for arg in "$@"; do
  case "$arg" in
    --rust-only) RUN_FRONTEND=0 ;;
    --frontend-only) RUN_RUST=0 ;;
    -h|--help)
      sed -n '2,/^set -euo/p' "${BASH_SOURCE[0]}" | sed '$d' | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *)
      echo "ci-local: unknown argument '$arg' (expected --rust-only or --frontend-only)" >&2
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
# CI uses pnpm/action-setup with version 10. Locally, prefer a pnpm >= 10 on
# PATH; otherwise let corepack resolve the pinned version from package.json.
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
  local manifest="src-tauri/Cargo.toml"
  step cargo fmt --manifest-path "$manifest" --check
  step cargo clippy --manifest-path "$manifest" --all-targets -- -D warnings
  step cargo test --manifest-path "$manifest"
  step cargo deny --manifest-path "$manifest" check --config deny.toml
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

[[ "$RUN_RUST" == 1 ]] && run_rust
[[ "$RUN_FRONTEND" == 1 ]] && run_frontend

printf '\n\033[1;32mci-local: all selected checks passed\033[0m\n'
