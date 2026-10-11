//! The PR reviewer: a port of claude-code-reviewer onto the agent runtime
//! (`super`). Design: `docs/superpowers/specs/2026-10-11-review-and-support-agents-design.md`, part B.
//!
//! A rule `review` on `gh:<owner>/<repo>` makes the reviewer:
//!
//! 1. **poll** (`ReviewPoller`): list the repo's open pull requests on the
//!    rule's host (`gh pr list`), turn them into rows and due reviews
//!    ([`plan`]), and ask GitHub whether the rows that vanished from the
//!    list were closed or merged;
//! 2. **run** a job (`ReviewRunner`): fetch the diff and the base branch's
//!    `.claudeignore`, filter and size-gate the diff ([`diff`]), prepare a
//!    detached worktree of exactly the head the job is about
//!    (`super::workspace`), run the locked-down review (`super::vehicle`,
//!    tools `Read,Grep,Glob`, the skill as system prompt), parse and place
//!    the answer ([`output`]), and record it (`review_records`).
//!
//! 3. **post** by the rule's level: `watch` records the review only;
//!    `propose` queues it as a `post_review` action a person approves;
//!    `act` queues it approved, and the run pass's executor
//!    (`ReviewExecutor`) posts it ([`post`], [`format`]) unless the head
//!    moved on meanwhile.
//!
//! The poll also verifies posted reviews (a deleted or dismissed one is
//! reviewed again) and reads new PR comments for the `/review` trigger
//! (`comment_trigger`): either is a forced job, keyed apart from the head's
//! own so it runs even at a reviewed head.
//!
//! Every call on the host goes through [`ReviewHost`], so the poller and the
//! runner are tested against a fake one.

pub mod diff;
pub mod format;
pub mod output;
pub mod plan;
pub mod post;
pub mod prompt;
#[cfg(test)]
mod tests;

use super::vehicle::{self, PrintOutcome, PrintRun};
use super::workspace::{self, Prepared, Source};
use super::{AgentCursorRow, AgentImpl, AgentPoller, AgentRunner, ErrorKind, Polled, RunOutcome};
use crate::ipc_error::{lock, IpcError};
use crate::shell::quote;
use crate::ssh::SshClient;
use crate::store::{AgentJobRow, AgentRuleRow, NewReviewRecord, Store};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// A rule's options, from its `config` JSON; every field has the original
/// service's default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ReviewConfig {
    /// Lines of filtered diff past which a pull request is skipped.
    pub max_diff_lines: usize,
    pub skip_drafts: bool,
    pub skip_wip: bool,
    /// Seconds a new head must rest before it is reviewed.
    pub debounce_secs: i64,
    /// Globs never shown to the reviewer; they beat the repo's `.claudeignore`.
    pub exclude_paths: Vec<String>,
    /// Read the base branch's `.claudeignore`.
    pub respect_claudeignore: bool,
    pub max_turns: u32,
    pub timeout_secs: u64,
    /// `--model`; empty is the login's default.
    pub model: String,
    /// Findings under this confidence (0-100) are left out; 0 keeps all.
    pub confidence_threshold: u8,
    /// Globs the prompt calls security-sensitive.
    pub security_paths: Vec<String>,
    pub focus_paths: Vec<String>,
    pub require_tests: bool,
    /// medium | high | critical.
    pub test_blocking_importance: String,
    /// Post `REQUEST_CHANGES` as a blocking review; off, it is a comment.
    pub request_changes_event: bool,
    /// A PR comment matching this (a regex, per line) forces a review.
    pub comment_trigger: String,
    /// Minutes between two checks that a posted review is still there.
    pub verify_interval_mins: i64,
}

impl Default for ReviewConfig {
    fn default() -> Self {
        ReviewConfig {
            max_diff_lines: 5000,
            skip_drafts: true,
            skip_wip: true,
            debounce_secs: 60,
            exclude_paths: Vec::new(),
            respect_claudeignore: true,
            max_turns: 15,
            timeout_secs: 600,
            model: String::new(),
            confidence_threshold: 0,
            security_paths: [
                "**/auth/**",
                "**/crypto/**",
                "**/security/**",
                "**/*.env*",
                "**/secrets/**",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            focus_paths: Vec::new(),
            require_tests: true,
            test_blocking_importance: "high".into(),
            request_changes_event: true,
            comment_trigger: r"^\s*/review\s*$".into(),
            verify_interval_mins: 60,
        }
    }
}

impl ReviewConfig {
    /// PURE: a rule's `config` text as options, checked.
    pub fn parse(config: &str) -> Result<Self, String> {
        let c: ReviewConfig =
            serde_json::from_str(config).map_err(|e| format!("review config: {e}"))?;
        if !(1..=vehicle::MAX_TURNS).contains(&c.max_turns) {
            return Err(format!("max_turns must be 1-{}", vehicle::MAX_TURNS));
        }
        if !(60..=3600).contains(&c.timeout_secs) {
            return Err("timeout_secs must be 60-3600".into());
        }
        if c.max_diff_lines == 0 || c.debounce_secs < 0 || c.confidence_threshold > 100 {
            return Err(
                "max_diff_lines, debounce_secs or confidence_threshold is out of range".into(),
            );
        }
        if !["medium", "high", "critical"].contains(&c.test_blocking_importance.as_str()) {
            return Err("test_blocking_importance must be medium, high or critical".into());
        }
        if c.verify_interval_mins < 1 {
            return Err("verify_interval_mins must be at least 1".into());
        }
        c.trigger().map_err(|e| format!("comment_trigger: {e}"))?;
        Ok(c)
    }

    /// The `/review` trigger, matched per line; empty turns it off.
    pub fn trigger(&self) -> Result<Option<regex::Regex>, regex::Error> {
        if self.comment_trigger.is_empty() {
            return Ok(None);
        }
        regex::RegexBuilder::new(&self.comment_trigger)
            .multi_line(true)
            .size_limit(1 << 16)
            .build()
            .map(Some)
    }
}

/// `gh:<owner>/<repo>` as `owner/repo`.
pub fn repo_of_scope(scope: &str) -> Option<&str> {
    scope.strip_prefix("gh:")
}

/// What the reviewer does on a rule's host.
#[async_trait::async_trait]
pub trait ReviewHost: Send + Sync {
    /// The repo's open pull requests.
    async fn open_prs(&self, host: &str, repo: &str) -> Result<Vec<plan::OpenPr>, String>;
    /// `MERGED`, `CLOSED` or `OPEN`.
    async fn pr_state(&self, host: &str, repo: &str, number: i64) -> Result<String, String>;
    /// The pull request's diff; `Err` carries what `gh` said.
    async fn diff(&self, host: &str, repo: &str, number: i64) -> Result<String, String>;
    /// The base branch's `.claudeignore`, `None` when it has none.
    async fn claude_ignore(&self, host: &str, repo: &str, base: &str) -> Option<String>;
    /// The project's checkout on the host.
    async fn checkout(&self, host: &str, project_id: i64) -> Result<String, IpcError>;
    async fn prepare(
        &self,
        host: &str,
        root: &str,
        job_id: i64,
        number: i64,
        sha: &str,
    ) -> Result<(Prepared, String), IpcError>;
    async fn run(&self, host: &str, run: &PrintRun, prompt: &str)
        -> Result<PrintOutcome, IpcError>;
    async fn cleanup(&self, host: &str, root: &str, job_id: i64);
    async fn prune(&self, host: &str, root: &str);
    /// Post a review with `event`; GitHub's id for it.
    async fn post_review(&self, req: &post::ReviewRequest, event: &str) -> Result<i64, String>;
    /// A posted review's state (`COMMENTED`, `DISMISSED` …); `None` when
    /// GitHub no longer has it.
    async fn review_state(
        &self,
        host: &str,
        repo: &str,
        number: i64,
        review_id: i64,
    ) -> Result<Option<String>, String>;
    /// The repo's issue and PR comments updated since `since` (RFC 3339).
    async fn comments_since(
        &self,
        host: &str,
        repo: &str,
        since: &str,
    ) -> Result<Vec<PrComment>, String>;
}

/// One comment on a pull request (or an issue: its number is then not a
/// tracked pull request's, and nothing comes of it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrComment {
    pub id: i64,
    pub number: i64,
    pub body: String,
    /// `User` or `Bot`.
    pub user_type: String,
}

/// PURE: `gh api …/issues/comments --jq …`'s lines as comments.
pub fn parse_comments(stdout: &str) -> Vec<PrComment> {
    stdout
        .lines()
        .filter_map(|l| {
            let v: serde_json::Value = serde_json::from_str(l.trim()).ok()?;
            let number = v
                .get("issue_url")?
                .as_str()?
                .rsplit_once("/issues/")?
                .1
                .parse()
                .ok()?;
            Some(PrComment {
                id: v.get("id")?.as_i64()?,
                number,
                body: v.get("body")?.as_str()?.to_string(),
                user_type: v
                    .get("user_type")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("User")
                    .to_string(),
            })
        })
        .collect()
}

/// PURE: unix seconds as RFC 3339 UTC (`2026-10-11T09:30:00Z`).
pub fn rfc3339(unix: i64) -> String {
    let (y, m, d) = crate::service::routines::cron::civil_from_days(unix.div_euclid(86_400));
    let secs = unix.rem_euclid(86_400);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        secs / 3600,
        secs % 3600 / 60,
        secs % 60
    )
}

const CONNECT: Duration = Duration::from_secs(15);
const GH_WALL_CLOCK: Duration = Duration::from_secs(120);

/// The production host: every call is a script over SSH (or a local
/// shell), each value quoted.
pub struct SshReviewHost {
    pub store: Arc<Mutex<Store>>,
    pub ssh: Arc<SshClient>,
}

impl SshReviewHost {
    async fn sh(&self, host: &str, script: &str) -> Result<std::process::Output, String> {
        crate::ssh::run_shell_bounded(&*self.ssh, host, script, CONNECT, GH_WALL_CLOCK)
            .await
            .map_err(|e| e.message)
    }
}

/// PURE: `gh pr list --json …`'s answer as open pull requests.
pub fn parse_pr_list(stdout: &str) -> Result<Vec<plan::OpenPr>, String> {
    let v: serde_json::Value =
        serde_json::from_str(stdout.trim()).map_err(|e| format!("gh pr list: {e}"))?;
    let arr = v.as_array().ok_or("gh pr list: not an array")?;
    Ok(arr
        .iter()
        .filter_map(|p| {
            let s = |k: &str| {
                p.get(k)
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string)
            };
            Some(plan::OpenPr {
                number: p.get("number")?.as_i64().filter(|n| *n > 0)?,
                title: s("title").unwrap_or_default(),
                head_sha: s("headRefOid").filter(|h| !h.is_empty())?,
                is_draft: p
                    .get("isDraft")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
                base_ref: s("baseRefName").unwrap_or_default(),
                head_ref: s("headRefName").unwrap_or_default(),
                author: p
                    .get("author")
                    .and_then(|a| a.get("login"))
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
            })
        })
        .collect())
}

fn gh_error(out: &std::process::Output) -> String {
    let err = String::from_utf8_lossy(&out.stderr);
    let line = err
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("gh failed");
    line.trim().chars().take(300).collect()
}

#[async_trait::async_trait]
impl ReviewHost for SshReviewHost {
    async fn open_prs(&self, host: &str, repo: &str) -> Result<Vec<plan::OpenPr>, String> {
        let out = self
            .sh(
                host,
                &format!(
                    "gh pr list --repo {} --state open --limit 1000 \
                     --json number,title,headRefOid,isDraft,baseRefName,headRefName,author",
                    quote(repo)
                ),
            )
            .await?;
        if !out.status.success() {
            return Err(gh_error(&out));
        }
        parse_pr_list(&String::from_utf8_lossy(&out.stdout))
    }

    async fn pr_state(&self, host: &str, repo: &str, number: i64) -> Result<String, String> {
        let out = self
            .sh(
                host,
                &format!(
                    "gh pr view {number} --repo {} --json state --jq .state",
                    quote(repo)
                ),
            )
            .await?;
        if !out.status.success() {
            return Err(gh_error(&out));
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    async fn diff(&self, host: &str, repo: &str, number: i64) -> Result<String, String> {
        let out = self
            .sh(host, &format!("gh pr diff {number} --repo {}", quote(repo)))
            .await?;
        if !out.status.success() {
            return Err(gh_error(&out));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    async fn claude_ignore(&self, host: &str, repo: &str, base: &str) -> Option<String> {
        use base64::Engine as _;
        let out = self
            .sh(
                host,
                &format!(
                    "gh api -X GET {} -f ref={} --jq .content",
                    quote(&format!("repos/{repo}/contents/.claudeignore")),
                    quote(base)
                ),
            )
            .await
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let b64: String = String::from_utf8_lossy(&out.stdout)
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        let bytes = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;
        String::from_utf8(bytes).ok()
    }

    async fn checkout(&self, host: &str, project_id: i64) -> Result<String, IpcError> {
        crate::service::sessions::project_checkout_on(&self.store, &self.ssh, host, project_id)
            .await
    }

    async fn prepare(
        &self,
        host: &str,
        root: &str,
        job_id: i64,
        number: i64,
        sha: &str,
    ) -> Result<(Prepared, String), IpcError> {
        workspace::prepare(
            &self.ssh,
            host,
            root,
            job_id,
            &Source::PullHead(number as u64),
            Some(sha),
        )
        .await
    }

    async fn run(
        &self,
        host: &str,
        run: &PrintRun,
        prompt: &str,
    ) -> Result<PrintOutcome, IpcError> {
        vehicle::run_print(&self.ssh, host, run, prompt).await
    }

    async fn cleanup(&self, host: &str, root: &str, job_id: i64) {
        workspace::cleanup(&self.ssh, host, root, job_id).await
    }

    async fn prune(&self, host: &str, root: &str) {
        workspace::prune(&self.ssh, host, root).await
    }

    async fn post_review(&self, req: &post::ReviewRequest, event: &str) -> Result<i64, String> {
        let script = format!(
            "gh api --method POST {} --input - --jq .id",
            quote(&format!("repos/{}/pulls/{}/reviews", req.repo, req.number))
        );
        let out = self
            .ssh
            .run_with_stdin(
                &req.host,
                &["bash", "-lc", &quote(&script)],
                post::api_body(req, event).into_bytes(),
                CONNECT,
                GH_WALL_CLOCK,
                64 * 1024,
            )
            .await
            .map_err(|e| e.message)?;
        if !out.status.success() {
            return Err(gh_error(&out));
        }
        String::from_utf8_lossy(&out.stdout)
            .trim()
            .parse()
            .map_err(|_| "GitHub answered no review id".to_string())
    }

    async fn review_state(
        &self,
        host: &str,
        repo: &str,
        number: i64,
        review_id: i64,
    ) -> Result<Option<String>, String> {
        let out = self
            .sh(
                host,
                &format!(
                    "gh api {} --jq .state",
                    quote(&format!("repos/{repo}/pulls/{number}/reviews/{review_id}"))
                ),
            )
            .await?;
        if !out.status.success() {
            let err = gh_error(&out);
            if err.contains("Not Found") || err.contains("404") {
                return Ok(None);
            }
            return Err(err);
        }
        Ok(Some(
            String::from_utf8_lossy(&out.stdout).trim().to_string(),
        ))
    }

    async fn comments_since(
        &self,
        host: &str,
        repo: &str,
        since: &str,
    ) -> Result<Vec<PrComment>, String> {
        let out = self
            .sh(
                host,
                &format!(
                    "gh api -X GET {} -f since={} -f per_page=100 --paginate \
                     --jq '.[] | {{id, body, issue_url, user_type: .user.type}}'",
                    quote(&format!("repos/{repo}/issues/comments")),
                    quote(since)
                ),
            )
            .await?;
        if !out.status.success() {
            return Err(gh_error(&out));
        }
        Ok(parse_comments(&String::from_utf8_lossy(&out.stdout)))
    }
}

/// The reviewer as the runtime registers it.
pub fn agent(store: &Arc<Mutex<Store>>, ssh: &Arc<SshClient>) -> AgentImpl {
    let host: Arc<dyn ReviewHost> = Arc::new(SshReviewHost {
        store: Arc::clone(store),
        ssh: Arc::clone(ssh),
    });
    AgentImpl {
        name: "review",
        poller: Some(Box::new(ReviewPoller {
            store: Arc::clone(store),
            host: Arc::clone(&host),
        })),
        runner: Some(Box::new(ReviewRunner {
            store: Arc::clone(store),
            host: Arc::clone(&host),
        })),
        executor: Some(Box::new(ReviewExecutor {
            store: Arc::clone(store),
            host,
        })),
    }
}

pub struct ReviewPoller {
    pub store: Arc<Mutex<Store>>,
    pub host: Arc<dyn ReviewHost>,
}

#[async_trait::async_trait]
impl AgentPoller for ReviewPoller {
    async fn poll(
        &self,
        rule: &AgentRuleRow,
        cursor: Option<&AgentCursorRow>,
        now: i64,
    ) -> Result<Polled, String> {
        let repo = repo_of_scope(&rule.scope).ok_or("not a review scope")?;
        let cfg = ReviewConfig::parse(&rule.config)?;
        let open = self.host.open_prs(&rule.host_alias, repo).await?;
        let gate = plan::Gate {
            skip_drafts: cfg.skip_drafts,
            skip_wip: cfg.skip_wip,
            debounce_secs: cfg.debounce_secs,
        };
        let p = {
            let s = lock(&self.store).map_err(|e| e.message)?;
            let existing = s.list_review_prs(repo).map_err(|e| e.to_string())?;
            let p = plan::plan(rule.id, repo, &open, &existing, gate, now);
            for row in &p.rows {
                s.put_review_pr(row).map_err(|e| e.to_string())?;
            }
            p
        };
        for number in &p.to_check {
            let state = match self.host.pr_state(&rule.host_alias, repo, *number).await {
                Ok(st) => st,
                Err(e) => {
                    tracing::warn!(repo, number, error = %e, "[agents] review: PR state unknown");
                    continue;
                }
            };
            let status = match state.as_str() {
                "MERGED" => "merged",
                "CLOSED" => "closed",
                _ => continue,
            };
            let s = lock(&self.store).map_err(|e| e.message)?;
            if let Ok(Some(mut row)) = s.review_pr(repo, *number) {
                row.status = status.into();
                row.closed_at = Some(now);
                row.updated_at = now;
                s.put_review_pr(&row).map_err(|e| e.to_string())?;
            }
        }
        let mut jobs = p.jobs;
        jobs.extend(self.verify(rule, repo, &cfg, now).await);
        let (forced, cursor) = self.comment_triggers(rule, repo, &cfg, cursor, now).await;
        jobs.extend(forced);
        Ok(Polled { jobs, cursor })
    }
}

/// Posted reviews checked per poll.
pub const MAX_VERIFIES: usize = 5;

/// Seconds a comment cursor reaches back past the last poll, for clocks
/// that disagree a little; the job keys keep a comment from counting twice.
const COMMENT_SLACK_SECS: i64 = 120;

impl ReviewPoller {
    /// Check posted reviews not seen for `verify_interval_mins`: one GitHub
    /// deleted or dismissed is reviewed again (a forced job).
    async fn verify(
        &self,
        rule: &AgentRuleRow,
        repo: &str,
        cfg: &ReviewConfig,
        now: i64,
    ) -> Vec<crate::store::NewAgentJob> {
        let due: Vec<crate::store::ReviewPrRow> = {
            let Ok(s) = self.store.lock() else {
                return Vec::new();
            };
            s.list_review_prs(repo)
                .unwrap_or_default()
                .into_iter()
                .filter(|r| {
                    r.status == "reviewed"
                        && r.review_id.is_some()
                        && r.review_verified_at.unwrap_or(0) + cfg.verify_interval_mins * 60 <= now
                })
                .take(MAX_VERIFIES)
                .collect()
        };
        let mut jobs = Vec::new();
        for mut row in due {
            let Some(review_id) = row.review_id else {
                continue;
            };
            let gone = match self
                .host
                .review_state(&rule.host_alias, repo, row.number, review_id)
                .await
            {
                Ok(None) => true,
                Ok(Some(state)) => state == "DISMISSED",
                Err(e) => {
                    tracing::warn!(repo, number = row.number, error = %e, "[agents] review: verify failed");
                    continue;
                }
            };
            row.review_verified_at = Some(now);
            if gone {
                row.review_id = None;
                row.status = "pending_review".into();
                jobs.push(crate::store::NewAgentJob {
                    rule_id: rule.id,
                    agent: "review".into(),
                    subject: plan::subject(repo, row.number),
                    trigger: "reverify".into(),
                    dedup_key: plan::forced_key(
                        repo,
                        row.number,
                        &row.head_sha,
                        &format!("r{review_id}"),
                    ),
                    not_before: now,
                });
            }
            if let Ok(s) = self.store.lock() {
                if let Err(e) = s.put_review_pr(&row) {
                    tracing::warn!(repo, number = row.number, error = %e, "[agents] review: verify not recorded");
                }
            }
        }
        jobs
    }

    /// The `/review` comments since the last poll, as forced jobs, and the
    /// cursor for the next poll. The first poll of a rule only sets the
    /// cursor: a comment from before the rule asks nothing of it.
    async fn comment_triggers(
        &self,
        rule: &AgentRuleRow,
        repo: &str,
        cfg: &ReviewConfig,
        cursor: Option<&AgentCursorRow>,
        now: i64,
    ) -> (Vec<crate::store::NewAgentJob>, Option<String>) {
        let next = Some(rfc3339(now - COMMENT_SLACK_SECS));
        let Ok(Some(trigger)) = cfg.trigger() else {
            return (Vec::new(), next);
        };
        let Some(since) = cursor.and_then(|c| c.cursor.clone()) else {
            return (Vec::new(), next);
        };
        let comments = match self
            .host
            .comments_since(&rule.host_alias, repo, &since)
            .await
        {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(repo, error = %e, "[agents] review: reading comments failed");
                // Keep the cursor: the next poll asks again from it.
                return (Vec::new(), Some(since));
            }
        };
        let Ok(s) = self.store.lock() else {
            return (Vec::new(), Some(since));
        };
        let jobs = comments
            .into_iter()
            .filter(|c| {
                c.user_type != "Bot"
                    && !c.body.contains(format::MARKER)
                    && !c.body.contains(format::LEGACY_MARKER)
                    && trigger.is_match(&c.body)
            })
            .filter_map(|c| {
                let row = s.review_pr(repo, c.number).ok().flatten()?;
                // A forced review still waits out a draft or a WIP title,
                // as the original's did.
                let held = matches!(row.skip_reason.as_deref(), Some("draft" | "wip_title"));
                (row.is_open() && !held).then(|| crate::store::NewAgentJob {
                    rule_id: rule.id,
                    agent: "review".into(),
                    subject: plan::subject(repo, c.number),
                    trigger: "comment".into(),
                    dedup_key: plan::forced_key(
                        repo,
                        c.number,
                        &row.head_sha,
                        &format!("c{}", c.id),
                    ),
                    not_before: now,
                })
            })
            .collect();
        (jobs, next)
    }
}

pub struct ReviewRunner {
    pub store: Arc<Mutex<Store>>,
    pub host: Arc<dyn ReviewHost>,
}

/// Output kept from a review run.
const OUTPUT_CAP: usize = 1 << 20;

fn skipped(reason: impl Into<String>) -> RunOutcome {
    RunOutcome::Skipped {
        reason: reason.into(),
    }
}

fn failed(kind: ErrorKind, message: impl Into<String>) -> RunOutcome {
    RunOutcome::Failed {
        kind,
        message: message.into(),
        cost_micros: 0,
    }
}

/// PURE: what a `gh pr diff` failure means: a diff GitHub will not serve
/// for its size is a skip, a missing pull request is permanent, the rest
/// as any failed call.
pub fn diff_failure(said: &str) -> Option<ErrorKind> {
    let s = said.to_lowercase();
    if s.contains("too large") || s.contains("too_large") || s.contains("maximum number of") {
        return None;
    }
    if s.contains("could not resolve") || s.contains("not found") || s.contains("404") {
        return Some(ErrorKind::Permanent);
    }
    Some(vehicle::classify_failure(said))
}

impl ReviewRunner {
    fn project_of(&self, rule: &AgentRuleRow, repo: &str) -> Option<i64> {
        if rule.project_id.is_some() {
            return rule.project_id;
        }
        let (owner, name) = repo.split_once('/')?;
        let s = self.store.lock().ok()?;
        s.list_projects()
            .ok()?
            .into_iter()
            .find(|p| p.owner == owner && p.repo == name && !p.system)
            .map(|p| p.id)
    }

    async fn review(&self, job: &AgentJobRow, rule: &AgentRuleRow) -> RunOutcome {
        let Some((repo, number)) = plan::parse_subject(&job.subject) else {
            return failed(
                ErrorKind::Permanent,
                format!("not a review subject: {}", job.subject),
            );
        };
        let Some(sha) = plan::sha_of_key(&job.dedup_key) else {
            return failed(ErrorKind::Permanent, "the job names no head");
        };
        let cfg = match ReviewConfig::parse(&rule.config) {
            Ok(c) => c,
            Err(e) => return failed(ErrorKind::Permanent, e),
        };
        let (row, previous) = {
            let Ok(s) = self.store.lock() else {
                return failed(ErrorKind::Transient, "store lock");
            };
            let row = s.review_pr(repo, number).ok().flatten();
            let previous = s.last_review_record(repo, number).ok().flatten();
            (row, previous)
        };
        let Some(row) = row else {
            return skipped("the pull request is not tracked");
        };
        if !row.is_open() {
            return skipped(format!("the pull request is {}", row.status));
        }
        if row.head_sha != sha {
            return skipped("the head moved on; a newer job reviews it");
        }
        // A forced review (`/review`, a vanished posted review) looks again
        // past a size skip; nothing looks past a draft or a WIP title.
        let forced = job.trigger == "comment" || job.trigger == "reverify";
        let size_skip = matches!(
            row.skip_reason.as_deref(),
            Some("diff_too_large" | "empty_diff")
        );
        if row.status == "skipped" && !(forced && size_skip) {
            return skipped(format!(
                "skipped: {}",
                row.skip_reason.as_deref().unwrap_or("held")
            ));
        }
        let host = rule.host_alias.as_str();

        // The diff, filtered and gated before anything is checked out.
        let raw = match self.host.diff(host, repo, number).await {
            Ok(d) => d,
            Err(e) => match diff_failure(&e) {
                None => {
                    self.skip(repo, number, sha, "diff_too_large", None);
                    return skipped("diff_too_large: GitHub will not serve the diff");
                }
                Some(kind) => return failed(kind, e),
            },
        };
        let ignore = if cfg.respect_claudeignore && !row.base_ref.is_empty() {
            self.host.claude_ignore(host, repo, &row.base_ref).await
        } else {
            None
        };
        let patterns = diff::exclude_patterns(&cfg.exclude_paths, ignore.as_deref());
        let (filtered, _) = diff::filter_diff(&raw, &patterns);
        if !filtered.split('\n').any(|l| l.starts_with("diff --git ")) {
            self.skip(repo, number, sha, "empty_diff", Some(0));
            return skipped("empty_diff: nothing left after the exclusions");
        }
        let lines = diff::count_diff_lines(&filtered);
        if lines > cfg.max_diff_lines {
            self.skip(repo, number, sha, "diff_too_large", Some(lines as i64));
            return skipped(format!(
                "diff_too_large: {lines} lines (max {})",
                cfg.max_diff_lines
            ));
        }

        // The workspace: exactly the head this job is about.
        let Some(project_id) = self.project_of(rule, repo) else {
            return failed(
                ErrorKind::Permanent,
                format!("no project {repo} on this hub; grant the rule a project"),
            );
        };
        let root = match self.host.checkout(host, project_id).await {
            Ok(r) => r,
            Err(e) => return failed(ErrorKind::Transient, e.message),
        };
        self.host.prune(host, &root).await;
        let cwd = match self.host.prepare(host, &root, job.id, number, sha).await {
            Ok((Prepared::Ready, cwd)) => cwd,
            Ok((Prepared::Moved, _)) => {
                return skipped("the head moved on; a newer job reviews it")
            }
            Ok((Prepared::NoRoot, _)) => {
                return failed(ErrorKind::Permanent, format!("no git checkout at {root}"))
            }
            Ok((other, _)) => return failed(ErrorKind::Transient, format!("workspace: {other:?}")),
            Err(e) => return failed(ErrorKind::Transient, e.message),
        };

        let previous = previous.and_then(|p| {
            let placed: output::PlacedReview = serde_json::from_str(&p.review).ok()?;
            Some(prompt::Previous {
                verdict: p.verdict,
                sha: p.sha,
                findings: placed.inline.into_iter().chain(placed.orphans).collect(),
            })
        });
        let previous_findings: Vec<output::Finding> = previous
            .as_ref()
            .map(|p| p.findings.clone())
            .unwrap_or_default();
        let security: Vec<String> = diff::diff_paths(&filtered)
            .into_iter()
            .filter(|p| cfg.security_paths.iter().any(|g| diff::glob_match(g, p)))
            .collect();
        let user_prompt = prompt::build(&prompt::PromptInput {
            title: &row.title,
            diff: &filtered,
            previous: previous.as_ref(),
            focus_paths: &cfg.focus_paths,
            security_paths: &security,
            require_tests: cfg.require_tests,
            test_blocking_importance: &cfg.test_blocking_importance,
        });
        let run = PrintRun {
            cwd,
            tools: vehicle::READ_ONLY_TOOLS
                .iter()
                .map(|t| t.to_string())
                .collect(),
            allowed_tools: Vec::new(),
            mcp_config: None,
            system_prompt: Some(prompt::SKILL.to_string()),
            model: Some(cfg.model.clone()).filter(|m| !m.is_empty()),
            max_turns: cfg.max_turns,
            timeout_secs: cfg.timeout_secs,
            output_cap: OUTPUT_CAP,
        };
        let outcome = self.host.run(host, &run, &user_prompt).await;
        self.host.cleanup(host, &root, job.id).await;
        let (answer, cost) = match outcome {
            Ok(PrintOutcome::Answered { text, cost_micros }) => (text, cost_micros),
            Ok(PrintOutcome::Failed {
                kind,
                message,
                cost_micros,
            }) => {
                return RunOutcome::Failed {
                    kind,
                    message,
                    cost_micros,
                }
            }
            Err(e) => return failed(vehicle::classify_failure(&e.message), e.message),
        };
        let Some(review) = output::parse_review(&answer) else {
            let head: String = answer.chars().take(300).collect();
            return RunOutcome::Failed {
                kind: ErrorKind::Permanent,
                message: format!("the answer is not a review: {head}"),
                cost_micros: cost,
            };
        };
        let placed = output::place(
            review,
            &diff::commentable_lines(&filtered),
            cfg.confidence_threshold,
        );
        let placed_json = match serde_json::to_string(&placed) {
            Ok(j) => j,
            Err(e) => return failed(ErrorKind::Permanent, e.to_string()),
        };
        let record_id = {
            let Ok(s) = self.store.lock() else {
                return failed(ErrorKind::Transient, "store lock");
            };
            match s.record_review(
                &NewReviewRecord {
                    repo,
                    number,
                    sha,
                    job_id: Some(job.id),
                    verdict: &placed.verdict,
                    review: &placed_json,
                },
                crate::store::now_unix(),
            ) {
                Ok(id) => id,
                Err(e) => return failed(ErrorKind::Transient, e.to_string()),
            }
        };
        let req = post::build(&post::BuildInput {
            placed: &placed,
            previous: &previous_findings,
            host,
            repo,
            number,
            sha,
            record_id,
            request_changes_event: cfg.request_changes_event,
        });
        let mut result = serde_json::json!({
            "record_id": record_id,
            "verdict": placed.verdict,
            "event": req.event,
            "summary": placed.summary,
            "inline": placed.inline.len(),
            "orphans": placed.orphans.len(),
            "dropped": placed.dropped,
        });
        if rule.level == "watch" {
            return RunOutcome::Done {
                result: Some(result.to_string()),
                cost_micros: cost,
            };
        }
        let payload = match serde_json::to_string(&req) {
            Ok(p) => p,
            Err(e) => return failed(ErrorKind::Permanent, e.to_string()),
        };
        let now = crate::store::now_unix();
        let queued = {
            let Ok(s) = self.store.lock() else {
                return failed(ErrorKind::Transient, "store lock");
            };
            if rule.level == "act" {
                s.insert_approved_agent_action(job.id, post::POST_REVIEW, &payload, "rule:act", now)
            } else {
                s.insert_agent_action(job.id, post::POST_REVIEW, &payload, now)
            }
        };
        let action_id = match queued {
            Ok(id) => id,
            Err(e) => return failed(ErrorKind::Transient, e.to_string()),
        };
        result["action_id"] = action_id.into();
        if rule.level == "act" {
            RunOutcome::Done {
                result: Some(result.to_string()),
                cost_micros: cost,
            }
        } else {
            RunOutcome::NeedsPerson {
                result: Some(result.to_string()),
                reason: format!("review ready: approve action {action_id} to post it"),
                cost_micros: cost,
            }
        }
    }

    fn skip(&self, repo: &str, number: i64, sha: &str, reason: &str, lines: Option<i64>) {
        if let Ok(s) = self.store.lock() {
            if let Err(e) =
                s.skip_review_pr(repo, number, sha, reason, lines, crate::store::now_unix())
            {
                tracing::warn!(repo, number, error = %e, "[agents] review: recording a skip failed");
            }
        }
    }
}

#[async_trait::async_trait]
impl AgentRunner for ReviewRunner {
    async fn run(&self, job: &AgentJobRow, rule: Option<&AgentRuleRow>) -> RunOutcome {
        match rule {
            None => skipped("the rule was revoked"),
            Some(rule) => self.review(job, rule).await,
        }
    }
}

/// Posts an approved `post_review` action: unless the pull request closed
/// or its head moved on since the review (expired), as the action's event,
/// once more as a comment when GitHub refuses the event because the poster
/// authored the pull request.
pub struct ReviewExecutor {
    pub store: Arc<Mutex<Store>>,
    pub host: Arc<dyn ReviewHost>,
}

#[async_trait::async_trait]
impl super::AgentExecutor for ReviewExecutor {
    async fn execute(&self, action: &crate::store::AgentActionRow) -> super::ActionResult {
        use super::ActionResult;
        if action.kind != post::POST_REVIEW {
            return ActionResult::Failed(format!("the reviewer does not do {}", action.kind));
        }
        let req: post::ReviewRequest = match serde_json::from_str(&action.payload) {
            Ok(r) => r,
            Err(e) => return ActionResult::Failed(format!("payload: {e}")),
        };
        let row = match self.store.lock() {
            Ok(s) => s.review_pr(&req.repo, req.number).ok().flatten(),
            Err(_) => return ActionResult::Failed("store lock".into()),
        };
        match row {
            None => return ActionResult::Expired("the pull request is not tracked".into()),
            Some(r) if !r.is_open() => {
                return ActionResult::Expired(format!("the pull request is {}", r.status))
            }
            Some(r) if r.head_sha != req.sha => {
                return ActionResult::Expired("the head moved on since the review".into())
            }
            Some(_) => {}
        }
        let posted = match self.host.post_review(&req, &req.event).await {
            Err(e) if req.event != "COMMENT" && post::is_self_review_refusal(&e) => {
                self.host.post_review(&req, "COMMENT").await
            }
            other => other,
        };
        let review_id = match posted {
            Ok(id) => id,
            Err(e) => return ActionResult::Failed(e),
        };
        if let Ok(s) = self.store.lock() {
            if let Err(e) = s.mark_review_posted(req.record_id, review_id, crate::store::now_unix())
            {
                tracing::warn!(record = req.record_id, error = %e, "[agents] review: posted, not recorded");
            }
        }
        ActionResult::Executed
    }
}
