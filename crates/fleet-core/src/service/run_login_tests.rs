use super::*;
use crate::service::account_limits::seed_usage;
use crate::store::SignedOutLogin;

const NOW: i64 = 1_000;

/// mac: its own login on `acct-own` (20% used), profile `work` on
/// `acct-work` (10%); one project.
fn fleet() -> (Store, i64) {
    let s = Store::open_in_memory().unwrap();
    seed_usage(&s, "mac", None, "acct-own", 20.0, NOW);
    seed_usage(&s, "mac", Some("work"), "acct-work", 10.0, NOW);
    let p = s.upsert_project("acme", "api", "/srv/acme/api").unwrap();
    (s, p)
}

fn sign_out(s: &Store, profile: Option<&str>) {
    s.set_host_signed_out_logins(
        "mac",
        &[SignedOutLogin {
            profile: profile.map(str::to_string),
            since: 1,
            checked_at: 1,
        }],
    )
    .unwrap();
}

#[test]
fn a_login_is_stopped_when_forbidden_or_signed_out() {
    let (s, p) = fleet();
    assert!(stop(&s, Some(p), "mac", None, NOW).unwrap().is_none());
    s.set_account_forbidden_projects("acct-own", &[p]).unwrap();
    let e = refuse(&s, Some(p), "mac", None, NOW).unwrap_err();
    assert_eq!(e.code, codes::E_ACCOUNT_FORBIDDEN);
    // `""` is the host's own login too.
    assert!(refuse(&s, Some(p), "mac", Some(" "), NOW).is_err());
    // Another login, no project: allowed.
    assert!(refuse(&s, Some(p), "mac", Some("work"), NOW).is_ok());
    assert!(refuse(&s, None, "mac", None, NOW).is_ok());

    sign_out(&s, Some("work"));
    let e = refuse(&s, Some(p), "mac", Some("work"), NOW).unwrap_err();
    assert_eq!(e.code, codes::E_CLAUDE_CLI);
    assert!(e.message.contains("signed out"), "{}", e.message);
    assert!(e.message.contains("claude /login"), "{}", e.message);
}

#[test]
fn choose_steps_off_a_forbidden_login_and_refuses_when_none_is_left() {
    let (s, p) = fleet();
    assert_eq!(choose(&s, Some(p), "mac", None, NOW).unwrap(), None);
    s.set_account_forbidden_projects("acct-own", &[p]).unwrap();
    assert_eq!(
        choose(&s, Some(p), "mac", None, NOW).unwrap().as_deref(),
        Some("work")
    );
    // The login a caller asked for is kept when it is allowed.
    assert_eq!(
        choose(&s, Some(p), "mac", Some("work"), NOW)
            .unwrap()
            .as_deref(),
        Some("work")
    );
    s.set_account_forbidden_projects("acct-work", &[p]).unwrap();
    assert_eq!(
        choose(&s, Some(p), "mac", None, NOW).unwrap_err().code,
        codes::E_ACCOUNT_FORBIDDEN
    );
    // Signed out, and nothing else to take: refused.
    let (s, p) = fleet();
    sign_out(&s, None);
    assert_eq!(
        choose(&s, Some(p), "mac", None, NOW).unwrap_err().code,
        codes::E_CLAUDE_CLI
    );
}

#[test]
fn an_unattended_run_also_waits_past_the_pause_line() {
    let (s, p) = fleet();
    assert_eq!(
        unattended_hold(&s, Some(p), "mac", None, NOW).unwrap(),
        None
    );
    seed_usage(&s, "mac", None, "acct-own", 97.0, NOW);
    let h = unattended_hold(&s, Some(p), "mac", None, NOW)
        .unwrap()
        .expect("held");
    assert!(h.reason.contains("pause line"), "{}", h.reason);
    // A login a run is not refused on is still not stopped outright.
    assert!(stop(&s, Some(p), "mac", None, NOW).unwrap().is_none());
    s.set_account_forbidden_projects("acct-own", &[p]).unwrap();
    let h = unattended_hold(&s, Some(p), "mac", None, NOW)
        .unwrap()
        .expect("held");
    assert!(h.reason.contains("forbidden"), "{}", h.reason);
    assert_eq!(h.until, None, "a forbid waits on a person");
}

/// One Claude launch site: a function that starts Claude (a pane, a
/// background agent, a `claude -p` run) and the account-rule guard it must
/// call (`service::run_login`, or the guards `sessions::new_session` calls
/// itself).
struct Site {
    file: &'static str,
    signature: &'static str,
    guards: &'static [&'static str],
}

/// The text of the item that starts at `signature` in `src`, through its
/// closing brace.
fn body<'a>(src: &'a str, file: &str, signature: &str) -> &'a str {
    let start = src
        .find(signature)
        .unwrap_or_else(|| panic!("{file}: no `{signature}`"));
    let open = start + src[start..].find('{').expect("a body");
    let mut depth = 0usize;
    for (i, c) in src[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &src[start..open + i + 1];
                }
            }
            _ => {}
        }
    }
    panic!("{file}: `{signature}` never closes")
}

/// The source files under `crates/fleet-core/src` that build a Claude
/// launch (a headless job, an agent's print run, the planner's script, a
/// background agent, a session pane), test files aside.
const MARKERS: &[&str] = &[
    "HeadlessJob {",
    "PrintRun {",
    "planner_script_as(",
    "bg_script_with(",
    ".launch_command(",
    ".start_command(",
];

/// Files that build a launch and how each is held to the account rules:
/// either a guarded site in [`SITES`], or a reason it needs none.
const EXEMPT: &[(&str, &str)] = &[
    (
        "claude_cli.rs",
        "builds `claude --bg`; its one caller, bg_sessions::new_bg_session_tracked, refuses first",
    ),
    (
        "service/headless.rs",
        "the headless job's definition; each run's caller picks and checks its login",
    ),
    (
        "service/agents/vehicle.rs",
        "PrintRun's definition and `login_hold`; the runners call `login_hold_in` before a run",
    ),
    (
        "service/work/orchestrate/planner.rs",
        "the planner's script; run_planner takes its login from limit::choose_in",
    ),
    (
        "service/context_help/mod.rs",
        "runs under the session's own stored login: the account its work already runs on",
    ),
    (
        "service/drafts.rs",
        "runs under the session's own stored login: the account its work already runs on",
    ),
    (
        "service/watch_summary.rs",
        "runs under the session's own stored login: the account its work already runs on",
    ),
    (
        "service/work/summary.rs",
        "a fork of a past session's own conversation, on its own host",
    ),
    (
        "service/decide/haiku.rs",
        "the decision bench's baseline: a redacted closed-set question, no project's work",
    ),
];

/// Every launch site of Claude work and the guard it calls. A new launch
/// path either joins this list, calling `run_login` (or the start's own
/// guards), or [`EXEMPT`] with a reason.
const SITES: &[Site] = &[
    Site {
        file: "service/sessions/lifecycle.rs",
        signature: "pub async fn new_session(",
        guards: &["refuse_forbidden(", "refuse_over_daily_limit("],
    },
    Site {
        file: "service/sessions/review.rs",
        signature: "pub async fn spawn_review(",
        guards: &["run_login::choose("],
    },
    Site {
        file: "service/bg_sessions.rs",
        signature: "pub async fn new_bg_session_tracked(",
        guards: &["run_login::refuse("],
    },
    Site {
        file: "service/work/orchestrate/checks.rs",
        signature: "fn login_for(",
        guards: &["run_login::choose("],
    },
    Site {
        file: "service/work/orchestrate/checks.rs",
        signature: "pub fn plan(",
        guards: &["login_for("],
    },
    Site {
        file: "service/work/ticket_check.rs",
        signature: "async fn start_checked(",
        guards: &["run_login::choose("],
    },
    Site {
        file: "service/work/brief_draft.rs",
        signature: "pub async fn draft_into(",
        guards: &["run_login::choose("],
    },
    Site {
        file: "service/work/title_draft.rs",
        signature: "pub async fn draft_title(",
        // No project yet, so no account is forbidden: the host's own login.
        guards: &["draft_script(&model, &title_prompt(notes), None)"],
    },
    Site {
        file: "service/work/orchestrate/limit.rs",
        signature: "fn logins(",
        guards: &["mission_forbidden(", "is_signed_out("],
    },
    Site {
        file: "service/work/orchestrate/mod.rs",
        signature: "pub async fn run_planner(",
        guards: &["limit::choose_in("],
    },
    Site {
        file: "service/work/orchestrate/drafts.rs",
        signature: "pub async fn release_note(",
        guards: &["limit::draft_login("],
    },
    Site {
        file: "service/work/orchestrate/triage.rs",
        signature: "pub async fn triage_with(",
        guards: &["limit::draft_login("],
    },
    Site {
        file: "service/agents/review/mod.rs",
        signature: "    async fn review(&self, job: &AgentJobRow, rule: &AgentRuleRow)",
        guards: &["login_hold_in("],
    },
    Site {
        file: "service/agents/review/mod.rs",
        signature: "    async fn debate(&self, job: &AgentJobRow, rule: &AgentRuleRow)",
        guards: &["login_hold_in("],
    },
    Site {
        file: "service/agents/support/mod.rs",
        signature: "    async fn investigate(&self, job: &AgentJobRow, rule: &AgentRuleRow)",
        guards: &["login_hold_in("],
    },
    Site {
        file: "service/fleet_agents/runner.rs",
        signature: "async fn run_in_workspace(",
        guards: &["login_hold_in("],
    },
];

/// Accounts review: every place that starts Claude holds to the account
/// rules (a forbidden account, a spent daily limit, a signed-out login), or
/// says why it needs none. A new launch path fails here until it is listed.
#[test]
fn every_claude_launch_path_holds_to_the_account_rules() {
    let read = |file: &str| crate::repo_files::read(&format!("crates/fleet-core/src/{file}"));
    for site in SITES {
        let src = read(site.file);
        let text = body(&src, site.file, site.signature);
        for g in site.guards {
            assert!(
                text.contains(g),
                "{} `{}` no longer calls `{g}`: every Claude launch is held to the \
                 account rules (service::run_login)",
                site.file,
                site.signature
            );
        }
    }
    // Every file that builds a launch is a site or exempt.
    let root = crate::repo_files::path("crates/fleet-core/src");
    let mut stack = vec![root.clone()];
    let mut unlisted = Vec::new();
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                if path.file_name().is_some_and(|n| n == "tests") {
                    continue;
                }
                stack.push(path);
                continue;
            }
            let rel = path
                .strip_prefix(&root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            if !rel.ends_with(".rs")
                || name == "tests.rs"
                || name.ends_with("_tests.rs")
                || name == "testing.rs"
                || name == "testkit.rs"
            {
                continue;
            }
            let src = std::fs::read_to_string(&path).unwrap();
            // Only the code before a file's own `mod tests`.
            let code = src.split("#[cfg(test)]\nmod tests").next().unwrap_or("");
            if !MARKERS.iter().any(|m| code.contains(m)) {
                continue;
            }
            let listed =
                SITES.iter().any(|s| s.file == rel) || EXEMPT.iter().any(|(f, _)| *f == rel);
            if !listed {
                unlisted.push(rel);
            }
        }
    }
    unlisted.sort();
    assert!(
        unlisted.is_empty(),
        "these files start Claude and are neither a guarded site nor exempt in \
         run_login_tests: {unlisted:?}"
    );
    for (file, _) in EXEMPT {
        assert!(
            crate::repo_files::path(&format!("crates/fleet-core/src/{file}")).exists(),
            "{file} is gone: drop it from EXEMPT"
        );
    }
}
