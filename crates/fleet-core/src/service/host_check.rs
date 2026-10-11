//! The host detail's health checklist (Orbit Fleet redesign step 4.7): one
//! on-demand read of a host that answers what the reconcile probe does not —
//! which agents are on its `PATH`, and whether fleet's hooks and the worker
//! guard are installed in its `~/.claude/settings.json`. SSH and tmux come
//! back from the same round trip; the agent version and skills drift are
//! already on the host row and the asset inventory, so the checklist reads
//! them there.
//!
//! The settings file is parsed here and reduced to two booleans: it carries
//! the hook bearer token, and nothing of it leaves this function.
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::ipc_error::IpcError;
use crate::service::hooks_install::{is_fleet_owned_hook, HOOK_HEADERS_FILE};
use crate::ssh::SshClient;

/// The agent binaries the checklist looks for, in the order it lists them:
/// Claude Code, Codex, Agy, Gemini CLI. Only names fleet knows how to run
/// (or will) are asked about; the answer is the subset found.
pub const AGENT_BINARIES: [&str; 4] = ["claude", "codex", "agy", "gemini"];

/// Opens the settings section of [`check_script`]'s output.
const SETTINGS_MARK: &str = "---FLEET:settings";

/// How long one check may take: one SSH round trip and a small file.
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(20);

/// What [`check_host`] found. Every field but `alias` and `checked_at` is
/// `None` when the host could not be asked (`error` then says why).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HostCheck {
    pub alias: String,
    pub checked_at: i64,
    /// Why the host could not be asked: an SSH failure, a timeout, a local
    /// shell that would not start. `None`: it answered.
    pub error: Option<String>,
    /// `tmux -V` without the prefix; `None` when tmux is not installed.
    pub tmux_version: Option<String>,
    /// Which of [`AGENT_BINARIES`] are on the login shell's `PATH`.
    pub agents_on_path: Option<Vec<String>>,
    /// `true`: fleet's reporting hooks are in `~/.claude/settings.json`.
    /// `None`: no readable settings file.
    pub fleet_hooks: Option<bool>,
    /// `true`: the worker guard (`PreToolUse(Bash)`) is installed.
    /// `None`: no readable settings file.
    pub guard_hook: Option<bool>,
    /// The projects base path asked about ([`CheckHostArgs::base_path`],
    /// M15 G7.12: Settings › Projects checks a base path on its host before
    /// it is saved). Absent when none was asked about or the answer was cut.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_path: Option<BasePathCheck>,
}

/// `check_host`'s arguments: the host, and optionally a projects base path
/// to check there.
#[derive(Debug, Clone, Deserialize)]
pub struct CheckHostArgs {
    pub alias: String,
    /// A projects base path (absolute, or `~/…`) to check on the host:
    /// whether it exists, and whether the login user may write there.
    #[serde(default)]
    pub base_path: Option<String>,
}

/// What the host said about a projects base path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BasePathCheck {
    /// The path as asked.
    pub path: String,
    /// `ok` (a directory the user may write), `creatable` (missing, and its
    /// nearest existing parent is writable), `unwritable` (the directory or
    /// that parent is not writable), `not_dir` (something else is there).
    pub state: String,
    /// The login user on the host, for "not writable by user dev".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
}

/// The base path states [`BasePathCheck::state`] takes.
pub const BASE_PATH_STATES: [&str; 4] = ["ok", "creatable", "unwritable", "not_dir"];

/// Refuse a base path that is not absolute, `~` or `~/…`, or carries a control
/// character; the shell only ever sees it quoted.
pub fn base_path_problem(path: &str) -> Option<&'static str> {
    if !(path.starts_with('/') || path == "~" || path.starts_with("~/")) {
        return Some("a base path must be absolute or start with ~/");
    }
    if path.chars().any(|c| c.is_control()) {
        return Some("a base path cannot hold a control character");
    }
    None
}

/// The shell that checks `path` (already passed [`base_path_problem`]).
/// `~/` becomes `$HOME/`; the rest is quoted with [`crate::shell::quote`].
fn base_path_script(path: &str) -> String {
    let target = match path.strip_prefix("~/") {
        Some(rest) => format!("\"$HOME\"/{}", crate::shell::quote(rest)),
        None if path == "~" => "\"$HOME\"".to_string(),
        None => crate::shell::quote(path),
    };
    format!(
        "p={target}; d=\"$p\"; \
         while [ ! -e \"$d\" ] && [ \"$d\" != / ]; do d=$(dirname \"$d\"); done; \
         printf 'baseuser=%s\\n' \"$(id -un 2>/dev/null)\"; \
         if [ -d \"$p\" ]; then if [ -w \"$p\" ]; then echo basew=ok; else echo basew=unwritable; fi; \
         elif [ -e \"$p\" ]; then echo basew=not_dir; \
         elif [ -d \"$d\" ] && [ -w \"$d\" ]; then echo basew=creatable; else echo basew=unwritable; fi; "
    )
}

/// The one script a check runs: tmux, each agent found on `PATH`, then the
/// settings file behind [`SETTINGS_MARK`]. Nothing is interpolated.
pub fn check_script() -> String {
    check_script_with(None)
}

/// [`check_script`], checking a projects base path first when one is given.
pub fn check_script_with(base_path: Option<&str>) -> String {
    format!(
        "{}printf 'tmuxv=%s\\n' \"$(tmux -V 2>/dev/null)\"; \
         for a in {}; do command -v \"$a\" >/dev/null 2>&1 && printf 'agent=%s\\n' \"$a\"; done; \
         printf '%s\\n' '{SETTINGS_MARK}'; cat \"$HOME/.claude/settings.json\" 2>/dev/null; true",
        base_path.map(base_path_script).unwrap_or_default(),
        AGENT_BINARIES.join(" ")
    )
}

/// Parse [`check_script`]'s output into everything but the alias, stamp and
/// error. Output without the settings mark is a cut answer: the agents and
/// hooks are then unknown rather than "none".
pub fn parse_check(stdout: &str) -> HostCheck {
    let (head, settings) = match stdout.split_once(&format!("{SETTINGS_MARK}\n")) {
        Some((h, s)) => (h, Some(s)),
        None => (stdout, None),
    };
    let mut c = HostCheck::default();
    let mut agents = Vec::new();
    let mut base_user = None;
    for line in head.lines() {
        if let Some(v) = line.strip_prefix("baseuser=") {
            base_user = Some(v.trim().to_string()).filter(|u| !u.is_empty());
        } else if let Some(v) = line.strip_prefix("basew=") {
            let v = v.trim();
            if BASE_PATH_STATES.contains(&v) {
                c.base_path = Some(BasePathCheck {
                    path: String::new(),
                    state: v.to_string(),
                    user: None,
                });
            }
        } else if let Some(v) = line.strip_prefix("tmuxv=") {
            c.tmux_version = crate::service::hosts::parse_tmux_version(v.trim());
        } else if let Some(a) = line.strip_prefix("agent=") {
            let a = a.trim();
            if AGENT_BINARIES.contains(&a) && !agents.iter().any(|x: &String| x == a) {
                agents.push(a.to_string());
            }
        }
    }
    if let Some(b) = c.base_path.as_mut() {
        b.user = base_user;
    }
    if let Some(settings) = settings {
        c.agents_on_path = Some(agents);
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(settings.trim()) {
            c.fleet_hooks = Some(fleet_hooks_installed(&v));
            c.guard_hook = Some(guard_hook_installed(&v));
        }
    }
    c
}

/// Every hook entry under `hooks.<event>[].hooks[]`, with its event and
/// matcher.
fn hook_entries(settings: &serde_json::Value) -> Vec<(&str, &str, &serde_json::Value)> {
    let mut out = Vec::new();
    let Some(events) = settings.get("hooks").and_then(|h| h.as_object()) else {
        return out;
    };
    for (event, groups) in events {
        for group in groups.as_array().into_iter().flatten() {
            let matcher = group.get("matcher").and_then(|m| m.as_str()).unwrap_or("");
            for h in group
                .get("hooks")
                .and_then(|h| h.as_array())
                .into_iter()
                .flatten()
            {
                out.push((event.as_str(), matcher, h));
            }
        }
    }
    out
}

/// Fleet's `Stop` hook, the completion signal every other state hangs on,
/// in any shape fleet has installed.
pub fn fleet_hooks_installed(settings: &serde_json::Value) -> bool {
    hook_entries(settings)
        .into_iter()
        .any(|(event, _, h)| event == "Stop" && is_fleet_owned_hook(h))
}

/// The worker guard: a `PreToolUse(Bash)` command hook that reads the hook
/// body and posts with fleet's headers file
/// ([`crate::service::hooks_install::guard_command`]).
pub fn guard_hook_installed(settings: &serde_json::Value) -> bool {
    hook_entries(settings)
        .into_iter()
        .any(|(event, matcher, h)| {
            event == "PreToolUse"
                && matcher == "Bash"
                && h.get("type").and_then(|t| t.as_str()) == Some("command")
                && h.get("command").and_then(|c| c.as_str()).is_some_and(|c| {
                    c.starts_with("input=$(cat);")
                        && c.contains(&format!(".claude/{HOOK_HEADERS_FILE}"))
                })
        })
}

/// Run the checklist's read on `alias` (`local` or over SSH).
pub async fn check_host(ssh: &Arc<SshClient>, alias: &str, now: i64) -> HostCheck {
    // No base path: nothing to refuse.
    check_host_with(ssh, alias, None, now)
        .await
        .unwrap_or_else(|e| finish(alias, now, Err(e)))
}

/// [`check_host`], also checking a projects base path there (M15 G7.12).
/// A base path [`base_path_problem`] refuses is `E_INVALID` before anything
/// runs.
pub async fn check_host_with(
    ssh: &Arc<SshClient>,
    alias: &str,
    base_path: Option<&str>,
    now: i64,
) -> Result<HostCheck, IpcError> {
    let base_path = base_path.map(str::trim).filter(|p| !p.is_empty());
    if let Some(problem) = base_path.and_then(base_path_problem) {
        return Err(IpcError::new(crate::ipc_error::codes::E_INVALID, problem));
    }
    let res = crate::service::catalog::inventory::run_host_script_with(
        ssh,
        alias,
        &check_script_with(base_path),
        CHECK_TIMEOUT,
        &tokio_util::sync::CancellationToken::new(),
    )
    .await;
    let mut c = finish(alias, now, res);
    if let (Some(b), Some(path)) = (c.base_path.as_mut(), base_path) {
        b.path = path.to_string();
    }
    Ok(c)
}

fn finish(alias: &str, now: i64, res: Result<String, IpcError>) -> HostCheck {
    match res {
        Ok(out) => HostCheck {
            alias: alias.to_string(),
            checked_at: now,
            ..parse_check(&out)
        },
        Err(e) => HostCheck {
            alias: alias.to_string(),
            checked_at: now,
            error: Some(e.message),
            ..HostCheck::default()
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::hooks_install::merge_hook_into_settings_json_with;

    fn installed() -> String {
        merge_hook_into_settings_json_with("", "https://hub.example/hook", "tok", false).unwrap()
    }

    #[test]
    fn a_full_answer_reads_tmux_agents_and_both_hooks() {
        let out = format!(
            "tmuxv=tmux 3.5a\nagent=claude\nagent=codex\nagent=bogus\n{SETTINGS_MARK}\n{}\n",
            installed()
        );
        let c = parse_check(&out);
        assert_eq!(c.tmux_version.as_deref(), Some("3.5a"));
        assert_eq!(
            c.agents_on_path,
            Some(vec!["claude".to_string(), "codex".to_string()]),
            "only names fleet asked about"
        );
        assert_eq!((c.fleet_hooks, c.guard_hook), (Some(true), Some(true)));
    }

    #[test]
    fn a_failing_hook_is_reported_not_guessed() {
        // The settings a person edited by hand: fleet's Stop hook is there,
        // the guard is gone.
        let mut v: serde_json::Value = serde_json::from_str(&installed()).unwrap();
        v["hooks"].as_object_mut().unwrap().remove("PreToolUse");
        let out = format!("tmuxv=tmux 3.4\n{SETTINGS_MARK}\n{v}\n");
        let c = parse_check(&out);
        assert_eq!((c.fleet_hooks, c.guard_hook), (Some(true), Some(false)));
        assert_eq!(c.agents_on_path, Some(vec![]));

        // No fleet hooks at all.
        let c = parse_check(&format!("{SETTINGS_MARK}\n{{\"hooks\":{{}}}}\n"));
        assert_eq!((c.fleet_hooks, c.guard_hook), (Some(false), Some(false)));
    }

    #[test]
    fn no_settings_file_or_a_cut_answer_reads_unknown() {
        let c = parse_check(&format!("tmuxv=\n{SETTINGS_MARK}\n"));
        assert_eq!(c.tmux_version, None, "no tmux");
        assert_eq!((c.fleet_hooks, c.guard_hook), (None, None));
        let c = parse_check("tmuxv=tmux 3.5a\nagent=claude\n");
        assert_eq!(c.agents_on_path, None, "cut before the mark");
    }

    #[test]
    fn an_unreachable_host_carries_the_reason_and_nothing_else() {
        let c = finish(
            "mercury",
            7,
            Err(IpcError::new(
                crate::ipc_error::codes::E_TIMEOUT,
                "timed out",
            )),
        );
        assert_eq!(c.error.as_deref(), Some("timed out"));
        assert_eq!((c.alias.as_str(), c.checked_at), ("mercury", 7));
        assert_eq!(c.agents_on_path, None);
    }

    #[test]
    fn a_base_path_must_be_absolute_or_home_relative() {
        assert!(base_path_problem("/srv/work").is_none());
        assert!(base_path_problem("~/projects").is_none());
        assert!(base_path_problem("~").is_none());
        assert!(base_path_problem("projects").is_some());
        assert!(base_path_problem("~projects").is_some());
        assert!(base_path_problem("/srv/a\nb").is_some());
    }

    #[test]
    fn the_base_path_answer_reads_its_state_and_user() {
        let c = parse_check(&format!(
            "baseuser=dev\nbasew=unwritable\ntmuxv=tmux 3.4\n{SETTINGS_MARK}\n"
        ));
        let b = c.base_path.expect("asked");
        assert_eq!(
            (b.state.as_str(), b.user.as_deref()),
            ("unwritable", Some("dev"))
        );
        assert_eq!(
            parse_check("basew=maybe\n").base_path,
            None,
            "only known states"
        );
        assert_eq!(parse_check("tmuxv=\n").base_path, None, "not asked");
    }

    #[cfg(unix)]
    #[test]
    fn the_script_checks_a_base_path_quoted_on_the_host() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let run = |path: &str| {
            let out = std::process::Command::new("sh")
                .args(["-c", &check_script_with(Some(path))])
                .env("HOME", home.path())
                .output()
                .unwrap();
            parse_check(&String::from_utf8_lossy(&out.stdout))
                .base_path
                .map(|b| b.state)
        };
        std::fs::create_dir(home.path().join("work")).unwrap();
        assert_eq!(run("~/work").as_deref(), Some("ok"));
        assert_eq!(run("~").as_deref(), Some("ok"));
        assert_eq!(run("~/new place/projects").as_deref(), Some("creatable"));
        std::fs::write(home.path().join("file"), "x").unwrap();
        assert_eq!(run("~/file").as_deref(), Some("not_dir"));
        // A quote or `$(…)` in the path stays a path.
        assert_eq!(run("~/it's $(touch pwned)").as_deref(), Some("creatable"));
        assert!(!home.path().join("pwned").exists());
        let ro = home.path().join("ro");
        std::fs::create_dir(&ro).unwrap();
        std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o555)).unwrap();
        // root writes anywhere, so only an ordinary user sees "unwritable".
        let root = std::process::Command::new("id").arg("-u").output().unwrap();
        if String::from_utf8_lossy(&root.stdout).trim() != "0" {
            assert_eq!(run("~/ro").as_deref(), Some("unwritable"));
            assert_eq!(run("~/ro/sub").as_deref(), Some("unwritable"));
        }
        std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn the_script_runs_and_never_prints_the_settings_it_reads() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir(home.path().join(".claude")).unwrap();
        std::fs::write(home.path().join(".claude/settings.json"), installed()).unwrap();
        let out = std::process::Command::new("sh")
            .args(["-c", &check_script()])
            .env("HOME", home.path())
            .output()
            .unwrap();
        let c = parse_check(&String::from_utf8_lossy(&out.stdout));
        assert_eq!((c.fleet_hooks, c.guard_hook), (Some(true), Some(true)));
        // The parsed result is all a caller gets: no token in it.
        assert!(!format!("{c:?}").contains("tok"));
    }
}
