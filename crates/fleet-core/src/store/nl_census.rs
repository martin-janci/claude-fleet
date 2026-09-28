//! The texts the language census reads (Jev evaluation, D40): the rows a
//! decision model would be shown, each with the org it belongs to. Read
//! only; the texts never leave the process — `service::nl::census` turns
//! them into counts on the spot.

use super::Store;
use crate::ipc_error::IpcError;

/// The oldest schema the census understands: orgs (050) on top of the work
/// graph's `work_links.claude_session_id` (049).
pub const NL_CENSUS_MIN_SCHEMA: i64 = 50;

/// Journal kinds written by a person or by Claude, not by fleet's templates.
pub const NL_CENSUS_JOURNAL_KINDS: &[&str] = &[
    "conversation",
    "progress",
    "compact_summary",
    "outcome",
    "note",
    "handover",
    "summary",
];

/// A conversation's first prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CensusPrompt {
    pub org_id: Option<i64>,
    pub text: String,
    /// The session's `last_prompt`: what fleet itself last typed there, for
    /// the loop guard.
    pub last_prompt: Option<String>,
}

/// A work item's title and cached description.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CensusItem {
    pub org_id: Option<i64>,
    /// The tracker's provider, or the item's own `source` (`local`).
    pub provider: String,
    pub title: String,
    pub description: Option<String>,
}

/// A journal body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CensusJournal {
    pub org_id: Option<i64>,
    pub kind: String,
    pub body: String,
}

/// A confirmed link seen from both ends: the prompt that opened the
/// conversation it was decided in, and the title of the item it names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CensusPair {
    pub org_id: Option<i64>,
    pub prompt: String,
    pub last_prompt: Option<String>,
    pub title: String,
}

impl Store {
    /// First prompts of conversations started at or after `since`, newest
    /// first, at most `limit`.
    pub fn nl_census_prompts(&self, since: i64, limit: u32) -> Result<Vec<CensusPrompt>, IpcError> {
        let mut st = self.conn.prepare(concat!(
            "SELECT c.first_prompt, s.last_prompt, ",
            crate::session_org_sql!("s"),
            " FROM conversations c JOIN sessions s ON s.id = c.session_id \
             WHERE c.first_prompt IS NOT NULL AND c.started_at >= ?1 \
             ORDER BY c.started_at DESC, c.id DESC LIMIT ?2"
        ))?;
        let rows = st
            .query_map(rusqlite::params![since, limit], |r| {
                Ok(CensusPrompt {
                    text: r.get(0)?,
                    last_prompt: r.get(1)?,
                    org_id: r.get(2)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Work items written at or after `since`, newest first, at most `limit`.
    pub fn nl_census_items(&self, since: i64, limit: u32) -> Result<Vec<CensusItem>, IpcError> {
        let mut st = self.conn.prepare(
            "SELECT w.title, w.meta, COALESCE(t.provider, w.source), t.org_id \
             FROM work_items w LEFT JOIN trackers t ON t.id = w.tracker_id \
             WHERE w.updated_at >= ?1 \
             ORDER BY w.updated_at DESC, w.id DESC LIMIT ?2",
        )?;
        let rows = st
            .query_map(rusqlite::params![since, limit], |r| {
                let meta: Option<String> = r.get(1)?;
                Ok(CensusItem {
                    title: r.get(0)?,
                    description: super::ItemMeta::parse(meta.as_deref()).description,
                    provider: r.get(2)?,
                    org_id: r.get(3)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Journal bodies of [`NL_CENSUS_JOURNAL_KINDS`] not written by fleet,
    /// at or after `since`, newest first, at most `limit`. The org is the
    /// session's: through the row's participant, else through the newest
    /// conversation with its Claude id.
    pub fn nl_census_journal(
        &self,
        since: i64,
        limit: u32,
    ) -> Result<Vec<CensusJournal>, IpcError> {
        let kinds = NL_CENSUS_JOURNAL_KINDS
            .iter()
            .map(|k| format!("'{k}'"))
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            concat!(
                "SELECT j.kind, j.body, ",
                crate::session_org_sql!("s"),
                " FROM work_journal j \
                 LEFT JOIN participants p ON p.id = j.participant_id \
                 LEFT JOIN sessions s ON s.id = COALESCE(p.session_id, \
                   (SELECT c.session_id FROM conversations c \
                     WHERE c.claude_session_id = j.claude_session_id \
                     ORDER BY c.id DESC LIMIT 1)) \
                 WHERE j.body IS NOT NULL AND j.source <> 'fleet' \
                   AND j.kind IN ({}) AND j.at >= ?1 \
                 ORDER BY j.at DESC, j.id DESC LIMIT ?2"
            ),
            kinds
        );
        let mut st = self.conn.prepare(&sql)?;
        let rows = st
            .query_map(rusqlite::params![since, limit], |r| {
                Ok(CensusJournal {
                    kind: r.get(0)?,
                    body: r.get(1)?,
                    org_id: r.get(2)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Confirmed links decided at or after `since` whose conversation has a
    /// first prompt, newest first, at most `limit`. The org is the item's
    /// tracker's, else the session's.
    pub fn nl_census_pairs(&self, since: i64, limit: u32) -> Result<Vec<CensusPair>, IpcError> {
        let mut st = self.conn.prepare(concat!(
            "SELECT c.first_prompt, s.last_prompt, w.title, COALESCE(t.org_id, ",
            crate::session_org_sql!("s"),
            ") FROM work_links l \
             JOIN participants p ON p.id = l.participant_id \
             JOIN sessions s ON s.id = p.session_id \
             JOIN conversations c ON c.session_id = s.id \
                                 AND c.claude_session_id = l.claude_session_id \
             JOIN work_items w ON w.id = l.item_id \
             LEFT JOIN trackers t ON t.id = w.tracker_id \
             WHERE l.state = 'confirmed' AND c.first_prompt IS NOT NULL \
               AND COALESCE(l.decided_at, l.created_at) >= ?1 \
             ORDER BY COALESCE(l.decided_at, l.created_at) DESC, l.id DESC LIMIT ?2"
        ))?;
        let rows = st
            .query_map(rusqlite::params![since, limit], |r| {
                Ok(CensusPair {
                    prompt: r.get(0)?,
                    last_prompt: r.get(1)?,
                    title: r.get(2)?,
                    org_id: r.get(3)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}
