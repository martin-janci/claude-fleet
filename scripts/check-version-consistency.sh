#!/usr/bin/env bash
# Assert that every version carrier in this repo agrees, and (optionally) that
# a release tag agrees with them too.
#
# Nothing in CI used to check this: `scripts/release.sh` rewrites seven files
# (six version carriers plus the hub image pin — see below) and syncs four
# Cargo.lock entries, and a hand bump of one of them (as happened for 0.2.27,
# commit fa8f3a30) went unnoticed — while `release.yml` never compared the
# pushed tag against `src-tauri/tauri.conf.json`, the file the desktop bundle
# filenames come from (F-C5, F-C6, F-C10).
#
# The carrier list is NOT copied here: it is `scripts/release.sh --list`, i.e.
# that script's own VERSION_FILES, so a carrier added there is checked here the
# same day. `package.json` is the source of truth (docs/RELEASING.md); every
# other carrier, and every Cargo.lock entry for a crate this repo versions,
# must hold the byte-identical string.
#
# One more file is checked by a different rule: the hub image pin in
# `deploy/hub/docker-compose.yml` (path from `release.sh --list-image-pin`).
# Its version lives in a Docker tag, which carries no `v` and must not follow a
# pre-release — see section 5.
#
# Usage:
#   scripts/check-version-consistency.sh                       # carriers only
#   scripts/check-version-consistency.sh --expect-tag v1.2.3    # also assert the tag
#
# Exit status: 0 = everything agrees; 1 = a disagreement (every offending file
# and its value is printed); 2 = usage error.
#
# Fast by construction: text reads plus one `node` per file. No cargo, no
# network, no pnpm install.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

# --- deliberate exceptions ---------------------------------------------------
# Crates that are deliberately NOT versioned in step with the app. Format:
# <path to Cargo.toml>|<crate name>|<pinned version>|<why>. An entry here is
# checked, not skipped: the pinned version must still be exactly what the file
# and Cargo.lock say, and the crate must carry `publish = false` so the
# exception cannot be mistaken for drift. An exempt path that also shows up in
# `release.sh --list` is a contradiction and fails.
EXEMPT=(
  "crates/fleet-core/Cargo.toml|fleet-core|0.1.0|internal library crate, consumed only by path inside this workspace; excluded from scripts/release.sh's VERSION_FILES"
)

usage() {
  echo "usage: scripts/check-version-consistency.sh [--expect-tag <vX.Y.Z>]" >&2
  exit 2
}

EXPECT_TAG=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --expect-tag)
      [[ $# -ge 2 ]] || usage
      EXPECT_TAG="$2"
      shift 2
      ;;
    -h | --help)
      sed -n '2,/^set -euo/p' "${BASH_SOURCE[0]}" | sed '$d' | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *) usage ;;
  esac
done

PROBLEMS=()
problem() { PROBLEMS+=("$1"); }

# Value of <field> in a .toml file's [package] table — scoped to that table, so
# a `[dependencies.foo]` or `[workspace.package]` table ahead of it can never
# answer instead (the same scoping scripts/release.sh uses). Surrounding double
# quotes are stripped; a missing field prints nothing and exits 0, so callers
# can distinguish "absent" from "wrong".
toml_package_field() {
  node - "$1" "$2" <<'JS'
const fs = require("fs");
const [f, field] = process.argv.slice(2);
const src = fs.readFileSync(f, "utf8");
const header = src.match(/^\[package\]\s*$/m);
if (!header) { console.error(`check-version-consistency: no [package] table in ${f}`); process.exit(1); }
const from = header.index + header[0].length;
const rest = src.slice(from);
const next = rest.match(/^\[/m);
const table = next ? rest.slice(0, next.index) : rest;
const m = table.match(new RegExp(`^${field}\\s*=\\s*(.+?)\\s*$`, "m"));
if (!m) process.exit(0);
process.stdout.write(m[1].replace(/^"(.*)"$/, "$1"));
JS
}

# Top-level "version" of a .json file.
json_version() {
  node - "$1" <<'JS'
const fs = require("fs");
const f = process.argv[2];
const j = JSON.parse(fs.readFileSync(f, "utf8"));
if (typeof j.version !== "string") { console.error(`check-version-consistency: no string "version" in ${f}`); process.exit(1); }
process.stdout.write(j.version);
JS
}

carrier_version() {
  case "$1" in
    *.toml) toml_package_field "$1" version ;;
    *.json) json_version "$1" ;;
    *)
      echo "check-version-consistency: don't know how to read a version from '$1'" >&2
      exit 1
      ;;
  esac
}

# The `version` of a [[package]] entry in Cargo.lock. Prints nothing when the
# crate has no entry at all.
lock_version() {
  awk -v want="$1" '
    /^\[\[package\]\]/ { name = ""; next }
    /^name = "/    { name = $0; sub(/^name = "/, "", name); sub(/"$/, "", name); next }
    /^version = "/ {
      if (name == want) {
        ver = $0; sub(/^version = "/, "", ver); sub(/"$/, "", ver)
        print ver; exit
      }
    }
  ' Cargo.lock
}

# --- 1. the carrier list, straight from the release script -------------------
# A read loop, not `mapfile`: no bash 4 builtin, so this also runs under the
# bash 3.2 that ships with macOS (like every other script in scripts/).
CARRIERS=()
while IFS= read -r line; do CARRIERS+=("$line"); done < <(scripts/release.sh --list)
[[ ${#CARRIERS[@]} -gt 0 ]] || {
  echo "check-version-consistency: scripts/release.sh --list printed nothing" >&2
  exit 1
}
for f in "${CARRIERS[@]}"; do
  [[ -f "$f" ]] || {
    echo "check-version-consistency: scripts/release.sh --list names a missing file: $f" >&2
    exit 1
  }
done

# package.json is the source of truth every other carrier is compared against.
printf '%s\n' "${CARRIERS[@]}" | grep -qx 'package.json' || {
  echo "check-version-consistency: package.json is not in scripts/release.sh --list; it is the source of truth" >&2
  exit 1
}
VERSION="$(json_version package.json)"
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]] || {
  echo "check-version-consistency: package.json version '$VERSION' is not semver" >&2
  exit 1
}
echo "expected version (package.json): $VERSION"

# --- 2. every carrier holds it verbatim --------------------------------------
for f in "${CARRIERS[@]}"; do
  got="$(carrier_version "$f")"
  if [[ "$got" == "$VERSION" ]]; then
    printf '  ok    %-32s %s\n' "$f" "$got"
  else
    printf '  WRONG %-32s %s\n' "$f" "${got:-<no version field>}"
    problem "$f has version '${got:-<none>}', expected '$VERSION'"
  fi
done

# --- 3. Cargo.lock follows the crates this repo versions ---------------------
# One entry per .toml carrier, addressed by the crate's real [package] name
# (`src-tauri/Cargo.toml` is the crate `claude-fleet`, not `src-tauri`).
for f in "${CARRIERS[@]}"; do
  [[ "$f" == *.toml ]] || continue
  name="$(toml_package_field "$f" name)"
  [[ -n "$name" ]] || {
    echo "check-version-consistency: no name in [package] of $f" >&2
    exit 1
  }
  got="$(lock_version "$name")"
  if [[ "$got" == "$VERSION" ]]; then
    printf '  ok    %-32s %s\n' "Cargo.lock ($name)" "$got"
  else
    printf '  WRONG %-32s %s\n' "Cargo.lock ($name)" "${got:-<no entry>}"
    problem "Cargo.lock entry for '$name' is '${got:-<none>}', expected '$VERSION' — run: cargo update -p $name"
  fi
done

# --- 4. the declared exceptions are exactly as declared ----------------------
for entry in "${EXEMPT[@]}"; do
  IFS='|' read -r path name pinned why <<<"$entry"
  printf '  note  %-32s %s (allowlisted: %s)\n' "$path" "$pinned" "$why"
  if printf '%s\n' "${CARRIERS[@]}" | grep -qx -- "$path"; then
    problem "$path is both allowlisted as version-exempt and listed by scripts/release.sh --list — pick one"
    continue
  fi
  [[ -f "$path" ]] || {
    problem "allowlisted $path does not exist — drop it from EXEMPT in $(basename "${BASH_SOURCE[0]}")"
    continue
  }
  got="$(toml_package_field "$path" version)"
  [[ "$got" == "$pinned" ]] || problem "$path has version '${got:-<none>}', but the allowlist pins it at '$pinned'"
  got_name="$(toml_package_field "$path" name)"
  [[ "$got_name" == "$name" ]] || problem "$path is the crate '$got_name', but the allowlist calls it '$name'"
  # publish = false is what makes the exception enforced rather than
  # conventional: nothing can push this 0.1.0 to a registry (F-C22).
  pub="$(toml_package_field "$path" publish)"
  [[ "$pub" == "false" ]] || problem "$path is version-exempt but has no 'publish = false' in [package] (found '${pub:-<none>}')"
  got_lock="$(lock_version "$name")"
  [[ "$got_lock" == "$pinned" ]] || problem "Cargo.lock entry for '$name' is '${got_lock:-<none>}', but the allowlist pins it at '$pinned'"
done

# --- 5. the shipped hub deployment pins a real, published image --------------
# deploy/hub/docker-compose.yml is the file operators curl and run, and its
# `image:` tag is a version carrier in everything but syntax (F-C9). Its rule
# differs from the carriers above in two ways, both deliberate:
#   * the image tag has no `v` prefix (`v1.2.3` -> `1.2.3`), and
#   * on a PRE-RELEASE the pin must NOT follow. `main` is where docs/hub.md
#     tells operators to fetch this file from, so a `0.3.0-rc.1` pin there
#     would hand a release candidate to the next hub that is set up. The pin
#     stays on the last stable release until the finalising tag moves it —
#     the same rule hub-image.yml applies to `latest`.
#
# "the last stable release" is asserted, not merely "some plain X.Y.Z". The
# looser rule let any stable-shaped string through during an rc window — pin
# 0.1.0 under version 0.3.0-rc.1 passed, and 0.1.0 has never been published as
# a hub image — which is precisely the window where the pin gets hand-edited.
# The oracle is CHANGELOG.md, newest-first and in this same tree, so the check
# stays offline: its newest heading without a pre-release suffix IS the last
# stable release.
#
# WHAT THIS DOES AND DOES NOT PROVE. It proves the pin agrees with the rest of
# the tree. It does NOT prove the image is in the registry — nothing here
# touches ghcr, and the pin is written into the release commit BEFORE
# hub-image.yml has built anything. `scripts/check-release-drift.sh` check 5
# is what asks the registry, daily.
# The path comes from `scripts/release.sh --list-image-pin`, not a copy here.
PIN_FILE="$(scripts/release.sh --list-image-pin)"
if [[ ! -f "$PIN_FILE" ]]; then
  echo "check-version-consistency: scripts/release.sh --list-image-pin names a missing file: $PIN_FILE" >&2
  exit 1
fi
# The `image:` line for fleet-hub, in `name:tag` form, ignoring comments.
PIN="$(sed -n 's|^[[:space:]]*image:[[:space:]]*ghcr\.io/[^:@[:space:]]*/fleet-hub:\([^[:space:]#]*\).*|\1|p' "$PIN_FILE" | head -n1)"
if [[ -z "$PIN" ]]; then
  echo "check-version-consistency: no 'image: ghcr.io/<owner>/fleet-hub:<tag>' line in $PIN_FILE" >&2
  echo "  (a digest pin is not checkable here — if that is intended, teach this script about it)" >&2
  exit 1
fi
if [[ "$VERSION" == *-* ]]; then
  # Pre-release: the pin must be the LAST STABLE release. The newest
  # `## [X.Y.Z]` heading in CHANGELOG.md with no pre-release suffix — the
  # pattern cannot match `## [0.3.0-rc.1]`, because a `-` stands where the `]`
  # has to be, so pre-release sections are skipped without a second rule.
  LAST_STABLE="$(sed -n 's/^## \[\([0-9][0-9]*\.[0-9][0-9]*\.[0-9][0-9]*\)\].*/\1/p' CHANGELOG.md | head -n1)"
  if [[ -z "$LAST_STABLE" ]]; then
    # Nothing stable has ever been released. Fall back to the shape rule and
    # say so, rather than inventing an expectation out of an empty CHANGELOG.
    if [[ "$PIN" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
      printf '  ok    %-32s %s (pre-release %s; CHANGELOG.md records no stable release to compare with)\n' "$PIN_FILE" "$PIN" "$VERSION"
    else
      printf '  WRONG %-32s %s\n' "$PIN_FILE" "$PIN"
      problem "$PIN_FILE pins the hub image at '$PIN'; during the pre-release $VERSION it must stay on a stable X.Y.Z"
    fi
  elif [[ "$PIN" == "$LAST_STABLE" ]]; then
    printf '  ok    %-32s %s (pre-release %s does not move the pin off the last stable release)\n' "$PIN_FILE" "$PIN" "$VERSION"
  else
    printf '  WRONG %-32s %s\n' "$PIN_FILE" "$PIN"
    problem "$PIN_FILE pins the hub image at '$PIN'; during the pre-release $VERSION it must stay on the last stable release, '$LAST_STABLE' (newest stable section in CHANGELOG.md)"
  fi
elif [[ "$PIN" == "$VERSION" ]]; then
  printf '  ok    %-32s %s\n' "$PIN_FILE" "$PIN"
else
  printf '  WRONG %-32s %s\n' "$PIN_FILE" "$PIN"
  problem "$PIN_FILE pins the hub image at '$PIN', expected '$VERSION' (no 'v' prefix) — scripts/release.sh rewrites this line"
fi

# --- 6. on a release tag: the tag name too -----------------------------------
if [[ -n "$EXPECT_TAG" ]]; then
  if [[ "$EXPECT_TAG" == "v$VERSION" ]]; then
    printf '  ok    %-32s %s\n' "git tag" "$EXPECT_TAG"
  else
    printf '  WRONG %-32s %s\n' "git tag" "$EXPECT_TAG"
    problem "tag '$EXPECT_TAG' does not match the repo version — expected 'v$VERSION'"
  fi
fi

# --- verdict -----------------------------------------------------------------
if [[ ${#PROBLEMS[@]} -gt 0 ]]; then
  echo >&2
  echo "check-version-consistency: ${#PROBLEMS[@]} disagreement(s):" >&2
  for p in "${PROBLEMS[@]}"; do echo "  - $p" >&2; done
  echo >&2
  echo "Every carrier is rewritten by scripts/release.sh <X.Y.Z>; do not edit one by hand." >&2
  exit 1
fi

echo "check-version-consistency: all carriers agree on $VERSION"
