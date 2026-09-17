-- Wave 5 G2 (PROD-8): Handoff is replaced by move_session and Freeze is
-- descoped (docs/adr/0001-descope-freeze-ship-move.md). Neither ever had
-- code behind it:
--
-- handoffs                    created by 001, never written or read.
-- sessions.frozen_scrollback  created by 001, never written or read.
--
-- SQLite supports DROP COLUMN since 3.35; rusqlite 0.32 (libsqlite3-sys
-- 0.30, `bundled`) ships 3.46. The column is plain TEXT with no index,
-- constraint, view or trigger on it, so no table rebuild is needed.
-- 001 no longer creates `handoffs` (it re-runs on every launch), hence
-- IF EXISTS.

DROP TABLE IF EXISTS handoffs;

ALTER TABLE sessions DROP COLUMN frozen_scrollback;

INSERT OR IGNORE INTO schema_version (version) VALUES (22);
