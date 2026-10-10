#!/usr/bin/env bash
# Tests for scripts/renumber-migrations.sh against a throwaway repo whose
# refs/remotes/origin/main plays main. bash + git + perl, about a second.
# PASS/FAIL lines, exit 1 on any failure.
set -uo pipefail

# Under a git hook git exports GIT_DIR and friends; drop them so the
# throwaway repo's commands never act on the real one.
while IFS= read -r v; do unset "$v"; done < <(env | sed -n 's/^\(GIT_[A-Za-z_]*\)=.*/\1/p')

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
S="$REPO/scripts/renumber-migrations.sh"
PASS=0; FAIL=0
T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT

D=crates/fleet-core/migrations
SCHEMA=crates/fleet-core/src/store/schema.rs
g() { git -C "$T/r" -c user.email=t@t -c user.name=t -c commit.gpgsign=false "$@"; }
ok() { PASS=$((PASS + 1)); echo "PASS: $1"; }
bad() { FAIL=$((FAIL + 1)); echo "FAIL: $1" >&2; [[ -n "${2:-}" ]] && printf '%s\n' "$2" >&2; }
has() { if grep -qF -- "$2" "$T/r/$3"; then ok "$1"; else bad "$1" "$(cat "$T/r/$3")"; fi; }
lacks() { if grep -qF -- "$2" "$T/r/$3"; then bad "$1" "$(cat "$T/r/$3")"; else ok "$1"; fi; }

sql() { printf -- '-- %d: %s\nCREATE TABLE IF NOT EXISTS %s (id INTEGER);\nINSERT OR IGNORE INTO schema_version (version) VALUES (%d);\n' "$1" "$2" "$2" "$1"; }
schema() {
  printf 'const MIGRATIONS: &[Migration] = &[\n'
  printf '    Migration::plain(1, include_str!("../../migrations/001_init.sql")),\n'
  for e in "$@"; do printf '%s\n' "$e"; done
  printf '];\n'
}

# main: 001. The branch adds 002_tasks (a plain entry, a doc comment naming
# it); main meanwhile adds 002_assets (a guarded entry).
mkdir -p "$T/r/$D" "$T/r/$(dirname "$SCHEMA")" "$T/r/crates/fleet-core/src/store"
g init -q -b work
sql 1 init > "$T/r/$D/001_init.sql"
schema > "$T/r/$SCHEMA"
printf '//! Store helpers (migration 1 made the first table).\n' > "$T/r/crates/fleet-core/src/store/doc.rs"
mkdir -p "$T/r/src"; printf 'base\n' > "$T/r/src/page.ts"
g add -A && g commit -qm main
g update-ref refs/remotes/origin/main HEAD

sql 2 tasks > "$T/r/$D/002_tasks.sql"
schema '    // tasks
    Migration::plain(2, include_str!("../../migrations/002_tasks.sql")),' > "$T/r/$SCHEMA"
printf '//! Store helpers (migration 1 made the first table).\n//! `tasks` (migration 2) holds the work.\n' > "$T/r/crates/fleet-core/src/store/doc.rs"
printf 'branch\n' > "$T/r/src/page.ts"
g add -A && g commit -qm "branch: 002_tasks"

g checkout -q --detach origin/main
sql 2 assets > "$T/r/$D/002_assets.sql"
schema '    // assets
    Migration {
        version: 2,
        sql: include_str!("../../migrations/002_assets.sql"),
        already_applied: Some(has_assets),
    },' > "$T/r/$SCHEMA"
printf 'main\n' > "$T/r/src/page.ts"
g add -A && g commit -qm "main: 002_assets"
g update-ref refs/remotes/origin/main HEAD
g checkout -q work

# The merge conflicts in MIGRATIONS, as it does for real.
g merge -q --no-edit origin/main >/dev/null 2>&1
if grep -q '^<<<<<<<' "$T/r/$SCHEMA"; then ok "the merge leaves a MIGRATIONS conflict"; else bad "no conflict to resolve"; fi

out="$(cd "$T/r" && bash "$S" 2>&1)"; code=$?
if [[ $code == 0 ]]; then ok "exits 0"; else bad "exit $code" "$out"; fi
if [[ "$out" == *"002_tasks.sql -> 003_tasks.sql"* ]]; then ok "names the move"; else bad "names the move" "$out"; fi

if [[ -f "$T/r/$D/003_tasks.sql" && ! -e "$T/r/$D/002_tasks.sql" ]]; then ok "file renamed"; else bad "file renamed" "$(ls "$T/r/$D")"; fi
has "schema_version row renumbered" "VALUES (3);" "$D/003_tasks.sql"
has "leading comment renumbered" "-- 3: tasks" "$D/003_tasks.sql"
has "main's file untouched" "VALUES (2);" "$D/002_assets.sql"
lacks "conflict resolved" "<<<<<<<" "$SCHEMA"
has "main's entry kept" 'version: 2,' "$SCHEMA"
has "branch entry renumbered" 'Migration::plain(3, include_str!("../../migrations/003_tasks.sql"))' "$SCHEMA"
if awk '/002_assets/{a=NR} /003_tasks/{b=NR} END{exit !(a && b && a < b)}' "$T/r/$SCHEMA"; then ok "main's entry first"; else bad "main's entry first" "$(cat "$T/r/$SCHEMA")"; fi
has "the branch's doc comment follows" '`tasks` (migration 3)' crates/fleet-core/src/store/doc.rs
has "main's doc comment does not" 'migration 1 made' crates/fleet-core/src/store/doc.rs
if [[ "$(g diff --name-only --diff-filter=U)" == "src/page.ts" ]]; then ok "an unrelated conflict stays unmerged"; else bad "unmerged paths" "$(g diff --name-only --diff-filter=U)"; fi
printf 'both\n' > "$T/r/src/page.ts"; g add src/page.ts

out="$(cd "$T/r" && bash "$S" 2>&1)"
if [[ "$out" == *"003_tasks.sql: already 3"* ]]; then ok "a second run changes nothing"; else bad "second run" "$out"; fi

# A conflict outside MIGRATIONS is left alone.
g commit -qm merged
g checkout -q -b other HEAD~1 2>/dev/null || g checkout -q -b other
printf 'fn a() {}\n' >> "$T/r/$SCHEMA"; g commit -qam "other: fn a"
g checkout -q work
printf 'fn b() {}\n' >> "$T/r/$SCHEMA"; g commit -qam "work: fn b"
g merge -q --no-edit other >/dev/null 2>&1
before="$(cat "$T/r/$SCHEMA")"
out="$(cd "$T/r" && bash "$S" --base other 2>&1)"; code=$?
if [[ $code != 0 && "$(cat "$T/r/$SCHEMA")" == "$before" ]]; then ok "a non-MIGRATIONS conflict stops it, file untouched"; else bad "non-MIGRATIONS conflict" "exit $code: $out"; fi

# Both sides add a multi-line plain entry: git keeps their shared `),` after
# the conflict, and main's entry must not lose it.
rm -rf "$T/r"; mkdir -p "$T/r/$D" "$T/r/$(dirname "$SCHEMA")"
g init -q -b work
sql 1 init > "$T/r/$D/001_init.sql"
schema > "$T/r/$SCHEMA"
g add -A && g commit -qm main
g update-ref refs/remotes/origin/main HEAD
sql 2 tasks > "$T/r/$D/002_tasks.sql"
schema '    // tasks
    Migration::plain(
        2,
        include_str!("../../migrations/002_tasks.sql"),
    ),' > "$T/r/$SCHEMA"
g add -A && g commit -qm "branch: 002_tasks"
g checkout -q --detach origin/main
sql 2 assets > "$T/r/$D/002_assets.sql"
schema '    // assets
    Migration::plain(
        2,
        include_str!("../../migrations/002_assets.sql"),
    ),' > "$T/r/$SCHEMA"
g add -A && g commit -qm "main: 002_assets"
g update-ref refs/remotes/origin/main HEAD
g checkout -q work
g merge -q --no-edit origin/main >/dev/null 2>&1
out="$(cd "$T/r" && bash "$S" 2>&1)"; code=$?
if [[ $code == 0 ]]; then ok "multi-line entries: exits 0"; else bad "multi-line entries: exit $code" "$out"; fi
if [[ "$(grep -c '^    ),$' "$T/r/$SCHEMA")" == 2 ]]; then ok "each multi-line entry keeps its closing line"; else bad "closing lines" "$(cat "$T/r/$SCHEMA")"; fi
has "multi-line branch entry renumbered" '003_tasks.sql' "$SCHEMA"

# The branch adds two (002, 003) and main takes 002: each doc comment moves
# once, 002 -> 003 and 003 -> 004, never 002 -> 003 -> 004.
rm -rf "$T/r"; mkdir -p "$T/r/$D" "$T/r/$(dirname "$SCHEMA")"
g init -q -b work
sql 1 init > "$T/r/$D/001_init.sql"
schema > "$T/r/$SCHEMA"
g add -A && g commit -qm main
g update-ref refs/remotes/origin/main HEAD
sql 2 tasks > "$T/r/$D/002_tasks.sql"
sql 3 notes > "$T/r/$D/003_notes.sql"
schema '    Migration::plain(2, include_str!("../../migrations/002_tasks.sql")),
    Migration::plain(3, include_str!("../../migrations/003_notes.sql")),' > "$T/r/$SCHEMA"
printf '//! `tasks` (migration 2).\n//! `notes` (migration 3).\n' > "$T/r/crates/fleet-core/src/store/two.rs"
g add -A && g commit -qm "branch: 002_tasks, 003_notes"
g checkout -q --detach origin/main
sql 2 assets > "$T/r/$D/002_assets.sql"
schema '    Migration::plain(2, include_str!("../../migrations/002_assets.sql")),' > "$T/r/$SCHEMA"
g add -A && g commit -qm "main: 002_assets"
g update-ref refs/remotes/origin/main HEAD
g checkout -q work
g merge -q --no-edit origin/main >/dev/null 2>&1
out="$(cd "$T/r" && bash "$S" 2>&1)"; code=$?
if [[ $code == 0 ]]; then ok "two moves: exits 0"; else bad "two moves: exit $code" "$out"; fi
has "two moves: the first comment moves once" '`tasks` (migration 3)' crates/fleet-core/src/store/two.rs
has "two moves: the second comment moves once" '`notes` (migration 4)' crates/fleet-core/src/store/two.rs
lacks "two moves: no placeholder left" '{{renum:' crates/fleet-core/src/store/two.rs

echo "renumber-migrations-test: $PASS passed, $FAIL failed"
[[ $FAIL == 0 ]]
