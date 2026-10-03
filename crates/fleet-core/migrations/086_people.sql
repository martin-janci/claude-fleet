-- Multi-user M1, T1 (plan docs/superpowers/plans/2026-09-30-multi-user-m1-private-sessions.md,
-- spec docs/superpowers/specs/2026-09-30-multi-user-gap-analysis.md §4.1): a
-- hub learns who its PEOPLE are.
--
-- Until now every identity a hub knew was a token KIND — the master, a
-- paired client, a per-host token — so two colleagues paired to one hub were
-- indistinguishable from one person carrying two phones, and every session
-- either of them started was readable by both. `people` is the row a human
-- gets; a device points at one through `client_tokens.person_id`.
--
-- What this script does, statement by statement (named rather than counted:
-- `045_session_participants.sql`'s header says how MANY places insert a
-- session row, and has been stale ever since one of them moved):
--
--   CREATE TABLE people              one row per human this hub knows
--   idx_people_live_name             one LIVE person per name
--   idx_people_personal_owner        at most one personal owner, ever
--   ALTER TABLE client_tokens        `person_id`: whose device this is
--   auth_epoch_client_tokens_person  re-binding a device invalidates every
--                                    cached caller
--   INSERT INTO people               this hub's personal owner
--   UPDATE client_tokens             every live DEVICE becomes that owner's
--                                    (a peer hub link and an updater token
--                                    are not devices and are skipped)
--
-- name          what the operator types and what a grant is addressed to.
--               Validated like a client name (`store/people.rs`): one line,
--               nothing that could break the untrusted-content marker.
-- display_name  the profile layer (M2). NULL means "show `name`".
-- disabled_at   the person has left. The row STAYS — grants and (from
--               migration 087) sessions point at it, and re-attributing a
--               departed colleague's work is not an operation M1 has. It is
--               deliberately not part of the owner index below.
CREATE TABLE IF NOT EXISTS people (
  id                INTEGER PRIMARY KEY AUTOINCREMENT,
  name              TEXT    NOT NULL,
  display_name      TEXT,
  is_personal_owner INTEGER NOT NULL DEFAULT 0,
  created_at        INTEGER NOT NULL,
  disabled_at       INTEGER
);

-- One live person per name, exactly as `idx_client_tokens_live_name`
-- (migration 032) does it for devices: `name` is not UNIQUE outright,
-- because a disabled person keeps their row (grants and sessions still
-- point at it) and must not hold the name against a new colleague.
CREATE UNIQUE INDEX IF NOT EXISTS idx_people_live_name
  ON people(name) WHERE disabled_at IS NULL;

-- Exactly one personal owner, enforced by the schema rather than by a
-- convention about ids. The same partial-index device, on a flag rather
-- than on a NULL.
--
-- Neither the name (explicitly renameable — `store/people.rs`) nor "the
-- lowest id" (an accident of insertion order, and fragile the first time a
-- row is deleted and re-created) can be the key: both re-home or orphan the
-- fleet's owner silently. `disabled_at` is deliberately NOT in this index —
-- the hub's owner being disabled must not free the slot for a second one.
CREATE UNIQUE INDEX IF NOT EXISTS idx_people_personal_owner
  ON people(is_personal_owner) WHERE is_personal_owner = 1;

-- Whose device this is. Deliberately NO foreign key, the migration-066
-- rationale for `client_tokens.org_id`: deleting a person must leave the
-- token bound to an id nothing has (fail closed), never widen it to every
-- person or to none.
ALTER TABLE client_tokens ADD COLUMN person_id INTEGER;

-- A device's person is part of who it is: re-binding it must invalidate
-- every cached caller, exactly as a mode or an org change does.
--
-- A separate, narrow companion rather than an edit to migration 060: its
-- `auth_epoch_client_tokens_update` is a fixed `WHEN OLD.x IS NOT NEW.x`
-- list already installed in every live database, and SQLite has no
-- ALTER TRIGGER. `auth_epoch_client_tokens_org` (066) and
-- `auth_epoch_client_tokens_assets_admin` (074) are the precedents.
CREATE TRIGGER IF NOT EXISTS auth_epoch_client_tokens_person
  AFTER UPDATE OF person_id ON client_tokens
  WHEN OLD.person_id IS NOT NEW.person_id
BEGIN
  UPDATE auth_epoch SET epoch = epoch + 1 WHERE id = 1;
END;

-- This hub's personal owner. Every hub has at least one person and it is
-- created here, not by a registration flow: ownership of a session is then
-- never ambiguous, which is the property M1 actually needs.
--
-- `owner` is a PLACEHOLDER name, and that is all it is. A migration cannot
-- read a hostname; the only fleet-identity setting is `fleet.id`
-- (`service/address.rs`), a lazily minted UUID that is not a human's name;
-- and there is nobody to ask at upgrade time. It is renamed through
-- `store/people.rs` whenever the operator likes — which is exactly why
-- `Store::personal_owner_id()` keys on the flag and never on this string.
--
-- `WHERE NOT EXISTS` rather than a bare INSERT: the flagged row is unique by
-- index, so on any path that reaches this script with one already present
-- (the collision repair in `store/schema.rs`, a hand-migrated database) a
-- second insert would be a constraint error instead of a no-op.
INSERT INTO people (name, is_personal_owner, created_at)
SELECT 'owner', 1, CAST(strftime('%s', 'now') AS INTEGER)
WHERE NOT EXISTS (SELECT 1 FROM people WHERE is_personal_owner = 1);

-- No pre-existing device survives the upgrade person-less. A token whose
-- `person_id` is NULL is the `person: None` privilege level the spec (§2.7,
-- §4.1) calls out: a caller nobody owns, which a gate reading "is this the
-- personal owner?" must refuse and a scope builder cannot resolve. After
-- this, every live device on an upgraded hub belongs to the owner it already
-- effectively was — the upgrade widens nothing (rule 7).
--
-- Revoked rows are left alone: they resolve to no caller, and rewriting them
-- would only make the audit trail say something that was never true.
--
-- A `peer` row (a federated hub link) and an `updater` row (`fleet-updater`
-- acting for this hub) are left alone too, and for a stronger reason: neither
-- is a DEVICE and neither belongs to a human, so giving either one a person
-- would make another fleet — or the updater — a reader of that person's
-- private sessions the moment T3 keys session reads on `Caller::person`.
-- `TokenMode::is_single_purpose` is the same distinction in Rust
-- (`mcp/auth.rs`), and `store/people.rs::set_client_person` refuses exactly
-- these two modes; this filter is what keeps the migration and the setter
-- saying the same thing.
--
-- The trigger above is in place, so this UPDATE bumps `auth_epoch` once:
-- any caller a running process cached before the upgrade is re-resolved.
UPDATE client_tokens
   SET person_id = (SELECT id FROM people WHERE is_personal_owner = 1)
 WHERE person_id IS NULL
   AND revoked_at IS NULL
   AND mode NOT IN ('peer', 'updater');

INSERT OR IGNORE INTO schema_version (version) VALUES (86);
