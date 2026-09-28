//! The `/update` wire (design §6), frozen under `update_proto: 1` (U4).
//!
//! It only ever grows additively. Every enum a reader receives has an
//! `#[serde(other)]` fallback, so a newer hub never breaks an older client's
//! parse. A breaking change is `update_proto: 2`, served beside 1.

use serde::{Deserialize, Serialize};

use crate::manifest::Artifact;
use crate::model::{Component, Mode, Platform, Source, Track, Window};
use crate::phase::UpdatePhase;
use crate::Version;

pub const UPDATE_PROTO: u32 = 1;

/// What is installed on the caller, as it reports itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Installed {
    pub version: Version,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_id: Option<String>,
    /// The running image digest, for a container.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
}

impl Installed {
    pub fn version(version: Version) -> Self {
        Installed {
            version,
            commit: None,
            build_id: None,
            digest: None,
        }
    }
}

/// The caller's own protocol window: trusted for compatibility only.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Speaks {
    /// A client's `[MIN_HUB_CONTRACT, MAX_HUB_CONTRACT]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contract_accepts: Option<Window>,
    /// An agent's `PROTO_VERSION`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_proto: Option<u32>,
}

/// `POST /update/check`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckRequest {
    pub update_proto: u32,
    pub component: Component,
    pub platform: Platform,
    pub installed: Installed,
    #[serde(default)]
    pub speaks: Speaks,
    #[serde(default)]
    pub phase: UpdatePhase,
    #[serde(default)]
    pub attempt: Option<String>,
}

/// The closed set a decision's `status` is drawn from (design §6.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    UpToDate,
    UpdateAvailable,
    UpdateRequired,
    ClientTooNew,
    Rollback,
    Hold,
    #[serde(other)]
    Unknown,
}

/// Why, as a stable code. The text beside it is for people and may change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasonCode {
    UpToDate,
    /// Installed is newer than `recommended` but still compatible.
    Ahead,
    NewerRecommended,
    /// Installed speaks no protocol the hub serves, and is behind it.
    Incompatible,
    /// Installed speaks no protocol the hub serves, and is ahead of it.
    ClientAhead,
    Pinned,
    /// The operator's pin is not a release the publisher permits (U6).
    PinRefused,
    Withdrawn,
    BelowSignedMinimum,
    BelowPolicyMinimum,
    Mandatory,
    MandatoryPastDeadline,
    ManualMode,
    RolloutPaused,
    NotInWave,
    OutsideWindow,
    ChannelStale,
    NoChannel,
    /// No verified artifact of a newer release installs on this platform.
    NoArtifact,
    /// A required update exists in principle but no permitted release fits.
    NoCompatibleRelease,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reason {
    pub code: ReasonCode,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocRef {
    pub url: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelRef {
    pub sequence: u64,
}

/// The signed documents a decision rests on, relayed verbatim (the exact
/// bytes that were signed), so the caller verifies the target itself without
/// a second fetch and without trusting whoever relayed them (U1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Evidence {
    pub channel: String,
    pub channel_sig: String,
    pub manifest: String,
    pub manifest_sig: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Target {
    pub version: Version,
    pub mandatory: bool,
    #[serde(default)]
    pub deadline: Option<String>,
    pub manifest: DocRef,
    pub channel: ChannelRef,
    pub artifact: Artifact,
    /// Resolved download URL of `artifact`, when it has one.
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<Evidence>,
}

/// The response to a check: what the caller should do (design §6.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Decision {
    pub update_proto: u32,
    pub component: Component,
    pub status: Status,
    pub source: Source,
    pub track: Track,
    pub mode: Mode,
    pub installed: Version,
    #[serde(default)]
    pub target: Option<Target>,
    pub reason: Reason,
    pub next_check_secs: u64,
}

/// `POST /update/report`: observed state and one state-machine transition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Report {
    pub update_proto: u32,
    pub component: Component,
    pub installed: Installed,
    pub phase: UpdatePhase,
    #[serde(default)]
    pub attempt: Option<String>,
    #[serde(default)]
    pub from: Option<Version>,
    #[serde(default)]
    pub to: Option<Version>,
    #[serde(default)]
    pub detail: serde_json::Value,
    #[serde(default)]
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_newer_hubs_values_do_not_break_the_parse() {
        let d: Decision = serde_json::from_str(
            r#"{"update_proto":1,"component":"desktop","status":"quarantined","source":"hub",
                "track":"stable","mode":"notify","installed":"0.3.3",
                "reason":{"code":"solar_flare","text":"x"},"next_check_secs":60,"new_field":true}"#,
        )
        .unwrap();
        assert_eq!(d.status, Status::Unknown);
        assert_eq!(d.reason.code, ReasonCode::Other);
        assert!(d.target.is_none());
    }
}
