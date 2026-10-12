//! Where core background tasks run.
//!
//! The Tauri app starts its ticks and the MCP server from the `setup`
//! closure on the main thread (inside macOS `did_finish_launching`), where
//! no tokio runtime is entered: a bare `tokio::spawn` there panics ("no
//! reactor running") and, because that callback cannot unwind, aborts the
//! process. The desktop therefore installs its runtime handle once, and
//! every core spawn goes through [`spawn`], which prefers the current
//! runtime (the daemon runs under `#[tokio::main]`) and falls back to the
//! installed one.

use std::future::Future;
use std::sync::OnceLock;
use tokio::runtime::Handle;
use tokio::task::JoinHandle;

static INSTALLED: OnceLock<Handle> = OnceLock::new();

/// Install the runtime an embedder wants core tasks to run on. Idempotent:
/// a second call is ignored.
pub fn install(handle: Handle) {
    let _ = INSTALLED.set(handle);
}

/// Spawn on the current tokio runtime when inside one, else on the installed
/// handle. Panics when neither exists — that is a bootstrap bug, not a
/// runtime condition.
pub fn spawn<F>(fut: F) -> JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    match Handle::try_current() {
        Ok(h) => h.spawn(fut),
        Err(_) => INSTALLED
            .get()
            .expect("fleet_core::rt::spawn called outside a tokio runtime and before rt::install")
            .spawn(fut),
    }
}

/// [`spawn`] when a runtime is reachable, else `None` (the future is
/// dropped). For best-effort follow-ups fired from code that synchronous
/// unit tests also drive, where neither a current nor an installed runtime
/// exists.
pub fn try_spawn<F>(fut: F) -> Option<JoinHandle<F::Output>>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    match Handle::try_current() {
        Ok(h) => Some(h.spawn(fut)),
        Err(_) => INSTALLED.get().map(|h| h.spawn(fut)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn spawn_uses_the_current_runtime() {
        let v = spawn(async { 41 + 1 }).await.unwrap();
        assert_eq!(v, 42);
    }

    #[test]
    fn spawn_from_a_plain_thread_uses_the_installed_handle() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        install(rt.handle().clone());
        // A plain OS thread: no runtime is current there.
        let out = std::thread::spawn(|| {
            let jh = spawn(async { "ran" });
            // Block on the join from outside any runtime.
            block_on_outside_any_runtime(jh)
        })
        .join()
        .unwrap();
        assert_eq!(out, "ran");
    }

    /// Minimal block_on so the test needs no extra crate: it builds a
    /// current-thread runtime just to join the handle.
    fn block_on_outside_any_runtime<T>(jh: JoinHandle<T>) -> T {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        rt.block_on(jh).unwrap()
    }
}

/// A single-flight flag a spawned pass holds: cleared when the guard drops,
/// however the pass ends, so a panic in one pass does not stop every later
/// one until restart (review r06 F6).
pub struct ClearOnDrop(Flag);

enum Flag {
    Static(&'static std::sync::atomic::AtomicBool),
    Shared(std::sync::Arc<std::sync::atomic::AtomicBool>),
}

impl ClearOnDrop {
    /// Guard a `static` flag the caller already set.
    pub fn of_static(flag: &'static std::sync::atomic::AtomicBool) -> Self {
        Self(Flag::Static(flag))
    }
    /// Guard a shared flag the caller already set.
    pub fn of_shared(flag: std::sync::Arc<std::sync::atomic::AtomicBool>) -> Self {
        Self(Flag::Shared(flag))
    }
}

impl Drop for ClearOnDrop {
    fn drop(&mut self) {
        let flag = match &self.0 {
            Flag::Static(f) => *f,
            Flag::Shared(f) => f.as_ref(),
        };
        flag.store(false, std::sync::atomic::Ordering::Release);
    }
}

#[cfg(test)]
mod clear_on_drop_tests {
    use super::ClearOnDrop;
    use std::sync::atomic::{AtomicBool, Ordering};

    static FLAG: AtomicBool = AtomicBool::new(false);

    #[test]
    fn a_panicking_holder_still_clears_its_flag() {
        FLAG.store(true, Ordering::SeqCst);
        let r = std::panic::catch_unwind(|| {
            let _g = ClearOnDrop::of_static(&FLAG);
            panic!("a pass that panics");
        });
        assert!(r.is_err());
        assert!(!FLAG.load(Ordering::SeqCst));
        let shared = std::sync::Arc::new(AtomicBool::new(true));
        drop(ClearOnDrop::of_shared(std::sync::Arc::clone(&shared)));
        assert!(!shared.load(Ordering::SeqCst));
    }
}

/// Keys being started, per store: the gap between a check ("is a run
/// open?") and the row that answers it later is SSH, so a second caller in
/// that gap passes the check too (review r06 F1, F2). The first caller
/// claims its key under the same lock as its check; the claim ends on drop.
/// `owner` tells stores apart (one process may serve several: tests do).
pub type KeySet = std::sync::Mutex<Option<std::collections::HashSet<(usize, String)>>>;

/// A claim on one key of a [`KeySet`]; released on drop, however the
/// start ends.
pub struct KeyedClaim {
    set: &'static KeySet,
    key: (usize, String),
}

/// Claim `key` for `owner` (`Arc::as_ptr` of its store), or `None` when it
/// is already claimed.
pub fn claim(set: &'static KeySet, owner: usize, key: String) -> Option<KeyedClaim> {
    let key = (owner, key);
    let fresh = set
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get_or_insert_with(Default::default)
        .insert(key.clone());
    // Built only once the lock is released, and only when the key was free:
    // a claim built and dropped under the lock would lock it again in its
    // `Drop` and deadlock.
    fresh.then(|| KeyedClaim { set, key })
}

impl Drop for KeyedClaim {
    fn drop(&mut self) {
        let mut guard = self.set.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(keys) = guard.as_mut() {
            keys.remove(&self.key);
        }
    }
}

#[cfg(test)]
mod claim_tests {
    use super::{claim, KeySet};

    static KEYS: KeySet = std::sync::Mutex::new(None);

    #[test]
    fn a_key_is_claimed_once_until_its_claim_drops() {
        let first = claim(&KEYS, 1, "a".into()).expect("free");
        assert!(claim(&KEYS, 1, "a".into()).is_none());
        let other_store = claim(&KEYS, 2, "a".into()).expect("another store's key");
        drop(first);
        assert!(claim(&KEYS, 1, "a".into()).is_some());
        drop(other_store);
    }
}
