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
