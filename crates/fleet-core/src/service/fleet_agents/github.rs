//! GitHub for fleet agents (spec step 6): the `github_prs` and
//! `github_comments` trigger sources, through the PR reviewer's own `gh`
//! calls (`agents::review::SshReviewHost`).
//!
//! * `github_prs`: a job per open, non-draft pull request at each new head
//!   (the key carries the head's sha, so a push is a new job).
//! * `github_comments`: a job per new issue or PR comment carrying a phrase
//!   or matching the regex, never a bot's or a fleet agent's own (its marker).
//! * `github_review_replies`: a job per person's reply in a review thread a
//!   fleet agent started (a marker on the thread's first comment, and the
//!   `match` there when set: usually another instance's, a reviewer's), until
//!   this instance has answered `max_rounds` times: the PR reviewer's test
//!   debate.
//! * `github_prs`' `reverify_mins`: the instance's last review of an open
//!   pull request ([`reverify_due`]) that GitHub deleted or dismissed is
//!   made again at the same head (the PR reviewer's verify).
//!
//! Both keep the job's subject when they find it (`runner::keep_subject`):
//! the runner reads it back rather than asking GitHub again.

use super::runner::{comment_matches, render_opt, Instance, AGENT, POLL_SLACK_SECS};
use crate::service::agents::review::debate::ReviewComment;
use crate::service::agents::review::plan::OpenPr;
use crate::service::agents::review::PrComment;
use crate::store::NewAgentJob;
use serde_json::{json, Value};

/// PURE: whether `s` is `owner/repo`.
pub fn repo_ok(s: &str) -> bool {
    let part = |p: &str| {
        !p.is_empty()
            && p != "."
            && p != ".."
            && p.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    matches!(s.split_once('/'), Some((o, r)) if part(o) && part(r))
}

/// The repo a trigger names, rendered over the inputs.
pub fn repo_of(inst: &Instance, ctx: &Value) -> Result<String, String> {
    let r = render_opt(inst.loaded().definition.trigger.repo.as_deref(), ctx)
        .trim()
        .to_string();
    if repo_ok(&r) {
        Ok(r)
    } else {
        Err(format!("trigger › repo {r:?} is not owner/repo"))
    }
}

fn pr_url(repo: &str, number: i64) -> String {
    format!("https://github.com/{repo}/pull/{number}")
}

/// The trigger's gate on a pull request: `skip_wip`, `debounce_secs`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PrGate {
    pub skip_wip: bool,
    pub debounce_secs: i64,
}

/// PURE: a pull request's subject.
pub fn pr_subject(repo: &str, p: &OpenPr) -> Value {
    json!({
        "repo": repo,
        "number": p.number,
        "title": p.title,
        "head_sha": p.head_sha,
        "base_ref": p.base_ref,
        "head_ref": p.head_ref,
        "author": p.author,
        "url": pr_url(repo, p.number),
    })
}

/// PURE: a job per open, non-draft pull request at its head (not a WIP one
/// with `skip_wip`), due `debounce_secs` after the head was first seen,
/// with the subject to keep. A job of a head that moved on is skipped when
/// it runs (`runner`).
pub fn plan_pr_jobs(
    rule_id: i64,
    repo: &str,
    prs: &[OpenPr],
    gate: PrGate,
    now: i64,
) -> Vec<(NewAgentJob, Value)> {
    let due = now + gate.debounce_secs.max(0);
    gated(prs, gate)
        .filter(|p| !p.head_sha.is_empty())
        .map(|p| pr_job(rule_id, repo, p, "pushed", "", due))
        .collect()
}

/// PURE: the open pull requests the trigger's gate lets through: none a
/// draft, none a WIP one with `skip_wip`.
pub fn gated(prs: &[OpenPr], gate: PrGate) -> impl Iterator<Item = &OpenPr> {
    prs.iter().filter(move |p| {
        !p.is_draft && !(gate.skip_wip && crate::service::agents::review::plan::is_wip(&p.title))
    })
}

/// PURE: pull request `p`'s job for `trigger`, keyed by its head (then
/// `key_suffix`), due at `not_before`, with the subject to keep.
fn pr_job(
    rule_id: i64,
    repo: &str,
    p: &OpenPr,
    trigger: &str,
    key_suffix: &str,
    not_before: i64,
) -> (NewAgentJob, Value) {
    let subject = format!("gh:{repo}#{}", p.number);
    (
        NewAgentJob {
            rule_id,
            agent: AGENT.into(),
            subject: subject.clone(),
            trigger: trigger.into(),
            dedup_key: format!("r{rule_id}:{subject}@{}{key_suffix}", p.head_sha),
            not_before,
        },
        pr_subject(repo, p),
    )
}

/// Pull requests whose review threads one poll reads, at most.
pub const MAX_THREAD_PRS: usize = 10;
/// Reviews one poll looks up, at most (the PR reviewer's `MAX_VERIFIES`).
pub const MAX_REVERIFIES: usize = 5;

/// PURE: the review id to look up when the instance's last review of a
/// pull request (`actions::remembered_review`) is at `head_sha`, still has
/// GitHub's id, and was last looked at `mins` or more ago.
pub fn reverify_due(review: &Value, head_sha: &str, mins: i64, now: i64) -> Option<i64> {
    let id = review.get("id")?.as_i64()?;
    let at = review
        .get("verified_at")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    (review.get("sha")?.as_str()? == head_sha && at + mins * 60 <= now).then_some(id)
}

/// PURE: a reverified review's job: keyed apart from its head's own, so it
/// runs at a head already reviewed.
pub fn reverify_job(
    rule_id: i64,
    repo: &str,
    p: &OpenPr,
    review_id: i64,
    now: i64,
) -> (NewAgentJob, Value) {
    pr_job(rule_id, repo, p, "reverify", &format!(":r{review_id}"), now)
}

/// PURE: the instance's marker and the trigger's `match` over a body.
fn matcher(inst: &Instance, ctx: &Value) -> Result<(Vec<String>, Option<regex::Regex>), String> {
    let m = inst
        .loaded()
        .definition
        .trigger
        .match_
        .clone()
        .unwrap_or_default();
    let phrases: Vec<String> = render_opt(m.any_phrase.as_deref(), ctx)
        .split(',')
        .map(|p| p.trim().to_lowercase())
        .filter(|p| !p.is_empty())
        .collect();
    let regex = match &m.regex {
        Some(r) => Some(
            regex::RegexBuilder::new(r)
                .size_limit(1 << 16)
                .build()
                .map_err(|e| format!("trigger › match › regex: {e}"))?,
        ),
        None => None,
    };
    Ok((phrases, regex))
}

/// PURE: a job per comment that asks (a phrase, or the regex on a line),
/// never a bot's or a fleet agent's own, with the subject to keep.
pub fn plan_comment_jobs(
    rule_id: i64,
    inst: &Instance,
    ctx: &Value,
    repo: &str,
    comments: &[PrComment],
    now: i64,
) -> Result<Vec<(NewAgentJob, Value)>, String> {
    let (phrases, regex) = matcher(inst, ctx)?;
    Ok(comments
        .iter()
        .filter(|c| c.user_type != "Bot")
        .filter(|c| !super::actions::is_agent_comment(&c.body))
        .filter(|c| comment_matches(&c.body, &phrases, regex.as_ref()))
        .map(|c| {
            let subject = format!("gh:{repo}#{}", c.number);
            (
                NewAgentJob {
                    rule_id,
                    agent: AGENT.into(),
                    subject: subject.clone(),
                    trigger: "comment".into(),
                    dedup_key: format!("r{rule_id}:{subject}#c{}", c.id),
                    not_before: now,
                },
                json!({
                    "repo": repo,
                    "number": c.number,
                    "title": "",
                    "url": pr_url(repo, c.number),
                    "comment": { "id": c.id, "body": c.body },
                }),
            )
        })
        .collect())
}

/// PURE: a comment job's subject with the pull request's head, base and
/// branch when the comment is on an open pull request (an issue's has none).
pub fn with_pr_heads(
    planned: Vec<(NewAgentJob, Value)>,
    prs: &[OpenPr],
) -> Vec<(NewAgentJob, Value)> {
    planned
        .into_iter()
        .map(|(job, mut subject)| {
            let n = subject.get("number").and_then(Value::as_i64);
            if let Some(p) = prs.iter().find(|p| Some(p.number) == n) {
                subject["head_sha"] = json!(p.head_sha);
                subject["base_ref"] = json!(p.base_ref);
                subject["head_ref"] = json!(p.head_ref);
                subject["title"] = json!(p.title);
            }
            (job, subject)
        })
        .collect()
}

/// The RFC 3339 time a comment poll reads from: the last poll, less the slack.
pub fn comments_since(since: i64) -> String {
    crate::service::agents::review::rfc3339(since - POLL_SLACK_SECS)
}

/// PURE: the pull requests whose threads a reply poll reads: those with a
/// person's reply among `replies`, open, at most [`MAX_THREAD_PRS`].
pub fn reply_prs<'a>(replies: &[ReviewComment], prs: &'a [OpenPr]) -> Vec<&'a OpenPr> {
    let mut out: Vec<&OpenPr> = Vec::new();
    for c in replies.iter().filter(|c| is_persons_reply(c)) {
        if out.iter().any(|p| p.number == c.number) {
            continue;
        }
        if let Some(p) = prs.iter().find(|p| p.number == c.number) {
            out.push(p);
        }
    }
    out.truncate(MAX_THREAD_PRS);
    out
}

fn is_persons_reply(c: &ReviewComment) -> bool {
    c.in_reply_to.is_some() && c.user_type != "Bot" && !super::actions::is_agent_comment(&c.body)
}

/// PURE: a job per person's reply among `replies` in a thread of `thread`
/// (pull request `p`'s review comments) whose first comment is a fleet
/// agent's and carries the trigger's `match` when it has one, the last
/// word in the thread being a person's and this instance (its marker
/// `mark`) having answered it fewer than the trigger's `max_rounds` times; with the
/// subject to keep.
#[allow(clippy::too_many_arguments)]
pub fn plan_reply_jobs(
    rule_id: i64,
    inst: &Instance,
    ctx: &Value,
    repo: &str,
    mark: &str,
    p: &OpenPr,
    replies: &[ReviewComment],
    thread: &[ReviewComment],
    now: i64,
) -> Result<Vec<(NewAgentJob, Value)>, String> {
    let (phrases, regex) = matcher(inst, ctx)?;
    let trigger = &inst.loaded().definition.trigger;
    let matching = trigger.match_.is_some();
    let max_rounds = trigger
        .max_rounds
        .unwrap_or(super::definition::DEFAULT_MAX_ROUNDS);
    let mut out = Vec::new();
    // One job per thread a poll, for its newest word.
    let mut seen = std::collections::BTreeSet::new();
    for c in replies
        .iter()
        .filter(|c| c.number == p.number && is_persons_reply(c))
    {
        let Some(root) = thread.iter().find(|r| Some(r.id) == c.in_reply_to) else {
            continue;
        };
        if !seen.insert(root.id)
            || !super::actions::is_agent_comment(&root.body)
            || (matching && !comment_matches(&root.body, &phrases, regex.as_ref()))
        {
            continue;
        }
        let mut answers: Vec<&ReviewComment> = thread
            .iter()
            .filter(|r| r.in_reply_to == Some(root.id))
            .collect();
        answers.sort_by_key(|r| r.id);
        let rounds = answers.iter().filter(|r| r.body.contains(mark)).count();
        // Answered, or past its rounds, the thread is left to the people.
        let Some(&last) = answers.last().filter(|r| is_persons_reply(r)) else {
            continue;
        };
        if rounds >= max_rounds {
            continue;
        }
        let strip = |b: &str| super::actions::without_markers(b);
        let mut subject = pr_subject(repo, p);
        subject["comment"] = json!({ "id": last.id, "body": last.body, "user": last.user });
        subject["thread"] = json!({
            "root_id": root.id,
            "path": root.path,
            "line": root.line,
            "diff_hunk": root.diff_hunk,
            "finding": strip(&root.body),
            "rounds": rounds,
            "replies": answers.iter().map(|r| json!({
                "user": r.user,
                "body": strip(&r.body),
                "mine": r.body.contains(mark),
            })).collect::<Vec<_>>(),
        });
        let key = format!("gh:{repo}#{}@t{}", p.number, root.id);
        out.push((
            NewAgentJob {
                rule_id,
                agent: AGENT.into(),
                subject: key.clone(),
                trigger: "reply".into(),
                dedup_key: format!("r{rule_id}:{key}:c{}", last.id),
                not_before: now,
            },
            subject,
        ));
    }
    Ok(out)
}
