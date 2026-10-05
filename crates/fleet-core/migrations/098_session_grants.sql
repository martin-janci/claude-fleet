-- Multi-user M1, T4 (plan docs/superpowers/plans/2026-09-30-multi-user-m1-private-sessions.md,
-- spec docs/superpowers/specs/2026-09-30-multi-user-gap-analysis.md §4.3): the
-- owner SHARES a session, explicitly, revocably, and downward only.
--
-- Migration 094 gave the hub its people and 095 gave every session an owner
-- and a visibility. A `private` row is readable by exactly one person, which
-- is the point — and useless the first time two colleagues need to look at
-- the same agent. `session_grants` is the one way a second person ever
-- reaches a row: one row per (session, recipient), two grantable levels, a
-- revocation stamp, and nothing else. There is no ACL engine here and no
-- inheritance; the table is deliberately small because every column is a way
-- round the privacy rule if it is the wrong column.
--
-- What this script does, statement by statement (named rather than counted,
-- 096 keeps 094's convention):
--
--   CREATE TABLE session_grants      one live grant per (session, recipient)
--   idx_session_grants_live          ... enforced, NULL-safely
--   idx_session_grants_person        the per-request "what am I granted?" read
--   idx_session_grants_session       the Share sheet's read, and the cascade
--
-- Every statement is `IF NOT EXISTS`, so the script is idempotent as written
-- and needs no `already_applied` guard in `store/schema.rs` — there is no
-- `ALTER TABLE ... ADD COLUMN` here, which is the one statement shape that
-- cannot be written that way.
--
-- The RULES are not in this file. They are in `store/session_grants.rs`,
-- because three of the four cannot be expressed as a constraint: "only the
-- owner creates a grant" is a join against `sessions.owner_person_id`, "a
-- grantee cannot grant on" is the same check seen from the other side, and
-- "downward only" is a property of which statements exist at all. What the
-- schema CAN carry it does carry — the level domain, the exactly-one
-- recipient rule, and the one-live-grant index — so that a hand-written
-- UPDATE cannot produce a row the Rust would have refused.

-- session_id   the row shared, not "the session": a grant is on a row, and a
--              `move_session` creates a new row on the target host, so grants
--              do not survive a move (owner's decision 2, T5). `ON DELETE
--              CASCADE` so a reaped session takes its grants with it — pinned
--              by a test rather than trusted, because `sessions.id` is a
--              rowid alias (001) and SQLite reuses the highest deleted value,
--              so a leaked grant would silently re-attach to somebody else's
--              session.
-- person_id    the recipient, and in M1 the ONLY kind of recipient there is.
-- org_id       reserved for M2. The column exists so the unique index and the
--              CHECK below are written once, in their final shape; M1's
--              `Store::grant_session` refuses an org recipient outright
--              (`E_INVALID`), because an org names a recipient SET whose
--              membership `work_admin { assign_client }` writes under
--              `Access::Master` — an admin would bind their own device to the
--              org and read the session with no grant touched and no owner
--              consent (spec §4.3, *Team sharing is out of M1*). Deliberately
--              no foreign key to `orgs`, the migration-066 rationale 094 and
--              095 both repeat: a deleted org must leave the row pointing at
--              an id nothing has (fail closed), never widen it.
-- level        'watch' | 'drive'. There is no 'own': `own` is the tier of
--              operations only the owner may perform (spec §4.3, invariant
--              5's table is its one authoritative definition) and no grant
--              ever reaches it. The CHECK is what makes a third level a
--              migration instead of an accident.
-- granted_by   the person who granted it — always the owner at the time, and
--              kept because a revoked row stays for the audit trail. No
--              foreign key to `people`, as above.
-- granted_at   when.
-- revoked_at   when it stopped applying. The row STAYS: the
--              `client_tokens.revoked_at` convention, so "A shared this with
--              B and took it back" remains answerable. Every read in
--              `store/session_grants.rs` filters on `revoked_at IS NULL`.
--
-- Note what is NOT here. There is no `level_changed_at`, no history of
-- levels, and no `granted_to_org_id` beside `person_id`: a grant is only
-- ever created, narrowed from 'drive' to 'watch', or revoked, so a level's
-- history is at most one step and the audit trail of a widening does not
-- exist because widening does not exist.
CREATE TABLE IF NOT EXISTS session_grants (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  session_id  INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  person_id   INTEGER,
  org_id      INTEGER,
  level       TEXT    NOT NULL CHECK (level IN ('watch', 'drive')),
  granted_by  INTEGER NOT NULL,
  granted_at  INTEGER NOT NULL,
  revoked_at  INTEGER,
  -- Exactly one of the two recipient columns is set. As schema rather than
  -- as a comment, so a row addressed to nobody — or to both a person and an
  -- org, which no reader knows how to interpret — cannot be written at all.
  CHECK ((person_id IS NULL) <> (org_id IS NULL))
);

-- ONE live grant per (session, recipient), which is what makes "re-granting
-- refuses rather than upgrades" enforceable: `Store::grant_session` does NOT
-- pre-check for an existing grant, it lets this index refuse the INSERT and
-- maps the violation to `E_EXISTS`. Narrowing is then the only way a level
-- ever moves, and it only moves one way.
--
-- The `COALESCE` is the whole point, and the obvious form enforces nothing:
-- `UNIQUE (session_id, person_id, org_id) WHERE revoked_at IS NULL` lets two
-- live grants to the same person on the same session BOTH insert, because
-- exactly one recipient column is NULL by design and SQLite treats NULLs as
-- distinct in a unique index. The repo's own precedent for a NULL-safe
-- uniqueness key is `ux_work_views_name ON work_views(COALESCE(owner_org, 0),
-- name)` (066); `0` is safe as the sentinel because both recipient columns
-- are `AUTOINCREMENT` ids, which start at 1.
--
-- Partial on `revoked_at IS NULL` so a revoked grant never blocks a fresh
-- one: re-sharing after a revoke is a new grant, at whatever level the owner
-- picks, which is a creation and not a widening.
CREATE UNIQUE INDEX IF NOT EXISTS idx_session_grants_live
  ON session_grants(session_id, COALESCE(person_id, 0), COALESCE(org_id, 0))
  WHERE revoked_at IS NULL;

-- `grants_for_person` is the HOT read: it runs once per request to build the
-- caller's scope, so it gets its own index rather than riding one led by
-- `session_id`, which cannot serve it. Partial twice over — live rows only,
-- and person recipients only — so it stays the size of the sharing that is
-- actually in force.
CREATE INDEX IF NOT EXISTS idx_session_grants_person
  ON session_grants(person_id)
  WHERE revoked_at IS NULL AND person_id IS NOT NULL;

-- The other direction: the Share sheet's "who can see this?", and the index
-- SQLite needs for the `ON DELETE CASCADE` above — without it, deleting a
-- session scans `session_grants` (migration 085 is the same lesson, learned
-- afterwards for `work_unlinks`). Not partial: the cascade looks for every
-- row of the session, revoked ones included.
CREATE INDEX IF NOT EXISTS idx_session_grants_session
  ON session_grants(session_id);

INSERT OR IGNORE INTO schema_version (version) VALUES (96);
