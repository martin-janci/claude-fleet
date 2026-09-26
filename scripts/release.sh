#!/usr/bin/env bash
# Cut a claude-fleet release: bump versions, prefill CHANGELOG, commit, tag.
# Usage: scripts/release.sh <X.Y.Z[-rc.N]>  (run from a clean checkout of main)
#        scripts/release.sh auto           (version derived from the commits)
#        scripts/release.sh --next         (print that derived version; changes nothing)
#        scripts/release.sh --list         (print the version carriers; changes nothing)
# Env:   RELEASE_DRY_RUN=1        edit files only — no cargo/commit/tag (for testing)
#        RELEASE_SKIP_CI_CHECK=1  tag without proving CI is green on HEAD
# Needs: bash, git, cargo, node, awk, date, gh. See docs/RELEASING.md.
#
# The tag this creates is the trigger for a PUBLISHED release: pushing it runs
# release.yml, which builds every asset, verifies the release is complete and
# then publishes it with no further human step. Hence the CI check below —
# there is no longer a draft standing between a bad tag and users.
set -euo pipefail

REPO_URL="https://github.com/martin-janci/claude-fleet"
# crates/fleet-core stays at 0.1.0 and is deliberately NOT in this list: it is
# an internal library crate consumed only by path within this workspace, not
# versioned in step with the app. It carries `publish = false` so that
# exception is enforced rather than conventional, and
# scripts/check-version-consistency.sh names it in the allowlist whose marker
# it verifies.
VERSION_FILES=(package.json src-tauri/tauri.conf.json src-tauri/Cargo.toml crates/fleet-hub/Cargo.toml crates/fleet-proto/Cargo.toml crates/fleet-agent/Cargo.toml)

die() { echo "release.sh: $*" >&2; exit 1; }

# `--list` prints exactly the paths this script rewrites, one per line, relative
# to the repo root — then exits 0 having touched nothing (no git, no cargo, no
# writes). scripts/check-version-consistency.sh consumes it, so CI reads the
# real propagation list instead of keeping a second copy that can drift from it
# (F-C5).
if [[ ${1:-} == "--list" ]]; then
  [[ $# -eq 1 ]] || die "usage: scripts/release.sh --list"
  printf '%s\n' "${VERSION_FILES[@]}"
  exit 0
fi

[[ $# -eq 1 ]] || die "usage: scripts/release.sh <X.Y.Z[-rc.N]> | auto | --next | --list"

# X.Y.Z, or X.Y.Z-<pre> for a release candidate. The pre-release suffix is the
# review channel that replaced the draft habit: `v0.3.0-rc.1` builds the same
# eleven assets, is published like any other release, and is marked
# `prerelease: true` by release.yml so it never becomes the download users
# land on and never moves ghcr's `fleet-hub:latest`.
#
# Build metadata (`0.3.0+build.7`) is refused even though Cargo accepts it:
# `+` is not a legal character in a Docker tag, so hub-image.yml could not
# publish the image for such a version at all.
check_version() {
  [[ "$1" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$ ]] \
    || die "'$1' is not X.Y.Z or X.Y.Z-rc.N ('+build' metadata is not supported: it cannot be a Docker tag)"
}

# The next version, from the Conventional Commits since the last tag — the
# same table docs/RELEASING.md documents and the CHANGELOG grouping below
# uses: a `!` marker or a BREAKING CHANGE footer means major, any `feat` means
# minor, anything else is a patch. Two deliberate limits: it never invents a
# pre-release (an -rc.N is always someone's explicit decision), and from a
# pre-release base it finalises rather than bumps, so `auto` after
# 0.3.0-rc.2 is 0.3.0 and not 0.3.1.
derive_next() {
  local base="$1" range="$2" subjects bodies level major minor patch
  subjects="$(git log --no-merges --format='%s' "$range")"
  [[ -n "$subjects" ]] || die "no commits in $range — nothing to release"
  if [[ "$base" == *-* ]]; then
    echo "${base%%-*}"
    return 0
  fi
  bodies="$(git log --no-merges --format='%b' "$range")"
  if grep -qE '^[a-z]+(\([^)]*\))?!: ' <<<"$subjects" || grep -qE '^BREAKING[ -]CHANGE' <<<"$bodies"; then
    level="major"
  elif grep -qE '^feat(\([^)]*\))?: ' <<<"$subjects"; then
    level="minor"
  else
    level="patch"
  fi
  IFS=. read -r major minor patch <<<"$base"
  case "$level" in
    major) echo "$((major + 1)).0.0" ;;
    minor) echo "$major.$((minor + 1)).0" ;;
    patch) echo "$major.$minor.$((patch + 1))" ;;
  esac
}

NEXT_ONLY=""
case "$1" in
  --next) NEXT_ONLY=1 ;;
  auto) ;;
  *) check_version "$1"; NEW="$1" ;;
esac

cd "$(git rev-parse --show-toplevel)"
CUR="$(node -e 'process.stdout.write(require("./package.json").version)')"
LAST_TAG="$(git describe --tags --abbrev=0 --match 'v[0-9]*' 2>/dev/null || true)"
RANGE="${LAST_TAG:+$LAST_TAG..}HEAD"

if [[ -z "${NEW:-}" ]]; then
  NEW="$(derive_next "$CUR" "$RANGE")"
  check_version "$NEW"
fi

# `--next` answers a question; it touches nothing, so it does not care whether
# the tree is clean or which branch it is on.
if [[ -n "$NEXT_ONLY" ]]; then
  echo "$NEW"
  exit 0
fi

BRANCH="$(git branch --show-current)"
[[ "$BRANCH" == "main" ]] || die "must run on main (currently on '$BRANCH')"
[[ -z "$(git status --porcelain)" ]] || die "working tree is dirty — commit or discard first"
git rev-parse -q --verify "refs/tags/v$NEW" >/dev/null && die "tag v$NEW already exists"
[[ "$CUR" != "$NEW" ]] || die "package.json is already at $NEW"

# CI must have passed on the commit being released. Pushing the tag this
# script creates now publishes a release without anyone looking at it, so the
# one remaining place to catch a red tree is here, before the tag exists.
# scripts/check-ci-green.sh defines "green" and, importantly, refuses when it
# cannot tell — no run for this sha, a run still going, a cancelled or skipped
# run. What it CANNOT cover: the release commit created below is by
# construction newer than anything CI has seen; release.yml's own
# version-consistency job is what covers that commit.
if [[ -n "${RELEASE_SKIP_CI_CHECK:-}" ]]; then
  echo "release.sh: RELEASE_SKIP_CI_CHECK is set — releasing $NEW without checking CI on HEAD" >&2
else
  scripts/check-ci-green.sh \
    || die "CI is not green on HEAD (see above). Fix it, or set RELEASE_SKIP_CI_CHECK=1 if you know why this is safe."
fi

echo "Bumping $CUR -> $NEW"

# --- 1. Version fields (first occurrence only; formatting preserved) ---------
# For a .toml file the bump is scoped to the [package] table — not just the
# first `version = "…"` anywhere in the file — so a `[dependencies.foo]` or
# `[workspace.package]` table ahead of `[package]` can never be bumped by
# mistake (#152).
node - "$NEW" "${VERSION_FILES[@]}" <<'JS'
const fs = require("fs");
const [next, ...files] = process.argv.slice(2);
// Returns [from, to): the byte range of the [package] table's body (after
// its header line, up to the next line starting with `[`, or EOF).
function packageTableRange(f, src) {
  const header = src.match(/^\[package\]\s*$/m);
  if (!header) { console.error(`release.sh: no [package] table in ${f}`); process.exit(1); }
  const from = header.index + header[0].length;
  const rest = src.slice(from);
  const nextHeader = rest.match(/^\[/m);
  const to = nextHeader ? from + nextHeader.index : src.length;
  return [from, to];
}
for (const f of files) {
  const src = fs.readFileSync(f, "utf8");
  if (f.endsWith(".toml")) {
    const [from, to] = packageTableRange(f, src);
    const table = src.slice(from, to);
    const re = /^version = "[^"]+"/m;
    if (!re.test(table)) { console.error(`release.sh: no version field in [package] of ${f}`); process.exit(1); }
    fs.writeFileSync(f, src.slice(0, from) + table.replace(re, `version = "${next}"`) + src.slice(to));
  } else {
    const re = /"version": "[^"]+"/;
    if (!re.test(src)) { console.error(`release.sh: no version field in ${f}`); process.exit(1); }
    fs.writeFileSync(f, src.replace(re, `"version": "${next}"`));
  }
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
    # Scoped to the [package] table only — the first `name = "…"` anywhere in
    # the file could belong to a `[lib]`/`[[bin]]` table ahead of [package]
    # and silently name the wrong crate (#152).
    name="$(node -e '
      const fs = require("fs");
      const f = process.argv[1];
      const src = fs.readFileSync(f, "utf8");
      const header = src.match(/^\[package\]\s*$/m);
      if (!header) { console.error("release.sh: no [package] table in " + f); process.exit(1); }
      const from = header.index + header[0].length;
      const rest = src.slice(from);
      const nextHeader = rest.match(/^\[/m);
      const table = nextHeader ? rest.slice(0, nextHeader.index) : rest;
      const m = table.match(/^name\s*=\s*"([^"]+)"/m);
      if (!m) { console.error("release.sh: no name in [package] of " + f); process.exit(1); }
      process.stdout.write(m[1]);
    ' "$f")" || die "could not read [package] name from $f"
    CARGO_PKGS+=("$name")
  done
  UPDATE_ARGS=()
  for p in "${CARGO_PKGS[@]}"; do UPDATE_ARGS+=(-p "$p"); done
  if ! cargo update "${UPDATE_ARGS[@]}" --offline >/dev/null 2>&1; then
    # Offline failed silently (redirected above) — the online retry's own
    # output is captured and always shown, success or failure, since cargo's
    # error text is what actually names which package in the batch failed.
    if ONLINE_OUT="$(cargo update "${UPDATE_ARGS[@]}" 2>&1)"; then
      printf '%s\n' "$ONLINE_OUT"
    else
      printf '%s\n' "$ONLINE_OUT" >&2
      die "cargo update failed for: ${CARGO_PKGS[*]} (both --offline and online) — Cargo.lock was NOT updated; fix and re-run"
    fi
  fi
  echo "  Cargo.lock (${CARGO_PKGS[*]})"
fi

# --- 3. CHANGELOG.md section, grouped by Conventional Commit type -------------
# LAST_TAG / RANGE were resolved before the bump, because `auto` derives the
# version from the very same commit range this section is built from.
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
echo
echo "That push builds every asset and — once verify-release passes — PUBLISHES"
echo "the release. Nothing else to press. If a leg fails, the release stays a"
echo "draft; fix the leg and re-run the workflow from the same tag."
