//! `fleet-hub shepherd …` — the person's side of the PR shepherd
//! (`fleet_core::service::pr_shepherd`): grant a project a standing rule,
//! revoke it, revoke them all, and read what the shepherd did.
//!
//! **Written straight to `state.db`**, as `session claim` is: the rule is
//! a person's act, and no control API action writes it, so an agent holding
//! the master token still cannot grant itself one. A running hub reads the
//! rules on every reconcile tick, so no restart is needed.

use crate::config::HubOptions;
use crate::out;
use clap::Subcommand;
use fleet_core::store::{ShepherdRuleRow, Store, SHEPHERD_LEVELS};
use std::collections::HashMap;
use std::process::ExitCode;

#[derive(Subcommand, Debug)]
pub enum ShepherdCmd {
    /// Let the shepherd look after one project's PRs.
    ///
    /// `watch` records what is wrong on the session's timeline; `nudge`
    /// also asks the session that opened the PR to fix a conflict, a lag
    /// behind the base branch or red CI; `merge` is `nudge` until the merge
    /// queue lands.
    Grant {
        /// The project as owner/repo.
        project: String,
        /// watch | nudge | merge
        #[arg(long, default_value = "watch")]
        level: String,
        /// The rule ends after this many hours. [default: never]
        #[arg(long)]
        hours: Option<i64>,
        /// Text added to conflict prompts: the project's regenerate and
        /// check commands, at most 2000 characters.
        #[arg(long)]
        recipes: Option<String>,
    },
    /// Remove one project's rule.
    Revoke {
        /// The project as owner/repo.
        project: String,
    },
    /// Remove every rule: the shepherd stops at its next tick.
    PauseAll,
    /// The rules and the newest episodes.
    Status {
        /// How many episodes to print. [default: 20]
        #[arg(long, default_value_t = 20)]
        limit: usize,
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

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn run(
    cmd: ShepherdCmd,
    opts: &HubOptions,
    env: &HashMap<String, String>,
) -> Result<ExitCode, String> {
    crate::serve::existing_db(&crate::config::resolve_data_dir(opts, env))?;
    let store = crate::serve::open_store(opts, env)?;
    run_on(&store, cmd, now())
}

fn run_on(store: &Store, cmd: ShepherdCmd, now: i64) -> Result<ExitCode, String> {
    match cmd {
        ShepherdCmd::Grant {
            project,
            level,
            hours,
            recipes,
        } => {
            if !SHEPHERD_LEVELS.contains(&level.as_str()) {
                return Err(format!(
                    "--level must be one of {}",
                    SHEPHERD_LEVELS.join(", ")
                ));
            }
            if hours.is_some_and(|h| h <= 0) {
                return Err("--hours must be positive".into());
            }
            let id = project_id(store, &project)?;
            let expires_at = hours.map(|h| now + h * 3600);
            store
                .grant_shepherd_rule(&ShepherdRuleRow {
                    project_id: id,
                    level: level.clone(),
                    granted_by: "console".into(),
                    granted_at: now,
                    expires_at,
                    recipes,
                })
                .map_err(|e| e.to_string())?;
            out::line(&format!(
                "{project}: shepherd at {level}{}",
                hours.map(|h| format!(" for {h} h")).unwrap_or_default()
            ));
        }
        ShepherdCmd::Revoke { project } => {
            let id = project_id(store, &project)?;
            if store.revoke_shepherd_rule(id).map_err(|e| e.to_string())? {
                out::line(&format!("{project}: rule removed"));
            } else {
                out::line(&format!("{project} had no rule"));
            }
        }
        ShepherdCmd::PauseAll => {
            let n = store
                .revoke_all_shepherd_rules()
                .map_err(|e| e.to_string())?;
            out::line(&format!("{n} rule(s) removed"));
        }
        ShepherdCmd::Status { limit } => {
            let names: HashMap<i64, String> = store
                .list_projects()
                .map_err(|e| e.to_string())?
                .into_iter()
                .map(|p| (p.id, format!("{}/{}", p.owner, p.repo)))
                .collect();
            let rules = store.list_shepherd_rules().map_err(|e| e.to_string())?;
            if rules.is_empty() {
                out::line("no rules: the shepherd is idle");
            }
            for r in &rules {
                let name = names
                    .get(&r.project_id)
                    .cloned()
                    .unwrap_or_else(|| format!("project {}", r.project_id));
                let state = match r.expires_at {
                    Some(e) if e <= now => " (expired)".to_string(),
                    Some(e) => format!(" (ends in {} min)", (e - now) / 60),
                    None => String::new(),
                };
                out::line(&format!("{name}\t{}{state}", r.level));
            }
            for e in store
                .list_shepherd_episodes(limit)
                .map_err(|e| e.to_string())?
            {
                out::line(&format!(
                    "{}\tsession {}\t{}\t{}\t{}",
                    e.at,
                    e.session_id,
                    e.condition,
                    e.outcome,
                    e.pr_url.as_deref().unwrap_or("-")
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
    /// with one project `o/r`. The directory lives as long as the store.
    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open_with_bus(
            &dir.path().join("state.db"),
            std::sync::Arc::new(fleet_core::events::NoopEventBus),
        )
        .unwrap();
        s.upsert_project("o", "r", "/p/o/r").unwrap();
        (dir, s)
    }

    fn grant(level: &str, hours: Option<i64>) -> ShepherdCmd {
        ShepherdCmd::Grant {
            project: "o/r".into(),
            level: level.into(),
            hours,
            recipes: None,
        }
    }

    #[test]
    fn grant_writes_a_rule_with_its_expiry_and_revoke_removes_it() {
        let (_dir, s) = store();
        run_on(&s, grant("nudge", Some(2)), 1000).unwrap();
        let rules = s.list_shepherd_rules().unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].level, "nudge");
        assert_eq!(rules[0].expires_at, Some(1000 + 7200));
        assert_eq!(rules[0].granted_by, "console");
        run_on(
            &s,
            ShepherdCmd::Revoke {
                project: "o/r".into(),
            },
            1000,
        )
        .unwrap();
        assert!(s.list_shepherd_rules().unwrap().is_empty());
    }

    #[test]
    fn grant_refuses_a_bad_level_hours_or_project() {
        let (_dir, s) = store();
        assert!(run_on(&s, grant("auto", None), 0).is_err());
        assert!(run_on(&s, grant("watch", Some(0)), 0).is_err());
        let missing = ShepherdCmd::Grant {
            project: "o/nope".into(),
            level: "watch".into(),
            hours: None,
            recipes: None,
        };
        assert!(run_on(&s, missing, 0).is_err());
        assert!(s.list_shepherd_rules().unwrap().is_empty());
    }

    #[test]
    fn pause_all_removes_every_rule() {
        let (_dir, s) = store();
        run_on(&s, grant("watch", None), 0).unwrap();
        run_on(&s, ShepherdCmd::PauseAll, 0).unwrap();
        assert!(s.list_shepherd_rules().unwrap().is_empty());
    }
}
