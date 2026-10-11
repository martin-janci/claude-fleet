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
//! Step 3 records the review only, whatever the rule's level: posting it
//! (`propose` queues it for a person, `act` posts it) is step 4.
//!
//! Every call on the host goes through [`ReviewHost`], so the poller and the
//! runner are tested against a fake one.

pub mod diff;
pub mod output;
pub mod plan;
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
        Ok(c)
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
        _cursor: Option<&AgentCursorRow>,
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
        Ok(Polled {
            jobs: p.jobs,
            cursor: None,
        })
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
        if row.status == "skipped" {
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
        RunOutcome::Done {
            result: Some(
                serde_json::json!({
                    "record_id": record_id,
                    "verdict": placed.verdict,
                    "summary": placed.summary,
                    "inline": placed.inline.len(),
                    "orphans": placed.orphans.len(),
                    "dropped": placed.dropped,
                })
                .to_string(),
            ),
            cost_micros: cost,
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
