//! Transport-agnostic command logic.
//!
//! Each function here is the real implementation of a claude-fleet operation.
//! It is callable from both the Tauri IPC command wrappers (`commands/`) and
//! the embedded MCP server (`mcp/`) — neither path is privileged.
//!
//! Service functions take plain references (`&Mutex<Store>`, `&Arc<SshClient>`,
//! `&Arc<CancellationRegistry>`) rather than `tauri::State`, so they carry no
//! dependency on the Tauri runtime and are directly unit-testable.

pub mod health;
pub mod hooks;
pub mod hosts;
pub mod projects;
pub mod sessions;
