//! `fleet-hub realm …` — users, groups, roles, role mappings and the audit
//! log from the hub machine (identity sub-project 1, the realm model).
//!
//! **Written straight to `state.db`, not through a running hub**, for
//! `person.rs`'s reason: the operator's authority here is shell access to
//! the hub machine, and the hub honours a change from its next request. Each
//! subcommand builds a [`RealmAdminArgs`] and calls the very function the
//! `realm_admin` control-API tool calls, as [`Me::LOCAL`] — the fleet
//! authority with no device and no person, so the audit log records the
//! actor as none. The service records every write's admin event itself; this
//! module records nothing extra and holds no rule of its own (a managed
//! group refusing a role mapping, the last `fleet-admin`, an org fence, all
//! stay in one place).
//!
//! The service never returns a secret, so neither does `--json`.

use crate::config::HubOptions;
use crate::out;
use crate::pair::{fmt_time, table};
use clap::{Args, Subcommand};
use fleet_core::service::org_admin::Me;
use fleet_core::service::realm_admin::{self, RealmAdminArgs};
use fleet_core::store::Store;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::process::ExitCode;
use std::sync::Mutex;

/// Shared by every subcommand.
#[derive(Args, Debug)]
pub struct Fmt {
    /// Print the service's answer as JSON instead of a table.
    #[arg(long)]
    pub json: bool,
}

#[derive(Subcommand, Debug)]
pub enum RealmCmd {
    /// People: list, show, update a profile, enable, disable.
    User {
        #[command(subcommand)]
        cmd: UserCmd,
    },
    /// Groups, by path (`/orgs/acme/members`): list, show, create, rename,
    /// move, delete, and their members.
    Group {
        #[command(subcommand)]
        cmd: GroupCmd,
    },
    /// Roles, by reference (`fleet-admin`, or `<client>:<name>` such as
    /// `fleet:org-admin`): list, create, delete, composites and mappings.
    Role {
        #[command(subcommand)]
        cmd: RoleCmd,
    },
    /// A person's effective roles, and through what each one comes.
    Effective {
        /// The person, by name.
        user: String,
        #[command(flatten)]
        fmt: Fmt,
    },
    /// The admin audit log, newest first.
    Audit {
        /// Only events from this Unix second on.
        #[arg(long)]
        since: Option<i64>,
        /// At most this many rows (default 100, at most 500).
        #[arg(long)]
        limit: Option<u32>,
        #[command(flatten)]
        fmt: Fmt,
    },
}

#[derive(Subcommand, Debug)]
pub enum UserCmd {
    /// Every person, with email, groups and roles.
    List {
        #[command(flatten)]
        fmt: Fmt,
    },
    /// One person in full.
    Show {
        user: String,
        #[command(flatten)]
        fmt: Fmt,
    },
    /// Change a profile; a flag left out leaves that field as it is.
    Update {
        user: String,
        #[arg(long)]
        email: Option<String>,
        #[arg(long)]
        first_name: Option<String>,
        #[arg(long)]
        last_name: Option<String>,
        #[arg(long)]
        email_verified: Option<bool>,
        #[command(flatten)]
        fmt: Fmt,
    },
    /// Lift a disable (nothing it revoked comes back).
    Enable {
        user: String,
        #[command(flatten)]
        fmt: Fmt,
    },
    /// End a person's reach (their devices and the shares made to them).
    Disable {
        user: String,
        #[command(flatten)]
        fmt: Fmt,
    },
}

#[derive(Subcommand, Debug)]
pub enum GroupCmd {
    /// Every group, with members and roles.
    List {
        #[command(flatten)]
        fmt: Fmt,
    },
    /// One group in full.
    Show {
        path: String,
        #[command(flatten)]
        fmt: Fmt,
    },
    /// Create a group under `--parent` (top level when absent).
    Create {
        name: String,
        #[arg(long)]
        parent: Option<String>,
        #[command(flatten)]
        fmt: Fmt,
    },
    /// Give a group a new name, keeping its parent.
    Rename {
        path: String,
        name: String,
        #[command(flatten)]
        fmt: Fmt,
    },
    /// Move a group under `--parent` (top level when absent).
    Move {
        path: String,
        #[arg(long)]
        parent: Option<String>,
        #[command(flatten)]
        fmt: Fmt,
    },
    /// Delete a group.
    Delete {
        path: String,
        #[command(flatten)]
        fmt: Fmt,
    },
    /// Add a person to a group.
    Add {
        path: String,
        user: String,
        #[command(flatten)]
        fmt: Fmt,
    },
    /// Remove a person from a group.
    Remove {
        path: String,
        user: String,
        #[command(flatten)]
        fmt: Fmt,
    },
}

#[derive(Subcommand, Debug)]
pub enum RoleCmd {
    /// Every role, with its composites.
    List {
        #[command(flatten)]
        fmt: Fmt,
    },
    /// Create a role: a realm role, or a client's with `--client`.
    Create {
        name: String,
        #[arg(long)]
        client: Option<String>,
        #[arg(long)]
        description: Option<String>,
        #[command(flatten)]
        fmt: Fmt,
    },
    /// Delete a role.
    Delete {
        /// `fleet-admin` or `<client>:<name>`.
        role: String,
        #[command(flatten)]
        fmt: Fmt,
    },
    /// Make `parent` include `child`.
    Compose {
        parent: String,
        child: String,
        #[command(flatten)]
        fmt: Fmt,
    },
    /// Stop `parent` including `child`.
    Decompose {
        parent: String,
        child: String,
        #[command(flatten)]
        fmt: Fmt,
    },
    /// Map a role to exactly one of a person or a group.
    Map {
        role: String,
        #[command(flatten)]
        to: Target,
        #[command(flatten)]
        fmt: Fmt,
    },
    /// Undo a mapping.
    Unmap {
        role: String,
        #[command(flatten)]
        to: Target,
        #[command(flatten)]
        fmt: Fmt,
    },
}

/// Who a role is mapped to: exactly one of the two.
#[derive(Args, Debug)]
#[group(required = true, multiple = false)]
pub struct Target {
    /// A person, by name.
    #[arg(long)]
    pub user: Option<String>,
    /// A group, by path.
    #[arg(long)]
    pub group: Option<String>,
}

fn args(action: &str) -> RealmAdminArgs {
    RealmAdminArgs {
        action: action.to_string(),
        ..Default::default()
    }
}

/// What a subcommand asks of the service, and whether it wants JSON.
fn request(cmd: RealmCmd) -> (RealmAdminArgs, bool) {
    match cmd {
        RealmCmd::User { cmd } => match cmd {
            UserCmd::List { fmt } => (args("list_users"), fmt.json),
            UserCmd::Show { user, fmt } => (
                RealmAdminArgs {
                    user: Some(user),
                    ..args("get_user")
                },
                fmt.json,
            ),
            UserCmd::Update {
                user,
                email,
                first_name,
                last_name,
                email_verified,
                fmt,
            } => {
                let mut p = serde_json::Map::new();
                if let Some(v) = email {
                    p.insert("email".into(), json!(v));
                }
                if let Some(v) = first_name {
                    p.insert("first_name".into(), json!(v));
                }
                if let Some(v) = last_name {
                    p.insert("last_name".into(), json!(v));
                }
                if let Some(v) = email_verified {
                    p.insert("email_verified".into(), json!(v));
                }
                (
                    RealmAdminArgs {
                        user: Some(user),
                        profile: Some(Value::Object(p)),
                        ..args("update_user")
                    },
                    fmt.json,
                )
            }
            UserCmd::Enable { user, fmt } => (
                RealmAdminArgs {
                    user: Some(user),
                    ..args("enable_user")
                },
                fmt.json,
            ),
            UserCmd::Disable { user, fmt } => (
                RealmAdminArgs {
                    user: Some(user),
                    ..args("disable_user")
                },
                fmt.json,
            ),
        },
        RealmCmd::Group { cmd } => match cmd {
            GroupCmd::List { fmt } => (args("list_groups"), fmt.json),
            GroupCmd::Show { path, fmt } => (
                RealmAdminArgs {
                    group: Some(path),
                    ..args("get_group")
                },
                fmt.json,
            ),
            GroupCmd::Create { name, parent, fmt } => (
                RealmAdminArgs {
                    name: Some(name),
                    parent,
                    ..args("create_group")
                },
                fmt.json,
            ),
            GroupCmd::Rename { path, name, fmt } => (
                RealmAdminArgs {
                    group: Some(path),
                    name: Some(name),
                    ..args("rename_group")
                },
                fmt.json,
            ),
            GroupCmd::Move { path, parent, fmt } => (
                RealmAdminArgs {
                    group: Some(path),
                    // Absent parent = top level, which the service reads
                    // from "".
                    parent: Some(parent.unwrap_or_default()),
                    ..args("move_group")
                },
                fmt.json,
            ),
            GroupCmd::Delete { path, fmt } => (
                RealmAdminArgs {
                    group: Some(path),
                    ..args("delete_group")
                },
                fmt.json,
            ),
            GroupCmd::Add { path, user, fmt } => (
                RealmAdminArgs {
                    group: Some(path),
                    user: Some(user),
                    ..args("add_member")
                },
                fmt.json,
            ),
            GroupCmd::Remove { path, user, fmt } => (
                RealmAdminArgs {
                    group: Some(path),
                    user: Some(user),
                    ..args("remove_member")
                },
                fmt.json,
            ),
        },
        RealmCmd::Role { cmd } => match cmd {
            RoleCmd::List { fmt } => (args("list_roles"), fmt.json),
            RoleCmd::Create {
                name,
                client,
                description,
                fmt,
            } => (
                RealmAdminArgs {
                    name: Some(name),
                    client,
                    description,
                    ..args("create_role")
                },
                fmt.json,
            ),
            RoleCmd::Delete { role, fmt } => (
                RealmAdminArgs {
                    role: Some(role),
                    ..args("delete_role")
                },
                fmt.json,
            ),
            RoleCmd::Compose { parent, child, fmt } => (
                RealmAdminArgs {
                    role: Some(parent),
                    child: Some(child),
                    ..args("add_composite")
                },
                fmt.json,
            ),
            RoleCmd::Decompose { parent, child, fmt } => (
                RealmAdminArgs {
                    role: Some(parent),
                    child: Some(child),
                    ..args("remove_composite")
                },
                fmt.json,
            ),
            RoleCmd::Map { role, to, fmt } => (
                RealmAdminArgs {
                    role: Some(role),
                    user: to.user,
                    group: to.group,
                    ..args("map_role")
                },
                fmt.json,
            ),
            RoleCmd::Unmap { role, to, fmt } => (
                RealmAdminArgs {
                    role: Some(role),
                    user: to.user,
                    group: to.group,
                    ..args("unmap_role")
                },
                fmt.json,
            ),
        },
        RealmCmd::Effective { user, fmt } => (
            RealmAdminArgs {
                user: Some(user),
                ..args("effective_roles")
            },
            fmt.json,
        ),
        RealmCmd::Audit { since, limit, fmt } => (
            RealmAdminArgs {
                since,
                limit,
                ..args("audit")
            },
            fmt.json,
        ),
    }
}

fn s(v: &Value, k: &str) -> String {
    match &v[k] {
        Value::Null => "-".to_string(),
        Value::String(x) if x.is_empty() => "-".to_string(),
        Value::String(x) => x.clone(),
        other => other.to_string(),
    }
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array()
        .map(|a| {
            a.iter()
                .map(|x| {
                    x.as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| x.to_string())
                })
                .collect()
        })
        .unwrap_or_default()
}

fn list_or_dash(v: Vec<String>) -> String {
    if v.is_empty() {
        "-".to_string()
    } else {
        v.join(", ")
    }
}

/// `client:name`, or `name` for a realm role.
fn role_ref(v: &Value) -> String {
    match v["client"].as_str() {
        Some(c) => format!("{c}:{}", s(v, "name")),
        None => s(v, "name"),
    }
}

/// The table for a read; `None` for a write (its answer prints as JSON).
fn render(action: &str, v: &Value) -> Option<String> {
    let rows = v.as_array()?;
    Some(match action {
        "list_users" => {
            let r: Vec<Vec<String>> = rows
                .iter()
                .map(|u| {
                    let roles: Vec<String> = u["roles"]
                        .as_array()
                        .map(|a| a.iter().map(role_ref).collect())
                        .unwrap_or_default();
                    vec![
                        s(u, "name"),
                        s(u, "uuid"),
                        s(u, "email"),
                        if u["enabled"].as_bool() == Some(true) {
                            "yes"
                        } else {
                            "no"
                        }
                        .to_string(),
                        list_or_dash(strings(&u["groups"])),
                        list_or_dash(roles),
                    ]
                })
                .collect();
            table(&["NAME", "UUID", "EMAIL", "ENABLED", "GROUPS", "ROLES"], &r)
        }
        "list_groups" => {
            let r: Vec<Vec<String>> = rows
                .iter()
                .map(|g| {
                    vec![
                        s(g, "path"),
                        if g["managed"].as_bool() == Some(true) {
                            "yes"
                        } else {
                            "no"
                        }
                        .to_string(),
                        list_or_dash(strings(&g["members"])),
                        list_or_dash(strings(&g["roles"])),
                    ]
                })
                .collect();
            table(&["PATH", "MANAGED", "MEMBERS", "ROLES"], &r)
        }
        "list_roles" => {
            let r: Vec<Vec<String>> = rows
                .iter()
                .map(|x| {
                    vec![
                        role_ref(x),
                        s(x, "description"),
                        if x["seeded"].as_bool() == Some(true) {
                            "yes"
                        } else {
                            "no"
                        }
                        .to_string(),
                    ]
                })
                .collect();
            table(&["ROLE", "DESCRIPTION", "SEEDED"], &r)
        }
        "effective_roles" => {
            let r: Vec<Vec<String>> = rows
                .iter()
                .map(|x| vec![role_ref(x), list_or_dash(strings(&x["via"]))])
                .collect();
            table(&["ROLE", "VIA"], &r)
        }
        "audit" => {
            let r: Vec<Vec<String>> = rows
                .iter()
                .map(|e| {
                    vec![
                        fmt_time(e["ts"].as_i64()),
                        e["actor_person"]
                            .as_i64()
                            .map_or("-".to_string(), |p| p.to_string()),
                        s(e, "action"),
                        s(e, "resource_type"),
                        s(e, "resource_id"),
                    ]
                })
                .collect();
            table(&["TIME", "ACTOR", "ACTION", "RESOURCE", "ID"], &r)
        }
        _ => return None,
    })
}

/// Run one request against `store` as the fleet authority and print it.
fn execute(store: Store, cmd: RealmCmd) -> Result<ExitCode, String> {
    let (a, json) = request(cmd);
    let store = Mutex::new(store);
    let v = realm_admin::run(&a, &store, Me::LOCAL).map_err(|e| e.message)?;
    print(&a.action, &v, json)?;
    Ok(ExitCode::SUCCESS)
}

fn print(action: &str, v: &Value, json: bool) -> Result<(), String> {
    if !json {
        if let Some(t) = render(action, v) {
            out::line(&t);
            return Ok(());
        }
    }
    out::line(&serde_json::to_string_pretty(v).map_err(|e| e.to_string())?);
    Ok(())
}

pub fn run(
    cmd: RealmCmd,
    opts: &HubOptions,
    env: &HashMap<String, String>,
) -> Result<ExitCode, String> {
    crate::serve::existing_db(&crate::config::resolve_data_dir(opts, env))?;
    let store = crate::serve::open_store(opts, env)?;
    execute(store, cmd)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct T {
        #[command(subcommand)]
        cmd: RealmCmd,
    }

    fn parse(argv: &[&str]) -> RealmCmd {
        let mut v = vec!["t"];
        v.extend_from_slice(argv);
        T::try_parse_from(v)
            .unwrap_or_else(|e| panic!("{argv:?}: {e}"))
            .cmd
    }

    #[test]
    fn role_map_takes_exactly_one_target() {
        assert!(T::try_parse_from(["t", "role", "map", "fleet-admin"]).is_err());
        assert!(T::try_parse_from([
            "t",
            "role",
            "map",
            "fleet-admin",
            "--user",
            "a",
            "--group",
            "/g"
        ])
        .is_err());
    }

    #[test]
    fn update_builds_a_profile_of_only_the_flags_given() {
        let (a, _) = request(parse(&["user", "update", "ada", "--email", "a@x.io"]));
        assert_eq!(a.action, "update_user");
        assert_eq!(a.profile, Some(json!({ "email": "a@x.io" })));
    }

    #[test]
    fn a_group_created_by_the_cli_shows_in_the_audit_log_and_effective_roles() {
        let dir = tempfile::tempdir().expect("tempdir");
        let opts = HubOptions {
            data_dir: Some(dir.path().to_path_buf()),
            ..HubOptions::default()
        };
        let store = crate::serve::open_store(&opts, &HashMap::new()).expect("store");
        let owner = store.create_person("ada", None).expect("person");
        let me = Mutex::new(store);
        let call = |argv: &[&str]| {
            let (a, _) = request(parse(argv));
            realm_admin::run(&a, &me, Me::LOCAL).map_err(|e| e.message)
        };
        call(&["group", "create", "eng"]).expect("create");
        call(&["group", "add", "/eng", "ada"]).expect("add");
        call(&["role", "map", "fleet-admin", "--group", "/eng"]).expect("map");
        let eff = call(&["effective", "ada"]).expect("effective");
        let table = render("effective_roles", &eff).expect("table");
        assert!(table.contains("fleet-admin"), "{table}");
        let log = call(&["audit"]).expect("audit");
        let t = render("audit", &log).expect("table");
        assert!(t.contains("create_group") && t.contains("map_role"), "{t}");
        let _ = owner;
    }
}
