//! Posting a review, ported from claude-code-reviewer's `reviewer.ts`
//! (the verdict adjustments, the event mapping) and `github.ts`
//! (`POST /repos/{o}/{r}/pulls/{n}/reviews`).
//!
//! * [`adjust_verdict`]: on a re-review, a previous blocking finding no
//!   resolution settles escalates to `REQUEST_CHANGES`; a `COMMENT` with
//!   nothing blocking and every previous blocker settled becomes `APPROVE`.
//! * [`event_for`]: `APPROVE` approves, `REQUEST_CHANGES` blocks only when
//!   the rule's `request_changes_event` is on, everything else comments.
//! * [`ReviewRequest`] is what an `agents` action carries: exactly what
//!   will be sent, so a person approves the bytes.

use super::format;
use super::output::{Finding, PlacedReview};
use serde::{Deserialize, Serialize};

/// The action kind that posts a review.
pub const POST_REVIEW: &str = "post_review";

/// PURE: the verdict to post, from the model's and the previous review's
/// findings.
pub fn adjust_verdict(r: &PlacedReview, previous: &[Finding]) -> String {
    if previous.is_empty() {
        return r.verdict.clone();
    }
    let settled = previous.iter().all(|pf| {
        !pf.blocking
            || r.resolutions
                .iter()
                .find(|x| x.path == pf.path && x.line == pf.line)
                .is_some_and(|x| x.resolution != "open")
    });
    if !r.resolutions.is_empty() && !settled {
        return "REQUEST_CHANGES".into();
    }
    let blocking_now = r.inline.iter().chain(&r.orphans).any(|f| f.blocking);
    if r.verdict == "COMMENT" && !blocking_now && settled {
        return "APPROVE".into();
    }
    r.verdict.clone()
}

/// PURE: the GitHub review event for a verdict.
pub fn event_for(verdict: &str, request_changes_event: bool) -> &'static str {
    match verdict {
        "APPROVE" => "APPROVE",
        "REQUEST_CHANGES" if request_changes_event => "REQUEST_CHANGES",
        _ => "COMMENT",
    }
}

/// One inline comment as GitHub takes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewComment {
    pub path: String,
    pub line: u32,
    pub body: String,
}

/// A review to post, as an action carries it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewRequest {
    /// The host whose `gh` posts it (the rule's).
    pub host: String,
    pub repo: String,
    pub number: i64,
    /// The head it reviews; a pull request whose head moved is not posted.
    pub sha: String,
    /// The `review_records` row it posts.
    pub record_id: i64,
    pub event: String,
    pub body: String,
    pub comments: Vec<ReviewComment>,
}

/// What [`build`] needs.
pub struct BuildInput<'a> {
    pub placed: &'a PlacedReview,
    pub previous: &'a [Finding],
    pub host: &'a str,
    pub repo: &'a str,
    pub number: i64,
    pub sha: &'a str,
    pub record_id: i64,
    pub request_changes_event: bool,
}

/// PURE: the review to post.
pub fn build(i: &BuildInput<'_>) -> ReviewRequest {
    let verdict = adjust_verdict(i.placed, i.previous);
    ReviewRequest {
        host: i.host.to_string(),
        repo: i.repo.to_string(),
        number: i.number,
        sha: i.sha.to_string(),
        record_id: i.record_id,
        event: event_for(&verdict, i.request_changes_event).to_string(),
        body: format::review_body(i.placed, &verdict, i.sha),
        comments: i
            .placed
            .inline
            .iter()
            .map(|f| ReviewComment {
                path: f.path.clone(),
                line: f.line,
                body: format::inline_comment(f),
            })
            .collect(),
    }
}

/// PURE: the JSON body of `POST /repos/{o}/{r}/pulls/{n}/reviews` for
/// `req` with `event`.
pub fn api_body(req: &ReviewRequest, event: &str) -> String {
    serde_json::json!({
        "body": req.body,
        "event": event,
        "commit_id": req.sha,
        "comments": req.comments,
    })
    .to_string()
}

/// PURE: whether GitHub refused the event because the poster authored the
/// pull request (it then takes the same review as a comment).
pub fn is_self_review_refusal(said: &str) -> bool {
    said.to_lowercase().contains("own pull request")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::agents::review::output::Resolution;

    fn f(blocking: bool, line: u32) -> Finding {
        Finding {
            severity: if blocking { "issue" } else { "nitpick" }.into(),
            blocking,
            path: "a.rs".into(),
            line,
            body: "b".into(),
            confidence: None,
            security_related: None,
            test_related: None,
            is_new: None,
        }
    }

    fn placed(verdict: &str, findings: Vec<Finding>, res: &[(u32, &str)]) -> PlacedReview {
        PlacedReview {
            verdict: verdict.into(),
            summary: "s".into(),
            inline: findings,
            orphans: vec![],
            dropped: 0,
            pr_summary: None,
            test_coverage: None,
            resolutions: res
                .iter()
                .map(|(line, r)| Resolution {
                    path: "a.rs".into(),
                    line: *line,
                    body: "x".into(),
                    resolution: (*r).into(),
                })
                .collect(),
            overall: None,
            description: None,
        }
    }

    #[test]
    fn a_re_review_escalates_and_de_escalates() {
        let prev = [f(true, 3), f(false, 9)];
        // First review: the model's verdict stands.
        assert_eq!(
            adjust_verdict(&placed("COMMENT", vec![], &[]), &[]),
            "COMMENT"
        );
        // A previous blocker still open: escalate.
        assert_eq!(
            adjust_verdict(&placed("APPROVE", vec![], &[(3, "open")]), &prev),
            "REQUEST_CHANGES"
        );
        // A blocker no resolution mentions is not settled either.
        assert_eq!(
            adjust_verdict(&placed("APPROVE", vec![], &[(9, "resolved")]), &prev),
            "REQUEST_CHANGES"
        );
        // Every blocker settled, nothing new blocking: COMMENT → APPROVE.
        assert_eq!(
            adjust_verdict(
                &placed("COMMENT", vec![f(false, 1)], &[(3, "wont_fix")]),
                &prev
            ),
            "APPROVE"
        );
        // Something new blocks: COMMENT stays.
        assert_eq!(
            adjust_verdict(
                &placed("COMMENT", vec![f(true, 1)], &[(3, "resolved")]),
                &prev
            ),
            "COMMENT"
        );
    }

    #[test]
    fn the_event_and_the_request() {
        assert_eq!(event_for("APPROVE", false), "APPROVE");
        assert_eq!(event_for("REQUEST_CHANGES", true), "REQUEST_CHANGES");
        assert_eq!(event_for("REQUEST_CHANGES", false), "COMMENT");
        assert_eq!(event_for("COMMENT", true), "COMMENT");
        let p = placed("REQUEST_CHANGES", vec![f(true, 3)], &[]);
        let req = build(&BuildInput {
            placed: &p,
            previous: &[],
            host: "box",
            repo: "o/r",
            number: 4,
            sha: "abc1234",
            record_id: 9,
            request_changes_event: true,
        });
        assert_eq!(
            (req.event.as_str(), req.comments.len()),
            ("REQUEST_CHANGES", 1)
        );
        assert!(req.body.starts_with(format::MARKER));
        let body: serde_json::Value = serde_json::from_str(&api_body(&req, "COMMENT")).unwrap();
        assert_eq!(body["commit_id"], "abc1234");
        assert_eq!(body["event"], "COMMENT");
        assert_eq!(body["comments"][0]["line"], 3);
        assert!(is_self_review_refusal(
            "HTTP 422: Unprocessable Entity (Can not approve your own pull request)"
        ));
    }
}
