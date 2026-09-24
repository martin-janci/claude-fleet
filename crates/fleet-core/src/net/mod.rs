//! Outbound HTTP for fleet-core (work graph M3.0).
//!
//! Hand-rolled HTTP/1.1 over `tokio-rustls` (ring) and the platform trust
//! store — the stack the desktop already used to reach a hub, lifted here so
//! tracker providers can use it without a second TLS stack entering the tree
//! (see the M3 plan's *Revisions* for why this beat `reqwest`).
//!
//! * [`http1`] — response-head parsing and chunked decoding.
//! * [`tls`] — the cached rustls client config.
//! * [`conn`] — connect (plain or TLS) and one `Connection: close` exchange.
//! * [`https`] — the [`https::HttpTransport`] seam providers talk to, its
//!   real [`https::DirectTransport`] and the scripted [`https::FakeTransport`].

pub mod conn;
pub mod http1;
pub mod https;
pub mod tls;
