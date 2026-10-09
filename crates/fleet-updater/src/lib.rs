//! The library half of `fleet-updater`: what the Docker sidecar (this
//! crate's binary), `fleet-agent update` and `fleet-hub update apply` share
//! (update-channel design §8, §8.6; slices S6 and S9).
//!
//! - [`http`]: a small HTTP/1.1 client (TCP or TLS with the host's CA bundle).
//! - [`net`]: the hub as an update channel ([`net::HubHttp`]), and GitHub
//!   ([`net::GitFetch`]).
//! - [`binary`]: the binary target — a versioned install directory, a
//!   `current` symlink, a systemd unit restarted onto it, gated and rolled
//!   back like the container.
//! - [`common`]: the install rule, the release keys, the replay guard,
//!   pairing, and the database restore.
//!
//! Like `fleet-update`, this has no `fleet-core` dependency, so `fleet-agent`
//! may use it.

// `#[async_trait]` expands each async trait method into a `#[must_use]` fn that
// returns a boxed future, which is already `#[must_use]`; clippy 1.99 flags that
// macro output as `double_must_use`. It is not code we wrote — allow it crate-wide.
#![allow(clippy::double_must_use)]

pub mod binary;
pub mod common;
pub mod http;
pub mod net;
pub mod systemd;
