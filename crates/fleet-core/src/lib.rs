//! claude-fleet core: everything the desktop app and the `fleet-hub` daemon
//! share. No Tauri dependency — see
//! `docs/superpowers/specs/2026-09-17-hub-daemon-design.md`.
//!
//! `service/` is the transport-agnostic command logic, `store/` the SQLite
//! layer, `ssh`/`tmux` the host transport, `mcp/` the control API server.

pub mod app_version;
pub mod cancel;
pub mod claude_agents;
pub mod claude_cli;
pub mod events;
#[cfg(test)]
mod fleet_e2e_tests;
pub mod humanize;
pub mod ipc_error;
pub mod logging;
pub mod mcp;
#[cfg(test)]
mod no_eprintln_tests;
pub mod projects;
pub mod repo_url;
pub mod rt;
pub mod service;
pub mod shell;
pub mod ssh;
pub mod ssh_config;
#[cfg(test)]
pub mod ssh_fake;
pub mod store;
pub mod tmux;
pub mod validate;
