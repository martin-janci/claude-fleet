//! `fleet-agent` — the outbound transport agent.
//!
//! A host the hub cannot reach runs this: it dials `wss://<hub>/agent`, keeps
//! one authenticated connection open, and executes the frames the hub sends
//! (`fleet_proto`). Design:
//! `docs/superpowers/specs/2026-09-18-host-agent-design.md`.
//!
//! Task 2 landed only the frame protocol. `run`, `install` and `status` are
//! Task 6; until then the binary exists so the crate is a workspace member
//! that builds without the Tauri system libraries, and it refuses to pretend
//! it works.

fn main() {
    eprintln!(
        "fleet-agent {}: not implemented yet (the CLI lands in Task 6; \
         the frame protocol is in fleet-proto)",
        env!("CARGO_PKG_VERSION")
    );
    std::process::exit(1);
}
