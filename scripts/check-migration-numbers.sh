#!/usr/bin/env bash
# Fail when a migration this branch adds has a number main already used.
#
# Migrations are `crates/fleet-core/migrations/NNN_<topic>.sql`, applied in
# number order (store/schema.rs `MIGRATIONS`). Two branches that each add
# "the next number" both pass their own tests and collide only when the
# second one reaches main — and by then the renumbering touches the SQL file,
# its `MIGRATIONS` entry, every schema-version assertion and any doc naming
# it. During the Assets redesign (2026-09-29 – 10-05) main took the branch's
# number four times (086, 089, 092, 098–101), each costing 45 min to 1h50m.
# This check moves that discovery to the push.
#
# It fails when:
#   - a migration file the branch adds (present here, absent on the base) has
#     a number <= the highest number on the base, or
#   - two migration files share a number, or
#   - an added file is not named NNN_<topic>.sql.
#
# Usage:
#   scripts/check-migration-numbers.sh            # the working tree vs origin/main
#   scripts/check-migration-numbers.sh --ref SHA  # a commit's tree instead (pre-push)
#   scripts/check-migration-numbers.sh --base REF # another base than origin/main
#   scripts/check-migration-numbers.sh --fetch    # fetch origin's main first
#
# --fetch is best-effort: offline, it warns and checks against the base as it
# is. A base that does not resolve at all (a CI checkout with no origin/main)
# still gets the shared-number check — on a PR's merge commit that is the
# collision itself — and skips the comparison with a warning.
set -euo pipefail

DIR=crates/fleet-core/migrations
base=origin/main
ref=""
fetch=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --base) base=${2:?--base needs a ref}; shift 2 ;;
    --ref) ref=${2:?--ref needs a commit}; shift 2 ;;
    --fetch) fetch=1; shift ;;
    -h|--help) sed -n '2,/^set -euo/p' "$0" | sed -e '$d' -e 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "check-migration-numbers: unknown argument: $1 (see --help)" >&2; exit 2 ;;
  esac
done

ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"

if [[ $fetch == 1 && "$base" == origin/main ]]; then
  if ! git fetch --quiet origin main 2>/dev/null; then
    echo "check-migration-numbers: could not fetch origin main; checking against the local $base" >&2
  fi
fi

base_ok=1
on_base=""
if git rev-parse --verify --quiet "$base^{commit}" >/dev/null; then
  # Basenames, one per line, no blank lines.
  on_base="$(git ls-tree --name-only "$base" -- "$DIR/" | sed -e 's#.*/##' -e '/^$/d' | sort)"
else
  base_ok=0
  echo "check-migration-numbers: $base does not resolve; checking only for shared numbers" >&2
fi
if [[ -n "$ref" ]]; then
  here="$(git ls-tree --name-only "$ref" -- "$DIR/" | sed -e 's#.*/##' -e '/^$/d' | sort)"
else
  here="$( (ls -1 "$DIR" 2>/dev/null || true) | sed '/^$/d' | sort)"
fi

base_max=0
while IFS= read -r f; do
  [[ "$f" =~ ^([0-9]+)_ ]] || continue
  n=$((10#${BASH_REMATCH[1]}))
  if (( n > base_max )); then base_max=$n; fi
done <<< "$on_base"

problems=()

# Shared numbers anywhere in this tree.
dups="$(printf '%s\n' "$here" | sed -n 's/^\([0-9][0-9]*\)_.*/\1/p' | sort | uniq -d)"
for n in $dups; do
  problems+=("number $n is used by more than one file: $(printf '%s\n' "$here" | grep "^${n}_" | tr '\n' ' ')")
done

# What this branch adds.
added="$(comm -13 <(printf '%s\n' "$on_base") <(printf '%s\n' "$here") | sed '/^$/d')"
while IFS= read -r f; do
  [[ -z "$f" ]] && continue
  if [[ ! "$f" =~ ^([0-9]{3})_[A-Za-z0-9_]+\.sql$ ]]; then
    problems+=("$f: not named NNN_<topic>.sql")
    continue
  fi
  n=$((10#${BASH_REMATCH[1]}))
  if (( base_ok == 1 && n <= base_max )); then
    problems+=("$f: number $n is not above $base's highest, $(printf '%03d' "$base_max")")
  fi
done <<< "$added"

if [[ ${#problems[@]} -gt 0 ]]; then
  echo "check-migration-numbers: FAILED" >&2
  for p in "${problems[@]}"; do echo "  $p" >&2; done
  cat >&2 <<EOF

Renumber this branch's migrations from $(printf '%03d' $((base_max + 1))): merge $base first,
then rename each file and update its MIGRATIONS entry in
crates/fleet-core/src/store/schema.rs, the schema-version assertions and any
doc that names the number. The latest number on main is always:
  git ls-tree --name-only $base $DIR/ | tail -1
EOF
  exit 1
fi

if [[ $base_ok == 0 ]]; then
  echo "check-migration-numbers: ok (no shared numbers; base not checked)"
  exit 0
fi
count=$(printf '%s\n' "$added" | sed '/^$/d' | wc -l | tr -d ' ')
echo "check-migration-numbers: ok ($count added; $base's highest is $(printf '%03d' "$base_max"))"
