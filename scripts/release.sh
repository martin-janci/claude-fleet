#!/usr/bin/env bash
# Cut a claude-fleet release: bump versions, prefill CHANGELOG, commit, tag.
# Usage: scripts/release.sh <X.Y.Z>        (run from a clean checkout of main)
# Env:   RELEASE_DRY_RUN=1  edit files only — no cargo/commit/tag (for testing)
# Needs: bash, git, cargo, node, awk, date. See docs/RELEASING.md.
set -euo pipefail

REPO_URL="https://github.com/martin-janci/claude-fleet"
CRATE="claude-fleet"
VERSION_FILES=(package.json src-tauri/tauri.conf.json src-tauri/Cargo.toml)

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
  cargo update -p "$CRATE" --manifest-path src-tauri/Cargo.toml --offline >/dev/null 2>&1 \
    || cargo update -p "$CRATE" --manifest-path src-tauri/Cargo.toml
  echo "  src-tauri/Cargo.lock"
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
awk -v header="## [$NEW] - $TODAY" -v section="$SECTION" -v link="$LINK" '
  !ins && /^## \[/ { print header; print ""; print section; ins = 1 }
  !lnk && /^\[[^]]+\]: /  { print link; lnk = 1 }
  { print }
  END {
    if (!ins) { print ""; print header; print ""; print section }
    if (!lnk) { print link }
  }' CHANGELOG.md > CHANGELOG.md.tmp && mv CHANGELOG.md.tmp CHANGELOG.md
echo "  CHANGELOG.md (${LAST_TAG:-<no previous tag>}..HEAD)"

if [[ -n "${RELEASE_DRY_RUN:-}" ]]; then
  echo "Dry run: files edited, nothing committed. Review with: git diff"; exit 0
fi

# --- 4. Commit + annotated tag -----------------------------------------------
if [[ -t 0 ]]; then
  echo; echo "Edit CHANGELOG.md now if the generated section needs polishing, then press Enter."
  read -r _
fi
git add "${VERSION_FILES[@]}" src-tauri/Cargo.lock CHANGELOG.md
git commit -q -m "chore(release): v$NEW"
git tag -a "v$NEW" -m "claude-fleet v$NEW"
echo
echo "Committed chore(release): v$NEW and created tag v$NEW. To publish:"
echo "  git push origin main --follow-tags"
