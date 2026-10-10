//! Pull requests (`pull_requests`, redesign step 6.4): every PR a session's
//! branch has had, upserted by reconcile from the `gh pr view` probe it
//! already runs (`service::outcome`), and kept once the session is gone.
//! Read by `prs { action: list }` (`service::prs`).

use super::Store;
use crate::service::outcome::PrEvidence;
use rusqlite::{params, OptionalExtension, Result};
use serde::{Deserialize, Serialize};

/// One pull request, as Work › Pull requests lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequestRow {
    pub id: i64,
    pub url: String,
    /// `owner/name`, from the URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_ref: Option<String>,
    /// OPEN | CLOSED | MERGED.
    pub state: String,
    #[serde(default)]
    pub draft: bool,
    /// passing | failing | pending; `None` without checks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ci_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_decision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge_state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merged_at: Option<i64>,
    /// The session that opened it (the first one seen on it). It may be gone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_alias: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<i64>,
    pub first_seen_at: i64,
    pub updated_at: i64,
    /// The diffstat, lines added and removed (gap plan G3.10). Not stored:
    /// `prs { list }` reads it from the opening session's latest probe of
    /// this PR (`sessions.pr_evidence`), so it is absent once that session
    /// is gone or on another PR.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additions: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deletions: Option<u32>,
}

/// Timeline kinds a change of a PR row writes (M15 step G2.4), on the
/// session that opened it, its URL as the detail: what an event routine on
/// a pull request fires on (`service::routines::EVENTS`).
pub const PR_EVENT_REVIEW: &str = "pr_review";
pub const PR_EVENT_CI_FAILED: &str = "pr_ci_failed";
pub const PR_EVENT_CI_PASSED: &str = "pr_ci_passed";
pub const PR_EVENT_MERGED: &str = "pr_merged";

/// What one change of a PR row is, as timeline kinds: a review decision
/// (approved, changes requested) it did not have, checks that turned
/// failing or passing, and a merge. A PR seen for the first time is news
/// for its review and checks only while open; one first seen merged or
/// closed is history, and fires nothing.
pub fn pr_events(before: Option<&PullRequestRow>, after: &PullRequestRow) -> Vec<&'static str> {
    let mut out = Vec::new();
    if before.is_none() && after.state != "OPEN" {
        return out;
    }
    let review = after.review_decision.as_deref();
    if matches!(review, Some("APPROVED" | "CHANGES_REQUESTED"))
        && before.and_then(|b| b.review_decision.as_deref()) != review
    {
        out.push(PR_EVENT_REVIEW);
    }
    let ci = after.ci_status.as_deref();
    if before.and_then(|b| b.ci_status.as_deref()) != ci {
        match ci {
            Some("failing") => out.push(PR_EVENT_CI_FAILED),
            Some("passing") => out.push(PR_EVENT_CI_PASSED),
            _ => {}
        }
    }
    if after.state == "MERGED" && before.is_some_and(|b| b.state != "MERGED") {
        out.push(PR_EVENT_MERGED);
    }
    out
}

/// The session a reconcile pass saw on a PR.
#[derive(Debug, Clone, Copy)]
pub struct PrSeenBy<'a> {
    pub session_id: i64,
    pub session_name: &'a str,
    pub host_alias: &'a str,
    pub project_id: Option<i64>,
}

const COLS: &str = "id, url, repo, number, title, head_ref, state, draft, ci_status, \
                    review_decision, merge_state, merged_at, session_id, session_name, \
                    host_alias, project_id, first_seen_at, updated_at";

fn row(r: &rusqlite::Row<'_>) -> Result<PullRequestRow> {
    Ok(PullRequestRow {
        id: r.get(0)?,
        url: r.get(1)?,
        repo: r.get(2)?,
        number: r.get(3)?,
        title: r.get(4)?,
        head_ref: r.get(5)?,
        state: r.get(6)?,
        draft: r.get::<_, i64>(7)? != 0,
        ci_status: r.get(8)?,
        review_decision: r.get(9)?,
        merge_state: r.get(10)?,
        merged_at: r.get(11)?,
        session_id: r.get(12)?,
        session_name: r.get(13)?,
        host_alias: r.get(14)?,
        project_id: r.get(15)?,
        first_seen_at: r.get(16)?,
        updated_at: r.get(17)?,
        additions: None,
        deletions: None,
    })
}

/// `https://<host>/<owner>/<name>/pull/<n>` → (`owner/name`, n). Anything
/// else answers `(None, None)`: the URL is still the row's identity.
pub fn repo_and_number(url: &str) -> (Option<String>, Option<i64>) {
    let path = url
        .strip_prefix("https://")
        .and_then(|rest| rest.split_once('/'))
        .map(|(_, p)| p)
        .unwrap_or("");
    let parts: Vec<&str> = path.trim_end_matches('/').split('/').collect();
    match parts.as_slice() {
        [owner, name, "pull", n, ..] if !owner.is_empty() && !name.is_empty() => (
            Some(format!("{owner}/{name}")),
            n.parse::<i64>().ok().filter(|n| *n > 0),
        ),
        _ => (None, None),
    }
}

impl Store {
    /// Record what one probe saw of a PR. The first session seen on it stays
    /// its opener; a field this reading lacks (an older `gh` answering only
    /// the basic fields) keeps its stored value. A PR first seen merged gets
    /// gh's `mergedAt`, else this pass's time, once. Answers whether the row
    /// changed.
    pub(crate) fn upsert_pull_request_in_tx(
        tx: &rusqlite::Connection,
        url: &str,
        ci_status: Option<&str>,
        ev: Option<&PrEvidence>,
        by: PrSeenBy<'_>,
        now: i64,
    ) -> Result<bool> {
        let (repo, number) = repo_and_number(url);
        let before = tx
            .query_row(
                &format!("SELECT {COLS} FROM pull_requests WHERE url = ?1"),
                params![url],
                row,
            )
            .optional()?;
        let state = ev.and_then(|e| e.state.as_deref());
        let merged_at = ev
            .and_then(|e| e.merged_at)
            .or_else(|| (state == Some("MERGED")).then_some(now));
        tx.execute(
            "INSERT INTO pull_requests (url, repo, number, title, head_ref, state, draft, \
               ci_status, review_decision, merge_state, merged_at, session_id, session_name, \
               host_alias, project_id, first_seen_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, COALESCE(?6, 'OPEN'), ?7, ?8, ?9, ?10, ?11, ?12, ?13, \
               ?14, ?15, ?16, ?16) \
             ON CONFLICT(url) DO UPDATE SET \
               title = COALESCE(excluded.title, title), \
               head_ref = COALESCE(excluded.head_ref, head_ref), \
               state = COALESCE(?6, state), \
               draft = CASE WHEN ?17 THEN excluded.draft ELSE draft END, \
               ci_status = excluded.ci_status, \
               review_decision = CASE WHEN ?17 THEN excluded.review_decision \
                 ELSE review_decision END, \
               merge_state = CASE WHEN ?17 THEN excluded.merge_state ELSE merge_state END, \
               merged_at = CASE WHEN COALESCE(?6, state) = 'MERGED' \
                 THEN COALESCE(merged_at, excluded.merged_at) ELSE NULL END, \
               session_id = COALESCE(session_id, excluded.session_id), \
               session_name = COALESCE(session_name, excluded.session_name), \
               host_alias = COALESCE(host_alias, excluded.host_alias), \
               project_id = COALESCE(project_id, excluded.project_id)",
            params![
                url,
                repo,
                number,
                ev.and_then(|e| e.title.as_deref()),
                ev.and_then(|e| e.head_ref.as_deref()),
                state,
                ev.is_some_and(|e| e.draft),
                ci_status,
                ev.and_then(|e| e.review_decision.as_deref()),
                ev.and_then(|e| e.merge_state.as_deref()),
                merged_at,
                by.session_id,
                by.session_name,
                by.host_alias,
                by.project_id,
                now,
                ev.is_some(),
            ],
        )?;
        let after = tx
            .query_row(
                &format!("SELECT {COLS} FROM pull_requests WHERE url = ?1"),
                params![url],
                row,
            )
            .optional()?;
        let changed = match (&before, &after) {
            (Some(b), Some(a)) => {
                let mut a = a.clone();
                a.updated_at = b.updated_at;
                &a != b
            }
            _ => true,
        };
        if changed && before.is_some() {
            tx.execute(
                "UPDATE pull_requests SET updated_at = ?2 WHERE url = ?1",
                params![url, now],
            )?;
        }
        if let Some(after) = after.as_ref().filter(|_| changed) {
            Self::write_pr_events_in_tx(tx, before.as_ref(), after, by.session_id, now)?;
        }
        Ok(changed)
    }

    /// Write [`pr_events`] of one change on the timeline of the session that
    /// opened the PR, or of the session that saw it when the opener's row
    /// is gone. Quiet: no bus frame, a routine's tick reads them.
    fn write_pr_events_in_tx(
        tx: &rusqlite::Connection,
        before: Option<&PullRequestRow>,
        after: &PullRequestRow,
        seen_by: i64,
        now: i64,
    ) -> Result<()> {
        let kinds = pr_events(before, after);
        if kinds.is_empty() {
            return Ok(());
        }
        let opener = match after.session_id {
            Some(id) => tx
                .query_row("SELECT id FROM sessions WHERE id = ?1", [id], |r| {
                    r.get::<_, i64>(0)
                })
                .optional()?,
            None => None,
        };
        let session = opener.unwrap_or(seen_by);
        for kind in kinds {
            tx.execute(
                "INSERT INTO session_events (session_id, at, kind, detail) VALUES (?1, ?2, ?3, ?4)",
                params![session, now, kind, after.url],
            )?;
        }
        Ok(())
    }

    /// The pull request at `url`.
    pub fn pull_request_by_url(&self, url: &str) -> Result<Option<PullRequestRow>> {
        self.conn
            .query_row(
                &format!("SELECT {COLS} FROM pull_requests WHERE url = ?1"),
                params![url],
                row,
            )
            .optional()
    }

    /// Every pull request, the most recently changed first. `states` narrows
    /// to those states (OPEN, CLOSED, MERGED); empty is all of them. The
    /// filter is SQL, so one state walks `idx_pull_requests_state_updated`
    /// instead of reading every PR ever seen.
    pub fn list_pull_requests(&self, states: &[&str]) -> Result<Vec<PullRequestRow>> {
        let filter = if states.is_empty() {
            String::new()
        } else {
            format!("WHERE state IN ({}) ", vec!["?"; states.len()].join(", "))
        };
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT {COLS} FROM pull_requests {filter}ORDER BY updated_at DESC, id DESC"
        ))?;
        let rows = stmt.query_map(rusqlite::params_from_iter(states), row)?;
        rows.collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(state: &str) -> PrEvidence {
        PrEvidence {
            state: Some(state.into()),
            title: Some("Fix login".into()),
            head_ref: Some("fix/login".into()),
            review_decision: Some("APPROVED".into()),
            ..PrEvidence::default()
        }
    }

    const URL: &str = "https://github.com/o/r/pull/42";

    fn by(id: i64, name: &str) -> PrSeenBy<'_> {
        PrSeenBy {
            session_id: id,
            session_name: name,
            host_alias: "trn",
            project_id: Some(3),
        }
    }

    #[test]
    fn the_url_names_the_repo_and_number() {
        assert_eq!(repo_and_number(URL), (Some("o/r".into()), Some(42)));
        assert_eq!(
            repo_and_number("https://ghe.example.com/a/b/pull/7/files"),
            (Some("a/b".into()), Some(7))
        );
        assert_eq!(
            repo_and_number("https://github.com/o/r/issues/4"),
            (None, None)
        );
        assert_eq!(repo_and_number("not a url"), (None, None));
    }

    #[test]
    fn a_pr_is_recorded_once_and_keeps_the_session_that_opened_it() {
        let s = Store::open_in_memory().unwrap();
        assert!(Store::upsert_pull_request_in_tx(
            &s.conn,
            URL,
            Some("pending"),
            Some(&ev("OPEN")),
            by(1, "api"),
            100
        )
        .unwrap());
        // The same reading again changes nothing.
        assert!(!Store::upsert_pull_request_in_tx(
            &s.conn,
            URL,
            Some("pending"),
            Some(&ev("OPEN")),
            by(1, "api"),
            160
        )
        .unwrap());
        // A second session on the branch is not its opener.
        Store::upsert_pull_request_in_tx(
            &s.conn,
            URL,
            Some("passing"),
            Some(&ev("OPEN")),
            by(2, "review"),
            200,
        )
        .unwrap();
        let rows = s.list_pull_requests(&[]).unwrap();
        assert_eq!(rows.len(), 1);
        let r = &rows[0];
        assert_eq!(
            (r.session_id, r.session_name.as_deref()),
            (Some(1), Some("api"))
        );
        assert_eq!((r.repo.as_deref(), r.number), (Some("o/r"), Some(42)));
        assert_eq!(r.ci_status.as_deref(), Some("passing"));
        assert_eq!((r.first_seen_at, r.updated_at), (100, 200));
        assert_eq!(r.merged_at, None);
    }

    #[test]
    fn a_basic_reading_keeps_what_it_cannot_see() {
        let s = Store::open_in_memory().unwrap();
        Store::upsert_pull_request_in_tx(&s.conn, URL, None, Some(&ev("OPEN")), by(1, "api"), 100)
            .unwrap();
        // An older gh: url and checks only.
        Store::upsert_pull_request_in_tx(&s.conn, URL, Some("failing"), None, by(1, "api"), 200)
            .unwrap();
        let r = &s.list_pull_requests(&[]).unwrap()[0];
        assert_eq!(r.title.as_deref(), Some("Fix login"));
        assert_eq!(r.review_decision.as_deref(), Some("APPROVED"));
        assert_eq!(r.state, "OPEN");
        assert_eq!(r.ci_status.as_deref(), Some("failing"));
    }

    #[test]
    fn states_filter_the_list() {
        let s = Store::open_in_memory().unwrap();
        Store::upsert_pull_request_in_tx(&s.conn, URL, None, Some(&ev("OPEN")), by(1, "a"), 100)
            .unwrap();
        Store::upsert_pull_request_in_tx(
            &s.conn,
            "https://github.com/o/r/pull/43",
            None,
            Some(&ev("MERGED")),
            by(2, "b"),
            200,
        )
        .unwrap();
        let merged = s.list_pull_requests(&["MERGED"]).unwrap();
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].number, Some(43));
        assert_eq!(
            merged[0].merged_at,
            Some(200),
            "first seen merged: this pass's time"
        );
        let all = s.list_pull_requests(&[]).unwrap();
        assert_eq!(
            all.iter().map(|r| r.number).collect::<Vec<_>>(),
            vec![Some(43), Some(42)]
        );
        let both = s.list_pull_requests(&["OPEN", "MERGED"]).unwrap();
        assert_eq!(both, all);
        assert!(s.list_pull_requests(&["CLOSED"]).unwrap().is_empty());
    }

    /// One state reads its rows through the (state, updated_at) index.
    #[test]
    fn one_state_walks_the_state_index() {
        let s = Store::open_in_memory().unwrap();
        let plan: Vec<String> = s
            .conn
            .prepare(&format!(
                "EXPLAIN QUERY PLAN SELECT {COLS} FROM pull_requests WHERE state IN (?) \
                 ORDER BY updated_at DESC, id DESC"
            ))
            .unwrap()
            .query_map(["OPEN"], |r| r.get::<_, String>(3))
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap();
        assert!(
            plan.iter()
                .any(|d| d.contains("idx_pull_requests_state_updated")),
            "{plan:?}"
        );
    }
}
