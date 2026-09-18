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

/// `errSecItemNotFound` — the keychain simply has no such entry, which for us
/// means "not paired". Spelled out rather than pulled from
/// `security-framework-sys` so this file needs only the safe crate.
#[cfg(target_os = "macos")]
const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;

/// The macOS keychain, through the Security framework directly.
///
/// This used to shell out to `/usr/bin/security`, which put the token on a
/// child process's **argv**. That is not the short-lived race the old comment
/// claimed: EndpointSecurity `NOTIFY_EXEC`, OpenBSM audit and every EDR agent
/// capture the full argv of every exec and ship it to a retained, off-box log,
/// so the capture is guaranteed rather than lucky. Disconnect does not revoke
/// (see [`TokenStore::clear`]), so a token leaked that way stays valid until
/// an operator revokes it on the hub — which nobody will, because nobody knows.
///
/// The framework call passes the secret in process memory and execs nothing.
#[cfg(target_os = "macos")]
impl TokenStore for OsTokenStore {
    fn get(&self) -> Result<Option<String>, String> {
        match security_framework::passwords::get_generic_password(SERVICE, ACCOUNT) {
            Ok(bytes) => {
                let token = String::from_utf8_lossy(&bytes).trim().to_string();
                Ok(if token.is_empty() { None } else { Some(token) })
            }
            Err(e) if e.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(None),
            // `e` is an OSStatus and its message; neither can contain the
            // secret, because the secret is never part of the query.
            Err(e) => Err(format!("keychain lookup failed: {e}")),
        }
    }

    fn set(&self, token: &str) -> Result<(), String> {
        security_framework::passwords::set_generic_password(
            SERVICE,
            ACCOUNT,
            token.trim().as_bytes(),
        )
        .map_err(|e| format!("keychain write failed: {e}"))
    }

    fn clear(&self) -> Result<(), String> {
        match security_framework::passwords::delete_generic_password(SERVICE, ACCOUNT) {
            // Already gone is success: Disconnect may be pressed twice.
            Ok(()) => Ok(()),
            Err(e) if e.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(()),
            Err(e) => Err(format!("keychain delete failed: {e}")),
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
        use std::io::Write;
        let path = self.fallback_path();
        // Owner-only **at creation**. `fs::write` then chmod left a window in
        // which the file existed at 0644 (0666 & ~umask) holding a bearer
        // credential for the whole fleet; any local user reading in that
        // window wins. `mode` applies only when O_CREAT actually creates the
        // file, so the chmod below still has to run for a file left behind by
        // an older build or restored from a backup.
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts
            .open(&path)
            .map_err(|e| format!("cannot open the hub token file: {e}"))?;
        // Trimmed on the way in as well as on the way out, so the bytes on
        // disk are the token and nothing else — a token is pasted into
        // Settings and arrives with whatever whitespace came with it.
        f.write_all(token.trim().as_bytes())
            .map_err(|e| format!("cannot write the hub token file: {e}"))?;
        // Same treatment `state.db` gets (SEC-11): owner-only. Still needed
        // for the pre-existing-file case above.
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

/// The file-backed fallback, which is what this Linux box actually runs.
///
/// These are the tests the review asked for: nothing exercised `set` or
/// `clear` before, which is exactly why the world-readable creation window
/// survived Task 1.
#[cfg(all(test, not(target_os = "macos")))]
mod fallback_tests {
    use super::*;

    fn store() -> (tempfile::TempDir, OsTokenStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = OsTokenStore::new(dir.path().to_path_buf());
        (dir, store)
    }

    #[test]
    fn an_unpaired_app_has_no_token() {
        let (_dir, store) = store();
        assert_eq!(store.get().unwrap(), None);
    }

    #[test]
    fn a_token_round_trips_and_clear_forgets_it() {
        let (_dir, store) = store();
        store.set("cl_abc123").unwrap();
        assert_eq!(store.get().unwrap().as_deref(), Some("cl_abc123"));
        store.set("cl_replaced").unwrap();
        assert_eq!(store.get().unwrap().as_deref(), Some("cl_replaced"));
        store.clear().unwrap();
        assert_eq!(store.get().unwrap(), None);
        // Clearing twice is not an error — Disconnect may be pressed twice.
        store.clear().unwrap();
    }

    /// Surrounding whitespace comes from a paste into Settings. `get` already
    /// trimmed on the way out; trimming on the way in means the stored bytes
    /// are the token and nothing else.
    #[test]
    fn a_pasted_token_is_trimmed_before_it_is_stored() {
        let (_dir, store) = store();
        store.set("  cl_abc123\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(store.fallback_path()).unwrap(),
            "cl_abc123",
            "the file must hold the token and nothing else"
        );
        assert_eq!(store.get().unwrap().as_deref(), Some("cl_abc123"));
    }

    /// The review's SHOULD-FIX: `fs::write` created the file at 0644 and
    /// chmodded afterwards, leaving a window in which any local user could
    /// read a bearer credential for the whole fleet.
    #[cfg(unix)]
    #[test]
    fn the_token_file_is_never_world_readable() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, store) = store();
        store.set("cl_abc123").unwrap();
        let mode = std::fs::metadata(store.fallback_path())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "mode was {mode:o}");
    }

    /// A file left behind by an older build (or by a restore) is tightened on
    /// the next write: creation flags only apply when the file is created.
    #[cfg(unix)]
    #[test]
    fn a_pre_existing_loose_file_is_tightened() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, store) = store();
        let path = store.fallback_path();
        std::fs::write(&path, "old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        store.set("cl_new").unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "mode was {mode:o}");
        assert_eq!(store.get().unwrap().as_deref(), Some("cl_new"));
    }

    /// A shorter token must not leave the tail of a longer one behind.
    #[test]
    fn replacing_a_token_truncates_the_file() {
        let (_dir, store) = store();
        store.set("cl_a_very_long_token_value").unwrap();
        store.set("cl_short").unwrap();
        assert_eq!(store.get().unwrap().as_deref(), Some("cl_short"));
    }

    /// The trait's contract: an implementation must never put the token into
    /// its error text.
    #[test]
    fn an_unwritable_store_fails_without_naming_the_token() {
        let dir = tempfile::tempdir().unwrap();
        // A directory where the token file's name is taken by a directory:
        // every write fails, deterministically and without a chmod dance.
        std::fs::create_dir(dir.path().join(ACCOUNT)).unwrap();
        let store = OsTokenStore::new(dir.path().to_path_buf());
        let e = store.set("cl_s3cret").expect_err("a write must fail");
        assert!(!e.contains("cl_s3cret"), "the error leaked the token: {e}");
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
