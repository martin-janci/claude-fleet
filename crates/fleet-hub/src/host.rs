//! `fleet-hub host …` — the hub operator's host administration that the
//! desktop refuses in hub-client mode. One tool call over loopback with the
//! master token, like `fleet-hub org …`.

use crate::config::HubOptions;
use crate::out;
use crate::pair::{call_tool, hub_conn};
use clap::Subcommand;
use serde_json::json;
use std::collections::HashMap;
use std::process::ExitCode;

#[derive(Subcommand, Debug)]
pub enum HostCmd {
    /// Fold one host alias into another (worktrees, sessions, usage), then delete it.
    Merge {
        /// The alias to retire.
        from: String,
        /// The alias that keeps its rows.
        into: String,
    },
}

pub async fn run(
    cmd: HostCmd,
    opts: &HubOptions,
    env: &HashMap<String, String>,
) -> Result<ExitCode, String> {
    let conn = hub_conn(opts, env)?;
    match cmd {
        HostCmd::Merge { from, into } => {
            let v = call_tool(&conn, "merge_host", json!({ "from": from, "into": into }))
                .await
                .map_err(with_confirm_advice)?;
            out::line(&format!(
                "merged {from} into {into}: {} worktrees, {} sessions moved, {} dropped, {} usage days summed",
                v["worktrees_moved"], v["sessions_moved"], v["sessions_dropped"], v["usage_days_merged"]
            ));
            Ok(ExitCode::SUCCESS)
        }
    }
}

/// `merge_host` is confirm-gated, and a hub has no approver to confirm it:
/// an `E_CONFIRM_REQUIRED` refusal says what to do instead.
fn with_confirm_advice(e: String) -> String {
    if e.contains("E_CONFIRM_REQUIRED") {
        format!(
            "{e}\nmerge_host is confirm-gated and a hub has no approver: turn \
             mcp.confirm_destructive off, merge, then turn it back on if you want it"
        )
    } else {
        e
    }
}

#[cfg(test)]
mod tests {
    use super::with_confirm_advice;

    #[test]
    fn a_confirm_refusal_says_how_to_merge_on_a_hub() {
        let e = with_confirm_advice(r#"{"code":"E_CONFIRM_REQUIRED"}"#.into());
        assert!(e.contains("mcp.confirm_destructive off"), "{e}");
        assert_eq!(with_confirm_advice("E_NOT_FOUND".into()), "E_NOT_FOUND");
    }
}
