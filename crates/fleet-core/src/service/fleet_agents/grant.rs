//! Granting a fleet agent (spec *F. Configuring an instance*): a rule with
//! `agent = fleet`, its `definition`, the instance's input values (the
//! rule's `config`) and its store `bindings`.
//!
//! [`grant`] is the whole act, in the order the locks allow:
//! 1. [`Sources::from_catalog`]: the definition and the MCP servers the
//!    bindings name, out of the loaded catalog (the catalog registry's lock
//!    is never held with the store's);
//! 2. the secrets the rule's host sees (`catalog::sync::secrets::resolve`);
//! 3. [`prepare`], under the store lock: the instance name, the level and
//!    hours, the host and project, the input values against the
//!    definition's form and fleet's own references, the bindings against
//!    the definition and the catalog, and each MCP binding rendered;
//! 4. [`check_stores`], no lock held: each MCP binding started on the host
//!    and asked for its tools ([`super::probe`]);
//! 5. the rule written.
//!
//! A store that is not there, or lacks a tool the definition uses, fails
//! the grant with the reason.

use super::bindings::{self, Binding};
use super::definition::{InputType, Loaded, StoreKind, StoreRole};
use super::probe;
use crate::ipc_error::{codes, lock, IpcError};
use crate::pages::forms;
use crate::service::agents::admin::{AgentsArgs, AgentsChanged, MAX_RULE_HOURS};
use crate::service::catalog::model::{AssetSpec, Kind};
use crate::store::{NewAgentRule, Store, AGENT_CONFIG_MAX_CHARS, AGENT_LEVELS};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::sync::Mutex;

fn invalid(msg: impl Into<String>) -> IpcError {
    IpcError::new(codes::E_VALIDATE, msg.into())
}

fn db(e: rusqlite::Error) -> IpcError {
    IpcError::new(codes::E_INTERNAL, e.to_string())
}

/// What a grant reads from the catalog.
#[derive(Debug, Clone, PartialEq)]
pub struct Sources {
    pub definition: Loaded,
    /// The `mcp_server` assets the bindings name, by asset name.
    pub servers: BTreeMap<String, AssetSpec>,
}

impl Sources {
    /// The definition `name` and the servers `bindings` name, from the
    /// personal catalog. An invalid definition is refused with its problems.
    pub fn from_catalog(name: &str, bindings: &[Binding]) -> Result<Sources, IpcError> {
        use crate::service::catalog::CatalogTarget;
        CatalogTarget::Personal.with(|cat| {
            let asset = cat.find(Kind::FleetAgent, name).ok_or_else(|| {
                IpcError::new(
                    codes::E_NOTFOUND,
                    format!("the catalog has no fleet agent {name:?}"),
                )
            })?;
            let definition = asset.fleet_agent().map_err(|p| {
                invalid(format!("fleet agent {name} is not valid: {}", p.join("; ")))
            })?;
            let servers = bindings
                .iter()
                .filter_map(|b| b.server.as_deref())
                .filter_map(|s| {
                    cat.find(Kind::McpServer, s)
                        .map(|a| (s.to_string(), a.spec.clone()))
                })
                .collect();
            Ok(Sources {
                definition,
                servers,
            })
        })
    }
}

/// One MCP binding to check on the host before the rule is written.
#[derive(Clone, PartialEq)]
pub struct StoreProbe {
    pub role: StoreRole,
    pub binding: String,
    /// The rendered entry: secrets in it.
    pub entry: Value,
}

impl std::fmt::Debug for StoreProbe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoreProbe")
            .field("role", &self.role.role)
            .field("binding", &self.binding)
            .finish_non_exhaustive()
    }
}

/// A checked grant, not yet written.
#[derive(Debug, Clone, PartialEq)]
pub struct Prepared {
    pub rule: NewAgentRule,
    pub probes: Vec<StoreProbe>,
}

/// PURE: an instance name (the rule's scope): `[a-z0-9][a-z0-9-]*`, ≤ 60.
pub fn instance_ok(s: &str) -> bool {
    let mut c = s.chars();
    matches!(c.next(), Some(f) if f.is_ascii_lowercase() || f.is_ascii_digit())
        && s.len() <= 60
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// The bindings of a grant's arguments.
pub fn parse_bindings(args: &AgentsArgs) -> Result<Vec<Binding>, IpcError> {
    match &args.bindings {
        None => Ok(Vec::new()),
        Some(v) => serde_json::from_value(v.clone()).map_err(|e| invalid(format!("bindings: {e}"))),
    }
}

/// Check the instance's input values: an input left out takes its default
/// (`value`), then the definition's form, then the references (a tracker
/// id, a project `owner/repo`, a host) against this hub. Answers the
/// accepted values.
pub fn check_values(
    s: &Store,
    loaded: &Loaded,
    values: &Map<String, Value>,
) -> Result<Map<String, Value>, IpcError> {
    let mut values = values.clone();
    for i in &loaded.definition.inputs {
        if let Some(d) = &i.value {
            values.entry(i.name.clone()).or_insert_with(|| d.clone());
        }
    }
    let accepted = match super::definition::inputs_form(&loaded.definition) {
        None if values.is_empty() => Map::new(),
        None => return Err(invalid("this agent takes no inputs")),
        Some(form) => {
            forms::check_answers(&form, &values)
                .map_err(|ps| {
                    invalid(
                        ps.iter()
                            .map(|p| format!("inputs › {}: {}", p.field, p.problem))
                            .collect::<Vec<_>>()
                            .join("; "),
                    )
                })?
                .values
        }
    };
    for i in &loaded.definition.inputs {
        let Some(v) = accepted.get(&i.name).and_then(Value::as_str) else {
            continue;
        };
        let v = v.trim();
        let at = format!("inputs › {}", i.name);
        match i.kind {
            InputType::Tracker => {
                let id: i64 = v
                    .parse()
                    .map_err(|_| invalid(format!("{at}: a tracker is its id")))?;
                if s.get_tracker(id)?.is_none() {
                    return Err(IpcError::new(
                        codes::E_NOTFOUND,
                        format!("{at}: no tracker {id}"),
                    ));
                }
            }
            InputType::Project => {
                let (o, r) = v
                    .split_once('/')
                    .ok_or_else(|| invalid(format!("{at}: a project is owner/repo")))?;
                if !s
                    .list_projects()
                    .map_err(db)?
                    .iter()
                    .any(|p| p.owner == o && p.repo == r)
                {
                    return Err(IpcError::new(
                        codes::E_NOTFOUND,
                        format!("{at}: no project {v}"),
                    ));
                }
            }
            InputType::Host if s.get_host_row(v).map_err(db)?.is_none() => {
                return Err(IpcError::new(
                    codes::E_NOTFOUND,
                    format!("{at}: no host {v}"),
                ));
            }
            _ => {}
        }
    }
    Ok(accepted)
}

/// Check a fleet agent's grant and render its MCP bindings with `secrets`
/// (the host's). Writes nothing.
pub fn prepare(
    s: &Store,
    args: &AgentsArgs,
    sources: &Sources,
    secrets: &BTreeMap<String, String>,
    granted_by: &str,
    now: i64,
) -> Result<Prepared, IpcError> {
    let def_name = args
        .definition
        .as_deref()
        .ok_or_else(|| invalid("a fleet grant names its definition"))?;
    if def_name != sources.definition.name {
        return Err(invalid("the definition read is not the one named"));
    }
    let instance = args
        .scope
        .as_deref()
        .map(str::trim)
        .ok_or_else(|| invalid("a fleet grant names its instance (scope)"))?;
    if !instance_ok(instance) {
        return Err(invalid(format!(
            "instance {instance:?} is [a-z0-9][a-z0-9-]*, at most 60"
        )));
    }
    let level = args.level.as_deref().unwrap_or("watch");
    if !AGENT_LEVELS.contains(&level) {
        return Err(invalid(format!(
            "level must be one of {}",
            AGENT_LEVELS.join(", ")
        )));
    }
    if let Some(h) = args.hours {
        if !(1..=MAX_RULE_HOURS).contains(&h) {
            return Err(invalid(format!("hours must be 1-{MAX_RULE_HOURS}")));
        }
    }
    let host = args
        .host
        .as_deref()
        .map(str::trim)
        .filter(|h| !h.is_empty())
        .ok_or_else(|| invalid("grant needs host"))?;
    if s.get_host_row(host).map_err(db)?.is_none() {
        return Err(IpcError::new(codes::E_NOTFOUND, format!("no host {host}")));
    }
    if let Some(pid) = args.project_id {
        if !s.list_projects().map_err(db)?.iter().any(|p| p.id == pid) {
            return Err(IpcError::new(
                codes::E_NOTFOUND,
                format!("no project {pid}"),
            ));
        }
    }
    let loaded = &sources.definition;
    if loaded.definition.run.workspace.needs_project() && args.project_id.is_none() {
        return Err(invalid(
            "this agent runs in a project's checkout: give project_id",
        ));
    }
    let values = match &args.config {
        None => Map::new(),
        Some(Value::Object(m)) => m.clone(),
        Some(_) => return Err(invalid("config (the input values) must be a JSON object")),
    };
    let values = check_values(s, loaded, &values)?;
    let config = Value::Object(values).to_string();
    if config.chars().count() > AGENT_CONFIG_MAX_CHARS {
        return Err(invalid(format!(
            "the input values must be at most {AGENT_CONFIG_MAX_CHARS} characters"
        )));
    }
    let binds = parse_bindings(args)?;
    let lookup = |name: &str| sources.servers.get(name).cloned();
    let problems = bindings::check(&loaded.definition, &binds, &lookup);
    if !problems.is_empty() {
        return Err(invalid(problems.join("; ")));
    }
    let mut probes = Vec::new();
    for b in &binds {
        let role = loaded
            .definition
            .stores
            .iter()
            .find(|r| r.role == b.role)
            .cloned()
            .ok_or_else(|| invalid(format!("no store role {}", b.role)))?;
        if role.kind != StoreKind::Mcp {
            continue;
        }
        let spec = lookup(b.server.as_deref().unwrap_or_default())
            .ok_or_else(|| invalid(format!("bindings › {}: no server", b.role)))?;
        let entry = bindings::render_entry(&spec, &b.env, secrets)
            .map_err(|e| invalid(format!("bindings › {}: {e} on {host}", b.role)))?;
        probes.push(StoreProbe {
            role,
            binding: b.server_name().to_string(),
            entry,
        });
    }
    Ok(Prepared {
        rule: NewAgentRule {
            agent: "fleet".into(),
            scope: instance.to_string(),
            level: level.to_string(),
            host_alias: host.to_string(),
            project_id: args.project_id,
            config,
            granted_by: granted_by.to_string(),
            granted_at: now,
            expires_at: args.hours.map(|h| now + h * 3600),
            definition: Some(def_name.to_string()),
            bindings: serde_json::to_string(&binds).unwrap_or_else(|_| "[]".into()),
        },
        probes,
    })
}

/// PURE: what a store's answer means for the grant: `None` when it lists
/// every tool the role uses.
pub fn probe_problem(p: &StoreProbe, listed: Result<Vec<String>, String>) -> Option<String> {
    let at = format!("bindings › {} ({})", p.role.role, p.binding);
    match listed {
        Err(e) => Some(format!("{at}: the server could not be asked: {e}")),
        Ok(tools) => {
            let missing = probe::missing_tools(&p.role, &tools);
            (!missing.is_empty()).then(|| format!("{at}: the server has no {}", missing.join(", ")))
        }
    }
}

/// Ask each MCP binding for its tools on `host`.
pub async fn check_stores(
    ssh: &crate::ssh::SshClient,
    host: &str,
    probes: &[StoreProbe],
) -> Result<(), IpcError> {
    let mut problems = Vec::new();
    for p in probes {
        let listed = probe::list_tools(ssh, host, &p.entry).await;
        problems.extend(probe_problem(p, listed));
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(invalid(problems.join("; ")))
    }
}

/// [`grant`], from the grant form's `answers` when the arguments carry
/// them (`form::answers_to_args`), else from the arguments themselves.
pub async fn grant_or_answers(
    store: &Mutex<Store>,
    ssh: &crate::ssh::SshClient,
    args: &AgentsArgs,
    granted_by: &str,
    now: i64,
) -> Result<AgentsChanged, IpcError> {
    let Some(answers) = &args.answers else {
        return grant(store, ssh, args, granted_by, now).await;
    };
    let answers = answers
        .as_object()
        .ok_or_else(|| invalid("answers must be an object"))?;
    let def_name = args
        .definition
        .as_deref()
        .ok_or_else(|| invalid("a fleet grant names its definition"))?;
    let sources = Sources::from_catalog(def_name, &[])?;
    let ctx = super::form::FormContext::read(store)?;
    let from_form =
        super::form::answers_to_args(&sources.definition, &ctx, answers).map_err(invalid)?;
    grant(store, ssh, &from_form, granted_by, now).await
}

/// The whole grant: read, check, ask the stores, write. `granted_by` names
/// the device or the console.
pub async fn grant(
    store: &Mutex<Store>,
    ssh: &crate::ssh::SshClient,
    args: &AgentsArgs,
    granted_by: &str,
    now: i64,
) -> Result<AgentsChanged, IpcError> {
    let def_name = args
        .definition
        .as_deref()
        .ok_or_else(|| invalid("a fleet grant names its definition"))?;
    let binds = parse_bindings(args)?;
    let sources = Sources::from_catalog(def_name, &binds)?;
    let host = args.host.as_deref().map(str::trim).unwrap_or_default();
    let secrets = crate::service::catalog::sync::secrets::resolve(store, host)?;
    let prepared = {
        let s = lock(store)?;
        prepare(&s, args, &sources, &secrets, granted_by, now)?
    };
    check_stores(ssh, &prepared.rule.host_alias, &prepared.probes).await?;
    let id = lock(store)?.grant_agent_rule(&prepared.rule).map_err(db)?;
    Ok(AgentsChanged {
        changed: 1,
        id: Some(id),
    })
}
