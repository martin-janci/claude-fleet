//! `fleet-hub org …` — the hub operator's side of `work_admin`'s org actions
//! (work graph M5.2): name organisations, the rules that place sessions in
//! them, which org each host and tracker belongs to, and whether an org
//! isolates its sessions. Each subcommand is one `work_admin` call over
//! loopback with the master token, exactly like `fleet-hub tracker …`.
//!
//! A host's org is its per-host token's BOUNDARY: that host's Claude then
//! reads only its org's (and unassigned) work. Only this master path can set
//! it — a host can never move itself.

use crate::config::HubOptions;
use crate::out;
use crate::pair::{call_tool, hub_conn};
use clap::{Subcommand, ValueEnum};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::process::ExitCode;

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum OnOff {
    On,
    Off,
}

impl OnOff {
    fn on(self) -> bool {
        self == OnOff::On
    }
}

#[derive(Subcommand, Debug)]
pub enum OrgCmd {
    /// Print the orgs with their rules, hosts and trackers.
    List,
    /// Name an org.
    Add {
        name: String,
        /// #rgb or #rrggbb, for the UI's colour bar.
        #[arg(long)]
        color: Option<String>,
        /// Also fence sessions (list, peer reads, messages) between this org
        /// and every other, for per-host tokens. Work data is always fenced.
        #[arg(long, value_enum)]
        isolate_sessions: Option<OnOff>,
    },
    /// Rename, recolour, or turn session isolation on or off.
    Set {
        /// The org's id, from `org list`.
        id: i64,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        color: Option<String>,
        #[arg(long, value_enum)]
        isolate_sessions: Option<OnOff>,
    },
    /// Remove an org: its rules go, its hosts become unassigned. Refused
    /// while a tracker belongs to it.
    Rm { id: i64 },
    /// Add or remove a placement rule.
    Rule {
        #[command(subcommand)]
        cmd: RuleCmd,
    },
    /// Put a host in an org — its token's boundary.
    AssignHost {
        host: String,
        /// The org's id; omit with --none to take the host out of its org.
        org: Option<i64>,
        /// Unassign the host.
        #[arg(long, conflicts_with = "org")]
        none: bool,
    },
    /// Put a tracker (and so its tickets) in an org.
    AssignTracker {
        tracker: i64,
        /// The org's id; omit with --none to unassign.
        org: Option<i64>,
        #[arg(long, conflicts_with = "org")]
        none: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum RuleCmd {
    /// Sessions matching every given field belong to the org. The most
    /// specific rule wins: path > owner/repo > owner > host.
    Add {
        /// The org's id.
        org: i64,
        /// A GitHub owner (`acme`); never `local`.
        #[arg(long)]
        owner: Option<String>,
        /// With --owner: one repo.
        #[arg(long, requires = "owner")]
        repo: Option<String>,
        /// A path prefix (`/home/me/work/acme`), matched on a directory
        /// boundary against the worktree's, else the project's path.
        #[arg(long)]
        path: Option<String>,
        /// Only on this host (alone: every session on the host).
        #[arg(long)]
        host: Option<String>,
    },
    /// Remove a rule by its id, from `org list`.
    Rm { id: i64 },
}

/// The `work_admin` arguments for one subcommand (everything but `list`).
fn admin_args(cmd: &OrgCmd) -> Result<Value, String> {
    let mut a = match cmd {
        OrgCmd::List => json!({ "action": "list_orgs" }),
        OrgCmd::Add {
            name,
            color,
            isolate_sessions,
        } => {
            let mut a = json!({ "action": "add_org", "name": name });
            if let Some(c) = color {
                a["color"] = json!(c);
            }
            if let Some(i) = isolate_sessions {
                a["isolate_sessions"] = json!(i.on());
            }
            a
        }
        OrgCmd::Set {
            id,
            name,
            color,
            isolate_sessions,
        } => {
            if name.is_none() && color.is_none() && isolate_sessions.is_none() {
                return Err("nothing to set: pass --name, --color or --isolate-sessions".into());
            }
            let mut a = json!({ "action": "update_org", "org_id": id });
            if let Some(n) = name {
                a["name"] = json!(n);
            }
            if let Some(c) = color {
                a["color"] = json!(c);
            }
            if let Some(i) = isolate_sessions {
                a["isolate_sessions"] = json!(i.on());
            }
            a
        }
        OrgCmd::Rm { id } => json!({ "action": "remove_org", "org_id": id }),
        OrgCmd::Rule { cmd } => match cmd {
            RuleCmd::Add {
                org,
                owner,
                repo,
                path,
                host,
            } => {
                if owner.is_none() && path.is_none() && host.is_none() {
                    return Err("a rule needs --owner, --path or --host".into());
                }
                json!({ "action": "add_rule", "org_id": org, "owner": owner, "repo": repo,
                        "path_prefix": path, "host_alias": host })
            }
            RuleCmd::Rm { id } => json!({ "action": "remove_rule", "rule_id": id }),
        },
        OrgCmd::AssignHost { host, org, none } => match (org, none) {
            (Some(o), false) => json!({ "action": "assign_host", "host_alias": host, "org_id": o }),
            (None, true) => json!({ "action": "unassign_host", "host_alias": host }),
            _ => return Err("give the org's id, or --none".into()),
        },
        OrgCmd::AssignTracker { tracker, org, none } => match (org, none) {
            (Some(o), false) => {
                json!({ "action": "assign_tracker", "tracker_id": tracker, "org_id": o })
            }
            (None, true) => json!({ "action": "assign_tracker", "tracker_id": tracker }),
            _ => return Err("give the org's id, or --none".into()),
        },
    };
    // Unset optional fields travel as absent, not null.
    if let Some(m) = a.as_object_mut() {
        m.retain(|_, v| !v.is_null());
    }
    Ok(a)
}

/// A rule as a chip: `acme/*`, `acme/api`, `path: /src/acme`, `host: h`.
fn rule_chip(r: &Value) -> String {
    let mut parts = Vec::new();
    match (r["owner"].as_str(), r["repo"].as_str()) {
        (Some(o), Some(repo)) => parts.push(format!("{o}/{repo}")),
        (Some(o), None) => parts.push(format!("{o}/*")),
        _ => {}
    }
    if let Some(p) = r["path_prefix"].as_str() {
        parts.push(format!("path: {p}"));
    }
    if let Some(h) = r["host_alias"].as_str() {
        parts.push(format!("host: {h}"));
    }
    format!(
        "#{} {}",
        r["id"].as_i64().unwrap_or_default(),
        parts.join(" · ")
    )
}

fn org_lines(o: &Value) -> Vec<String> {
    let names = |k: &str, f: &dyn Fn(&Value) -> String| -> String {
        o[k].as_array()
            .map(|a| a.iter().map(f).collect::<Vec<_>>().join(", "))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "—".into())
    };
    vec![
        format!(
            "{:>4}  {}{}{}",
            o["id"].as_i64().unwrap_or_default(),
            o["name"].as_str().unwrap_or_default(),
            o["color"]
                .as_str()
                .map(|c| format!("  {c}"))
                .unwrap_or_default(),
            if o["isolate_sessions"].as_bool().unwrap_or(false) {
                "  [isolates sessions]"
            } else {
                ""
            }
        ),
        format!("      rules:    {}", names("rules", &rule_chip)),
        format!(
            "      hosts:    {}",
            names("hosts", &|h| h.as_str().unwrap_or_default().to_string())
        ),
        format!(
            "      trackers: {}",
            names("trackers", &|t| format!(
                "{} (#{})",
                t["name"].as_str().unwrap_or_default(),
                t["id"].as_i64().unwrap_or_default()
            ))
        ),
    ]
}

pub async fn run(
    cmd: OrgCmd,
    opts: &HubOptions,
    env: &HashMap<String, String>,
) -> Result<ExitCode, String> {
    let args = admin_args(&cmd)?;
    let conn = hub_conn(opts, env)?;
    let v = call_tool(&conn, "work_admin", args).await?;
    match cmd {
        OrgCmd::List => {
            let rows = v.as_array().cloned().unwrap_or_default();
            if rows.is_empty() {
                out::line("no orgs; add one with `fleet-hub org add <name>`");
            }
            for o in rows {
                for l in org_lines(&o) {
                    out::line(&l);
                }
            }
        }
        OrgCmd::Add { .. } | OrgCmd::Set { .. } => {
            for l in org_lines(&v) {
                out::line(&l);
            }
            if matches!(cmd, OrgCmd::Add { .. }) {
                out::line(&format!(
                    "next: fleet-hub org rule add {} --owner <github-owner>",
                    v["id"].as_i64().unwrap_or_default()
                ));
            }
        }
        OrgCmd::Rule {
            cmd: RuleCmd::Add { .. },
        } => out::line(&format!("added rule {}", rule_chip(&v))),
        OrgCmd::Rule {
            cmd: RuleCmd::Rm { id },
        } => out::line(&format!("removed rule {id}")),
        OrgCmd::Rm { id } => out::line(&format!("removed org {id}; its hosts are unassigned")),
        OrgCmd::AssignHost { host, .. } => match v["org_id"].as_i64() {
            Some(o) => out::line(&format!(
                "host {host} is in org {o}; its token now reads only that org's work"
            )),
            None => out::line(&format!(
                "host {host} has no org; its token now reads only unassigned work"
            )),
        },
        OrgCmd::AssignTracker { tracker, .. } => out::line(&format!(
            "tracker {tracker}: org {}",
            v["org_id"]
                .as_i64()
                .map(|o| o.to_string())
                .unwrap_or_else(|| "none".into())
        )),
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct T {
        #[command(subcommand)]
        cmd: OrgCmd,
    }

    fn parse(args: &[&str]) -> Result<OrgCmd, clap::Error> {
        let mut all = vec!["t"];
        all.extend_from_slice(args);
        T::try_parse_from(all).map(|t| t.cmd)
    }

    fn args(a: &[&str]) -> Value {
        admin_args(&parse(a).unwrap()).unwrap()
    }

    #[test]
    fn each_subcommand_is_one_work_admin_call() {
        assert_eq!(args(&["list"]), json!({ "action": "list_orgs" }));
        assert_eq!(
            args(&[
                "add",
                "Company A",
                "--color",
                "#f00",
                "--isolate-sessions",
                "on"
            ]),
            json!({ "action": "add_org", "name": "Company A", "color": "#f00",
                    "isolate_sessions": true })
        );
        assert_eq!(
            args(&["set", "2", "--isolate-sessions", "off"]),
            json!({ "action": "update_org", "org_id": 2, "isolate_sessions": false })
        );
        assert_eq!(
            args(&["rm", "2"]),
            json!({ "action": "remove_org", "org_id": 2 })
        );
        assert_eq!(
            args(&["rule", "add", "1", "--owner", "acme", "--repo", "api"]),
            json!({ "action": "add_rule", "org_id": 1, "owner": "acme", "repo": "api" })
        );
        assert_eq!(
            args(&["rule", "add", "1", "--path", "/work/acme", "--host", "h"]),
            json!({ "action": "add_rule", "org_id": 1, "path_prefix": "/work/acme",
                    "host_alias": "h" })
        );
        assert_eq!(
            args(&["rule", "rm", "4"]),
            json!({ "action": "remove_rule", "rule_id": 4 })
        );
        assert_eq!(
            args(&["assign-host", "hetzner-a", "1"]),
            json!({ "action": "assign_host", "host_alias": "hetzner-a", "org_id": 1 })
        );
        assert_eq!(
            args(&["assign-host", "hetzner-a", "--none"]),
            json!({ "action": "unassign_host", "host_alias": "hetzner-a" })
        );
        assert_eq!(
            args(&["assign-tracker", "3", "1"]),
            json!({ "action": "assign_tracker", "tracker_id": 3, "org_id": 1 })
        );
        assert_eq!(
            args(&["assign-tracker", "3", "--none"]),
            json!({ "action": "assign_tracker", "tracker_id": 3 })
        );
    }

    #[test]
    fn incomplete_or_contradictory_commands_are_refused() {
        assert!(admin_args(&parse(&["set", "2"]).unwrap()).is_err());
        assert!(admin_args(&parse(&["rule", "add", "1"]).unwrap()).is_err());
        assert!(admin_args(&parse(&["assign-host", "h"]).unwrap()).is_err());
        assert!(parse(&["assign-host", "h", "1", "--none"]).is_err());
        assert!(
            parse(&["rule", "add", "1", "--repo", "api"]).is_err(),
            "repo needs owner"
        );
        assert!(parse(&["add", "A", "--isolate-sessions", "maybe"]).is_err());
    }

    #[test]
    fn an_org_prints_its_rules_as_chips() {
        let lines = org_lines(&json!({
            "id": 1, "name": "Company A", "color": "#f00", "isolate_sessions": true,
            "rules": [ { "id": 3, "org_id": 1, "owner": "acme" },
                       { "id": 4, "org_id": 1, "owner": "acme", "repo": "api", "host_alias": "h" },
                       { "id": 5, "org_id": 1, "path_prefix": "/w/acme" } ],
            "hosts": ["hetzner-a"], "trackers": [ { "id": 2, "name": "acme" } ]
        }));
        assert!(lines[0].contains("Company A") && lines[0].contains("isolates"));
        assert!(lines[1].contains("#3 acme/*"));
        assert!(lines[1].contains("#4 acme/api · host: h"));
        assert!(lines[1].contains("#5 path: /w/acme"));
        assert!(lines[2].contains("hetzner-a"));
        assert!(lines[3].contains("acme (#2)"));
    }
}
