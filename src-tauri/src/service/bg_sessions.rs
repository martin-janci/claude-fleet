//! Service functions for Claude CLI background-session operations.

use crate::claude_cli;
use crate::ipc_error::IpcError;
use crate::ssh::SshClient;
use crate::store::Store;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

// ─── args / result types ─────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct NewBgSessionArgs {
    pub host_alias: String,
    pub name: String,
    pub prompt: String,
}

impl NewBgSessionArgs {
    pub fn validate(&self) -> Result<(), IpcError> {
        if self.name.trim().is_empty() {
            return Err(IpcError::new("E_INVALID", "session name must not be empty"));
        }
        if self.prompt.trim().is_empty() {
            return Err(IpcError::new("E_INVALID", "prompt must not be empty"));
        }
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
}

#[derive(Debug, Deserialize)]
pub struct PeekSessionArgs {
    pub host_alias: String,
    pub claude_session_id: String,
}

impl PeekSessionArgs {
    pub fn validate(&self) -> Result<(), IpcError> {
        if self.claude_session_id.trim().is_empty() {
            return Err(IpcError::new(
                "E_INVALID",
                "claude_session_id must not be empty",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
pub struct PurgeProjectArgs {
    pub host_alias: String,
    pub project_path: String,
    pub project_id: i64,
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
    if args.project_path.trim().is_empty() {
        return Err(IpcError::new("E_INVALID", "project_path must not be empty"));
    }
    claude_cli::claude_purge_project(ssh, &args.host_alias, &args.project_path).await?;
    let s = store
        .lock()
        .map_err(|_| IpcError::new("E_LOCK", "store mutex poisoned"))?;
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
}
