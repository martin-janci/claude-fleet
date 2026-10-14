//! GitHub for fleet agents (spec step 6): the `github_prs` and
//! `github_comments` trigger sources, through the PR reviewer's own `gh`
//! calls (`agents::review::SshReviewHost`).
//!
//! * `github_prs`: a job per open, non-draft pull request at each new head
//!   (the key carries the head's sha, so a push is a new job).
//! * `github_comments`: a job per new issue or PR comment carrying a phrase
//!   or matching the regex, never a bot's or a fleet agent's own (its marker).
//!
//! Both keep the job's subject when they find it (`runner::keep_subject`):
//! the runner reads it back rather than asking GitHub again.

use super::runner::{comment_matches, render_opt, Instance, AGENT, POLL_SLACK_SECS};
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

/// PURE: a job per open, non-draft pull request at its head, with the
/// subject to keep.
pub fn plan_pr_jobs(
    rule_id: i64,
    repo: &str,
    prs: &[OpenPr],
    now: i64,
) -> Vec<(NewAgentJob, Value)> {
    prs.iter()
        .filter(|p| !p.is_draft && !p.head_sha.is_empty())
        .map(|p| {
            let subject = format!("gh:{repo}#{}", p.number);
            (
                NewAgentJob {
                    rule_id,
                    agent: AGENT.into(),
                    subject: subject.clone(),
                    trigger: "pushed".into(),
                    dedup_key: format!("r{rule_id}:{subject}@{}", p.head_sha),
                    not_before: now,
                },
                json!({
                    "repo": repo,
                    "number": p.number,
                    "title": p.title,
                    "head_sha": p.head_sha,
                    "base_ref": p.base_ref,
                    "head_ref": p.head_ref,
                    "author": p.author,
                    "url": pr_url(repo, p.number),
                }),
            )
        })
        .collect()
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

/// The RFC 3339 time a comment poll reads from: the last poll, less the slack.
pub fn comments_since(since: i64) -> String {
    crate::service::agents::review::rfc3339(since - POLL_SLACK_SECS)
}
