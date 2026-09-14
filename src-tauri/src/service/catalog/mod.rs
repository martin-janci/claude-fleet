//! Asset catalog: harness-neutral skills / agents / hooks / MCP servers /
//! plugin refs kept in a git repo, rendered per harness, and compared with
//! what each host actually has installed.
//! Spec: docs/superpowers/specs/2026-09-14-asset-catalog-design.md

// The error codes, `CATALOG`, and the `repo` module's public items are the
// contract for commands/git/scan logic landing in later catalog tasks; only
// this module's own tests call them today. See store.rs for the same
// pattern.
#![allow(dead_code)]

pub mod harness;
pub mod model;
pub mod repo;

pub const E_CATALOG_NOT_CONFIGURED: &str = "E_CATALOG_NOT_CONFIGURED";
pub const E_CATALOG_GIT: &str = "E_CATALOG_GIT";
pub const E_CATALOG_PARSE: &str = "E_CATALOG_PARSE";
pub const E_ASSET_UNSUPPORTED: &str = "E_ASSET_UNSUPPORTED";
pub const E_ASSET_EXISTS: &str = "E_ASSET_EXISTS";
pub const E_ASSET_NOT_FOUND: &str = "E_ASSET_NOT_FOUND";

/// The loaded catalog, process-wide. `None` until `load` succeeds. Both the
/// Tauri commands and the MCP tools read it; only `load` writes it.
pub static CATALOG: once_cell::sync::Lazy<std::sync::RwLock<Option<repo::Catalog>>> =
    once_cell::sync::Lazy::new(|| std::sync::RwLock::new(None));

pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// `CATALOG` is process-global, so tests that write it must serialise.
#[cfg(test)]
pub static CATALOG_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
