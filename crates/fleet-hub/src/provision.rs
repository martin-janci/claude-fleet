//! `fleet-hub provision [--host <alias>] [--content-only]` — the
//! `provision_hosts` tool over loopback with the master token, so the
//! hub-client desktop's refusal ("provision from the hub with `fleet-hub`")
//! names something that exists (hosts F1).

use crate::config::HubOptions;
use crate::out;
use crate::pair::{call_tool, hub_conn};
use serde_json::json;
use std::collections::HashMap;
use std::process::ExitCode;

pub async fn run(
    host: Option<String>,
    content_only: bool,
    opts: &HubOptions,
    env: &HashMap<String, String>,
) -> Result<ExitCode, String> {
    let conn = hub_conn(opts, env)?;
    let v = call_tool(
        &conn,
        "provision_hosts",
        json!({ "host": host, "content_only": content_only }),
    )
    .await?;
    let mut failed = false;
    for r in v.as_array().into_iter().flatten() {
        let host = r["host"].as_str().unwrap_or("?");
        let status = r["status"].as_str().unwrap_or("?");
        let detail = r["detail"]
            .as_str()
            .map(|d| format!(" — {d}"))
            .unwrap_or_default();
        failed |= status == "failed";
        out::line(&format!("{host}: {status}{detail}"));
    }
    Ok(if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}
