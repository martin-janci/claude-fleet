//! Hub↔hub federation (cycle 3). `wire`, `validate` and `backoff` are pure;
//! `apply`, `listen`, `dial` and `supervisor` do the I/O. Spec:
//! `docs/superpowers/specs/2026-09-24-hub-federation-design.md`.

pub mod apply;
pub mod backoff;
pub mod listen;
pub mod validate;
pub mod wire;

#[cfg(test)]
pub(crate) mod testkit {
    use crate::ssh::SshClient;
    use crate::store::Store;
    use std::sync::{Arc, Mutex};

    pub fn hub(fleet: &str) -> (Arc<Mutex<Store>>, Arc<SshClient>) {
        let s = Store::open_in_memory().unwrap();
        s.set_setting("fleet.id", fleet).unwrap();
        s.upsert_host("local").unwrap();
        (Arc::new(Mutex::new(s)), Arc::new(SshClient::new()))
    }

    pub fn session(store: &Mutex<Store>, name: &str) -> i64 {
        store
            .lock()
            .unwrap()
            .upsert_session(name, "local", None, None, 0, 0, "running", None)
            .unwrap()
    }

    /// A peer client token row. The digest is the name's hex, padded — unique
    /// per name (up to 32 bytes), where a length-derived one collided.
    pub fn peer_client(store: &Mutex<Store>, name: &str) -> i64 {
        let hex: String = name.bytes().map(|b| format!("{b:02x}")).collect();
        store
            .lock()
            .unwrap()
            .insert_client_token(name, &format!("{hex:0>64}"), "peer")
            .unwrap()
            .id
    }
}
