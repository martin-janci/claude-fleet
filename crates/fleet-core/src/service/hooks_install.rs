//! Installing fleet's Claude Code hooks into a `~/.claude/settings.json`.
//!
//! The hook block (`Stop`, `UserPromptSubmit`, `PostToolUse(EnterWorktree|
//! ExitWorktree)`, `SessionEnd`, `StopFailure`, `Notification`, `PreCompact`,
//! `PostCompact`) is a set of `type: "http"` hooks pointing at fleet's
//! `/hook` endpoint with the host's bearer token in an `Authorization`
//! header and the calling tmux pane in an `X-Fleet-Pane` header.
//! `SessionStart` cannot be an http hook (Claude Code allows only command /
//! mcp_tool there), so it is installed as a `curl` command hook instead
//! ([`session_start_command`]); its bearer token lives in a 0600 headers file
//! ([`HOOK_HEADERS_FILE`]) rather than in argv or the command string.
//! [`merge_hook_into_settings_json`] is the pure, idempotent merge shared by
//! the local install (`commands::mcp::install_fleet_hook`, the enable-time
//! auto-install) and the remote one (`provision::provision_hook`).

use crate::ipc_error::{codes, lock, IpcError};
use crate::service::hub::HubBase;
use crate::store::Store;
use std::sync::Mutex;

/// Seconds Claude Code waits for the hook endpoint before giving up. The
/// server answers in milliseconds; a down server must not stall a turn.
const HOOK_TIMEOUT_SECS: u32 = 5;

/// One Claude Code `type: "http"` hook entry pointing at `hook_url`
/// (`HubBase::hook_url()`). The bearer token rides in an `Authorization`
/// header — never in a process argv (SEC-3) — and the settings file that
/// carries it is written 0600. `X-Fleet-Pane` carries the calling tmux pane
/// (`$TMUX_PANE`, allow-listed via `allowedEnvVars` since Claude Code does
/// not expand env vars in a header value by default) so the receiving hook
/// can resolve which conversation fired it; an unexpanded literal or empty
/// value is rejected by `mcp::hooks::pane_header`.
pub fn hook_entry(hook_url: &str, token: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "http",
        "url": hook_url,
        "headers": {
            "Authorization": format!("Bearer {token}"),
            "X-Fleet-Pane": "$TMUX_PANE"
        },
        "allowedEnvVars": ["TMUX_PANE"],
        "timeout": HOOK_TIMEOUT_SECS
    })
}

/// True when `h` has the shape of a [`hook_entry`]: `type: "http"`, a `url`
/// of `http(s)://<authority>/hook` with nothing else in the path (every
/// `HubBase` URL has no path of its own), a `Bearer` `Authorization` header
/// and fleet's timeout. A user's own http hook at a deeper path, or with a
/// different timeout, never matches, so it is never stripped. Deliberately
/// does not require `X-Fleet-Pane` / `allowedEnvVars`: the previous entry
/// shape (before the pane header was added) must still match so re-merging
/// upgrades an old install rather than leaving a duplicate beside the new
/// one.
fn is_fleet_hook_entry(h: &serde_json::Value) -> bool {
    let url_is_fleet = h
        .get("url")
        .and_then(|u| u.as_str())
        .and_then(|u| {
            u.strip_prefix("https://")
                .or_else(|| u.strip_prefix("http://"))
        })
        .and_then(|rest| rest.strip_suffix("/hook"))
        .is_some_and(|authority| !authority.is_empty() && !authority.contains(['/', '?', '#']));
    let bearer = h
        .get("headers")
        .and_then(|hd| hd.get("Authorization"))
        .and_then(|a| a.as_str())
        .is_some_and(|a| a.starts_with("Bearer "));
    h.get("type").and_then(|t| t.as_str()) == Some("http")
        && url_is_fleet
        && bearer
        && h.get("timeout").and_then(|t| t.as_u64()) == Some(u64::from(HOOK_TIMEOUT_SECS))
}

/// Matcher of fleet's PostToolUse hook (Claude Code matchers are regexes):
/// `EnterWorktree` registers a worktree row for the calling host,
/// `ExitWorktree` with `action: "remove"` drops it. The `WorktreeCreate` /
/// `WorktreeRemove` hook EVENTS are deliberately never installed: they
/// replace git's own worktree creation / removal, so an http hook there would
/// break it on every host.
pub const WORKTREE_TOOL_MATCHER: &str = "EnterWorktree|ExitWorktree";

/// `SessionEnd` reasons fleet cares about. `logout` / `prompt_input_exit` /
/// `other` mean the Claude process is gone (→ stopped). `clear` and `resume`
/// mean the *conversation* ended while the process lives on under a new
/// session id — `service::hooks::apply_session_end_hook` closes the
/// conversation timeline without marking the row stopped.
pub const SESSION_END_MATCHER: &str = "logout|prompt_input_exit|other|clear|resume";

/// `Notification` types fleet turns into state (see
/// `service::hooks::notification_effect`). `idle_prompt`, `auth_success`,
/// the `elicitation_complete/response` pair and the agent-view-only
/// `agent_*` types carry nothing fleet needs.
pub const NOTIFICATION_MATCHER: &str = "permission_prompt|elicitation_dialog|elicitation_url_dialog|quota_auto_resume_stale|quota_auto_resume_disabled|quota_auto_resume_fired";

/// Whether an event in [`FLEET_HOOK_EVENTS`] is installed as an http hook
/// ([`hook_entry`]) or, for `SessionStart` (the one event Claude Code refuses
/// to fire as `type: "http"`), a `curl` command hook ([`command_hook_entry`]).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum HookKind {
    Http,
    /// Events that accept only command / mcp_tool handlers (SessionStart).
    Command,
}

/// The hook events fleet installs, with their matcher and installation kind.
/// `Stop` is the completion signal (turn over → idle, `turn_seq` bump),
/// `UserPromptSubmit` the busy signal (turn starting → working),
/// `PostToolUse(EnterWorktree|ExitWorktree)` the worktree registration and
/// removal, `SessionEnd` the exit/conversation-end signal, `StopFailure` the
/// API-error completion (→ idle, `turn_seq` bump, `stop_failure` timeline
/// event), `Notification` the waiting-on-a-human signal (→ blocked),
/// `SessionStart` the conversation-open signal (installed as a command hook,
/// see [`session_start_command`]), and `PreCompact`/`PostCompact` bracket a
/// compaction.
pub const FLEET_HOOK_EVENTS: &[(&str, &str, HookKind)] = &[
    ("Stop", "", HookKind::Http),
    ("UserPromptSubmit", "", HookKind::Http),
    ("PostToolUse", WORKTREE_TOOL_MATCHER, HookKind::Http),
    ("SessionEnd", SESSION_END_MATCHER, HookKind::Http),
    ("StopFailure", "", HookKind::Http),
    ("Notification", NOTIFICATION_MATCHER, HookKind::Http),
    ("SessionStart", "", HookKind::Command),
    ("PreCompact", "", HookKind::Http),
    ("PostCompact", "", HookKind::Http),
];

/// Name of the file (in `~/.claude`, mode 0600) holding the bearer header the
/// SessionStart command hook sends with `curl -H @file`. Keeps the token out
/// of argv (SEC-3) and out of the command string in settings.json.
pub const HOOK_HEADERS_FILE: &str = "fleet-hook.headers";

/// The content of [`HOOK_HEADERS_FILE`]: a single `curl`-style header line.
pub fn hook_headers_content(token: &str) -> String {
    format!("Authorization: Bearer {token}\n")
}

/// The SessionStart command: POST the hook body from stdin with the token
/// header file and the pane id. Never fails the session start.
pub fn session_start_command(hook_url: &str) -> String {
    format!(
        "curl -sS -m {HOOK_TIMEOUT_SECS} -o /dev/null -X POST \
         -H @\"$HOME/.claude/{HOOK_HEADERS_FILE}\" \
         -H \"X-Fleet-Pane: ${{TMUX_PANE:-}}\" \
         -H 'Content-Type: application/json' \
         --data-binary @- {} || true",
        crate::shell::quote(hook_url)
    )
}

/// One Claude Code `type: "command"` hook entry for `SessionStart`: a `curl`
/// invocation of [`session_start_command`], run asynchronously so it never
/// stalls the session start.
fn command_hook_entry(hook_url: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "command",
        "command": session_start_command(hook_url),
        "async": true,
        "timeout": HOOK_TIMEOUT_SECS
    })
}

/// The marker of a pre-Track-B command hook: the token in the hook URL.
const LEGACY_TOKEN_HOOK: &str = "/hook?token=";

/// Pure merge: given `existing` (current `~/.claude/settings.json` content,
/// possibly empty), return new pretty-JSON with every event in
/// [`FLEET_HOOK_EVENTS`] installed/refreshed (an http hook for most, a
/// tokenless `curl` command hook for `SessionStart`).
///
/// Any prior fleet hook entries are stripped first so re-running stays
/// idempotent and upgrades old installs in place: every entry of fleet's
/// exact http shape ([`is_fleet_hook_entry`], whatever base URL it points at,
/// so a base-URL change never leaves a stale hook behind), and the
/// pre-Track-B `curl … /hook?token=` command form under any base. Other
/// hooks (e.g. the user's own tsc pre-commit) are preserved verbatim.
///
/// Shared by the local `install_fleet_hook` command (via [`install_hook_at`])
/// and `provision::provision_hook` for remote hosts.
pub fn merge_hook_into_settings_json(
    existing: &str,
    hook_url: &str,
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
                codes::E_PROVISION,
                format!("~/.claude/settings.json is not valid JSON, refusing to overwrite it: {e}"),
            )
        })?
    };

    if !settings.is_object() {
        return Err(IpcError::new(
            codes::E_PARSE,
            "settings.json root is not a JSON object",
        ));
    }

    let fleet_prefix = hook_url.to_string();

    let strip_fleet = |arr: &serde_json::Value| -> serde_json::Value {
        let items = arr.as_array().cloned().unwrap_or_default();
        serde_json::Value::Array(
            items
                .into_iter()
                .filter(|block| {
                    let hooks_arr = block.get("hooks").and_then(|h| h.as_array());
                    hooks_arr.is_none_or(|hs| {
                        !hs.iter().any(|h| {
                            let url_match = is_fleet_hook_entry(h);
                            // A pre-Track-B `curl … /hook?token=` command
                            // hook carries a master token in argv: strip it
                            // whatever base it points at, so a base change
                            // (desktop -> public hub) never leaves one behind.
                            // A SessionStart command hook is fleet's own when
                            // it points at the current base OR references the
                            // headers file (any prior base) — the latter
                            // catches a base-URL change so the old command
                            // entry (and the token file it named) never
                            // survives beside the new one.
                            let cmd_match =
                                h.get("command").and_then(|c| c.as_str()).is_some_and(|c| {
                                    c.contains(&fleet_prefix)
                                        || c.contains(LEGACY_TOKEN_HOOK)
                                        || c.contains(HOOK_HEADERS_FILE)
                                });
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
        .ok_or_else(|| IpcError::new(codes::E_PARSE, "hooks is not an object"))?;

    for (event, matcher, kind) in FLEET_HOOK_EVENTS {
        let mut arr = strip_fleet(hooks.get(*event).unwrap_or(&serde_json::json!([])));
        let entry = match kind {
            HookKind::Http => hook_entry(hook_url, token),
            HookKind::Command => command_hook_entry(hook_url),
        };
        arr.as_array_mut().unwrap().push(serde_json::json!({
            "matcher": matcher,
            "hooks": [entry]
        }));
        hooks.insert((*event).to_string(), arr);
    }

    serde_json::to_string_pretty(&settings)
        .map_err(|e| IpcError::new(codes::E_SERIALIZE, e.to_string()))
}

/// The `local` host's per-host token, minted on first use. Refuses when the
/// control API has never been enabled (no master token yet).
pub fn local_hook_token(store: &Mutex<Store>) -> Result<String, IpcError> {
    let s = lock(store)?;
    if crate::mcp::settings::McpSettings::read(&s)?.token.is_none() {
        return Err(IpcError::new(
            codes::E_NO_TOKEN,
            "MCP token not configured — enable the MCP server first",
        ));
    }
    Ok(match s.get_host_token("local")? {
        Some(row) => row.token,
        None => {
            let fresh = crate::mcp::generate_token();
            s.upsert_host_token("local", &fresh)?;
            fresh
        }
    })
}

/// `~/.claude/settings.json` on this machine.
pub fn local_settings_path() -> Result<std::path::PathBuf, IpcError> {
    Ok(directories::BaseDirs::new()
        .map(|b| b.home_dir().to_path_buf())
        .ok_or_else(|| IpcError::new(codes::E_HOME, "cannot determine home directory"))?
        .join(".claude")
        .join("settings.json"))
}

/// What `install_hook_at` did to the settings file and the SessionStart
/// headers file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookInstall {
    /// Either file was created or (re)written.
    Written,
    /// Both files already carried exactly this hook config; nothing touched.
    Unchanged,
}

/// Merge fleet's hooks into the settings file at `settings_path`, and write
/// the SessionStart command hook's bearer-token headers file beside it
/// (shared by the Settings button and the enable-time auto-install).
/// Idempotent: `Unchanged` only when neither file needed a change (and then
/// neither is touched, nor is any backup). The user's own hooks, permissions
/// and env survive via
/// `merge_hook_into_settings_json`; an unparseable file is refused, never
/// replaced.
pub fn install_hook_at(
    settings_path: &std::path::Path,
    hook_url: &str,
    token: &str,
) -> Result<HookInstall, IpcError> {
    let existing = if settings_path.exists() {
        std::fs::read_to_string(settings_path)
            .map_err(|e| IpcError::new(codes::E_IO, format!("read settings.json: {e}")))?
    } else {
        String::new()
    };

    let merged = merge_hook_into_settings_json(&existing, hook_url, token)?;
    let settings_changed = merged != existing;

    let headers_path = settings_path.with_file_name(HOOK_HEADERS_FILE);
    let headers_content = hook_headers_content(token);
    let existing_headers = if headers_path.exists() {
        std::fs::read_to_string(&headers_path)
            .map_err(|e| IpcError::new(codes::E_IO, format!("read {HOOK_HEADERS_FILE}: {e}")))?
    } else {
        String::new()
    };
    let headers_changed = existing_headers != headers_content;

    if !settings_changed && !headers_changed {
        return Ok(HookInstall::Unchanged);
    }

    if let Some(parent) = settings_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| IpcError::new(codes::E_IO, format!("create .claude dir: {e}")))?;
    }
    if settings_changed {
        // Back up the previous file before touching it (it may carry the
        // user's permissions/env), then write the merged file 0600 from
        // creation.
        if !existing.trim().is_empty() {
            let bak = settings_path.with_extension("json.fleet-bak");
            super::provision::write_private_file(&bak, &existing).map_err(|e| {
                IpcError::new(codes::E_IO, format!("write settings.json.fleet-bak: {e}"))
            })?;
        }
        super::provision::write_private_file(settings_path, &merged)
            .map_err(|e| IpcError::new(codes::E_IO, format!("write settings.json: {e}")))?;
    }
    if headers_changed {
        super::provision::write_private_file(&headers_path, &headers_content)
            .map_err(|e| IpcError::new(codes::E_IO, format!("write {HOOK_HEADERS_FILE}: {e}")))?;
    }
    Ok(HookInstall::Written)
}

/// Enable-time auto-install of the local hook (Q8). Best-effort: a failure
/// (malformed settings.json, unwritable home) is logged and never fails the
/// enable itself; the Settings button still reports the error verbatim.
/// Skipped when `~/.claude` does not exist, i.e. Claude Code was never run
/// on this machine, so fleet does not create a config dir nobody uses.
pub fn auto_install_local_hook(store: &Mutex<Store>, base: &HubBase) {
    let result = (|| -> Result<Option<HookInstall>, IpcError> {
        let path = local_settings_path()?;
        if !path.parent().is_some_and(std::path::Path::is_dir) {
            return Ok(None);
        }
        let token = local_hook_token(store)?;
        install_hook_at(&path, &base.hook_url(), &token).map(Some)
    })();
    match result {
        Ok(Some(HookInstall::Written)) => {
            tracing::info!(
                url = %base.hook_url(),
                "[mcp] installed the fleet hook in local ~/.claude/settings.json"
            );
        }
        Ok(Some(HookInstall::Unchanged)) => {
            tracing::debug!("[mcp] local fleet hook already current");
        }
        Ok(None) => {
            tracing::info!("[mcp] no local ~/.claude dir; skipping hook auto-install");
        }
        Err(e) => {
            tracing::warn!(error = %e.message, "[mcp] local hook auto-install failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_hook_at_creates_then_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".claude").join("settings.json");
        assert_eq!(
            install_hook_at(&path, "http://127.0.0.1:4180/hook", "tok").unwrap(),
            HookInstall::Written
        );
        let first = std::fs::read_to_string(&path).unwrap();
        assert!(first.contains("http://127.0.0.1:4180/hook"));
        // No backup for a file that did not exist.
        assert!(!path.with_extension("json.fleet-bak").exists());
        let hdr = path.with_file_name(HOOK_HEADERS_FILE);
        let first_hdr = std::fs::read_to_string(&hdr).unwrap();
        assert_eq!(first_hdr, "Authorization: Bearer tok\n");
        // Second run: nothing to change (settings AND headers unchanged), files untouched.
        assert_eq!(
            install_hook_at(&path, "http://127.0.0.1:4180/hook", "tok").unwrap(),
            HookInstall::Unchanged
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), first);
        assert_eq!(std::fs::read_to_string(&hdr).unwrap(), first_hdr);
        assert!(!path.with_extension("json.fleet-bak").exists());
        // A token change alone (settings.json content is token-bearing too,
        // so this also changes settings) is `Written`.
        assert_eq!(
            install_hook_at(&path, "http://127.0.0.1:4180/hook", "tok2").unwrap(),
            HookInstall::Written
        );
        assert_eq!(
            std::fs::read_to_string(&hdr).unwrap(),
            "Authorization: Bearer tok2\n"
        );
    }

    #[test]
    fn install_writes_a_private_headers_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".claude").join("settings.json");
        install_hook_at(&path, "http://127.0.0.1:4180/hook", "tok").unwrap();
        let hdr = path.with_file_name(HOOK_HEADERS_FILE);
        assert_eq!(
            std::fs::read_to_string(&hdr).unwrap(),
            "Authorization: Bearer tok\n"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&hdr).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn install_hook_at_keeps_user_hooks_and_backs_up() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let user = r#"{"permissions":{"allow":["Bash(ls)"]},"hooks":{"Stop":[{"matcher":"","hooks":[{"type":"command","command":"notify-send done"}]}]}}"#;
        std::fs::write(&path, user).unwrap();
        assert_eq!(
            install_hook_at(&path, "http://127.0.0.1:4180/hook", "tok").unwrap(),
            HookInstall::Written
        );
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(v["permissions"]["allow"][0], "Bash(ls)");
        let stop = v["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(
            stop.len(),
            2,
            "user Stop hook kept beside fleet's: {stop:?}"
        );
        assert_eq!(stop[0]["hooks"][0]["command"], "notify-send done");
        assert_eq!(
            std::fs::read_to_string(path.with_extension("json.fleet-bak")).unwrap(),
            user
        );
    }

    #[test]
    fn install_hook_at_refuses_malformed_settings_without_touching_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, "{ not json").unwrap();
        assert!(install_hook_at(&path, "http://127.0.0.1:4180/hook", "tok").is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not json");
    }

    #[test]
    fn hook_entry_is_an_http_hook_with_bearer_header_and_no_token_in_url() {
        let e = hook_entry("http://127.0.0.1:4180/hook", "tok");
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
    fn http_entries_carry_the_pane_header() {
        let e = hook_entry("http://127.0.0.1:4180/hook", "tok");
        assert_eq!(e["headers"]["X-Fleet-Pane"], "$TMUX_PANE");
        assert_eq!(e["allowedEnvVars"], serde_json::json!(["TMUX_PANE"]));
        assert!(is_fleet_hook_entry(&e));
    }

    #[test]
    fn is_fleet_hook_entry_still_matches_the_pre_pane_header_shape() {
        // The shape installed before X-Fleet-Pane / allowedEnvVars existed
        // must still be recognised, so a re-merge upgrades it in place
        // instead of leaving it beside the new entry.
        let old = serde_json::json!({
            "type": "http",
            "url": "http://127.0.0.1:4180/hook",
            "headers": { "Authorization": "Bearer tok" },
            "timeout": HOOK_TIMEOUT_SECS
        });
        assert!(is_fleet_hook_entry(&old));
        let existing = serde_json::json!({
            "hooks": { "Stop": [{ "matcher": "", "hooks": [old] }] }
        })
        .to_string();
        let out =
            merge_hook_into_settings_json(&existing, "http://127.0.0.1:4180/hook", "tok").unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        let stop = v["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(
            stop.len(),
            1,
            "old shape must be replaced, not kept beside the new one: {stop:?}"
        );
        assert_eq!(stop[0]["hooks"][0]["headers"]["X-Fleet-Pane"], "$TMUX_PANE");
    }

    #[test]
    fn session_start_is_a_tokenless_command_hook() {
        let merged =
            merge_hook_into_settings_json("", "http://127.0.0.1:4180/hook", "sekrit").unwrap();
        let v: serde_json::Value = serde_json::from_str(&merged).unwrap();
        let h = &v["hooks"]["SessionStart"][0]["hooks"][0];
        assert_eq!(h["type"], "command");
        assert_eq!(h["async"], true);
        let cmd = h["command"].as_str().unwrap();
        assert!(cmd.contains("fleet-hook.headers"));
        assert!(cmd.contains("'http://127.0.0.1:4180/hook'"));
        assert!(!cmd.contains("sekrit"));
    }

    #[test]
    fn all_events_are_installed_once_and_remerge_is_idempotent() {
        let url = "http://127.0.0.1:4180/hook";
        let once = merge_hook_into_settings_json("", url, "t").unwrap();
        let twice = merge_hook_into_settings_json(&once, url, "t").unwrap();
        assert_eq!(once, twice);
        let v: serde_json::Value = serde_json::from_str(&once).unwrap();
        for ev in [
            "Stop",
            "UserPromptSubmit",
            "PostToolUse",
            "SessionEnd",
            "StopFailure",
            "Notification",
            "SessionStart",
            "PreCompact",
            "PostCompact",
        ] {
            assert_eq!(v["hooks"][ev].as_array().unwrap().len(), 1, "{ev}");
        }
        assert_eq!(FLEET_HOOK_EVENTS.len(), 9);
        assert_eq!(
            v["hooks"]["SessionEnd"][0]["matcher"],
            "logout|prompt_input_exit|other|clear|resume"
        );
        assert_eq!(
            SESSION_END_MATCHER,
            "logout|prompt_input_exit|other|clear|resume"
        );
        assert_eq!(
            NOTIFICATION_MATCHER,
            "permission_prompt|elicitation_dialog|elicitation_url_dialog|quota_auto_resume_stale|quota_auto_resume_disabled|quota_auto_resume_fired"
        );
    }

    #[test]
    fn a_base_url_change_replaces_the_session_start_command() {
        let a = merge_hook_into_settings_json("", "http://127.0.0.1:4180/hook", "t").unwrap();
        let b = merge_hook_into_settings_json(&a, "https://fleet.example.com/hook", "t").unwrap();
        let v: serde_json::Value = serde_json::from_str(&b).unwrap();
        let arr = v["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert!(arr[0]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .contains("fleet.example.com"));
    }

    #[test]
    fn a_user_session_start_hook_survives() {
        let user = r#"{"hooks":{"SessionStart":[{"matcher":"","hooks":[{"type":"command","command":"echo hi"}]}]}}"#;
        let merged =
            merge_hook_into_settings_json(user, "http://127.0.0.1:4180/hook", "t").unwrap();
        let v: serde_json::Value = serde_json::from_str(&merged).unwrap();
        assert_eq!(v["hooks"]["SessionStart"].as_array().unwrap().len(), 2);
    }

    #[test]
    #[cfg(unix)]
    fn session_start_command_is_valid_posix_sh() {
        let cmd = session_start_command("http://127.0.0.1:4180/hook");
        // -H @file is curl's header-file syntax: '@' immediately before the
        // (possibly-quoted) path, no space.
        assert!(cmd.contains("-H @\"$HOME/.claude/fleet-hook.headers\""));
        let out = std::process::Command::new("sh")
            .args(["-n", "-c", &cmd])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "not valid POSIX sh: {cmd}\nstderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[test]
    fn hook_block_installs_all_nine_events_with_their_matchers() {
        let merged =
            merge_hook_into_settings_json("", "http://127.0.0.1:4180/hook", "tok").unwrap();
        let v: serde_json::Value = serde_json::from_str(&merged).unwrap();
        let http_expect = [
            ("Stop", ""),
            ("UserPromptSubmit", ""),
            ("PostToolUse", WORKTREE_TOOL_MATCHER),
            ("SessionEnd", SESSION_END_MATCHER),
            ("StopFailure", ""),
            ("Notification", NOTIFICATION_MATCHER),
            ("PreCompact", ""),
            ("PostCompact", ""),
        ];
        for (event, matcher) in http_expect {
            let arr = v["hooks"][event]
                .as_array()
                .unwrap_or_else(|| panic!("{event} missing"));
            assert_eq!(arr.len(), 1, "{event}: {arr:?}");
            assert_eq!(arr[0]["matcher"], matcher, "{event}");
            assert_eq!(arr[0]["hooks"][0]["type"], "http", "{event}");
            assert_eq!(arr[0]["hooks"][0]["url"], "http://127.0.0.1:4180/hook");
        }
        let start = v["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(start.len(), 1);
        assert_eq!(start[0]["hooks"][0]["type"], "command");
        // Re-running is idempotent for every event.
        let again =
            merge_hook_into_settings_json(&merged, "http://127.0.0.1:4180/hook", "tok").unwrap();
        assert_eq!(merged, again);
    }

    #[test]
    fn merge_hook_into_empty_settings() {
        let out = merge_hook_into_settings_json("", "http://127.0.0.1:4180/hook", "tok").unwrap();
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
        let pre = merge_hook_into_settings_json("", "http://127.0.0.1:4180/hook", "tok").unwrap();
        let mut v: serde_json::Value = serde_json::from_str(&pre).unwrap();
        v["hooks"]
            .as_object_mut()
            .unwrap()
            .remove("UserPromptSubmit");
        let stripped = serde_json::to_string_pretty(&v).unwrap();
        let out =
            merge_hook_into_settings_json(&stripped, "http://127.0.0.1:4180/hook", "tok").unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["hooks"]["UserPromptSubmit"].as_array().unwrap().len(), 1);
        let again =
            merge_hook_into_settings_json(&out, "http://127.0.0.1:4180/hook", "tok").unwrap();
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
                "hooks": [hook_entry("http://127.0.0.1:4180/hook", "old")]
            }] }
        })
        .to_string();
        let out =
            merge_hook_into_settings_json(&stale, "http://127.0.0.1:4180/hook", "tok").unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        let ptu = v["hooks"]["PostToolUse"].as_array().unwrap();
        assert_eq!(ptu.len(), 1, "{ptu:?}");
        assert_eq!(ptu[0]["matcher"], WORKTREE_TOOL_MATCHER);
        assert!(!out.contains("WorktreeCreate"));
        assert_eq!(
            out,
            merge_hook_into_settings_json(&out, "http://127.0.0.1:4180/hook", "tok").unwrap()
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
        let out =
            merge_hook_into_settings_json(existing, "http://127.0.0.1:4180/hook", "tok").unwrap();
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
        let out2 =
            merge_hook_into_settings_json(&out, "http://127.0.0.1:4180/hook", "tok2").unwrap();
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
        let out3 =
            merge_hook_into_settings_json(&out2, "http://127.0.0.1:4180/hook", "tok2").unwrap();
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
        let out =
            merge_hook_into_settings_json(legacy, "http://127.0.0.1:4180/hook", "new").unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        let stop = v["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 1, "legacy entry must be replaced: {stop:?}");
        assert_eq!(stop[0]["hooks"][0]["type"], "http");
        assert!(!out.contains("token=old"));
    }

    #[test]
    fn merge_hook_strips_legacy_token_command_hooks_under_any_base() {
        // A desktop-era curl hook carries the (old) master token in argv; a
        // migrated hub's master token must not survive a base change there.
        let legacy = r#"{
          "hooks": {
            "Stop": [
              {
                "matcher": "",
                "hooks": [{"type":"command","command":"curl -s -X POST http://127.0.0.1:4180/hook?token=abc -d @-"}]
              },
              {
                "matcher": "",
                "hooks": [{"type":"command","command":"notify-send 'turn over'"}]
              }
            ]
          }
        }"#;
        let out =
            merge_hook_into_settings_json(legacy, "https://fleet.example.com/hook", "new").unwrap();
        assert!(!out.contains("token=abc"), "legacy token hook kept:\n{out}");
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        let stop = v["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 2, "user hook + fleet's: {stop:?}");
        assert_eq!(stop[0]["hooks"][0]["command"], "notify-send 'turn over'");
        assert_eq!(stop[1]["hooks"][0]["url"], "https://fleet.example.com/hook");
    }

    #[test]
    fn merge_hook_refuses_malformed_settings_json() {
        // A corrupted settings.json must never be replaced with `{}` — that
        // would wipe the user's permissions / env / own hooks. The merge
        // errors before any write and names the file.
        let err =
            merge_hook_into_settings_json("not json at all", "http://127.0.0.1:4180/hook", "tok")
                .unwrap_err();
        assert_eq!(err.code, "E_PROVISION");
        assert!(err.message.contains("settings.json"), "{}", err.message);
        // A truncated file is malformed too.
        let err = merge_hook_into_settings_json(
            r#"{"hooks": {"Stop": ["#,
            "http://127.0.0.1:4180/hook",
            "tok",
        )
        .unwrap_err();
        assert_eq!(err.code, "E_PROVISION");
        // A non-object root is refused as well (existing E_PARSE path).
        assert!(
            merge_hook_into_settings_json("[1,2]", "http://127.0.0.1:4180/hook", "tok").is_err()
        );
    }

    #[test]
    fn merge_hook_with_a_public_base_writes_that_url() {
        let out =
            merge_hook_into_settings_json("", "https://fleet.example.com/hook", "tok").unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            v["hooks"]["Stop"][0]["hooks"][0]["url"],
            "https://fleet.example.com/hook"
        );
        // Re-merging under a different base replaces the fleet entries (no duplicates).
        let out2 =
            merge_hook_into_settings_json(&out, "http://127.0.0.1:4180/hook", "tok").unwrap();
        let v2: serde_json::Value = serde_json::from_str(&out2).unwrap();
        assert_eq!(v2["hooks"]["Stop"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn merge_hook_keeps_a_users_bearer_http_hook_at_a_deeper_path() {
        let user_block = serde_json::json!({
            "matcher": "",
            "hooks": [
                {
                    "type": "http",
                    "url": "https://ci.example.com/api/v2/hook",
                    "headers": { "Authorization": "Bearer x" },
                    "timeout": 5
                },
                { "type": "command", "command": "notify-send done" }
            ]
        });
        let existing = serde_json::json!({ "hooks": { "Stop": [user_block.clone()] } }).to_string();
        for hook_url in [
            "http://127.0.0.1:4180/hook",
            "https://fleet.example.com/hook",
        ] {
            let out = merge_hook_into_settings_json(&existing, hook_url, "tok").unwrap();
            let v: serde_json::Value = serde_json::from_str(&out).unwrap();
            let stop = v["hooks"]["Stop"].as_array().unwrap();
            assert_eq!(
                stop.len(),
                2,
                "user block kept beside fleet's under {hook_url}: {stop:?}"
            );
            assert_eq!(
                stop[0], user_block,
                "both user hooks intact under {hook_url}"
            );
            assert_eq!(stop[1]["hooks"][0]["url"], hook_url);
        }
    }
}
