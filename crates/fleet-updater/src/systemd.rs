//! The real [`Host`] for the binary target: a systemd unit, `tar`, the
//! binary's own `--version`, and (the hub) `fleet-hub healthcheck --ready
//! --json` and `fleet-hub backup --json` run as the unit's user with the
//! unit's environment.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use fleet_update::{Fetch, Version};

use crate::binary::{parse_version_output, Health, Host};
use crate::net::{GitFetch, HubHttp};

/// What the hub's own commands need to run as the service does.
#[derive(Debug, Clone, Default)]
pub struct HubCommands {
    /// The unit's `User=`; `None` runs them as this process.
    pub user: Option<String>,
    /// The unit's environment (`EnvironmentFile=` and `Environment=`).
    pub env: Vec<(String, String)>,
}

pub struct SystemdHost {
    pub unit: String,
    /// `systemctl --user`.
    pub user_scope: bool,
    /// `Some` for the hub: readiness and backups come from its own commands.
    /// `None` for the agent: ready means its `STATUS=` says it is connected.
    pub hub: Option<HubCommands>,
    /// The path the unit runs (for a backup by the build that runs now).
    pub link: PathBuf,
    pub git: GitFetch,
    pub mirror: Option<HubHttp>,
}

/// The agent's `STATUS=` while it is connected (`fleet_agent::conn::CONNECTED`).
pub const AGENT_CONNECTED: &str = "connected to";

fn run(cmd: &mut std::process::Command) -> Result<std::process::Output, String> {
    cmd.output()
        .map_err(|e| format!("could not run {:?}: {e}", cmd.get_program()))
}

impl SystemdHost {
    fn systemctl(&self, args: &[&str]) -> Result<std::process::Output, String> {
        let mut c = std::process::Command::new("systemctl");
        if self.user_scope {
            c.arg("--user");
        }
        c.args(args);
        run(&mut c)
    }

    fn systemctl_ok(&self, args: &[&str]) -> Result<(), String> {
        let out = self.systemctl(args)?;
        if out.status.success() {
            Ok(())
        } else {
            Err(format!(
                "systemctl {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            ))
        }
    }

    /// One of the hub's own subcommands, as the unit's user, with its env.
    fn hub_command(&self, bin: &Path, args: &[&str]) -> Result<std::process::Output, String> {
        let hub = self.hub.as_ref().ok_or("not a hub")?;
        let as_other = hub
            .user
            .as_deref()
            .filter(|u| Some(*u) != current_user().as_deref());
        let mut c = match as_other {
            Some(u) => {
                let mut c = std::process::Command::new("runuser");
                c.args(["-u", u, "--"]).arg(bin);
                c
            }
            None => std::process::Command::new(bin),
        };
        c.args(args).envs(hub.env.iter().map(|(k, v)| (k, v)));
        run(&mut c)
    }
}

fn current_user() -> Option<String> {
    std::env::var("USER").ok().filter(|u| !u.is_empty())
}

/// `KEY=VALUE` lines of a systemd `EnvironmentFile=` (comments, blanks and
/// surrounding quotes handled; no expansion, as systemd does none either).
pub fn parse_env_file(text: &str) -> Vec<(String, String)> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with(';'))
        .filter_map(|l| {
            let l = l.strip_prefix("export ").unwrap_or(l);
            let (k, v) = l.split_once('=')?;
            let k = k.trim();
            if k.is_empty()
                || k.starts_with(|c: char| c.is_ascii_digit())
                || !k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            {
                return None;
            }
            let v = v.trim();
            let v = v
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .or_else(|| v.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
                .unwrap_or(v);
            Some((k.to_string(), v.to_string()))
        })
        .collect()
}

/// `systemctl show -p A -p B` output as pairs.
pub fn parse_show(text: &str) -> std::collections::HashMap<String, String> {
    text.lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[async_trait]
impl Host for SystemdHost {
    async fn fetch(&self, url: &str, max_bytes: u64) -> Result<Vec<u8>, String> {
        self.git
            .get(url, max_bytes)
            .await
            .map_err(|e| e.to_string())
    }

    async fn fetch_mirror(&self, path: &str, max_bytes: u64) -> Result<Vec<u8>, String> {
        match &self.mirror {
            Some(h) => h.get(path, max_bytes).await,
            None => Err("no hub to fetch from".into()),
        }
    }

    fn unpack(&self, archive: &Path, into: &Path) -> Result<(), String> {
        let out = run(std::process::Command::new("tar")
            .arg("-xzf")
            .arg(archive)
            .arg("-C")
            .arg(into)
            .arg("--no-same-owner"))?;
        if out.status.success() {
            Ok(())
        } else {
            Err(format!(
                "tar: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ))
        }
    }

    fn version_of(&self, bin: &Path) -> Result<Version, String> {
        let out = run(std::process::Command::new(bin).arg("--version"))?;
        if !out.status.success() {
            return Err(format!("{} --version failed", bin.display()));
        }
        parse_version_output(&String::from_utf8_lossy(&out.stdout))
    }

    fn restart(&self) -> Result<(), String> {
        self.systemctl_ok(&["restart", &self.unit])
    }

    fn stop(&self) -> Result<(), String> {
        self.systemctl_ok(&["stop", &self.unit])
    }

    async fn health(&self) -> Health {
        let out = match self.systemctl(&[
            "show",
            &self.unit,
            "-p",
            "ActiveState",
            "-p",
            "SubState",
            "-p",
            "NRestarts",
            "-p",
            "MainPID",
            "-p",
            "StatusText",
        ]) {
            Ok(o) => o,
            Err(e) => return Health::Down(e),
        };
        let show = parse_show(&String::from_utf8_lossy(&out.stdout));
        let active = show.get("ActiveState").map(String::as_str).unwrap_or("");
        if active != "active" {
            return Health::Down(format!(
                "{} is {active}/{}",
                self.unit,
                show.get("SubState").map(String::as_str).unwrap_or("")
            ));
        }
        let restarts = show
            .get("NRestarts")
            .and_then(|n| n.parse().ok())
            .unwrap_or(0);
        let exe = show
            .get("MainPID")
            .and_then(|p| p.parse::<u32>().ok())
            .filter(|p| *p > 0)
            .and_then(|pid| std::fs::read_link(format!("/proc/{pid}/exe")).ok());
        let version = exe.as_deref().and_then(|e| self.version_of(e).ok());
        match &self.hub {
            None => {
                let status = show.get("StatusText").cloned().unwrap_or_default();
                let ready = status.starts_with(AGENT_CONNECTED);
                Health::Up {
                    version,
                    ready,
                    schema: None,
                    restarts,
                    why: (!ready).then(|| {
                        if status.is_empty() {
                            "not connected to its hub yet".into()
                        } else {
                            status
                        }
                    }),
                }
            }
            Some(_) => {
                let bin = exe.unwrap_or_else(|| self.link.clone());
                let report = self
                    .hub_command(&bin, &["healthcheck", "--ready", "--json"])
                    .ok()
                    .and_then(|o| serde_json::from_slice::<serde_json::Value>(&o.stdout).ok());
                let Some(r) = report else {
                    return Health::Up {
                        version,
                        ready: false,
                        schema: None,
                        restarts,
                        why: Some("healthcheck --ready printed no report".into()),
                    };
                };
                let ready = r["ready"].as_bool().unwrap_or(false)
                    && r["live"].as_bool().unwrap_or(false)
                    && r["fresh"].as_bool().unwrap_or(false);
                Health::Up {
                    version: version.or_else(|| {
                        r["hub"]["version"]
                            .as_str()
                            .and_then(|v| Version::parse(v).ok())
                    }),
                    ready,
                    schema: r["hub"]["schema"].as_i64(),
                    restarts,
                    why: (!ready)
                        .then(|| r["error"].as_str().unwrap_or("not ready yet").to_string()),
                }
            }
        }
    }

    fn backup(&self, prefix: &str) -> Result<Option<(PathBuf, Option<i64>)>, String> {
        if self.hub.is_none() {
            return Ok(None);
        }
        let out = self.hub_command(&self.link, &["backup", "--prefix", prefix, "--json"])?;
        if !out.status.success() {
            return Err(format!(
                "fleet-hub backup failed (exit {:?}): {}",
                out.status.code(),
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        let v: serde_json::Value = serde_json::from_slice(&out.stdout)
            .map_err(|e| format!("fleet-hub backup printed something else: {e}"))?;
        let path = v["path"]
            .as_str()
            .map(PathBuf::from)
            .ok_or("fleet-hub backup named no path")?;
        Ok(Some((path, v["schema"].as_i64())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_environment_file_reads_like_systemd_reads_it() {
        let env = parse_env_file(
            "# comment\n\nFLEET_HUB_TOKEN=abc\nexport FLEET_HUB_BIND=\"0.0.0.0:4180\"\nX='y z'\nbad line\n1=x\n",
        );
        assert_eq!(
            env,
            vec![
                ("FLEET_HUB_TOKEN".into(), "abc".into()),
                ("FLEET_HUB_BIND".into(), "0.0.0.0:4180".into()),
                ("X".into(), "y z".into()),
            ]
        );
    }

    #[test]
    fn systemctl_show_reads_as_pairs() {
        let s =
            parse_show("ActiveState=active\nNRestarts=2\nStatusText=connected to wss://h/agent\n");
        assert_eq!(s["ActiveState"], "active");
        assert_eq!(s["NRestarts"], "2");
        assert!(s["StatusText"].starts_with(AGENT_CONNECTED));
    }
}
