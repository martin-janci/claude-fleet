//! SSH self-diagnosis: classify an ssh client failure, and (later PRs)
//! diagnose it, fix it, and remember the user's standing grants.
//! Design: docs/superpowers/specs/2026-09-23-ssh-self-diagnosis-design.md

pub mod classify;

pub use classify::{classify, SshFailure, SshFailureKind};
