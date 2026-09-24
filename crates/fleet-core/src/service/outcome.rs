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
/// The `gh pr view` fields fleet reads. The last four are work detection's
/// (M4.2): read, parsed and dropped — the body is capped at 4k by `--jq`
/// and never stored.
const PR_FIELDS: &str = "url,statusCheckRollup,headRefName,title,body,closingIssuesReferences";
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
}

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
        // their trailers: no extra round trip, at most 200 lines / 8k.
        script.push_str(&format!(
            "if cd {cwd} 2>/dev/null; then \
             out=\"$(gh pr view --json {fields} --jq '.body |= ((. // \"\")[0:4000])' 2>&1)\"; rc=$?; \
             if [ \"$rc\" -ne 0 ] && ! printf '%s' \"$out\" | grep -qi 'no pull requests found'; then \
             out=\"$(gh pr view --json {basic} 2>&1)\"; rc=$?; fi; \
             msgs=\"$(git log --format=%B '@{{u}}..HEAD' 2>/dev/null | head -n 200 | head -c 8000 | tr '\\n\\t' '\\037 ')\"; \
             else out=''; rc={rc_no_cwd}; msgs=''; fi; \
             printf '{prefix}%s\\t%s\\t%s\\n' {name} \"$rc\" \"$(printf '%s' \"$out\" | tr -d '\\n\\t')\"; \
             printf '{tprefix}%s\\t%s\\n' {name} \"$msgs\"\n",
            name = quote(name),
            cwd = quote(cwd),
            fields = PR_FIELDS,
            basic = PR_FIELDS_BASIC,
            rc_no_cwd = RC_NO_CWD,
            prefix = RESULT_PREFIX.replace('\t', "\\t"),
            tprefix = TRAILERS_PREFIX.replace('\t', "\\t"),
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
    for line in stdout.lines() {
        match line.trim() {
            NO_GH_MARKER => return ProbeOutput::NoGh,
            NO_AUTH_MARKER => return ProbeOutput::NoAuth,
            _ => {}
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
    ProbeOutput::Results(out)
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
    PrInfo {
        pr_url,
        ci_status,
        signals,
    }
}

/// Collapse a PR's check rollup to one badge: any failure wins, then any
/// still-running check, otherwise passing. `None` when there are no checks.
///
/// GitHub's rollup mixes two shapes: check runs (`status` +
/// `conclusion`) and commit statuses (`state`). Both are handled.
pub fn reduce_ci_status(checks: &[serde_json::Value]) -> Option<String> {
    if checks.is_empty() {
        return None;
    }
    let mut pending = false;
    for c in checks {
        let conclusion = c
            .get("conclusion")
            .and_then(|v| v.as_str())
            .map(|s| s.to_ascii_uppercase());
        let status = c
            .get("status")
            .and_then(|v| v.as_str())
            .map(|s| s.to_ascii_uppercase());
        let state = c
            .get("state")
            .and_then(|v| v.as_str())
            .map(|s| s.to_ascii_uppercase());
        match (conclusion.as_deref(), status.as_deref(), state.as_deref()) {
            (
                Some("FAILURE" | "TIMED_OUT" | "CANCELLED" | "ACTION_REQUIRED" | "STARTUP_FAILURE"),
                _,
                _,
            )
            | (_, _, Some("FAILURE" | "ERROR")) => return Some("failing".into()),
            (_, Some("QUEUED" | "IN_PROGRESS" | "PENDING" | "WAITING" | "REQUESTED"), _)
            | (_, _, Some("PENDING" | "EXPECTED")) => pending = true,
            // Check run still without a conclusion is in flight.
            (None, Some(_), None) => pending = true,
            _ => {}
        }
    }
    Some(if pending { "pending" } else { "passing" }.into())
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
}
