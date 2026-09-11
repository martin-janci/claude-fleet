//! Service functions for Claude CLI background-session operations.

use crate::claude_cli;
use crate::ipc_error::IpcError;
use crate::ssh::SshClient;
use crate::store::Store;
use crate::validate;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

// ─── args / result types ─────────────────────────────────────────────────────
//
// Every arg struct validates the host alias (it becomes an `ssh` operand) and
// every value that becomes a `claude` positional / option value (a leading
// `-` would be read as a flag). `claude_cli` re-checks the same rules when it
// builds the script, so DevTools / MCP callers cannot bypass them.

#[derive(Debug, Deserialize)]
pub struct NewBgSessionArgs {
    pub host_alias: String,
    pub name: String,
    pub prompt: String,
}

impl NewBgSessionArgs {
    pub fn validate(&self) -> Result<(), IpcError> {
        validate::host_alias(&self.host_alias)?;
        validate::not_option_like("session name", &self.name)?;
        if self.name.chars().any(|c| c.is_control()) {
            return Err(IpcError::new(
                "E_INVALID",
                "session name must not contain control characters",
            ));
        }
        // The prompt lands after `--` (see `claude_cli::bg_script`), so a
        // leading `-` is fine — only blank prompts are rejected.
        validate::not_blank("prompt", &self.prompt)?;
        Ok(())
    }
}

#[derive(Debug, Serialize)]
pub struct NewBgSessionResult {
    pub claude_session_id: Option<String>,
    /// Populated when `claude --bg` ran but no session id could be parsed from
    /// its output — the session may still be live, but the fleet can't track
    /// it by id (and thus can't `peek` it). Surfaced so the caller can warn.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
    /// The fleet row (MCP-7): `new_bg_session_tracked` runs a single-host
    /// reconcile right after launch so the `bg:<id>` sentinel exists before
    /// the caller's next tool call. The key is ABSENT from the JSON (not
    /// `null`) when the agent could not be matched yet (it appears on the
    /// next tick) or when the untracked path was used.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<crate::store::SessionRow>,
}

#[derive(Debug, Deserialize)]
pub struct PeekSessionArgs {
    pub host_alias: String,
    pub claude_session_id: String,
}

impl PeekSessionArgs {
    pub fn validate(&self) -> Result<(), IpcError> {
        validate::host_alias(&self.host_alias)?;
        validate::claude_session_id(&self.claude_session_id)?;
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
pub struct PurgeProjectArgs {
    pub host_alias: String,
    pub project_path: String,
    pub project_id: i64,
}

impl PurgeProjectArgs {
    pub fn validate(&self) -> Result<(), IpcError> {
        validate::host_alias(&self.host_alias)?;
        validate::not_option_like("project_path", &self.project_path)?;
        if self.project_path.chars().any(|c| c.is_control()) {
            return Err(IpcError::new(
                "E_INVALID",
                "project_path must not contain control characters",
            ));
        }
        Ok(())
    }
}

// ─── service functions ───────────────────────────────────────────────────────

pub async fn new_bg_session(
    args: NewBgSessionArgs,
    ssh: &Arc<SshClient>,
) -> Result<NewBgSessionResult, IpcError> {
    args.validate()?;
    let claude_session_id =
        claude_cli::claude_bg(ssh, &args.host_alias, &args.name, &args.prompt).await?;
    Ok(bg_session_result(claude_session_id))
}

/// Warning surfaced when `claude --bg` succeeded but its output didn't yield a
/// parseable session id.
pub const BG_NO_ID_WARNING: &str = "could not parse session id from claude --bg output";

/// Build the `new_bg_session` result, attaching a warning when no session id
/// was parsed. Pure so the warn-on-null decision is unit-testable without the
/// (SSH/local) `claude --bg` exec.
fn bg_session_result(claude_session_id: Option<String>) -> NewBgSessionResult {
    let warning = if claude_session_id.is_none() {
        Some(BG_NO_ID_WARNING.to_string())
    } else {
        None
    };
    NewBgSessionResult {
        claude_session_id,
        warning,
        session: None,
    }
}

/// `new_bg_session` + immediate registration (MCP-7): after `claude --bg`
/// returns its id, reconcile the host once (as `spawn_review` does) so the
/// synthetic `bg:<id>` row exists, then stamp the row with the launch prompt
/// (`last_prompt`, `started_at`, and a prompt-derived friendly name — PROD-4).
/// Every post-launch step is best-effort: the agent is already running, so a
/// reconcile hiccup degrades to `session: None` rather than an error.
pub async fn new_bg_session_tracked(
    args: NewBgSessionArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<NewBgSessionResult, IpcError> {
    let host_alias = args.host_alias.clone();
    let prompt = args.prompt.clone();
    let mut res = new_bg_session(args, ssh).await?;
    let Some(ref claude_id) = res.claude_session_id else {
        return Ok(res);
    };
    if let Err(e) = crate::service::sessions::reconcile_one_host(store, ssh, &host_alias).await {
        eprintln!("[bg] post-launch reconcile of {host_alias} failed: {e}");
        return Ok(res);
    }
    res.session = stamp_bg_row(store, claude_id, &prompt);
    Ok(res)
}

/// Find the bg row for `claude_id` and record the launch prompt on it.
/// Returns the refreshed row, or `None` when reconcile has not surfaced the
/// agent yet.
fn stamp_bg_row(
    store: &Mutex<Store>,
    claude_id: &str,
    prompt: &str,
) -> Option<crate::store::SessionRow> {
    let s = store.lock().ok()?;
    let row = s.get_session_by_claude_id(claude_id).ok().flatten()?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let _ = s.set_started_at(row.id, now);
    let _ = s.set_last_prompt(row.id, prompt);
    if row.friendly_name.is_none() {
        if let Some(name) = crate::service::sessions::friendly_name_from_prompt(prompt) {
            let _ = s.set_friendly_name(&row.host_alias, &row.tmux_name, Some(&name));
        }
    }
    let _ = s.insert_session_event(
        row.id,
        "prompt_sent",
        Some(&prompt.chars().take(120).collect::<String>()),
    );
    s.get_session_by_id(row.id).ok().flatten()
}

/// Resolve a `peek_session` target (MCP-7) from any of: a fleet `session_id`,
/// or a `claude_session_id` plus its `host_alias` (the id a `new_bg_session`
/// caller already holds, before reconcile has surfaced the row). Returns the
/// `(host_alias, claude_session_id)` pair to peek.
///
/// A fleet row without a Claude id yields `E_INVALID_STATE` so the caller can
/// tell "not tracked yet" apart from "no such session" (`E_NOTFOUND`).
pub fn resolve_peek_target(
    s: &Store,
    session_id: Option<i64>,
    host_alias: Option<&str>,
    claude_session_id: Option<&str>,
) -> Result<(String, String), IpcError> {
    if let Some(id) = session_id {
        let row = s
            .get_session_by_id(id)?
            .ok_or_else(|| IpcError::new("E_NOTFOUND", format!("session {id} not found")))?;
        return match row.claude_session_id {
            Some(cid) => Ok((row.host_alias, cid)),
            None => Err(IpcError::new(
                "E_INVALID_STATE",
                "this session has no Claude session id yet — nothing to peek",
            )),
        };
    }
    match (host_alias, claude_session_id) {
        (Some(host), Some(cid)) => {
            validate::host_alias(host)?;
            validate::claude_session_id(cid)?;
            Ok((host.to_string(), cid.to_string()))
        }
        // A bare Claude id: use the fleet row's host when the agent is
        // already tracked; otherwise the host is genuinely unknown.
        (None, Some(cid)) => {
            validate::claude_session_id(cid)?;
            match s.get_session_by_claude_id(cid)? {
                Some(row) => Ok((row.host_alias, cid.to_string())),
                None => Err(IpcError::new(
                    "E_INVALID",
                    "pass host_alias with claude_session_id (the agent is not tracked yet)",
                )),
            }
        }
        _ => Err(IpcError::new(
            "E_INVALID",
            "pass session_id, or claude_session_id (+ host_alias)",
        )),
    }
}

pub async fn peek_session(args: PeekSessionArgs, ssh: &Arc<SshClient>) -> Result<String, IpcError> {
    args.validate()?;
    claude_cli::claude_logs(ssh, &args.host_alias, &args.claude_session_id).await
}

pub async fn purge_project(
    args: PurgeProjectArgs,
    store: &Arc<Mutex<Store>>,
    ssh: &Arc<SshClient>,
) -> Result<(), IpcError> {
    args.validate()?;
    claude_cli::claude_purge_project(ssh, &args.host_alias, &args.project_path).await?;
    let s = store.lock().map_err(|_| IpcError::lock())?;
    s.delete_project(args.project_id)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use std::sync::Arc;

    fn make_store() -> Arc<Mutex<Store>> {
        Arc::new(Mutex::new(Store::open_in_memory().unwrap()))
    }

    #[test]
    fn new_bg_session_args_validates_empty_prompt() {
        let _ = make_store(); // ensure it compiles; tests only need validate()
        let args = NewBgSessionArgs {
            host_alias: "local".into(),
            name: "test-session".into(),
            prompt: "".into(),
        };
        assert!(args.validate().is_err());
    }

    #[test]
    fn new_bg_session_args_validates_empty_name() {
        let args = NewBgSessionArgs {
            host_alias: "local".into(),
            name: "".into(),
            prompt: "Do the thing".into(),
        };
        assert!(args.validate().is_err());
    }

    #[test]
    fn stamp_bg_row_names_and_stamps_the_reconciled_row() {
        let store = make_store();
        {
            let s = store.lock().unwrap();
            s.upsert_host("local").unwrap();
            s.upsert_bg_session("local", "bg:u1", None, "u1", Some("working"), 5)
                .unwrap();
        }
        let row = stamp_bg_row(&store, "u1", "Review the auth PR, carefully!").expect("row");
        assert_eq!(
            row.friendly_name.as_deref(),
            Some("review the auth pr carefully")
        );
        assert_eq!(
            row.last_prompt.as_deref(),
            Some("Review the auth PR, carefully!")
        );
        assert!(row.started_at.is_some());
        // Unknown id ⇒ None, no panic.
        assert!(stamp_bg_row(&store, "nope", "x").is_none());
    }

    #[test]
    fn resolve_peek_target_accepts_fleet_id_or_claude_id() {
        let store = make_store();
        let (tracked_id, untracked_id) = {
            let s = store.lock().unwrap();
            s.upsert_host("local").unwrap();
            let bg = s
                .upsert_bg_session(
                    "local",
                    &format!("bg:{UUID}"),
                    None,
                    UUID,
                    Some("working"),
                    5,
                )
                .unwrap();
            let plain = s
                .upsert_session("dev-plain", "local", None, None, 1, 1, "running", None)
                .unwrap();
            (bg, plain)
        };
        let s = store.lock().unwrap();
        assert_eq!(
            resolve_peek_target(&s, Some(tracked_id), None, None).unwrap(),
            ("local".to_string(), UUID.to_string())
        );
        assert_eq!(
            resolve_peek_target(&s, Some(untracked_id), None, None)
                .unwrap_err()
                .code,
            "E_INVALID_STATE"
        );
        assert_eq!(
            resolve_peek_target(&s, Some(999), None, None)
                .unwrap_err()
                .code,
            "E_NOTFOUND"
        );
        // Claude id alone resolves through the tracked row …
        assert_eq!(
            resolve_peek_target(&s, None, None, Some(UUID)).unwrap().0,
            "local"
        );
        // … an untracked id needs its host …
        let other = "6ba7b810-9dad-11d1-80b4-00c04fd430c8";
        assert_eq!(
            resolve_peek_target(&s, None, None, Some(other))
                .unwrap_err()
                .code,
            "E_INVALID"
        );
        assert_eq!(
            resolve_peek_target(&s, None, Some("mefistos"), Some(other)).unwrap(),
            ("mefistos".to_string(), other.to_string())
        );
        // … and garbage is rejected before any lookup.
        assert_eq!(
            resolve_peek_target(&s, None, Some("local"), Some("--foo"))
                .unwrap_err()
                .code,
            "E_INVALID"
        );
        assert_eq!(
            resolve_peek_target(&s, None, None, None).unwrap_err().code,
            "E_INVALID"
        );
    }

    #[test]
    fn bg_session_result_warns_when_id_missing() {
        let res = bg_session_result(None);
        assert!(res.claude_session_id.is_none());
        assert_eq!(res.warning.as_deref(), Some(BG_NO_ID_WARNING));
    }

    #[test]
    fn bg_session_result_no_warning_when_id_present() {
        let res = bg_session_result(Some("abc-123".into()));
        assert_eq!(res.claude_session_id.as_deref(), Some("abc-123"));
        assert!(res.warning.is_none());
    }

    #[test]
    fn peek_session_args_validates_missing_session_id() {
        let args = PeekSessionArgs {
            host_alias: "local".into(),
            claude_session_id: "".into(),
        };
        assert!(args.validate().is_err());
    }

    const UUID: &str = "550e8400-e29b-41d4-a716-446655440000";

    #[test]
    fn new_bg_session_args_rejects_option_like_values_and_bad_host() {
        let ok = NewBgSessionArgs {
            host_alias: "mefistos".into(),
            name: "review-1".into(),
            prompt: "Summarise the diff".into(),
        };
        assert!(ok.validate().is_ok());

        // A prompt may start with `-` (markdown list): bg_script emits `--`
        // before it, so it can never be parsed as a flag.
        let dash_prompt = NewBgSessionArgs {
            prompt: "- fix login\n- add test".into(),
            ..ok
        };
        assert!(dash_prompt.validate().is_ok());

        let blank_prompt = NewBgSessionArgs {
            prompt: "   ".into(),
            ..dash_prompt
        };
        assert_eq!(blank_prompt.validate().unwrap_err().code, "E_INVALID");

        let bad_name = NewBgSessionArgs {
            name: "-n".into(),
            prompt: "fine".into(),
            ..blank_prompt
        };
        assert_eq!(bad_name.validate().unwrap_err().code, "E_INVALID");

        let ctrl_name = NewBgSessionArgs {
            name: "a\nb".into(),
            ..bad_name
        };
        assert_eq!(ctrl_name.validate().unwrap_err().code, "E_INVALID");

        let bad_host = NewBgSessionArgs {
            host_alias: "-oProxyCommand=id".into(),
            name: "ok".into(),
            ..ctrl_name
        };
        assert_eq!(bad_host.validate().unwrap_err().code, "E_INVALID");
    }

    #[test]
    fn peek_session_args_requires_uuid_id_and_valid_host() {
        let ok = PeekSessionArgs {
            host_alias: "local".into(),
            claude_session_id: UUID.into(),
        };
        assert!(ok.validate().is_ok());
        for bad in ["--foo", "-h", "abc-123", "'; rm -rf / #"] {
            let args = PeekSessionArgs {
                host_alias: "local".into(),
                claude_session_id: bad.into(),
            };
            assert_eq!(args.validate().unwrap_err().code, "E_INVALID", "{bad:?}");
        }
        let bad_host = PeekSessionArgs {
            host_alias: "-tt".into(),
            claude_session_id: UUID.into(),
        };
        assert_eq!(bad_host.validate().unwrap_err().code, "E_INVALID");
    }

    #[test]
    fn purge_project_args_rejects_option_like_path_and_bad_host() {
        let ok = PurgeProjectArgs {
            host_alias: "local".into(),
            project_path: "/home/me/projects/x".into(),
            project_id: 1,
        };
        assert!(ok.validate().is_ok());
        for bad in ["", "  ", "--all", "-rf", "/a\nb"] {
            let args = PurgeProjectArgs {
                host_alias: "local".into(),
                project_path: bad.into(),
                project_id: 1,
            };
            assert_eq!(args.validate().unwrap_err().code, "E_INVALID", "{bad:?}");
        }
        let bad_host = PurgeProjectArgs {
            host_alias: "has space".into(),
            project_path: "/x".into(),
            project_id: 1,
        };
        assert_eq!(bad_host.validate().unwrap_err().code, "E_INVALID");
    }
}
