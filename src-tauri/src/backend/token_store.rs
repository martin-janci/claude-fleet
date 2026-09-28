//! Where the hub client token lives.
//!
//! The desktop pairs with a `fleet-hub` as an ordinary client and keeps the
//! token it was handed. That token is a bearer credential for someone else's
//! fleet, so it deliberately does **not** go into `state.db` (which is
//! plaintext, backed up and copied around) and never reaches a log line — the
//! only thing this module ever hands out is the secret itself, to the caller
//! that asked for it.

/// A place to keep the hub client token: the OS credential store in the app
/// (the macOS keychain, Windows Credential Manager), an in-memory double in
/// tests.
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

/// The keychain entry's service/account pair on macOS, the credential's
/// `SERVICE/ACCOUNT` target name on Windows, and (the account alone) the file
/// name of the fallback on Linux — which is also the file an older Windows
/// build left behind.
#[cfg_attr(not(any(target_os = "macos", windows)), allow(dead_code))]
const SERVICE: &str = "claude-fleet";
const ACCOUNT: &str = "hub-client-token";

/// The real store: the macOS keychain through the Security framework
/// directly (see the `TokenStore` impl below — no more shelling out to
/// `/usr/bin/security`), Windows Credential Manager through the Win32 API,
/// and an owner-only file in the app data dir on Linux, which has no system
/// keychain we can reach without a new dependency.
pub struct OsTokenStore {
    /// Only the file fallback reads this (on Windows, only to move an older
    /// build's file into Credential Manager), but it is held unconditionally
    /// so the struct has one shape — and one constructor — on every platform.
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    data_dir: std::path::PathBuf,
    /// The Credential Manager target. `SERVICE/ACCOUNT` in the app; a test
    /// names its own so it never touches a real pairing on the machine.
    #[cfg(windows)]
    target: String,
}

impl OsTokenStore {
    pub fn new(data_dir: std::path::PathBuf) -> Self {
        Self {
            data_dir,
            #[cfg(windows)]
            target: format!("{SERVICE}/{ACCOUNT}"),
        }
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
        // `generic_password(PasswordOptions::new_generic_password(..))` rather
        // than the two-argument `get_generic_password`, which is `#[doc(hidden)]`
        // upstream (`security-framework-3.7.0/src/passwords.rs:39`) and so a
        // deprecation waiting to happen. Not a behaviour change: the hidden
        // function's whole body is this call.
        let options =
            security_framework::passwords::PasswordOptions::new_generic_password(SERVICE, ACCOUNT);
        match security_framework::passwords::generic_password(options) {
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

/// The file fallback: the store itself on Linux; on Windows only what an
/// older build left behind, read once and moved into Credential Manager.
#[cfg(not(target_os = "macos"))]
impl OsTokenStore {
    fn file_get(&self) -> Result<Option<String>, String> {
        match std::fs::read_to_string(self.fallback_path()) {
            Ok(s) => {
                let token = s.trim().to_string();
                Ok(if token.is_empty() { None } else { Some(token) })
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("cannot read the hub token file: {e}")),
        }
    }

    fn file_clear(&self) -> Result<(), String> {
        match std::fs::remove_file(self.fallback_path()) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("cannot remove the hub token file: {e}")),
        }
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
impl TokenStore for OsTokenStore {
    fn get(&self) -> Result<Option<String>, String> {
        self.file_get()
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
        self.file_clear()
    }
}

/// Windows Credential Manager: a generic credential, through the Win32 API
/// directly, for the same reason as the keychain above — the token stays in
/// process memory and no child process ever sees it. The credential belongs
/// to the signed-in user (`CRED_PERSIST_LOCAL_MACHINE`: this user on this
/// machine, never roamed), and Windows encrypts it with the user's logon
/// secret (DPAPI). The fallback file an older build wrote, owner-only on Unix
/// but a plain file on Windows, is moved in on first read and deleted.
#[cfg(windows)]
impl TokenStore for OsTokenStore {
    fn get(&self) -> Result<Option<String>, String> {
        if let Some(bytes) = wincred::read(&self.target)? {
            let token = String::from_utf8_lossy(&bytes).trim().to_string();
            return Ok(if token.is_empty() { None } else { Some(token) });
        }
        let Some(token) = self.file_get()? else {
            return Ok(None);
        };
        self.set(&token)?;
        Ok(Some(token))
    }

    fn set(&self, token: &str) -> Result<(), String> {
        wincred::write(&self.target, ACCOUNT, token.trim().as_bytes())?;
        // Only once the credential holds it: a failed write keeps the old
        // file as the one copy there is.
        self.file_clear()
    }

    fn clear(&self) -> Result<(), String> {
        wincred::delete(&self.target)?;
        self.file_clear()
    }
}

/// The three Credential Manager calls, with the `unsafe` kept in here. No
/// error text carries the secret: it is never part of a message, only of the
/// blob.
#[cfg(windows)]
mod wincred {
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_NOT_FOUND};
    use windows_sys::Win32::Security::Credentials::{
        CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE,
        CRED_TYPE_GENERIC,
    };

    /// NUL-terminated UTF-16, as every `W` call wants.
    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// The blob of `target`, or `None` when there is no such credential.
    pub(super) fn read(target: &str) -> Result<Option<Vec<u8>>, String> {
        let name = wide(target);
        let mut cred: *mut CREDENTIALW = std::ptr::null_mut();
        // SAFETY: `name` is NUL-terminated and outlives the call; `cred` is
        // an out-pointer the call sets only on success.
        let ok = unsafe { CredReadW(name.as_ptr(), CRED_TYPE_GENERIC, 0, &mut cred) };
        if ok == 0 {
            // SAFETY: no preconditions; read right after the failed call.
            let err = unsafe { GetLastError() };
            return if err == ERROR_NOT_FOUND {
                Ok(None)
            } else {
                Err(format!("Credential Manager read failed (error {err})"))
            };
        }
        // SAFETY: on success `cred` points at one CREDENTIALW the system
        // allocated, whose blob is `CredentialBlobSize` bytes at
        // `CredentialBlob` (null when empty). Copied out, then freed once.
        let bytes = unsafe {
            let c = &*cred;
            let bytes = if c.CredentialBlob.is_null() || c.CredentialBlobSize == 0 {
                Vec::new()
            } else {
                std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize).to_vec()
            };
            CredFree(cred as *const std::ffi::c_void);
            bytes
        };
        Ok(Some(bytes))
    }

    /// Create or replace the generic credential `target`.
    pub(super) fn write(target: &str, user: &str, secret: &[u8]) -> Result<(), String> {
        let mut name = wide(target);
        let mut user = wide(user);
        let size = u32::try_from(secret.len())
            .map_err(|_| "the token is too large for Credential Manager".to_string())?;
        // SAFETY: all-zero is a valid CREDENTIALW (null pointers, zero sizes
        // and flags); the fields that matter are set below.
        let mut cred: CREDENTIALW = unsafe { std::mem::zeroed() };
        cred.Type = CRED_TYPE_GENERIC;
        cred.TargetName = name.as_mut_ptr();
        cred.UserName = user.as_mut_ptr();
        cred.CredentialBlobSize = size;
        // The API takes `*mut` but only reads the blob.
        cred.CredentialBlob = secret.as_ptr() as *mut u8;
        cred.Persist = CRED_PERSIST_LOCAL_MACHINE;
        // SAFETY: every pointer in `cred` is valid for the duration of the
        // call, which copies what it keeps.
        let ok = unsafe { CredWriteW(&cred, 0) };
        if ok == 0 {
            // SAFETY: as in `read`.
            let err = unsafe { GetLastError() };
            return Err(format!("Credential Manager write failed (error {err})"));
        }
        Ok(())
    }

    /// Delete `target`; already gone is success (Disconnect may be pressed
    /// twice).
    pub(super) fn delete(target: &str) -> Result<(), String> {
        let name = wide(target);
        // SAFETY: `name` is NUL-terminated and outlives the call.
        let ok = unsafe { CredDeleteW(name.as_ptr(), CRED_TYPE_GENERIC, 0) };
        if ok == 0 {
            // SAFETY: as in `read`.
            let err = unsafe { GetLastError() };
            if err != ERROR_NOT_FOUND {
                return Err(format!("Credential Manager delete failed (error {err})"));
            }
        }
        Ok(())
    }
}

/// The real Credential Manager, under a target of the test's own, so a run
/// never touches a pairing on the machine it runs on.
#[cfg(all(test, windows))]
mod credential_manager_tests {
    use super::*;

    fn store(tag: &str) -> (tempfile::TempDir, OsTokenStore) {
        let dir = tempfile::tempdir().unwrap();
        let mut store = OsTokenStore::new(dir.path().to_path_buf());
        store.target = format!("{SERVICE}-test/{tag}-{}", std::process::id());
        let _ = store.clear();
        (dir, store)
    }

    #[test]
    fn a_token_round_trips_and_clear_forgets_it() {
        let (_dir, store) = store("round-trip");
        assert_eq!(store.get().unwrap(), None);
        store.set("  cl_abc123\n").unwrap();
        assert_eq!(store.get().unwrap().as_deref(), Some("cl_abc123"));
        store.set("cl_short").unwrap();
        assert_eq!(store.get().unwrap().as_deref(), Some("cl_short"));
        store.clear().unwrap();
        assert_eq!(store.get().unwrap(), None);
        store.clear().unwrap();
    }

    /// An older Windows build kept the token in a plain file beside
    /// `state.db`. The first read moves it in and deletes the file.
    #[test]
    fn an_old_token_file_is_moved_into_credential_manager() {
        let (_dir, store) = store("migrate");
        std::fs::write(store.fallback_path(), "cl_from_file\n").unwrap();
        assert_eq!(store.get().unwrap().as_deref(), Some("cl_from_file"));
        assert!(!store.fallback_path().exists(), "the file must be gone");
        assert_eq!(
            wincred::read(&store.target).unwrap().as_deref(),
            Some(&b"cl_from_file"[..])
        );
        store.clear().unwrap();
    }
}

/// The file-backed fallback, which is what this Linux box actually runs.
///
/// These tests close a real gap: nothing exercised `set` or `clear` before,
/// which is exactly why the world-readable creation window went unnoticed.
#[cfg(all(test, not(any(target_os = "macos", windows))))]
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

/// How long the launch waits on the token store before giving up on it.
pub const KEYCHAIN_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// A [`TokenStore`] whose `get` runs on its own thread and answers `Err`
/// past `wait`. The macOS keychain prompts on a locked keychain, and the
/// prompt can sit for hours while `Backend::resolve` blocks Tauri's setup
/// closure on the main thread (perf-logs §4). Past the bound the launch goes
/// on as "configured hub unusable" — the existing banner, with the reason —
/// and never as standalone (a paired app that guesses standalone is the
/// two-brains failure `backend::startup` exists to prevent). The blocked
/// thread finishes on its own whenever the keychain finally answers.
pub struct BoundedTokenStore {
    inner: std::sync::Arc<dyn TokenStore>,
    wait: std::time::Duration,
}

impl BoundedTokenStore {
    pub fn new(inner: std::sync::Arc<dyn TokenStore>, wait: std::time::Duration) -> Self {
        Self { inner, wait }
    }
}

impl TokenStore for BoundedTokenStore {
    fn get(&self) -> Result<Option<String>, String> {
        let (tx, rx) = std::sync::mpsc::channel();
        let inner = std::sync::Arc::clone(&self.inner);
        let started = std::time::Instant::now();
        tracing::info!("startup: reading the hub client token (a locked keychain prompts here)");
        std::thread::Builder::new()
            .name("hub-token-read".into())
            .spawn(move || {
                let _ = tx.send(inner.get());
            })
            .map_err(|e| format!("cannot start the token read: {e}"))?;
        match rx.recv_timeout(self.wait) {
            Ok(answer) => {
                tracing::info!(
                    elapsed_ms = started.elapsed().as_millis() as u64,
                    found = matches!(answer, Ok(Some(_))),
                    "startup: hub client token read finished"
                );
                answer
            }
            Err(_) => {
                tracing::warn!(
                    waited_secs = self.wait.as_secs(),
                    "startup: the token store did not answer; continuing with the hub marked \
                     unusable (unlock the keychain and relaunch)"
                );
                Err(format!(
                    "the token store did not answer within {} s (a locked keychain?) — unlock it and relaunch",
                    self.wait.as_secs()
                ))
            }
        }
    }

    fn set(&self, token: &str) -> Result<(), String> {
        self.inner.set(token)
    }

    fn clear(&self) -> Result<(), String> {
        self.inner.clear()
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
