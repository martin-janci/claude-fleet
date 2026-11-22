//! `fleet-hub project …` — the hub operator's project administration. One
//! tool call over loopback with the master token, like `fleet-hub host …`.

use crate::config::HubOptions;
use crate::out;
use crate::pair::{call_tool, hub_conn};
use clap::Subcommand;
use serde_json::json;
use std::collections::HashMap;
use std::process::ExitCode;

#[derive(Subcommand, Debug)]
pub enum ProjectCmd {
    /// Fold one project into another (sessions, worktrees, pull requests,
    /// work, rules), then delete it. For a repository that changed owner or
    /// name and was registered twice.
    Merge {
        /// Id of the project to retire.
        from: i64,
        /// Id of the project that keeps its rows.
        into: i64,
    },
}

pub async fn run(
    cmd: ProjectCmd,
    opts: &HubOptions,
    env: &HashMap<String, String>,
) -> Result<ExitCode, String> {
    let conn = hub_conn(opts, env)?;
    match cmd {
        ProjectCmd::Merge { from, into } => {
            let v = call_tool(
                &conn,
                "merge_project",
                json!({ "from": from, "into": into }),
            )
            .await?;
            out::line(&format!(
                "merged {} into {}: {} sessions and {} worktrees moved, {} worktrees folded, {} other rows moved",
                v["from_name"].as_str().unwrap_or("?"),
                v["into_name"].as_str().unwrap_or("?"),
                v["sessions_moved"],
                v["worktrees_moved"],
                v["worktrees_folded"],
                v["other_rows_moved"]
            ));
            if let Some(prefix) = v["ticket_space_left"].as_str() {
                out::line(&format!(
                    "the ticket space {prefix} stayed with the old project ({} has its own); its keys still resolve",
                    v["into_name"].as_str().unwrap_or("?")
                ));
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}
