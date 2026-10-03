//! The hub side of the `fleet-agent` transport: a registry of live agent
//! connections, and an [`SshExec`](crate::ssh::SshExec) implementation that
//! talks to them.
//!
//! See `docs/superpowers/specs/2026-09-18-host-agent-design.md`. The wire
//! types live in `fleet-proto`, which the agent binary also compiles.
//!
//! ```text
//! service/*  ──▶ dyn SshExec ──┬──▶ SshClient       (unchanged)
//!                              └──▶ AgentTransport ──▶ AgentRegistry ──▶ one connection per host
//! ```
//!
//! Nothing here knows about WebSockets: a connection is an
//! `mpsc::UnboundedSender<HubFrame>` that some owner (Task 5's `/agent`
//! endpoint) drains into a socket, plus [`AgentRegistry::deliver`] for the
//! frames coming back. That is what lets the whole transport be tested
//! without a socket.

#[cfg(test)]
pub mod fake;
pub mod registry;
pub mod router;
pub mod transport;
pub mod ws;

pub use registry::{AgentHello, AgentRegistry, AgentStatus, ConnId};
pub use router::HostRouter;
pub use transport::AgentTransport;
