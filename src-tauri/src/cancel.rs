//! Per-call cancellation tokens, keyed by a frontend-provided `call_id`.
//!
//! When the frontend invokes a long-running command via `invokeCmdAbortable`
//! (Task 17), it mints a `call_id`, passes it in the args, and registers an
//! `AbortSignal` listener. On abort it fires the `cancel_command(call_id)`
//! Tauri command, which calls `CancellationRegistry::cancel(call_id)` to fire
//! the token. The command handler then sees `token.cancelled().await` resolve
//! and returns `Err(E_CANCELLED)`.

use dashmap::DashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub struct CancellationRegistry {
    tokens: DashMap<u64, CancellationToken>,
    next_id: AtomicU64,
}

impl CancellationRegistry {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            tokens: DashMap::new(),
            next_id: AtomicU64::new(1),
        })
    }

    /// Mint a fresh token and bind it to a brand-new internal id (used by
    /// commands that don't receive a frontend `call_id`).
    pub fn register_anonymous(&self) -> (u64, CancellationToken) {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let token = CancellationToken::new();
        self.tokens.insert(id, token.clone());
        (id, token)
    }

    /// Bind a token under a frontend-supplied `call_id`. Replaces any existing
    /// token under the same id (the frontend is responsible for not re-using
    /// ids on overlapping calls — `nextCallId()` increments monotonically).
    pub fn bind(&self, call_id: u64, token: CancellationToken) {
        self.tokens.insert(call_id, token);
    }

    /// Cancel the token bound to `call_id`. If no token is bound (the call
    /// already finished or was never registered), this is a no-op — that's
    /// the right behaviour because the frontend has no way to know the call
    /// state when the user clicks Cancel.
    pub fn cancel(&self, call_id: u64) {
        if let Some((_, token)) = self.tokens.remove(&call_id) {
            token.cancel();
        }
    }

    /// Drop the token entry without firing cancellation. Used by handlers
    /// that completed successfully and want to release the registry slot.
    pub fn unregister(&self, call_id: u64) {
        self.tokens.remove(&call_id);
    }
}

/// RAII guard that unregisters a cancellation token when dropped — including
/// on a panic / unwind. Without it, a command that panics before reaching
/// its manual `unregister` call would leak the `DashMap` slot forever, so
/// the registry would grow unbounded over the lifetime of the process.
pub struct CancelGuard {
    reg: Arc<CancellationRegistry>,
    id: u64,
}

impl CancelGuard {
    pub fn new(reg: Arc<CancellationRegistry>, id: u64) -> Self {
        Self { reg, id }
    }
}

impl Drop for CancelGuard {
    fn drop(&mut self) {
        self.reg.unregister(self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn anonymous_token_can_be_cancelled() {
        let reg = CancellationRegistry::new();
        let (id, token) = reg.register_anonymous();
        let cancelled = tokio::spawn(async move {
            token.cancelled().await;
            true
        });
        reg.cancel(id);
        assert!(cancelled.await.unwrap());
    }

    #[tokio::test]
    async fn bind_then_cancel_via_external_id() {
        let reg = CancellationRegistry::new();
        let token = CancellationToken::new();
        reg.bind(7, token.clone());
        let cancelled = tokio::spawn(async move {
            token.cancelled().await;
            true
        });
        reg.cancel(7);
        assert!(cancelled.await.unwrap());
    }

    #[test]
    fn cancel_unknown_id_is_noop() {
        let reg = CancellationRegistry::new();
        reg.cancel(99); // must not panic
    }

    #[test]
    fn cancel_guard_unregisters_on_drop() {
        let reg = CancellationRegistry::new();
        let (id, _token) = reg.register_anonymous();
        assert_eq!(reg.tokens.len(), 1);
        {
            let _g = CancelGuard::new(Arc::clone(&reg), id);
        }
        assert_eq!(reg.tokens.len(), 0, "guard must release the slot on drop");
    }
}
