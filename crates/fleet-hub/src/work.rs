//! `fleet-hub work …` — the hub operator's side of the work graph's
//! read-only administration (work graph M13.2). Each subcommand is one
//! `work_admin` call over loopback with the master token.

use crate::config::HubOptions;
use crate::out;
use crate::pair::{call_tool, hub_conn};
use clap::Subcommand;
use fleet_core::service::work::usage::UsageSummary;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::process::ExitCode;

#[derive(Subcommand, Debug)]
pub enum WorkCmd {
    /// How the work graph is used: links, detection, handovers, resumes,
    /// briefs, tidy-up and tracker passes, as counts and ids only. Nothing
    /// leaves the hub. Paste it into an acceptance run's record.
    Usage {
        /// The window in days (1-365). [default: 30]
        #[arg(long)]
        days: Option<i64>,
        /// Print the answer as JSON instead of lines.
        #[arg(long)]
        json: bool,
    },
}

/// The `work_admin` arguments for `usage`.
fn usage_args(days: Option<i64>) -> Value {
    let mut args = json!({ "action": "usage" });
    if let Some(d) = days {
        args["days"] = json!(d);
    }
    args
}

pub async fn run(
    cmd: WorkCmd,
    opts: &HubOptions,
    env: &HashMap<String, String>,
) -> Result<ExitCode, String> {
    let conn = hub_conn(opts, env)?;
    match cmd {
        WorkCmd::Usage { days, json } => {
            let v = call_tool(&conn, "work_admin", usage_args(days)).await?;
            if json {
                out::line(&serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?);
            } else {
                let u: UsageSummary = serde_json::from_value(v)
                    .map_err(|e| format!("the hub's usage answer did not parse: {e}"))?;
                for l in u.lines() {
                    out::line(&l);
                }
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_sends_days_only_when_given() {
        assert_eq!(usage_args(None), json!({ "action": "usage" }));
        assert_eq!(usage_args(Some(7)), json!({ "action": "usage", "days": 7 }));
    }

    #[test]
    fn usage_parses_on_the_command_line() {
        use clap::Parser;
        #[derive(Parser)]
        struct T {
            #[command(subcommand)]
            cmd: WorkCmd,
        }
        let t = T::try_parse_from(["t", "usage", "--days", "90", "--json"]).unwrap();
        assert!(matches!(
            t.cmd,
            WorkCmd::Usage {
                days: Some(90),
                json: true
            }
        ));
    }
}
