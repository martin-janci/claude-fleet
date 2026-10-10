//! claude-fleet core: everything the desktop app and the `fleet-hub` daemon
//! share. No Tauri dependency — see
//! `docs/superpowers/specs/2026-09-17-hub-daemon-design.md`.
//!
//! `service/` is the transport-agnostic command logic, `store/` the SQLite
//! layer, `ssh`/`tmux` the host transport, `mcp/` the control API server.

// `#[async_trait]` expands each async trait method into a `#[must_use]` fn that
// returns a boxed future, which is already `#[must_use]`; clippy 1.99 flags that
// macro output as `double_must_use`. It is not code we wrote — allow it crate-wide.
#![allow(clippy::double_must_use)]

pub mod agent;
pub mod agent_adapter;
pub mod app_version;
pub mod cancel;
pub mod claude_agents;
pub mod claude_cli;
#[cfg(test)]
mod desktop_command_tests;
pub mod events;
#[cfg(test)]
mod fleet_e2e_tests;
pub mod home;
pub mod http_client;
pub mod humanize;
pub mod ipc_error;
pub mod json;
pub mod logging;
pub mod mcp;
pub mod net;
#[cfg(test)]
mod no_eprintln_tests;
pub mod pages;
pub mod proc;
pub mod projects;
#[cfg(test)]
mod repo_files;
pub mod repo_url;
pub mod rt;
#[cfg(test)]
mod scope_guard_tests;
pub mod search_text;
pub mod service;
pub mod shell;
pub mod ssh;
pub mod ssh_config;
pub mod ssh_diag;
#[cfg(test)]
pub mod ssh_fake;
pub mod store;
pub mod tmux;
pub mod validate;
pub mod wire_contract;
pub mod wsl;
