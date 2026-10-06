-- Worktree rows named after a branch with a `/`. `new_session { new_worktree:
-- "feat/imports" }` and a fork into a new worktree used the branch verbatim
-- as the directory (`.worktrees/feat/imports`) and as the row's name, but a
-- worktree name is one path component wherever it is used: recreating or
-- repairing such a session failed with "worktree name must not contain a
-- path separator". New worktrees are created flat (`projects::
-- worktree_dir_name`, `feat-imports`); this renames the existing rows the
-- same way. The path and branch stay: the checkout is where it is, and
-- repair opens a remote host's checkout at its recorded path.
--
-- A name another row of the same project and host already has (or that an
-- older slash-named row takes first) gets the row id appended, so the rename
-- never trips UNIQUE(project_id, host_alias, name). Linked sessions take the
-- new name as their key, the name reconcile now derives from the row
-- (`worktree_key_for_host`). Idempotent: a second run finds no `/`.
CREATE TEMP TABLE IF NOT EXISTS worktree_renames_105 (
  id       INTEGER PRIMARY KEY,
  new_name TEXT NOT NULL
);
DELETE FROM worktree_renames_105;

INSERT INTO worktree_renames_105 (id, new_name)
SELECT w.id,
       CASE WHEN EXISTS (
              SELECT 1 FROM worktrees o
               WHERE o.project_id = w.project_id
                 AND o.host_alias = w.host_alias
                 AND o.id <> w.id
                 AND replace(o.name, '/', '-') = replace(w.name, '/', '-')
                 AND (instr(o.name, '/') = 0 OR o.id < w.id))
            THEN replace(w.name, '/', '-') || '-' || w.id
            ELSE replace(w.name, '/', '-')
       END
  FROM worktrees w
 WHERE instr(w.name, '/') > 0;

UPDATE sessions
   SET worktree_key = (SELECT r.new_name FROM worktree_renames_105 r
                        WHERE r.id = sessions.worktree_id)
 WHERE worktree_id IN (SELECT id FROM worktree_renames_105);

UPDATE worktrees
   SET name = (SELECT r.new_name FROM worktree_renames_105 r
                WHERE r.id = worktrees.id)
 WHERE id IN (SELECT id FROM worktree_renames_105);

DROP TABLE worktree_renames_105;

INSERT OR IGNORE INTO schema_version (version) VALUES (105);
