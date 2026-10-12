//! The per-track channel document (design §5): mutable, signed, ordered by
//! `sequence`, written only by CI to the `update-channels` branch.
//!
//! This is where the manifest's "recommended / minimum-supported / mandatory /
//! rollback" live (F4), because every one of them is decided after a release
//! is cut.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::model::{Component, Track};
use crate::Version;

pub const CHANNEL_SCHEMA: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelDoc {
    pub schema: u32,
    pub track: Track,
    /// Strictly increasing per track. A reader refuses a lower one (replay).
    pub sequence: u64,
    pub generated_at: String,
    /// Past this, the document is stale: still good for "what is installed",
    /// never for offering or requiring an update (freeze).
    pub expires_at: String,
    /// The newest release on the track.
    pub current: Version,
    /// What `notify` / `automatic` target; lags `current` when the publisher
    /// holds a release back.
    pub recommended: Version,
    /// The publisher's signed floor per component (U6).
    #[serde(default)]
    pub minimum_supported: BTreeMap<String, Version>,
    #[serde(default)]
    pub mandatory: Vec<Mandatory>,
    /// The one release a target may be sent to below `minimum_supported`.
    #[serde(default)]
    pub rollback: Option<Version>,
    #[serde(default)]
    pub withdrawn: Vec<Withdrawn>,
    pub releases: Vec<ReleaseRef>,
    /// Keys the current key vouches for (rotation, design §10). Accepted only
    /// from a document that verified.
    #[serde(default)]
    pub next_keys: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Mandatory {
    pub version: Version,
    /// Empty means every component.
    #[serde(default)]
    pub components: Vec<String>,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub deadline: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Withdrawn {
    pub version: Version,
    #[serde(default)]
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReleaseRef {
    pub version: Version,
    /// URL of that release's `release-manifest.json`.
    pub manifest: String,
    /// Lowercase hex SHA-256 of the manifest's exact bytes.
    pub manifest_sha256: String,
    /// Signed additions to the release's manifest published after it
    /// (design §4, owner's answer to §13.2): fleet-mobile's APK, whose
    /// release runs after this one. An older reader ignores the field.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub amendments: Vec<AmendmentRef>,
}

/// One amendment of a release's manifest: which component it adds, where
/// its document lives, and the exact bytes' sha256.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AmendmentRef {
    pub component: String,
    pub manifest: String,
    pub manifest_sha256: String,
}

impl ChannelDoc {
    pub fn is_withdrawn(&self, v: &Version) -> bool {
        self.withdrawn.iter().any(|w| &w.version == v)
    }

    pub fn signed_minimum(&self, c: Component) -> Option<&Version> {
        self.minimum_supported.get(c.as_str())
    }

    pub fn release(&self, v: &Version) -> Option<&ReleaseRef> {
        self.releases.iter().find(|r| &r.version == v)
    }

    /// Whether `v` may be sent to `c` at all under the publisher's bounds:
    /// listed, not withdrawn, and not below the signed floor unless it is the
    /// signed rollback target (U6).
    pub fn permits(&self, c: Component, v: &Version) -> bool {
        self.release(v).is_some()
            && !self.is_withdrawn(v)
            && (self.signed_minimum(c).is_none_or(|min| v >= min)
                || self.rollback.as_ref() == Some(v))
    }

    /// The `mandatory` entries that cover `c`.
    pub fn mandatory_for(&self, c: Component) -> impl Iterator<Item = &Mandatory> {
        self.mandatory.iter().filter(move |m| {
            m.components.is_empty() || m.components.iter().any(|x| x == c.as_str())
        })
    }
}
