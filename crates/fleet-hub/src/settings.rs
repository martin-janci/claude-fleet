//! `fleet-hub settings …` — the hub operator's review of settings proposals
//! (declarative pages P5, design D-P4). An agent proposes a change over the
//! control API (`set_setting { propose: true }`); the operator applies or
//! rejects it here, and reads a setting's history.
//!
//! Reads and writes `state.db` directly, as the person at the hub's
//! console: an applied proposal is recorded with actor `person`. The hub
//! reads settings on use, so a change applies without a restart (a key
//! marked "applies after a restart" still needs one).

use crate::config::{self, HubOptions};
use crate::out;
use crate::serve;
use clap::Subcommand;
use fleet_core::service::settings_review;
use fleet_core::store::Store;
use std::collections::HashMap;
use std::process::ExitCode;
use std::sync::Arc;

#[derive(Subcommand, Debug)]
pub enum SettingsCmd {
    /// List the proposals waiting for review: key, now → proposed, who, why.
    Proposals {
        /// Print the answer as JSON instead of lines.
        #[arg(long)]
        json: bool,
    },
    /// Apply proposals by id (from `settings proposals`).
    Apply {
        #[arg(required = true)]
        ids: Vec<i64>,
    },
    /// Reject proposals by id.
    Reject {
        #[arg(required = true)]
        ids: Vec<i64>,
    },
    /// A setting's writes, newest first: when, who, before → after.
    History {
        key: String,
        /// How many (1-100). [default: 20]
        #[arg(long)]
        limit: Option<i64>,
        #[arg(long)]
        json: bool,
    },
}

fn open(opts: &HubOptions, env: &HashMap<String, String>) -> Result<Store, String> {
    let path = serve::existing_db(&config::resolve_data_dir(opts, env))?;
    Store::open_with_bus(&path, Arc::new(fleet_core::events::NoopEventBus))
        .map_err(|e| format!("open {}: {e}", path.display()))
}

fn shown(v: &str) -> &str {
    if v.is_empty() {
        "(empty)"
    } else {
        v
    }
}

/// A proposal's `why` as one console line. The text is the proposer's (an
/// agent or a paired device), so a newline in it could print a forged
/// proposal line under it and an escape sequence would reach the terminal.
fn why_line(why: &str) -> String {
    format!("      why: {}", fleet_core::mcp::guard::scrub_line(why))
}

pub fn run(
    cmd: SettingsCmd,
    opts: &HubOptions,
    env: &HashMap<String, String>,
) -> Result<ExitCode, String> {
    let s = open(opts, env)?;
    match cmd {
        SettingsCmd::Proposals { json } => {
            let rows = settings_review::pending(&s).map_err(|e| e.message)?;
            if json {
                out::line(&serde_json::to_string_pretty(&rows).map_err(|e| e.to_string())?);
            } else if rows.is_empty() {
                out::line("No settings proposals are waiting for review.");
            } else {
                for p in &rows {
                    let who = p.row.source_detail.as_deref().unwrap_or(&p.row.source);
                    out::line(&format!(
                        "#{}  {}: {} → {}  ({who})",
                        p.row.id,
                        p.row.key,
                        shown(&p.current),
                        shown(&p.row.value)
                    ));
                    if let Some(why) = &p.row.why {
                        out::line(&why_line(why));
                    }
                }
                out::line("Apply with `fleet-hub settings apply <id>…`, or `reject <id>…`.");
            }
            Ok(ExitCode::SUCCESS)
        }
        SettingsCmd::Apply { ids } => report(settings_review::decide(&s, &ids, &[])),
        SettingsCmd::Reject { ids } => report(settings_review::decide(&s, &[], &ids)),
        SettingsCmd::History { key, limit, json } => {
            let rows = settings_review::history(&s, &key, limit).map_err(|e| e.message)?;
            if json {
                out::line(&serde_json::to_string_pretty(&rows).map_err(|e| e.to_string())?);
            } else if rows.is_empty() {
                out::line(&format!(
                    "{key} has not been changed since history was kept."
                ));
            } else {
                for h in &rows {
                    let who = match &h.actor_detail {
                        Some(d) => format!("{} ({d})", h.actor),
                        None => h.actor.clone(),
                    };
                    let proposal = h
                        .proposal_id
                        .map(|id| format!(", proposal #{id}"))
                        .unwrap_or_default();
                    out::line(&format!(
                        "{}  {} → {}  by {who}{proposal}",
                        h.at,
                        h.before.as_deref().map_or("(default)", shown),
                        shown(&h.after)
                    ));
                }
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn report(
    r: Result<settings_review::Decided, fleet_core::ipc_error::IpcError>,
) -> Result<ExitCode, String> {
    let d = r.map_err(|e| e.message)?;
    for id in &d.applied {
        out::line(&format!("applied #{id}"));
    }
    for id in &d.rejected {
        out::line(&format!("rejected #{id}"));
    }
    for f in &d.failed {
        out::error(&format!("#{}: {}", f.id, f.error));
    }
    Ok(if d.failed.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct T {
        #[command(subcommand)]
        cmd: SettingsCmd,
    }

    #[test]
    fn a_why_prints_as_one_line_without_escapes() {
        let line = why_line("ok\n#7  decide.jev.enabled: true → false  (person)\u{1b}[1A");
        assert_eq!(line.lines().count(), 1, "{line:?}");
        assert!(!line.chars().any(char::is_control), "{line:?}");
    }

    #[test]
    fn apply_and_reject_need_an_id() {
        assert!(T::try_parse_from(["t", "apply"]).is_err());
        let t = T::try_parse_from(["t", "reject", "3", "4"]).unwrap();
        assert!(matches!(t.cmd, SettingsCmd::Reject { ids } if ids == [3, 4]));
    }

    #[test]
    fn apply_writes_the_value_as_a_person_through_the_database() {
        let dir = tempfile::tempdir().unwrap();
        let opts = HubOptions {
            data_dir: Some(dir.path().to_path_buf()),
            ..HubOptions::default()
        };
        let env = HashMap::new();
        let path = dir.path().join("state.db");
        let id = {
            let s =
                Store::open_with_bus(&path, Arc::new(fleet_core::events::NoopEventBus)).unwrap();
            settings_review::propose(
                &s,
                "work.recent_days",
                "3",
                None,
                fleet_core::service::settings::Actor::Agent("control API"),
            )
            .unwrap()
            .id
        };
        assert_eq!(
            run(SettingsCmd::Apply { ids: vec![id] }, &opts, &env).unwrap(),
            ExitCode::SUCCESS
        );
        assert_eq!(
            run(SettingsCmd::Apply { ids: vec![id] }, &opts, &env).unwrap(),
            ExitCode::FAILURE,
            "already applied"
        );
        let s = open(&opts, &env).unwrap();
        let h = settings_review::history(&s, "work.recent_days", None).unwrap();
        assert_eq!(
            (h[0].actor.as_str(), h[0].proposal_id),
            ("person", Some(id))
        );
    }
}
