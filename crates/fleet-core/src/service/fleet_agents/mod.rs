//! Configurable agents: an agent described as data (`kind: fleet_agent` in
//! the asset catalog) rather than compiled into fleet. Design:
//! `docs/superpowers/specs/2026-10-12-configurable-agents-design.md`.
//!
//! Step 1 is the definitions: the typed model and its validator
//! ([`definition`]), templates ([`template`]) and the answer's schema and
//! conditions ([`answer`]). Nothing runs a definition yet; the catalog loads
//! and lints them and `fleet-hub agents define` checks one.

pub mod answer;
pub mod definition;
pub mod template;
#[cfg(test)]
mod tests;
