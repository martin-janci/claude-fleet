#!/usr/bin/env bash
# Tests for `scripts/release.sh --tag-only <v>` in a throwaway git repo: it
# tags HEAD only when the release commit is already merged (package.json and
# the CHANGELOG section at <v>, the carriers consistent), creates no commit,
# and refuses everything else without a tag. The version-consistency check is
# a stub (its own rules are not under test here); the remote preflight is
# skipped with the two documented switches. bash + git + node, about a second.
# PASS/FAIL lines, exit 1 on any failure.
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PASS=0; FAIL=0

# Run from a git hook (the pre-commit hook runs ci-local.sh), git exports
# GIT_DIR and GIT_INDEX_FILE: with them, every `git -C <throwaway>` below
# would commit into the repository being committed to. Drop them all.
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_COMMON_DIR GIT_PREFIX \
  GIT_OBJECT_DIRECTORY GIT_ALTERNATE_OBJECT_DIRECTORIES GIT_NAMESPACE

t="$(mktemp -d)"
trap 'rm -rf "$t"' EXIT

setup() { # setup <package.json version> <CHANGELOG section version>
  rm -rf "$t/r"; mkdir -p "$t/r/scripts"
  cp "$REPO/scripts/release.sh" "$t/r/scripts/release.sh"
  cat >"$t/r/scripts/check-version-consistency.sh" <<'EOF'
#!/usr/bin/env bash
echo "$*" >>"$(dirname "$0")/../.consistency-args"
[[ -z "${FAKE_INCONSISTENT:-}" ]]
EOF
  chmod +x "$t/r/scripts/"*.sh
  printf '{\n  "name": "x",\n  "version": "%s"\n}\n' "$1" >"$t/r/package.json"
  printf '# Changelog\n\n## [%s] - 2026-11-21\n\n### Fixed\n- a thing\n' "$2" >"$t/r/CHANGELOG.md"
  printf '.consistency-args\n' >"$t/r/.gitignore"
  git -C "$t/r" init -q -b main
  # Never anything but the throwaway repository.
  [[ "$(git -C "$t/r" rev-parse --absolute-git-dir)" == "$t/r/.git" ]] \
    || { echo "FAIL: the test repository is not $t/r/.git; refusing to go on" >&2; exit 1; }
  git -C "$t/r" -c user.name=t -c user.email=t@t add -A
  git -C "$t/r" -c user.name=t -c user.email=t@t commit -q -m "chore(release): v$1"
}

# run <args...>: release.sh in the repo, remote preflight skipped
run() {
  (cd "$t/r" && GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@t \
    RELEASE_SKIP_CI_CHECK=1 RELEASE_ALLOW_BEHIND_ORIGIN=1 \
    GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=tag.gpgsign GIT_CONFIG_VALUE_0=false \
    bash scripts/release.sh "$@" 2>&1)
}

check() { # check <name> <ok 0|1> [detail]
  if [[ $2 == 1 ]]; then PASS=$((PASS + 1)); echo "PASS: $1"
  else FAIL=$((FAIL + 1)); echo "FAIL: $1${3:+ — $3}"; fi
}

tag_sha() { git -C "$t/r" rev-parse -q --verify "refs/tags/v$1^{commit}" 2>/dev/null; }
commits() { git -C "$t/r" rev-list --count HEAD; }

# 1. The release commit is merged: tag HEAD, commit nothing, print the push.
setup 1.2.3 1.2.3
before="$(commits)"; head="$(git -C "$t/r" rev-parse HEAD)"
out="$(run --tag-only 1.2.3)"; rc=$?
ok=1
[[ $rc == 0 ]] || ok=0
[[ "$(tag_sha 1.2.3)" == "$head" ]] || ok=0
[[ "$(commits)" == "$before" ]] || ok=0
[[ "$out" == *"git push origin v1.2.3"* ]] || ok=0
grep -qx -- "--expect-tag v1.2.3" "$t/r/.consistency-args" 2>/dev/null || ok=0
check "tags HEAD and nothing else" $ok "rc=$rc out=$out"

# 2. A tag that already exists is refused.
out="$(run --tag-only 1.2.3)"; rc=$?
check "refuses an existing tag" "$([[ $rc != 0 && "$out" == *"already exists"* ]] && echo 1 || echo 0)" "$out"

# 3. package.json at another version: the release commit is not merged.
setup 1.2.2 1.2.3
out="$(run --tag-only 1.2.3)"; rc=$?
check "refuses when package.json is not at <v>" \
  "$([[ $rc != 0 && -z "$(tag_sha 1.2.3)" && "$out" == *"package.json is at 1.2.2"* ]] && echo 1 || echo 0)" "$out"

# 4. No CHANGELOG section for <v>: release.yml would publish with no notes.
setup 1.2.3 1.2.2
out="$(run --tag-only 1.2.3)"; rc=$?
check "refuses without a CHANGELOG section" \
  "$([[ $rc != 0 && -z "$(tag_sha 1.2.3)" && "$out" == *"no '## [1.2.3]' section"* ]] && echo 1 || echo 0)" "$out"

# 5. A carrier disagrees.
setup 1.2.3 1.2.3
out="$(FAKE_INCONSISTENT=1 run --tag-only 1.2.3)"; rc=$?
check "refuses inconsistent carriers" \
  "$([[ $rc != 0 && -z "$(tag_sha 1.2.3)" && "$out" == *"do not all say 1.2.3"* ]] && echo 1 || echo 0)" "$out"

# 6. A dry run creates nothing.
setup 1.2.3 1.2.3
out="$(RELEASE_DRY_RUN=1 run --tag-only 1.2.3)"; rc=$?
check "dry run creates no tag" \
  "$([[ $rc == 0 && -z "$(tag_sha 1.2.3)" && "$out" == *"would tag v1.2.3"* ]] && echo 1 || echo 0)" "$out"

# 7. Off main is refused.
setup 1.2.3 1.2.3
git -C "$t/r" checkout -q -b other
out="$(run --tag-only 1.2.3)"; rc=$?
check "refuses off main" "$([[ $rc != 0 && -z "$(tag_sha 1.2.3)" ]] && echo 1 || echo 0)" "$out"

# 8. `auto` makes no sense without a bump.
setup 1.2.3 1.2.3
out="$(run --tag-only auto)"; rc=$?
check "refuses auto" "$([[ $rc != 0 && "$out" == *"explicit version"* ]] && echo 1 || echo 0)" "$out"

# 9. A plain release of the version package.json already holds points here.
setup 1.2.3 1.2.3
out="$(run 1.2.3)"; rc=$?
check "plain release names --tag-only" \
  "$([[ $rc != 0 && -z "$(tag_sha 1.2.3)" && "$out" == *"--tag-only 1.2.3"* ]] && echo 1 || echo 0)" "$out"

echo "release-tag-only: $PASS passed, $FAIL failed"
[[ $FAIL == 0 ]]
