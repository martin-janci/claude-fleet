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
use crate::store::{Decider, DetectionState, Store};
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
            // An issue on an enterprise instance (M11.4) says so by its URL's
            // host: its key carries that host, so it can never be taken for
            // the same-named github.com repository's.
            let instance = r
                .get("url")
                .and_then(|u| u.as_str())
                .and_then(site_host)
                .filter(|h| crate::store::ghes_host_ok(h))
                .map(|h| format!("{h}/"))
                .unwrap_or_default();
            let key = match (owner, name, n) {
                (Some(o), Some(nm), Some(n)) => Some(format!(
                    "{instance}{}/{}#{n}",
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
    let trackers = s.list_trackers()?;
    for t in &trackers {
        for p in &t.config.key_prefixes {
            let p = p.to_ascii_uppercase();
            *owners.entry(p.clone()).or_default() += 1;
            if !ctx.prefixes.contains(&p) {
                ctx.prefixes.push(p);
            }
        }
    }
    ctx.trackers = tracker_hosts(&trackers);
    Ok(TrackerView {
        ctx,
        shared_prefixes: owners
            .into_iter()
            .filter(|(_, n)| *n > 1)
            .map(|(p, _)| p)
            .collect(),
    })
}

/// Configured trackers as recognition sees them: each one's site host (an
/// enterprise GitHub tracker's names its instance, M11.4).
pub(crate) fn tracker_hosts(trackers: &[crate::store::TrackerRow]) -> Vec<TrackerHost> {
    trackers
        .iter()
        .filter_map(|t| {
            site_host(&t.site_url).map(|host| TrackerHost {
                id: t.id,
                host,
                provider: t.provider.clone(),
            })
        })
        .collect()
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
    /// A GitHub tracker of `r`'s instance is configured: github.com's for
    /// `owner/repo#n`, the enterprise host's for `host/owner/repo#n`.
    fn github_instance_tracked(&self, r: &str) -> bool {
        let host = crate::store::github_ref(r)
            .and_then(|(repo, _)| crate::store::split_github_repo(repo))
            .map(|(h, _)| h.map(str::to_ascii_lowercase));
        let Some(host) = host else {
            return false;
        };
        self.ctx.trackers.iter().any(|t| {
            t.provider == "github"
                && match &host {
                    None => t.host == "github.com" || t.host == "www.github.com",
                    Some(h) => t.host.eq_ignore_ascii_case(h),
                }
        })
    }

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
    // tracker of ITS instance exists (M6; github.com or the enterprise host,
    // M11.4): never auto-linked before then (R3u).
    for r in sig.closing.iter().filter(|r| tv.admits(r)) {
        let mut c = candidate(
            r.clone(),
            Signal::PrClosing,
            Strength::Strong,
            evidence(Signal::PrClosing, r, now, conv),
        );
        c.untracked = r.contains('#') && !tv.github_instance_tracked(r);
        pr.push(c);
    }
    // A PR text naming more than the dump guard's worth of work (a release
    // PR, an audit) is a reference list: none of it is this session's work,
    // and dropping it here withdraws what a shorter text proposed (R7).
    let text_distinct: BTreeSet<&str> = sig.text.iter().map(String::as_str).collect();
    let no_refs = Vec::new();
    let text = if text_distinct.len() > DUMP_GUARD_MAX {
        &no_refs
    } else {
        &sig.text
    };
    let mut events = Vec::new();
    for (signal, refs) in [(Signal::PrText, text), (Signal::Trailer, &sig.trailers)] {
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

/// Why a prompt is not evidence (the loop guard, C14), or `None`. The one
/// test of "a person typed this": the census and the J1 benchmark
/// (`nl::census::FleetPrompts`) and the hook's touch go through it too.
pub fn loop_guard(
    prompt: &str,
    last_prompt: Option<&str>,
    handovers: &[String],
) -> Option<&'static str> {
    // Claude Code's own blocks at the head are nobody's words; what follows
    // them is checked as the prompt.
    let p = match crate::service::prompt_origin::human_part(prompt) {
        Some(p) => p,
        None if prompt.trim().is_empty() => return None,
        None => return Some("harness"),
    };
    let p: &str = &p;
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

/// The trusted-projects setting as a set. Best-effort (a failed read is the
/// empty set): a caller inside `Store::atomically` checks
/// `Store::ensure_in_tx` after it.
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

/// The detection state of `session_id`, or `None` for a session that is
/// never a detection subject: the operator (the UX agent). It coordinates
/// every task, so the keys it reads and names are the fleet's, not a sign of
/// what it works on — detecting them would hang a suggestion of every task on
/// it. Its conversations are tied to tasks by threads instead (task → session
/// spec §3.3).
fn subject_state(s: &Store, session_id: i64) -> Result<Option<DetectionState>, IpcError> {
    if crate::service::operator::operator_ref(s).is_some() {
        // `operator_ref` swallows its settings read, as `trusted_projects`
        // does in `run`.
        s.ensure_in_tx()?;
        if let Some(row) = s.get_session_by_id(session_id)? {
            if crate::service::operator::is_operator_session(s, &row.host_alias, &row.tmux_name) {
                return Ok(None);
            }
        }
    }
    s.detection_state(session_id)
}

/// Resolve `session_id` from its stored state plus `events`, and apply the
/// changes. `true` when a link changed (the row was emitted).
pub fn resolve_with(s: &Store, session_id: i64, events: Vec<Candidate>) -> Result<bool, IpcError> {
    let Some(st) = subject_state(s, session_id)? else {
        return Ok(false);
    };
    let tv = tracker_view(s, st.repo.clone())?;
    run(s, &st, &tv, events)
}

/// One spelling per target: a candidate naming a tracker item by an alias
/// (a moved issue) reads as the item's current key, the same key
/// `detection_links` labels its links with.
fn canonicalize<'a>(
    s: &Store,
    cands: impl Iterator<Item = &'a mut Candidate>,
) -> Result<(), IpcError> {
    for c in cands {
        let canonical = s.canonical_work_target(&c.target, c.tracker_id)?;
        if canonical != c.target {
            c.target = canonical;
        }
    }
    Ok(())
}

/// The raw values of a session's state signals, which a person's unlink
/// is held against (R9u): the branch, the PR's head branch, and the PR
/// itself (its URL, else `head:<branch>`; `None` when never probed or no
/// PR). Full values, not the evidence's capped text.
struct StateValues {
    branch: Option<String>,
    head: Option<String>,
    pr: Option<String>,
}

impl StateValues {
    fn of(st: &DetectionState) -> StateValues {
        let sig: Option<PrSignals> = st
            .pr_probed
            .then_some(st.pr_signals.as_deref())
            .flatten()
            .and_then(|j| serde_json::from_str(j).ok());
        let head = sig.as_ref().and_then(|p| p.head.clone());
        let pr = sig.as_ref().and_then(|_| {
            st.pr_url
                .clone()
                .or_else(|| head.as_ref().map(|h| format!("head:{h}")))
        });
        StateValues {
            branch: st.branch.clone(),
            head,
            pr,
        }
    }

    /// The `(signal, value)` a state candidate is held by, `None` for an
    /// event candidate.
    fn hold_of(&self, c: &Candidate) -> Option<(&'static str, &str)> {
        match c.signal {
            Signal::Branch => self.branch.as_deref().map(|v| ("branch", v)),
            // A PR's head IS a branch: holding the branch name holds it.
            Signal::PrHead => self.head.as_deref().map(|v| ("branch", v)),
            Signal::PrClosing => self.pr.as_deref().map(|v| ("pr", v)),
            _ => None,
        }
    }
}

/// R9u: drop the state candidates a person's unlink holds — the same
/// target from the same signal with the same value as when they cleared
/// it. Another value (the branch moved on, another PR) is not held; the
/// same value again is. Event candidates are never held.
fn drop_held(
    s: &Store,
    st: &DetectionState,
    branch: &mut Option<Vec<Candidate>>,
    pr: &mut Option<Vec<Candidate>>,
) -> Result<(), IpcError> {
    let holds = s.work_unlink_holds(st.participant)?;
    if holds.is_empty() {
        return Ok(());
    }
    let vals = StateValues::of(st);
    let held = |c: &Candidate| {
        vals.hold_of(c).is_some_and(|(signal, value)| {
            holds
                .iter()
                .any(|(t, sg, v)| *t == c.target && sg == signal && v == value)
        })
    };
    for v in branch.iter_mut().chain(pr.iter_mut()) {
        v.retain(|c| !held(c));
    }
    Ok(())
}

/// What a PERSON's unlink of `link_id` must hold (R9u): one `(signal,
/// value)` per current state signal that names the link's target — the
/// signals the next resolver run would make the same link again from (R3,
/// R3b). Empty when no state signal names it (a link from a prompt, or
/// one whose branch already moved on): a plain unlink then.
pub fn unlink_holds(
    s: &Store,
    session_id: i64,
    link_id: i64,
) -> Result<Vec<(&'static str, String)>, IpcError> {
    let Some(st) = s.detection_state(session_id)? else {
        return Ok(Vec::new());
    };
    let Some(target) = s
        .detection_links(st.participant)?
        .into_iter()
        .find(|(l, _)| l.id == link_id)
        .map(|(_, t)| t)
    else {
        return Ok(Vec::new());
    };
    let tv = tracker_view(s, st.repo.clone())?;
    let now = crate::service::catalog::now_secs();
    let (mut branch, mut pr, _) = state_candidates(&st, &tv, now);
    canonicalize(
        s,
        branch.iter_mut().flatten().chain(pr.iter_mut().flatten()),
    )?;
    let vals = StateValues::of(&st);
    let mut out: Vec<(&'static str, String)> = Vec::new();
    for c in branch.iter().flatten().chain(pr.iter().flatten()) {
        if c.target != target {
            continue;
        }
        if let Some((signal, value)) = vals.hold_of(c) {
            let hold = (signal, value.to_string());
            if !out.contains(&hold) {
                out.push(hold);
            }
        }
    }
    Ok(out)
}

fn run(
    s: &Store,
    st: &DetectionState,
    tv: &TrackerView,
    mut events: Vec<Candidate>,
) -> Result<bool, IpcError> {
    let now = crate::service::catalog::now_secs();
    let (mut branch, mut pr, pr_events) = state_candidates(st, tv, now);
    events.extend(pr_events);
    canonicalize(
        s,
        branch
            .iter_mut()
            .flatten()
            .chain(pr.iter_mut().flatten())
            .chain(events.iter_mut()),
    )?;
    // A person's "Clear work" holds against the unchanged state (R9u).
    drop_held(s, st, &mut branch, &mut pr)?;
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
    let trusted = st
        .project_id
        .is_some_and(|p| trusted_projects(s).contains(&p));
    // `trusted_projects` swallows its settings read, and a resolve with no
    // changes never reaches `apply_link_changes`' savepoint: a read SQLite
    // answered by rolling the caller's transaction back stops here.
    s.ensure_in_tx()?;
    let input = ResolveInput {
        conversation: st.claude_session_id.clone(),
        branch,
        pr,
        events,
        links,
        trusted,
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
/// `first_prompt`: this is the conversation's first prompt. Harness blocks
/// at the prompt's head are not the person's and are not read.
pub fn on_prompt(
    s: &Store,
    session_id: i64,
    prompt: &str,
    first_prompt: bool,
) -> Result<bool, IpcError> {
    let person = crate::service::prompt_origin::human_part(prompt);
    let prompt: &str = person.as_deref().unwrap_or(prompt);
    let Some(st) = subject_state(s, session_id)? else {
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
            // A swallowed settings read, like `trusted_projects` in `run`.
            s.ensure_in_tx()?;
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
    let Some(st) = subject_state(s, session_id)? else {
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

/// The decision model's answer for a session no rule could link (J1
/// `work_link` in assist, [`crate::service::decide::work_link`]): one
/// [`Signal::Jev`] event through the same resolver run as the agent's
/// inference, which makes it a pre-selected suggestion (R12) and keeps R9.
/// `target` is normalised (`ABC-7`); `tracker_id` binds it to the item.
/// The model's confidence rides the evidence's note (`82%`), which Review
/// and the row's popover show beside "Proposed by Jev".
pub fn on_jev_proposal(
    s: &Store,
    session_id: i64,
    target: &str,
    tracker_id: Option<i64>,
    confidence_pct: Option<u8>,
) -> Result<bool, IpcError> {
    let Some(st) = subject_state(s, session_id)? else {
        return Ok(false);
    };
    let tv = tracker_view(s, st.repo.clone())?;
    let now = crate::service::catalog::now_secs();
    let mut ev = evidence(Signal::Jev, target, now, st.claude_session_id.as_deref());
    ev.note = confidence_pct.map(|p| format!("{p}%"));
    let mut c = candidate(target.to_string(), Signal::Jev, Strength::Inferred, ev);
    c.tracker_id = tracker_id;
    run(s, &st, &tv, vec![c])
}

/// `decider` confirmed or rejected suggestion `link_id`; the link records
/// who ([`Store::decide_work_link`]). A person confirming a branch
/// suggestion counts toward the project's automatic trust
/// ([`AUTO_TRUST_AFTER`]); an agent's confirmation never does. Re-resolves
/// afterwards (a rejection can leave a sole candidate). Returns whether the
/// project became trusted.
pub fn decide(
    s: &Store,
    session_id: i64,
    link_id: i64,
    confirm: bool,
    decider: Decider,
) -> Result<bool, IpcError> {
    decide_as(s, session_id, link_id, confirm, true, decider)
}

/// [`decide`], confirming as a secondary link when `take_primary` is false
/// (work graph M14.1c).
pub fn decide_as(
    s: &Store,
    session_id: i64,
    link_id: i64,
    confirm: bool,
    take_primary: bool,
    decider: Decider,
) -> Result<bool, IpcError> {
    let link = s.decide_work_link_as(session_id, link_id, confirm, take_primary, decider)?;
    if decider == Decider::Person {
        // J1 (redesign 6.8): a person's answer is the label of the decision
        // model's proposal about this session, whichever link it was on.
        let now = crate::service::catalog::now_secs();
        if let Err(e) = crate::service::decide::work_link::record_decision(
            s,
            session_id,
            link.rule.as_deref(),
            link.item_id,
            confirm,
            now,
        ) {
            tracing::warn!("[decide] work_link follow-up not recorded: {}", e.message);
        }
        // J6 (redesign 6.8): the same answer labels the main-ticket proposal.
        if let Err(e) = crate::service::decide::main_ticket::record_decision(
            s, session_id, link_id, confirm, now,
        ) {
            tracing::warn!("[decide] main_ticket follow-up not recorded: {}", e.message);
        }
    }
    let mut trusted_now = false;
    if confirm && decider == Decider::Person && matches!(link.rule.as_deref(), Some("R3b" | "R4")) {
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
