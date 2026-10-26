//! A configurable agent's definition (`kind: fleet_agent` in the asset
//! catalog), typed, and its validator. Design:
//! `docs/superpowers/specs/2026-10-12-configurable-agents-design.md`, A–E.
//!
//! [`parse`] reads the definition's fields (the asset's YAML without its
//! catalog header) strictly, unknown keys refused; [`problems`] checks what
//! serde cannot: names, sizes, which keys a trigger source or an action
//! kind takes, that every template path and condition names something that
//! exists, that a run's tools are read-only and an `mcp_call` names a write
//! tool its store declares. Every problem says where it is, in the pages'
//! style: `actions › learn: tool add_known_error is not a write_tool of
//! store kb`.

use super::answer;
use super::template::{self, Path};
use crate::pages::forms::{
    self, FieldCondition, FieldType, FormField, FormOption, FormSpec, FormStep,
};
use crate::service::agents::vehicle;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const MAX_INPUTS: usize = 30;
pub const MAX_STORES: usize = 8;
pub const MAX_ACTIONS: usize = 12;
pub const MAX_CONTEXT: usize = 8;
/// The asset's YAML, at most.
pub const MAX_YAML_BYTES: usize = 64 * 1024;
/// A template or a body, at most.
const MAX_TEXT: usize = 20_000;

/// The catalog header's keys: everything else in `asset.yaml` is the definition.
pub const HEADER_KEYS: [&str; 9] = [
    "kind",
    "name",
    "version",
    "description",
    "tags",
    "source",
    "install_as",
    "scope",
    "targets",
];

/// A definition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Definition {
    #[serde(default)]
    pub inputs: Vec<Input>,
    #[serde(default)]
    pub stores: Vec<StoreRole>,
    pub trigger: Trigger,
    #[serde(default)]
    pub run: Run,
    #[serde(default)]
    pub actions: Vec<Action>,
    #[serde(default)]
    pub limits: Limits,
}

/// An input's type: a form field's (no `secret`: secrets belong to store
/// bindings) or a reference to something fleet knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputType {
    Text,
    Textarea,
    Number,
    Bool,
    Select,
    Multiselect,
    /// A configured tracker (its id).
    Tracker,
    /// A project (`owner/repo`).
    Project,
    /// A host (its alias).
    Host,
}

impl InputType {
    pub fn is_reference(self) -> bool {
        matches!(
            self,
            InputType::Tracker | InputType::Project | InputType::Host
        )
    }
}

/// One input: a variable an instance sets, asked in its grant form.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: InputType,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
    #[serde(default)]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<FieldCondition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_len: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    #[serde(default)]
    pub integer: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<FormOption>>,
    #[serde(default)]
    pub other: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StoreKind {
    /// A catalog MCP server, bound per instance with its own secrets.
    Mcp,
    /// A namespace of fleet's own records (`agent_records`).
    Fleet,
}

/// A write tool of an MCP store: its name, or `{name, auto_ok}` when the
/// definition allows it to run at `act` without a person.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum WriteTool {
    Name(String),
    Full {
        name: String,
        #[serde(default)]
        auto_ok: bool,
    },
}

impl WriteTool {
    pub fn name(&self) -> &str {
        match self {
            WriteTool::Name(n) | WriteTool::Full { name: n, .. } => n,
        }
    }
    pub fn auto_ok(&self) -> bool {
        matches!(self, WriteTool::Full { auto_ok: true, .. })
    }
}

/// A store the definition needs; an instance binds it to a concrete one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoreRole {
    pub role: String,
    pub kind: StoreKind,
    pub label: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub read_tools: Vec<String>,
    #[serde(default)]
    pub write_tools: Vec<WriteTool>,
    /// fleet: bind a shared namespace by default rather than a private one.
    #[serde(default)]
    pub shared_by_default: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerSource {
    TrackerComments,
    TrackerAssigned,
    TrackerQuery,
    GithubPrs,
    GithubComments,
    /// A person's reply in a review thread a fleet agent started (its line
    /// comment, usually a reviewer instance's): the PR reviewer's test debate.
    GithubReviewReplies,
    McpScan,
    Schedule,
    Manual,
}

impl TriggerSource {
    pub fn word(self) -> &'static str {
        match self {
            TriggerSource::TrackerComments => "tracker_comments",
            TriggerSource::TrackerAssigned => "tracker_assigned",
            TriggerSource::TrackerQuery => "tracker_query",
            TriggerSource::GithubPrs => "github_prs",
            TriggerSource::GithubComments => "github_comments",
            TriggerSource::GithubReviewReplies => "github_review_replies",
            TriggerSource::McpScan => "mcp_scan",
            TriggerSource::Schedule => "schedule",
            TriggerSource::Manual => "manual",
        }
    }

    pub fn is_github(self) -> bool {
        matches!(
            self,
            TriggerSource::GithubPrs
                | TriggerSource::GithubComments
                | TriggerSource::GithubReviewReplies
        )
    }

    /// The trigger keys this source takes (besides `source`), and which of
    /// them it needs.
    fn keys(self) -> (&'static [&'static str], &'static [&'static str]) {
        match self {
            // The query is JQL, so a poll of Jira only: on GitHub, Linear or
            // Asana it is left out (`runner`).
            TriggerSource::TrackerComments => {
                (&["tracker", "query", "match"], &["tracker", "match"])
            }
            TriggerSource::TrackerAssigned => {
                (&["tracker", "query", "match"], &["tracker", "match"])
            }
            TriggerSource::TrackerQuery => (&["tracker", "query"], &["tracker", "query"]),
            TriggerSource::GithubPrs => (
                &["repo", "skip_wip", "debounce_secs", "reverify_mins"],
                &["repo"],
            ),
            TriggerSource::GithubComments => (&["repo", "match"], &["repo", "match"]),
            TriggerSource::GithubReviewReplies => (&["repo", "match", "max_rounds"], &["repo"]),
            TriggerSource::McpScan => (
                &[
                    "store",
                    "tool",
                    "arguments",
                    "every_mins",
                    "key_template",
                    "window_mins",
                    "max_jobs",
                ],
                &["store", "tool", "every_mins", "key_template"],
            ),
            TriggerSource::Schedule => (&["cron"], &["cron"]),
            TriggerSource::Manual => (&[], &[]),
        }
    }

    /// The fields of the subject a job of this source carries.
    pub fn subject_fields(self) -> &'static [&'static str] {
        const ISSUE: &[&str] = &[
            "tracker",
            "key",
            "summary",
            "description",
            "status",
            "assignee",
            "url",
        ];
        match self {
            TriggerSource::TrackerComments => &[
                "tracker",
                "key",
                "summary",
                "description",
                "status",
                "assignee",
                "url",
                "comment",
            ],
            TriggerSource::TrackerAssigned | TriggerSource::TrackerQuery => ISSUE,
            TriggerSource::GithubPrs => &[
                "repo", "number", "title", "head_sha", "base_ref", "head_ref", "author", "url",
                "previous", "paths",
            ],
            TriggerSource::GithubComments => &[
                "repo", "number", "title", "url", "comment", "head_sha", "base_ref", "head_ref",
                "previous", "paths",
            ],
            TriggerSource::GithubReviewReplies => &[
                "repo", "number", "title", "url", "comment", "thread", "head_sha", "base_ref",
                "head_ref", "previous", "paths",
            ],
            TriggerSource::McpScan => &["key", "entry"],
            TriggerSource::Schedule => &["at", "request", "scope"],
            TriggerSource::Manual => &["request", "scope"],
        }
    }
}

/// What a comment must carry to trigger (`tracker_comments`,
/// `github_comments`), or who an issue must be assigned to
/// (`tracker_assigned`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Match {
    /// Comma-separated phrases (a template: usually an input); any one, any case.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub any_phrase: Option<String>,
    /// A regex, matched per line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regex: Option<String>,
    /// The tracker account id (a template).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,
}

/// The trigger: a source from a fixed set, configured by templates over the
/// inputs. Which keys a source takes is [`TriggerSource::keys`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Trigger {
    pub source: TriggerSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracker: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    #[serde(default, rename = "match", skip_serializing_if = "Option::is_none")]
    pub match_: Option<Match>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub store: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub every_mins: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_template: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cron: Option<String>,
    /// `mcp_scan`: the tool is called with `from` / `to` (unix seconds)
    /// covering the last `window_mins`, a little more than `every_mins`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_mins: Option<i64>,
    /// `mcp_scan`: new entries one scan turns into jobs, at most
    /// (default and most [`MAX_SCAN_JOBS`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_jobs: Option<usize>,
    /// `github_prs`: no job for a pull request whose title starts with WIP.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub skip_wip: bool,
    /// `github_prs`: a new head's job waits this long, so a burst of pushes
    /// is reviewed once, at its last head (0-3600).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub debounce_secs: Option<i64>,
    /// `github_prs`: every this many minutes the instance's last review of
    /// an open pull request is looked up, and one GitHub deleted or
    /// dismissed is made again at the same head (1-1440; unset is never).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reverify_mins: Option<i64>,
    /// `github_review_replies`: the instance answers a thread at most this
    /// many times (1-5, default [`DEFAULT_MAX_ROUNDS`]); then it is the
    /// people's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_rounds: Option<usize>,
}

/// `github_review_replies`: replies per thread when `max_rounds` is unset.
pub const DEFAULT_MAX_ROUNDS: usize = 2;

/// Most new entries one scan turns into jobs.
pub const MAX_SCAN_JOBS: usize = 10;

/// `mcp_scan`: minutes between scans when `every_mins` is unset.
pub const DEFAULT_SCAN_EVERY_MINS: i64 = 60;

impl Trigger {
    /// The keys set, by name.
    fn set_keys(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        let mut put = |k: &'static str, set: bool| {
            if set {
                out.push(k)
            }
        };
        put("tracker", self.tracker.is_some());
        put("query", self.query.is_some());
        put("match", self.match_.is_some());
        put("repo", self.repo.is_some());
        put("store", self.store.is_some());
        put("tool", self.tool.is_some());
        put("arguments", self.arguments.is_some());
        put("every_mins", self.every_mins.is_some());
        put("key_template", self.key_template.is_some());
        put("cron", self.cron.is_some());
        put("window_mins", self.window_mins.is_some());
        put("max_jobs", self.max_jobs.is_some());
        put("skip_wip", self.skip_wip);
        put("debounce_secs", self.debounce_secs.is_some());
        put("reverify_mins", self.reverify_mins.is_some());
        put("max_rounds", self.max_rounds.is_some());
        out
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Workspace {
    /// A detached worktree of the instance's project.
    #[default]
    Project,
    /// The host's home directory.
    Home,
    /// A detached worktree of the project at the pull request's head the
    /// job is about (`github_prs`, or a `github_comments` comment on an
    /// open PR): the head moving on skips the run.
    PrHead,
}

impl Workspace {
    /// Whether runs read the instance's project (it needs one).
    pub fn needs_project(self) -> bool {
        matches!(self, Workspace::Project | Workspace::PrHead)
    }
}

/// `run.diff`: the pull request's diff in the prompt, filtered as the PR
/// reviewer filters it (`agents::review::diff`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiffSpec {
    /// Paths left out (globs, as the reviewer's `exclude_paths`).
    #[serde(default)]
    pub exclude_paths: Vec<String>,
    /// Also leave out what the repo's `.claudeignore` (on the base) names.
    #[serde(default = "yes")]
    pub respect_claudeignore: bool,
    /// A diff longer than this (after the exclusions) skips the run.
    #[serde(default = "default_diff_lines")]
    pub max_lines: usize,
}

fn yes() -> bool {
    true
}
fn default_diff_lines() -> usize {
    3000
}

/// Longest diff a definition may take into its prompt.
pub const MAX_DIFF_LINES: usize = 20_000;

/// A lookup made before the run, its result put into the prompt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextLookup {
    /// A fleet store role.
    pub store: String,
    /// One record by key (a template).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// Or the newest N records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recent: Option<u32>,
}

fn default_max_turns() -> u32 {
    40
}
fn default_timeout() -> u64 {
    900
}
fn default_tools() -> Vec<String> {
    vehicle::READ_ONLY_TOOLS
        .iter()
        .map(|t| t.to_string())
        .collect()
}

/// The run: the locked-down `claude -p` and the answer it gives.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Run {
    #[serde(default)]
    pub workspace: Workspace,
    #[serde(default = "default_tools")]
    pub tools: Vec<String>,
    #[serde(default = "default_max_turns")]
    pub max_turns: u32,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    /// `--model`; absent is the login's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The answer's JSON Schema (`answer.rs`'s subset).
    #[serde(default)]
    pub output_schema: Value,
    #[serde(default)]
    pub context: Vec<ContextLookup>,
    /// This definition's instances running at once, at most.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_parallel: Option<u32>,
    /// Give the run `fleet-diagnostics`, the hub's read-only server over the
    /// fleet's own logs, error reports, runs and host health
    /// (`diagnostics.rs`): the self debug agent's eyes.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub diagnostics: bool,
    /// A subject a run of this instance finished less than this many
    /// seconds ago waits until that many seconds have passed (at most a
    /// week; 0 is none).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub cooldown_secs: i64,
    /// `project`: the branch the worktree is made from (a template over
    /// the inputs; default origin's default branch).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// The pull request's diff in the prompt (a GitHub trigger).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff: Option<DiffSpec>,
    /// The fingerprint of the error the answer names, and what fleet knows
    /// of it (`fingerprints.rs`): added to the answer before the actions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<FingerprintSpec>,
}

/// `run.fingerprint`: the support agent's fingerprint of an error,
/// `sha256(class | normalised message | first 5 app frames)`, kept in a
/// fleet store with how often it was seen and the bug filed for it. Its
/// findings join the answer as [`FINGERPRINT_FIELDS`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FingerprintSpec {
    /// The fleet store role that keeps the fingerprints.
    pub store: String,
    /// The exception class (a template over the answer); empty: no
    /// fingerprint this run.
    pub class: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stack: Option<String>,
    /// The affected component (a template over the answer): an open bug's
    /// summary naming it scores 30 of the 100.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component: Option<String>,
    /// The frames that are the application's contain this (a template over
    /// the inputs); empty: every `at …` line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_frame_prefix: Option<String>,
    /// A bug filed for the fingerprint counts while it was seen this
    /// recently (`knownBug`); 0 is always.
    #[serde(default = "default_incident_window")]
    pub window_mins: i64,
    /// Search the tracker's open bugs for one like it (Jira).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bugs: Option<BugSearch>,
}

fn default_incident_window() -> i64 {
    120
}

/// `run.fingerprint.bugs`: where open bugs are looked for.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BugSearch {
    /// The tracker (a template over the inputs); default the trigger's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracker: Option<String>,
    /// The project (a template over the inputs).
    pub project: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue_type: Option<String>,
}

/// What `run.fingerprint` adds to the answer, with its JSON type:
/// `fingerprint` (hex), `seenCount` (this run included), `knownBug` (the bug
/// filed for it, seen inside the window), `similarBug` (an open bug scoring
/// at least 80), `relatedBug` (at least 60).
pub const FINGERPRINT_FIELDS: [(&str, &str); 5] = [
    ("fingerprint", "string"),
    ("seenCount", "integer"),
    ("knownBug", "string"),
    ("similarBug", "string"),
    ("relatedBug", "string"),
];

/// PURE: the answer's schema as the actions see it: `output_schema`, and
/// [`FINGERPRINT_FIELDS`] when the run has a fingerprint.
pub fn actions_schema(r: &Run) -> Value {
    let mut schema = r.output_schema.clone();
    if r.fingerprint.is_some() {
        if let Some(props) = schema.get_mut("properties").and_then(Value::as_object_mut) {
            for (name, ty) in FINGERPRINT_FIELDS {
                props
                    .entry(name.to_string())
                    .or_insert_with(|| serde_json::json!({ "type": ty }));
            }
        }
    }
    schema
}

fn is_zero(n: &i64) -> bool {
    *n == 0
}

impl Default for Run {
    fn default() -> Self {
        Run {
            workspace: Workspace::default(),
            tools: default_tools(),
            max_turns: default_max_turns(),
            timeout_secs: default_timeout(),
            model: None,
            output_schema: Value::Null,
            context: Vec::new(),
            max_parallel: None,
            diagnostics: false,
            cooldown_secs: 0,
            branch: None,
            diff: None,
            fingerprint: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionKind {
    TrackerComment,
    TrackerIssue,
    TrackerLink,
    PrReview,
    PrComment,
    PrLabels,
    /// The pull request's description, when it has none (or, with
    /// `overwrite`, on the instance's first review of it).
    PrDescription,
    /// A reply in the review thread the job is about
    /// (`github_review_replies`), resolving it with `resolve`.
    PrReply,
    McpCall,
    Record,
    /// A task in fleet's Work view, optionally with proposed subtasks.
    WorkTask,
    /// A Claude session started on a project to fix something.
    FixSession,
    /// One fleet setting changed (only keys an agent may propose).
    Setting,
    /// One session restored (lost) or restarted (stuck).
    SessionRecover,
}

impl ActionKind {
    pub fn word(self) -> &'static str {
        match self {
            ActionKind::TrackerComment => "tracker_comment",
            ActionKind::TrackerIssue => "tracker_issue",
            ActionKind::TrackerLink => "tracker_link",
            ActionKind::PrReview => "pr_review",
            ActionKind::PrComment => "pr_comment",
            ActionKind::PrLabels => "pr_labels",
            ActionKind::PrDescription => "pr_description",
            ActionKind::PrReply => "pr_reply",
            ActionKind::McpCall => "mcp_call",
            ActionKind::Record => "record",
            ActionKind::WorkTask => "work_task",
            ActionKind::FixSession => "fix_session",
            ActionKind::Setting => "setting",
            ActionKind::SessionRecover => "session_recover",
        }
    }

    /// The action keys this kind takes (besides id, when, do, auto, stop),
    /// and which of them it needs. `body` / `body_template` are one choice.
    fn keys(self) -> (&'static [&'static str], &'static [&'static str]) {
        match self {
            ActionKind::TrackerComment => (
                &["tracker", "target", "body", "body_template"],
                &["target", "body"],
            ),
            ActionKind::TrackerIssue => (
                &[
                    "tracker",
                    "project",
                    "issue_type",
                    "summary",
                    "body",
                    "body_template",
                    "labels",
                    "link",
                    "fingerprint",
                ],
                &["project", "summary", "body"],
            ),
            ActionKind::TrackerLink => (
                &["tracker", "from", "to", "link_type"],
                &["from", "to", "link_type"],
            ),
            ActionKind::PrReview => (
                &[
                    "target",
                    "body",
                    "body_template",
                    "verdict",
                    "findings",
                    "min_confidence",
                    "resolutions",
                ],
                &["target", "body"],
            ),
            ActionKind::PrDescription => (
                &["target", "body", "body_template", "overwrite"],
                &["target", "body"],
            ),
            ActionKind::PrReply => (
                &["target", "body", "body_template", "resolve"],
                &["target", "body"],
            ),
            ActionKind::PrComment => (&["target", "body", "body_template"], &["target", "body"]),
            ActionKind::PrLabels => (&["target", "labels"], &["target", "labels"]),
            ActionKind::McpCall => (
                &["store", "tool", "arguments"],
                &["store", "tool", "arguments"],
            ),
            ActionKind::Record => (
                &["store", "key", "value", "expires_secs"],
                &["store", "key", "value"],
            ),
            ActionKind::WorkTask => (
                &["summary", "body", "body_template", "steps", "fingerprint"],
                &["summary"],
            ),
            ActionKind::FixSession => (
                &[
                    "project",
                    "host",
                    "summary",
                    "body",
                    "body_template",
                    "fingerprint",
                ],
                &["project", "summary", "body"],
            ),
            ActionKind::Setting => (
                &["key", "value", "body", "body_template", "fingerprint"],
                &["key", "value", "body"],
            ),
            ActionKind::SessionRecover => (
                &["target", "host", "body", "body_template", "fingerprint"],
                &["target", "body"],
            ),
        }
    }
}

/// What an issue is linked to after it is created.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IssueLink {
    /// The other issue (a template).
    pub to: String,
    pub link_type: String,
}

/// One action: what an answer may turn into.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Action {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<FieldCondition>,
    #[serde(rename = "do")]
    pub kind: ActionKind,
    /// Runs at `act` without a person.
    #[serde(default)]
    pub auto: bool,
    /// No later action runs once this one is taken.
    #[serde(default)]
    pub stop: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracker: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// A named template of `skill.md`'s front matter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_template: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<IssueLink>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub store: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_secs: Option<i64>,
    /// An array field of the answer: the action is planned once per item
    /// (at most [`MAX_EACH`]), `{{item.…}}` naming that item and `when`
    /// tested on it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub each: Option<String>,
    /// `work_task`: proposed subtasks, one per line (a template).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steps: Option<String>,
    /// `fix_session`, `session_recover`: the host (a template).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// `work_task`, `fix_session`, `setting`, `session_recover`,
    /// `tracker_issue`: what the finding is (a template), kept on the
    /// action so a later run can tell it was proposed (and rejected)
    /// before (the diagnostics `proposals` tool answers it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
    /// `pr_review`: APPROVE | REQUEST_CHANGES | COMMENT (a template;
    /// anything else is COMMENT).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<String>,
    /// `pr_review`: an array field of the answer, each item a line comment
    /// `{path, line, body}`, placed on the PR's diff when it is sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub findings: Option<String>,
    /// `pr_review`: a finding whose `confidence` (0-100) is below this is
    /// dropped, as the PR reviewer's `confidence_threshold` (one without a
    /// confidence counts as 100).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_confidence: Option<u8>,
    /// `pr_review`: an array field of the answer, each item
    /// `{path, line, resolution}` saying what became of a previous review's
    /// finding (`fixed`, `wontfix`, `open` …): with it the verdict is
    /// adjusted as the PR reviewer adjusts it (`adjusted_verdict`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolutions: Option<String>,
    /// `pr_description`: also replace a description someone wrote, on the
    /// instance's first review of the pull request.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub overwrite: bool,
    /// `pr_reply`: resolve the thread once the reply is posted.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub resolve: bool,
}

/// Most items one `each` action is planned for.
pub const MAX_EACH: usize = 10;

impl Action {
    fn set_keys(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        let mut put = |k: &'static str, set: bool| {
            if set {
                out.push(k)
            }
        };
        put("tracker", self.tracker.is_some());
        put("target", self.target.is_some());
        put("body", self.body.is_some());
        put("body_template", self.body_template.is_some());
        put("project", self.project.is_some());
        put("issue_type", self.issue_type.is_some());
        put("summary", self.summary.is_some());
        put("labels", self.labels.is_some());
        put("link", self.link.is_some());
        put("from", self.from.is_some());
        put("to", self.to.is_some());
        put("link_type", self.link_type.is_some());
        put("store", self.store.is_some());
        put("tool", self.tool.is_some());
        put("arguments", self.arguments.is_some());
        put("key", self.key.is_some());
        put("value", self.value.is_some());
        put("expires_secs", self.expires_secs.is_some());
        put("steps", self.steps.is_some());
        put("host", self.host.is_some());
        put("fingerprint", self.fingerprint.is_some());
        put("verdict", self.verdict.is_some());
        put("findings", self.findings.is_some());
        put("min_confidence", self.min_confidence.is_some());
        put("resolutions", self.resolutions.is_some());
        put("overwrite", self.overwrite);
        put("resolve", self.resolve);
        out
    }
}

fn default_max_body() -> usize {
    5000
}

/// What every write is checked against when it is sent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    #[serde(default = "default_max_body")]
    pub max_body_chars: usize,
    /// Tracker projects writes may land in (templates).
    #[serde(default)]
    pub write_projects: Vec<String>,
    /// Repos (`owner/repo`, templates) PR writes may land in.
    #[serde(default)]
    pub repos: Vec<String>,
    /// An `auto` `pr_review` may APPROVE (at `act`, with no person):
    /// off, an unattended approval is sent as COMMENT.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub auto_approve: bool,
    /// What an instance's runs may cost in a UTC day, at most (USD, up to
    /// [`MAX_DAILY_BUDGET_USD`]): past it, a job waits for the next day.
    /// Unset is no cap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daily_budget_usd: Option<f64>,
    /// A `number` input whose value lowers the budget for one instance; a
    /// value above `daily_budget_usd` is held to it, so an instance can
    /// spend less than its definition allows, never more.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_input: Option<String>,
}

/// The highest `limits.daily_budget_usd` a definition may set.
pub const MAX_DAILY_BUDGET_USD: f64 = 1000.0;

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_body_chars: default_max_body(),
            write_projects: Vec::new(),
            repos: Vec::new(),
            auto_approve: false,
            daily_budget_usd: None,
            budget_input: None,
        }
    }
}

impl Limits {
    /// PURE: an instance's daily budget in micro-USD: the definition's,
    /// lowered by the `budget_input` value when it is a lower number.
    pub fn budget_micros(&self, values: &serde_json::Map<String, Value>) -> Option<i64> {
        let ceiling = self.daily_budget_usd?;
        let own = self
            .budget_input
            .as_ref()
            .and_then(|i| values.get(i))
            .and_then(|v| v.as_f64().or_else(|| v.as_str()?.trim().parse().ok()))
            .filter(|v| v.is_finite() && *v >= 0.0);
        let usd = own.map_or(ceiling, |o| o.min(ceiling));
        Some((usd * 1_000_000.0).round() as i64)
    }
}

/// A definition as the catalog keeps it: the definition and its skill.
#[derive(Debug, Clone, PartialEq)]
pub struct Loaded {
    pub name: String,
    pub definition: Definition,
    /// `skill.md` without its front matter: the run's system prompt.
    pub skill: String,
    /// The front matter's named body templates.
    pub templates: BTreeMap<String, String>,
}

/// PURE: `skill.md` split into its front matter's templates (a YAML
/// mapping between `---` lines, with a `templates:` mapping of name →
/// text) and the rest.
pub fn split_skill(body: &str) -> Result<(BTreeMap<String, String>, String), String> {
    let norm = body.replace("\r\n", "\n");
    let Some(rest) = norm.strip_prefix("---\n") else {
        return Ok((BTreeMap::new(), norm));
    };
    let Some(end) = rest
        .find("\n---\n")
        .or_else(|| rest.strip_suffix("\n---").map(|r| r.len()))
    else {
        return Err("skill.md: the front matter has no closing ---".into());
    };
    let front = &rest[..end];
    let after = rest.get(end + 5..).unwrap_or("").to_string();
    let v: serde_yaml::Value =
        serde_yaml::from_str(front).map_err(|e| format!("skill.md front matter: {e}"))?;
    let mut templates = BTreeMap::new();
    if let Some(m) = v.as_mapping() {
        for (k, val) in m {
            if k.as_str() != Some("templates") {
                return Err(format!(
                    "skill.md front matter: unknown key {k:?}; only templates"
                ));
            }
            let Some(t) = val.as_mapping() else {
                return Err("skill.md front matter: templates is a mapping of name → text".into());
            };
            for (name, text) in t {
                match (name.as_str(), text.as_str()) {
                    (Some(n), Some(s)) => {
                        templates.insert(n.to_string(), s.to_string());
                    }
                    _ => return Err("skill.md front matter: a template is name: text".into()),
                }
            }
        }
    }
    Ok((templates, after))
}

/// PURE: the definition's fields of an asset's YAML (its catalog header
/// removed), strictly.
pub fn parse_value(value: &serde_json::Value) -> Result<Definition, String> {
    let mut obj = value
        .as_object()
        .cloned()
        .ok_or("a definition is a mapping")?;
    for k in HEADER_KEYS {
        obj.remove(k);
    }
    serde_json::from_value(Value::Object(obj)).map_err(|e| format!("definition: {e}"))
}

/// PURE: an `asset.yaml` text and its `skill.md`, loaded and checked: the
/// definition, or every problem.
pub fn load(asset_yaml: &str, skill_md: &str) -> Result<Loaded, Vec<String>> {
    if asset_yaml.len() > MAX_YAML_BYTES {
        return Err(vec![format!(
            "asset.yaml is over {} KiB",
            MAX_YAML_BYTES / 1024
        )]);
    }
    let yaml: serde_yaml::Value =
        serde_yaml::from_str(asset_yaml).map_err(|e| vec![format!("asset.yaml: {e}")])?;
    let json: Value = serde_json::to_value(&yaml).map_err(|e| vec![format!("asset.yaml: {e}")])?;
    if json.get("kind").and_then(Value::as_str) != Some("fleet_agent") {
        return Err(vec!["asset.yaml: kind is not fleet_agent".into()]);
    }
    let name = json
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let definition = parse_value(&json).map_err(|e| vec![e])?;
    let (templates, skill) = split_skill(skill_md).map_err(|e| vec![e])?;
    let p = problems(&definition, &templates);
    if p.is_empty() {
        Ok(Loaded {
            name,
            definition,
            skill,
            templates,
        })
    } else {
        Err(p)
    }
}

fn name_ok(s: &str, max: usize) -> bool {
    let mut c = s.chars();
    matches!(c.next(), Some(f) if f.is_ascii_lowercase())
        && s.len() <= max
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn tool_name_ok(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// What a template in one place may name.
#[derive(Clone, Copy)]
struct Scope<'a> {
    inputs: &'a [Input],
    /// `None`: subject paths are refused here.
    subject: Option<&'a [&'a str]>,
    /// `None`: answer paths are refused here.
    answer: Option<&'a Value>,
    now: bool,
    /// `Some(schema)`: `{{item.…}}` names an item of an `each` array.
    item: Option<&'a Value>,
}

fn check_template(text: &str, at: &str, scope: Scope, out: &mut Vec<String>) {
    if text.chars().count() > MAX_TEXT {
        out.push(format!("{at}: longer than {MAX_TEXT} characters"));
        return;
    }
    match template::paths(text) {
        Err(e) => out.push(format!("{at}: {e}")),
        Ok(paths) => {
            for p in paths {
                check_path(&p, at, scope, out);
            }
        }
    }
}

fn check_value_templates(v: &Value, at: &str, scope: Scope, out: &mut Vec<String>) {
    match template::paths_in(v) {
        Err(e) => out.push(format!("{at}: {e}")),
        Ok(paths) => {
            for p in paths {
                check_path(&p, at, scope, out);
            }
        }
    }
}

fn check_path(p: &Path, at: &str, scope: Scope, out: &mut Vec<String>) {
    let text = p.text();
    match p.root.as_str() {
        "inputs" => {
            if !scope.inputs.iter().any(|i| i.name == p.rest[0]) {
                out.push(format!("{at}: {{{{{text}}}}} names no input"));
            } else if p.rest.len() > 1 {
                out.push(format!("{at}: {{{{{text}}}}}: an input has no fields"));
            }
        }
        "subject" => match scope.subject {
            None => out.push(format!("{at}: {{{{{text}}}}}: there is no subject here")),
            Some(fields) if !fields.contains(&p.rest[0].as_str()) => out.push(format!(
                "{at}: {{{{{text}}}}}: the subject has {}",
                fields.join(", ")
            )),
            _ => {}
        },
        "answer" => match scope.answer {
            None => out.push(format!("{at}: {{{{{text}}}}}: there is no answer yet here")),
            Some(schema) if !answer::has_path(schema, &p.rest) => out.push(format!(
                "{at}: {{{{{text}}}}}: output_schema has no such field"
            )),
            _ => {}
        },
        "item" => match scope.item {
            None => out.push(format!(
                "{at}: {{{{{text}}}}}: there is no item here (an action with each has one)"
            )),
            Some(schema) if !answer::has_path(schema, &p.rest) => out.push(format!(
                "{at}: {{{{{text}}}}}: the each items have no such field"
            )),
            _ => {}
        },
        "now" if !scope.now => out.push(format!("{at}: {{{{now}}}} is not available here")),
        _ => {}
    }
}

/// PURE: the definition's inputs as a one-step form (a reference input is
/// a text field: fleet checks the value itself); `None` without inputs.
pub fn inputs_form(d: &Definition) -> Option<FormSpec> {
    let fields: Vec<FormField> = d
        .inputs
        .iter()
        .map(|i| FormField {
            name: i.name.clone(),
            kind: match i.kind {
                InputType::Text | InputType::Tracker | InputType::Project | InputType::Host => {
                    FieldType::Text
                }
                InputType::Textarea => FieldType::Textarea,
                InputType::Number => FieldType::Number,
                InputType::Bool => FieldType::Bool,
                InputType::Select => FieldType::Select,
                InputType::Multiselect => FieldType::Multiselect,
            },
            label: i.label.clone(),
            help: i.help.clone(),
            required: i.required,
            value: i.value.clone(),
            when: i.when.clone(),
            placeholder: i.placeholder.clone(),
            max_len: i.max_len,
            min: i.min,
            max: i.max,
            integer: i.integer,
            options: i.options.clone(),
            other: i.other,
            disabled_reason: None,
            drafted: None,
            secret_note: None,
            env_target: None,
        })
        .collect();
    if fields.is_empty() {
        return None;
    }
    Some(FormSpec {
        spec: forms::FORM_SPEC_VERSION.into(),
        title: "inputs".into(),
        intro: None,
        submit: None,
        save_later: false,
        steps: vec![FormStep {
            title: "Inputs".into(),
            name: None,
            kind: Default::default(),
            intro: None,
            when: None,
            fields,
        }],
        checks: Vec::new(),
    })
}

/// PURE: every problem with a definition; empty when it is fine.
/// `templates` are `skill.md`'s named body templates.
pub fn problems(d: &Definition, templates: &BTreeMap<String, String>) -> Vec<String> {
    let mut out = Vec::new();

    // ── inputs: the form validator's rules, plus the reference types ──
    if d.inputs.len() > MAX_INPUTS {
        out.push(format!("inputs: at most {MAX_INPUTS}"));
    }
    for i in d.inputs.iter().filter(|i| i.name.starts_with("f_")) {
        out.push(format!(
            "inputs › {}: f_ names are the grant form's own",
            i.name
        ));
    }
    for i in d.inputs.iter().filter(|i| i.kind.is_reference()) {
        if i.options.is_some() || i.max_len.is_some() || i.placeholder.is_some() {
            out.push(format!(
                "inputs › {}: a {:?} input takes no options, max_len or placeholder: fleet lists them",
                i.name, i.kind
            ));
        }
    }
    if let Some(form) = inputs_form(d) {
        for p in forms::validate(&form) {
            out.push(format!("inputs: {}", p.trim_start_matches("step 1 › ")));
        }
    }

    // ── stores ──
    if d.stores.len() > MAX_STORES {
        out.push(format!("stores: at most {MAX_STORES}"));
    }
    let mut roles: Vec<&str> = Vec::new();
    for s in &d.stores {
        let at = format!("stores › {}", s.role);
        // ≤ 30: the grant form names its fields `f_store_<role>`.
        if !name_ok(&s.role, 30) {
            out.push(format!("{at}: a role is [a-z][a-z0-9_]*, at most 30"));
        }
        if roles.contains(&s.role.as_str()) {
            out.push(format!("{at}: the role appears twice"));
        }
        roles.push(&s.role);
        if s.label.trim().is_empty() {
            out.push(format!("{at}: give it a label"));
        }
        match s.kind {
            StoreKind::Mcp => {
                if s.read_tools.is_empty() && s.write_tools.is_empty() {
                    out.push(format!("{at}: name its read_tools or write_tools"));
                }
                if s.shared_by_default {
                    out.push(format!("{at}: shared_by_default is for fleet stores"));
                }
                for t in s
                    .read_tools
                    .iter()
                    .map(String::as_str)
                    .chain(s.write_tools.iter().map(WriteTool::name))
                {
                    if !tool_name_ok(t) {
                        out.push(format!("{at}: tool {t:?} is not a tool name"));
                    }
                }
                for w in &s.write_tools {
                    if s.read_tools.iter().any(|r| r == w.name()) {
                        out.push(format!(
                            "{at}: {} is both a read and a write tool",
                            w.name()
                        ));
                    }
                }
            }
            StoreKind::Fleet => {
                if !s.read_tools.is_empty() || !s.write_tools.is_empty() {
                    out.push(format!(
                        "{at}: a fleet store has no tools: write to it with `record`"
                    ));
                }
            }
        }
    }
    let role = |name: &str| d.stores.iter().find(|s| s.role == name);
    let inputs_only = Scope {
        inputs: &d.inputs,
        subject: None,
        answer: None,
        now: false,
        item: None,
    };

    // ── trigger ──
    let t = &d.trigger;
    let at = format!("trigger ({})", t.source.word());
    let (allowed, needed) = t.source.keys();
    for k in t.set_keys() {
        if !allowed.contains(&k) {
            out.push(format!("{at}: {k} is not for this source"));
        }
    }
    for k in needed {
        if !t.set_keys().contains(k) {
            out.push(format!("{at}: needs {k}"));
        }
    }
    for (k, v) in [
        ("tracker", &t.tracker),
        ("query", &t.query),
        ("repo", &t.repo),
    ] {
        if let Some(v) = v {
            check_template(v, &format!("{at} › {k}"), inputs_only, &mut out);
        }
    }
    if let Some(m) = &t.match_ {
        let set = [
            m.any_phrase.is_some(),
            m.regex.is_some(),
            m.assignee.is_some(),
        ];
        match t.source {
            TriggerSource::TrackerAssigned if !set[2] || set[0] || set[1] => out.push(format!(
                "{at} › match: an assignment matches only on assignee"
            )),
            TriggerSource::TrackerComments | TriggerSource::GithubComments
                if set[2] || (!set[0] && !set[1]) =>
            {
                out.push(format!(
                    "{at} › match: a comment matches on any_phrase or regex"
                ))
            }
            _ => {}
        }
        for (k, v) in [("any_phrase", &m.any_phrase), ("assignee", &m.assignee)] {
            if let Some(v) = v {
                check_template(v, &format!("{at} › match › {k}"), inputs_only, &mut out);
            }
        }
        if let Some(r) = &m.regex {
            if let Err(e) = regex::RegexBuilder::new(r).size_limit(1 << 16).build() {
                out.push(format!("{at} › match › regex: {e}"));
            }
        }
    }
    if t.debounce_secs.is_some_and(|s| !(0..=3600).contains(&s)) {
        out.push(format!("{at}: debounce_secs is 0-3600"));
    }
    if t.reverify_mins.is_some_and(|m| !(1..=1440).contains(&m)) {
        out.push(format!("{at}: reverify_mins is 1-1440"));
    }
    if t.max_rounds.is_some_and(|m| !(1..=5).contains(&m)) {
        out.push(format!("{at}: max_rounds is 1-5"));
    }
    if t.source == TriggerSource::McpScan {
        match t.store.as_deref().map(|s| (s, role(s))) {
            Some((s, None)) => out.push(format!("{at}: store {s} is not a role")),
            Some((s, Some(r))) if r.kind != StoreKind::Mcp => {
                out.push(format!("{at}: store {s} is not an mcp store"))
            }
            Some((s, Some(r))) => {
                if let Some(tool) = &t.tool {
                    if !r.read_tools.contains(tool) {
                        out.push(format!("{at}: tool {tool} is not a read_tool of store {s}"));
                    }
                }
            }
            None => {}
        }
        if t.every_mins.is_some_and(|m| !(5..=1440).contains(&m)) {
            out.push(format!("{at}: every_mins is 5-1440"));
        }
        if t.window_mins.is_some_and(|m| !(1..=10_080).contains(&m)) {
            out.push(format!("{at}: window_mins is 1-10080"));
        }
        let every = t.every_mins.unwrap_or(DEFAULT_SCAN_EVERY_MINS);
        // A scan runs on a poll tick once `every_mins` has passed, so two
        // scans are always a little more than `every_mins` apart.
        if t.window_mins.is_some_and(|w| w <= every) {
            out.push(format!(
                "{at}: window_mins must be more than every_mins ({every}): scans run a poll late, and would skip the entries between them"
            ));
        }
        if t.max_jobs
            .is_some_and(|n| !(1..=MAX_SCAN_JOBS).contains(&n))
        {
            out.push(format!("{at}: max_jobs is 1-{MAX_SCAN_JOBS}"));
        }
        if let Some(a) = &t.arguments {
            if !a.is_object() {
                out.push(format!("{at}: arguments is an object"));
            }
            check_value_templates(a, &format!("{at} › arguments"), inputs_only, &mut out);
        }
        if let Some(k) = &t.key_template {
            let subject = Scope {
                subject: Some(&["entry"]),
                ..inputs_only
            };
            check_template(k, &format!("{at} › key_template"), subject, &mut out);
            if template::paths(k).is_ok_and(|p| p.is_empty()) {
                out.push(format!(
                    "{at} › key_template: name a field of the entry ({{{{subject.entry.…}}}})"
                ));
            }
        }
    }
    if let Some(c) = &t.cron {
        if let Err(e) = crate::service::routines::cron::Cron::parse(c) {
            out.push(format!("{at} › cron: {e}"));
        }
    }

    // ── run ──
    let r = &d.run;
    for tool in &r.tools {
        if !vehicle::READ_ONLY_TOOLS.contains(&tool.as_str()) {
            out.push(format!(
                "run › tools: {tool} is not read-only; a run may have {}",
                vehicle::READ_ONLY_TOOLS.join(", ")
            ));
        }
    }
    if !(1..=vehicle::MAX_TURNS).contains(&r.max_turns) || !(60..=3600).contains(&r.timeout_secs) {
        out.push(format!(
            "run: max_turns is 1-{} and timeout_secs 60-3600",
            vehicle::MAX_TURNS
        ));
    }
    if let Some(m) = &r.model {
        if m.is_empty()
            || m.len() > 64
            || !m
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-._[]".contains(c))
        {
            out.push(format!("run › model: {m:?} is not a model name"));
        }
    }
    if !(0..=7 * 86_400).contains(&r.cooldown_secs) {
        out.push("run › cooldown_secs is 0-604800".into());
    }
    if r.workspace == Workspace::PrHead && !t.source.is_github() {
        out.push("run › workspace pr_head reads a pull request: the trigger is github_prs or github_comments".into());
    }
    if let Some(df) = &r.diff {
        if !t.source.is_github() {
            out.push(
                "run › diff is a pull request's: the trigger is github_prs or github_comments"
                    .into(),
            );
        }
        if !(1..=MAX_DIFF_LINES).contains(&df.max_lines) {
            out.push(format!("run › diff › max_lines is 1-{MAX_DIFF_LINES}"));
        }
        if df.exclude_paths.len() > 50
            || df
                .exclude_paths
                .iter()
                .any(|p| p.is_empty() || p.len() > 200)
        {
            out.push("run › diff › exclude_paths: at most 50 globs of 1-200 characters".into());
        }
    }
    if let Some(b) = &r.branch {
        if r.workspace != Workspace::Project {
            out.push("run › branch is for a project workspace".into());
        }
        check_template(b, "run › branch", inputs_only, &mut out);
    }
    if r.max_parallel.is_some_and(|n| !(1..=20).contains(&n)) {
        out.push("run › max_parallel is 1-20".into());
    }
    let schema_ok = if r.output_schema.is_null() {
        out.push("run › output_schema: describe the answer".into());
        false
    } else {
        let p = answer::check_schema(&r.output_schema);
        let ok = p.is_empty();
        out.extend(p.into_iter().map(|x| format!("run › {x}")));
        ok
    };
    if r.context.len() > MAX_CONTEXT {
        out.push(format!("run › context: at most {MAX_CONTEXT}"));
    }
    let subject_fields = t.source.subject_fields();
    let before_run = Scope {
        inputs: &d.inputs,
        subject: Some(subject_fields),
        answer: None,
        now: true,
        item: None,
    };
    for (i, c) in r.context.iter().enumerate() {
        let at = format!("run › context › {}", i + 1);
        match role(&c.store) {
            None => out.push(format!("{at}: store {} is not a role", c.store)),
            Some(s) if s.kind != StoreKind::Fleet => {
                out.push(format!("{at}: store {} is not a fleet store", c.store))
            }
            _ => {}
        }
        match (&c.key, c.recent) {
            (Some(k), None) => check_template(k, &format!("{at} › key"), before_run, &mut out),
            (None, Some(n)) if (1..=50).contains(&n) => {}
            (None, Some(_)) => out.push(format!("{at}: recent is 1-50")),
            _ => out.push(format!("{at}: give key or recent, one of them")),
        }
    }

    if let Some(f) = &r.fingerprint {
        let at = "run › fingerprint";
        match role(&f.store) {
            None => out.push(format!("{at}: store {} is not a role", f.store)),
            Some(s) if s.kind != StoreKind::Fleet => {
                out.push(format!("{at}: store {} is not a fleet store", f.store))
            }
            _ => {}
        }
        let answered = Scope {
            answer: schema_ok.then_some(&r.output_schema),
            ..before_run
        };
        for (k, v) in [
            ("class", Some(&f.class)),
            ("message", f.message.as_ref()),
            ("stack", f.stack.as_ref()),
            ("component", f.component.as_ref()),
        ] {
            if let Some(v) = v {
                check_template(v, &format!("{at} › {k}"), answered, &mut out);
            }
        }
        if let Some(p) = &f.app_frame_prefix {
            check_template(
                p,
                &format!("{at} › app_frame_prefix"),
                inputs_only,
                &mut out,
            );
        }
        if !(0..=10_080).contains(&f.window_mins) {
            out.push(format!("{at}: window_mins is 0-10080"));
        }
        if let Some(b) = &f.bugs {
            for (k, v) in [
                ("tracker", b.tracker.as_ref()),
                ("project", Some(&b.project)),
                ("issue_type", b.issue_type.as_ref()),
            ] {
                if let Some(v) = v {
                    check_template(v, &format!("{at} › bugs › {k}"), inputs_only, &mut out);
                }
            }
            let has_tracker = b.tracker.is_some()
                || t.tracker.is_some()
                || d.inputs.iter().any(|i| i.kind == InputType::Tracker);
            if !has_tracker {
                out.push(format!(
                    "{at} › bugs: name the tracker (the trigger has none)"
                ));
            }
        }
        let own = answer::fields(&r.output_schema);
        for (name, _) in FINGERPRINT_FIELDS {
            if own.iter().any(|f| f == name) {
                out.push(format!(
                    "{at}: output_schema has {name}, which the fingerprint adds"
                ));
            }
        }
    }

    // ── actions ──
    if d.actions.len() > MAX_ACTIONS {
        out.push(format!("actions: at most {MAX_ACTIONS}"));
    }
    let schema = actions_schema(r);
    let after_run = Scope {
        inputs: &d.inputs,
        subject: Some(subject_fields),
        answer: if schema_ok { Some(&schema) } else { None },
        now: true,
        item: None,
    };
    let answer_fields = answer::fields(&schema);
    let mut ids: Vec<&str> = Vec::new();
    for a in &d.actions {
        let at = format!("actions › {}", a.id);
        if !name_ok(&a.id, 40) {
            out.push(format!("{at}: an id is [a-z][a-z0-9_]*, at most 40"));
        }
        if ids.contains(&a.id.as_str()) {
            out.push(format!("{at}: the id appears twice"));
        }
        ids.push(&a.id);
        // `each`: the array the action is planned over, and its items' schema
        // for `{{item.…}}` and `when`.
        let item_schema = a.each.as_ref().and_then(|f| {
            let items = r
                .output_schema
                .get("properties")
                .and_then(|p| p.get(f))
                .filter(|v| v.get("type").and_then(Value::as_str) == Some("array"))
                .and_then(|v| v.get("items"))
                .filter(|i| i.get("type").and_then(Value::as_str) == Some("object"));
            if schema_ok && items.is_none() {
                out.push(format!(
                    "{at}: each {f} is not an array of objects in output_schema"
                ));
            }
            items
        });
        if let Some(c) = &a.when {
            out.extend(answer::check_condition(c, &format!("{at} › when")));
            let fields = item_schema.map_or_else(|| answer_fields.clone(), answer::fields);
            if schema_ok {
                for f in answer::condition_fields(c) {
                    if !fields.contains(&f) {
                        out.push(format!("{at} › when: {f} is not a field of output_schema"));
                    }
                }
            }
        }
        let after_run = Scope {
            item: item_schema,
            ..after_run
        };
        let (allowed, needed) = a.kind.keys();
        let set = a.set_keys();
        for k in &set {
            if !allowed.contains(k) {
                out.push(format!("{at}: {k} is not for {}", a.kind.word()));
            }
        }
        for k in needed {
            let has = if *k == "body" {
                a.body.is_some() || a.body_template.is_some()
            } else {
                set.contains(k)
            };
            if !has {
                out.push(format!("{at}: {} needs {k}", a.kind.word()));
            }
        }
        if a.body.is_some() && a.body_template.is_some() {
            out.push(format!("{at}: give body or body_template, not both"));
        }
        if let Some(name) = &a.body_template {
            match templates.get(name) {
                None => out.push(format!("{at}: skill.md has no template {name}")),
                Some(text) => check_template(
                    text,
                    &format!("{at} › template {name}"),
                    after_run,
                    &mut out,
                ),
            }
        }
        for (k, v) in [
            ("tracker", &a.tracker),
            ("target", &a.target),
            ("body", &a.body),
            ("project", &a.project),
            ("issue_type", &a.issue_type),
            ("summary", &a.summary),
            ("from", &a.from),
            ("to", &a.to),
            ("link_type", &a.link_type),
            ("key", &a.key),
            ("steps", &a.steps),
            ("host", &a.host),
            ("fingerprint", &a.fingerprint),
            ("verdict", &a.verdict),
        ] {
            if let Some(v) = v {
                check_template(v, &format!("{at} › {k}"), after_run, &mut out);
            }
        }
        for (k, v) in [
            ("labels", &a.labels),
            ("arguments", &a.arguments),
            ("value", &a.value),
        ] {
            if let Some(v) = v {
                check_value_templates(v, &format!("{at} › {k}"), after_run, &mut out);
            }
        }
        if let Some(l) = &a.link {
            check_template(&l.to, &format!("{at} › link › to"), after_run, &mut out);
            check_template(
                &l.link_type,
                &format!("{at} › link › link_type"),
                inputs_only,
                &mut out,
            );
        }
        // Where an action writes, and what may write unattended.
        match a.kind {
            ActionKind::TrackerComment | ActionKind::TrackerIssue | ActionKind::TrackerLink => {
                let has_tracker = a.tracker.is_some()
                    || t.tracker.is_some()
                    || d.inputs.iter().any(|i| i.kind == InputType::Tracker);
                if !has_tracker {
                    out.push(format!("{at}: name the tracker (the trigger has none)"));
                }
                if a.kind == ActionKind::TrackerIssue && d.limits.write_projects.is_empty() {
                    out.push(format!(
                        "{at}: list the projects issues may go to in limits.write_projects"
                    ));
                }
            }
            ActionKind::PrReview
            | ActionKind::PrComment
            | ActionKind::PrLabels
            | ActionKind::PrDescription
            | ActionKind::PrReply => {
                if !t.source.is_github() {
                    out.push(format!("{at}: {} needs a github trigger", a.kind.word()));
                }
                if a.kind == ActionKind::PrReply && t.source != TriggerSource::GithubReviewReplies {
                    out.push(format!(
                        "{at}: pr_reply answers a review thread: the trigger is github_review_replies"
                    ));
                }
                if a.kind == ActionKind::PrReview && t.source != TriggerSource::GithubPrs {
                    out.push(format!(
                        "{at}: pr_review reviews a PR's head: the trigger is github_prs"
                    ));
                }
                if a.min_confidence.is_some_and(|c| c > 100) {
                    out.push(format!("{at}: min_confidence is 0-100"));
                }
                if let Some(f) = a.resolutions.as_ref().filter(|_| schema_ok) {
                    let is_array = r
                        .output_schema
                        .get("properties")
                        .and_then(|p| p.get(f))
                        .and_then(|v| v.get("type"))
                        .and_then(Value::as_str)
                        == Some("array");
                    if !is_array {
                        out.push(format!(
                            "{at}: resolutions {f} is not an array in output_schema"
                        ));
                    }
                }
                if a.kind == ActionKind::PrReview && a.each.is_some() {
                    out.push(format!("{at}: pr_review is one review: no each"));
                }
                if let Some(f) = a.findings.as_ref().filter(|_| schema_ok) {
                    let item = r
                        .output_schema
                        .get("properties")
                        .and_then(|p| p.get(f))
                        .filter(|v| v.get("type").and_then(Value::as_str) == Some("array"))
                        .and_then(|v| v.get("items"));
                    let has = |k: &str| {
                        item.and_then(|i| i.get("properties"))
                            .is_some_and(|p| p.get(k).is_some())
                    };
                    if !(has("path") && has("line") && has("body")) {
                        out.push(format!(
                            "{at}: findings {f} is not an array of {{path, line, body}} in output_schema"
                        ));
                    }
                }
            }
            ActionKind::McpCall => {
                if let Some(s) = &a.store {
                    match role(s) {
                        None => out.push(format!("{at}: store {s} is not a role")),
                        Some(r) if r.kind != StoreKind::Mcp => {
                            out.push(format!("{at}: store {s} is not an mcp store"))
                        }
                        Some(r) => {
                            if let Some(tool) = &a.tool {
                                match r.write_tools.iter().find(|w| w.name() == tool) {
                                    None => out.push(format!(
                                        "{at}: tool {tool} is not a write_tool of store {s}"
                                    )),
                                    Some(w) if a.auto && !w.auto_ok() => out.push(format!(
                                        "{at}: auto needs the store to mark {tool} auto_ok"
                                    )),
                                    _ => {}
                                }
                            }
                        }
                    }
                }
                if a.arguments.as_ref().is_some_and(|v| !v.is_object()) {
                    out.push(format!("{at}: arguments is an object"));
                }
            }
            ActionKind::WorkTask => {}
            ActionKind::FixSession | ActionKind::Setting | ActionKind::SessionRecover => {
                if a.auto {
                    out.push(format!(
                        "{at}: {} never runs without a person: drop auto",
                        a.kind.word()
                    ));
                }
            }
            ActionKind::Record => {
                if let Some(s) = &a.store {
                    match role(s) {
                        None => out.push(format!("{at}: store {s} is not a role")),
                        Some(r) if r.kind != StoreKind::Fleet => {
                            out.push(format!("{at}: store {s} is not a fleet store"))
                        }
                        _ => {}
                    }
                }
                if a.expires_secs.is_some_and(|e| e < 60) {
                    out.push(format!("{at}: expires_secs is at least 60"));
                }
            }
        }
    }

    // ── limits ──
    if !(500..=30_000).contains(&d.limits.max_body_chars) {
        out.push("limits: max_body_chars is 500-30000".into());
    }
    if d.limits
        .daily_budget_usd
        .is_some_and(|b| !(b > 0.0 && b <= MAX_DAILY_BUDGET_USD))
    {
        out.push(format!(
            "limits: daily_budget_usd is more than 0, at most {MAX_DAILY_BUDGET_USD}"
        ));
    }
    if let Some(name) = &d.limits.budget_input {
        if d.limits.daily_budget_usd.is_none() {
            out.push("limits: budget_input lowers daily_budget_usd: set it".into());
        }
        match d.inputs.iter().find(|i| &i.name == name) {
            Some(i) if i.kind == InputType::Number => {}
            Some(_) => out.push(format!("limits: budget_input {name} is not a number input")),
            None => out.push(format!("limits: budget_input {name} is not an input")),
        }
    }
    for (i, p) in d.limits.write_projects.iter().enumerate() {
        check_template(
            p,
            &format!("limits › write_projects › {}", i + 1),
            inputs_only,
            &mut out,
        );
    }
    for (i, p) in d.limits.repos.iter().enumerate() {
        check_template(
            p,
            &format!("limits › repos › {}", i + 1),
            inputs_only,
            &mut out,
        );
    }
    for (name, text) in templates {
        if !d
            .actions
            .iter()
            .any(|a| a.body_template.as_deref() == Some(name))
        {
            // An unused template is checked too: it may be used next.
            check_template(
                text,
                &format!("skill.md › template {name}"),
                after_run,
                &mut out,
            );
        }
    }
    out
}
