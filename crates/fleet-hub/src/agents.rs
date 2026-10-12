//! `fleet-hub agents …` — the person's side of the built-in agents
//! (`fleet_core::service::agents`): grant an agent a standing rule on a
//! repo or a Jira project, revoke it, revoke them all, answer an action an
//! agent proposed, and read what the agents did.
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
    /// Let an agent work on a repo or a Jira project.
    ///
    /// `watch` runs the agent and keeps its result in fleet; `propose`
    /// also queues each write it wants (a review, a ticket comment) for a
    /// person to approve; `act` writes, except what the agent always asks
    /// about.
    Grant {
        /// review | support
        agent: String,
        /// gh:<owner>/<repo> for review, jira:<tracker id>:<PROJECT> for support.
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
    },
    /// Remove one rule, by its id (see `status`).
    Revoke { id: i64 },
    /// Remove every rule: the agents stop at their next tick.
    PauseAll,
    /// Approve an action an agent proposed, by its id.
    Approve { id: i64 },
    /// Reject an action an agent proposed, by its id.
    Reject { id: i64 },
    /// The rules, the newest jobs and the actions waiting for a person.
    Status {
        /// How many jobs and actions to print. [default: 20]
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },
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

pub fn run(
    cmd: AgentsCmd,
    opts: &HubOptions,
    env: &HashMap<String, String>,
) -> Result<ExitCode, String> {
    crate::serve::existing_db(&crate::config::resolve_data_dir(opts, env))?;
    let store = crate::serve::open_store(opts, env)?;
    run_on(&Mutex::new(store), cmd, fleet_core::store::now_unix())
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
        } => {
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
        }
    }

    fn rules(s: &Mutex<Store>) -> Vec<fleet_core::store::AgentRuleRow> {
        s.lock().unwrap().list_agent_rules().unwrap()
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
}
