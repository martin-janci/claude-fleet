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

/// One session's probe result. `pr_url == None` means "no open PR for this
/// branch" (gh exited non-zero) — the caller clears a stale link.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PrInfo {
    pub pr_url: Option<String>,
    /// `passing` | `failing` | `pending`, or `None` when the PR has no checks.
    pub ci_status: Option<String>,
}

/// Build the per-host probe script. `targets` is `(tmux_name, cwd)` for every
/// session due this pass. Each value is shell-quoted; the output is one
/// `__FLEET_PR__\t<name>\t<json-or-empty>` line per target, JSON kept on one
/// line so a multi-line rollup can't split a record.
pub fn build_pr_probe_script(targets: &[(String, String)]) -> String {
    let mut script = String::new();
    script.push_str(&format!(
        "command -v gh >/dev/null 2>&1 || {{ echo {NO_GH_MARKER}; exit 0; }}\n"
    ));
    for (name, cwd) in targets {
        // printf (builtin and /usr/bin) expands `\t` in the FORMAT string on
        // every platform; sed's replacement `\t` does not on BSD, so the
        // prefix goes through printf.
        script.push_str(&format!(
            "printf '{prefix}%s\\t%s\\n' {name} \"$(cd {cwd} 2>/dev/null && \
             gh pr view --json url,statusCheckRollup 2>/dev/null | tr -d '\\n')\"\n",
            name = quote(name),
            cwd = quote(cwd),
            prefix = RESULT_PREFIX.replace('\t', "\\t"),
        ));
    }
    script
}

/// Outcome of parsing one probe run.
#[derive(Debug, PartialEq, Eq)]
pub enum ProbeOutput {
    /// `gh` is not installed on the host — nothing was probed.
    NoGh,
    /// Per-session results, keyed by tmux_name. Every target the script ran
    /// for is present (a session with no PR maps to `PrInfo::default()`).
    Results(HashMap<String, PrInfo>),
}

/// Parse the probe script's stdout. Tolerant: unknown lines are skipped and a
/// malformed JSON blob counts as "no PR" rather than aborting the pass.
pub fn parse_pr_probe_output(stdout: &str) -> ProbeOutput {
    let mut out = HashMap::new();
    for line in stdout.lines() {
        if line.trim() == NO_GH_MARKER {
            return ProbeOutput::NoGh;
        }
        // The prefix carries a literal tab; the script's sed inserted it as
        // an escaped `\t` which GNU/BSD sed both render as a tab in the
        // replacement, but be lenient and accept either spelling.
        let rest = match line
            .strip_prefix(RESULT_PREFIX)
            .or_else(|| line.strip_prefix("__FLEET_PR__\\t"))
        {
            Some(r) => r,
            None => continue,
        };
        let (name, json) = match rest.split_once('\t') {
            Some(parts) => parts,
            None => (rest, ""),
        };
        out.insert(name.to_string(), pr_info_from_json(json));
    }
    ProbeOutput::Results(out)
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
    PrInfo { pr_url, ci_status }
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
    static CACHE: once_cell::sync::Lazy<Arc<PrProbeCache>> =
        once_cell::sync::Lazy::new(|| Arc::new(PrProbeCache::new(PR_PROBE_TTL)));
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
        assert!(script.contains("'dev-x'"));
        // The single quote in the path is escaped by `quote`.
        assert!(script.contains("it'\\''s"));
        assert!(script.contains("gh pr view --json url,statusCheckRollup"));
    }

    #[test]
    fn parse_detects_missing_gh() {
        assert_eq!(
            parse_pr_probe_output("__FLEET_NO_GH__\n"),
            ProbeOutput::NoGh
        );
    }

    #[test]
    fn parse_maps_each_target_and_treats_empty_as_no_pr() {
        let stdout = "__FLEET_PR__\tdev-a\t{\"url\":\"https://github.com/o/r/pull/7\",\"statusCheckRollup\":[]}\n\
                      __FLEET_PR__\tdev-b\t\n\
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
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn parse_accepts_escaped_tab_prefix_from_a_literal_sed() {
        let stdout = "__FLEET_PR__\\tdev-a\t{\"url\":\"https://x/pull/1\"}\n";
        let ProbeOutput::Results(map) = parse_pr_probe_output(stdout) else {
            panic!("expected results");
        };
        assert_eq!(
            map.get("dev-a").unwrap().pr_url.as_deref(),
            Some("https://x/pull/1")
        );
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
