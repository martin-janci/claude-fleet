//! `fleet-hub routine …` — debugging the routines from the hub's console
//! (`fleet_core::service::routines::debug`): list them, explain why one
//! does or does not fire, print its decision trace, and dry-run it. Every
//! subcommand only reads `state.db`, as the hub's own reader: nothing
//! starts a run. The scheduler's loop health lives in the running hub's
//! process, so `explain` here says the scheduler has not passed; ask the
//! hub (`routines { action: explain }`) for that row.

use crate::config::HubOptions;
use crate::out;
use clap::Subcommand;
use fleet_core::service::routines;
use fleet_core::service::view_scope::ViewScope;
use fleet_core::store::Store;
use std::collections::HashMap;
use std::process::ExitCode;
use std::sync::Mutex;

#[derive(Subcommand, Debug)]
pub enum RoutineCmd {
    /// Every routine: id, on or off, trigger and name.
    List,
    /// Why one routine does or does not fire: its checks in the
    /// scheduler's order, an event routine's newest events judged, and its
    /// newest decisions.
    Explain {
        id: i64,
        /// How many decisions to print. [default: 30]
        #[arg(long)]
        limit: Option<i64>,
    },
    /// One routine's decision trace alone, newest first.
    Decisions {
        id: i64,
        /// How many to print. [default: 30]
        #[arg(long)]
        limit: Option<i64>,
    },
    /// What a fire now would start and the exact prompt it would type.
    DryRun {
        id: i64,
        /// The session event to try an event routine on. [default: its
        /// kind's newest]
        #[arg(long)]
        event: Option<i64>,
    },
}

pub fn run(
    cmd: RoutineCmd,
    opts: &HubOptions,
    env: &HashMap<String, String>,
) -> Result<ExitCode, String> {
    crate::serve::existing_db(&crate::config::resolve_data_dir(opts, env))?;
    let store = crate::serve::open_store(opts, env)?;
    run_on(&Mutex::new(store), cmd, fleet_core::store::now_unix())
}

fn decision_line(d: &fleet_core::store::RoutineDecisionRow) -> String {
    format!(
        "{}\t{}\t{}\t{}",
        d.at,
        d.trigger,
        d.verdict,
        d.reason.as_deref().unwrap_or("-")
    )
}

fn run_on(store: &Mutex<Store>, cmd: RoutineCmd, now: i64) -> Result<ExitCode, String> {
    let err = |e: fleet_core::ipc_error::IpcError| e.message;
    let scope = ViewScope::internal();
    match cmd {
        RoutineCmd::List => {
            let rows = routines::list(store, &scope).map_err(err)?;
            if rows.is_empty() {
                out::line("no routines");
            }
            for r in rows {
                out::line(&format!(
                    "routine {}\t{}\t{}\t{}",
                    r.id,
                    if r.enabled { "on" } else { "off" },
                    r.trigger,
                    r.name
                ));
            }
        }
        RoutineCmd::Explain { id, limit } => {
            let e = routines::explain(store, &scope, id, limit, now).map_err(err)?;
            out::line(&format!(
                "routine {} ({}): {}",
                e.routine_id, e.name, e.summary
            ));
            for c in &e.checks {
                out::line(&format!("  {}\t{}\t{}", c.state, c.name, c.detail));
            }
            if let Some(ev) = &e.event {
                out::line(&format!(
                    "{} events: read up to #{}{}",
                    ev.kind,
                    ev.cursor,
                    ev.unread
                        .map(|n| format!(", {n} unread"))
                        .unwrap_or_default()
                ));
                for v in &ev.recent {
                    out::line(&format!(
                        "  #{}\t{}\t{}\t{}",
                        v.event_id,
                        match (v.session_id, v.subject.as_deref()) {
                            (Some(sid), _) => format!("session {sid}"),
                            (None, Some(subject)) => subject.to_string(),
                            (None, None) => "-".to_string(),
                        },
                        if v.fires { "fires" } else { "no" },
                        v.reason.as_deref().unwrap_or("-")
                    ));
                }
            }
            for d in &e.decisions {
                out::line(&decision_line(d));
            }
        }
        RoutineCmd::Decisions { id, limit } => {
            for d in routines::decisions(store, &scope, id, limit).map_err(err)? {
                out::line(&decision_line(&d));
            }
        }
        RoutineCmd::DryRun { id, event } => {
            let d = routines::dry_run(store, &scope, id, event, now).map_err(err)?;
            if d.starts {
                out::line(&format!(
                    "starts a session on {}{}{}",
                    d.session.host_alias,
                    d.session
                        .profile
                        .as_deref()
                        .map(|p| format!(" as profile {p}"))
                        .unwrap_or_default(),
                    d.note
                        .as_deref()
                        .map(|n| format!(" ({n})"))
                        .unwrap_or_default()
                ));
            } else {
                out::line(&format!("skipped: {}", d.reason.as_deref().unwrap_or("-")));
            }
            if let Some(v) = &d.event {
                out::line(&format!(
                    "event #{} ({}): {}",
                    v.event_id,
                    v.kind,
                    if v.fires {
                        "fires it".to_string()
                    } else {
                        format!("does not fire it: {}", v.reason.as_deref().unwrap_or("-"))
                    }
                ));
            }
            for c in d.scheduled_only.iter().filter(|c| c.state != "ok") {
                out::line(&format!(
                    "a scheduled fire also needs {}: {}",
                    c.name, c.detail
                ));
            }
            out::line("---");
            out::line(&d.prompt);
        }
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn every_subcommand_reads_and_an_unknown_id_is_not_found() {
        let (_dir, s) = store();
        run_on(&s, RoutineCmd::List, 1000).unwrap();
        let pid = s.lock().unwrap().list_projects().unwrap()[0].id;
        let r = routines::save(
            &s,
            &ViewScope::internal(),
            None,
            &routines::RoutineInput {
                name: "Sweep".into(),
                trigger: "manual".into(),
                host_alias: "box".into(),
                project_id: pid,
                prompt: "Look.".into(),
                ..Default::default()
            },
        )
        .unwrap();
        run_on(
            &s,
            RoutineCmd::Explain {
                id: r.id,
                limit: None,
            },
            1000,
        )
        .unwrap();
        run_on(
            &s,
            RoutineCmd::Decisions {
                id: r.id,
                limit: Some(5),
            },
            1000,
        )
        .unwrap();
        run_on(
            &s,
            RoutineCmd::DryRun {
                id: r.id,
                event: None,
            },
            1000,
        )
        .unwrap();
        assert!(s.lock().unwrap().routine_runs(r.id, 10).unwrap().is_empty());
        let e = run_on(
            &s,
            RoutineCmd::Explain {
                id: 999,
                limit: None,
            },
            1000,
        )
        .unwrap_err();
        assert!(e.contains("not found"), "{e}");
    }
}
