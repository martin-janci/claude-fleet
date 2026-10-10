//! Outcome fields for work sessions (PROD-5): `pr_url` and a reduced CI
//! status, populated during reconcile from `gh pr view` run in the session's
//! worktree.
//!
//! Cost control: one shell round-trip per host per pass covers every session
//! due for a probe, and each session is re-probed at most once per
//! [`PR_PROBE_TTL`]. A host without `gh` on PATH is remembered for the same
//! window so a fleet of plain shell boxes never pays for the call. Everything
//! here except the cache is pure and unit-tested; the reconcile wiring in
//! `service::sessions` is thin.

use crate::shell::quote;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long a session's PR probe result is trusted before `gh` is asked again.
pub const PR_PROBE_TTL: Duration = Duration::from_secs(300);

/// How often a steady PR's `sessions.pr_checked_at` is re-stamped. A changed
/// reading is stamped at once; an unchanged one only when the stored stamp
/// is at least this old, so a PR that sits green costs one row event per
/// refresh instead of one per probe. Two probe windows.
pub const PR_CHECKED_REFRESH_SECS: i64 = 2 * PR_PROBE_TTL.as_secs() as i64;

/// A `pr_checked_at` older than this describes the past: the probe has not
/// observed the PR for a while (host unreachable, `gh` failing). Three
/// probe windows, so a steady PR re-stamped every
/// [`PR_CHECKED_REFRESH_SECS`] and probed every [`PR_PROBE_TTL`] never
/// crosses it while its probes succeed.
pub const PR_EVIDENCE_STALE_SECS: i64 = 3 * PR_PROBE_TTL.as_secs() as i64;

/// Line prefix the probe script prints before each session's result.
const RESULT_PREFIX: &str = "__FLEET_PR__\t";
/// Sentinel the script prints (and exits with) when `gh` is not on PATH.
const NO_GH_MARKER: &str = "__FLEET_NO_GH__";
/// Sentinel for `gh` present but not authenticated (`gh auth status` failed):
/// every `gh pr view` would fail, so the host is skipped for one TTL window.
const NO_AUTH_MARKER: &str = "__FLEET_NO_AUTH__";
/// Exit code the script reports when the session's cwd no longer exists.
const RC_NO_CWD: &str = "97";
/// Line prefix of a session's commit messages since its upstream (work
/// graph M4.2: commit trailers). Lines are joined with `\x1f`.
const TRAILERS_PREFIX: &str = "__FLEET_TRAILERS__\t";
/// Line prefix of a session's worktree state (result evidence §1):
/// `<name>\t<HEAD>\t<commits ahead of upstream>\t<dirty 0|1>`, each field
/// empty when git could not answer it.
const GIT_PREFIX: &str = "__FLEET_GIT__\t";
/// The `gh pr view` fields fleet reads. `headRefName` … `closingIssuesReferences`
/// are work detection's (M4.2): read, parsed and dropped — the body is capped
/// at 4k by `--jq` and never stored. `state` tells tidy-up a merged PR (M7).
/// `headRefOid` … `isDraft` are result evidence's: the commit the checks
/// describe, and what else stands between the PR and a merge.
/// `mergedAt` is the Pull requests view's (redesign 6.4); `additions` and
/// `deletions` its diffstat (gap plan G3.10).
const PR_FIELDS: &str =
    "url,statusCheckRollup,headRefName,title,body,closingIssuesReferences,state,\
     headRefOid,reviewDecision,mergeStateStatus,isDraft,mergedAt,additions,deletions";
/// The fields an older `gh` without `closingIssuesReferences` (or `--jq`)
/// still answers: the probe falls back to them.
const PR_FIELDS_BASIC: &str = "url,statusCheckRollup";

/// One session's probe result. `pr_url == None` means "no open PR for this
/// branch" (gh exited non-zero) — the caller clears a stale link.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PrInfo {
    pub pr_url: Option<String>,
    /// `passing` | `failing` | `pending`, or `None` when the PR has no checks.
    pub ci_status: Option<String>,
    /// What work detection reads from the PR (M4.2), `None` without a PR or
    /// when the host's `gh` answered only the basic fields.
    pub signals: Option<crate::service::work::detect::PrSignals>,
    /// What the evidence card reads (result evidence §1), `None` without a
    /// PR or when the host's `gh` answered only the basic fields.
    pub evidence: Option<PrEvidence>,
}

/// One PR's evidence as the probe read it: the commit GitHub's checks
/// describe, the worktree's own commit next to it, and what else stands
/// between the PR and a merge. Stored as JSON in `sessions.pr_evidence`
/// (design `docs/specs/2026-09-29-result-evidence-design.md` §2). Every
/// field is optional on the wire: an absent one is "not observed", never
/// "fine".
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct PrEvidence {
    /// `headRefOid`: the commit the check rollup belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_oid: Option<String>,
    /// The worktree's `HEAD` when the probe ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_head: Option<String>,
    /// Commits on `HEAD` not on its upstream; `None` without an upstream.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ahead: Option<u32>,
    /// Tracked files differ from `HEAD`; `None` when git could not tell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dirty: Option<bool>,
    #[serde(default)]
    pub draft: bool,
    /// `reviewDecision`: APPROVED | CHANGES_REQUESTED | REVIEW_REQUIRED;
    /// `None` for GitHub's `""` (the repository requires no review).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_decision: Option<String>,
    /// `mergeStateStatus`: CLEAN | BLOCKED | BEHIND | DIRTY | UNSTABLE |
    /// HAS_HOOKS | DRAFT | UNKNOWN.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge_state: Option<String>,
    /// `state`: OPEN | CLOSED | MERGED. Also in `pr_signals` (tidy-up reads
    /// it there); here so the assessment can say "merged" without it.
    /// Absent from readings stored before it was added.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(default)]
    pub checks: CheckSummary,
    /// `title`, `headRefName` and `mergedAt` (unix seconds): what the Pull
    /// requests view lists (redesign 6.4, `pull_requests`). Absent from
    /// readings stored before they were added.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merged_at: Option<i64>,
    /// `additions` / `deletions`: the PR's diffstat, lines added and removed
    /// (gap plan G3.10, "+18 −6"). Absent from readings stored before they
    /// were added.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additions: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deletions: Option<u32>,
}

/// Most characters of a PR title kept for the Pull requests view.
pub const PR_TITLE_MAX: usize = 200;

/// The check rollup, counted (result evidence §1). `failing` is non-empty
/// exactly when [`reduce_ci_status`] says `failing`: both read
/// [`classify_check`].
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct CheckSummary {
    #[serde(default)]
    pub total: u32,
    #[serde(default)]
    pub pending: u32,
    /// Skipped or neutral: neither a pass nor a blocker.
    #[serde(default)]
    pub skipped: u32,
    /// At most [`FAILING_CHECKS_MAX`]; `failing_total` counts them all.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failing: Vec<FailingCheck>,
    #[serde(default)]
    pub failing_total: u32,
}

/// One failing check, by the name GitHub shows.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FailingCheck {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// Most failing checks one summary names.
pub const FAILING_CHECKS_MAX: usize = 5;

/// Build the per-host probe script. `targets` is `(tmux_name, cwd)` for every
/// session due this pass. Each value is shell-quoted; the output is one
/// `__FLEET_PR__\t<name>\t<gh exit code>\t<stdout+stderr on one line>` line
/// per target. The exit code lets the parser tell "no PR for this branch"
/// (a real observation that clears a stale link) from "gh failed" (offline,
/// rate-limited, repo not on GitHub — not an observation).
pub fn build_pr_probe_script(targets: &[(String, String)]) -> String {
    let mut script = String::new();
    script.push_str(&format!(
        "command -v gh >/dev/null 2>&1 || {{ echo {NO_GH_MARKER}; exit 0; }}\n\
         gh auth status >/dev/null 2>&1 || {{ echo {NO_AUTH_MARKER}; exit 0; }}\n"
    ));
    for (name, cwd) in targets {
        // printf (builtin and /usr/bin) expands `\t` in the FORMAT string on
        // every platform, so the tab-separated record is built with it. The
        // payload has tabs/newlines squeezed out so one record is one line.
        // The full field list first; an older `gh` that refuses a field or
        // `--jq` gets the basic list (a "no pull requests found" answer is
        // already final). Then the commit messages since the upstream, for
        // their trailers: no extra round trip, at most 200 lines / 8k. Then
        // the worktree's HEAD, its lead on the upstream and whether tracked
        // files differ from HEAD (result evidence §1): local reads, no
        // network. `git diff --quiet` exits 1 for a change and >1 when it
        // cannot tell, which must not read as "clean".
        script.push_str(&format!(
            "if cd {cwd} 2>/dev/null; then \
             out=\"$(gh pr view --json {fields} --jq '.body |= ((. // \"\")[0:4000])' 2>&1)\"; rc=$?; \
             if [ \"$rc\" -ne 0 ] && ! printf '%s' \"$out\" | grep -qi 'no pull requests found'; then \
             out=\"$(gh pr view --json {basic} 2>&1)\"; rc=$?; fi; \
             msgs=\"$(git log --format=%B '@{{u}}..HEAD' 2>/dev/null | head -n 200 | head -c 8000 | tr '\\n\\t' '\\037 ')\"; \
             head=\"$(git rev-parse HEAD 2>/dev/null)\"; \
             ahead=\"$(git rev-list --count '@{{u}}..HEAD' 2>/dev/null)\"; \
             git diff --quiet HEAD -- 2>/dev/null; \
             case $? in 0) dirty=0;; 1) dirty=1;; *) dirty='';; esac; \
             else out=''; rc={rc_no_cwd}; msgs=''; head=''; ahead=''; dirty=''; fi; \
             printf '{prefix}%s\\t%s\\t%s\\n' {name} \"$rc\" \"$(printf '%s' \"$out\" | tr -d '\\n\\t')\"; \
             printf '{tprefix}%s\\t%s\\n' {name} \"$msgs\"; \
             printf '{gprefix}%s\\t%s\\t%s\\t%s\\n' {name} \"$head\" \"$ahead\" \"$dirty\"\n",
            name = quote(name),
            cwd = quote(cwd),
            fields = PR_FIELDS,
            basic = PR_FIELDS_BASIC,
            rc_no_cwd = RC_NO_CWD,
            prefix = RESULT_PREFIX.replace('\t', "\\t"),
            tprefix = TRAILERS_PREFIX.replace('\t', "\\t"),
            gprefix = GIT_PREFIX.replace('\t', "\\t"),
        ));
    }
    script
}

/// Outcome of parsing one probe run.
#[derive(Debug, PartialEq, Eq)]
pub enum ProbeOutput {
    /// `gh` is not installed on the host — nothing was probed.
    NoGh,
    /// `gh` is installed but not logged in — nothing was probed.
    NoAuth,
    /// Per-session results, keyed by tmux_name. Only sessions that were
    /// actually OBSERVED are present: a PR (`pr_url: Some`) or a definite
    /// "no pull requests found" (`PrInfo::default()`). A target whose `gh`
    /// call failed for any other reason (offline, rate limit, not a GitHub
    /// repo, cwd gone) is absent so its stored fields survive.
    Results(HashMap<String, PrInfo>),
}

/// Parse the probe script's stdout. Tolerant: unknown lines are skipped and a
/// malformed JSON blob counts as "no PR" rather than aborting the pass.
pub fn parse_pr_probe_output(stdout: &str) -> ProbeOutput {
    let mut out = HashMap::new();
    let mut trailers: HashMap<String, String> = HashMap::new();
    let mut git: HashMap<String, GitState> = HashMap::new();
    for line in stdout.lines() {
        match line.trim() {
            NO_GH_MARKER => return ProbeOutput::NoGh,
            NO_AUTH_MARKER => return ProbeOutput::NoAuth,
            _ => {}
        }
        if let Some(rest) = line.strip_prefix(GIT_PREFIX) {
            let mut f = rest.split('\t');
            if let Some(name) = f.next() {
                let state = parse_git_state(
                    f.next().unwrap_or(""),
                    f.next().unwrap_or(""),
                    f.next().unwrap_or(""),
                );
                git.insert(name.to_string(), state);
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix(TRAILERS_PREFIX) {
            if let Some((name, msgs)) = rest.split_once('\t') {
                trailers.insert(name.to_string(), msgs.replace('\u{1f}', "\n"));
            }
            continue;
        }
        let Some(rest) = line.strip_prefix(RESULT_PREFIX) else {
            continue;
        };
        let mut parts = rest.splitn(3, '\t');
        let (Some(name), Some(rc), payload) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        let payload = payload.unwrap_or("");
        if let Some(info) = classify_probe_record(rc, payload) {
            out.insert(name.to_string(), info);
        }
    }
    // Trailers ride only a PR fleet read the other fields of: without them
    // they would be the one PR signal left, and a branch with no PR has no
    // PR to explain.
    for (name, msgs) in trailers {
        if let Some(sig) = out.get_mut(&name).and_then(|i| i.signals.as_mut()) {
            sig.add_trailers(&msgs);
        }
    }
    // Likewise the worktree's git state: it is evidence about a PR, so it
    // rides only a PR whose evidence fields `gh` answered.
    for (name, st) in git {
        if let Some(ev) = out.get_mut(&name).and_then(|i| i.evidence.as_mut()) {
            ev.local_head = st.head;
            ev.ahead = st.ahead;
            ev.dirty = st.dirty;
        }
    }
    ProbeOutput::Results(out)
}

/// One `__FLEET_GIT__` record, each field `None` when git did not answer.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct GitState {
    head: Option<String>,
    ahead: Option<u32>,
    dirty: Option<bool>,
}

/// Pure: read the three git fields. A HEAD that is not a hex object id, a
/// count that is not a number (no upstream prints nothing) and a dirty flag
/// other than `0` / `1` are all "not observed".
fn parse_git_state(head: &str, ahead: &str, dirty: &str) -> GitState {
    let head = head.trim();
    GitState {
        head: (head.len() >= 7 && head.len() <= 64 && head.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| head.to_ascii_lowercase()),
        ahead: ahead.trim().parse().ok(),
        dirty: match dirty.trim() {
            "0" => Some(false),
            "1" => Some(true),
            _ => None,
        },
    }
}

/// Pure: turn one `(gh exit code, output)` record into an observation, or
/// `None` when the call proves nothing about the branch's PR state.
pub fn classify_probe_record(rc: &str, payload: &str) -> Option<PrInfo> {
    if rc.trim() == "0" {
        let info = pr_info_from_json(payload);
        // Exit 0 without a URL means we could not parse gh's answer — not an
        // observation either.
        return if info.pr_url.is_some() {
            Some(info)
        } else {
            None
        };
    }
    let lower = payload.to_ascii_lowercase();
    if lower.contains("no pull requests found") {
        return Some(PrInfo::default());
    }
    None
}

/// Reduce one `gh pr view --json url,statusCheckRollup` payload.
pub fn pr_info_from_json(json: &str) -> PrInfo {
    let v: serde_json::Value = match serde_json::from_str(json.trim()) {
        Ok(v) => v,
        Err(_) => return PrInfo::default(),
    };
    let pr_url = v
        .get("url")
        .and_then(|u| u.as_str())
        .filter(|u| u.starts_with("https://"))
        .map(|u| u.to_string());
    if pr_url.is_none() {
        return PrInfo::default();
    }
    let ci_status = v
        .get("statusCheckRollup")
        .and_then(|r| r.as_array())
        .and_then(|checks| reduce_ci_status(checks));
    // Only a `gh` that answered the detection fields gives signals; the
    // basic fallback's answer leaves the stored ones alone.
    let signals = v
        .get("headRefName")
        .is_some()
        .then(|| crate::service::work::detect::PrSignals::from_gh_json(&v));
    let evidence = v
        .get("headRefOid")
        .is_some()
        .then(|| evidence_from_json(&v));
    PrInfo {
        pr_url,
        ci_status,
        signals,
        evidence,
    }
}

/// Pure: the evidence fields of one full `gh pr view` answer. The worktree
/// fields are filled in by the parser from the `__FLEET_GIT__` line.
fn evidence_from_json(v: &serde_json::Value) -> PrEvidence {
    let text = |k: &str| {
        v.get(k)
            .and_then(|x| x.as_str())
            .map(str::trim)
            .filter(|x| !x.is_empty())
            .map(str::to_string)
    };
    let count = |k: &str| {
        v.get(k)
            .and_then(|x| x.as_u64())
            .and_then(|n| u32::try_from(n).ok())
    };
    PrEvidence {
        head_oid: text("headRefOid").map(|o| o.to_ascii_lowercase()),
        review_decision: text("reviewDecision").map(|d| d.to_ascii_uppercase()),
        merge_state: text("mergeStateStatus").map(|m| m.to_ascii_uppercase()),
        state: text("state").map(|m| m.to_ascii_uppercase()),
        title: text("title").map(|t| t.chars().take(PR_TITLE_MAX).collect()),
        head_ref: text("headRefName"),
        merged_at: text("mergedAt").and_then(|t| crate::service::account_usage::parse_rfc3339(&t)),
        draft: v.get("isDraft").and_then(|d| d.as_bool()).unwrap_or(false),
        additions: count("additions"),
        deletions: count("deletions"),
        checks: v
            .get("statusCheckRollup")
            .and_then(|r| r.as_array())
            .map(|c| summarize_checks(c))
            .unwrap_or_default(),
        ..PrEvidence::default()
    }
}

/// Collapse a PR's check rollup to one badge: any failure wins, then any
/// still-running check, otherwise passing. `None` when there are no checks.
///
/// GitHub's rollup mixes two shapes: check runs (`status` +
/// `conclusion`) and commit statuses (`state`). Both are handled, by
/// [`classify_check`].
pub fn reduce_ci_status(checks: &[serde_json::Value]) -> Option<String> {
    if checks.is_empty() {
        return None;
    }
    let mut pending = false;
    for c in checks {
        match classify_check(c) {
            CheckState::Failing => return Some("failing".into()),
            CheckState::Pending => pending = true,
            CheckState::Passing | CheckState::Skipped => {}
        }
    }
    Some(if pending { "pending" } else { "passing" }.into())
}

/// Count a PR's check rollup for the evidence card: how many, how many
/// still running or skipped, and the failing ones by name.
pub fn summarize_checks(checks: &[serde_json::Value]) -> CheckSummary {
    let mut out = CheckSummary {
        total: checks.len() as u32,
        ..CheckSummary::default()
    };
    for c in checks {
        match classify_check(c) {
            CheckState::Failing => {
                out.failing_total += 1;
                if out.failing.len() < FAILING_CHECKS_MAX {
                    let field = |k: &str| {
                        c.get(k)
                            .and_then(|x| x.as_str())
                            .filter(|x| !x.is_empty())
                            .map(str::to_string)
                    };
                    out.failing.push(FailingCheck {
                        // Check runs have a `name`, commit statuses a `context`.
                        name: field("name")
                            .or_else(|| field("context"))
                            .unwrap_or_else(|| "(unnamed check)".into()),
                        url: field("detailsUrl")
                            .or_else(|| field("targetUrl"))
                            .filter(|u| u.starts_with("https://")),
                    });
                }
            }
            CheckState::Pending => out.pending += 1,
            CheckState::Skipped => out.skipped += 1,
            CheckState::Passing => {}
        }
    }
    out
}

/// What one rollup entry says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckState {
    Failing,
    Pending,
    /// A skipped or neutral run: it blocks nothing and proves nothing.
    Skipped,
    Passing,
}

/// Pure: classify one rollup entry, a check run or a commit status. The
/// single definition [`reduce_ci_status`] and [`summarize_checks`] share, so
/// the badge and the card cannot disagree about a check.
pub fn classify_check(c: &serde_json::Value) -> CheckState {
    let upper = |k: &str| {
        c.get(k)
            .and_then(|v| v.as_str())
            .map(|s| s.to_ascii_uppercase())
    };
    let (conclusion, status, state) = (upper("conclusion"), upper("status"), upper("state"));
    match (conclusion.as_deref(), status.as_deref(), state.as_deref()) {
        (
            Some("FAILURE" | "TIMED_OUT" | "CANCELLED" | "ACTION_REQUIRED" | "STARTUP_FAILURE"),
            _,
            _,
        )
        | (_, _, Some("FAILURE" | "ERROR")) => CheckState::Failing,
        (_, Some("QUEUED" | "IN_PROGRESS" | "PENDING" | "WAITING" | "REQUESTED"), _)
        | (_, _, Some("PENDING" | "EXPECTED")) => CheckState::Pending,
        // Check run still without a conclusion is in flight.
        (None, Some(_), None) => CheckState::Pending,
        (Some("SKIPPED" | "NEUTRAL"), _, _) => CheckState::Skipped,
        _ => CheckState::Passing,
    }
}

/// Process-wide probe throttle: `(host, tmux_name) → last probe`, plus the
/// per-host "gh missing" memo. Lives outside `ReconcileDeps` because every
/// entry point builds fresh deps; tests use [`PrProbeCache::new`] directly.
pub struct PrProbeCache {
    inner: Mutex<CacheInner>,
    ttl: Duration,
}

#[derive(Default)]
struct CacheInner {
    probed: HashMap<(String, String), Instant>,
    no_gh: HashMap<String, Instant>,
}

impl PrProbeCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            inner: Mutex::new(CacheInner::default()),
            ttl,
        }
    }

    /// The sessions on `host` (from `candidates`) whose last probe is older
    /// than the TTL. Empty when the host is memoised as having no `gh`.
    pub fn due<'a>(
        &self,
        host: &str,
        candidates: &'a [(String, String)],
    ) -> Vec<&'a (String, String)> {
        let inner = match self.inner.lock() {
            Ok(g) => g,
            Err(_) => return Vec::new(),
        };
        if inner
            .no_gh
            .get(host)
            .map(|t| t.elapsed() < self.ttl)
            .unwrap_or(false)
        {
            return Vec::new();
        }
        candidates
            .iter()
            .filter(|(name, _)| {
                inner
                    .probed
                    .get(&(host.to_string(), name.clone()))
                    .map(|t| t.elapsed() >= self.ttl)
                    .unwrap_or(true)
            })
            .collect()
    }

    /// Stamp the sessions that were just probed.
    pub fn mark_probed<'a>(&self, host: &str, names: impl IntoIterator<Item = &'a str>) {
        if let Ok(mut inner) = self.inner.lock() {
            let now = Instant::now();
            for n in names {
                inner.probed.insert((host.to_string(), n.to_string()), now);
            }
        }
    }

    /// Remember that `host` has no `gh` for one TTL window.
    pub fn mark_no_gh(&self, host: &str) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.no_gh.insert(host.to_string(), Instant::now());
        }
    }

    /// Drop stamps for sessions that no longer exist on `host` so the map
    /// can't grow with every session ever seen.
    pub fn retain_host(&self, host: &str, live: &[String]) {
        if let Ok(mut inner) = self.inner.lock() {
            inner
                .probed
                .retain(|(h, n), _| h != host || live.iter().any(|l| l == n));
        }
    }
}

/// The shared cache used by production reconcile.
pub fn pr_probe_cache() -> Arc<PrProbeCache> {
    static CACHE: std::sync::LazyLock<Arc<PrProbeCache>> =
        std::sync::LazyLock::new(|| Arc::new(PrProbeCache::new(PR_PROBE_TTL)));
    Arc::clone(&CACHE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_guards_on_gh_and_quotes_every_value() {
        let script = build_pr_probe_script(&[(
            "dev-x".into(),
            "/home/u/projects/github.com/o/r/.worktrees/it's".into(),
        )]);
        assert!(script.starts_with("command -v gh >/dev/null"));
        assert!(script.contains("__FLEET_NO_GH__"));
        assert!(script.contains("gh auth status"));
        assert!(script.contains("__FLEET_NO_AUTH__"));
        assert!(script.contains("'dev-x'"));
        // The single quote in the path is escaped by `quote`.
        assert!(script.contains("it'\\''s"));
        assert!(script.contains("gh pr view --json url,statusCheckRollup"));
    }

    /// Work detection (M4.2): the probe asks for the detection fields with
    /// the body capped, falls back to the basic fields for an older `gh`,
    /// and reads the commit messages since the upstream in the same script.
    #[test]
    fn script_reads_detection_fields_and_trailers_in_the_same_round_trip() {
        let script = build_pr_probe_script(&[("dev".into(), "/w/x".into())]);
        assert!(script.contains(&format!("gh pr view --json {PR_FIELDS} --jq")));
        assert!(
            script.contains("[0:4000]"),
            "the body is capped by gh itself"
        );
        assert!(script.contains(&format!("gh pr view --json {PR_FIELDS_BASIC} 2>&1")));
        assert!(script.contains("git log --format=%B '@{u}..HEAD'"));
        assert!(script.contains("head -n 200"));
    }

    #[test]
    fn parse_attaches_signals_and_trailers_to_an_observed_pr() {
        let stdout = "__FLEET_PR__\tdev\t0\t{\"url\":\"https://github.com/o/r/pull/7\",\
             \"statusCheckRollup\":[],\"headRefName\":\"abc-1-x\",\"title\":\"t\",\
             \"body\":null,\"closingIssuesReferences\":[]}\n\
             __FLEET_TRAILERS__\tdev\tfix it\u{1f}\u{1f}Refs: ABC-2\u{1f}\n\
             __FLEET_PR__\told\t0\t{\"url\":\"https://github.com/o/r/pull/8\",\"statusCheckRollup\":[]}\n\
             __FLEET_TRAILERS__\told\tRefs: ABC-3\n\
             __FLEET_TRAILERS__\tnopr\tRefs: ABC-4\n";
        let ProbeOutput::Results(map) = parse_pr_probe_output(stdout) else {
            panic!("results");
        };
        let sig = map["dev"].signals.as_ref().expect("signals");
        assert_eq!(sig.head.as_deref(), Some("abc-1-x"));
        assert_eq!(sig.trailers, vec!["ABC-2"]);
        assert_eq!(map["old"].signals, None, "basic fields only: no signals");
        assert!(!map.contains_key("nopr"));
    }

    #[test]
    fn parse_detects_missing_gh_and_missing_auth() {
        assert_eq!(
            parse_pr_probe_output("__FLEET_NO_GH__\n"),
            ProbeOutput::NoGh
        );
        assert_eq!(
            parse_pr_probe_output("__FLEET_NO_AUTH__\n"),
            ProbeOutput::NoAuth
        );
    }

    #[test]
    fn classify_only_clears_on_a_definite_no_pr_answer() {
        // Real PR.
        let ok = classify_probe_record("0", "{\"url\":\"https://x/pull/1\"}").unwrap();
        assert_eq!(ok.pr_url.as_deref(), Some("https://x/pull/1"));
        // gh's own "nothing here" answer ⇒ observed, clears.
        assert_eq!(
            classify_probe_record("1", "no pull requests found for branch \"feat\""),
            Some(PrInfo::default())
        );
        // Anything else ⇒ not an observation.
        assert_eq!(
            classify_probe_record("1", "error connecting to api.github.com"),
            None
        );
        assert_eq!(
            classify_probe_record("1", "HTTP 403: API rate limit exceeded"),
            None
        );
        assert_eq!(classify_probe_record("97", ""), None);
        assert_eq!(classify_probe_record("0", "not json"), None);
    }

    #[test]
    fn parse_maps_observed_targets_and_skips_failed_calls() {
        let stdout = "__FLEET_PR__\tdev-a\t0\t{\"url\":\"https://github.com/o/r/pull/7\",\"statusCheckRollup\":[]}\n\
                      __FLEET_PR__\tdev-b\t1\tno pull requests found for branch \"b\"\n\
                      __FLEET_PR__\tdev-c\t1\terror connecting to api.github.com\n\
                      __FLEET_PR__\tdev-d\t97\t\n\
                      garbage line\n";
        let ProbeOutput::Results(map) = parse_pr_probe_output(stdout) else {
            panic!("expected results");
        };
        assert_eq!(
            map.get("dev-a").unwrap().pr_url.as_deref(),
            Some("https://github.com/o/r/pull/7")
        );
        assert_eq!(map.get("dev-a").unwrap().ci_status, None);
        assert_eq!(map.get("dev-b"), Some(&PrInfo::default()));
        assert!(
            !map.contains_key("dev-c"),
            "transport failure is not an observation"
        );
        assert!(
            !map.contains_key("dev-d"),
            "missing cwd is not an observation"
        );
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn pr_info_rejects_non_https_url_and_bad_json() {
        assert_eq!(
            pr_info_from_json("{\"url\":\"javascript:alert(1)\"}"),
            PrInfo::default()
        );
        assert_eq!(pr_info_from_json("not json"), PrInfo::default());
    }

    fn check(status: &str, conclusion: Option<&str>) -> serde_json::Value {
        let mut v = serde_json::json!({ "status": status });
        if let Some(c) = conclusion {
            v["conclusion"] = serde_json::Value::String(c.into());
        }
        v
    }

    #[test]
    fn ci_status_failure_beats_pending_beats_passing() {
        assert_eq!(reduce_ci_status(&[]), None);
        assert_eq!(
            reduce_ci_status(&[check("COMPLETED", Some("SUCCESS"))]).as_deref(),
            Some("passing")
        );
        assert_eq!(
            reduce_ci_status(&[
                check("COMPLETED", Some("SUCCESS")),
                check("IN_PROGRESS", None)
            ])
            .as_deref(),
            Some("pending")
        );
        assert_eq!(
            reduce_ci_status(&[
                check("IN_PROGRESS", None),
                check("COMPLETED", Some("FAILURE"))
            ])
            .as_deref(),
            Some("failing")
        );
        // Legacy commit-status shape.
        assert_eq!(
            reduce_ci_status(&[serde_json::json!({ "state": "ERROR" })]).as_deref(),
            Some("failing")
        );
        assert_eq!(
            reduce_ci_status(&[serde_json::json!({ "state": "SUCCESS" })]).as_deref(),
            Some("passing")
        );
        // Skipped / neutral runs do not block a passing verdict.
        assert_eq!(
            reduce_ci_status(&[
                check("COMPLETED", Some("SKIPPED")),
                check("COMPLETED", Some("NEUTRAL"))
            ])
            .as_deref(),
            Some("passing")
        );
    }

    #[test]
    fn cache_throttles_per_session_and_memoises_missing_gh() {
        let cache = PrProbeCache::new(Duration::from_secs(60));
        let cands = vec![
            ("a".to_string(), "/a".to_string()),
            ("b".to_string(), "/b".to_string()),
        ];
        assert_eq!(cache.due("h", &cands).len(), 2);
        cache.mark_probed("h", ["a"]);
        let due: Vec<_> = cache
            .due("h", &cands)
            .iter()
            .map(|(n, _)| n.as_str())
            .collect();
        assert_eq!(due, vec!["b"]);
        // A different host is independent.
        assert_eq!(cache.due("other", &cands).len(), 2);
        cache.mark_no_gh("h");
        assert!(cache.due("h", &cands).is_empty());
        assert_eq!(cache.due("other", &cands).len(), 2);
    }

    #[test]
    fn cache_ttl_zero_means_always_due() {
        let cache = PrProbeCache::new(Duration::from_secs(0));
        let cands = vec![("a".to_string(), "/a".to_string())];
        cache.mark_probed("h", ["a"]);
        assert_eq!(cache.due("h", &cands).len(), 1);
    }

    #[test]
    fn cache_retain_drops_dead_sessions() {
        let cache = PrProbeCache::new(Duration::from_secs(60));
        cache.mark_probed("h", ["a", "b"]);
        cache.retain_host("h", &["a".to_string()]);
        let cands = vec![("b".to_string(), "/b".to_string())];
        assert_eq!(cache.due("h", &cands).len(), 1, "b's stamp was dropped");
    }

    // ── result evidence (docs/specs/2026-09-29-result-evidence-design.md) ──

    /// A full `gh pr view` answer shaped like claude-fleet #381 (measured
    /// 2026-09-29): repository without required reviews, so `reviewDecision`
    /// is `""`; merged, so `mergeStateStatus` is `UNKNOWN`.
    fn full_answer(rollup: serde_json::Value) -> String {
        serde_json::json!({
            "url": "https://github.com/o/r/pull/381",
            "statusCheckRollup": rollup,
            "headRefName": "fix/x",
            "title": "t",
            "body": "",
            "closingIssuesReferences": [],
            "state": "OPEN",
            "headRefOid": "621183B14FE358FC587E2D7B8401F3830244506F",
            "reviewDecision": "",
            "mergeStateStatus": "unknown",
            "isDraft": false
        })
        .to_string()
    }

    fn run(name: &str, conclusion: &str) -> serde_json::Value {
        serde_json::json!({
            "__typename": "CheckRun", "name": name, "status": "COMPLETED",
            "conclusion": conclusion,
            "detailsUrl": format!("https://github.com/o/r/actions/runs/1/job/{name}")
        })
    }

    #[test]
    fn script_reads_evidence_fields_and_the_worktree_state() {
        let script = build_pr_probe_script(&[("dev".into(), "/w/x".into())]);
        for f in [
            "headRefOid",
            "reviewDecision",
            "mergeStateStatus",
            "isDraft",
        ] {
            assert!(PR_FIELDS.contains(f), "{f} is asked for");
            assert!(
                !PR_FIELDS_BASIC.contains(f),
                "the old-gh fallback stays basic"
            );
        }
        assert!(script.contains("git rev-parse HEAD"));
        assert!(script.contains("git rev-list --count '@{u}..HEAD'"));
        assert!(script.contains("git diff --quiet HEAD --"));
        assert!(
            script.contains("*) dirty='';;"),
            "a diff that cannot tell is not clean"
        );
        assert!(script.contains("__FLEET_GIT__\\t%s"));
        assert!(
            !PR_FIELDS.contains(char::is_whitespace),
            "the line continuation leaves no space inside the field list"
        );
    }

    /// Redesign 6.4: what the Pull requests view lists rides the evidence.
    #[test]
    fn evidence_carries_the_title_head_and_merge_time() {
        let mut v: serde_json::Value =
            serde_json::from_str(&full_answer(serde_json::json!([]))).unwrap();
        v["state"] = "MERGED".into();
        v["title"] = "x".repeat(300).into();
        v["mergedAt"] = "2026-10-08T12:00:00Z".into();
        let ev = pr_info_from_json(&v.to_string()).evidence.unwrap();
        assert_eq!(ev.state.as_deref(), Some("MERGED"));
        assert_eq!(ev.head_ref.as_deref(), Some("fix/x"));
        assert_eq!(ev.merged_at, Some(1_791_460_800));
        assert_eq!(ev.title.map(|t| t.chars().count()), Some(PR_TITLE_MAX));
        // An open PR: gh answers `mergedAt` as null.
        v["mergedAt"] = serde_json::Value::Null;
        let ev = pr_info_from_json(&v.to_string()).evidence.unwrap();
        assert_eq!(ev.merged_at, None);
        assert!(PR_FIELDS.contains("mergedAt"));
    }

    /// Gap plan G3.10: the diffstat rides the evidence; a gh that leaves it
    /// out (or a negative number) reads as unknown, not as zero.
    #[test]
    fn evidence_carries_the_diffstat() {
        let mut v: serde_json::Value =
            serde_json::from_str(&full_answer(serde_json::json!([]))).unwrap();
        v["additions"] = 18.into();
        v["deletions"] = 6.into();
        let ev = pr_info_from_json(&v.to_string()).evidence.unwrap();
        assert_eq!((ev.additions, ev.deletions), (Some(18), Some(6)));
        v["additions"] = (-1).into();
        v.as_object_mut().unwrap().remove("deletions");
        let ev = pr_info_from_json(&v.to_string()).evidence.unwrap();
        assert_eq!((ev.additions, ev.deletions), (None, None));
        assert!(PR_FIELDS.contains("additions,deletions"));
    }

    #[test]
    fn evidence_reads_the_full_answer_and_treats_empty_as_absent() {
        let info = pr_info_from_json(&full_answer(serde_json::json!([
            run("rust", "SUCCESS"),
            run("docs", "SKIPPED")
        ])));
        let ev = info.evidence.expect("full answer has evidence");
        assert_eq!(
            ev.head_oid.as_deref(),
            Some("621183b14fe358fc587e2d7b8401f3830244506f")
        );
        assert_eq!(ev.review_decision, None, "\"\" = no review requirement");
        assert_eq!(ev.merge_state.as_deref(), Some("UNKNOWN"));
        assert_eq!(ev.state.as_deref(), Some("OPEN"));
        assert!(!ev.draft);
        assert_eq!(ev.checks.total, 2);
        assert_eq!(ev.checks.skipped, 1);
        assert!(ev.checks.failing.is_empty());
        assert_eq!(
            ev.local_head, None,
            "worktree fields come from the git line"
        );
        assert_eq!(
            info.ci_status.as_deref(),
            Some("passing"),
            "the badge is unchanged"
        );
    }

    #[test]
    fn evidence_is_absent_from_a_basic_answer() {
        let info = pr_info_from_json(
            "{\"url\":\"https://github.com/o/r/pull/1\",\"statusCheckRollup\":[]}",
        );
        assert!(info.pr_url.is_some());
        assert_eq!(info.evidence, None);
    }

    #[test]
    fn summary_names_failing_checks_from_both_shapes_capped() {
        let mut rollup: Vec<serde_json::Value> = (0..7)
            .map(|i| run(&format!("job-{i}"), "FAILURE"))
            .collect();
        rollup.push(serde_json::json!({
            "__typename": "StatusContext", "context": "ci/legacy", "state": "ERROR",
            "targetUrl": "https://ci.example/1"
        }));
        rollup.push(serde_json::json!({ "status": "IN_PROGRESS", "name": "slow" }));
        rollup.push(serde_json::json!({
            "name": "evil", "conclusion": "FAILURE", "detailsUrl": "javascript:alert(1)"
        }));
        let s = summarize_checks(&rollup);
        assert_eq!(s.total, 10);
        assert_eq!(s.pending, 1);
        assert_eq!(s.failing_total, 9);
        assert_eq!(s.failing.len(), FAILING_CHECKS_MAX);
        assert_eq!(s.failing[0].name, "job-0");
        assert_eq!(
            s.failing[0].url.as_deref(),
            Some("https://github.com/o/r/actions/runs/1/job/job-0")
        );
        let legacy = summarize_checks(&rollup[7..8]);
        assert_eq!(legacy.failing[0].name, "ci/legacy");
        assert_eq!(
            legacy.failing[0].url.as_deref(),
            Some("https://ci.example/1")
        );
        let evil = summarize_checks(&rollup[9..10]);
        assert_eq!(evil.failing[0].url, None, "only https links are kept");
    }

    /// The card and the badge read one classifier: over every rollup shape
    /// the tests know, the summary names a failing check exactly when the
    /// badge says `failing`, and counts a pending one exactly when the badge
    /// would otherwise say `pending`.
    #[test]
    fn summary_and_badge_agree_on_every_fixture() {
        let shapes = [
            check("COMPLETED", Some("SUCCESS")),
            check("COMPLETED", Some("FAILURE")),
            check("COMPLETED", Some("TIMED_OUT")),
            check("COMPLETED", Some("SKIPPED")),
            check("COMPLETED", Some("NEUTRAL")),
            check("IN_PROGRESS", None),
            check("QUEUED", None),
            check("WEIRD", None),
            serde_json::json!({ "state": "SUCCESS" }),
            serde_json::json!({ "state": "ERROR" }),
            serde_json::json!({ "state": "PENDING" }),
            serde_json::json!({}),
        ];
        let mut rollups: Vec<Vec<serde_json::Value>> = vec![vec![]];
        for a in &shapes {
            rollups.push(vec![a.clone()]);
            for b in &shapes {
                rollups.push(vec![a.clone(), b.clone()]);
            }
        }
        for r in rollups {
            let badge = reduce_ci_status(&r);
            let sum = summarize_checks(&r);
            assert_eq!(
                sum.failing_total > 0,
                badge.as_deref() == Some("failing"),
                "{r:?}"
            );
            if badge.as_deref() != Some("failing") {
                assert_eq!(
                    sum.pending > 0,
                    badge.as_deref() == Some("pending"),
                    "{r:?}"
                );
            }
            assert_eq!(sum.total == 0, badge.is_none(), "{r:?}");
        }
    }

    #[test]
    fn git_line_rides_only_a_pr_with_evidence() {
        let full = full_answer(serde_json::json!([]));
        let stdout = format!(
            "__FLEET_PR__\tclean\t0\t{full}\n\
             __FLEET_GIT__\tclean\t621183b14fe358fc587e2d7b8401f3830244506f\t0\t0\n\
             __FLEET_PR__\tahead\t0\t{full}\n\
             __FLEET_GIT__\tahead\tABCDEF1234567\t2\t1\n\
             __FLEET_PR__\tnoup\t0\t{full}\n\
             __FLEET_GIT__\tnoup\tabcdef1\t\t\n\
             __FLEET_PR__\tgarbled\t0\t{full}\n\
             __FLEET_GIT__\tgarbled\tnot-a-sha\tmany\tmaybe\n\
             __FLEET_PR__\tbasic\t0\t{{\"url\":\"https://github.com/o/r/pull/2\",\"statusCheckRollup\":[]}}\n\
             __FLEET_GIT__\tbasic\tabcdef1\t0\t0\n\
             __FLEET_PR__\tnopr\t1\tno pull requests found for branch \"x\"\n\
             __FLEET_GIT__\tnopr\tabcdef1\t0\t1\n\
             __FLEET_GIT__\tstray\tabcdef1\t0\t0\n"
        );
        let ProbeOutput::Results(map) = parse_pr_probe_output(&stdout) else {
            panic!("results");
        };
        let ev = |n: &str| map[n].evidence.clone();
        let clean = ev("clean").unwrap();
        assert_eq!(clean.local_head, clean.head_oid);
        assert_eq!((clean.ahead, clean.dirty), (Some(0), Some(false)));
        let ahead = ev("ahead").unwrap();
        assert_eq!(ahead.local_head.as_deref(), Some("abcdef1234567"));
        assert_eq!((ahead.ahead, ahead.dirty), (Some(2), Some(true)));
        let noup = ev("noup").unwrap();
        assert_eq!(noup.local_head.as_deref(), Some("abcdef1"));
        assert_eq!(
            (noup.ahead, noup.dirty),
            (None, None),
            "no upstream is not 0"
        );
        let garbled = ev("garbled").unwrap();
        assert_eq!(
            (garbled.local_head, garbled.ahead, garbled.dirty),
            (None, None, None)
        );
        assert_eq!(
            ev("basic"),
            None,
            "no evidence fields, nothing to attach to"
        );
        assert_eq!(map["nopr"], PrInfo::default(), "no PR, nothing to assess");
        assert!(!map.contains_key("stray"));
    }

    #[test]
    fn evidence_round_trips_as_stored_json() {
        let mut ev = pr_info_from_json(&full_answer(serde_json::json!([run("rust", "FAILURE")])))
            .evidence
            .unwrap();
        ev.local_head = Some("abcdef1".into());
        ev.ahead = Some(1);
        ev.dirty = Some(false);
        let json = serde_json::to_string(&ev).unwrap();
        assert_eq!(serde_json::from_str::<PrEvidence>(&json).unwrap(), ev);
        // An older reader's (or a hand-edited) empty object still parses.
        assert_eq!(
            serde_json::from_str::<PrEvidence>("{}").unwrap(),
            PrEvidence::default()
        );
    }
}
