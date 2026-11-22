//! The desktop's event bus: forwards every `RowChange` to the Svelte frontend
//! through `tauri::AppHandle::emit`. The trait and the test buses live in
//! `fleet_core::events`.

use fleet_core::events::{EventBus, RowChange};

/// Production event bus: forwards every event to the Tauri frontend.
///
/// Events are serialized immediately (a cheap in-memory operation) and handed
/// to a dedicated drain thread over an mpsc channel; the thread performs the
/// actual `AppHandle::emit`. This matters because the `Store` is mutated
/// while its `Mutex` is held: a bus call must NOT block, or it would stall
/// every other thread waiting on `store.lock()` for the duration of the
/// emit. A channel `send` is effectively instant.
///
/// Ordering is preserved (single channel, single consumer). Emit errors are
/// intentionally swallowed — if the webview isn't ready the Store mutation
/// has already committed and we don't want to roll it back.
pub struct AppHandleEventBus {
    // `mpsc::Sender` is `Send` but not `Sync`; the `EventBus` trait requires
    // `Sync`, so the sender lives behind a `Mutex`. The lock is held only for
    // the duration of a non-blocking `send`, so contention is negligible.
    tx: std::sync::Mutex<std::sync::mpsc::Sender<(&'static str, serde_json::Value)>>,
}

impl AppHandleEventBus {
    pub fn new(handle: tauri::AppHandle) -> Self {
        let (tx, rx) = std::sync::mpsc::channel::<(&'static str, serde_json::Value)>();
        std::thread::spawn(move || {
            // Lives for the lifetime of the app; exits when the bus (and so
            // the Sender) is dropped and `recv` returns Err.
            while let Ok((name, payload)) = rx.recv() {
                let _ = tauri::Emitter::emit(&handle, name, payload);
            }
        });
        Self {
            tx: std::sync::Mutex::new(tx),
        }
    }
}

impl EventBus for AppHandleEventBus {
    fn emit(&self, e: &RowChange) {
        if let Ok(tx) = self.tx.lock() {
            let _ = tx.send((e.name(), e.payload()));
        }
    }
}

/// The hub event bridge's end of the same bus.
///
/// This is where a remote event and a local one become one thing: both arrive
/// as a `(&'static str, serde_json::Value)` pair and go down the same channel,
/// to the same drain thread, which makes the same `Emitter::emit` call. There
/// is no second path to the frontend — which is what makes "the stores cannot
/// tell the difference" a property of the code rather than a claim about it.
///
/// Note what is *not* here: no `RowChange` is reconstructed. Remote mode has
/// no local `Store` and so no row to build one from, and rebuilding one by
/// deserialising the payload would quietly drop any field this build does not
/// know about, because every optional field carries `#[serde(default)]`. The
/// name is still a `&'static str` from `EVENT_NAMES`, resolved by
/// `backend::events::known_event_name`, so nothing off the wire can invent an
/// event the frontend did not already have.
impl crate::backend::events::RemoteEventSink for AppHandleEventBus {
    fn emit_remote(&self, name: &'static str, payload: serde_json::Value) {
        if let Ok(tx) = self.tx.lock() {
            let _ = tx.send((name, payload));
        }
    }
}

/// The event that tells the window its store is poisoned (review w19-1);
/// `StorePoisonBanner.svelte` listens for it.
pub const STORE_POISONED_EVENT: &str = "app:store-poisoned";

/// How often the poison watch looks at the store's lock.
pub const STORE_POISON_CHECK: std::time::Duration = std::time::Duration::from_secs(5);

/// Watch the store's lock for poison, the desktop's half of the hub's
/// `watch_store_poison` (review w19-1). A panic that held `lock(store)`
/// poisons it, and from then on every command answers `E_LOCK` until the app
/// restarts. The hub exits for its supervisor; a desktop has none, so this
/// calls `on_poisoned` on every check once the lock is poisoned (the window
/// may not be listening at the first one) and the window offers a restart.
/// Logs once.
pub fn watch_store_poison<T: Send + 'static>(
    store: std::sync::Arc<std::sync::Mutex<T>>,
    every: std::time::Duration,
    on_poisoned: impl Fn() + Send + 'static,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name("store-poison-watch".into())
        .spawn(move || {
            let mut logged = false;
            loop {
                std::thread::sleep(every);
                if store.is_poisoned() {
                    if !logged {
                        tracing::error!(
                            "the store lock is poisoned (a panic held it); every command \
                             fails until Orbit Fleet restarts"
                        );
                        logged = true;
                    }
                    on_poisoned();
                }
            }
        })
}

#[cfg(test)]
mod poison_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    #[test]
    fn the_poison_watch_says_so_only_once_the_lock_is_poisoned() {
        let store = Arc::new(Mutex::new(0u8));
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = Arc::clone(&calls);
        watch_store_poison(Arc::clone(&store), Duration::from_millis(5), move || {
            seen.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();
        std::thread::sleep(Duration::from_millis(40));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "a healthy lock says nothing"
        );
        let poisoner = Arc::clone(&store);
        let _ = std::thread::spawn(move || {
            let _guard = poisoner.lock().unwrap();
            panic!("poison the lock");
        })
        .join();
        assert!(store.is_poisoned());
        let deadline = Instant::now() + Duration::from_secs(5);
        while calls.load(Ordering::SeqCst) < 2 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            calls.load(Ordering::SeqCst) >= 2,
            "it keeps saying so for a late window"
        );
    }
}
