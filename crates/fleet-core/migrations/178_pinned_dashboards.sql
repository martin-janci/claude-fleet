-- Dashboards a person pinned from an agent's proposal (persona design,
-- step 6): kept on the hub, so every device of theirs shows them and the
-- agent reads what is pinned before it proposes a change. Until now they
-- lived in one desktop's local storage. The rules are in
-- `service::dashboards`.
--
-- pinned_dashboards
--   dash_id     the block's `id` ("dash.release-42"); one per person
--   person_id   whose it is (the caller's person); NULL = fleet's own (a
--               standalone desktop, a single-person hub)
--   title       the block's title, for a list without the spec
--   spec        the `dashboard` fleet.ui/1 block, JSON (chat-blocks.md)
--   why         the agent's one sentence on why, as the block gave it
--   device      the device it was pinned from; NULL = the hub's own desktop
CREATE TABLE IF NOT EXISTS pinned_dashboards (
  dash_id     TEXT    NOT NULL,
  person_id   INTEGER,
  title       TEXT    NOT NULL,
  spec        TEXT    NOT NULL,
  why         TEXT,
  device      TEXT,
  pinned_at   INTEGER NOT NULL,
  updated_at  INTEGER NOT NULL
);

CREATE UNIQUE INDEX IF NOT EXISTS pinned_dashboards_one
  ON pinned_dashboards (dash_id, COALESCE(person_id, 0));

INSERT OR IGNORE INTO schema_version (version) VALUES (178);
