//! Hub↔hub federation (cycle 3). `wire`, `validate` and `backoff` are pure;
//! `apply`, `listen`, `dial` and `supervisor` do the I/O. Spec:
//! `docs/superpowers/specs/2026-09-24-hub-federation-design.md`.

pub mod backoff;
pub mod validate;
pub mod wire;
