//! Configurable agents: an agent described as data (`kind: fleet_agent` in
//! the asset catalog) rather than compiled into fleet. Design:
//! `docs/superpowers/specs/2026-10-12-configurable-agents-design.md`.
//!
//! Step 1 is the definitions: the typed model and its validator
//! ([`definition`]), templates ([`template`]) and the answer's schema and
//! conditions ([`answer`]); the catalog loads and lints them and `fleet-hub
//! agents define` checks one.
//!
//! Step 2 is the stores: an instance (an `agent_rules` row with
//! `agent = fleet`) binds each store role ([`bindings`]); a grant checks the
//! input values and the bindings and asks each MCP binding for its tools on
//! the host ([`grant`], [`probe`]); the fleet store's read server serves a
//! run ([`records`]). Nothing runs an instance yet (step 3).

pub mod answer;
pub mod bindings;
pub mod definition;
pub mod grant;
pub mod probe;
pub mod records;
pub mod template;
#[cfg(test)]
mod tests;
