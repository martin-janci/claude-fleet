#!/usr/bin/env bash
# Move this branch's migrations to the next free numbers on main.
#
# `migrations_are_contiguous_from_one` allows no gap in MIGRATIONS, so two
# branches that each add a migration both take "the next number", and the one
# that merges second has to renumber: the SQL file, its `schema_version` row,
# its MIGRATIONS entry in crates/fleet-core/src/store/schema.rs and every doc
# comment that names the number. On 2026-10-08 the redesign lanes did that by
# hand for numbers 127-133, several times each. This does it in one step.
#
# Run it after `git merge origin/main`, with or without the merge's conflict
# in schema.rs still open:
#
#   git merge origin/main            # schema.rs: MIGRATIONS conflicts
#   scripts/renumber-migrations.sh   # resolves it, renumbers, stages
#   scripts/check-migration-numbers.sh && git commit
#
# It:
#   - resolves a conflict in schema.rs whose every hunk sits in MIGRATIONS
#     (both sides only add entries) by keeping main's entries, then this
#     branch's; any other conflict there stops it, untouched;
#   - renumbers each migration file this branch adds (present here, absent on
#     the base) to base's highest + 1, + 2, ... in its current order: the file
#     name, its `INSERT ... schema_version (version) VALUES (N)` row and a
#     leading `-- N:` comment, its MIGRATIONS entry, and "migration N" /
#     `migration_N_` on the lines this branch added to other files;
#   - stages what it changed and prints every other line this branch added
#     that still names an old number, for a look by hand (a test that
#     deletes `version >= N`, say).
#
# Usage:
#   scripts/renumber-migrations.sh              # against origin/main
#   scripts/renumber-migrations.sh --base REF   # another base
#   scripts/renumber-migrations.sh --dry-run    # print the plan only
set -euo pipefail

DIR=crates/fleet-core/migrations
SCHEMA=crates/fleet-core/src/store/schema.rs
base=origin/main
dry=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --base) base=${2:?--base needs a ref}; shift 2 ;;
    --dry-run) dry=1; shift ;;
    -h|--help) sed -n '2,/^set -euo/p' "$0" | sed -e '$d' -e 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "renumber-migrations: unknown argument: $1 (see --help)" >&2; exit 2 ;;
  esac
done

cd "$(git rev-parse --show-toplevel)"
git rev-parse --verify --quiet "$base^{commit}" >/dev/null \
  || { echo "renumber-migrations: $base does not resolve" >&2; exit 2; }

# 1. The MIGRATIONS conflict, if the merge left one.
if grep -q '^<<<<<<< ' "$SCHEMA"; then
  if [[ $dry == 1 ]]; then
    echo "would resolve the MIGRATIONS conflict in $SCHEMA (main's entries, then this branch's)"
  else
    perl -0777 -e '
      local $/; my $t = <>; my $bad = 0;
      my $entry = qr#^\s*(?://.*|Migration(?:::plain)?\s*[({].*|\d+,|version:\s*\d+,|sql:\s*include_str!.*|already_applied:.*|include_str!.*|\),|\},)?$#;
      my $fix = sub {
        my ($ours, $theirs, $after) = @_;
        for my $l (split /\n/, $ours . $theirs) { $bad = 1 unless $l =~ $entry; }
        # When both sides end in the same `),` or `},`, git leaves that line
        # after the conflict, and the base side needs its own copy of it.
        my $open = () = $theirs =~ /[({]/g;
        my $shut = () = $theirs =~ /[)}]/g;
        $theirs .= $after if $open > $shut;
        return $theirs . $ours;
      };
      $t =~ s/^<<<<<<< [^\n]*\n(.*?)^=======\n(.*?)^>>>>>>> [^\n]*\n(?=((?:[^\n]*\n)?))/$fix->($1, $2, $3)/gsme;
      die "a conflict in schema.rs is not only MIGRATIONS entries; resolve it by hand\n" if $bad;
      die "schema.rs still has conflict markers; resolve it by hand\n" if $t =~ /^(?:<{7}|>{7}) /m;
      print $t;
    ' "$SCHEMA" > "$SCHEMA.renumber" && mv "$SCHEMA.renumber" "$SCHEMA" \
      || { rm -f "$SCHEMA.renumber"; echo "renumber-migrations: $SCHEMA left as it was" >&2; exit 1; }
    git add "$SCHEMA"
  fi
fi

# 2. What to renumber.
base_max=0
while IFS= read -r f; do
  [[ "$f" =~ ^([0-9]+)_ ]] || continue
  n=$((10#${BASH_REMATCH[1]}))
  (( n > base_max )) && base_max=$n
done < <(git ls-tree --name-only "$base" -- "$DIR/" | sed 's#.*/##')

on_base="$(git ls-tree --name-only "$base" -- "$DIR/" | sed 's#.*/##' | sort)"
here="$(ls -1 "$DIR" | sort)"
mapfile -t added < <(comm -13 <(printf '%s\n' "$on_base") <(printf '%s\n' "$here") | grep -E '^[0-9]+_.*\.sql$' | sort -n)

if [[ ${#added[@]} -eq 0 ]]; then
  echo "renumber-migrations: this branch adds no migration"
  exit 0
fi

# Files this branch changed, for the doc comments (not the SQL, not schema's
# MIGRATIONS entry, both handled on their own).
mapfile -t touched < <(git diff --name-only "$base" -- crates src src-tauri docs | grep -v "^$DIR/" || true)

next=$((base_max + 1))
moved=0
changed=()
for f in "${added[@]}"; do
  [[ "$f" =~ ^([0-9]+)_(.*)$ ]]
  old=$((10#${BASH_REMATCH[1]})); topic=${BASH_REMATCH[2]}
  new=$next; next=$((next + 1))
  if (( old == new )); then
    echo "$f: already $new"
    continue
  fi
  nf="$(printf '%03d' "$new")_$topic"
  echo "$f -> $nf"
  [[ $dry == 1 ]] && continue
  moved=1
  git mv -f "$DIR/$f" "$DIR/$nf" 2>/dev/null || mv "$DIR/$f" "$DIR/$nf"
  OLD=$old NEW=$new perl -i -pe '
    s/(schema_version\s*\(version\)\s*VALUES\s*\()$ENV{OLD}(\))/${1}$ENV{NEW}$2/;
    s/^-- $ENV{OLD}:/-- $ENV{NEW}:/;
  ' "$DIR/$nf"
  OF=$f NF=$nf OLD=$old NEW=$new perl -0777 -i -pe '
    my ($of, $nf) = (quotemeta $ENV{OF}, $ENV{NF});
    s{(Migration::plain\(\s*)$ENV{OLD}(,\s*include_str!\("\.\./\.\./migrations/)$of}{${1}$ENV{NEW}${2}$nf}g;
    s{(version:\s*)$ENV{OLD}(,\s*sql:\s*include_str!\("\.\./\.\./migrations/)$of}{${1}$ENV{NEW}${2}$nf}g;
  ' "$SCHEMA"
  # Doc comments: only the lines this branch added, never main's own.
  for t in "${touched[@]}"; do
    [[ -f "$t" ]] || continue
    before="$(git hash-object "$t")"
    git show "$base:$t" > "$t.renumber-base" 2>/dev/null || : > "$t.renumber-base"
    OLD=$old NEW=$new BASEF="$t.renumber-base" perl -i -ne '
      BEGIN { open my $b, "<", $ENV{BASEF}; %seen = map { $_ => 1 } <$b>; }
      unless ($seen{$_}) {
        s/\bmigration $ENV{OLD}\b/migration {{renum:$ENV{NEW}}}/g;
        s/\bmigration_$ENV{OLD}_/migration_{{renum:$ENV{NEW}}}_/g;
      }
      print;
    ' "$t"
    rm -f "$t.renumber-base"
    if [[ "$(git hash-object "$t")" != "$before" ]]; then changed+=("$t"); fi
  done
done
# The doc comments went through a placeholder: with two moves, 135 -> 136
# then 136 -> 137, a direct rewrite would carry the first one's new
# "migration 136" on to 137.
for t in ${changed[@]+"${changed[@]}"}; do
  perl -i -pe 's/\{\{renum:(\d+)\}\}/$1/g' "$t"
done

[[ $dry == 1 ]] && exit 0
if [[ $moved == 1 ]]; then
  # Only what this script edited: staging a file the merge left in conflict
  # would mark it resolved with its markers still in.
  git add -A "$DIR" "$SCHEMA" ${changed[@]+"${changed[@]}"}
fi

# 3. What is left for a look by hand.
left="$(git diff --cached -U0 "$base" -- . ":!$DIR" | grep -E '^\+[^+]' \
  | grep -E "\\b($(printf '%s|' "${added[@]%%_*}" | sed 's/|$//' | sed 's/\b0*\([0-9]\)/\1/g'))\\b" || true)"
if [[ -n "$left" ]]; then
  echo "renumber-migrations: lines this branch added that still name an old number (check by hand):"
  printf '%s\n' "$left" | head -20
fi
echo "renumber-migrations: done; run scripts/check-migration-numbers.sh, then the schema tests"
