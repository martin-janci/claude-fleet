//! The PR reviewer's rows (migration 171): what it knows of each pull
//! request it watches, and the reviews it wrote. The reviewer is
//! `service::agents::review`.

use super::Store;
use rusqlite::{params, OptionalExtension, Row};

/// `review_prs.status` values.
pub const REVIEW_PR_STATUSES: [&str; 6] = [
    "pending_review",
    "reviewed",
    "changes_pushed",
    "skipped",
    "closed",
    "merged",
];

/// `review_prs.skip_reason` values.
pub const REVIEW_SKIP_REASONS: [&str; 4] = ["draft", "wip_title", "diff_too_large", "empty_diff"];

/// What the reviewer knows of one pull request.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ReviewPrRow {
    /// `owner/repo`.
    pub repo: String,
    pub number: i64,
    pub status: String,
    pub skip_reason: Option<String>,
    pub title: String,
    pub author: Option<String>,
    pub is_draft: bool,
    pub head_sha: String,
    pub base_ref: String,
    pub head_ref: String,
    /// When the reviewer first saw the current head.
    pub last_push_at: i64,
    pub last_reviewed_sha: Option<String>,
    pub last_reviewed_at: Option<i64>,
    pub skipped_at_sha: Option<String>,
    pub skip_diff_lines: Option<i64>,
    pub first_seen_at: i64,
    pub updated_at: i64,
    pub closed_at: Option<i64>,
    /// The posted review GitHub holds.
    pub review_id: Option<i64>,
    /// When the reviewer last saw that review on GitHub.
    pub review_verified_at: Option<i64>,
}

impl ReviewPrRow {
    /// Neither closed nor merged.
    pub fn is_open(&self) -> bool {
        self.status != "closed" && self.status != "merged"
    }
}

/// One review the reviewer wrote.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ReviewRecordRow {
    pub id: i64,
    pub repo: String,
    pub number: i64,
    pub sha: String,
    pub job_id: Option<i64>,
    pub verdict: String,
    /// The placed review, JSON.
    pub review: String,
    pub posted: bool,
    pub review_id: Option<i64>,
    pub created_at: i64,
}

/// A review to record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewReviewRecord<'a> {
    pub repo: &'a str,
    pub number: i64,
    pub sha: &'a str,
    pub job_id: Option<i64>,
    pub verdict: &'a str,
    /// The placed review, JSON.
    pub review: &'a str,
}

const PR_COLS: &str = "repo, number, status, skip_reason, title, author, is_draft, head_sha, \
                       base_ref, head_ref, last_push_at, last_reviewed_sha, last_reviewed_at, \
                       skipped_at_sha, skip_diff_lines, first_seen_at, updated_at, closed_at, \
                       review_id, review_verified_at";
const RECORD_COLS: &str =
    "id, repo, number, sha, job_id, verdict, review, posted, review_id, created_at";

fn pr_row(r: &Row<'_>) -> rusqlite::Result<ReviewPrRow> {
    Ok(ReviewPrRow {
        repo: r.get(0)?,
        number: r.get(1)?,
        status: r.get(2)?,
        skip_reason: r.get(3)?,
        title: r.get(4)?,
        author: r.get(5)?,
        is_draft: r.get(6)?,
        head_sha: r.get(7)?,
        base_ref: r.get(8)?,
        head_ref: r.get(9)?,
        last_push_at: r.get(10)?,
        last_reviewed_sha: r.get(11)?,
        last_reviewed_at: r.get(12)?,
        skipped_at_sha: r.get(13)?,
        skip_diff_lines: r.get(14)?,
        first_seen_at: r.get(15)?,
        updated_at: r.get(16)?,
        closed_at: r.get(17)?,
        review_id: r.get(18)?,
        review_verified_at: r.get(19)?,
    })
}

fn record_row(r: &Row<'_>) -> rusqlite::Result<ReviewRecordRow> {
    Ok(ReviewRecordRow {
        id: r.get(0)?,
        repo: r.get(1)?,
        number: r.get(2)?,
        sha: r.get(3)?,
        job_id: r.get(4)?,
        verdict: r.get(5)?,
        review: r.get(6)?,
        posted: r.get(7)?,
        review_id: r.get(8)?,
        created_at: r.get(9)?,
    })
}

impl Store {
    /// Write a pull request's row as given, replacing the one with its
    /// repo and number.
    pub fn put_review_pr(&self, row: &ReviewPrRow) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            &format!(
                "INSERT OR REPLACE INTO review_prs ({PR_COLS}) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, \
                         ?16, ?17, ?18, ?19, ?20)"
            ),
            params![
                row.repo,
                row.number,
                row.status,
                row.skip_reason,
                row.title,
                row.author,
                row.is_draft,
                row.head_sha,
                row.base_ref,
                row.head_ref,
                row.last_push_at,
                row.last_reviewed_sha,
                row.last_reviewed_at,
                row.skipped_at_sha,
                row.skip_diff_lines,
                row.first_seen_at,
                row.updated_at,
                row.closed_at,
                row.review_id,
                row.review_verified_at
            ],
        )?;
        Ok(())
    }

    /// One pull request's row.
    pub fn review_pr(
        &self,
        repo: &str,
        number: i64,
    ) -> Result<Option<ReviewPrRow>, rusqlite::Error> {
        self.conn
            .query_row(
                &format!("SELECT {PR_COLS} FROM review_prs WHERE repo = ?1 AND number = ?2"),
                params![repo, number],
                pr_row,
            )
            .optional()
    }

    /// Every row of one repo, by number.
    pub fn list_review_prs(&self, repo: &str) -> Result<Vec<ReviewPrRow>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {PR_COLS} FROM review_prs WHERE repo = ?1 ORDER BY number"
        ))?;
        let rows = stmt.query_map(params![repo], pr_row)?;
        rows.collect()
    }

    /// Record a review and mark its pull request reviewed at `sha`, in one
    /// transaction. Returns the record's id.
    pub fn record_review(&self, r: &NewReviewRecord<'_>, now: i64) -> Result<i64, rusqlite::Error> {
        let NewReviewRecord {
            repo,
            number,
            sha,
            job_id,
            verdict,
            review,
        } = *r;
        let tx = self.conn.unchecked_transaction()?;
        let id = tx.query_row(
            "INSERT INTO review_records (repo, number, sha, job_id, verdict, review, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) RETURNING id",
            params![repo, number, sha, job_id, verdict, review, now],
            |r| r.get(0),
        )?;
        tx.execute(
            "UPDATE review_prs SET status = CASE WHEN head_sha = ?3 THEN 'reviewed' ELSE status END, \
               skip_reason = CASE WHEN head_sha = ?3 THEN NULL ELSE skip_reason END, \
               last_reviewed_sha = ?3, last_reviewed_at = ?4, updated_at = ?4 \
             WHERE repo = ?1 AND number = ?2",
            params![repo, number, sha, now],
        )?;
        tx.commit()?;
        Ok(id)
    }

    /// Mark a pull request skipped at `sha` for a size reason (the diff is
    /// empty after exclusions, or over the limit), unless its head moved on.
    pub fn skip_review_pr(
        &self,
        repo: &str,
        number: i64,
        sha: &str,
        reason: &str,
        diff_lines: Option<i64>,
        now: i64,
    ) -> Result<(), rusqlite::Error> {
        if !REVIEW_SKIP_REASONS.contains(&reason) {
            return Err(rusqlite::Error::InvalidParameterName(format!(
                "skip reason {reason:?}"
            )));
        }
        self.conn.execute(
            "UPDATE review_prs SET status = 'skipped', skip_reason = ?4, skipped_at_sha = ?3, \
               skip_diff_lines = ?5, updated_at = ?6 \
             WHERE repo = ?1 AND number = ?2 AND head_sha = ?3",
            params![repo, number, sha, reason, diff_lines, now],
        )?;
        Ok(())
    }

    /// Record that GitHub took review record `record_id` as `review_id`:
    /// the record is posted, and its pull request remembers the review to
    /// verify it later.
    pub fn mark_review_posted(
        &self,
        record_id: i64,
        review_id: i64,
        now: i64,
    ) -> Result<(), rusqlite::Error> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE review_records SET posted = 1, review_id = ?2 WHERE id = ?1",
            params![record_id, review_id],
        )?;
        tx.execute(
            "UPDATE review_prs SET review_id = ?2, review_verified_at = ?3, updated_at = ?3 \
             WHERE (repo, number) = (SELECT repo, number FROM review_records WHERE id = ?1)",
            params![record_id, review_id, now],
        )?;
        tx.commit()
    }

    /// One review record by id.
    pub fn review_record(&self, id: i64) -> Result<Option<ReviewRecordRow>, rusqlite::Error> {
        self.conn
            .query_row(
                &format!("SELECT {RECORD_COLS} FROM review_records WHERE id = ?1"),
                params![id],
                record_row,
            )
            .optional()
    }

    /// The newest review of a pull request.
    pub fn last_review_record(
        &self,
        repo: &str,
        number: i64,
    ) -> Result<Option<ReviewRecordRow>, rusqlite::Error> {
        self.conn
            .query_row(
                &format!(
                    "SELECT {RECORD_COLS} FROM review_records WHERE repo = ?1 AND number = ?2 \
                     ORDER BY created_at DESC, id DESC LIMIT 1"
                ),
                params![repo, number],
                record_row,
            )
            .optional()
    }

    /// The newest reviews, of one repo or all.
    pub fn list_review_records(
        &self,
        repo: Option<&str>,
        limit: usize,
    ) -> Result<Vec<ReviewRecordRow>, rusqlite::Error> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {RECORD_COLS} FROM review_records WHERE ?1 IS NULL OR repo = ?1 \
             ORDER BY created_at DESC, id DESC LIMIT ?2"
        ))?;
        let rows = stmt.query_map(params![repo, limit as i64], record_row)?;
        rows.collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn pr(number: i64, sha: &str) -> ReviewPrRow {
        ReviewPrRow {
            repo: "o/r".into(),
            number,
            status: "pending_review".into(),
            skip_reason: None,
            title: "t".into(),
            author: Some("a".into()),
            is_draft: false,
            head_sha: sha.into(),
            base_ref: "main".into(),
            head_ref: "f".into(),
            last_push_at: 10,
            last_reviewed_sha: None,
            last_reviewed_at: None,
            skipped_at_sha: None,
            skip_diff_lines: None,
            first_seen_at: 10,
            updated_at: 10,
            closed_at: None,
            review_id: None,
            review_verified_at: None,
        }
    }

    #[test]
    fn a_review_marks_its_head_reviewed_and_a_moved_head_stays_pending() {
        let s = Store::open_in_memory().unwrap();
        s.put_review_pr(&pr(1, "aaa")).unwrap();
        s.put_review_pr(&pr(2, "bbb")).unwrap();
        let rec = |number, sha, verdict| NewReviewRecord {
            repo: "o/r",
            number,
            sha,
            job_id: None,
            verdict,
            review: "{}",
        };
        s.record_review(&rec(1, "aaa", "APPROVE"), 20).unwrap();
        s.record_review(&rec(2, "old", "COMMENT"), 20).unwrap();
        let one = s.review_pr("o/r", 1).unwrap().unwrap();
        assert_eq!(
            (one.status.as_str(), one.last_reviewed_sha.as_deref()),
            ("reviewed", Some("aaa"))
        );
        let two = s.review_pr("o/r", 2).unwrap().unwrap();
        assert_eq!(
            two.status, "pending_review",
            "the head moved past the review"
        );
        assert_eq!(
            s.last_review_record("o/r", 1).unwrap().unwrap().verdict,
            "APPROVE"
        );
        assert_eq!(s.list_review_records(Some("o/r"), 10).unwrap().len(), 2);
        assert!(s.record_review(&rec(1, "aaa", "LGTM"), 20).is_err());
    }

    #[test]
    fn a_size_skip_holds_only_for_its_head() {
        let s = Store::open_in_memory().unwrap();
        s.put_review_pr(&pr(1, "aaa")).unwrap();
        s.skip_review_pr("o/r", 1, "aaa", "diff_too_large", Some(9000), 30)
            .unwrap();
        let r = s.review_pr("o/r", 1).unwrap().unwrap();
        assert_eq!(r.status, "skipped");
        assert_eq!(r.skip_reason.as_deref(), Some("diff_too_large"));
        assert_eq!(
            (r.skipped_at_sha.as_deref(), r.skip_diff_lines),
            (Some("aaa"), Some(9000))
        );
        s.skip_review_pr("o/r", 1, "zzz", "empty_diff", None, 40)
            .unwrap();
        assert_eq!(
            s.review_pr("o/r", 1)
                .unwrap()
                .unwrap()
                .skip_reason
                .as_deref(),
            Some("diff_too_large")
        );
        assert!(s
            .skip_review_pr("o/r", 1, "aaa", "draft-ish", None, 0)
            .is_err());
        assert_eq!(s.list_review_prs("o/r").unwrap().len(), 1);
    }
}
