-- Chat forms while they are written (redesign step 10.12, the ChatWizards
-- board's "Control builds it"): an agent sends the fleet.form/1 JSON it has
-- written so far with `ask { draft }`, and its session's chat draws the form
-- in (a skeleton field for each whole one) before `ask { form }` opens it.
-- One draft per session; opening the form, `ask { draft: "" }` or the tick
-- (10 minutes after the last write) removes it. Never answered, never kept.
-- draft       the text so far (at most 16 KiB, not yet valid JSON)
-- why         the agent's one sentence, shown as what it reads
-- updated_at  unix secs of the last write
CREATE TABLE IF NOT EXISTS form_drafts (
  session_id INTEGER PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
  draft      TEXT    NOT NULL,
  why        TEXT,
  updated_at INTEGER NOT NULL
);

INSERT OR IGNORE INTO schema_version (version) VALUES (153);
