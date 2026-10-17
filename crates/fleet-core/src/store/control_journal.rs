//! Control's journal and thread mutes (`control_journal`, `control_mutes`,
//! migration 195; Control review F10 and F12). What to write and when is
//! `service::control_journal`'s and `service::control_follow`'s; this is the
//! two tables.

use super::Store;
use rusqlite::{OptionalExtension, Result};
use serde::{Deserialize, Serialize};

/// Journal rows kept: older ones are dropped as new ones arrive.
pub const JOURNAL_KEEP: i64 = 200;
/// A journal entry is cut to this many characters.
pub const JOURNAL_TEXT_MAX: usize = 500;
/// What the person said is kept to this many characters, as a
/// conversation's `first_prompt` is.
pub const JOURNAL_SAID_MAX: usize = 200;
/// The kinds of a journal row.
pub const CONTROL_JOURNAL_KINDS: [&str; 3] = ["said", "ask", "note"];
/// The kinds of thread a mute names.
pub const MUTE_KINDS: [&str; 2] = ["session", "mission"];

/// One journal row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalEntry {
    pub id: i64,
    pub at: i64,
    /// `said` (the person to Control), `ask` (Control to the person, open
    /// until the person speaks) or `note` (Control's own).
    pub kind: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<i64>,
}

/// PURE: `text` on one line, cut to `max` characters with an ellipsis;
/// `None` when blank.
pub fn journal_text(text: &str, max: usize) -> Option<String> {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.is_empty() {
        return None;
    }
    if flat.chars().count() <= max {
        return Some(flat);
    }
    let cut: String = flat.chars().take(max.saturating_sub(1)).collect();
    Some(format!("{}…", cut.trim_end()))
}

fn entry(r: &rusqlite::Row<'_>) -> Result<JournalEntry> {
    Ok(JournalEntry {
        id: r.get(0)?,
        at: r.get(1)?,
        kind: r.get(2)?,
        text: r.get(3)?,
        resolved_at: r.get(4)?,
    })
}

impl Store {
    /// Write one journal row (`kind` one of [`CONTROL_JOURNAL_KINDS`]) and drop the
    /// ones past [`JOURNAL_KEEP`]; answers the row id. A `said` row resolves
    /// every open `ask`: the person spoke after it.
    pub fn insert_journal_entry(&self, kind: &str, text: &str, now: i64) -> Result<i64> {
        if kind == "said" {
            self.conn
                .prepare_cached(
                    "UPDATE control_journal SET resolved_at = ?1 \
                     WHERE kind = 'ask' AND resolved_at IS NULL",
                )?
                .execute([now])?;
        }
        self.conn
            .prepare_cached("INSERT INTO control_journal (at, kind, text) VALUES (?1, ?2, ?3)")?
            .execute(rusqlite::params![now, kind, text])?;
        let id = self.conn.last_insert_rowid();
        self.conn
            .prepare_cached("DELETE FROM control_journal WHERE id <= ?1")?
            .execute([id - JOURNAL_KEEP])?;
        Ok(id)
    }

    /// The newest `limit` rows of `kind`, newest first.
    pub fn journal_entries(&self, kind: &str, limit: i64) -> Result<Vec<JournalEntry>> {
        self.conn
            .prepare_cached(
                "SELECT id, at, kind, text, resolved_at FROM control_journal \
                 WHERE kind = ?1 ORDER BY id DESC LIMIT ?2",
            )?
            .query_map(rusqlite::params![kind, limit.clamp(1, JOURNAL_KEEP)], entry)?
            .collect()
    }

    /// The asks Control is still waiting on, oldest first.
    pub fn open_journal_asks(&self) -> Result<Vec<JournalEntry>> {
        self.conn
            .prepare_cached(
                "SELECT id, at, kind, text, resolved_at FROM control_journal \
                 WHERE kind = 'ask' AND resolved_at IS NULL ORDER BY id",
            )?
            .query_map([], entry)?
            .collect()
    }

    /// The last thing the person said to Control.
    pub fn last_journal_said(&self) -> Result<Option<JournalEntry>> {
        self.conn
            .prepare_cached(
                "SELECT id, at, kind, text, resolved_at FROM control_journal \
                 WHERE kind = 'said' ORDER BY id DESC LIMIT 1",
            )?
            .query_row([], entry)
            .optional()
    }

    /// Mute (`on`) or unmute a thread; answers whether anything changed.
    pub fn set_control_mute(&self, kind: &str, target_id: i64, on: bool, now: i64) -> Result<bool> {
        let n = if on {
            self.conn
                .prepare_cached(
                    "INSERT OR IGNORE INTO control_mutes (kind, target_id, at) VALUES (?1, ?2, ?3)",
                )?
                .execute(rusqlite::params![kind, target_id, now])?
        } else {
            self.conn
                .prepare_cached("DELETE FROM control_mutes WHERE kind = ?1 AND target_id = ?2")?
                .execute(rusqlite::params![kind, target_id])?
        };
        Ok(n > 0)
    }

    /// Every muted thread, as `(kind, id)`.
    pub fn control_mutes(&self) -> Result<Vec<(String, i64)>> {
        self.conn
            .prepare_cached("SELECT kind, target_id FROM control_mutes ORDER BY kind, target_id")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect()
    }

    /// The sessions Control may follow: those it started (`origin =
    /// 'operator'`) and those a receipt names, with the newest receipt's
    /// time (`0` for one it started and never briefed), `exclude` (its own
    /// row) left out. The follow decision itself is
    /// `service::control_follow`'s.
    pub fn control_follow_candidates(&self, exclude: Option<i64>) -> Result<Vec<(i64, i64)>> {
        self.conn
            .prepare_cached(
                "SELECT id, COALESCE((SELECT MAX(h.at) FROM control_handoffs h \
                                      WHERE h.session_id = s.id), 0) \
                 FROM sessions s \
                 WHERE s.id IS NOT ?1 AND (s.origin = 'operator' OR s.id IN \
                       (SELECT session_id FROM control_handoffs WHERE session_id IS NOT NULL))",
            )?
            .query_map([exclude], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect()
    }

    /// Whether `session_id` has an event of `kind` at or after `at` (unix
    /// seconds).
    pub fn session_event_at_or_after(&self, session_id: i64, kind: &str, at: i64) -> Result<bool> {
        Ok(self
            .conn
            .prepare_cached(
                "SELECT 1 FROM session_events WHERE session_id = ?1 AND kind = ?2 AND at >= ?3 \
                 LIMIT 1",
            )?
            .query_row(rusqlite::params![session_id, kind, at], |_| Ok(()))
            .optional()?
            .is_some())
    }

    /// Control's newest receipt naming `session_id`: when, and its preview.
    pub fn latest_brief_of(&self, session_id: i64) -> Result<Option<(i64, Option<String>)>> {
        self.conn
            .prepare_cached(
                "SELECT at, preview FROM control_handoffs WHERE session_id = ?1 \
                 ORDER BY id DESC LIMIT 1",
            )?
            .query_row([session_id], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()
    }

    /// Events of `kinds` on `session_id` strictly after event `after`,
    /// oldest first, as `(id, kind, detail)`.
    pub fn session_events_of_kinds_after(
        &self,
        session_id: i64,
        kinds: &[&str],
        after: i64,
    ) -> Result<Vec<(i64, String, Option<String>)>> {
        if kinds.is_empty() {
            return Ok(Vec::new());
        }
        let marks = (3..3 + kinds.len())
            .map(|i| format!("?{i}"))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT id, kind, detail FROM session_events \
             WHERE session_id = ?1 AND id > ?2 AND kind IN ({marks}) ORDER BY id LIMIT 100"
        );
        let mut params: Vec<rusqlite::types::Value> = vec![session_id.into(), after.into()];
        params.extend(
            kinds
                .iter()
                .map(|k| rusqlite::types::Value::from(k.to_string())),
        );
        let mut st = self.conn.prepare_cached(&sql)?;
        let rows = st
            .query_map(rusqlite::params_from_iter(params), |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })?
            .collect();
        rows
    }

    /// The sessions a receipt names that no longer have a row: removed,
    /// so never followed again.
    pub fn handed_off_sessions_gone(&self) -> Result<Vec<i64>> {
        self.conn
            .prepare_cached(
                "SELECT DISTINCT session_id FROM control_handoffs \
                 WHERE session_id IS NOT NULL \
                   AND session_id NOT IN (SELECT id FROM sessions)",
            )?
            .query_map([], |r| r.get(0))?
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn journal_text_is_one_line_and_cut() {
        assert_eq!(journal_text("  a\n b  ", 10), Some("a b".into()));
        assert_eq!(journal_text(" \n ", 10), None);
        let t = journal_text(&"x".repeat(50), 10).unwrap();
        assert_eq!(t.chars().count(), 10);
        assert!(t.ends_with('…'));
    }

    #[test]
    fn the_person_speaking_answers_every_open_ask_and_old_rows_go() {
        let s = Store::open_in_memory().unwrap();
        s.insert_journal_entry("ask", "start the review?", 10)
            .unwrap();
        s.insert_journal_entry("note", "watching #4", 11).unwrap();
        assert_eq!(s.open_journal_asks().unwrap().len(), 1);
        s.insert_journal_entry("said", "yes", 12).unwrap();
        assert!(s.open_journal_asks().unwrap().is_empty());
        assert_eq!(s.last_journal_said().unwrap().unwrap().text, "yes");
        s.insert_journal_entry("ask", "and merge?", 13).unwrap();
        assert_eq!(s.open_journal_asks().unwrap()[0].text, "and merge?");
        for i in 0..JOURNAL_KEEP {
            s.insert_journal_entry("note", &format!("n{i}"), 20 + i)
                .unwrap();
        }
        assert!(s.open_journal_asks().unwrap().is_empty(), "pruned");
        assert_eq!(
            s.journal_entries("note", 1000).unwrap().len() as i64,
            JOURNAL_KEEP
        );
    }

    #[test]
    fn a_mute_is_set_once_and_cleared() {
        let s = Store::open_in_memory().unwrap();
        assert!(s.set_control_mute("session", 3, true, 1).unwrap());
        assert!(!s.set_control_mute("session", 3, true, 2).unwrap());
        assert!(s.set_control_mute("mission", 3, true, 2).unwrap());
        assert_eq!(
            s.control_mutes().unwrap(),
            vec![("mission".into(), 3), ("session".into(), 3)]
        );
        assert!(s.set_control_mute("session", 3, false, 3).unwrap());
        assert!(!s.set_control_mute("session", 3, false, 3).unwrap());
        assert_eq!(s.control_mutes().unwrap().len(), 1);
    }
}
