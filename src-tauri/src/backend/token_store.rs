//! Where the hub client token lives.
//!
//! The desktop pairs with a `fleet-hub` as an ordinary client and keeps the
//! token it was handed. That token is a bearer credential for someone else's
//! fleet, so it deliberately does **not** go into `state.db` (which is
//! plaintext, backed up and copied around) and never reaches a log line — the
//! only thing this module ever hands out is the secret itself, to the caller
//! that asked for it.

/// A place to keep the hub client token: the OS keychain in the app, an
/// in-memory double in tests.
///
/// Errors are `String` rather than `IpcError` because a failure here is
/// diagnostic text for a log line or a Settings message, never a code the
/// frontend branches on. An implementation must never put the token into the
/// error text.
pub trait TokenStore: Send + Sync {
    /// The stored token, or `None` when this app has not been paired.
    fn get(&self) -> Result<Option<String>, String>;
    /// Store (or replace) the token. Used by pairing.
    fn set(&self, token: &str) -> Result<(), String>;
    /// Forget the token. Used by Disconnect; does not revoke anything on the
    /// hub — only the operator can do that.
    fn clear(&self) -> Result<(), String>;
}

/// The keychain entry's service/account pair on macOS, and the file name of
/// the fallback elsewhere.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const SERVICE: &str = "claude-fleet";
const ACCOUNT: &str = "hub-client-token";

/// The real store: the macOS keychain via `/usr/bin/security`, and an
/// owner-only file in the app data dir on the platforms that have no system
/// keychain we can reach without a new dependency.
pub struct OsTokenStore {
    /// Only the non-macOS fallback reads this, but it is held unconditionally
    /// so the struct has one shape — and one constructor — on every platform.
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    data_dir: std::path::PathBuf,
}

impl OsTokenStore {
    pub fn new(data_dir: std::path::PathBuf) -> Self {
        Self { data_dir }
    }

    /// The fallback file. Sibling of `state.db`, and like it owner-only.
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    fn fallback_path(&self) -> std::path::PathBuf {
        self.data_dir.join(ACCOUNT)
    }
}

#[cfg(target_os = "macos")]
impl TokenStore for OsTokenStore {
    fn get(&self) -> Result<Option<String>, String> {
        let out = std::process::Command::new("/usr/bin/security")
            .args(["find-generic-password", "-s", SERVICE, "-a", ACCOUNT, "-w"])
            .output()
            .map_err(|e| format!("cannot run /usr/bin/security: {e}"))?;
        if out.status.success() {
            let token = String::from_utf8_lossy(&out.stdout).trim().to_string();
            return Ok(if token.is_empty() { None } else { Some(token) });
        }
        // 44 is `security`'s SecItemNotFound: simply not paired.
        if out.status.code() == Some(44) {
            return Ok(None);
        }
        Err(format!(
            "keychain lookup failed (security exited {:?})",
            out.status.code()
        ))
    }

    fn set(&self, token: &str) -> Result<(), String> {
        // `-w <token>` puts the secret on the argv, where `ps` can see it for
        // the lifetime of this very short-lived child. `security` offers no
        // stdin path for a non-interactive write, and the alternative — a
        // temp file — trades one exposure for a worse one.
        let out = std::process::Command::new("/usr/bin/security")
            .args([
                "add-generic-password",
                "-U",
                "-s",
                SERVICE,
                "-a",
                ACCOUNT,
                "-w",
                token,
            ])
            .output()
            .map_err(|e| format!("cannot run /usr/bin/security: {e}"))?;
        if out.status.success() {
            Ok(())
        } else {
            Err(format!(
                "keychain write failed (security exited {:?})",
                out.status.code()
            ))
        }
    }

    fn clear(&self) -> Result<(), String> {
        let out = std::process::Command::new("/usr/bin/security")
            .args(["delete-generic-password", "-s", SERVICE, "-a", ACCOUNT])
            .output()
            .map_err(|e| format!("cannot run /usr/bin/security: {e}"))?;
        // Already gone is success.
        if out.status.success() || out.status.code() == Some(44) {
            Ok(())
        } else {
            Err(format!(
                "keychain delete failed (security exited {:?})",
                out.status.code()
            ))
        }
    }
}

#[cfg(not(target_os = "macos"))]
impl TokenStore for OsTokenStore {
    fn get(&self) -> Result<Option<String>, String> {
        match std::fs::read_to_string(self.fallback_path()) {
            Ok(s) => {
                let token = s.trim().to_string();
                Ok(if token.is_empty() { None } else { Some(token) })
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("cannot read the hub token file: {e}")),
        }
    }

    fn set(&self, token: &str) -> Result<(), String> {
        let path = self.fallback_path();
        std::fs::write(&path, token)
            .map_err(|e| format!("cannot write the hub token file: {e}"))?;
        // Same treatment `state.db` gets (SEC-11): owner-only.
        fleet_core::service::provision::set_private_mode(&path);
        Ok(())
    }

    fn clear(&self) -> Result<(), String> {
        match std::fs::remove_file(self.fallback_path()) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("cannot remove the hub token file: {e}")),
        }
    }
}

/// Test double: a token held in memory, or a store that always fails.
#[cfg(test)]
pub struct InMemoryTokenStore {
    token: std::sync::Mutex<Option<String>>,
    fails_with: Option<String>,
}

#[cfg(test)]
impl InMemoryTokenStore {
    pub fn with_token(token: &str) -> Self {
        Self {
            token: std::sync::Mutex::new(Some(token.to_string())),
            fails_with: None,
        }
    }

    pub fn empty() -> Self {
        Self {
            token: std::sync::Mutex::new(None),
            fails_with: None,
        }
    }

    pub fn failing(message: &str) -> Self {
        Self {
            token: std::sync::Mutex::new(None),
            fails_with: Some(message.to_string()),
        }
    }
}

#[cfg(test)]
impl TokenStore for InMemoryTokenStore {
    fn get(&self) -> Result<Option<String>, String> {
        match &self.fails_with {
            Some(e) => Err(e.clone()),
            None => Ok(self.token.lock().unwrap().clone()),
        }
    }

    fn set(&self, token: &str) -> Result<(), String> {
        match &self.fails_with {
            Some(e) => Err(e.clone()),
            None => {
                *self.token.lock().unwrap() = Some(token.to_string());
                Ok(())
            }
        }
    }

    fn clear(&self) -> Result<(), String> {
        match &self.fails_with {
            Some(e) => Err(e.clone()),
            None => {
                *self.token.lock().unwrap() = None;
                Ok(())
            }
        }
    }
}
