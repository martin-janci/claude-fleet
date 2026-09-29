//! The one update state machine (F6, design §7.2).
//!
//! ```text
//! idle → checking → available → downloading → verifying → ready → installing → validating → success
//!                        failed ← (any phase from checking to validating)
//!                        failed → rolling_back → recovered | rollback_failed
//! ```
//!
//! Every platform reports these phases verbatim. Which of them it can reach
//! is the platform layer's business (no platform but Docker and the agent
//! reaches `rolling_back`); which transitions are legal is decided here.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdatePhase {
    #[default]
    Idle,
    Checking,
    Available,
    Downloading,
    Verifying,
    Ready,
    Installing,
    Validating,
    Success,
    Failed,
    RollingBack,
    Recovered,
    /// Terminal until an operator clears it: the previous build did not come
    /// back either. Never retried on its own.
    RollbackFailed,
    #[serde(other)]
    Unknown,
}

impl UpdatePhase {
    /// The wire spelling (the serde name), as stored in `update_observed`.
    pub fn as_str(self) -> &'static str {
        match self {
            UpdatePhase::Idle => "idle",
            UpdatePhase::Checking => "checking",
            UpdatePhase::Available => "available",
            UpdatePhase::Downloading => "downloading",
            UpdatePhase::Verifying => "verifying",
            UpdatePhase::Ready => "ready",
            UpdatePhase::Installing => "installing",
            UpdatePhase::Validating => "validating",
            UpdatePhase::Success => "success",
            UpdatePhase::Failed => "failed",
            UpdatePhase::RollingBack => "rolling_back",
            UpdatePhase::Recovered => "recovered",
            UpdatePhase::RollbackFailed => "rollback_failed",
            UpdatePhase::Unknown => "unknown",
        }
    }

    /// Whether `self → to` is a legal transition.
    pub fn can_become(self, to: UpdatePhase) -> bool {
        use UpdatePhase::*;
        match (self, to) {
            (Unknown, _) | (_, Unknown) => false,
            (Idle, Checking) => true,
            (Checking, Idle | Available) => true,
            // A person or a policy may decline an available update.
            (Available, Downloading | Idle) => true,
            (Downloading, Verifying) => true,
            (Verifying, Ready) => true,
            // `ready` may wait for a quiet point, or be abandoned.
            (Ready, Installing | Idle) => true,
            (Installing, Validating) => true,
            (Validating, Success) => true,
            (Checking | Downloading | Verifying | Ready | Installing | Validating, Failed) => true,
            (Failed, RollingBack | Idle) => true,
            (RollingBack, Recovered | RollbackFailed) => true,
            (Success | Recovered, Idle) => true,
            (RollbackFailed, Idle) => true,
            _ => false,
        }
    }

    /// Phases after which the installed build may already have changed:
    /// a failure here needs `rolling_back` (where the platform can), a failure
    /// before it is harmless.
    pub fn touches_install(self) -> bool {
        matches!(
            self,
            UpdatePhase::Installing | UpdatePhase::Validating | UpdatePhase::RollingBack
        )
    }

    /// Nothing is in flight.
    pub fn is_settled(self) -> bool {
        matches!(
            self,
            UpdatePhase::Idle
                | UpdatePhase::Success
                | UpdatePhase::Recovered
                | UpdatePhase::RollbackFailed
        )
    }
}

/// `(version, digest)` pairs a target failed on (design §7.2's loop breaker):
/// a converging target never retries one until its desired version changes or
/// an operator clears it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BadList {
    pub entries: Vec<BadEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BadEntry {
    pub version: crate::Version,
    #[serde(default)]
    pub digest: Option<String>,
    pub why: String,
    pub at: String,
}

impl BadList {
    /// A version is bad if an entry names it with the same digest, or with no
    /// digest at all (a bad version is bad under any digest).
    pub fn is_bad(&self, version: &crate::Version, digest: Option<&str>) -> bool {
        self.entries
            .iter()
            .any(|e| &e.version == version && (e.digest.is_none() || e.digest.as_deref() == digest))
    }

    pub fn add(&mut self, entry: BadEntry) {
        if !self.is_bad(&entry.version, entry.digest.as_deref()) {
            self.entries.push(entry);
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::UpdatePhase::*;
    use super::*;

    const ALL: [UpdatePhase; 13] = [
        Idle,
        Checking,
        Available,
        Downloading,
        Verifying,
        Ready,
        Installing,
        Validating,
        Success,
        Failed,
        RollingBack,
        Recovered,
        RollbackFailed,
    ];

    #[test]
    fn the_happy_path_is_legal() {
        let path = [
            Idle,
            Checking,
            Available,
            Downloading,
            Verifying,
            Ready,
            Installing,
            Validating,
            Success,
            Idle,
        ];
        for w in path.windows(2) {
            assert!(w[0].can_become(w[1]), "{:?} → {:?}", w[0], w[1]);
        }
    }

    #[test]
    fn the_rollback_path_is_legal() {
        for w in [Installing, Validating, Failed, RollingBack, Recovered, Idle].windows(2) {
            assert!(w[0].can_become(w[1]), "{:?} → {:?}", w[0], w[1]);
        }
        assert!(RollingBack.can_become(RollbackFailed));
    }

    #[test]
    fn no_shortcuts() {
        // Nothing installs without verifying, nothing succeeds without validating.
        assert!(!Downloading.can_become(Ready));
        assert!(!Downloading.can_become(Installing));
        assert!(!Available.can_become(Installing));
        assert!(!Installing.can_become(Success));
        assert!(!Failed.can_become(Success));
        assert!(!RollbackFailed.can_become(RollingBack));
        for p in ALL {
            assert!(!p.can_become(p), "{p:?} → itself");
            assert!(!p.can_become(Unknown) && !Unknown.can_become(p));
        }
    }

    #[test]
    fn every_phase_has_a_way_back_to_idle() {
        for p in ALL {
            let mut cur = p;
            let mut steps = 0;
            while cur != Idle {
                cur = *ALL
                    .iter()
                    .find(|n| {
                        cur.can_become(**n) && matches!(n, Idle | Failed | Recovered | RollingBack)
                    })
                    .unwrap_or_else(|| panic!("{p:?} is stuck at {cur:?}"));
                steps += 1;
                assert!(steps < 5, "{p:?} loops");
            }
        }
    }

    #[test]
    fn as_str_is_the_serde_name() {
        for p in ALL.into_iter().chain([Unknown]) {
            assert_eq!(
                serde_json::to_value(p).unwrap(),
                serde_json::Value::String(p.as_str().into()),
                "{p:?}"
            );
        }
    }

    #[test]
    fn bad_list() {
        let v = |s: &str| crate::Version::parse(s).unwrap();
        let mut bad = BadList::default();
        bad.add(BadEntry {
            version: v("0.3.4"),
            digest: Some("sha256:a".into()),
            why: "ready timeout".into(),
            at: "2026-09-30T00:00:00Z".into(),
        });
        assert!(bad.is_bad(&v("0.3.4"), Some("sha256:a")));
        assert!(!bad.is_bad(&v("0.3.4"), Some("sha256:b")));
        assert!(!bad.is_bad(&v("0.3.5"), Some("sha256:a")));
        bad.clear();
        assert!(!bad.is_bad(&v("0.3.4"), Some("sha256:a")));
    }
}
