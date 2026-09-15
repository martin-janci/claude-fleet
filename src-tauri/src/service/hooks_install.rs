//! Installing fleet's Claude Code hooks into a `~/.claude/settings.json`.
//!
//! The hook block (`Stop`, `UserPromptSubmit`, `PostToolUse(EnterWorktree|
//! ExitWorktree)`, `SessionEnd`, `StopFailure`, `Notification`) is a set of
//! `type: "http"` hooks pointing at fleet's `/hook` endpoint with the host's
//! bearer token in an `Authorization` header. [`merge_hook_into_settings_json`]
//! is the pure, idempotent merge shared by the local install
//! (`commands::mcp::install_fleet_hook`, the enable-time auto-install) and
//! the remote one (`provision::provision_hook`).

use crate::ipc_error::{codes, lock, IpcError};
use crate::store::Store;
use std::sync::Mutex;

/// Seconds Claude Code waits for the hook endpoint before giving up. The
/// server answers in milliseconds; a down server must not stall a turn.
const HOOK_TIMEOUT_SECS: u32 = 5;

/// The fleet `/hook` URL for a given port. Identical on every host: on a
/// remote host `127.0.0.1:<port>` is the reverse tunnel's loopback end.
pub fn hook_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/hook")
}

/// One Claude Code `type: "http"` hook entry. The bearer token rides in an
/// `Authorization` header — never in a process argv (SEC-3) — and the
/// settings file that carries it is written 0600.
pub fn hook_entry(port: u16, token: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "http",
        "url": hook_url(port),
        "headers": { "Authorization": format!("Bearer {token}") },
        "timeout": HOOK_TIMEOUT_SECS
    })
}

/// Matcher of fleet's PostToolUse hook (Claude Code matchers are regexes):
/// `EnterWorktree` registers a worktree row for the calling host,
/// `ExitWorktree` with `action: "remove"` drops it. The `WorktreeCreate` /
/// `WorktreeRemove` hook EVENTS are deliberately never installed: they
/// replace git's own worktree creation / removal, so an http hook there would
/// break it on every host.
pub const WORKTREE_TOOL_MATCHER: &str = "EnterWorktree|ExitWorktree";

/// `SessionEnd` reasons that mean the Claude process is gone. `clear` and
/// `resume` are deliberately absent: the process lives on under a new
/// session id, and marking the row `stopped` would be wrong.
pub const SESSION_END_MATCHER: &str = "logout|prompt_input_exit|other";

/// `Notification` types fleet turns into state (see
/// `service::hooks::notification_effect`). `idle_prompt`, `auth_success`,
/// the `elicitation_complete/response` pair and the agent-view-only
/// `agent_*` types carry nothing fleet needs.
pub const NOTIFICATION_MATCHER: &str = "permission_prompt|elicitation_dialog|elicitation_url_dialog|quota_auto_resume_stale|quota_auto_resume_disabled|quota_auto_resume_fired";

/// The hook events fleet installs, with their matcher. `Stop` is the
/// completion signal (turn over → idle, `turn_seq` bump), `UserPromptSubmit`
/// the busy signal (turn starting → working), `PostToolUse(EnterWorktree|
/// ExitWorktree)` the worktree registration and removal, `SessionEnd` the
/// exit signal (→ stopped), `StopFailure` the API-error completion (→ idle,
/// `turn_seq` bump, `stop_failure` timeline event) and `Notification` the
/// waiting-on-a-human signal (→ blocked). `SessionStart` cannot be an http
/// hook (Claude Code allows only command / mcp_tool there) and is not used.
pub const FLEET_HOOK_EVENTS: &[(&str, &str)] = &[
    ("Stop", ""),
    ("UserPromptSubmit", ""),
    ("PostToolUse", WORKTREE_TOOL_MATCHER),
    ("SessionEnd", SESSION_END_MATCHER),
    ("StopFailure", ""),
    ("Notification", NOTIFICATION_MATCHER),
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
/// Shared by the local `install_fleet_hook` command (via [`install_hook_at`])
/// and `provision::provision_hook` for remote hosts.
pub fn merge_hook_into_settings_json(
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
        .ok_or_else(|| IpcError::new(codes::E_PARSE, "hooks is not an object"))?;

    for (event, matcher) in FLEET_HOOK_EVENTS {
        let mut arr = strip_fleet(hooks.get(*event).unwrap_or(&serde_json::json!([])));
        arr.as_array_mut().unwrap().push(serde_json::json!({
            "matcher": matcher,
            "hooks": [hook_entry(port, token)]
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

/// What `install_hook_at` did to the settings file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookInstall {
    /// The file was created or its fleet hook entries were (re)written.
    Written,
    /// The file already carried exactly this hook config; nothing touched.
    Unchanged,
}

/// Merge fleet's hooks into the settings file at `settings_path` (shared by
/// the Settings button and the enable-time auto-install). Idempotent: when
/// the merge changes nothing the file (and its backup) are left alone. The
/// user's own hooks, permissions and env survive via
/// `merge_hook_into_settings_json`; an unparseable file is refused, never
/// replaced.
pub fn install_hook_at(
    settings_path: &std::path::Path,
    port: u16,
    token: &str,
) -> Result<HookInstall, IpcError> {
    let existing = if settings_path.exists() {
        std::fs::read_to_string(settings_path)
            .map_err(|e| IpcError::new(codes::E_IO, format!("read settings.json: {e}")))?
    } else {
        String::new()
    };

    let merged = merge_hook_into_settings_json(&existing, port, token)?;
    if merged == existing {
        return Ok(HookInstall::Unchanged);
    }

    if let Some(parent) = settings_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| IpcError::new(codes::E_IO, format!("create .claude dir: {e}")))?;
    }
    // Back up the previous file before touching it (it may carry the user's
    // permissions/env), then write the merged file 0600 from creation.
    if !existing.trim().is_empty() {
        let bak = settings_path.with_extension("json.fleet-bak");
        super::provision::write_private_file(&bak, &existing).map_err(|e| {
            IpcError::new(codes::E_IO, format!("write settings.json.fleet-bak: {e}"))
        })?;
    }
    super::provision::write_private_file(settings_path, &merged)
        .map_err(|e| IpcError::new(codes::E_IO, format!("write settings.json: {e}")))?;
    Ok(HookInstall::Written)
}

/// Enable-time auto-install of the local hook (Q8). Best-effort: a failure
/// (malformed settings.json, unwritable home) is logged and never fails the
/// enable itself; the Settings button still reports the error verbatim.
/// Skipped when `~/.claude` does not exist, i.e. Claude Code was never run
/// on this machine, so fleet does not create a config dir nobody uses.
pub fn auto_install_local_hook(store: &Mutex<Store>, port: u16) {
    let result = (|| -> Result<Option<HookInstall>, IpcError> {
        let path = local_settings_path()?;
        if !path.parent().is_some_and(std::path::Path::is_dir) {
            return Ok(None);
        }
        let token = local_hook_token(store)?;
        install_hook_at(&path, port, &token).map(Some)
    })();
    match result {
        Ok(Some(HookInstall::Written)) => {
            tracing::info!(
                port,
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
            install_hook_at(&path, 4180, "tok").unwrap(),
            HookInstall::Written
        );
        let first = std::fs::read_to_string(&path).unwrap();
        assert!(first.contains("http://127.0.0.1:4180/hook"));
        // No backup for a file that did not exist.
        assert!(!path.with_extension("json.fleet-bak").exists());
        // Second run: nothing to change, file untouched.
        assert_eq!(
            install_hook_at(&path, 4180, "tok").unwrap(),
            HookInstall::Unchanged
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), first);
        assert!(!path.with_extension("json.fleet-bak").exists());
    }

    #[test]
    fn install_hook_at_keeps_user_hooks_and_backs_up() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let user = r#"{"permissions":{"allow":["Bash(ls)"]},"hooks":{"Stop":[{"matcher":"","hooks":[{"type":"command","command":"notify-send done"}]}]}}"#;
        std::fs::write(&path, user).unwrap();
        assert_eq!(
            install_hook_at(&path, 4180, "tok").unwrap(),
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
        assert!(install_hook_at(&path, 4180, "tok").is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not json");
    }

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
    fn hook_block_installs_all_six_events_with_their_matchers() {
        let merged = merge_hook_into_settings_json("", 4180, "tok").unwrap();
        let v: serde_json::Value = serde_json::from_str(&merged).unwrap();
        let expect = [
            ("Stop", ""),
            ("UserPromptSubmit", ""),
            ("PostToolUse", WORKTREE_TOOL_MATCHER),
            ("SessionEnd", SESSION_END_MATCHER),
            ("StopFailure", ""),
            ("Notification", NOTIFICATION_MATCHER),
        ];
        assert_eq!(FLEET_HOOK_EVENTS.len(), expect.len());
        for (event, matcher) in expect {
            let arr = v["hooks"][event]
                .as_array()
                .unwrap_or_else(|| panic!("{event} missing"));
            assert_eq!(arr.len(), 1, "{event}: {arr:?}");
            assert_eq!(arr[0]["matcher"], matcher, "{event}");
            assert_eq!(arr[0]["hooks"][0]["type"], "http", "{event}");
            assert_eq!(arr[0]["hooks"][0]["url"], "http://127.0.0.1:4180/hook");
        }
        assert_eq!(SESSION_END_MATCHER, "logout|prompt_input_exit|other");
        assert_eq!(
            NOTIFICATION_MATCHER,
            "permission_prompt|elicitation_dialog|elicitation_url_dialog|quota_auto_resume_stale|quota_auto_resume_disabled|quota_auto_resume_fired"
        );
        // Re-running is idempotent for the new events too.
        let again = merge_hook_into_settings_json(&merged, 4180, "tok").unwrap();
        let v2: serde_json::Value = serde_json::from_str(&again).unwrap();
        for (event, _) in expect {
            assert_eq!(v2["hooks"][event].as_array().unwrap().len(), 1, "{event}");
        }
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
}
