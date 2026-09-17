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
