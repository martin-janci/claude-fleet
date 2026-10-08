//! The merge queue (step 3): at the `merge` level, the shepherd merges a
//! green PR, at most one per project at a time.
//!
//! **Two readings, both must say green.** The planner picks a candidate
//! from what the reconcile probe stored; the executor then asks GitHub
//! again, right before merging, and merges only if the live answer agrees
//! ([`live_refusal`]): open, not draft, `mergeStateStatus = CLEAN`, the same
//! head, no `CHANGES_REQUESTED`, at least one check and every check passed
//! or skipped, none pending. The merge itself is
//! `gh pr merge --merge --match-head-commit <sha>`, so a push between the
//! reading and the merge makes GitHub refuse. GitHub's auto-merge is never
//! used: with no required checks on the base it merges without waiting.
//!
//! **One at a time per project.** After a merge (or a failed attempt) the
//! project waits [`MERGE_SPACING_SECS`] before the next, so GitHub recomputes
//! the other PRs' mergeability against the new base and the nudges get a
//! chance to fix the ones that now conflict, instead of a second merge
//! landing on top. Among ready PRs the lowest PR number goes first.
//!
//! A head is merged or fails once (`pr_shepherd_merges`, migration 140): a
//! failure is not retried until the next push. A refusal (the live answer
//! said not yet) is retried after [`REFUSAL_RETRY_SECS`], one probe period,
//! and does not count as the project's last merge.

use super::Candidate;
use crate::ipc_error::{codes, IpcError};
use crate::store::ShepherdRuleRow;
use std::collections::{HashMap, HashSet};

/// The pause between two merges of one project.
pub const MERGE_SPACING_SECS: i64 = 180;

/// How long a refused head waits before it is tried again: one PR probe
/// period, so the stored reading has been refreshed.
pub const REFUSAL_RETRY_SECS: i64 = crate::service::outcome::PR_PROBE_TTL.as_secs() as i64;

/// What the planner chose to merge this tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedMerge {
    pub session_id: i64,
    pub project_id: i64,
    pub host_alias: String,
    pub pr_url: String,
    pub head_oid: String,
}

/// Why the stored reading does not make a candidate ready, or `None` when
/// it does.
pub fn stored_refusal(c: &Candidate) -> Option<&'static str> {
    let ev = &c.evidence;
    if ev.state.as_deref() != Some("OPEN") {
        return Some("not_open");
    }
    if ev.draft {
        return Some("draft");
    }
    if ev.merge_state.as_deref() != Some("CLEAN") {
        return Some("not_clean");
    }
    if ev.review_decision.as_deref() == Some("CHANGES_REQUESTED") {
        return Some("changes_requested");
    }
    let ch = &ev.checks;
    if ch.total == 0 || ch.pending > 0 || ch.failing_total > 0 {
        return Some("checks");
    }
    if ev.ahead.is_some_and(|a| a > 0)
        || ev
            .local_head
            .as_deref()
            .is_some_and(|l| !l.is_empty() && Some(l) != ev.head_oid.as_deref())
    {
        return Some("unpushed");
    }
    None
}

/// The PR number from a PR URL, for ordering.
fn pr_number(url: &str) -> u64 {
    url.rsplit('/')
        .next()
        .and_then(|n| n.parse().ok())
        .unwrap_or(u64::MAX)
}

/// Pure: at most one merge per project with a `merge` rule, none while the
/// project's last attempt is younger than [`MERGE_SPACING_SECS`].
pub fn plan_merges(
    candidates: &[Candidate],
    rules: &HashMap<i64, ShepherdRuleRow>,
    tried: &HashSet<(i64, String)>,
    last_merge: &HashMap<i64, i64>,
    now: i64,
) -> Vec<PlannedMerge> {
    let mut best: HashMap<i64, (u64, PlannedMerge)> = HashMap::new();
    for c in candidates {
        if rules.get(&c.project_id).map(|r| r.level.as_str()) != Some("merge") {
            continue;
        }
        if last_merge
            .get(&c.project_id)
            .is_some_and(|t| now - t < MERGE_SPACING_SECS)
        {
            continue;
        }
        let (Some(url), Some(head)) = (c.pr_url.clone(), c.evidence.head_oid.clone()) else {
            continue;
        };
        if head.is_empty()
            || tried.contains(&(c.session_id, head.clone()))
            || stored_refusal(c).is_some()
            || validate_pr_url(&url).is_err()
        {
            continue;
        }
        let n = pr_number(&url);
        if best.get(&c.project_id).is_some_and(|(m, _)| *m <= n) {
            continue;
        }
        best.insert(
            c.project_id,
            (
                n,
                PlannedMerge {
                    session_id: c.session_id,
                    project_id: c.project_id,
                    host_alias: c.host_alias.clone(),
                    pr_url: url,
                    head_oid: head,
                },
            ),
        );
    }
    let mut out: Vec<PlannedMerge> = best.into_values().map(|(_, p)| p).collect();
    out.sort_by_key(|p| p.project_id);
    out
}

/// A PR URL fit to put on a command line: `https://<host>/<owner>/<repo>/pull/<n>`
/// with plain name characters only. It is shell-quoted as well; this keeps
/// anything odd out of `gh`'s argument parsing.
pub fn validate_pr_url(url: &str) -> Result<(), IpcError> {
    let bad = || {
        IpcError::new(
            codes::E_VALIDATE,
            format!("not a pull request URL: {url:?}"),
        )
    };
    let rest = url.strip_prefix("https://").ok_or_else(bad)?;
    let parts: Vec<&str> = rest.split('/').collect();
    let name_ok = |s: &str| {
        !s.is_empty()
            && !s.starts_with('-')
            && !s.starts_with('.')
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    match parts.as_slice() {
        [host, owner, repo, "pull", n]
            if name_ok(host)
                && name_ok(owner)
                && name_ok(repo)
                && !n.is_empty()
                && n.chars().all(|c| c.is_ascii_digit()) =>
        {
            Ok(())
        }
        _ => Err(bad()),
    }
}

/// A head commit as `gh --match-head-commit` takes it: 40 or 64 hex digits.
pub fn validate_sha(sha: &str) -> Result<(), IpcError> {
    if matches!(sha.len(), 40 | 64) && sha.chars().all(|c| c.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(IpcError::new(
            codes::E_VALIDATE,
            format!("not a commit id: {sha:?}"),
        ))
    }
}

/// The fields the live check reads.
pub const LIVE_FIELDS: &str =
    "state,isDraft,mergeStateStatus,headRefOid,reviewDecision,statusCheckRollup";

/// `gh pr view` for the live check.
pub fn view_script(url: &str) -> Result<String, IpcError> {
    validate_pr_url(url)?;
    Ok(format!(
        "gh pr view {} --json {LIVE_FIELDS}",
        crate::shell::quote(url)
    ))
}

/// The merge itself: a merge commit, refused by GitHub if the head moved.
pub fn merge_script(url: &str, sha: &str) -> Result<String, IpcError> {
    validate_pr_url(url)?;
    validate_sha(sha)?;
    Ok(format!(
        "gh pr merge {} --merge --match-head-commit {}",
        crate::shell::quote(url),
        crate::shell::quote(sha)
    ))
}

/// Why GitHub's live answer refuses the merge, or `None` when it agrees.
pub fn live_refusal(view_json: &str, sha: &str) -> Option<String> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(view_json) else {
        return Some("unreadable".into());
    };
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("");
    if s("state") != "OPEN" {
        return Some("not_open".into());
    }
    if v.get("isDraft").and_then(|x| x.as_bool()) != Some(false) {
        return Some("draft".into());
    }
    if s("headRefOid") != sha {
        return Some("head_moved".into());
    }
    if s("mergeStateStatus") != "CLEAN" {
        return Some(format!(
            "merge_state_{}",
            s("mergeStateStatus").to_ascii_lowercase()
        ))
        .filter(|w| w.len() <= 40)
        .or(Some("not_clean".into()));
    }
    if s("reviewDecision") == "CHANGES_REQUESTED" {
        return Some("changes_requested".into());
    }
    let checks = v
        .get("statusCheckRollup")
        .and_then(|x| x.as_array())
        .cloned()
        .unwrap_or_default();
    let sum = crate::service::outcome::summarize_checks(&checks);
    if sum.total == 0 || sum.pending > 0 || sum.failing_total > 0 {
        return Some("checks".into());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_and_shas_are_checked_before_any_command_is_built() {
        assert!(validate_pr_url("https://github.com/o/r/pull/12").is_ok());
        assert!(validate_pr_url("https://ghe.example.com/o-x/r.y/pull/1").is_ok());
        for bad in [
            "http://github.com/o/r/pull/1",
            "https://github.com/o/r/pull/1;rm -rf /",
            "https://github.com/o/r/pull/",
            "https://github.com/-o/r/pull/1",
            "https://github.com/o/r/issues/1",
            "https://github.com/o/r/pull/1/files",
        ] {
            assert!(validate_pr_url(bad).is_err(), "{bad}");
        }
        assert!(validate_sha(&"a".repeat(40)).is_ok());
        assert!(validate_sha("abc").is_err());
        assert!(validate_sha(&format!("{}--admin", "a".repeat(33))).is_err());
        let sha = "0123456789abcdef0123456789abcdef01234567";
        assert_eq!(
            merge_script("https://github.com/o/r/pull/7", sha).unwrap(),
            format!(
                "gh pr merge 'https://github.com/o/r/pull/7' --merge --match-head-commit '{sha}'"
            )
        );
    }

    fn live(merge_state: &str, head: &str, checks: serde_json::Value) -> String {
        serde_json::json!({
            "state": "OPEN", "isDraft": false, "mergeStateStatus": merge_state,
            "headRefOid": head, "reviewDecision": "", "statusCheckRollup": checks,
        })
        .to_string()
    }

    #[test]
    fn the_live_check_wants_clean_same_head_and_all_checks_done() {
        let sha = "a".repeat(40);
        let ok = serde_json::json!([
            {"__typename": "CheckRun", "status": "COMPLETED", "conclusion": "SUCCESS", "name": "rust"},
            {"__typename": "CheckRun", "status": "COMPLETED", "conclusion": "SKIPPED", "name": "win"},
        ]);
        assert_eq!(live_refusal(&live("CLEAN", &sha, ok.clone()), &sha), None);
        assert_eq!(
            live_refusal(&live("DIRTY", &sha, ok.clone()), &sha).as_deref(),
            Some("merge_state_dirty")
        );
        assert_eq!(
            live_refusal(&live("CLEAN", &"b".repeat(40), ok), &sha).as_deref(),
            Some("head_moved")
        );
        let pending = serde_json::json!([
            {"__typename": "CheckRun", "status": "IN_PROGRESS", "conclusion": "", "name": "rust"},
        ]);
        assert_eq!(
            live_refusal(&live("CLEAN", &sha, pending), &sha).as_deref(),
            Some("checks")
        );
        assert_eq!(
            live_refusal(&live("CLEAN", &sha, serde_json::json!([])), &sha).as_deref(),
            Some("checks")
        );
        assert_eq!(
            live_refusal("not json", &sha).as_deref(),
            Some("unreadable")
        );
    }
}
