---
name: migration-conflicts
description: Resolve a schema migration number collision in claude-fleet — when
  main (or another merged PR) already took this branch's migration number, a
  merge conflicts in store/schema.rs MIGRATIONS, check-migration-numbers.sh or
  the pre-push hook fails, or migrations_are_contiguous_from_one fails. Use it
  before adding a new migration too.
---

# Migration number conflicts

Migrations are `crates/fleet-core/migrations/NNN_<topic>.sql`, registered in
`MIGRATIONS` in `crates/fleet-core/src/store/schema.rs`
(`LATEST_SCHEMA_VERSION` is its last entry). The numbers must run 1..N with no
gap or duplicate (`migrations_are_contiguous_from_one`). Two branches that each
add "the next number" both pass alone and collide when the second merges.
The fix is mechanical and scripted; do not renumber by hand.

## Adding a migration

Take the number from `origin/main`, not your checkout:

```bash
git fetch origin main
git ls-tree --name-only origin/main crates/fleet-core/migrations/ | tail -1   # use this + 1
```

Another open PR may hold the same number; that is fine, whichever merges
second renumbers with the steps below.

## Fixing a collision

1. Bring main in (a merge, never a rebase or force-push on someone else's
   branch):

   ```bash
   git fetch origin main
   git merge origin/main        # a conflict in schema.rs MIGRATIONS is expected
   ```

2. See the plan, then renumber. The script resolves a MIGRATIONS-only conflict
   in schema.rs (main's entries first, then this branch's), moves each
   migration this branch adds to main's highest + 1, + 2, … and updates the
   file name, its `schema_version` INSERT, its `-- N:` header, its MIGRATIONS
   entry and the "migration N" / `migration_N_` mentions on lines this branch
   added. It stages what it changed.

   ```bash
   scripts/renumber-migrations.sh --dry-run
   scripts/renumber-migrations.sh
   ```

   If it stops with "not only MIGRATIONS entries", resolve the other hunks in
   schema.rs by hand (keep both sides), leave the MIGRATIONS hunks, and run it
   again. Resolve conflicts in other files as usual.

3. Read the "still name an old number" lines it prints and fix each by hand:
   typically a test that asserts `version >= N`, deletes rows by version,
   builds a database with `migrations_through(N)`, or a doc / `docs/status.md`
   line. Also grep for the old number in what the branch added:

   ```bash
   git diff origin/main -- . ':!crates/fleet-core/migrations' | grep -nE '^\+.*\b<OLD>\b'
   ```

4. Check and test:

   ```bash
   scripts/check-migration-numbers.sh
   cargo fleet-test -- store::schema      # contiguity, latest version, upgrade chain
   git diff --cached --stat               # only the renames + references you expect
   git commit                             # completes the merge
   ```

   Then run `scripts/verify.sh` as usual before pushing; the pre-push hook
   re-runs the number check against a freshly fetched main.

## Stacked branches

When branch B is stacked on branch A and both add migrations, renumber A
first, then merge A into B and run `scripts/renumber-migrations.sh --base <A>`
so B's migrations follow A's. After A merges, run it again against
`origin/main`.

## Don'ts

- Do not edit a migration number that is already on main; only this branch's
  added files move.
- Do not leave a gap or skip numbers "to be safe": the contiguity test fails.
- Do not commit with `--no-verify` to get past the pre-push number check.
