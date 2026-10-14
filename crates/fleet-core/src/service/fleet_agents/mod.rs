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
//! run ([`records`]).
//!
//! Step 3 is the generic runner ([`runner`]): the `fleet` agent of the
//! agent runtime, whose poller turns each rule's trigger into jobs and whose
//! runner runs them and checks the answer against `run.output_schema`.
//!
//! Step 4 is the actions ([`actions`]): the answer's writes planned,
//! queued by the rule's level and sent by the `fleet` executor after its
//! send-time checks, with the loop guard.
//!
//! Step 5 is the grant form ([`form`]): an instance as a `fleet.form/1`
//! form (New instance / Edit), its answers turned into the grant.

pub mod actions;
pub mod answer;
pub mod bindings;
pub mod definition;
pub mod form;
pub mod grant;
pub mod probe;
pub mod records;
pub mod runner;
#[cfg(test)]
mod runner_tests;
pub mod template;
#[cfg(test)]
mod tests;
