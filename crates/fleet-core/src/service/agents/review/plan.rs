//! PURE: what one poll of a repo's open pull requests changes, ported from
//! claude-code-reviewer's `evaluateTransitions` (`reviewer.ts`) and
//! `shouldReview` (`state/decisions.ts`), with their scenarios as tests.
//!
//! Per pull request the poll yields its row and, when it needs a review, a
//! job. A job's key is `gh:<repo>#<n>@<head>`, so a head is reviewed once:
//! the store writes a key it already has as nothing. A review waits for the
//! head to settle (`debounce_secs` from the first time the poll saw it).
//!
//! | what the poll sees                      | becomes                      |
//! |-----------------------------------------|------------------------------|
//! | a new pull request                      | `pending_review`             |
//! | a new head after a review               | `changes_pushed`             |
//! | a draft / a WIP title (when skipped)    | `skipped` (`draft`, `wip_title`) |
//! | that condition gone                     | `pending_review` (or `reviewed` at a reviewed head) |
//! | a size skip, and the head moved         | `pending_review`             |
//! | a closed or merged one, open again      | as a new head would          |
//!
//! A row the store holds as open that the list no longer has is closed or
//! merged; the poll asks GitHub which, at most [`MAX_CLOSED_CHECKS`] a pass.

use crate::store::{NewAgentJob, ReviewPrRow};
use std::collections::HashMap;

/// Rows a poll asks GitHub about (closed or merged?) per pass.
pub const MAX_CLOSED_CHECKS: usize = 5;

/// One open pull request as `gh pr list` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenPr {
    pub number: i64,
    pub title: String,
    pub head_sha: String,
    pub is_draft: bool,
    pub base_ref: String,
    pub head_ref: String,
    pub author: Option<String>,
}

/// The rule's options the planner reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gate {
    pub skip_drafts: bool,
    pub skip_wip: bool,
    pub debounce_secs: i64,
}

/// What a poll changes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PollPlan {
    /// Rows to write: new or changed.
    pub rows: Vec<ReviewPrRow>,
    /// Reviews due (or due once their debounce has passed).
    pub jobs: Vec<NewAgentJob>,
    /// Rows held open that the list no longer has.
    pub to_check: Vec<i64>,
}

/// PURE: a work-in-progress title: `WIP`, `[WIP]`, `wip:` … as a word at
/// the start, any case. The original matched any title starting with
/// `wip`, "wipe" included.
pub fn is_wip(title: &str) -> bool {
    let t = title.trim_start();
    let t = t.strip_prefix('[').unwrap_or(t);
    t.get(..3).is_some_and(|w| w.eq_ignore_ascii_case("wip"))
        && t[3..]
            .chars()
            .next()
            .is_none_or(|c| !c.is_alphanumeric() && c != '_')
}

/// The subject of a pull request's jobs.
pub fn subject(repo: &str, number: i64) -> String {
    format!("gh:{repo}#{number}")
}

/// The key that makes one head reviewed once.
pub fn dedup_key(repo: &str, number: i64, sha: &str) -> String {
    format!("{}@{sha}", subject(repo, number))
}

/// PURE: the head a job's key names (`…@<sha>`).
pub fn sha_of_key(key: &str) -> Option<&str> {
    key.rsplit_once('@')
        .map(|(_, sha)| sha)
        .filter(|s| !s.is_empty())
}

/// PURE: `gh:<owner>/<repo>#<n>` as `(owner/repo, n)`.
pub fn parse_subject(subject: &str) -> Option<(&str, i64)> {
    let (repo, n) = subject.strip_prefix("gh:")?.rsplit_once('#')?;
    let n: i64 = n.parse().ok()?;
    (n > 0 && repo.contains('/')).then_some((repo, n))
}

fn resting_status(row: &ReviewPrRow) -> &'static str {
    if row.last_reviewed_sha.as_deref() == Some(row.head_sha.as_str()) {
        "reviewed"
    } else if row.last_reviewed_sha.is_some() {
        "changes_pushed"
    } else {
        "pending_review"
    }
}

/// PURE: the plan for one poll of `repo` (`owner/repo`).
pub fn plan(
    rule_id: i64,
    repo: &str,
    open: &[OpenPr],
    existing: &[ReviewPrRow],
    gate: Gate,
    now: i64,
) -> PollPlan {
    let by_number: HashMap<i64, &ReviewPrRow> = existing.iter().map(|r| (r.number, r)).collect();
    let mut out = PollPlan::default();
    for pr in open {
        let before = by_number.get(&pr.number).copied();
        let mut row = before.cloned().unwrap_or_else(|| ReviewPrRow {
            repo: repo.to_string(),
            number: pr.number,
            status: "pending_review".into(),
            skip_reason: None,
            title: String::new(),
            author: None,
            is_draft: false,
            head_sha: pr.head_sha.clone(),
            base_ref: String::new(),
            head_ref: String::new(),
            last_push_at: now,
            last_reviewed_sha: None,
            last_reviewed_at: None,
            skipped_at_sha: None,
            skip_diff_lines: None,
            first_seen_at: now,
            updated_at: now,
            closed_at: None,
        });
        row.title = pr.title.clone();
        row.author = pr.author.clone();
        row.is_draft = pr.is_draft;
        row.base_ref = pr.base_ref.clone();
        row.head_ref = pr.head_ref.clone();
        if row.head_sha != pr.head_sha {
            row.head_sha = pr.head_sha.clone();
            row.last_push_at = now;
            if row.status == "reviewed" {
                row.status = "changes_pushed".into();
            }
        }
        if !row.is_open() {
            // Reopened.
            row.status = resting_status(&row).into();
            row.closed_at = None;
        }
        let held = if gate.skip_drafts && pr.is_draft {
            Some("draft")
        } else if gate.skip_wip && is_wip(&pr.title) {
            Some("wip_title")
        } else {
            None
        };
        if let Some(reason) = held {
            row.status = "skipped".into();
            row.skip_reason = Some(reason.into());
        } else if row.status == "skipped" {
            let cleared = match row.skip_reason.as_deref() {
                Some("diff_too_large" | "empty_diff") => {
                    row.skipped_at_sha.as_deref() != Some(row.head_sha.as_str())
                }
                _ => true,
            };
            if cleared {
                row.status = resting_status(&row).into();
                row.skip_reason = None;
            }
        }
        if matches!(row.status.as_str(), "pending_review" | "changes_pushed")
            && row.last_reviewed_sha.as_deref() != Some(row.head_sha.as_str())
        {
            out.jobs.push(NewAgentJob {
                rule_id,
                agent: "review".into(),
                subject: subject(repo, pr.number),
                trigger: if row.last_reviewed_sha.is_some() {
                    "pushed".into()
                } else {
                    "opened".into()
                },
                dedup_key: dedup_key(repo, pr.number, &row.head_sha),
                not_before: row.last_push_at.saturating_add(gate.debounce_secs),
            });
        }
        let changed = match before {
            None => true,
            Some(b) => {
                let mut b = b.clone();
                b.updated_at = row.updated_at;
                b != row
            }
        };
        if changed {
            row.updated_at = now;
            out.rows.push(row);
        }
    }
    let open_now: std::collections::HashSet<i64> = open.iter().map(|p| p.number).collect();
    out.to_check = existing
        .iter()
        .filter(|r| r.is_open() && !open_now.contains(&r.number))
        .map(|r| r.number)
        .take(MAX_CLOSED_CHECKS)
        .collect();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const GATE: Gate = Gate {
        skip_drafts: true,
        skip_wip: true,
        debounce_secs: 60,
    };

    fn open(n: i64, sha: &str) -> OpenPr {
        OpenPr {
            number: n,
            title: "Add a thing".into(),
            head_sha: sha.into(),
            is_draft: false,
            base_ref: "main".into(),
            head_ref: "feat".into(),
            author: Some("ann".into()),
        }
    }

    /// Apply a plan's rows over `rows`, as the poller writes them.
    fn apply(rows: &mut Vec<ReviewPrRow>, p: &PollPlan) {
        for r in &p.rows {
            rows.retain(|x| x.number != r.number);
            rows.push(r.clone());
        }
    }

    /// Plan a poll over `rows` and apply it.
    fn step(rows: &mut Vec<ReviewPrRow>, open: &[OpenPr], now: i64) -> PollPlan {
        let p = plan(1, "o/r", open, rows, GATE, now);
        apply(rows, &p);
        p
    }

    fn row(rows: &[ReviewPrRow], n: i64) -> &ReviewPrRow {
        rows.iter().find(|r| r.number == n).unwrap()
    }

    #[test]
    fn a_new_pr_is_reviewed_once_its_head_settles() {
        let p = plan(1, "o/r", &[open(7, "aaa")], &[], GATE, 1000);
        assert_eq!(p.rows.len(), 1);
        assert_eq!(p.rows[0].status, "pending_review");
        assert_eq!(p.jobs.len(), 1);
        let j = &p.jobs[0];
        assert_eq!(j.dedup_key, "gh:o/r#7@aaa");
        assert_eq!(j.subject, "gh:o/r#7");
        assert_eq!((j.trigger.as_str(), j.not_before), ("opened", 1060));
        assert_eq!(sha_of_key(&j.dedup_key), Some("aaa"));
        assert_eq!(parse_subject(&j.subject), Some(("o/r", 7)));
    }

    #[test]
    fn nothing_changes_nothing_is_written_and_the_same_head_keeps_its_key() {
        let mut rows = Vec::new();
        step(&mut rows, &[open(7, "aaa")], 1000);
        let again = plan(1, "o/r", &[open(7, "aaa")], &rows, GATE, 1300);
        assert!(again.rows.is_empty(), "{:?}", again.rows);
        assert_eq!(again.jobs.len(), 1, "the store dedups the key");
        assert_eq!(
            again.jobs[0].not_before, 1060,
            "the debounce counts from the first sight"
        );
    }

    #[test]
    fn a_push_after_a_review_is_changes_pushed_and_a_new_job() {
        let mut rows = Vec::new();
        step(&mut rows, &[open(7, "aaa")], 1000);
        let r = rows.iter_mut().find(|r| r.number == 7).unwrap();
        r.status = "reviewed".into();
        r.last_reviewed_sha = Some("aaa".into());
        let same = plan(1, "o/r", &[open(7, "aaa")], &rows, GATE, 2000);
        assert!(same.jobs.is_empty(), "already reviewed at this head");
        let p = plan(1, "o/r", &[open(7, "bbb")], &rows, GATE, 2000);
        assert_eq!(p.rows[0].status, "changes_pushed");
        assert_eq!(p.rows[0].last_push_at, 2000);
        assert_eq!(p.jobs[0].dedup_key, "gh:o/r#7@bbb");
        assert_eq!(
            (p.jobs[0].trigger.as_str(), p.jobs[0].not_before),
            ("pushed", 2060)
        );
    }

    #[test]
    fn drafts_and_wip_titles_wait_and_come_back() {
        let mut rows = Vec::new();
        let mut d = open(7, "aaa");
        d.is_draft = true;
        step(&mut rows, &[d], 1000);
        assert_eq!(row(&rows, 7).skip_reason.as_deref(), Some("draft"));
        let mut w = open(7, "aaa");
        w.title = "[WIP] thing".into();
        let p = plan(1, "o/r", &[w], &rows, GATE, 1100);
        assert!(p.jobs.is_empty());
        apply(&mut rows, &p);
        assert_eq!(row(&rows, 7).skip_reason.as_deref(), Some("wip_title"));
        let p = plan(1, "o/r", &[open(7, "aaa")], &rows, GATE, 1200);
        apply(&mut rows, &p);
        assert_eq!(row(&rows, 7).status, "pending_review");
        assert_eq!(p.jobs.len(), 1);
        // Off in the rule: a draft is reviewed.
        let gate = Gate {
            skip_drafts: false,
            ..GATE
        };
        let mut d = open(8, "ccc");
        d.is_draft = true;
        assert_eq!(plan(1, "o/r", &[d], &[], gate, 0).jobs.len(), 1);
    }

    #[test]
    fn a_size_skip_lifts_only_on_a_new_head() {
        let mut rows = Vec::new();
        step(&mut rows, &[open(7, "aaa")], 1000);
        let r = rows.iter_mut().find(|r| r.number == 7).unwrap();
        r.status = "skipped".into();
        r.skip_reason = Some("diff_too_large".into());
        r.skipped_at_sha = Some("aaa".into());
        assert!(plan(1, "o/r", &[open(7, "aaa")], &rows, GATE, 1100)
            .jobs
            .is_empty());
        let p = plan(1, "o/r", &[open(7, "bbb")], &rows, GATE, 1200);
        assert_eq!(p.rows[0].status, "pending_review");
        assert_eq!(p.jobs.len(), 1);
    }

    #[test]
    fn a_vanished_pr_is_checked_and_a_reopened_one_comes_back() {
        let mut rows = Vec::new();
        let many: Vec<OpenPr> = (1..=8).map(|n| open(n, "aaa")).collect();
        step(&mut rows, &many, 1000);
        let p = plan(1, "o/r", &[], &rows, GATE, 1100);
        assert_eq!(p.to_check, vec![1, 2, 3, 4, 5], "at most five a pass");
        let r = rows.iter_mut().find(|r| r.number == 1).unwrap();
        r.status = "merged".into();
        r.closed_at = Some(1100);
        r.last_reviewed_sha = Some("aaa".into());
        let p = plan(1, "o/r", &[open(1, "aaa")], &rows, GATE, 1200);
        assert_eq!(
            p.rows[0].status, "reviewed",
            "reopened at the reviewed head"
        );
        assert_eq!(p.rows[0].closed_at, None);
        assert!(p.jobs.is_empty());
    }

    #[test]
    fn wip_is_a_word_at_the_start() {
        for t in ["WIP: x", "wip x", "[WIP] x", "  wip", "Wip-thing"] {
            assert!(is_wip(t), "{t}");
        }
        for t in ["wipe the cache", "Fix WIP handling", "", "wi"] {
            assert!(!is_wip(t), "{t}");
        }
        assert_eq!(parse_subject("gh:o/r#x"), None);
        assert_eq!(parse_subject("jira:L2S-1"), None);
        assert_eq!(sha_of_key("gh:o/r#1@"), None);
    }
}
