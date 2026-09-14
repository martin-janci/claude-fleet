//! Asset catalog: harness-neutral skills / agents / hooks / MCP servers /
//! plugin refs kept in a git repo, rendered per harness, and compared with
//! what each host actually has installed.
//! Spec: docs/superpowers/specs/2026-09-14-asset-catalog-design.md

// These error codes are the contract for commands/git/scan logic landing in
// later catalog tasks, so they are unused today. See store.rs for the same
// pattern.
#![allow(dead_code)]

pub mod harness;
pub mod model;

pub const E_CATALOG_NOT_CONFIGURED: &str = "E_CATALOG_NOT_CONFIGURED";
pub const E_CATALOG_GIT: &str = "E_CATALOG_GIT";
pub const E_CATALOG_PARSE: &str = "E_CATALOG_PARSE";
pub const E_ASSET_UNSUPPORTED: &str = "E_ASSET_UNSUPPORTED";
pub const E_ASSET_EXISTS: &str = "E_ASSET_EXISTS";
pub const E_ASSET_NOT_FOUND: &str = "E_ASSET_NOT_FOUND";
