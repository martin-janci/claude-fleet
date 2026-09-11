//! Tauri IPC wrappers for the MCP control-API Settings panel.
//!
//! `mcp_status` reports the current state; `mcp_configure` persists the
//! enable/port/token settings and starts or stops the server live (no app
//! restart). The server logic lives in the top-level `mcp` module.
//!
//! Track B additions: per-host token management (`list_host_tokens`,
//! `set_host_token_mode`, `rotate_host_token`), the destructive-call
//! confirmation toggle (`confirm_destructive` on `mcp_configure`) and its
//! answer path (`mcp_confirm`, `mcp_pending_confirms`).

use crate::cancel::CancellationRegistry;
use crate::ipc_error::IpcError;
use crate::mcp::{self, McpGuards, McpRuntime};
use crate::ssh::SshClient;
use crate::store::Store;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use tauri::State;

#[derive(Serialize)]
pub struct McpStatus {
    /// The persisted on/off setting — may be `true` even when `running` is
    /// `false` (e.g. the configured port was already in use).
    pub enabled: bool,
    /// Whether the server is actually listening right now.
    pub running: bool,
    pub port: u16,
    /// The master bearer token (desktop / local clients). Generated on first
    /// read. Provisioned hosts get their own tokens — see `list_host_tokens`.
    pub token: String,
    /// Convenience: the full streamable-HTTP endpoint URL.
    pub url: String,
    /// The most recent start failure, if any.
    pub bind_error: Option<String>,
    /// `mcp.confirm_destructive`: every tool in `mcp::guard::CONFIRM_TOOLS`
    /// (broadcast, kill, delete_worktree, set_clipboard, repair_session,
    /// cancel_task, move_session) needs a desktop confirmation. Default off.
    pub confirm_destructive: bool,
}

#[derive(Deserialize)]
pub struct McpConfigureArgs {
    /// Desired on/off state.
    pub enabled: bool,
    /// New localhost port; `None` keeps the current one.
    pub port: Option<u16>,
    /// When `true`, mint a fresh master token (invalidates existing clients).
    pub regenerate_token: bool,
    /// New value for `mcp.confirm_destructive`; `None` keeps the current one.
    #[serde(default)]
    pub confirm_destructive: Option<bool>,
}

/// Read the persisted settings + live runtime into an `McpStatus`. Generates
/// and persists a token on first call so the UI always has one to display.
fn status(store: &Mutex<Store>, runtime: &Mutex<McpRuntime>) -> Result<McpStatus, IpcError> {
    let (enabled, port, token, confirm_destructive) = {
        let s = store.lock().map_err(|_| IpcError::lock())?;
        let enabled = s.get_setting(mcp::SETTING_ENABLED)?.as_deref() == Some("true");
        let port = s
            .get_setting(mcp::SETTING_PORT)?
            .and_then(|p| p.parse::<u16>().ok())
            .unwrap_or(mcp::DEFAULT_PORT);
        let token = match s.get_setting(mcp::SETTING_TOKEN)? {
            Some(t) if !t.is_empty() => t,
            _ => {
                let fresh = mcp::generate_token();
                s.set_setting(mcp::SETTING_TOKEN, &fresh)?;
                fresh
            }
        };
        let confirm = s
            .get_setting(mcp::guard::SETTING_CONFIRM_DESTRUCTIVE)?
            .as_deref()
            == Some("true");
        (enabled, port, token, confirm)
    };
    let rt = runtime.lock().map_err(|_| IpcError::lock())?;
    Ok(McpStatus {
        enabled,
        running: rt.is_running(),
        port,
        token,
        url: format!("http://127.0.0.1:{port}/mcp"),
        bind_error: rt.last_error().map(str::to_string),
        confirm_destructive,
    })
}

#[tauri::command]
pub fn mcp_status(
    store: State<'_, Arc<Mutex<Store>>>,
    runtime: State<'_, Mutex<McpRuntime>>,
) -> Result<McpStatus, IpcError> {
    status(&store, &runtime)
}

#[tauri::command]
pub async fn mcp_configure(
    args: McpConfigureArgs,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
    reg: State<'_, Arc<CancellationRegistry>>,
    runtime: State<'_, Mutex<McpRuntime>>,
    tunnels: State<'_, Arc<crate::service::tunnel::TunnelSupervisor>>,
    guards: State<'_, McpGuards>,
) -> Result<McpStatus, IpcError> {
    // 1. Persist the requested settings.
    {
        let s = store.lock().map_err(|_| IpcError::lock())?;
        if let Some(p) = args.port {
            s.set_setting(mcp::SETTING_PORT, &p.to_string())?;
        }
        if args.regenerate_token {
            s.set_setting(mcp::SETTING_TOKEN, &mcp::generate_token())?;
        }
        if let Some(c) = args.confirm_destructive {
            s.set_setting(
                mcp::guard::SETTING_CONFIRM_DESTRUCTIVE,
                if c { "true" } else { "false" },
            )?;
        }
        s.set_setting(
            mcp::SETTING_ENABLED,
            if args.enabled { "true" } else { "false" },
        )?;
    }

    // 2. Stop whatever is running — a port/token change is applied by restart.
    {
        let mut rt = runtime.lock().map_err(|_| IpcError::lock())?;
        rt.stop();
    }
    if !args.enabled {
        tunnels.stop_all();
    }

    // 3. If enabled, (re)start with the persisted port + token.
    if args.enabled {
        let (port, token) = {
            let s = store.lock().map_err(|_| IpcError::lock())?;
            let port = s
                .get_setting(mcp::SETTING_PORT)?
                .and_then(|p| p.parse::<u16>().ok())
                .unwrap_or(mcp::DEFAULT_PORT);
            let token = match s.get_setting(mcp::SETTING_TOKEN)? {
                Some(t) if !t.is_empty() => t,
                _ => {
                    let fresh = mcp::generate_token();
                    s.set_setting(mcp::SETTING_TOKEN, &fresh)?;
                    fresh
                }
            };
            (port, token)
        };
        let result = mcp::start(
            Arc::clone(&store),
            Arc::clone(&ssh),
            Arc::clone(&reg),
            Arc::clone(&tunnels),
            guards.inner().clone(),
            port,
            token,
        )
        .await;
        let mut rt = runtime.lock().map_err(|_| IpcError::lock())?;
        match result {
            Ok(shutdown) => {
                // Re-establish tunnels for already-provisioned hosts (best-effort).
                if let Err(e) =
                    crate::service::provision::reestablish_tunnels(&store, &tunnels, port)
                {
                    tracing::warn!(error = %e, "[mcp] re-establishing host tunnels failed");
                }
                rt.set_running(shutdown);
            }
            Err(e) => rt.set_error(e),
        }
    }

    // 4. Return the resulting status.
    status(&store, &runtime)
}

/// Read the configured port, refusing when the control API has never been
/// enabled (no master token yet — nothing to provision against).
fn configured_port(store: &Mutex<Store>) -> Result<u16, IpcError> {
    let s = store.lock().map_err(|_| IpcError::lock())?;
    let has_master = s
        .get_setting(mcp::SETTING_TOKEN)?
        .is_some_and(|t| !t.is_empty());
    if !has_master {
        return Err(IpcError::new(
            "E_PROVISION",
            "enable the control API first (no token yet)",
        ));
    }
    Ok(s.get_setting(mcp::SETTING_PORT)?
        .and_then(|p| p.parse().ok())
        .unwrap_or(mcp::DEFAULT_PORT))
}

#[tauri::command]
pub async fn provision_hosts(
    rotate: Option<bool>,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
    tunnels: State<'_, Arc<crate::service::tunnel::TunnelSupervisor>>,
) -> Result<Vec<crate::service::provision::HostProvisionResult>, IpcError> {
    let port = configured_port(&store)?;
    crate::service::provision::provision_hosts(
        &store,
        &*ssh,
        &tunnels,
        port,
        rotate.unwrap_or(false),
    )
    .await
}

// ---------------------------------------------------------------------------
// Per-host tokens
// ---------------------------------------------------------------------------

/// A host's token row WITHOUT the token itself — the frontend only needs to
/// know that one exists, its mode, and when it was minted.
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct HostTokenInfo {
    pub host_alias: String,
    /// `full` | `readonly`.
    pub mode: String,
    pub created_at: i64,
}

impl From<crate::store::HostTokenRow> for HostTokenInfo {
    fn from(r: crate::store::HostTokenRow) -> Self {
        Self {
            host_alias: r.host_alias,
            mode: r.mode,
            created_at: r.created_at,
        }
    }
}

#[tauri::command]
pub fn list_host_tokens(
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<Vec<HostTokenInfo>, IpcError> {
    let s = store.lock().map_err(|_| IpcError::lock())?;
    Ok(s.list_host_tokens()?
        .into_iter()
        .map(HostTokenInfo::from)
        .collect())
}

/// Pure: validate a mode string from the frontend.
pub fn parse_mode(mode: &str) -> Result<&'static str, IpcError> {
    match mode {
        "full" => Ok("full"),
        "readonly" => Ok("readonly"),
        other => Err(IpcError::new(
            "E_INVALID",
            format!("token mode must be 'full' or 'readonly', got {other:?}"),
        )),
    }
}

#[tauri::command]
pub fn set_host_token_mode(
    host_alias: String,
    mode: String,
    store: State<'_, Arc<Mutex<Store>>>,
) -> Result<HostTokenInfo, IpcError> {
    crate::validate::host_alias(&host_alias)?;
    let mode = parse_mode(&mode)?;
    let s = store.lock().map_err(|_| IpcError::lock())?;
    s.set_host_token_mode(&host_alias, mode)?;
    s.get_host_token(&host_alias)?
        .map(HostTokenInfo::from)
        .ok_or_else(|| IpcError::new("E_NOTFOUND", "host token vanished"))
}

/// Mint a fresh token for one host and re-provision it (the new token is
/// written to the host's `~/.claude.json` + hook block and only persisted
/// once that succeeded, so an unreachable host keeps its old token).
#[tauri::command]
pub async fn rotate_host_token(
    host_alias: String,
    store: State<'_, Arc<Mutex<Store>>>,
    ssh: State<'_, Arc<SshClient>>,
    tunnels: State<'_, Arc<crate::service::tunnel::TunnelSupervisor>>,
) -> Result<HostTokenInfo, IpcError> {
    crate::validate::host_alias(&host_alias)?;
    let port = configured_port(&store)?;
    crate::service::provision::provision_host_with_token(
        &store,
        &*ssh,
        &tunnels,
        &host_alias,
        port,
        true,
    )
    .await?;
    let s = store.lock().map_err(|_| IpcError::lock())?;
    s.get_host_token(&host_alias)?
        .map(HostTokenInfo::from)
        .ok_or_else(|| IpcError::new("E_NOTFOUND", "host token vanished"))
}

// ---------------------------------------------------------------------------
// Destructive-call confirmation
// ---------------------------------------------------------------------------

/// Answer a `mcp:confirm-required` prompt. Returns `false` when the nonce is
/// unknown or expired (the agent will be handed a fresh one on retry).
#[tauri::command]
pub fn mcp_confirm(
    nonce: String,
    approved: bool,
    guards: State<'_, McpGuards>,
) -> Result<bool, IpcError> {
    Ok(guards.confirms.resolve(&nonce, approved))
}

#[derive(Serialize)]
pub struct PendingConfirm {
    pub nonce: String,
    pub tool: String,
}

/// Outstanding confirmation requests (oldest first) — lets the desktop
/// re-render its queue after a reload.
#[tauri::command]
pub fn mcp_pending_confirms(guards: State<'_, McpGuards>) -> Result<Vec<PendingConfirm>, IpcError> {
    Ok(guards
        .confirms
        .pending_tools()
        .into_iter()
        .map(|(nonce, tool)| PendingConfirm { nonce, tool })
        .collect())
}

// ---------------------------------------------------------------------------
// Hook installation
// ---------------------------------------------------------------------------

/// Seconds Claude Code waits for the hook endpoint before giving up. The
/// server answers in milliseconds; a down server must not stall a turn.
const HOOK_TIMEOUT_SECS: u32 = 5;

/// The fleet `/hook` URL for a given port. Identical on every host: on a
/// remote host `127.0.0.1:<port>` is the reverse tunnel's loopback end.
pub(crate) fn hook_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/hook")
}

/// One Claude Code `type: "http"` hook entry. The bearer token rides in an
/// `Authorization` header — never in a process argv (SEC-3) — and the
/// settings file that carries it is written 0600.
pub(crate) fn hook_entry(port: u16, token: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "http",
        "url": hook_url(port),
        "headers": { "Authorization": format!("Bearer {token}") },
        "timeout": HOOK_TIMEOUT_SECS
    })
}

/// Build the hook entry array for one event type.
fn build_hook_block(port: u16, token: &str, matcher: &str) -> serde_json::Value {
    serde_json::json!([{
        "matcher": matcher,
        "hooks": [hook_entry(port, token)]
    }])
}

/// Build the full settings fragment (for tests / docs; the real install
/// merges into the existing file).
#[allow(dead_code)]
pub fn build_hook_config(port: u16, token: &str) -> String {
    let v = serde_json::json!({
        "hooks": {
            "Stop": build_hook_block(port, token, ""),
            "UserPromptSubmit": build_hook_block(port, token, ""),
            "PostToolUse": build_hook_block(port, token, WORKTREE_TOOL_MATCHER)
        }
    });
    serde_json::to_string_pretty(&v).unwrap()
}

/// Matcher of fleet's PostToolUse hook (Claude Code matchers are regexes):
/// `EnterWorktree` registers a worktree row for the calling host,
/// `ExitWorktree` with `action: "remove"` drops it. The `WorktreeCreate` /
/// `WorktreeRemove` hook EVENTS are deliberately never installed: they
/// replace git's own worktree creation / removal, so an http hook there would
/// break it on every host.
pub(crate) const WORKTREE_TOOL_MATCHER: &str = "EnterWorktree|ExitWorktree";

/// The hook events fleet installs, with their matcher. `Stop` is the
/// completion signal (turn over → idle, `turn_seq` bump), `UserPromptSubmit`
/// the busy signal (turn starting → working), `PostToolUse(EnterWorktree|
/// ExitWorktree)` the worktree registration and removal.
pub(crate) const FLEET_HOOK_EVENTS: &[(&str, &str)] = &[
    ("Stop", ""),
    ("UserPromptSubmit", ""),
    ("PostToolUse", WORKTREE_TOOL_MATCHER),
];

/// Pure merge: given `existing` (current `~/.claude/settings.json` content,
/// possibly empty), return new pretty-JSON with fleet's Stop +
/// UserPromptSubmit + PostToolUse(EnterWorktree|ExitWorktree) http hooks
/// installed/refreshed.
///
/// Any prior fleet hook entries pointing at the same port URL — the current
/// `type: "http"` form OR the pre-Track-B `curl … /hook?token=` command form
/// — are stripped first so re-running stays idempotent and upgrades old
/// installs in place. Other hooks (e.g. the user's own tsc pre-commit) are
/// preserved verbatim.
///
/// Shared by the local `install_fleet_hook` command and
/// `provision::provision_hook` for remote hosts.
pub(crate) fn merge_hook_into_settings_json(
    existing: &str,
    port: u16,
    token: &str,
) -> Result<String, IpcError> {
    let mut settings: serde_json::Value = if existing.trim().is_empty() {
        serde_json::json!({})
    } else {
        // Never "repair" a settings.json we cannot parse: it holds the user's
        // permissions, env and their own hooks, and replacing it with `{}`
        // would silently drop all of that. Refuse (the host is reported
        // failed) and let the user fix the file — same policy as
        // `merge_mcp_entry` for ~/.claude.json.
        serde_json::from_str(existing).map_err(|e| {
            IpcError::new(
                "E_PROVISION",
                format!("~/.claude/settings.json is not valid JSON, refusing to overwrite it: {e}"),
            )
        })?
    };

    if !settings.is_object() {
        return Err(IpcError::new(
            "E_PARSE",
            "settings.json root is not a JSON object",
        ));
    }

    let fleet_prefix = hook_url(port);

    let strip_fleet = |arr: &serde_json::Value| -> serde_json::Value {
        let items = arr.as_array().cloned().unwrap_or_default();
        serde_json::Value::Array(
            items
                .into_iter()
                .filter(|block| {
                    let hooks_arr = block.get("hooks").and_then(|h| h.as_array());
                    hooks_arr.is_none_or(|hs| {
                        !hs.iter().any(|h| {
                            let url_match = h
                                .get("url")
                                .and_then(|u| u.as_str())
                                .is_some_and(|u| u.starts_with(&fleet_prefix));
                            let cmd_match = h
                                .get("command")
                                .and_then(|c| c.as_str())
                                .is_some_and(|c| c.contains(&fleet_prefix));
                            url_match || cmd_match
                        })
                    })
                })
                .collect(),
        )
    };

    let hooks = settings
        .as_object_mut()
        .unwrap()
        .entry("hooks")
        .or_insert(serde_json::json!({}))
        .as_object_mut()
        .ok_or_else(|| IpcError::new("E_PARSE", "hooks is not an object"))?;

    for (event, matcher) in FLEET_HOOK_EVENTS {
        let mut arr = strip_fleet(hooks.get(*event).unwrap_or(&serde_json::json!([])));
        arr.as_array_mut().unwrap().push(serde_json::json!({
            "matcher": matcher,
            "hooks": [hook_entry(port, token)]
        }));
        hooks.insert((*event).to_string(), arr);
    }

    serde_json::to_string_pretty(&settings).map_err(|e| IpcError::new("E_SERIALIZE", e.to_string()))
}

/// Install (or update) the fleet hook in the local `~/.claude/settings.json`.
///
/// Uses the `local` host's own per-host token (minted here if the host was
/// never provisioned), so hook traffic from local sessions is attributed to
/// `local` like any other host. The file is written 0600 because it now
/// carries the token.
///
/// Only supports `host_alias == "local"` — remote hosts get their hook block
/// from `provision_hosts`, which also sets up the tunnel the hook needs.
#[tauri::command]
pub fn install_fleet_hook(
    host_alias: String,
    store: State<'_, Arc<Mutex<Store>>>,
    runtime: State<'_, Mutex<McpRuntime>>,
) -> Result<String, IpcError> {
    if host_alias != "local" {
        return Err(IpcError::new(
            "E_UNSUPPORTED",
            "install_fleet_hook only supports the local host; use Provision hosts for remote ones",
        ));
    }

    let (port, token) = {
        let s = store.lock().map_err(|_| IpcError::lock())?;
        let port = s
            .get_setting(mcp::SETTING_PORT)?
            .and_then(|p| p.parse::<u16>().ok())
            .unwrap_or(mcp::DEFAULT_PORT);
        let has_master = s
            .get_setting(mcp::SETTING_TOKEN)?
            .is_some_and(|t| !t.is_empty());
        if !has_master {
            return Err(IpcError::new(
                "E_NO_TOKEN",
                "MCP token not configured — enable the MCP server first",
            ));
        }
        let token = match s.get_host_token("local")? {
            Some(row) => row.token,
            None => {
                let fresh = mcp::generate_token();
                s.upsert_host_token("local", &fresh)?;
                fresh
            }
        };
        (port, token)
    };

    {
        let rt = runtime.lock().map_err(|_| IpcError::lock())?;
        if !rt.is_running() {
            return Err(IpcError::new(
                "E_NOT_RUNNING",
                "MCP server is not running — enable it in Settings > MCP first",
            ));
        }
    }

    let settings_path = dirs::home_dir()
        .ok_or_else(|| IpcError::new("E_HOME", "cannot determine home directory"))?
        .join(".claude")
        .join("settings.json");

    let existing = if settings_path.exists() {
        std::fs::read_to_string(&settings_path)
            .map_err(|e| IpcError::new("E_IO", format!("read settings.json: {e}")))?
    } else {
        String::new()
    };

    let merged = merge_hook_into_settings_json(&existing, port, &token)?;

    if let Some(parent) = settings_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| IpcError::new("E_IO", format!("create .claude dir: {e}")))?;
    }
    // Back up the previous file before touching it (it may carry the user's
    // permissions/env), then write the merged file 0600 from creation.
    if !existing.trim().is_empty() {
        let bak = settings_path.with_extension("json.fleet-bak");
        crate::service::provision::write_private_file(&bak, &existing)
            .map_err(|e| IpcError::new("E_IO", format!("write settings.json.fleet-bak: {e}")))?;
    }
    crate::service::provision::write_private_file(&settings_path, &merged)
        .map_err(|e| IpcError::new("E_IO", format!("write settings.json: {e}")))?;

    Ok(format!(
        "Hook installed at {} (http hook, bearer header)\nSettings written to {}",
        hook_url(port),
        settings_path.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_entry_is_an_http_hook_with_bearer_header_and_no_token_in_url() {
        let e = hook_entry(4180, "tok");
        assert_eq!(e["type"], "http");
        assert_eq!(e["url"], "http://127.0.0.1:4180/hook");
        assert_eq!(e["headers"]["Authorization"], "Bearer tok");
        assert_eq!(e["timeout"], HOOK_TIMEOUT_SECS);
        assert!(e.get("command").is_none(), "no argv form: {e}");
        assert!(
            !e["url"].as_str().unwrap().contains("tok"),
            "token must not be in the URL: {e}"
        );
    }

    #[test]
    fn build_hook_config_produces_valid_json() {
        let cfg = build_hook_config(4180, "abc");
        let v: serde_json::Value = serde_json::from_str(&cfg).unwrap();
        assert!(v["hooks"]["Stop"].is_array());
        assert!(v["hooks"]["UserPromptSubmit"].is_array());
        assert!(v["hooks"]["PostToolUse"].is_array());
        assert_eq!(v["hooks"]["UserPromptSubmit"][0]["matcher"], "");
        assert_eq!(
            v["hooks"]["UserPromptSubmit"][0]["hooks"][0]["url"],
            "http://127.0.0.1:4180/hook"
        );
        let h = &v["hooks"]["Stop"][0]["hooks"][0];
        assert_eq!(h["type"], "http");
        assert!(h["url"].as_str().unwrap().contains("4180"));
        assert_eq!(h["headers"]["Authorization"], "Bearer abc");
        assert_eq!(
            v["hooks"]["PostToolUse"][0]["matcher"],
            WORKTREE_TOOL_MATCHER
        );
    }

    #[test]
    fn merge_hook_into_empty_settings() {
        let out = merge_hook_into_settings_json("", 4180, "tok").unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        let stop = v["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 1);
        let h = &stop[0]["hooks"][0];
        assert_eq!(h["type"], "http");
        assert_eq!(h["url"], "http://127.0.0.1:4180/hook");
        assert_eq!(h["headers"]["Authorization"], "Bearer tok");
        let ptu = v["hooks"]["PostToolUse"].as_array().unwrap();
        assert_eq!(ptu[0]["matcher"], WORKTREE_TOOL_MATCHER);
        // The busy signal (Wave 3 Track E) rides the same bearer entry.
        let ups = v["hooks"]["UserPromptSubmit"].as_array().unwrap();
        assert_eq!(ups.len(), 1);
        assert_eq!(ups[0]["matcher"], "");
        assert_eq!(ups[0]["hooks"][0]["headers"]["Authorization"], "Bearer tok");
        assert!(
            !out.contains("token=tok"),
            "token must not appear in a URL: {out}"
        );
    }

    #[test]
    fn merge_hook_adds_user_prompt_submit_to_a_pre_track_e_install() {
        // A host provisioned before Track E has only Stop + PostToolUse; a
        // re-provision must add UserPromptSubmit once and keep it idempotent.
        let pre = merge_hook_into_settings_json("", 4180, "tok").unwrap();
        let mut v: serde_json::Value = serde_json::from_str(&pre).unwrap();
        v["hooks"]
            .as_object_mut()
            .unwrap()
            .remove("UserPromptSubmit");
        let stripped = serde_json::to_string_pretty(&v).unwrap();
        let out = merge_hook_into_settings_json(&stripped, 4180, "tok").unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["hooks"]["UserPromptSubmit"].as_array().unwrap().len(), 1);
        let again = merge_hook_into_settings_json(&out, 4180, "tok").unwrap();
        assert_eq!(out, again);
    }

    #[test]
    fn merge_hook_replaces_a_stale_worktree_create_entry() {
        // Earlier provisions registered PostToolUse(matcher "WorktreeCreate"),
        // which matches no tool. A re-provision must swap it for
        // "EnterWorktree", not add a second fleet entry beside it.
        let stale = serde_json::json!({
            "hooks": { "PostToolUse": [{
                "matcher": "WorktreeCreate",
                "hooks": [hook_entry(4180, "old")]
            }] }
        })
        .to_string();
        let out = merge_hook_into_settings_json(&stale, 4180, "tok").unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        let ptu = v["hooks"]["PostToolUse"].as_array().unwrap();
        assert_eq!(ptu.len(), 1, "{ptu:?}");
        assert_eq!(ptu[0]["matcher"], WORKTREE_TOOL_MATCHER);
        assert!(!out.contains("WorktreeCreate"));
        assert_eq!(
            out,
            merge_hook_into_settings_json(&out, 4180, "tok").unwrap()
        );
    }

    #[test]
    fn merge_hook_preserves_unrelated_hooks_and_is_idempotent() {
        let existing = r#"{
          "hooks": {
            "PostToolUse": [{
              "matcher": "Write|Edit",
              "hooks": [{"type":"command","command":"npx tsc --noEmit"}]
            }]
          },
          "otherTopLevel": {"keep": true}
        }"#;
        let out = merge_hook_into_settings_json(existing, 4180, "tok").unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();

        // User's tsc hook survives.
        let ptu = v["hooks"]["PostToolUse"].as_array().unwrap();
        assert!(
            ptu.iter().any(|b| b["matcher"] == "Write|Edit"),
            "user's Write|Edit hook must survive: {ptu:?}"
        );
        // Fleet's EnterWorktree hook landed.
        assert!(
            ptu.iter().any(|b| b["matcher"] == WORKTREE_TOOL_MATCHER),
            "fleet EnterWorktree hook must be present: {ptu:?}"
        );
        // Unrelated top-level keys preserved.
        assert_eq!(v["otherTopLevel"]["keep"], true);

        // Re-running with same port replaces fleet's entries instead of
        // duplicating them.
        let out2 = merge_hook_into_settings_json(&out, 4180, "tok2").unwrap();
        let v2: serde_json::Value = serde_json::from_str(&out2).unwrap();
        let ptu2 = v2["hooks"]["PostToolUse"].as_array().unwrap();
        let fleet_count = ptu2
            .iter()
            .filter(|b| b["matcher"] == WORKTREE_TOOL_MATCHER)
            .count();
        assert_eq!(
            fleet_count, 1,
            "fleet hook duplicated on re-merge: {ptu2:?}"
        );
        assert_eq!(
            ptu2.iter().filter(|b| b["matcher"] == "Write|Edit").count(),
            1
        );
        // Token updated on the surviving entry.
        let fleet_hdr = ptu2
            .iter()
            .find(|b| b["matcher"] == WORKTREE_TOOL_MATCHER)
            .unwrap()["hooks"][0]["headers"]["Authorization"]
            .as_str()
            .unwrap();
        assert_eq!(fleet_hdr, "Bearer tok2");
        // Byte-for-byte idempotent on a third run with the same token.
        let out3 = merge_hook_into_settings_json(&out2, 4180, "tok2").unwrap();
        assert_eq!(out2, out3);
    }

    #[test]
    fn merge_hook_upgrades_legacy_command_hooks_in_place() {
        // The pre-Track-B install wrote a curl command hook with the token in
        // the URL; a re-provision must replace it, not sit beside it.
        let legacy = r#"{
          "hooks": {
            "Stop": [{
              "matcher": "",
              "hooks": [{"type":"command","command":"curl -sS -X POST --data-binary @- 'http://127.0.0.1:4180/hook?token=old' 2>/dev/null || true"}]
            }]
          }
        }"#;
        let out = merge_hook_into_settings_json(legacy, 4180, "new").unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        let stop = v["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 1, "legacy entry must be replaced: {stop:?}");
        assert_eq!(stop[0]["hooks"][0]["type"], "http");
        assert!(!out.contains("token=old"));
    }

    #[test]
    fn merge_hook_refuses_malformed_settings_json() {
        // A corrupted settings.json must never be replaced with `{}` — that
        // would wipe the user's permissions / env / own hooks. The merge
        // errors before any write and names the file.
        let err = merge_hook_into_settings_json("not json at all", 4180, "tok").unwrap_err();
        assert_eq!(err.code, "E_PROVISION");
        assert!(err.message.contains("settings.json"), "{}", err.message);
        // A truncated file is malformed too.
        let err =
            merge_hook_into_settings_json(r#"{"hooks": {"Stop": ["#, 4180, "tok").unwrap_err();
        assert_eq!(err.code, "E_PROVISION");
        // A non-object root is refused as well (existing E_PARSE path).
        assert!(merge_hook_into_settings_json("[1,2]", 4180, "tok").is_err());
    }

    #[test]
    fn parse_mode_accepts_only_known_modes() {
        assert_eq!(parse_mode("full").unwrap(), "full");
        assert_eq!(parse_mode("readonly").unwrap(), "readonly");
        assert_eq!(parse_mode("admin").unwrap_err().code, "E_INVALID");
        assert_eq!(parse_mode("").unwrap_err().code, "E_INVALID");
    }

    #[test]
    fn host_token_info_never_carries_the_token() {
        let info = HostTokenInfo::from(crate::store::HostTokenRow {
            host_alias: "mefistos".into(),
            token: "s3cret".into(),
            created_at: 7,
            mode: "readonly".into(),
        });
        let json = serde_json::to_string(&info).unwrap();
        assert!(!json.contains("s3cret"), "{json}");
        assert_eq!(
            info,
            HostTokenInfo {
                host_alias: "mefistos".into(),
                mode: "readonly".into(),
                created_at: 7
            }
        );
    }
}
