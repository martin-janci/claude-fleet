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
        /// Print what would move and write nothing.
        #[arg(long)]
        dry_run: bool,
    },
}

pub async fn run(
    cmd: ProjectCmd,
    opts: &HubOptions,
    env: &HashMap<String, String>,
) -> Result<ExitCode, String> {
    let conn = hub_conn(opts, env)?;
    match cmd {
        ProjectCmd::Merge {
            from,
            into,
            dry_run,
        } => {
            let v = call_tool(
                &conn,
                "merge_project",
                json!({ "from": from, "into": into, "dry_run": dry_run }),
            )
            .await?;
            out::line(&merge_summary(&v));
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

/// The one line `project merge` prints: what moved, or with `--dry-run`
/// what would.
fn merge_summary(v: &serde_json::Value) -> String {
    let n = |k: &str| v[k].as_u64().unwrap_or(0);
    let head = if v["dry_run"].as_bool() == Some(true) {
        "would merge"
    } else {
        "merged"
    };
    let tail = if v["dry_run"].as_bool() == Some(true) {
        " (dry run: nothing written)"
    } else {
        ""
    };
    format!(
        "{head} {} into {}: {} sessions and {} worktrees moved, {} worktrees folded, {} other rows \
         moved, {} picks, {} local workspaces moved ({} dropped), {} agent inputs renamed, {} review \
         rules and {} routine repo filters moved{tail}",
        v["from_name"].as_str().unwrap_or("?"),
        v["into_name"].as_str().unwrap_or("?"),
        n("sessions_moved"),
        n("worktrees_moved"),
        n("worktrees_folded"),
        n("other_rows_moved"),
        n("picks_moved"),
        n("workspaces_moved"),
        n("workspaces_dropped"),
        n("agent_inputs_moved"),
        n("review_rules_moved"),
        n("routine_filters_moved"),
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_dry_run_says_it_wrote_nothing() {
        let v = serde_json::json!({
            "from_name": "a/x", "into_name": "b/x", "sessions_moved": 2,
            "picks_moved": 1, "dry_run": true,
        });
        let line = super::merge_summary(&v);
        assert!(
            line.starts_with("would merge a/x into b/x: 2 sessions"),
            "{line}"
        );
        assert!(line.ends_with("(dry run: nothing written)"), "{line}");
        let line =
            super::merge_summary(&serde_json::json!({ "from_name": "a/x", "into_name": "b/x" }));
        assert!(
            line.starts_with("merged a/x into b/x") && !line.contains("dry run"),
            "{line}"
        );
    }
}
