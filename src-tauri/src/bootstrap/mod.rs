//! Startup helpers that `run()` calls before and during Tauri setup: env
//! recovery for GUI launches, the single-instance reaper and the MCP
//! control-API start. `run()` and the `generate_handler!` list stay in
//! `lib.rs`, where `mcp::doc_gen` reads them.

pub(crate) mod env;
pub(crate) mod mcp;
pub(crate) mod singleton;
