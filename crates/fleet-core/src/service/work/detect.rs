//! Work detection (work graph M4.2/M4.3): turn what a session shows into
//! resolver input, run [`resolve`](super::resolve::resolve) and apply it.
//!
//! Signals (design §0.3):
//! * **branch** — `sessions.current_branch` (the transcript's `gitBranch`,
//!   read by `context::refresh` after every Stop), else the worktree's
//!   branch. State: only its present value counts.
//! * **PR** — `sessions.pr_signals`, written by the PR probe: the head
//!   branch and closing issue refs (state), keys in the title / body and in
//!   commit trailers (weak events, re-read while the PR exists).
//! * **prompt** — the UserPromptSubmit hook's full prompt: only the matches
//!   are kept (≤ 80 chars each, and a ±40-char redacted snippet unless
//!   `work.evidence_snippets` is off); the prompt itself is never stored.
//!
//! Every entry point is synchronous and runs with the store guard its caller
//! already holds, briefly: recognition is pure, and the resolver reads one
//! session's links. Nothing here awaits.

use super::recognize::{recognize, MatchKind, RecognizeCtx, TrackerHost};
use super::resolve::{resolve, Candidate, Evidence, ResolveInput, Signal, Strength};
use crate::ipc_error::{codes, IpcError};
use crate::store::{DetectionState, Store};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// A prompt naming more distinct references than this is a reference list
/// (the dump guard, C14): all of them are weak and none is pre-selected.
pub const DUMP_GUARD_MAX: usize = 3;
/// Longest matched text an evidence line keeps.
pub const EVIDENCE_TEXT_MAX: usize = 80;
/// Characters of prompt kept on each side of a match in a snippet.
pub const SNIPPET_RADIUS: usize = 40;
/// Branch links a person confirmed from suggestions before a project trusts
/// branch keys by itself (roadmap decision 5; visible and reversible).
pub const AUTO_TRUST_AFTER: i64 = 3;

/// What the PR probe keeps about a session's PR (`sessions.pr_signals`).
/// Parsed from `gh pr view`, the body never stored.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrSignals {
    /// `headRefName`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    /// `closingIssuesReferences` as `owner/repo#n`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub closing: Vec<String>,
    /// References in the title and body (keys, ticket URLs).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub text: Vec<String>,
    /// References in commit trailers (`Refs:`, `Fixes`, `Jira:`, `Closes #n`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trailers: Vec<String>,
    /// `state`: OPEN | CLOSED | MERGED (work graph M7: a merged PR's idle
    /// session is a tidy-up candidate). Absent from rows probed before M7.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
}

/// Most references of one kind a PR keeps.
const PR_REFS_MAX: usize = 10;

impl PrSignals {
    /// Read the fields of one `gh pr view --json
    /// headRefName,title,body,closingIssuesReferences` answer. `body` is
    /// capped at 4k by the probe; it is scanned here and dropped.
    pub fn from_gh_json(v: &serde_json::Value) -> PrSignals {
        let mut out = PrSignals {
            head: v
                .get("headRefName")
                .and_then(|h| h.as_str())
                .map(|h| h.chars().take(255).collect()),
            state: v
                .get("state")
                .and_then(|s| s.as_str())
                .filter(|s| s.len() <= 16)
                .map(str::to_ascii_uppercase),
            ..Default::default()
        };
        for r in v
            .get("closingIssuesReferences")
            .and_then(|c| c.as_array())
            .into_iter()
            .flatten()
        {
            let n = r.get("number").and_then(|n| n.as_i64());
            let repo = r.get("repository");
            let owner = repo
                .and_then(|x| x.get("owner"))
                .and_then(|o| o.get("login"))
                .and_then(|l| l.as_str());
            let name = repo.and_then(|x| x.get("name")).and_then(|n| n.as_str());
            let key = match (owner, name, n) {
                (Some(o), Some(nm), Some(n)) => Some(format!(
                    "{}/{}#{n}",
                    o.to_ascii_lowercase(),
                    nm.to_ascii_lowercase()
                )),
                _ => r
                    .get("url")
                    .and_then(|u| u.as_str())
                    .and_then(|u| {
                        recognize(u, &RecognizeCtx::default())
                            .into_iter()
                            .find(|m| m.kind == MatchKind::Url)
                    })
                    .map(|m| m.key),
            };
            if let Some(k) = key {
                push_unique(&mut out.closing, k);
            }
        }
        let title = v.get("title").and_then(|t| t.as_str()).unwrap_or("");
        let body: String = v
            .get("body")
            .and_then(|b| b.as_str())
            .unwrap_or("")
            .chars()
            .take(4096)
            .collect();
        for m in recognize(&format!("{title}\n{body}"), &RecognizeCtx::default()) {
            if !out.closing.contains(&m.key) {
                push_unique(&mut out.text, m.key);
            }
        }
        out
    }

    /// Add the references of commit messages' trailer lines.
    pub fn add_trailers(&mut self, messages: &str) {
        for line in messages.lines() {
            let l = line.trim();
            let lower = l.to_ascii_lowercase();
            let is_trailer = [
                "refs", "ref", "fixes", "fix", "closes", "close", "resolves", "resolve", "jira",
                "issue", "ticket",
            ]
            .iter()
            .any(|w| {
                lower.starts_with(w)
                    && lower[w.len()..].starts_with([':', ' '])
                    && lower.len() > w.len() + 1
            });
            if !is_trailer {
                continue;
            }
            for m in recognize(l, &RecognizeCtx::default()) {
                push_unique(&mut self.trailers, m.key);
            }
            // `Closes #42` names an issue of the PR's own repo: kept as
            // `#42`, resolved against the session's repo at resolve time.
            for w in l.split_whitespace() {
                let w = w.trim_end_matches([',', '.', ';', ')']);
                if let Some(n) = w.strip_prefix('#') {
                    if !n.is_empty() && n.len() <= 7 && n.bytes().all(|b| b.is_ascii_digit()) {
                        push_unique(&mut self.trailers, format!("#{n}"));
                    }
                }
            }
        }
    }

    /// The probe saw the PR merged.
    pub fn is_merged(&self) -> bool {
        self.state.as_deref() == Some("MERGED")
    }

    pub fn is_empty(&self) -> bool {
        self.head.is_none()
            && self.closing.is_empty()
            && self.text.is_empty()
            && self.trailers.is_empty()
    }
}

fn push_unique(v: &mut Vec<String>, s: String) {
    if v.len() < PR_REFS_MAX && !v.contains(&s) {
        v.push(s);
    }
}

/// The session's live branch from a transcript tail: the `gitBranch` of the
/// last main-thread entry that has one.
pub fn branch_from_jsonl(jsonl: &str) -> Option<String> {
    let mut last = None;
    for line in jsonl.lines() {
        // Cheap pre-filter: most lines are big and have no branch.
        if !line.contains("\"gitBranch\"") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v.get("isSidechain").and_then(|b| b.as_bool()) == Some(true) {
            continue;
        }
        if let Some(b) = v.get("gitBranch").and_then(|b| b.as_str()) {
            if !b.trim().is_empty() {
                last = Some(b.trim().to_string());
            }
        }
    }
    last
}

/// Trackers as recognition sees them, and the prefixes two or more claim.
struct TrackerView {
    ctx: RecognizeCtx,
    shared_prefixes: BTreeSet<String>,
}

fn tracker_view(s: &Store, repo: Option<String>) -> Result<TrackerView, IpcError> {
    let mut owners: BTreeMap<String, usize> = BTreeMap::new();
    let mut ctx = RecognizeCtx {
        repo,
        ..Default::default()
    };
    for t in s.list_trackers()? {
        for p in &t.config.key_prefixes {
            let p = p.to_ascii_uppercase();
            *owners.entry(p.clone()).or_default() += 1;
            if !ctx.prefixes.contains(&p) {
                ctx.prefixes.push(p);
            }
        }
        if let Some(host) = site_host(&t.site_url) {
            ctx.trackers.push(TrackerHost {
                id: t.id,
                host,
                provider: t.provider.clone(),
            });
        }
    }
    Ok(TrackerView {
        ctx,
        shared_prefixes: owners
            .into_iter()
            .filter(|(_, n)| *n > 1)
            .map(|(p, _)| p)
            .collect(),
    })
}

fn site_host(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let host = rest.split(['/', ':', '?']).next()?.to_ascii_lowercase();
    (!host.is_empty()).then_some(host)
}

fn prefix_of(key: &str) -> Option<&str> {
    key.split_once('-').map(|(p, _)| p)
}

impl TrackerView {
    /// A key whose prefix two trackers claim (R8). A URL's host settles it.
    fn ambiguous(&self, key: &str, tracker_id: Option<i64>) -> bool {
        tracker_id.is_none()
            && prefix_of(key)
                .is_some_and(|p| self.shared_prefixes.contains(&p.to_ascii_uppercase()))
    }

    /// A stored reference (from the probe, recognised without prefixes)
    /// still counts under today's trackers.
    fn admits(&self, target: &str) -> bool {
        if self.ctx.prefixes.is_empty() || target.contains('#') || target.contains(':') {
            return true;
        }
        prefix_of(target)
            .is_some_and(|p| self.ctx.prefixes.iter().any(|k| k.eq_ignore_ascii_case(p)))
    }
}

fn evidence(signal: Signal, text: &str, at: i64, conv: Option<&str>) -> Evidence {
    Evidence {
        signal,
        rule: String::new(),
        text: text.chars().take(EVIDENCE_TEXT_MAX).collect(),
        snippet: None,
        at,
        conversation: conv.map(str::to_string),
        note: None,
    }
}

fn candidate(target: String, signal: Signal, strength: Strength, ev: Evidence) -> Candidate {
    Candidate {
        target,
        signal,
        strength,
        ambiguous: false,
        first_prompt_sole: false,
        tracker_id: None,
        untracked: false,
        evidence: ev,
    }
}

/// The state-signal candidates of a session: `(branch, pr, pr events)`.
fn state_candidates(
    st: &DetectionState,
    tv: &TrackerView,
    now: i64,
) -> (
    Option<Vec<Candidate>>,
    Option<Vec<Candidate>>,
    Vec<Candidate>,
) {
    let conv = st.claude_session_id.as_deref();
    let branch_ctx = RecognizeCtx {
        repo: None,
        ..tv.ctx.clone()
    };
    let branch = st.branch.as_deref().map(|b| {
        super::recognize::first_key(b, &branch_ctx)
            .map(|k| {
                let mut c = candidate(
                    k.clone(),
                    Signal::Branch,
                    Strength::Strong,
                    evidence(Signal::Branch, b, now, conv),
                );
                c.ambiguous = tv.ambiguous(&k, None);
                c
            })
            .into_iter()
            .collect::<Vec<_>>()
    });
    if !st.pr_probed {
        return (branch, None, vec![]);
    }
    let sig: PrSignals = st
        .pr_signals
        .as_deref()
        .and_then(|j| serde_json::from_str(j).ok())
        .unwrap_or_default();
    let mut pr = Vec::new();
    if let Some(h) = sig.head.as_deref() {
        if let Some(k) = super::recognize::first_key(h, &branch_ctx) {
            let mut c = candidate(
                k.clone(),
                Signal::PrHead,
                Strength::Strong,
                evidence(Signal::PrHead, h, now, conv),
            );
            c.ambiguous = tv.ambiguous(&k, None);
            pr.push(c);
        }
    }
    // A GitHub issue ref is only a bare `owner/repo#n` until a GitHub
    // tracker exists (M6): never auto-linked before then (R3u).
    let github_tracker = tv.ctx.trackers.iter().any(|t| t.provider == "github");
    for r in sig.closing.iter().filter(|r| tv.admits(r)) {
        let mut c = candidate(
            r.clone(),
            Signal::PrClosing,
            Strength::Strong,
            evidence(Signal::PrClosing, r, now, conv),
        );
        c.untracked = r.contains('#') && !github_tracker;
        pr.push(c);
    }
    let mut events = Vec::new();
    for (signal, refs) in [
        (Signal::PrText, &sig.text),
        (Signal::Trailer, &sig.trailers),
    ] {
        for r in refs {
            let target = match (r.strip_prefix('#'), st.repo.as_deref()) {
                (Some(n), Some(repo)) => format!("{}#{n}", repo.to_ascii_lowercase()),
                (Some(_), None) => continue,
                _ => r.clone(),
            };
            if !tv.admits(&target) {
                continue;
            }
            let mut c = candidate(
                target.clone(),
                signal,
                Strength::Weak,
                evidence(signal, r, now, conv),
            );
            c.ambiguous = tv.ambiguous(&target, None);
            events.push(c);
        }
    }
    (branch, Some(pr), events)
}

/// Why a prompt is not evidence (the loop guard, C14), or `None`.
pub fn loop_guard(
    prompt: &str,
    last_prompt: Option<&str>,
    handovers: &[String],
) -> Option<&'static str> {
    let p = prompt.trim();
    if p.contains("[claude-fleet") {
        return Some("fleet_marked");
    }
    if let Some(last) = last_prompt.map(str::trim).filter(|l| !l.is_empty()) {
        let head: String = p.chars().take(crate::store::LAST_PROMPT_CHARS).collect();
        if head.trim() == last {
            return Some("fleet_sent");
        }
    }
    if p.chars().count() >= 16 && handovers.iter().any(|h| h.contains(p)) {
        return Some("handover");
    }
    None
}

/// A ±[`SNIPPET_RADIUS`]-char window of `text` around byte span `(a, b)`,
/// redacted, on one line.
fn snippet(text: &str, (a, b): (usize, usize)) -> String {
    let before: String = {
        let v: Vec<char> = text[..a].chars().rev().take(SNIPPET_RADIUS).collect();
        v.into_iter().rev().collect()
    };
    let after: String = text[b..].chars().take(SNIPPET_RADIUS).collect();
    let raw = format!("{before}{}{after}", &text[a..b]);
    let one_line: String = raw
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    crate::logging::redact(one_line.trim()).into_owned()
}

/// Event candidates from one submitted prompt (after the loop guard).
fn prompt_candidates(
    s: &Store,
    st: &DetectionState,
    tv: &TrackerView,
    prompt: &str,
    first_prompt: bool,
    snippets: bool,
    now: i64,
) -> Result<Vec<Candidate>, IpcError> {
    let conv = st.claude_session_id.as_deref();
    let mut matches = recognize(prompt, &tv.ctx);
    // With no tracker at all, a key from a prompt is only a candidate when
    // fleet already knows it (a local item, an earlier link): unknown keys
    // come from branch names only (design §0.3).
    if tv.ctx.prefixes.is_empty() {
        let mut kept = Vec::new();
        for m in matches {
            if m.kind != MatchKind::Key || s.work_key_known(&m.key)? {
                kept.push(m);
            }
        }
        matches = kept;
    }
    let distinct: BTreeSet<&str> = matches.iter().map(|m| m.key.as_str()).collect();
    let dump = distinct.len() > DUMP_GUARD_MAX;
    let sole = first_prompt && distinct.len() == 1;
    Ok(matches
        .iter()
        .map(|m| {
            let signal = match m.kind {
                MatchKind::Url => Signal::PromptUrl,
                MatchKind::Key => Signal::PromptKey,
                MatchKind::RepoIssue => Signal::PromptIssue,
            };
            let strength = match (m.kind, dump) {
                (_, true) => Strength::Weak,
                (MatchKind::Url, _) => Strength::Strong,
                (MatchKind::Key, false) if sole => Strength::Strong,
                _ => Strength::Weak,
            };
            let mut ev = evidence(signal, &m.text, now, conv);
            if snippets {
                ev.snippet = Some(snippet(prompt, m.span));
            }
            if dump {
                ev.note = Some("reference".into());
            }
            Candidate {
                target: m.key.clone(),
                signal,
                strength,
                ambiguous: tv.ambiguous(&m.key, m.tracker_id),
                first_prompt_sole: sole && !dump,
                tracker_id: m.tracker_id,
                untracked: false,
                evidence: ev,
            }
        })
        .collect())
}

/// The trusted-projects setting as a set.
pub fn trusted_projects(s: &Store) -> BTreeSet<i64> {
    crate::service::settings::parse_id_set(&crate::service::settings::get_string(
        s,
        crate::service::settings::WORK_TRUSTED_BRANCH_PROJECTS,
    ))
    .unwrap_or_default()
}

/// Trust (or stop trusting) branch keys in `project_id`. Returns the set.
pub fn set_project_trust(s: &Store, project_id: i64, on: bool) -> Result<BTreeSet<i64>, IpcError> {
    if s.get_project(project_id)?.is_none() {
        return Err(IpcError::new(
            codes::E_NOTFOUND,
            format!("project {project_id} not found"),
        ));
    }
    let mut set = trusted_projects(s);
    if on {
        set.insert(project_id);
    } else {
        set.remove(&project_id);
    }
    let json = serde_json::to_string(&set.iter().collect::<Vec<_>>())
        .map_err(|e| IpcError::new(codes::E_INTERNAL, e.to_string()))?;
    crate::service::settings::set(
        s,
        crate::service::settings::WORK_TRUSTED_BRANCH_PROJECTS,
        &json,
    )?;
    Ok(set)
}

/// Resolve `session_id` from its stored state plus `events`, and apply the
/// changes. `true` when a link changed (the row was emitted).
pub fn resolve_with(s: &Store, session_id: i64, events: Vec<Candidate>) -> Result<bool, IpcError> {
    let Some(st) = s.detection_state(session_id)? else {
        return Ok(false);
    };
    let tv = tracker_view(s, st.repo.clone())?;
    run(s, &st, &tv, events)
}

fn run(
    s: &Store,
    st: &DetectionState,
    tv: &TrackerView,
    mut events: Vec<Candidate>,
) -> Result<bool, IpcError> {
    let now = crate::service::catalog::now_secs();
    let (branch, pr, pr_events) = state_candidates(st, tv, now);
    events.extend(pr_events);
    let links = s
        .detection_links(st.participant)?
        .into_iter()
        .map(|(l, target)| super::resolve::ExistingLink {
            id: l.id,
            target,
            strength: l.strength.as_deref().and_then(Strength::parse),
            claude_session_id: l.claude_session_id,
            is_primary: l.is_primary,
            decided_at: l.decided_at.unwrap_or(l.created_at),
            evidence_len: l.evidence.len(),
            state: l.state,
            source: l.source,
        })
        .collect();
    let input = ResolveInput {
        conversation: st.claude_session_id.clone(),
        branch,
        pr,
        events,
        links,
        trusted: st
            .project_id
            .is_some_and(|p| trusted_projects(s).contains(&p)),
    };
    let changes = resolve(&input);
    s.apply_link_changes(
        st.session_id,
        st.participant,
        st.claude_session_id.as_deref(),
        &changes,
    )
}

/// Re-resolve a session from its stored signals (a trigger with no new
/// event: a branch or PR change, a sync binding keys, a decision, a
/// conversation boundary). Best-effort for callers on a hook path: they log
/// and go on.
pub fn resolve_session(s: &Store, session_id: i64) -> Result<bool, IpcError> {
    resolve_with(s, session_id, Vec::new())
}

/// The UserPromptSubmit trigger: recognise references in the FULL prompt
/// (only matches are kept), apply the loop and dump guards, and resolve.
/// `first_prompt`: this is the conversation's first prompt.
pub fn on_prompt(
    s: &Store,
    session_id: i64,
    prompt: &str,
    first_prompt: bool,
) -> Result<bool, IpcError> {
    let Some(st) = s.detection_state(session_id)? else {
        return Ok(false);
    };
    let handovers = s.recent_handover_bodies(st.participant, 5)?;
    let tv = tracker_view(s, st.repo.clone())?;
    let events = match loop_guard(prompt, st.last_prompt.as_deref(), &handovers) {
        Some(why) => {
            tracing::debug!(session_id, why, "[work] prompt skipped by the loop guard");
            Vec::new()
        }
        None => {
            let snippets = crate::service::settings::get_bool(
                s,
                crate::service::settings::WORK_EVIDENCE_SNIPPETS,
            );
            let now = crate::service::catalog::now_secs();
            prompt_candidates(s, &st, &tv, prompt, first_prompt, snippets, now)?
        }
    };
    run(s, &st, &tv, events)
}

/// The agent's answer to the classification nudge (work graph M4.6):
/// `work_link { action: link, source: agent_inferred }`. Never a decision —
/// one [`Signal::AgentInferred`] event through the same resolver run as a
/// prompt, which makes it a pre-selected suggestion (R11) and keeps R9: a
/// pair the person rejected is not proposed again. `target` is normalised
/// (`ABC-7`); `tracker_id` binds it when the caller named an item.
pub fn on_agent_inference(
    s: &Store,
    session_id: i64,
    target: &str,
    tracker_id: Option<i64>,
) -> Result<bool, IpcError> {
    let Some(st) = s.detection_state(session_id)? else {
        return Ok(false);
    };
    let tv = tracker_view(s, st.repo.clone())?;
    let now = crate::service::catalog::now_secs();
    let ev = evidence(
        Signal::AgentInferred,
        target,
        now,
        st.claude_session_id.as_deref(),
    );
    let mut c = candidate(
        target.to_string(),
        Signal::AgentInferred,
        Strength::Inferred,
        ev,
    );
    c.tracker_id = tracker_id;
    run(s, &st, &tv, vec![c])
}

/// A person confirmed or rejected suggestion `link_id`. Confirming a branch
/// suggestion counts toward the project's automatic trust
/// ([`AUTO_TRUST_AFTER`]). Re-resolves afterwards (a rejection can leave a
/// sole candidate). Returns whether the project became trusted.
pub fn decide(s: &Store, session_id: i64, link_id: i64, confirm: bool) -> Result<bool, IpcError> {
    let link = s.decide_work_link(session_id, link_id, confirm)?;
    let mut trusted_now = false;
    if confirm && matches!(link.rule.as_deref(), Some("R3b" | "R4")) {
        if let Some(pid) = s.detection_state(session_id)?.and_then(|st| st.project_id) {
            if !trusted_projects(s).contains(&pid)
                && s.confirmed_branch_suggestions(pid)? >= AUTO_TRUST_AFTER
            {
                set_project_trust(s, pid, true)?;
                trusted_now = true;
                tracing::info!(project_id = pid, "[work] branch keys now trusted (auto)");
            }
        }
    }
    resolve_session(s, session_id)?;
    Ok(trusted_now)
}

#[cfg(test)]
mod tests;
