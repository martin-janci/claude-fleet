//! Tests only; see `Cargo.toml` for why this is a crate of its own. The test
//! lives in the library's unit-test target, so `cargo fleet-test` (`--lib
//! --bins`) runs it with everything else.

// A real hub and agent over a socket: the agent is Unix-only.
#[cfg(all(test, unix))]
mod agent_e2e;
