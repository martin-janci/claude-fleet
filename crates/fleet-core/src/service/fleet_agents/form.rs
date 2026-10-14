//! The grant form (spec step 5, *F. Configuring an instance*): a
//! definition's instance as a `fleet.form/1` form, and its answers as the
//! grant.
//!
//! [`grant_form`] is PURE over a definition and what fleet has
//! ([`FormContext`]: hosts, projects, trackers, catalog MCP servers and the
//! shared fleet namespaces in use). Its steps:
//! 1. *What*: the instance's name and the definition's inputs (a `tracker`,
//!    `project` or `host` input is a select of what fleet has);
//! 2. *Where*: the host and, for a definition that runs in a project, the
//!    project;
//! 3. *Stores*: each role a select of the catalog's MCP servers (with an
//!    optional `NAME=${SECRET}` override per line) or of the fleet
//!    namespaces (`rule`, or a shared one; another is typed);
//! 4. *How much*: the level and the hours, under a line naming what the
//!    agent may write.
//!
//! The form's own fields are named `f_…` (an input may not be). Secrets are
//! not asked here: an override names a secret already in the hub's secret
//! store (Assets › Secrets), so no answer carries a value.
//!
//! [`answers_to_args`] turns the answers into the `agents` tool's grant,
//! which [`super::grant::grant`] checks and writes like any other. The same
//! spec is what the desktop's *New instance* / *Edit* dialog shows, and what
//! the phone and a chat card can render.

use super::bindings::Binding;
use super::definition::{ActionKind, InputType, Loaded, StoreKind, Workspace};
use crate::pages::forms::{self, FieldType, FormField, FormOption, FormSpec, FormStep};
use crate::service::agents::admin::{AgentsArgs, MAX_RULE_HOURS};
use crate::store::AgentRuleRow;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

pub const F_INSTANCE: &str = "f_instance";
pub const F_HOST: &str = "f_host";
pub const F_PROJECT: &str = "f_project";
pub const F_LEVEL: &str = "f_level";
pub const F_HOURS: &str = "f_hours";

fn f_store(role: &str) -> String {
    format!("f_store_{role}")
}
fn f_env(role: &str) -> String {
    format!("f_env_{role}")
}

/// What fleet has to offer the form.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FormContext {
    pub hosts: Vec<String>,
    /// `(id, "owner/repo")`.
    pub projects: Vec<(i64, String)>,
    /// `(id, name)`.
    pub trackers: Vec<(i64, String)>,
    /// The catalog's MCP servers, by name.
    pub servers: Vec<String>,
    /// `shared:<name>` namespaces some rule binds.
    pub shared: Vec<String>,
}

impl FormContext {
    /// From the catalog (registry lock only) and then the store.
    pub fn read(
        store: &std::sync::Mutex<crate::store::Store>,
    ) -> Result<FormContext, crate::ipc_error::IpcError> {
        use crate::service::catalog::{model::Kind, CatalogTarget};
        let servers: Vec<String> = CatalogTarget::Personal
            .with(|cat| {
                Ok(cat
                    .assets
                    .iter()
                    .filter(|a| a.kind() == Kind::McpServer)
                    .map(|a| a.header.name.clone())
                    .collect())
            })
            .unwrap_or_default();
        let s = crate::ipc_error::lock(store)?;
        let db = |e: rusqlite::Error| {
            crate::ipc_error::IpcError::new(crate::ipc_error::codes::E_INTERNAL, e.to_string())
        };
        let mut shared: Vec<String> = Vec::new();
        for r in s.list_agent_rules().map_err(db)? {
            for b in super::bindings::parse(&r.bindings).unwrap_or_default() {
                if let Some(ns) = b.namespace.filter(|n| n.starts_with("shared:")) {
                    if !shared.contains(&ns) {
                        shared.push(ns);
                    }
                }
            }
        }
        shared.sort();
        Ok(FormContext {
            hosts: s
                .list_hosts()
                .map_err(db)?
                .into_iter()
                .map(|h| h.alias)
                .collect(),
            projects: s
                .list_projects()
                .map_err(db)?
                .into_iter()
                .map(|p| (p.id, format!("{}/{}", p.owner, p.repo)))
                .collect(),
            trackers: s
                .list_trackers()?
                .into_iter()
                .map(|t| (t.id, t.name))
                .collect(),
            servers,
            shared,
        })
    }
}

fn pair(v: impl Into<String>, l: impl Into<String>) -> FormOption {
    FormOption::Pair(v.into(), l.into())
}

fn field(name: &str, kind: FieldType, label: &str) -> FormField {
    FormField {
        name: name.into(),
        kind,
        label: label.into(),
        help: None,
        required: false,
        value: None,
        when: None,
        placeholder: None,
        max_len: None,
        min: None,
        max: None,
        integer: false,
        options: None,
        other: false,
        disabled_reason: None,
        drafted: None,
        secret_note: None,
        env_target: None,
    }
}

fn step(title: &str, fields: Vec<FormField>) -> FormStep {
    FormStep {
        title: title.into(),
        name: None,
        kind: Default::default(),
        intro: None,
        when: None,
        fields,
    }
}

/// PURE: what the agent may write, in words, for the last step.
pub fn writes_line(loaded: &Loaded) -> String {
    let d = &loaded.definition;
    let mut parts: Vec<String> = Vec::new();
    for a in &d.actions {
        let what = match a.kind {
            ActionKind::McpCall => format!(
                "calls {}.{}",
                a.store.as_deref().unwrap_or_default(),
                a.tool.as_deref().unwrap_or_default()
            ),
            k => k.word().replace('_', " "),
        };
        let unattended = a.auto
            && (a.kind != ActionKind::McpCall
                || d.stores.iter().any(|s| {
                    Some(&s.role) == a.store.as_ref()
                        && s.write_tools
                            .iter()
                            .any(|w| Some(w.name()) == a.tool.as_deref() && w.auto_ok())
                }));
        parts.push(if unattended {
            format!("{what} (unattended at act)")
        } else {
            what
        });
    }
    if parts.is_empty() {
        "This agent writes nothing: its answers stay in fleet.".into()
    } else {
        format!(
            "It may write: {}. At watch nothing is sent; at propose each write waits for you; at act the unattended ones are sent.",
            parts.join(", ")
        )
    }
}

/// The current values of `rule` (an instance being edited), by field.
fn current(rule: Option<&AgentRuleRow>, ctx: &FormContext) -> Map<String, Value> {
    let mut out = Map::new();
    let Some(r) = rule else { return out };
    if let Ok(Value::Object(m)) = serde_json::from_str::<Value>(&r.config) {
        out.extend(m);
    }
    out.insert(F_INSTANCE.into(), r.scope.clone().into());
    out.insert(F_HOST.into(), r.host_alias.clone().into());
    if let Some(p) = r
        .project_id
        .and_then(|id| ctx.projects.iter().find(|(pid, _)| *pid == id))
    {
        out.insert(F_PROJECT.into(), p.1.clone().into());
    }
    out.insert(F_LEVEL.into(), r.level.clone().into());
    for b in super::bindings::parse(&r.bindings).unwrap_or_default() {
        if let Some(s) = &b.server {
            out.insert(f_store(&b.role), s.clone().into());
            let env: Vec<String> = b.env.iter().map(|(k, v)| format!("{k}={v}")).collect();
            if !env.is_empty() {
                out.insert(f_env(&b.role), env.join("\n").into());
            }
        }
        if let Some(ns) = &b.namespace {
            out.insert(f_store(&b.role), ns.clone().into());
        }
    }
    out
}

/// PURE: the grant form of `loaded`; `rule` prefills it (Edit).
pub fn grant_form(loaded: &Loaded, ctx: &FormContext, rule: Option<&AgentRuleRow>) -> FormSpec {
    let d = &loaded.definition;
    let now = current(rule, ctx);
    let with_value = |mut f: FormField| {
        if let Some(v) = now.get(&f.name) {
            f.value = Some(v.clone());
        }
        f
    };

    // 1. What
    let mut what = vec![with_value(FormField {
        required: true,
        help: Some(
            "Its name among this agent's instances: lowercase letters, digits and -.".into(),
        ),
        placeholder: Some("l2-prod".into()),
        max_len: Some(60),
        ..field(F_INSTANCE, FieldType::Text, "Instance")
    })];
    let inputs_form = super::definition::inputs_form(d);
    for (i, f) in d
        .inputs
        .iter()
        .zip(inputs_form.iter().flat_map(|f| f.steps[0].fields.iter()))
    {
        let mut f = f.clone();
        match i.kind {
            InputType::Tracker => {
                f.kind = FieldType::Select;
                f.options = Some(
                    ctx.trackers
                        .iter()
                        .map(|(id, name)| pair(id.to_string(), name.clone()))
                        .collect(),
                );
            }
            InputType::Project => {
                f.kind = FieldType::Select;
                f.options = Some(
                    ctx.projects
                        .iter()
                        .map(|(_, n)| pair(n.clone(), n.clone()))
                        .collect(),
                );
            }
            InputType::Host => {
                f.kind = FieldType::Select;
                f.options = Some(
                    ctx.hosts
                        .iter()
                        .map(|h| pair(h.clone(), h.clone()))
                        .collect(),
                );
            }
            _ => {}
        }
        what.push(with_value(f));
    }

    // 2. Where
    let mut where_ = vec![with_value(FormField {
        required: true,
        help: Some("Where its runs happen; its MCP servers start there.".into()),
        options: Some(
            ctx.hosts
                .iter()
                .map(|h| pair(h.clone(), h.clone()))
                .collect(),
        ),
        ..field(F_HOST, FieldType::Select, "Host")
    })];
    if d.run.workspace == Workspace::Project {
        where_.push(with_value(FormField {
            required: true,
            help: Some("The checkout its runs read (at main).".into()),
            options: Some(
                ctx.projects
                    .iter()
                    .map(|(_, n)| pair(n.clone(), n.clone()))
                    .collect(),
            ),
            ..field(F_PROJECT, FieldType::Select, "Project")
        }));
    }

    // 3. Stores
    let mut stores = Vec::new();
    for r in &d.stores {
        match r.kind {
            StoreKind::Mcp => {
                // Left empty, an optional role is not bound.
                let opts: Vec<FormOption> = ctx
                    .servers
                    .iter()
                    .map(|s| pair(s.clone(), s.clone()))
                    .collect();
                stores.push(with_value(FormField {
                    required: r.required,
                    help: Some(format!(
                        "A catalog MCP server. Read: {}{}.",
                        if r.read_tools.is_empty() {
                            "nothing".to_string()
                        } else {
                            r.read_tools.join(", ")
                        },
                        if r.write_tools.is_empty() {
                            String::new()
                        } else {
                            format!(
                                "; write: {}",
                                r.write_tools
                                    .iter()
                                    .map(|w| w.name())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            )
                        }
                    )),
                    options: Some(opts),
                    ..field(&f_store(&r.role), FieldType::Select, &r.label)
                }));
                stores.push(with_value(FormField {
                    help: Some(
                        "Optional, one per line: NAME=${SECRET} points the server's ${NAME} at another secret of the hub's secret store (never a value)."
                            .into(),
                    ),
                    placeholder: Some("GRAYLOG_API_TOKEN=${GRAYLOG_PROD_TOKEN}".into()),
                    max_len: Some(2000),
                    ..field(&f_env(&r.role), FieldType::Textarea, &format!("{} secrets", r.label))
                }));
            }
            StoreKind::Fleet => {
                let mut opts = vec![pair("rule", "This instance's own")];
                opts.extend(ctx.shared.iter().map(|n| {
                    pair(
                        n.clone(),
                        format!("Shared: {}", n.trim_start_matches("shared:")),
                    )
                }));
                let mut f = FormField {
                    required: r.required,
                    help: Some("Fleet's own records. Another: type shared:<name> to share it with other instances.".into()),
                    options: Some(opts),
                    other: true,
                    ..field(&f_store(&r.role), FieldType::Select, &r.label)
                };
                if !now.contains_key(&f.name) {
                    f.value = Some(if r.shared_by_default {
                        format!("shared:{}", r.role).into()
                    } else {
                        "rule".into()
                    });
                }
                stores.push(with_value(f));
            }
        }
    }

    // 4. How much
    let how = vec![
        with_value(FormField {
            required: true,
            value: Some("watch".into()),
            options: Some(vec![
                pair("watch", "Watch: run, keep the answer, write nothing"),
                pair("propose", "Propose: each write waits for you"),
                pair("act", "Act: the unattended writes are sent"),
            ]),
            ..field(F_LEVEL, FieldType::Select, "Level")
        }),
        FormField {
            help: Some("The instance ends after this many hours; empty: never.".into()),
            min: Some(1.0),
            max: Some(MAX_RULE_HOURS as f64),
            integer: true,
            ..field(F_HOURS, FieldType::Number, "Hours")
        },
    ];

    let mut steps = vec![step("What", what), step("Where", where_)];
    if !stores.is_empty() {
        steps.push(step("Stores", stores));
    }
    let mut last = step("How much", how);
    last.intro = Some(writes_line(loaded));
    steps.push(last);
    FormSpec {
        spec: forms::FORM_SPEC_VERSION.into(),
        title: if rule.is_some() {
            format!("Edit {}", loaded.name)
        } else {
            format!("New {} instance", loaded.name)
        },
        intro: None,
        submit: Some(if rule.is_some() { "Save" } else { "Start" }.into()),
        save_later: false,
        steps,
        checks: Vec::new(),
    }
}

/// PURE: one `NAME=${SECRET}` per line as an override map.
pub fn parse_env(text: &str) -> Result<BTreeMap<String, String>, String> {
    let mut out = BTreeMap::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let (k, v) = line
            .split_once('=')
            .ok_or_else(|| format!("{line:?} is not NAME=${{SECRET}}"))?;
        out.insert(k.trim().to_string(), v.trim().to_string());
    }
    Ok(out)
}

/// PURE: a form's answers (checked against [`grant_form`]) as the `agents`
/// tool's fleet grant.
pub fn answers_to_args(
    loaded: &Loaded,
    ctx: &FormContext,
    answers: &Map<String, Value>,
) -> Result<AgentsArgs, String> {
    let d = &loaded.definition;
    let form = grant_form(loaded, ctx, None);
    let checked = forms::check_answers(&form, answers)
        .map_err(|ps| {
            ps.iter()
                .map(|p| format!("{}: {}", p.field, p.problem))
                .collect::<Vec<_>>()
                .join("; ")
        })?
        .values;
    let s = |k: &str| {
        checked
            .get(k)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty())
    };
    let project_id = match s(F_PROJECT) {
        Some(p) => Some(
            ctx.projects
                .iter()
                .find(|(_, n)| n == p)
                .map(|(id, _)| *id)
                .ok_or_else(|| format!("no project {p}"))?,
        ),
        None => None,
    };
    let mut values = Map::new();
    for i in &d.inputs {
        if let Some(v) = checked.get(&i.name) {
            values.insert(i.name.clone(), v.clone());
        }
    }
    let mut binds = Vec::new();
    for r in &d.stores {
        let Some(chosen) = s(&f_store(&r.role)) else {
            continue;
        };
        binds.push(match r.kind {
            StoreKind::Mcp => Binding {
                role: r.role.clone(),
                server: Some(chosen.to_string()),
                name: None,
                env: parse_env(s(&f_env(&r.role)).unwrap_or_default())
                    .map_err(|e| format!("{}: {e}", f_env(&r.role)))?,
                namespace: None,
            },
            StoreKind::Fleet => Binding {
                role: r.role.clone(),
                server: None,
                name: None,
                env: BTreeMap::new(),
                namespace: Some(chosen.to_string()),
            },
        });
    }
    Ok(AgentsArgs {
        action: "grant".into(),
        agent: Some("fleet".into()),
        scope: s(F_INSTANCE).map(str::to_string),
        level: s(F_LEVEL).map(str::to_string),
        host: s(F_HOST).map(str::to_string),
        project_id,
        config: Some(Value::Object(values)),
        hours: checked.get(F_HOURS).and_then(Value::as_i64),
        definition: Some(loaded.name.clone()),
        bindings: Some(json!(binds)),
        ..Default::default()
    })
}

/// One fleet agent of the catalog, as the Agents tab lists it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DefinitionView {
    pub name: String,
    pub description: String,
    /// The trigger's source.
    pub trigger: Option<String>,
    /// What is wrong with it; empty when it can be granted.
    pub problems: Vec<String>,
    /// [`writes_line`].
    pub writes: String,
}

/// The catalog's fleet agents.
pub fn definitions() -> Result<Vec<DefinitionView>, crate::ipc_error::IpcError> {
    use crate::service::catalog::{model::Kind, CatalogTarget};
    CatalogTarget::Personal.with(|cat| {
        Ok(cat
            .assets
            .iter()
            .filter(|a| a.kind() == Kind::FleetAgent)
            .map(|a| {
                let loaded = a.fleet_agent();
                DefinitionView {
                    name: a.header.name.clone(),
                    description: a.header.description.clone(),
                    trigger: loaded
                        .as_ref()
                        .ok()
                        .map(|l| l.definition.trigger.source.word().to_string()),
                    writes: loaded.as_ref().map(writes_line).unwrap_or_default(),
                    problems: loaded.err().unwrap_or_default(),
                }
            })
            .collect())
    })
}
