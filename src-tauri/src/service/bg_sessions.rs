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
        validate::not_option_like("prompt", &self.prompt)?;
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
        validate::host_alias(&self.host_alias)?;
        if self.claude_session_id.trim().is_empty() {
            return Err(IpcError::new(
                "E_INVALID",
                "claude_session_id must not be empty",
            ));
        }
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

        let bad_prompt = NewBgSessionArgs {
            prompt: "--dangerously-skip-permissions".into(),
            ..ok
        };
        assert_eq!(bad_prompt.validate().unwrap_err().code, "E_INVALID");

        let bad_name = NewBgSessionArgs {
            name: "-n".into(),
            prompt: "fine".into(),
            ..bad_prompt
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
