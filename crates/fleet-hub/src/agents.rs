//! `fleet-hub agents …` — the person's side of the built-in agents
//! (`fleet_core::service::agents`): grant an agent a standing rule on a
//! repo or a Jira project, revoke it, revoke them all, answer an action an
//! agent proposed, read what the agents did, and check a configurable
//! agent's definition (`define`).
//!
//! **Written straight to `state.db`**, as `shepherd grant` is: the rule is a
//! person's act, and no control API action an agent's master token reaches
//! writes it. A running hub reads the rules on every tick, so no restart is
//! needed. The checks are the control API's own
//! (`service::agents::admin::validate_grant`).

use crate::config::HubOptions;
use crate::out;
use clap::Subcommand;
use fleet_core::service::agents::admin::{self, AgentsArgs};
use fleet_core::store::Store;
use std::collections::HashMap;
use std::process::ExitCode;
use std::sync::Mutex;

#[derive(Subcommand, Debug)]
pub enum AgentsCmd {
    /// Let an agent work on a repo or a Jira project, or start an instance
    /// of a fleet agent (a catalog `fleet_agent`).
    ///
    /// `watch` runs the agent and keeps its result in fleet; `propose`
    /// also queues each write it wants (a review, a ticket comment) for a
    /// person to approve; `act` writes, except what the agent always asks
    /// about.
    ///
    /// A fleet agent: `grant fleet <instance> --definition <name> --values
    /// values.json --bindings bindings.json --host <h>`. Each MCP binding is
    /// started on the host and asked for its tools before the rule is
    /// written.
    Grant {
        /// review | support | fleet
        agent: String,
        /// gh:<owner>/<repo> for review, jira:<tracker id>:<PROJECT> for
        /// support, the instance name for fleet.
        scope: String,
        /// The host the agent's runs use.
        #[arg(long)]
        host: String,
        /// watch | propose | act
        #[arg(long, default_value = "watch")]
        level: String,
        /// The project, as owner/repo, whose checkout runs use.
        #[arg(long)]
        project: Option<String>,
        /// The agent's options, a JSON object.
        #[arg(long)]
        config: Option<String>,
        /// The rule ends after this many hours. [default: never]
        #[arg(long)]
        hours: Option<i64>,
        #[command(flatten)]
        fleet: Box<FleetGrant>,
    },
    /// Remove one rule, by its id (see `status`).
    Revoke { id: i64 },
    /// Remove every rule: the agents stop at their next tick.
    PauseAll,
    /// Approve an action an agent proposed, by its id.
    Approve { id: i64 },
    /// Reject an action an agent proposed, by its id.
    Reject { id: i64 },
    /// Per rule: whether it polls and why not, its next poll and its jobs
    /// by state. Its last poll error lives in the running hub's process:
    /// ask the hub (`agents { action: explain }`) to see it.
    Explain,
    /// One job whole: what it is waiting for or came to, its error, its
    /// answer and the actions it proposed.
    Job { id: i64 },
    /// Check a configurable agent's definition (a `fleet_agent` catalog
    /// asset): its directory (`asset.yaml` + `skill.md`) or its
    /// `asset.yaml`. Prints every problem; exits 1 when there is one.
    Define { path: std::path::PathBuf },
    /// The rules, the newest jobs and the actions waiting for a person.
    Status {
        /// How many jobs and actions to print. [default: 20]
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },
}

/// `grant fleet`'s own options.
#[derive(clap::Args, Debug, Default, Clone, PartialEq, Eq)]
pub struct FleetGrant {
    /// fleet: the catalog fleet_agent the instance runs.
    #[arg(long)]
    pub definition: Option<String>,
    /// fleet: a JSON file of the instance's input values (an object).
    #[arg(long)]
    pub values: Option<std::path::PathBuf>,
    /// fleet: a JSON file of the store bindings (an array).
    #[arg(long)]
    pub bindings: Option<std::path::PathBuf>,
}

fn project_id(store: &Store, spec: &str) -> Result<i64, String> {
    let (owner, repo) = spec
        .split_once('/')
        .ok_or_else(|| format!("{spec:?} is not owner/repo"))?;
    store
        .list_projects()
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|p| p.owner == owner && p.repo == repo)
        .map(|p| p.id)
        .ok_or_else(|| format!("no project {spec} on this hub"))
}

pub async fn run(
    cmd: AgentsCmd,
    opts: &HubOptions,
    env: &HashMap<String, String>,
) -> Result<ExitCode, String> {
    if let AgentsCmd::Define { path } = &cmd {
        return define(path);
    }
    crate::serve::existing_db(&crate::config::resolve_data_dir(opts, env))?;
    let store = Mutex::new(crate::serve::open_store(opts, env)?);
    let now = fleet_core::store::now_unix();
    if let AgentsCmd::Grant { agent, .. } = &cmd {
        if agent == "fleet" {
            let a = fleet_args(&store, cmd)?;
            fleet_core::service::catalog::load(false, &store).map_err(|e| e.message)?;
            let ssh = fleet_core::ssh::SshClient::new();
            let id =
                fleet_core::service::fleet_agents::grant::grant(&store, &ssh, &a, "console", now)
                    .await
                    .map_err(|e| e.message)?
                    .id
                    .unwrap_or_default();
            out::line(&format!(
                "rule {id}: fleet agent {} as {} at {}{}",
                a.definition.as_deref().unwrap_or_default(),
                a.scope.as_deref().unwrap_or_default(),
                a.level.as_deref().unwrap_or_default(),
                a.hours.map(|h| format!(" for {h} h")).unwrap_or_default()
            ));
            return Ok(ExitCode::SUCCESS);
        }
    }
    run_on(&store, cmd, now)
}

fn json_file(path: &std::path::Path) -> Result<serde_json::Value, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// `grant fleet …` as the `agents` tool's arguments.
fn fleet_args(store: &Mutex<Store>, cmd: AgentsCmd) -> Result<AgentsArgs, String> {
    let AgentsCmd::Grant {
        agent,
        scope,
        host,
        level,
        project,
        config,
        hours,
        fleet,
    } = cmd
    else {
        return Err("not a grant".into());
    };
    let FleetGrant {
        definition,
        values,
        bindings,
    } = *fleet;
    if config.is_some() {
        return Err("a fleet agent takes --values, not --config".into());
    }
    let project_id = match &project {
        Some(p) => {
            let s = store.lock().map_err(|e| e.to_string())?;
            Some(project_id(&s, p)?)
        }
        None => None,
    };
    Ok(AgentsArgs {
        agent: Some(agent),
        scope: Some(scope),
        level: Some(level),
        host: Some(host),
        project_id,
        config: values.as_deref().map(json_file).transpose()?,
        hours,
        definition: Some(definition.ok_or("a fleet agent needs --definition")?),
        bindings: bindings.as_deref().map(json_file).transpose()?,
        ..args("grant")
    })
}

/// `agents define`: load and check a definition, no hub needed.
fn define(path: &std::path::Path) -> Result<ExitCode, String> {
    let (yaml_path, dir) = if path.is_dir() {
        (path.join("asset.yaml"), path.to_path_buf())
    } else {
        (
            path.to_path_buf(),
            path.parent().map(|p| p.to_path_buf()).unwrap_or_default(),
        )
    };
    let yaml =
        std::fs::read_to_string(&yaml_path).map_err(|e| format!("{}: {e}", yaml_path.display()))?;
    let skill = std::fs::read_to_string(dir.join("skill.md")).unwrap_or_default();
    match fleet_core::service::fleet_agents::definition::load(&yaml, &skill) {
        Ok(l) => {
            let d = &l.definition;
            out::line(&format!(
                "{}: ok · {} input(s), {} store(s), trigger {}, {} action(s)",
                l.name,
                d.inputs.len(),
                d.stores.len(),
                d.trigger.source.word(),
                d.actions.len()
            ));
            Ok(ExitCode::SUCCESS)
        }
        Err(problems) => {
            for p in &problems {
                out::line(p);
            }
            out::line(&format!("{} problem(s)", problems.len()));
            Ok(ExitCode::from(1))
        }
    }
}

fn args(action: &str) -> AgentsArgs {
    AgentsArgs {
        action: action.into(),
        ..Default::default()
    }
}

fn run_on(store: &Mutex<Store>, cmd: AgentsCmd, now: i64) -> Result<ExitCode, String> {
    let err = |e: fleet_core::ipc_error::IpcError| e.message;
    match cmd {
        AgentsCmd::Grant {
            agent,
            scope,
            host,
            level,
            project,
            config,
            hours,
            fleet,
        } => {
            if *fleet != FleetGrant::default() {
                return Err("--definition, --values and --bindings are for agent fleet".into());
            }
            let project_id = match &project {
                Some(p) => {
                    let s = store.lock().map_err(|e| e.to_string())?;
                    Some(project_id(&s, p)?)
                }
                None => None,
            };
            let config = config
                .map(|c| serde_json::from_str(&c).map_err(|e| format!("--config: {e}")))
                .transpose()?;
            let a = AgentsArgs {
                agent: Some(agent.clone()),
                scope: Some(scope.clone()),
                level: Some(level.clone()),
                host: Some(host),
                project_id,
                config,
                hours,
                ..args("grant")
            };
            let id = admin::grant(store, &a, "console", now)
                .map_err(err)?
                .id
                .unwrap_or_default();
            out::line(&format!(
                "rule {id}: {agent} on {scope} at {level}{}",
                hours.map(|h| format!(" for {h} h")).unwrap_or_default()
            ));
        }
        AgentsCmd::Revoke { id } => {
            let a = AgentsArgs {
                id: Some(id),
                ..args("revoke")
            };
            if admin::revoke(store, &a).map_err(err)?.changed > 0 {
                out::line(&format!("rule {id} removed"));
            } else {
                out::line(&format!("no rule {id}"));
            }
        }
        AgentsCmd::PauseAll => {
            let n = admin::pause_all(store).map_err(err)?.changed;
            out::line(&format!("{n} rule(s) removed"));
        }
        AgentsCmd::Approve { id } | AgentsCmd::Reject { id } => {
            let approve = matches!(cmd, AgentsCmd::Approve { .. });
            let a = AgentsArgs {
                id: Some(id),
                ..args(if approve { "approve" } else { "reject" })
            };
            let row = admin::decide(store, &a, approve, "console", now).map_err(err)?;
            out::line(&format!("action {id}: {}", row.state));
        }
        AgentsCmd::Explain => {
            let e = fleet_core::service::agents::debug::explain(store, now).map_err(err)?;
            out::line(&format!(
                "{}polls every {} s, {} at once, {} retries",
                if e.automation_paused {
                    "Pause all is on; "
                } else {
                    ""
                },
                e.poll_interval_secs,
                e.max_concurrent,
                e.max_retries
            ));
            if e.rules.is_empty() {
                out::line("no rules: the agents are idle");
            }
            for r in &e.rules {
                let jobs: Vec<String> = r.jobs.iter().map(|(k, n)| format!("{n} {k}")).collect();
                out::line(&format!(
                    "rule {}\t{}\t{}\t{}\t{}",
                    r.rule.id,
                    r.state,
                    r.rule.scope,
                    r.summary,
                    if jobs.is_empty() {
                        "no jobs".into()
                    } else {
                        jobs.join(", ")
                    }
                ));
                if let Some(j) = &r.last_failed {
                    out::line(&format!(
                        "  last failed: job {}\t{}\t{}",
                        j.id,
                        j.subject,
                        j.error.as_deref().unwrap_or("-")
                    ));
                }
            }
        }
        AgentsCmd::Job { id } => {
            let d = fleet_core::service::agents::debug::job(store, id, now).map_err(err)?;
            out::line(&format!(
                "job {}\t{}\t{}\t{}",
                d.job.id, d.job.agent, d.job.subject, d.summary
            ));
            for a in &d.actions {
                out::line(&format!(
                    "  action {}\t{}\t{}{}",
                    a.id,
                    a.kind,
                    a.state,
                    a.error
                        .as_deref()
                        .map(|e| format!("\t{e}"))
                        .unwrap_or_default()
                ));
            }
            if let Some(r) = &d.job.result {
                out::line(r);
            }
        }
        AgentsCmd::Define { path } => return define(&path),
        AgentsCmd::Status { limit } => {
            let a = AgentsArgs {
                limit: Some(limit),
                ..args("status")
            };
            let st = admin::status(store, &a, now).map_err(err)?;
            if st.rules.is_empty() {
                out::line("no rules: the agents are idle");
            }
            for r in &st.rules {
                let state = match r.rule.expires_at {
                    Some(e) if e <= now => " (expired)".to_string(),
                    Some(e) => format!(" (ends in {} min)", (e - now) / 60),
                    None => String::new(),
                };
                out::line(&format!(
                    "rule {}\t{}\t{}\t{}\ton {}{state}",
                    r.rule.id, r.rule.agent, r.rule.scope, r.rule.level, r.rule.host_alias
                ));
            }
            for a in &st.pending {
                out::line(&format!(
                    "action {}\tjob {}\t{}\tpending",
                    a.id, a.job_id, a.kind
                ));
            }
            for j in &st.jobs {
                out::line(&format!(
                    "{}\tjob {}\t{}\t{}\t{}{}",
                    j.created_at,
                    j.id,
                    j.agent,
                    j.subject,
                    j.state,
                    j.error
                        .as_deref()
                        .map(|e| format!("\t{e}"))
                        .unwrap_or_default()
                ));
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A store on disk (fleet-core's in-memory store is test-only to it),
    /// with host `box` and project `o/r`. The directory lives as long as the
    /// store.
    fn store() -> (tempfile::TempDir, Mutex<Store>) {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open_with_bus(
            &dir.path().join("state.db"),
            std::sync::Arc::new(fleet_core::events::NoopEventBus),
        )
        .unwrap();
        s.upsert_host("box").unwrap();
        s.upsert_project("o", "r", "/p/o/r").unwrap();
        (dir, Mutex::new(s))
    }

    fn grant(level: &str, hours: Option<i64>) -> AgentsCmd {
        AgentsCmd::Grant {
            agent: "review".into(),
            scope: "gh:o/r".into(),
            host: "box".into(),
            level: level.into(),
            project: Some("o/r".into()),
            config: Some(r#"{"max_diff_lines":3000}"#.into()),
            hours,
            fleet: Box::default(),
        }
    }

    fn rules(s: &Mutex<Store>) -> Vec<fleet_core::store::AgentRuleRow> {
        s.lock().unwrap().list_agent_rules().unwrap()
    }

    #[test]
    fn explain_and_job_read_without_changing_anything() {
        let (_dir, s) = store();
        run_on(&s, AgentsCmd::Explain, 1000).unwrap();
        run_on(&s, grant("watch", None), 1000).unwrap();
        run_on(&s, AgentsCmd::Explain, 1000).unwrap();
        assert_eq!(rules(&s).len(), 1);
        let e = run_on(&s, AgentsCmd::Job { id: 42 }, 1000).unwrap_err();
        assert!(e.contains("no job 42"), "{e}");
    }

    #[test]
    fn grant_writes_a_rule_and_revoke_removes_it() {
        let (_dir, s) = store();
        run_on(&s, grant("propose", Some(2)), 1000).unwrap();
        let r = rules(&s);
        assert_eq!(r.len(), 1);
        assert_eq!(
            (r[0].level.as_str(), r[0].granted_by.as_str()),
            ("propose", "console")
        );
        assert_eq!(r[0].expires_at, Some(1000 + 7200));
        assert!(r[0].project_id.is_some());
        run_on(&s, AgentsCmd::Revoke { id: r[0].id }, 1000).unwrap();
        assert!(rules(&s).is_empty());
    }

    #[test]
    fn grant_refuses_bad_input() {
        let other = |scope: &str, host: &str, config: Option<&str>| AgentsCmd::Grant {
            agent: "review".into(),
            scope: scope.into(),
            host: host.into(),
            level: "watch".into(),
            project: None,
            config: config.map(Into::into),
            hours: None,
            fleet: Box::default(),
        };
        let (_dir, s) = store();
        for cmd in [
            grant("auto", None),
            grant("watch", Some(0)),
            other("o/r", "box", None),
            other("gh:o/r", "nowhere", None),
            other("gh:o/r", "box", Some("[1]")),
            other("gh:o/r", "box", Some("{")),
        ] {
            assert!(run_on(&s, cmd, 0).is_err());
        }
        assert!(rules(&s).is_empty());
    }

    #[test]
    fn pause_all_removes_every_rule() {
        let (_dir, s) = store();
        run_on(&s, grant("watch", None), 0).unwrap();
        run_on(&s, AgentsCmd::PauseAll, 0).unwrap();
        assert!(rules(&s).is_empty());
    }

    #[test]
    fn define_checks_a_definition_without_a_hub() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/fleet-agent-examples/l2-support");
        assert_eq!(define(&root).unwrap(), ExitCode::SUCCESS);
        let yaml = std::fs::read_to_string(root.join("asset.yaml")).unwrap();
        std::fs::write(
            dir.path().join("asset.yaml"),
            yaml.replace("source: tracker_comments", "source: carrier_pigeon"),
        )
        .unwrap();
        assert_eq!(define(dir.path()).unwrap(), ExitCode::from(1));
        assert!(define(&dir.path().join("nope.yaml")).is_err());
    }

    #[test]
    fn a_fleet_grant_reads_its_files_and_a_builtin_takes_none() {
        let (dir, s) = store();
        let values = dir.path().join("values.json");
        let binds = dir.path().join("bindings.json");
        std::fs::write(&values, r#"{"project":"L2S"}"#).unwrap();
        std::fs::write(&binds, r#"[{"role":"memory","namespace":"rule"}]"#).unwrap();
        let cmd = |config: Option<&str>| AgentsCmd::Grant {
            agent: "fleet".into(),
            scope: "l2-prod".into(),
            host: "box".into(),
            level: "watch".into(),
            project: Some("o/r".into()),
            config: config.map(Into::into),
            hours: Some(3),
            fleet: Box::new(FleetGrant {
                definition: Some("l2-support".into()),
                values: Some(values.clone()),
                bindings: Some(binds.clone()),
            }),
        };
        let a = fleet_args(&s, cmd(None)).unwrap();
        assert_eq!(a.config.unwrap()["project"], "L2S");
        assert_eq!(a.bindings.unwrap()[0]["namespace"], "rule");
        assert_eq!(a.definition.as_deref(), Some("l2-support"));
        assert!(a.project_id.is_some());
        assert!(fleet_args(&s, cmd(Some("{}"))).is_err());
        let mut builtin = grant("watch", None);
        if let AgentsCmd::Grant { fleet, .. } = &mut builtin {
            fleet.definition = Some("l2-support".into());
        }
        assert!(run_on(&s, builtin, 0).is_err());
        assert!(rules(&s).is_empty());
    }
}
