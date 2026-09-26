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
//! * [`via_host`] — transports that run on a fleet host over SSH: `gh`
//!   (M6.1) and `curl` (M6.3).
//! * `e2e_loopback` — the e2e fake tracker's transport; only with the
//!   test-only `e2e` feature (work graph M10.2).

pub mod conn;
#[cfg(feature = "e2e")]
pub mod e2e_loopback;
pub mod http1;
pub mod https;
pub mod tls;
pub mod via_host;
