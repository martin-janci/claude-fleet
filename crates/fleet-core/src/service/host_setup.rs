//! The add-host wizard (Orbit Fleet 4.9, board `Wizard`): drafts that
//! survive a restart, and the live checks of its "Check the host" step.
//!
//! A check is one small script over SSH to the alias being added, run one at
//! a time by the wizard so each row lands as its answer comes back. Nothing
//! here installs anything: a check only reads. Installing fleet-agent is a
//! separate job a hub runs ([`crate::service::agent_install`]).
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::ipc_error::{codes, IpcError};
use crate::service::host_check::AGENT_BINARIES;
use crate::ssh::SshExec;
use crate::store::{HostSetupRow, SetupCheck, Store};

/// Every check, in the board's order.
pub const SETUP_CHECKS: [&str; 6] = ["ssh", "tmux", "git", "agent", "disk", "agents"];

/// Free space on `$HOME` below which the disk check warns, and below which
/// it fails: worktrees and their builds live there.
pub const DISK_WARN_KB: i64 = 10 * 1024 * 1024;
pub const DISK_FAIL_KB: i64 = 2 * 1024 * 1024;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const CHECK_WALL: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Serialize, Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct SaveHostSetupArgs {
    pub ssh_alias: String,
    pub alias: String,
    pub step: i64,
    /// The frontend's own answers (agents picked, account noted).
    #[serde(default)]
    pub answers: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct HostSetupCheckArgs {
    pub ssh_alias: String,
    /// One of [`SETUP_CHECKS`].
    pub key: String,
}

fn validate(ssh_alias: &str) -> Result<(), IpcError> {
    crate::validate::host_alias_syntax(ssh_alias)?;
    if ssh_alias == crate::service::projects::LOCAL_HOST {
        return Err(IpcError::new(
            codes::E_INVALID,
            "this machine is already in fleet as `local`; the wizard adds SSH hosts",
        ));
    }
    Ok(())
}

/// Save where the person is. Checks already run for this alias are kept.
pub fn save(store: &Store, args: &SaveHostSetupArgs) -> Result<HostSetupRow, IpcError> {
    validate(&args.ssh_alias)?;
    crate::validate::host_alias(&args.alias)?;
    if !(1..=5).contains(&args.step) {
        return Err(IpcError::new(codes::E_INVALID, "step must be 1..5"));
    }
    let checks = store
        .host_setup(&args.ssh_alias)?
        .map(|r| r.checks)
        .unwrap_or_default();
    let answers = if args.answers.is_null() {
        serde_json::Value::Object(Default::default())
    } else {
        args.answers.clone()
    };
    Ok(store.save_host_setup(&args.ssh_alias, &args.alias, args.step, &checks, &answers)?)
}

/// Record one check's answer on the draft, replacing that key's last one.
/// No draft (the person discarded it meanwhile): nothing is written.
pub fn record(store: &Store, ssh_alias: &str, check: &SetupCheck) -> Result<(), IpcError> {
    let Some(row) = store.host_setup(ssh_alias)? else {
        return Ok(());
    };
    let mut checks: Vec<SetupCheck> = row
        .checks
        .into_iter()
        .filter(|c| c.key != check.key)
        .collect();
    checks.push(check.clone());
    checks.sort_by_key(|c| {
        SETUP_CHECKS
            .iter()
            .position(|k| *k == c.key)
            .unwrap_or(usize::MAX)
    });
    store.set_host_setup_checks(ssh_alias, &checks)?;
    Ok(())
}

/// The script one check runs. Nothing is interpolated.
pub fn check_script(key: &str) -> Option<String> {
    Some(match key {
        "ssh" => "printf 'who=%s@%s\\n' \"$(id -un)\" \"$(hostname)\"".to_string(),
        "tmux" => "printf 'tmuxv=%s\\n' \"$(tmux -V 2>/dev/null)\"".to_string(),
        "git" => "printf 'gitv=%s\\n' \"$(git --version 2>/dev/null)\"; \
                  if command -v gh >/dev/null 2>&1; then \
                    if gh auth status >/dev/null 2>&1; then echo gh=in; else echo gh=out; fi; \
                  else echo gh=none; fi"
            .to_string(),
        "agent" => "if command -v fleet-agent >/dev/null 2>&1; then \
                      printf 'fa=%s\\n' \"$(fleet-agent --version 2>/dev/null)\"; \
                    elif [ -x \"$HOME/.local/bin/fleet-agent\" ]; then \
                      printf 'fa=%s\\n' \"$(\"$HOME/.local/bin/fleet-agent\" --version 2>/dev/null)\"; \
                    else echo fa=; fi"
            .to_string(),
        "disk" => "df -Pk \"$HOME\" 2>/dev/null | awk 'NR==2 {print \"free=\" $4}'".to_string(),
        "agents" => format!(
            "for a in {}; do command -v \"$a\" >/dev/null 2>&1 && printf 'agent=%s\\n' \"$a\"; done; \
             printf 'claudev=%s\\n' \"$(claude --version 2>/dev/null | head -1)\"",
            AGENT_BINARIES.join(" ")
        ),
        _ => return None,
    })
}

fn value<'a>(stdout: &'a str, key: &str) -> Option<&'a str> {
    stdout
        .lines()
        .find_map(|l| l.strip_prefix(key).and_then(|r| r.strip_prefix('=')))
        .map(str::trim)
}

fn row(key: &str, state: &str, label: impl Into<String>, detail: impl Into<String>) -> SetupCheck {
    SetupCheck {
        key: key.to_string(),
        state: state.to_string(),
        label: label.into(),
        detail: detail.into(),
    }
}

/// `412 GB`, `1.2 TB`, `840 MB` from kB.
fn size(kb: i64) -> String {
    let gb = kb as f64 / (1024.0 * 1024.0);
    if gb >= 1024.0 {
        format!("{:.1} TB", gb / 1024.0)
    } else if gb >= 10.0 {
        format!("{} GB", gb.round() as i64)
    } else if gb >= 1.0 {
        format!("{gb:.1} GB")
    } else {
        format!("{} MB", kb / 1024)
    }
}

/// Turn a check's output into its row. `ms` is the round trip (the SSH
/// row's detail); `agents_accepted` says whether fleet-agent can be used
/// at all from here (only a hub accepts agents).
pub fn parse(key: &str, stdout: &str, ms: u128, agents_accepted: bool) -> SetupCheck {
    match key {
        "ssh" => row(
            "ssh",
            "ok",
            match value(stdout, "who") {
                Some(w) if !w.is_empty() && w != "@" => format!("SSH as {w}"),
                _ => "SSH".to_string(),
            },
            format!("{ms} ms"),
        ),
        "tmux" => {
            match value(stdout, "tmuxv").and_then(crate::service::hosts::parse_tmux_version) {
                Some(v) => row("tmux", "ok", format!("tmux {v}"), "ok"),
                None => row("tmux", "fail", "tmux not installed", "sessions run in tmux"),
            }
        }
        "git" => {
            let git = value(stdout, "gitv")
                .and_then(|v| v.strip_prefix("git version "))
                .map(|v| v.split_whitespace().next().unwrap_or(v).to_string());
            match (git, value(stdout, "gh")) {
                (None, _) => row("git", "fail", "git not installed", "worktrees need git"),
                (Some(v), Some("in")) => row("git", "ok", format!("git {v} · gh signed in"), "ok"),
                (Some(v), Some("out")) => row(
                    "git",
                    "warn",
                    format!("git {v} · gh not signed in"),
                    "pull requests need `gh auth login`",
                ),
                (Some(v), _) => row(
                    "git",
                    "warn",
                    format!("git {v} · no gh"),
                    "pull requests need gh",
                ),
            }
        }
        "agent" => match value(stdout, "fa").filter(|v| !v.is_empty()) {
            Some(v) => {
                let v = v.strip_prefix("fleet-agent ").unwrap_or(v);
                row("agent", "ok", format!("fleet-agent {v}"), "ok")
            }
            None if agents_accepted => row(
                "agent",
                "warn",
                "fleet-agent not installed",
                format!("Install {}", crate::app_version::get()),
            ),
            None => row(
                "agent",
                "na",
                "fleet-agent not needed",
                "this app reaches the host over SSH",
            ),
        },
        "disk" => match value(stdout, "free").and_then(|v| v.parse::<i64>().ok()) {
            Some(kb) => {
                let state = if kb < DISK_FAIL_KB {
                    "fail"
                } else if kb < DISK_WARN_KB {
                    "warn"
                } else {
                    "ok"
                };
                row(
                    "disk",
                    state,
                    "Disk space for worktrees",
                    format!("{} free", size(kb)),
                )
            }
            None => row("disk", "warn", "Disk space for worktrees", "could not read"),
        },
        "agents" => {
            let mut found: Vec<&str> = Vec::new();
            for l in stdout.lines() {
                if let Some(a) = l.strip_prefix("agent=").map(str::trim) {
                    if let Some(known) = AGENT_BINARIES.iter().find(|k| **k == a) {
                        if !found.contains(known) {
                            found.push(known);
                        }
                    }
                }
            }
            let names: Vec<&str> = found.iter().map(|a| agent_name(a)).collect();
            if !found.contains(&"claude") {
                row(
                    "agents",
                    "fail",
                    if names.is_empty() {
                        "No agents on PATH".to_string()
                    } else {
                        format!("{} on PATH", names.join(", "))
                    },
                    "Claude Code not found",
                )
            } else {
                let v = value(stdout, "claudev")
                    .and_then(|v| v.split_whitespace().next())
                    .filter(|v| !v.is_empty())
                    .map(|v| format!("Claude Code {v}"))
                    .unwrap_or_else(|| "ok".to_string());
                row("agents", "ok", format!("{} on PATH", names.join(", ")), v)
            }
        }
        other => row(other, "fail", other.to_string(), "unknown check"),
    }
}

/// The display name of an agent binary.
pub fn agent_name(bin: &str) -> &str {
    match bin {
        "claude" => "Claude Code",
        "codex" => "Codex",
        "agy" => "Agy",
        "gemini" => "Gemini CLI",
        other => other,
    }
}

/// Run one check against `ssh_alias`. A host that cannot be asked is a
/// `fail` row carrying the reason, never an error: the wizard shows it.
pub async fn run_check(
    ssh: &dyn SshExec,
    ssh_alias: &str,
    key: &str,
    agents_accepted: bool,
) -> Result<SetupCheck, IpcError> {
    validate(ssh_alias)?;
    let script = check_script(key)
        .ok_or_else(|| IpcError::new(codes::E_INVALID, format!("unknown check {key:?}")))?;
    let started = Instant::now();
    let out =
        crate::ssh::run_shell_bounded(ssh, ssh_alias, &script, CONNECT_TIMEOUT, CHECK_WALL).await;
    let ms = started.elapsed().as_millis();
    Ok(match out {
        Ok(o) if o.status.success() || key != "ssh" => parse(
            key,
            &String::from_utf8_lossy(&o.stdout),
            ms,
            agents_accepted,
        ),
        Ok(o) => row(
            key,
            "fail",
            "SSH",
            first_line(
                &String::from_utf8_lossy(&o.stderr),
                "the host did not answer",
            ),
        ),
        Err(e) => row(
            key,
            "fail",
            if key == "ssh" { "SSH" } else { key },
            e.message,
        ),
    })
}

fn first_line(s: &str, fallback: &str) -> String {
    s.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or(fallback)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_check_reads_its_answer() {
        let c = parse("ssh", "who=martin@mercury\n", 18, false);
        assert_eq!(
            (c.state.as_str(), c.label.as_str(), c.detail.as_str()),
            ("ok", "SSH as martin@mercury", "18 ms")
        );
        assert_eq!(
            parse("tmux", "tmuxv=tmux 3.4\n", 0, false).label,
            "tmux 3.4"
        );
        assert_eq!(parse("tmux", "tmuxv=\n", 0, false).state, "fail");

        let g = parse("git", "gitv=git version 2.47.1\ngh=in\n", 0, false);
        assert_eq!(
            (g.state.as_str(), g.label.as_str()),
            ("ok", "git 2.47.1 · gh signed in")
        );
        assert_eq!(
            parse("git", "gitv=git version 2.47\ngh=out\n", 0, false).state,
            "warn"
        );
        assert_eq!(
            parse("git", "gitv=git version 2.47\ngh=none\n", 0, false).state,
            "warn"
        );
        assert_eq!(parse("git", "gitv=\ngh=none\n", 0, false).state, "fail");

        let d = parse("disk", "free=432013312\n", 0, false);
        assert_eq!((d.state.as_str(), d.detail.as_str()), ("ok", "412 GB free"));
        assert_eq!(
            parse("disk", &format!("free={}\n", DISK_WARN_KB - 1), 0, false).state,
            "warn"
        );
        assert_eq!(parse("disk", "free=1024\n", 0, false).state, "fail");

        let a = parse(
            "agents",
            "agent=claude\nagent=codex\nagent=evil\nclaudev=2.1.3 (Claude Code)\n",
            0,
            false,
        );
        assert_eq!(
            (a.state.as_str(), a.label.as_str(), a.detail.as_str()),
            ("ok", "Claude Code, Codex on PATH", "Claude Code 2.1.3")
        );
        let a = parse("agents", "agent=codex\nclaudev=\n", 0, false);
        assert_eq!(
            (a.state.as_str(), a.detail.as_str()),
            ("fail", "Claude Code not found")
        );
    }

    #[test]
    fn fleet_agent_is_offered_only_where_agents_are_accepted() {
        assert_eq!(parse("agent", "fa=\n", 0, false).state, "na");
        let hub = parse("agent", "fa=\n", 0, true);
        assert_eq!(hub.state, "warn");
        assert!(hub.detail.starts_with("Install "));
        let there = parse("agent", "fa=fleet-agent 0.5.4\n", 0, true);
        assert_eq!(
            (there.state.as_str(), there.label.as_str()),
            ("ok", "fleet-agent 0.5.4")
        );
    }

    #[test]
    fn a_draft_keeps_one_answer_per_check_in_board_order() {
        let s = Store::open_in_memory().unwrap();
        let args = SaveHostSetupArgs {
            ssh_alias: "mercury".into(),
            alias: "mercury".into(),
            step: 2,
            answers: serde_json::Value::Null,
        };
        save(&s, &args).unwrap();
        record(&s, "mercury", &parse("tmux", "tmuxv=tmux 3.4\n", 0, false)).unwrap();
        record(&s, "mercury", &parse("ssh", "who=a@b\n", 5, false)).unwrap();
        record(&s, "mercury", &parse("tmux", "tmuxv=\n", 0, false)).unwrap();
        let checks = s.host_setup("mercury").unwrap().unwrap().checks;
        let keys: Vec<_> = checks
            .iter()
            .map(|c| (c.key.as_str(), c.state.as_str()))
            .collect();
        assert_eq!(keys, [("ssh", "ok"), ("tmux", "fail")]);
        // Saving the step again keeps the checks.
        save(&s, &SaveHostSetupArgs { step: 3, ..args }).unwrap();
        assert_eq!(s.host_setup("mercury").unwrap().unwrap().checks.len(), 2);
        // A discarded draft takes no more answers.
        s.delete_host_setup("mercury").unwrap();
        record(&s, "mercury", &parse("ssh", "who=a@b\n", 5, false)).unwrap();
        assert!(s.host_setups().unwrap().is_empty());
    }

    #[test]
    fn hostile_or_local_aliases_are_refused() {
        let s = Store::open_in_memory().unwrap();
        for bad in ["-oProxyCommand=x", "local"] {
            let args = SaveHostSetupArgs {
                ssh_alias: bad.into(),
                alias: "x".into(),
                step: 1,
                answers: serde_json::Value::Null,
            };
            assert!(save(&s, &args).is_err(), "{bad}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn every_script_runs_under_sh() {
        let home = tempfile::tempdir().unwrap();
        for key in SETUP_CHECKS {
            let out = std::process::Command::new("bash")
                .args(["-c", &check_script(key).unwrap()])
                .env("HOME", home.path())
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{key}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            let c = parse(key, &String::from_utf8_lossy(&out.stdout), 1, true);
            assert_eq!(c.key, key);
        }
        let disk = std::process::Command::new("bash")
            .args(["-c", &check_script("disk").unwrap()])
            .env("HOME", home.path())
            .output()
            .unwrap();
        assert!(String::from_utf8_lossy(&disk.stdout).starts_with("free="));
    }
}
