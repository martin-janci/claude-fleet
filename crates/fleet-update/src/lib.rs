//! The update engine every claude-fleet component shares.
//!
//! Design: `docs/superpowers/specs/2026-09-28-update-channel-design.md`.
//! The pieces, in the order a check runs through them:
//!
//! - [`manifest`] / [`channel_doc`]: the two signed documents (U2). A release
//!   manifest is immutable and says *what* a release is; a channel document is
//!   mutable, ordered by `sequence`, and says what is recommended, required,
//!   withdrawn and the rollback target.
//! - [`verify`]: minisign verification of both, the replay / freeze guard, key
//!   rotation, and [`verify::verify_target`] — the one check every install goes
//!   through, whichever channel produced the decision (U1, U6).
//! - [`decide`]: the pure decision function (U5). The hub runs it for its
//!   fleet, `GitUpdateChannel` runs it for a standalone client.
//! - [`phase`]: the one state machine every platform reports (F6).
//! - [`channel`]: the `UpdateChannel` trait and its two implementations (F3).
//! - [`wire`]: the frozen `/update` request / response shapes (U4).
//!
//! Nothing in this crate does I/O except through the [`channel::Fetch`] and
//! [`channel::HubTransport`] seams, so it has no runtime of its own.

pub mod channel;
pub mod channel_doc;
pub mod decide;
pub mod manifest;
pub mod model;
pub mod phase;
pub mod time;
pub mod verify;
pub mod wire;

#[cfg(test)]
pub(crate) mod testkit;

pub use channel::{
    CheckOutcome, Fetch, GitUpdateChannel, HubTransport, HubUpdateChannel, MemorySequenceStore,
    SequenceStore, UpdateChannel, UpdateError,
};
pub use channel_doc::ChannelDoc;
pub use decide::{decide, DecideInput, HubSpeaks, Pin, Policy, Rollout};
pub use manifest::{Artifact, ReleaseManifest};
pub use model::{Component, Mode, Platform, Source, Track, Window};
pub use phase::UpdatePhase;
pub use verify::{TrustedKeys, VerifiedChannel, VerifiedTarget, VerifyError};
pub use wire::{CheckRequest, Decision, Reason, ReasonCode, Report, Status, UPDATE_PROTO};

/// A release version. The train is semver throughout (U9): `0.3.4`,
/// `0.3.4-rc.1`, and nightly `0.3.5-dev.17.ga83f19d`, whose ordering semver
/// already gets right (`dev.17` < `dev.18` < `rc.1` < the final release).
pub type Version = semver::Version;
