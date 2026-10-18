-- The operator's (UX agent's) client token belongs to the hub's personal
-- owner. `ensure_operator` minted it with no `person_id`, which
-- `Caller::view_scope` turns into a REFUSING scope: the operator saw no
-- session at all, not even its own, while its session row was the owner's
-- (`hub_personal_owner`). `ensure_operator` now binds the token as it mints
-- it; this heals a hub whose live token was minted before that, without
-- re-minting a token the operator's host already holds.
--
-- Only the live, person-less `ux-agent` row, and only when the hub has a
-- personal owner — the same person 098's backfill gives every device. The
-- name is `service::operator::OPERATOR_CLIENT_NAME`. Its mode is `full`, so
-- `set_client_person`'s refusal of `peer` / `updater` does not apply; the
-- filter is kept anyway so this can never say something the setter would
-- refuse.
--
-- 098's `auth_epoch` trigger fires on the change, so a caller a running
-- process cached is re-resolved.
UPDATE client_tokens
   SET person_id = (SELECT id FROM people WHERE is_personal_owner = 1)
 WHERE name = 'ux-agent'
   AND person_id IS NULL
   AND revoked_at IS NULL
   AND mode NOT IN ('peer', 'updater')
   AND EXISTS (SELECT 1 FROM people WHERE is_personal_owner = 1 AND disabled_at IS NULL);

INSERT OR IGNORE INTO schema_version (version) VALUES (195);
