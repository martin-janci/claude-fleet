//! The sync engine: applies rendered assets to hosts, tracking what it
//! wrote in a per-harness managed manifest so a later sync can update or
//! remove exactly what an earlier one added, and substituting `${NAME}`
//! secret placeholders into rendered plans before anything is written.
//!
//! `manifest` owns the on-host manifest file's shape and diffing against the
//! catalog; `secrets` resolves `${NAME}` values (host override > global >
//! built-ins) and substitutes them into a `RenderPlan`. Both are consumed by
//! the sync engine proper (Task 5/6), which is not implemented yet.

pub mod manifest;
pub mod secrets;
