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

/// An org's auto-tidy override (work graph M7).
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum AutoTidy {
    On,
    Off,
    Inherit,
}

impl AutoTidy {
    fn as_str(self) -> &'static str {
        match self {
            AutoTidy::On => "on",
            AutoTidy::Off => "off",
            AutoTidy::Inherit => "inherit",
        }
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
    /// Rename, recolour, or change the org's switches (session isolation,
    /// auto-tidy, what its bound devices see).
    Set {
        /// The org's id, from `org list`.
        id: i64,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        color: Option<String>,
        #[arg(long, value_enum)]
        isolate_sessions: Option<OnOff>,
        /// This org's auto-tidy (work graph M7): on / off override
        /// `work.auto_tidy` for its sessions, inherit follows it.
        #[arg(long, value_enum)]
        auto_tidy: Option<AutoTidy>,
        /// Let this org's redacted texts go to the decision model (Jev
        /// evaluation, docs/decisions.md) when `decide.jev.enabled` and a
        /// feature's mode allow it. Off by default.
        #[arg(long, value_enum)]
        jev: Option<OnOff>,
        /// Also let this org's sessions' reply text (the end of the pane at
        /// a turn's end, J2 turn_outcome) go to the decision model — on top
        /// of --jev (decision D48). Off by default.
        #[arg(long, value_enum)]
        jev_reply: Option<OnOff>,
        /// Devices bound to this org (`pair --org`) also see unassigned work
        /// and sessions, as a host does (on, the default); off: only the
        /// org's own (decision D31).
        #[arg(long, value_enum)]
        bound_sees_unassigned: Option<OnOff>,
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
    /// Who is in an org, with which role (org administration phase D).
    /// Writes `state.db` directly, as `fleet-hub person` does; a running hub
    /// honours a change from its next request.
    Member {
        #[command(subcommand)]
        cmd: MemberCmd,
    },
    /// The company that owns this hub: its admins administer hosts (route
    /// them into orgs, see unclaimed counts). Writes `state.db` directly.
    OwnHub {
        /// The org's id; omit with --none for no company.
        org: Option<i64>,
        #[arg(long, conflicts_with = "org")]
        none: bool,
    },
    /// Whether an org's admins see the count of unclaimed sessions on the
    /// org's hosts (off by default). Writes `state.db` directly.
    UnclaimedCount {
        org: i64,
        #[arg(value_enum)]
        state: OnOff,
    },
}

/// A member's role.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Role {
    /// Administers the org: its settings, members and their devices.
    Admin,
    /// Sees the org's work and what is shared with the org.
    Member,
    /// Reads the org's work view and overview, read-only.
    Viewer,
}

impl Role {
    fn as_str(self) -> &'static str {
        match self {
            Role::Admin => "admin",
            Role::Member => "member",
            Role::Viewer => "viewer",
        }
    }
}

#[derive(Subcommand, Debug)]
pub enum MemberCmd {
    /// The org's live members, admins first.
    List {
        org: i64,
        #[arg(long)]
        json: bool,
    },
    /// Add a person to the org (by name; a new name becomes a person), or
    /// change their role. Their devices are then fenced to the org.
    Add {
        org: i64,
        person: String,
        #[arg(long, value_enum, default_value = "member")]
        role: Role,
    },
    /// Take a person out of the org. What was shared with them on the org's
    /// sessions is revoked too, unless --keep-grants. Their own sessions
    /// stay theirs; if this was their last org, their devices read nothing
    /// of any org.
    Rm {
        org: i64,
        person: String,
        #[arg(long)]
        keep_grants: bool,
    },
    /// How many grants TO a member stand on the org's sessions; --narrow
    /// lowers every drive to watch, --revoke takes them all back.
    Grants {
        org: i64,
        person: String,
        #[arg(long, conflicts_with = "revoke")]
        narrow: bool,
        #[arg(long)]
        revoke: bool,
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
            auto_tidy,
            jev,
            jev_reply,
            bound_sees_unassigned,
        } => {
            if name.is_none()
                && color.is_none()
                && isolate_sessions.is_none()
                && auto_tidy.is_none()
                && jev.is_none()
                && jev_reply.is_none()
                && bound_sees_unassigned.is_none()
            {
                return Err("nothing to set: pass --name, --color, --isolate-sessions, \
                     --auto-tidy, --jev, --jev-reply or --bound-sees-unassigned"
                    .into());
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
            if let Some(t) = auto_tidy {
                a["auto_tidy"] = json!(t.as_str());
            }
            if let Some(j) = jev {
                a["jev"] = json!(if j.on() { "on" } else { "off" });
            }
            if let Some(j) = jev_reply {
                a["jev_reply"] = json!(if j.on() { "on" } else { "off" });
            }
            if let Some(b) = bound_sees_unassigned {
                a["bound_sees_unassigned"] = json!(b.on());
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
        OrgCmd::Member { .. } | OrgCmd::OwnHub { .. } | OrgCmd::UnclaimedCount { .. } => {
            return Err("this subcommand writes state.db directly".into())
        }
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
            format!(
                "{}{}{}",
                if o["isolate_sessions"].as_bool().unwrap_or(false) {
                    "  [isolates sessions]"
                } else {
                    ""
                },
                match o["auto_tidy"].as_bool() {
                    Some(true) => "  [auto-tidy on]",
                    Some(false) => "  [auto-tidy off]",
                    None => "",
                },
                // D31: only the off state is news (on is the default, and an
                // older hub that sends no field has no bound devices).
                if o["bound_sees_unassigned"].as_bool() == Some(false) {
                    "  [bound devices: own org only]"
                } else {
                    ""
                }
            ) + if o["jev_allowed"].as_bool().unwrap_or(false) {
                "  [sends to Jev]"
            } else {
                ""
            } + if o["jev_reply_allowed"].as_bool().unwrap_or(false) {
                "  [replies too]"
            } else {
                ""
            } + if o["owns_hub"].as_bool().unwrap_or(false) {
                "  [owns this hub]"
            } else {
                ""
            } + if o["admins_see_unclaimed"].as_bool().unwrap_or(false) {
                "  [admins see unclaimed]"
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

/// The phase-D subcommands, straight to `state.db` (see `fleet-hub
/// person`'s module docs for why that is the operator's authority).
fn run_direct(
    cmd: &OrgCmd,
    opts: &HubOptions,
    env: &HashMap<String, String>,
) -> Result<(), String> {
    crate::serve::existing_db(&crate::config::resolve_data_dir(opts, env))?;
    let store = crate::serve::open_store(opts, env)?;
    let e = |e: fleet_core::ipc_error::IpcError| e.message;
    let person = |name: &str, create: bool| -> Result<i64, String> {
        let name = fleet_core::store::validate_person_name(name).map_err(e)?;
        match store.get_person_by_name(&name).map_err(e)? {
            Some(p) => Ok(p.id),
            None if create => Ok(store.create_person(&name, None).map_err(e)?.id),
            None => Err(format!(
                "no live person named '{name}'; `fleet-hub person list` names them"
            )),
        }
    };
    match cmd {
        OrgCmd::Member { cmd } => match cmd {
            MemberCmd::List { org, json } => {
                let members =
                    fleet_core::service::org_admin::list_members(&store, *org).map_err(e)?;
                if *json {
                    out::line(&serde_json::to_string_pretty(&members).map_err(|e| e.to_string())?);
                } else if members.is_empty() {
                    out::line(&format!(
                        "org {org} has no members; add one with `fleet-hub org member add {org} <person>`"
                    ));
                } else {
                    let rows: Vec<Vec<String>> = members
                        .iter()
                        .map(|m| {
                            vec![
                                m.person_id.to_string(),
                                m.name.clone(),
                                m.role.clone(),
                                m.devices.join(", "),
                            ]
                        })
                        .collect();
                    out::line(&crate::pair::table(
                        &["ID", "PERSON", "ROLE", "DEVICES"],
                        &rows,
                    ));
                }
            }
            MemberCmd::Add {
                org,
                person: name,
                role,
            } => {
                let p = person(name, true)?;
                let m = store
                    .set_org_member(*org, p, role.as_str(), None)
                    .map_err(e)?;
                out::line(&format!(
                    "{name} is {} of org {org}; their devices are fenced to it from their next request",
                    m.role
                ));
            }
            MemberCmd::Rm {
                org,
                person: name,
                keep_grants,
            } => {
                let p = person(name, false)?;
                if !store.remove_org_member(*org, p).map_err(e)? {
                    return Err(format!("{name} is not a member of org {org}"));
                }
                let revoked = if *keep_grants {
                    0
                } else {
                    store.revoke_person_grants_in_org(p, *org).map_err(e)?
                };
                out::line(&format!(
                    "{name} left org {org}; {revoked} grant(s) on its sessions revoked; their own \
                     sessions are still theirs"
                ));
            }
            MemberCmd::Grants {
                org,
                person: name,
                narrow,
                revoke,
            } => {
                let p = person(name, false)?;
                if *revoke {
                    let n = store.revoke_person_grants_in_org(p, *org).map_err(e)?;
                    out::line(&format!(
                        "revoked {n} grant(s) to {name} on org {org}'s sessions"
                    ));
                } else if *narrow {
                    let n = store.narrow_person_grants_in_org(p, *org).map_err(e)?;
                    out::line(&format!("narrowed {n} grant(s) to {name} to watch"));
                } else {
                    let (watch, drive) = store.person_grants_in_org(p, *org).map_err(e)?;
                    out::line(&format!(
                        "{name} holds {watch} watch and {drive} drive grant(s) on org {org}'s sessions"
                    ));
                }
            }
        },
        OrgCmd::OwnHub { org, none } => {
            let org = match (org, none) {
                (Some(o), false) => Some(*o),
                (None, true) => None,
                _ => return Err("give the org's id, or --none".into()),
            };
            store.set_hub_owner_org(org).map_err(e)?;
            out::line(&match org {
                Some(o) => format!("org {o} owns this hub: its admins administer hosts"),
                None => "no company owns this hub: its owner administers hosts".into(),
            });
        }
        OrgCmd::UnclaimedCount { org, state } => {
            store
                .set_org_admins_see_unclaimed(*org, state.on())
                .map_err(e)?;
            out::line(&format!(
                "org {org}'s admins {} the unclaimed count on its hosts",
                if state.on() {
                    "now see"
                } else {
                    "no longer see"
                }
            ));
        }
        _ => unreachable!("only the direct subcommands come here"),
    }
    Ok(())
}

pub async fn run(
    cmd: OrgCmd,
    opts: &HubOptions,
    env: &HashMap<String, String>,
) -> Result<ExitCode, String> {
    if matches!(
        cmd,
        OrgCmd::Member { .. } | OrgCmd::OwnHub { .. } | OrgCmd::UnclaimedCount { .. }
    ) {
        run_direct(&cmd, opts, env)?;
        return Ok(ExitCode::SUCCESS);
    }
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
        OrgCmd::Member { .. } | OrgCmd::OwnHub { .. } | OrgCmd::UnclaimedCount { .. } => {}
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
            args(&["set", "2", "--auto-tidy", "inherit"]),
            json!({ "action": "update_org", "org_id": 2, "auto_tidy": "inherit" })
        );
        assert_eq!(
            args(&["set", "2", "--jev", "on"]),
            json!({ "action": "update_org", "org_id": 2, "jev": "on" })
        );
        assert_eq!(
            args(&["set", "2", "--jev", "off"]),
            json!({ "action": "update_org", "org_id": 2, "jev": "off" })
        );
        assert_eq!(
            args(&["set", "2", "--jev-reply", "on"]),
            json!({ "action": "update_org", "org_id": 2, "jev_reply": "on" })
        );
        assert_eq!(
            args(&["set", "2", "--bound-sees-unassigned", "off"]),
            json!({ "action": "update_org", "org_id": 2, "bound_sees_unassigned": false })
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
    fn the_member_commands_parse_and_never_reach_work_admin() {
        for a in [
            &["member", "list", "1"][..],
            &["member", "add", "1", "jane", "--role", "admin"],
            &["member", "rm", "1", "jane", "--keep-grants"],
            &["member", "grants", "1", "jane", "--narrow"],
            &["own-hub", "1"],
            &["own-hub", "--none"],
            &["unclaimed-count", "1", "on"],
        ] {
            let cmd = parse(a).unwrap_or_else(|e| panic!("{a:?}: {e}"));
            assert!(admin_args(&cmd).is_err(), "{a:?} is not a work_admin call");
        }
        assert!(parse(&["member", "add", "1", "jane", "--role", "owner"]).is_err());
        assert!(parse(&["member", "grants", "1", "jane", "--narrow", "--revoke"]).is_err());
        assert!(parse(&["own-hub", "1", "--none"]).is_err());
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
        assert!(!lines[0].contains("Jev"));
        let consenting = org_lines(&json!({ "id": 2, "name": "B", "jev_allowed": true }));
        assert!(
            consenting[0].contains("[sends to Jev]"),
            "{}",
            consenting[0]
        );
        assert!(lines[1].contains("#3 acme/*"));
        assert!(lines[1].contains("#4 acme/api · host: h"));
        assert!(lines[1].contains("#5 path: /w/acme"));
        assert!(lines[2].contains("hetzner-a"));
        assert!(lines[3].contains("acme (#2)"));
        assert!(
            !lines[0].contains("bound devices"),
            "on (or absent) is the default"
        );
        let own = org_lines(&json!({ "id": 2, "name": "B", "bound_sees_unassigned": false }));
        assert!(own[0].contains("[bound devices: own org only]"));
    }
}
