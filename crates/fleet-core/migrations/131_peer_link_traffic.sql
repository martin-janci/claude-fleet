-- Orbit Fleet redesign step 11.5, the Federation page: how a hub link is
-- doing. All written by the exchange loops, never by a person.
--   latency_ms   round trip of this side's last exchange that did not park
--                (a dialer's; a listener cannot time the other side's call)
--   msgs_day     the UTC day msgs_today counts (unix day number)
--   msgs_today   messages carried either way on msgs_day
--   msgs_total   messages carried either way since this migration
ALTER TABLE peer_links ADD COLUMN latency_ms INTEGER;
ALTER TABLE peer_links ADD COLUMN msgs_day INTEGER;
ALTER TABLE peer_links ADD COLUMN msgs_today INTEGER NOT NULL DEFAULT 0;
ALTER TABLE peer_links ADD COLUMN msgs_total INTEGER NOT NULL DEFAULT 0;
INSERT OR IGNORE INTO schema_version (version) VALUES (131);
