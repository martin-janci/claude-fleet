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
    Ok(NewBgSessionResult { claude_session_id })
}

pub async fn peek_session(
    args: PeekSessionArgs,
    ssh: &Arc<SshClient>,
) -> Result<String, IpcError> {
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
    fn peek_session_args_validates_missing_session_id() {
        let args = PeekSessionArgs {
            host_alias: "local".into(),
            claude_session_id: "".into(),
        };
        assert!(args.validate().is_err());
    }
}
