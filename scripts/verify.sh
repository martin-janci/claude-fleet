#!/usr/bin/env bash
# The one verification command for a change in progress (CLAUDE.md →
# "Validation ladder"). It looks at what changed and runs each needed check
# once, in the validation ladder's single package selection, so nothing it
# runs is repeated by the pre-commit hook or rebuilt in another feature world.
#
#   scripts/verify.sh            # = quick
#   scripts/verify.sh quick      # before a commit
#   scripts/verify.sh full       # before a push: scripts/ci-local.sh, narrowed
#                                #   to the jobs the change touches; every mode
#                                #   first checks migration numbers against
#                                #   origin/main (scripts/check-migration-numbers.sh)
#   scripts/verify.sh remote     # the same `full`, on the persistent Buildkite
#                                #   builder, for the pushed HEAD
#                                #   (scripts/buildkite-verify.sh, docs/buildkite.md)
#
# quick, for a Rust change:
#   cargo fmt --all --check
#   cargo fleet-lint             clippy ⊇ check, test code included; the
#                                pre-commit hook's clippy is then a no-op
#   cargo fleet-test -- <filters>
#                                the tests of the modules the change touches:
#                                crates/fleet-core/src/service/work/view.rs
#                                → `service::work::view`. A crate root, a
#                                Cargo/toolchain file or a file outside a
#                                crate's src/ runs every unit test instead.
#   cargo test --workspace --test <name>
#                                a changed integration test file
# A file Rust tests read by its repo path (a src/lib/*.ts mirror, a docs/*.md
# guide; `repo_files::read`) adds the modules whose source names that path.
#
# quick, for a frontend change:
#   pnpm run check
#   pnpm exec vitest related --run <changed files>
#                                (all of `pnpm run test` when a package or
#                                config file changed)
#
# quick is a fast, change-shaped subset, not the gate: run `full` before a
# push. What changed is everything since the merge base with origin/main
# (else main), plus staged, unstaged and untracked files; --base REF picks
# another base.
#
# Options:
#   --base REF     diff against REF instead of the merge base
#   --dry-run      print the plan, run nothing
#   --files F...   treat F... as the changed files (the rest of the line;
#                  scripts/verify-test.sh uses it)
set -euo pipefail

usage() { sed -n '2,/^set -euo/p' "$0" | sed -e '$d' -e 's/^# \{0,1\}//'; }

level=quick
base=""
dry=0
files_given=0
files=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    quick|full) level=$1; shift ;;
    remote)
      shift
      [[ "${1:-}" == --dry-run ]] && { echo "verify remote: scripts/buildkite-verify.sh"; exit 0; }
      exec "$(git rev-parse --show-toplevel)/scripts/buildkite-verify.sh" "$@" ;;
    --base) base=${2:?--base needs a ref}; shift 2 ;;
    --dry-run) dry=1; shift ;;
    --files) files_given=1; shift; files=("$@"); break ;;
    -h|--help) usage; exit 0 ;;
    *) echo "verify: unknown argument: $1 (see --help)" >&2; exit 2 ;;
  esac
done

ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"

# --- what changed ------------------------------------------------------------
if [[ $files_given == 0 ]]; then
  if [[ -z "$base" ]]; then
    base="$(git merge-base HEAD origin/main 2>/dev/null || git merge-base HEAD main 2>/dev/null || true)"
  fi
  changed="$(
    {
      [[ -n "$base" ]] && git diff --name-only "$base" HEAD
      git diff --name-only --cached
      git diff --name-only
      git ls-files --others --exclude-standard
    } | sort -u
  )"
else
  changed="$(printf '%s\n' "${files[@]+"${files[@]}"}" | sed '/^$/d' | sort -u)"
fi

is_frontend() {
  case "$1" in
    src/*|package.json|pnpm-lock.yaml|pnpm-workspace.yaml) return 0 ;;
    vite.config.ts|vitest.config.ts|vitest.setup.ts|svelte.config.js|tsconfig.json|index.html) return 0 ;;
  esac
  return 1
}
is_frontend_config() {
  case "$1" in
    package.json|pnpm-lock.yaml|pnpm-workspace.yaml) return 0 ;;
    vite.config.ts|vitest.config.ts|vitest.setup.ts|svelte.config.js|tsconfig.json|index.html) return 0 ;;
  esac
  return 1
}
is_rust() {
  case "$1" in
    crates/*|src-tauri/*|Cargo.toml|Cargo.lock|.cargo/*|rust-toolchain.toml|deny.toml|clippy.toml|rustfmt.toml|.rustfmt.toml) return 0 ;;
  esac
  return 1
}

# Echoes the unit-test filter for a path in a crate, `ALL` when only every
# unit test covers it, `TEST:<name>` for an integration test file, `TESTS`
# for a helper under a tests/ directory (every test target), nothing for a
# path in no crate.
rust_target_of() {
  local p=$1 crate rel
  case "$p" in
    Cargo.toml|Cargo.lock|.cargo/*|rust-toolchain.toml|clippy.toml|rustfmt.toml|.rustfmt.toml) echo ALL; return ;;
    src-tauri/src/*) crate=src-tauri; rel=${p#src-tauri/src/} ;;
    crates/*/src/*) crate=${p%%/src/*}; rel=${p#"$crate"/src/} ;;
    crates/*/tests/*/*) echo TESTS; return ;;
    crates/*/tests/*.rs)
      rel=${p#crates/*/tests/}
      echo "TEST:${rel%.rs}"
      return ;;
    crates/*|src-tauri/*) echo ALL; return ;;
    *) return ;;
  esac
  case "$rel" in
    lib.rs|main.rs|bin/*) echo ALL; return ;;
  esac
  if [[ "$rel" == *.rs ]]; then
    rel=${rel%.rs}
    rel=${rel%/mod}
  else
    # A data file (SQL, JSON, a fixture): the module of its directory, cut
    # at a testdata/fixtures directory.
    rel=$(dirname "$rel")
    rel=${rel%%/testdata*}
    rel=${rel%%/fixtures*}
    [[ "$rel" == "." || "$rel" == testdata* || "$rel" == fixtures* ]] && { echo ALL; return; }
  fi
  echo "${rel//\//::}"
}

# The modules whose Rust source names this repo path (last two components,
# e.g. `lib/events.ts`, `docs/updates.md`): tests that read it at runtime.
readers_of() {
  local p=$1 key hit t
  key=$(echo "$p" | awk -F/ '{ if (NF >= 2) print $(NF-1) "/" $NF; else print $NF }')
  [[ -z "$key" ]] && return
  while IFS= read -r hit; do
    [[ -z "$hit" ]] && continue
    t=$(rust_target_of "$hit")
    # A crate root naming it says nothing about which tests read it.
    [[ "$t" == ALL || "$t" == TEST* || -z "$t" ]] && continue
    echo "$t"
  done < <(grep -rlF --include='*.rs' -- "$key\"" crates/*/src src-tauri/src 2>/dev/null || true)
}

rust_changed=0
frontend_changed=0
frontend_config=0
all_tests=0
all_targets=0
filters=""
itests=""
frontend_files=()
uncovered=()

while IFS= read -r f; do
  [[ -z "$f" ]] && continue
  if is_rust "$f"; then
    rust_changed=1
    t=$(rust_target_of "$f")
    case "$t" in
      ALL) all_tests=1 ;;
      TESTS) all_targets=1 ;;
      TEST:*) itests="$itests ${t#TEST:}" ;;
      "") ;;
      *) filters="$filters $t" ;;
    esac
    continue
  fi
  if is_frontend "$f"; then
    frontend_changed=1
    if is_frontend_config "$f"; then
      frontend_config=1
    elif [[ -e "$f" ]]; then
      frontend_files+=("$f")
    fi
  fi
  r=$(readers_of "$f" | sort -u | tr '\n' ' ')
  if [[ -n "${r// /}" ]]; then
    filters="$filters $r"
  elif ! is_frontend "$f"; then
    uncovered+=("$f")
  fi
done <<< "$changed"

filters=$(echo "$filters" | tr ' ' '\n' | sed '/^$/d' | sort -u | tr '\n' ' ')
itests=$(echo "$itests" | tr ' ' '\n' | sed '/^$/d' | sort -u | tr '\n' ' ')
nfilters=$(echo "$filters" | wc -w | tr -d ' ')
# Past this many modules a filtered run saves little over all of them.
[[ $nfilters -gt 40 ]] && all_tests=1

# --- full: the gate ----------------------------------------------------------
if [[ $level == full ]]; then
  args=()
  if [[ -n "$changed" && $frontend_changed == 0 ]]; then
    args=(--rust-only)
  elif [[ $frontend_changed == 1 && $rust_changed == 0 && $nfilters == 0 && ${#uncovered[@]} == 0 ]]; then
    args=(--frontend-only)
  fi
  echo "verify full: scripts/ci-local.sh ${args[*]+"${args[*]}"}"
  [[ $dry == 1 ]] && exit 0
  exec scripts/ci-local.sh ${args[@]+"${args[@]}"}
fi

# --- quick: the plan ---------------------------------------------------------
plan=()
# Only on Linux does the desktop crate need the Tauri system libraries; there,
# without them, lint and test the crates that build headless (as ci-local.sh
# and the pre-commit hook do).
# VERIFY_HEADLESS=0|1 overrides the probe (scripts/verify-test.sh).
headless=${VERIFY_HEADLESS:-}
if [[ -z "$headless" ]]; then
  headless=0
  if [[ "$(uname -s)" == Linux ]] && ! pkg-config --exists gtk+-3.0 2>/dev/null; then
    headless=1
  fi
fi
headless_pkgs="-p fleet-core -p fleet-hub -p fleet-proto -p fleet-agent -p fleet-update"

if [[ $rust_changed == 1 ]]; then
  plan+=("cargo fmt --all --check")
  if [[ $headless == 1 ]]; then
    plan+=("cargo clippy $headless_pkgs --all-targets -- -D warnings")
  else
    plan+=("cargo fleet-lint")
  fi
fi
q_filters=""
for f in $filters; do q_filters="$q_filters $(printf '%q' "$f")"; done
if [[ $all_targets == 1 ]]; then
  if [[ $headless == 1 ]]; then
    plan+=("cargo test $headless_pkgs")
  else
    plan+=("cargo test --workspace")
  fi
elif [[ $all_tests == 1 ]]; then
  if [[ $headless == 1 ]]; then
    plan+=("cargo test $headless_pkgs --lib --bins")
  else
    plan+=("cargo fleet-test")
  fi
elif [[ $nfilters -gt 0 ]]; then
  if [[ $headless == 1 ]]; then
    plan+=("cargo test $headless_pkgs --lib --bins --$q_filters")
  else
    plan+=("cargo fleet-test --$q_filters")
  fi
fi
if [[ $all_targets == 0 && -n "${itests// /}" ]]; then
  t=""
  for n in $itests; do t="$t --test $(printf '%q' "$n")"; done
  plan+=("cargo test --workspace$t")
fi
if [[ $frontend_changed == 1 ]]; then
  # A missing node_modules is installed at run time, so the plan does not
  # depend on the checkout's state.
  if [[ $frontend_config == 1 ]]; then
    plan+=("pnpm install --frozen-lockfile")
  fi
  plan+=("pnpm run check")
  if [[ $frontend_config == 1 ]]; then
    plan+=("pnpm run test")
  elif [[ ${#frontend_files[@]} -gt 0 ]]; then
    plan+=("pnpm exec vitest related --run$(printf ' %q' "${frontend_files[@]}")")
  fi
fi

if [[ ${#plan[@]} == 0 ]]; then
  echo "verify quick: nothing to check for this change."
fi
for c in ${plan[@]+"${plan[@]}"}; do echo "verify quick: $c"; done
if [[ ${#uncovered[@]} -gt 0 ]]; then
  echo "verify quick: not covered here (run 'scripts/verify.sh full'): ${uncovered[*]}"
fi
[[ $dry == 1 ]] && exit 0

# --- quick: run --------------------------------------------------------------
if [[ $frontend_changed == 1 && $frontend_config == 0 && ! -d node_modules ]]; then
  plan=("pnpm install --frozen-lockfile" ${plan[@]+"${plan[@]}"})
fi
summary=()
total_start=$(date +%s)
for c in ${plan[@]+"${plan[@]}"}; do
  echo
  echo "==> $c"
  s=$(date +%s)
  if ! eval "$c"; then
    echo
    echo "verify quick: FAILED after $(( $(date +%s) - total_start ))s: $c" >&2
    exit 1
  fi
  summary+=("$(printf '%5ss  %s' "$(( $(date +%s) - s ))" "$c")")
done
echo
for l in ${summary[@]+"${summary[@]}"}; do echo "verify quick: $l"; done
echo "verify quick: ok in $(( $(date +%s) - total_start ))s. Before a push: scripts/verify.sh full"
