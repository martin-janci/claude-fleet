#!/usr/bin/env bash
# The one command step of .buildkite/pipeline.yml, on the persistent builder
# (docs/buildkite.md): `scripts/verify.sh full` for the checked-out commit,
# against a target/ that outlives the checkout.
#
# Before building it checks what makes a persistent builder fast and safe:
#   - CARGO_TARGET_DIR and CARGO_HOME are set and live outside the checkout,
#     so Buildkite's `git clean -ffxdq` never deletes them (Appendix I.5);
#   - CARGO_INCREMENTAL is not 0: workspace crates rebuild incrementally;
#   - the disk has BUILDER_MIN_FREE_GB free (default 30). Below it, the
#     incremental caches go first, then the whole target dir, because a full
#     disk fails the build halfway (Appendix I.4) and costs more than a cold
#     rebuild.
#
# Tests' temp files go to tmpfs (BUILDER_TMPDIR, default /dev/shm/claude-fleet-ci)
# when it has BUILDER_MIN_TMPFS_GB free (default 4): thousands of tempdir SQLite
# stores on ext4 serialise behind its journal and the fleet-hub / src-tauri
# targets crawl (CLAUDE.md, "TMPDIR fixes it"). The directory is emptied first;
# concurrency 1 means no other build is using it.
set -euo pipefail

ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"

fail() { echo "buildkite-step: $*" >&2; exit 2; }
inside() { case "$1/" in "$ROOT"/*) return 0 ;; esac; return 1; }

[[ -n "${CARGO_TARGET_DIR:-}" ]] || fail "CARGO_TARGET_DIR is not set; see deploy/buildkite/hooks/environment"
[[ -n "${CARGO_HOME:-}" ]] || fail "CARGO_HOME is not set; see deploy/buildkite/hooks/environment"
inside "$CARGO_TARGET_DIR" && fail "CARGO_TARGET_DIR ($CARGO_TARGET_DIR) is inside the checkout; git clean would delete it"
inside "$CARGO_HOME" && fail "CARGO_HOME ($CARGO_HOME) is inside the checkout; git clean would delete it"
[[ "${CARGO_INCREMENTAL:-}" == 0 ]] && fail "CARGO_INCREMENTAL=0 is set; a persistent builder wants incremental builds"

min_gb=${BUILDER_MIN_FREE_GB:-30}
free_gb() { mkdir -p "$CARGO_TARGET_DIR"; df -Pk "$CARGO_TARGET_DIR" | awk 'NR==2 { print int($4 / 1048576) }'; }
if (( $(free_gb) < min_gb )); then
  echo "buildkite-step: $(free_gb) GB free < $min_gb GB; removing incremental caches"
  find "$CARGO_TARGET_DIR" -maxdepth 3 -type d -name incremental -prune -exec rm -rf {} +
  if (( $(free_gb) < min_gb )); then
    echo "buildkite-step: still $(free_gb) GB free; removing $CARGO_TARGET_DIR (the next build is cold)"
    rm -rf "$CARGO_TARGET_DIR"
  fi
fi
echo "buildkite-step: $(free_gb) GB free; target $CARGO_TARGET_DIR"

tmp=${BUILDER_TMPDIR:-/dev/shm/claude-fleet-ci}
tmp_min_gb=${BUILDER_MIN_TMPFS_GB:-4}
tmp_parent=$(dirname "$tmp")
if [[ -d "$tmp_parent" && -w "$tmp_parent" ]] \
  && (( $(df -Pk "$tmp_parent" | awk 'NR==2 { print int($4 / 1048576) }') >= tmp_min_gb )); then
  rm -rf "$tmp" && mkdir -p "$tmp"
  export TMPDIR="$tmp"
  echo "buildkite-step: TMPDIR $TMPDIR"
else
  echo "buildkite-step: $tmp_parent missing or under $tmp_min_gb GB free; TMPDIR stays ${TMPDIR:-/tmp}"
fi

# verify.sh narrows ci-local.sh by what changed since the merge base with
# origin/main, so that ref has to be current.
git fetch -q origin main

exec scripts/verify.sh full
