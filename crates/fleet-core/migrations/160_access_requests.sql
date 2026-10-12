-- Gap plan G4.2 (M15, contract 16): a person a session is shared with asks
-- its owner for a wider level ("Ask Martin for Answer", the Watch board).
--
-- A request is NOT a grant and confers nothing. It is a note to the owner,
-- who answers it with the operations they already have: granting is the
-- owner revoking the asker's grant and sharing again at the asked level
-- (`store/session_grants.rs` invariant 3 — a live grant never widens), and
-- declining only stamps the row. The rules are in `store/access_requests.rs`.
--
-- session_id    the row asked about. `ON DELETE CASCADE`, for the reason
--               `session_grants` gives (100): `sessions.id` is a rowid alias
--               SQLite reuses, so a leaked request would re-attach to
--               somebody else's session.
-- person_id     who asks. No foreign key to `people`, 100's rationale.
-- level         the level asked for: 'answer' or 'drive'. Never 'watch' (the
--               narrowest level is what a share starts at, so there is nothing
--               to ask for) and never 'own' (no grant reaches it).
-- requested_at  when.
-- resolved_at   when it stopped being open; NULL while it waits.
-- resolution    'granted' | 'declined' | 'withdrawn', set with resolved_at.
-- resolved_by   the person who resolved it (the owner, or the asker for a
--               withdrawal).
--
-- The row stays after it is resolved: "who asked, and what was the answer"
-- remains answerable, and a decline's time is what holds a repeat ask back.
CREATE TABLE IF NOT EXISTS access_requests (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  session_id   INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  person_id    INTEGER NOT NULL,
  level        TEXT    NOT NULL CHECK (level IN ('answer', 'drive')),
  requested_at INTEGER NOT NULL,
  resolved_at  INTEGER,
  resolution   TEXT CHECK (resolution IN ('granted', 'declined', 'withdrawn')),
  resolved_by  INTEGER,
  CHECK ((resolved_at IS NULL) = (resolution IS NULL))
);

-- One open request per (session, person): asking again while one waits is
-- `E_EXISTS`, so an owner never sees the same ask twice.
CREATE UNIQUE INDEX IF NOT EXISTS idx_access_requests_open
  ON access_requests(session_id, person_id)
  WHERE resolved_at IS NULL;

-- The cascade's index, and the owner's per-session read. Not partial: the
-- cascade looks for every row of the session.
CREATE INDEX IF NOT EXISTS idx_access_requests_session
  ON access_requests(session_id);

-- The asker's own open requests (`my_grants`).
CREATE INDEX IF NOT EXISTS idx_access_requests_person
  ON access_requests(person_id)
  WHERE resolved_at IS NULL;

INSERT OR IGNORE INTO schema_version (version) VALUES (160);
