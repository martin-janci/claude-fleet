//! The sync engine: applies rendered assets to hosts, tracking what it
//! wrote in a per-harness managed manifest so a later sync can update or
//! remove exactly what an earlier one added, and substituting `${NAME}`
//! secret placeholders into rendered plans before anything is written.
//!
//! `manifest` owns the on-host manifest file's shape and diffing against the
//! catalog; `secrets` resolves `${NAME}` values (host override > global >
//! built-ins) and substitutes them into a `RenderPlan`; `plan` consumes both
//! to decide, per asset and per host, what a sync would actually do, and
//! parks the result in a short-lived registry for the applier. The applier
//! itself (Task 6) and the commands that drive it (Task 7) are not
//! implemented yet, so nothing here is reachable from a non-test build.

pub mod manifest;
pub mod plan;
pub mod secrets;
