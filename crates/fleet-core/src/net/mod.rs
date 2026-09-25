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

pub mod conn;
#[cfg(any(test, feature = "e2e"))]
pub mod e2e_tracker;
pub mod http1;
pub mod https;
pub mod tls;
pub mod via_host;

/// The loopback base (`http://127.0.0.1:<port>`) the end-to-end fake tracker
/// listens on (work graph M10.2, [`e2e_tracker`] under the `e2e` feature).
/// Named in every build so one without the feature can refuse it.
pub const E2E_TRACKER_ENV: &str = "FLEET_E2E_TRACKER_URL";
