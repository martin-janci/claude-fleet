-- Work graph M13.4e (decision D3 = yes, the PR remote link ONLY): when a
-- session linked `manual` / `started` to a Jira issue has a pull request,
-- fleet adds one remote link to that issue, idempotent by its `globalId`
-- (`fleet:pr:<url>`), only to the tracker of the link's own org, and only
-- for a tracker whose admin opted in (`trackers.settings.pr_remote_link`).
-- No transition, no worklog, no comment.
--
-- tracker_write_outbox
--              one row per (tracker, issue, op, global_id): a repeated
--              trigger is a no-op (`INSERT OR IGNORE` on the unique index),
--              a failed write stays `pending` with a backoff and is retried
--              by the sync tick, and a write the link no longer allows is
--              `cancelled`, never sent. `link_id` has no FK: the row is
--              history once the link ends. Swept under M12.3 retention
--              (`work.retention.write_outbox_days`): only settled rows
--              (done | failed | cancelled) whose link is gone or ended
--              before the window. A removed tracker takes its rows along.
-- work_links.host_decided
--              1 when the link's latest decision (link, confirm, start)
--              came from a per-host token: such a link never writes, since
--              every write is refused for per-host tokens. A later decision
--              by a person (the desktop, the master, a client) clears it.
CREATE TABLE IF NOT EXISTS tracker_write_outbox (
  id              INTEGER PRIMARY KEY AUTOINCREMENT,
  tracker_id      INTEGER NOT NULL REFERENCES trackers(id) ON DELETE CASCADE,
  issue_id        TEXT    NOT NULL,
  issue_key       TEXT,
  link_id         INTEGER,
  op              TEXT    NOT NULL,
  global_id       TEXT    NOT NULL,
  url             TEXT    NOT NULL,
  title           TEXT    NOT NULL,
  state           TEXT    NOT NULL DEFAULT 'pending',
  attempts        INTEGER NOT NULL DEFAULT 0,
  next_attempt_at INTEGER NOT NULL,
  last_error      TEXT,
  created_at      INTEGER NOT NULL,
  updated_at      INTEGER NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS ux_tracker_write_outbox_op
  ON tracker_write_outbox(tracker_id, issue_id, op, global_id);
CREATE INDEX IF NOT EXISTS idx_tracker_write_outbox_due
  ON tracker_write_outbox(tracker_id, next_attempt_at) WHERE state = 'pending';

ALTER TABLE work_links ADD COLUMN host_decided INTEGER NOT NULL DEFAULT 0;

INSERT OR IGNORE INTO schema_version (version) VALUES (61);
