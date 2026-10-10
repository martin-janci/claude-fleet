//! Transport-agnostic command logic.
//!
//! Each function here is the real implementation of a claude-fleet operation.
//! It is callable from both the Tauri IPC command wrappers (`commands/`) and
//! the embedded MCP server (`mcp/`) — neither path is privileged.
//!
//! Service functions take plain references (`&Mutex<Store>`, `&Arc<SshClient>`,
//! `&Arc<CancellationRegistry>`) rather than `tauri::State`, so they carry no
//! dependency on the Tauri runtime and are directly unit-testable.

pub mod account_limits;
pub mod account_spend;
pub mod account_usage;
pub mod account_usage_poll;
pub mod add_account;
pub mod add_project;
pub mod address;
pub mod agent_install;
pub mod attachments;
pub mod attention;
pub mod bg_sessions;
pub mod catalog;
pub mod claude_print;
pub mod clipboard;
pub mod context;
pub mod context_help;
pub mod control_handoffs;
pub mod control_tokens;
pub mod debug_devices;
pub mod decide;
pub mod delivery;
pub mod diagnostics;
pub mod downloads;
pub mod drafts;
pub mod editor;
pub mod evidence;
pub mod forms;
pub mod fresh;
pub mod gc;
pub mod guides;
pub mod health;
pub mod hooks;
pub mod hooks_install;
pub mod host_check;
pub mod host_setup;
pub mod hosts;
pub mod hub;
pub mod library;
pub mod local_sync;
pub mod loops;
pub mod messages;
pub mod move_session;
pub mod names;
#[cfg(feature = "nl-detect")]
pub mod nl;
pub mod onboarding;
pub mod operator;
pub mod org_admin;
pub mod org_needs;
pub mod org_spend;
pub mod orgs;
pub mod outcome;
pub mod pane_intel;
pub mod peer;
pub mod playbooks;
pub mod pr_shepherd;
pub mod presence;
pub mod project_picks;
pub mod projects;
pub mod prompt_origin;
pub mod provision;
pub mod prs;
pub mod quick_replies;
#[cfg(test)]
mod reconcile_tests;
pub mod repair;
pub mod repair_tick;
pub mod repo;
pub mod repo_mutate;
pub mod repo_read;
pub mod reports;
pub mod rewind;
pub mod routines;
pub mod runs;
pub mod safe_kill;
pub mod sessions;
pub mod settings;
#[cfg(test)]
mod settings_doc_gen;
pub mod settings_review;
pub mod start_rules;
pub mod tasks;
pub mod tick;
pub mod trackers;
pub mod transcript;
pub mod tunnel;
pub mod update;
pub mod usage;
pub mod view_scope;
pub mod voice;
pub mod watch_summary;
pub mod work;
pub mod worktree_prune;
pub mod worktrees;
