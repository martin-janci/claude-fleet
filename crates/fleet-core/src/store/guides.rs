//! Guides stored at runtime (declarative pages, layout `guide`, migration
//! 088). The rules — what a guide may say, who proposes and who approves —
//! live in `service::guides`; this is the rows.

use super::{now_unix, Store};
use rusqlite::{OptionalExtension, Result};

/// Decided proposals (rejected, superseded, removed) are kept this long
/// (seconds), for the record. An approved guide is kept while it is live.
pub const DECIDED_GUIDE_KEEP_SECS: i64 = 30 * 24 * 60 * 60;

/// Superseded revisions of ONE guide id kept at once.
///
/// A new proposal for an id supersedes that id's pending row, and the row it
/// just superseded carries `decided_at = now`, so the 30-day retention above
/// never reaches it: N attempts at one id left N-1 rows of up to
/// `MAX_SPEC_BYTES` lying there for a month, from a per-host token that only
/// had to repeat itself. The record is worth keeping; every attempt is not.
pub const KEEP_SUPERSEDED_PER_GUIDE: usize = 3;

/// One proposed (or approved) guide.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GuideProposalRow {
    pub id: i64,
    pub at: i64,
    pub page_id: String,
    pub title: String,
    /// The `fleet.page/1` JSON, as validated when it was proposed.
    pub spec: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_detail: Option<String>,
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decided_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decided_by: Option<String>,
}

/// A proposal to insert.
pub struct NewGuideProposal<'a> {
    pub page_id: &'a str,
    pub title: &'a str,
    pub spec: &'a str,
    pub why: Option<&'a str>,
    pub source: &'a str,
    pub source_detail: Option<&'a str>,
}

const COLS: &str =
    "id, at, page_id, title, spec, why, source, source_detail, state, decided_at, decided_by";

fn row(r: &rusqlite::Row<'_>) -> Result<GuideProposalRow> {
    Ok(GuideProposalRow {
        id: r.get(0)?,
        at: r.get(1)?,
        page_id: r.get(2)?,
        title: r.get(3)?,
        spec: r.get(4)?,
        why: r.get(5)?,
        source: r.get(6)?,
        source_detail: r.get(7)?,
        state: r.get(8)?,
        decided_at: r.get(9)?,
        decided_by: r.get(10)?,
    })
}

impl Store {
    /// Insert a pending proposal, superseding a pending one for the same
    /// page id, and drop decided rows past [`DECIDED_GUIDE_KEEP_SECS`] plus
    /// this id's superseded revisions past [`KEEP_SUPERSEDED_PER_GUIDE`].
    pub fn insert_guide_proposal(&self, p: &NewGuideProposal<'_>) -> Result<GuideProposalRow> {
        let now = now_unix();
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE guide_proposals SET state = 'superseded', decided_at = ?2
             WHERE state = 'pending' AND page_id = ?1",
            rusqlite::params![p.page_id, now],
        )?;
        tx.execute(
            "DELETE FROM guide_proposals
             WHERE state IN ('rejected', 'superseded', 'removed') AND decided_at < ?1",
            rusqlite::params![now - DECIDED_GUIDE_KEEP_SECS],
        )?;
        tx.execute(
            "DELETE FROM guide_proposals
             WHERE state = 'superseded' AND page_id = ?1 AND id NOT IN (
                 SELECT id FROM guide_proposals
                  WHERE state = 'superseded' AND page_id = ?1
                  ORDER BY id DESC LIMIT ?2)",
            rusqlite::params![p.page_id, KEEP_SUPERSEDED_PER_GUIDE],
        )?;
        tx.execute(
            "INSERT INTO guide_proposals (at, page_id, title, spec, why, source, source_detail)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                now,
                p.page_id,
                p.title,
                p.spec,
                p.why,
                p.source,
                p.source_detail
            ],
        )?;
        let id = tx.last_insert_rowid();
        tx.commit()?;
        self.guide_proposal(id)
            .map(|r| r.expect("the row just inserted"))
    }

    pub fn guide_proposal(&self, id: i64) -> Result<Option<GuideProposalRow>> {
        self.conn
            .query_row(
                &format!("SELECT {COLS} FROM guide_proposals WHERE id = ?1"),
                [id],
                row,
            )
            .optional()
    }

    /// Rows in `state`, oldest first.
    pub fn guide_proposals_in(&self, state: &str) -> Result<Vec<GuideProposalRow>> {
        let mut st = self.conn.prepare(&format!(
            "SELECT {COLS} FROM guide_proposals WHERE state = ?1 ORDER BY id"
        ))?;
        let rows = st.query_map([state], row)?;
        rows.collect()
    }

    /// Approve a pending proposal, superseding the guide it replaces.
    /// `false` when it was not pending.
    pub fn approve_guide_proposal(&self, id: i64, by: &str) -> Result<bool> {
        let now = now_unix();
        let tx = self.conn.unchecked_transaction()?;
        let page_id: Option<String> = tx
            .query_row(
                "SELECT page_id FROM guide_proposals WHERE id = ?1 AND state = 'pending'",
                [id],
                |r| r.get(0),
            )
            .optional()?;
        let Some(page_id) = page_id else {
            return Ok(false);
        };
        tx.execute(
            "UPDATE guide_proposals SET state = 'superseded', decided_at = ?2
             WHERE state = 'approved' AND page_id = ?1",
            rusqlite::params![page_id, now],
        )?;
        tx.execute(
            "UPDATE guide_proposals SET state = 'approved', decided_at = ?2, decided_by = ?3
             WHERE id = ?1",
            rusqlite::params![id, now, by],
        )?;
        tx.commit()?;
        Ok(true)
    }

    /// Move a row from `from` to `to` (reject a pending one, remove an
    /// approved one). `false` when it was not in `from`.
    pub fn close_guide_proposal(&self, id: i64, from: &str, to: &str, by: &str) -> Result<bool> {
        let n = self.conn.execute(
            "UPDATE guide_proposals SET state = ?3, decided_at = ?4, decided_by = ?5
             WHERE id = ?1 AND state = ?2",
            rusqlite::params![id, from, to, now_unix(), by],
        )?;
        Ok(n == 1)
    }

    /// Overwrite a row's spec, as if a later build no longer reads it.
    #[cfg(test)]
    pub(crate) fn set_guide_spec_for_tests(&self, id: i64, spec: &str) {
        self.conn
            .execute(
                "UPDATE guide_proposals SET spec = ?1 WHERE id = ?2",
                rusqlite::params![spec, id],
            )
            .unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new<'a>(page_id: &'a str, spec: &'a str) -> NewGuideProposal<'a> {
        NewGuideProposal {
            page_id,
            title: "T",
            spec,
            why: Some("people ask"),
            source: "agent",
            source_detail: Some("host web-1"),
        }
    }

    #[test]
    fn one_pending_and_one_approved_guide_per_id() {
        let s = Store::open_in_memory().unwrap();
        let a = s.insert_guide_proposal(&new("guide.a", "1")).unwrap();
        let b = s.insert_guide_proposal(&new("guide.a", "2")).unwrap();
        assert_eq!(s.guide_proposal(a.id).unwrap().unwrap().state, "superseded");
        assert!(s.approve_guide_proposal(b.id, "person").unwrap());
        assert!(!s.approve_guide_proposal(b.id, "person").unwrap());

        let c = s.insert_guide_proposal(&new("guide.a", "3")).unwrap();
        // The approved guide stays live while its successor waits.
        assert_eq!(s.guide_proposals_in("approved").unwrap()[0].spec, "2");
        assert!(s.approve_guide_proposal(c.id, "person").unwrap());
        let live = s.guide_proposals_in("approved").unwrap();
        assert_eq!((live.len(), live[0].spec.as_str()), (1, "3"));
        assert_eq!(s.guide_proposal(b.id).unwrap().unwrap().state, "superseded");

        let d = s.insert_guide_proposal(&new("guide.b", "4")).unwrap();
        assert!(s
            .close_guide_proposal(d.id, "pending", "rejected", "person")
            .unwrap());
        assert!(!s
            .close_guide_proposal(d.id, "pending", "rejected", "person")
            .unwrap());
        assert!(s
            .close_guide_proposal(c.id, "approved", "removed", "person")
            .unwrap());
        assert!(s.guide_proposals_in("approved").unwrap().is_empty());
        assert!(s.guide_proposals_in("pending").unwrap().is_empty());
    }
}
