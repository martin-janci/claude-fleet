#!/usr/bin/env bash
# Cut a claude-fleet release: bump versions, prefill CHANGELOG, commit, tag.
# Usage: scripts/release.sh <X.Y.Z>        (run from a clean checkout of main)
# Env:   RELEASE_DRY_RUN=1  edit files only — no cargo/commit/tag (for testing)
# Needs: bash, git, cargo, node, awk, date. See docs/RELEASING.md.
set -euo pipefail

REPO_URL="https://github.com/martin-janci/claude-fleet"
# crates/fleet-core stays at 0.1.0 and is deliberately NOT in this list: it is
# an internal library crate consumed only by path within this workspace, not
# versioned in step with the app. (Its Cargo.toml has no `publish = false`
# marker, so this is a convention, not an enforced one — see issue #152.)
VERSION_FILES=(package.json src-tauri/tauri.conf.json src-tauri/Cargo.toml crates/fleet-hub/Cargo.toml crates/fleet-proto/Cargo.toml crates/fleet-agent/Cargo.toml)

die() { echo "release.sh: $*" >&2; exit 1; }

[[ $# -eq 1 ]] || die "usage: scripts/release.sh <X.Y.Z>"
NEW="$1"
[[ "$NEW" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "'$NEW' is not a plain semver X.Y.Z"

cd "$(git rev-parse --show-toplevel)"
BRANCH="$(git branch --show-current)"
[[ "$BRANCH" == "main" ]] || die "must run on main (currently on '$BRANCH')"
[[ -z "$(git status --porcelain)" ]] || die "working tree is dirty — commit or discard first"
git rev-parse -q --verify "refs/tags/v$NEW" >/dev/null && die "tag v$NEW already exists"

CUR="$(node -e 'process.stdout.write(require("./package.json").version)')"
[[ "$CUR" != "$NEW" ]] || die "package.json is already at $NEW"
echo "Bumping $CUR -> $NEW"

# --- 1. Version fields (first occurrence only; formatting preserved) ---------
node - "$NEW" "${VERSION_FILES[@]}" <<'JS'
const fs = require("fs");
const [next, ...files] = process.argv.slice(2);
for (const f of files) {
  const src = fs.readFileSync(f, "utf8");
  const re = f.endsWith(".toml") ? /^version = "[^"]+"/m : /"version": "[^"]+"/;
  if (!re.test(src)) { console.error(`release.sh: no version field in ${f}`); process.exit(1); }
  const rep = f.endsWith(".toml") ? `version = "${next}"` : `"version": "${next}"`;
  fs.writeFileSync(f, src.replace(re, rep));
  console.log(`  ${f}`);
}
JS

# --- 2. Cargo.lock follows Cargo.toml ----------------------------------------
if [[ -z "${RELEASE_DRY_RUN:-}" ]]; then
  # Every crate this script just bumped needs Cargo.lock synced, or a
  # `--locked` build of it fails after the release (#152). Derive the package
  # list from VERSION_FILES' .toml entries instead of hard-coding a second
  # list that can drift from the first.
  CARGO_PKGS=()
  for f in "${VERSION_FILES[@]}"; do
    [[ "$f" == *.toml ]] || continue
    name="$(node -e '
      const fs = require("fs");
      const src = fs.readFileSync(process.argv[1], "utf8");
      const m = src.match(/^name = "([^"]+)"/m);
      if (!m) { console.error("no [package] name in " + process.argv[1]); process.exit(1); }
      process.stdout.write(m[1]);
    ' "$f")" || die "could not read [package] name from $f"
    CARGO_PKGS+=("$name")
  done
  UPDATE_ARGS=()
  for p in "${CARGO_PKGS[@]}"; do UPDATE_ARGS+=(-p "$p"); done
  if ! cargo update "${UPDATE_ARGS[@]}" --offline >/dev/null 2>&1; then
    cargo update "${UPDATE_ARGS[@]}" \
      || die "cargo update failed for: ${CARGO_PKGS[*]} (both --offline and online) — Cargo.lock was NOT updated; fix and re-run"
  fi
  echo "  Cargo.lock (${CARGO_PKGS[*]})"
fi

# --- 3. CHANGELOG.md section, grouped by Conventional Commit type -------------
LAST_TAG="$(git describe --tags --abbrev=0 --match 'v[0-9]*' 2>/dev/null || true)"
RANGE="${LAST_TAG:+$LAST_TAG..}HEAD"
TODAY="$(date +%Y-%m-%d)"
SECTION="$(git log --no-merges --format='%s' "$RANGE" | awk '
  {
    s = $0; type = "other"; scope = ""
    if (match(s, /^[a-z]+(\([^)]*\))?!?: /)) {
      head = substr(s, 1, RLENGTH); s = substr(s, RLENGTH + 1)
      type = head; sub(/[(!:].*/, "", type)
      if (match(head, /\(.*\)/)) scope = substr(head, RSTART + 1, RLENGTH - 2)
    }
    line = "- " (scope != "" ? "**" scope ":** " : "") s
    g = (type == "feat") ? "Added" : (type == "fix") ? "Fixed" : (type == "docs") ? "Documentation" : "Changed"
    n[g]++; out[g, n[g]] = line
  }
  END {
    split("Added Changed Fixed Documentation", order, " ")
    for (k = 1; k <= 4; k++) {
      g = order[k]; if (!n[g]) continue
      print "### " g; for (i = 1; i <= n[g]; i++) print out[g, i]; print ""
    }
  }')"
[[ -n "$SECTION" ]] || SECTION="### Changed
- (no commits since ${LAST_TAG:-the beginning of history})
"
LINK="[$NEW]: $REPO_URL/releases/tag/v$NEW"
# The section is multi-line, and the BSD awk that ships with macOS rejects
# `-v` values containing a newline ("newline in string"), so hand it over
# through the environment. Two statements, not `awk ... && mv`: under
# `set -e` a failure before the last `&&` is ignored, which would commit and
# tag a release with no CHANGELOG entry.
SECTION="$SECTION" awk -v header="## [$NEW] - $TODAY" -v link="$LINK" '
  !ins && /^## \[/ { print header; print ""; print ENVIRON["SECTION"]; ins = 1 }
  !lnk && /^\[[^]]+\]: /  { print link; lnk = 1 }
  { print }
  END {
    if (!ins) { print ""; print header; print ""; print ENVIRON["SECTION"] }
    if (!lnk) { print link }
  }' CHANGELOG.md > CHANGELOG.md.tmp
mv CHANGELOG.md.tmp CHANGELOG.md
echo "  CHANGELOG.md (${LAST_TAG:-<no previous tag>}..HEAD)"

if [[ -n "${RELEASE_DRY_RUN:-}" ]]; then
  echo "Dry run: files edited, nothing committed. Review with: git diff"; exit 0
fi

# --- 4. Commit + annotated tag -----------------------------------------------
if [[ -t 0 ]]; then
  echo; echo "Edit CHANGELOG.md now if the generated section needs polishing, then press Enter."
  read -r _
fi
git add "${VERSION_FILES[@]}" Cargo.lock CHANGELOG.md
git commit -q -m "chore(release): v$NEW"
git tag -a "v$NEW" -m "claude-fleet v$NEW"
echo
echo "Committed chore(release): v$NEW and created tag v$NEW. To publish:"
echo "  git push origin main --follow-tags"
