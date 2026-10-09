//! The pure decision function (U5, design §7.1).
//!
//! One function, two callers: the hub runs it for every target in its fleet
//! under the fleet policy, and `GitUpdateChannel` runs it for a standalone
//! client under its local policy. Everything it needs is an argument; it
//! reads no clock, no store and no network.
//!
//! The rules, first match wins:
//!
//! 1. no channel → `unknown`; a stale channel → `hold`;
//! 2. installed speaks nothing the hub serves → `client_too_new` (ahead) or
//!    `update_required` (behind);
//! 3. an operator pin → `rollback` / `update_*` / `up_to_date`, or `hold`
//!    when the publisher does not permit the pinned release;
//! 4. installed withdrawn, below the signed minimum, or below the policy
//!    floor → `update_required`;
//! 5. a publisher `mandatory` entry → `update_available` + mandatory before
//!    its deadline, `update_required` after;
//! 6. manual mode, a paused rollout, outside the wave or the window → `hold`;
//! 7. a newer permitted release → `update_available`;
//! 8. otherwise `up_to_date`.
//!
//! **Target selection invariant** (rules 2, 4, 5, 7): a target is only ever a
//! release the publisher permits, that carries an artifact for the caller's
//! platform, and whose compatibility window contains what the hub serves —
//! the hub never names a release that would strand its caller.

use std::collections::BTreeMap;

use crate::manifest::{Compatibility, ReleaseManifest};
use crate::model::{Component, Mode, Platform, Source, Track, Window};
use crate::time::parse_rfc3339;
use crate::verify::VerifiedChannel;
use crate::wire::{
    ChannelRef, Decision, DocRef, Reason, ReasonCode, Speaks, Status, Target, UPDATE_PROTO,
};
use crate::Version;

/// What the running hub serves. `None` in Git mode: a standalone client has
/// no hub to be compatible with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HubSpeaks {
    pub contract_serves: u32,
    pub agent_proto_accepts: Window,
}

impl HubSpeaks {
    pub fn from_compat(c: &Compatibility) -> Self {
        HubSpeaks {
            contract_serves: c.contract.hub_serves,
            agent_proto_accepts: c.agent_proto.hub_accepts,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Pin {
    pub version: Version,
    pub mandatory: bool,
}

/// One component's policy, resolved for one target (design §7.3).
#[derive(Debug, Clone, PartialEq)]
pub struct Policy {
    pub mode: Mode,
    /// The operator's floor, `update.<component>.minimum`.
    pub minimum: Option<Version>,
    /// The operator's desired version for this target or component.
    pub pin: Option<Pin>,
    /// Outside the maintenance window (always false until windows exist).
    pub outside_window: bool,
    pub check_interval_secs: u64,
}

impl Default for Policy {
    fn default() -> Self {
        Policy {
            mode: Mode::Notify,
            minimum: None,
            pin: None,
            outside_window: false,
            check_interval_secs: 21_600,
        }
    }
}

/// A staged rollout of `version` (design §7.4). A target is in wave `w` iff
/// its cohort value is below `waves[w]` percent.
#[derive(Debug, Clone, PartialEq)]
pub struct Rollout {
    pub version: Version,
    pub waves: Vec<u8>,
    pub wave: usize,
    pub paused: bool,
}

pub struct DecideInput<'a> {
    pub component: Component,
    pub platform: &'a Platform,
    pub installed: &'a Version,
    pub speaks: &'a Speaks,
    pub hub: Option<HubSpeaks>,
    pub channel: Option<&'a VerifiedChannel>,
    /// Verified manifests, by version. A release whose manifest is missing
    /// here is never a target.
    pub manifests: &'a BTreeMap<Version, ReleaseManifest>,
    pub policy: &'a Policy,
    pub rollout: Option<&'a Rollout>,
    /// Stable identity for cohorts: `client:<id>`, `agent:<alias>`, `hub:self`.
    pub target_id: &'a str,
    pub source: Source,
    /// The track the caller follows; reported even when there is no channel.
    pub track: Track,
    pub now: i64,
}

enum Fit {
    Ok,
    Ahead,
    Behind,
}

pub fn decide(i: &DecideInput) -> Decision {
    let base = |status, target: Option<Target>, code: ReasonCode, text: String| Decision {
        update_proto: UPDATE_PROTO,
        component: i.component,
        status,
        source: i.source,
        track: i.track,
        mode: i.policy.mode,
        installed: i.installed.clone(),
        target,
        reason: Reason { code, text },
        next_check_secs: i.policy.check_interval_secs,
    };

    // 1. The channel.
    let Some(ch) = i.channel else {
        return base(
            Status::Unknown,
            None,
            ReasonCode::NoChannel,
            "No verified release channel yet.".into(),
        );
    };
    if !ch.fresh {
        return base(
            Status::Hold,
            None,
            ReasonCode::ChannelStale,
            format!(
                "The {} channel document has expired; nothing is offered until it is re-signed.",
                ch.doc.track.as_str()
            ),
        );
    }
    let doc = &ch.doc;
    let recommended = &doc.recommended;

    // A release this caller may be sent to, with its target.
    let target_of = |v: &Version, mandatory: bool, deadline: Option<String>| -> Option<Target> {
        if !doc.permits(i.component, v) {
            return None;
        }
        let m = i.manifests.get(v)?;
        if let Some(hub) = &i.hub {
            if !release_fits(&m.compatibility, i.component, hub) {
                return None;
            }
        }
        let artifact = m.artifact_for(i.component, i.platform)?.clone();
        let r = doc.release(v)?;
        Some(Target {
            version: v.clone(),
            mandatory,
            deadline,
            manifest: DocRef {
                url: r.manifest.clone(),
                sha256: r.manifest_sha256.clone(),
            },
            channel: ChannelRef {
                sequence: doc.sequence,
            },
            url: artifact.url(&m.release.assets_base),
            mirror: None,
            artifact,
            evidence: None,
        })
    };
    // The newest permitted release in `(above, ceiling]`, newest first.
    let select = |above: Option<&Version>,
                  below: Option<&Version>,
                  ceiling: &Version,
                  mandatory: bool,
                  deadline: Option<String>| {
        let mut vs: Vec<&Version> = doc
            .releases
            .iter()
            .map(|r| &r.version)
            .filter(|v| {
                *v <= ceiling && above.is_none_or(|a| *v > a) && below.is_none_or(|b| *v < b)
            })
            .collect();
        vs.sort();
        vs.into_iter()
            .rev()
            .find_map(|v| target_of(v, mandatory, deadline.clone()))
    };
    let upgrade = |mandatory: bool, deadline: Option<String>| {
        select(Some(i.installed), None, recommended, mandatory, deadline)
    };
    // A *required* update may go past `recommended` up to `current` when
    // nothing at or below `recommended` fits the hub: a lagging recommendation
    // must not strand a caller the hub has already moved past.
    let required = || {
        upgrade(true, None).or_else(|| select(Some(i.installed), None, &doc.current, true, None))
    };

    // 2. Protocol compatibility with the hub.
    if let Some(hub) = &i.hub {
        match installed_fit(i.component, i.speaks, hub) {
            Some(Fit::Ahead) => {
                let target = if i.component.can_downgrade() {
                    select(None, Some(i.installed), &doc.current, false, None)
                } else {
                    None
                };
                return base(
                    Status::ClientTooNew,
                    target,
                    ReasonCode::ClientAhead,
                    format!(
                        "This {} {} is newer than its hub understands; the hub needs updating.",
                        i.component.as_str(),
                        i.installed
                    ),
                );
            }
            Some(Fit::Behind) => {
                let target = required();
                let code = if target.is_some() {
                    ReasonCode::Incompatible
                } else {
                    ReasonCode::NoCompatibleRelease
                };
                return base(
                    Status::UpdateRequired,
                    target,
                    code,
                    format!(
                        "{} {} can no longer talk to this hub.",
                        i.component.as_str(),
                        i.installed
                    ),
                );
            }
            Some(Fit::Ok) | None => {}
        }
    }

    // 3. An operator pin.
    if let Some(pin) = &i.policy.pin {
        if &pin.version == i.installed {
            return base(
                Status::UpToDate,
                None,
                ReasonCode::Pinned,
                format!("Pinned to {}.", pin.version),
            );
        }
        let Some(target) = target_of(&pin.version, pin.mandatory, None) else {
            return base(
                Status::Hold,
                None,
                ReasonCode::PinRefused,
                format!(
                    "Pinned to {}, which the release channel does not permit here (withdrawn, below its minimum, incompatible, or no artifact).",
                    pin.version
                ),
            );
        };
        let (status, text) = if &pin.version < i.installed {
            (
                Status::Rollback,
                format!("Rolled back to {} by the operator.", pin.version),
            )
        } else if pin.mandatory {
            (
                Status::UpdateRequired,
                format!("The operator requires {}.", pin.version),
            )
        } else {
            (
                Status::UpdateAvailable,
                format!("The operator set {}.", pin.version),
            )
        };
        return base(status, Some(target), ReasonCode::Pinned, text);
    }

    // 4. Floors.
    let floor = if doc.is_withdrawn(i.installed) {
        Some((
            ReasonCode::Withdrawn,
            format!("{} was withdrawn.", i.installed),
        ))
    } else if let Some(min) = doc.signed_minimum(i.component).filter(|m| i.installed < *m) {
        Some((
            ReasonCode::BelowSignedMinimum,
            format!("{} is below the supported minimum {min}.", i.installed),
        ))
    } else {
        i.policy
            .minimum
            .as_ref()
            .filter(|m| i.installed < *m)
            .map(|min| {
                (
                    ReasonCode::BelowPolicyMinimum,
                    format!("This fleet requires at least {min}."),
                )
            })
    };
    if let Some((code, text)) = floor {
        let target = required();
        let code = if target.is_some() {
            code
        } else {
            ReasonCode::NoCompatibleRelease
        };
        return base(Status::UpdateRequired, target, code, text);
    }

    // 5. The publisher's mandatory entries.
    if let Some(m) = doc
        .mandatory_for(i.component)
        .filter(|m| i.installed < &m.version)
        .min_by_key(|m| {
            m.deadline
                .as_deref()
                .and_then(parse_rfc3339)
                .unwrap_or(i64::MAX)
        })
    {
        let past = m
            .deadline
            .as_deref()
            .and_then(parse_rfc3339)
            .is_some_and(|d| i.now >= d);
        let target = upgrade(true, m.deadline.clone());
        if target.is_some() {
            let why = if m.reason.is_empty() {
                String::new()
            } else {
                format!(" ({})", m.reason)
            };
            return if past {
                base(
                    Status::UpdateRequired,
                    target,
                    ReasonCode::MandatoryPastDeadline,
                    format!("{} was required by its deadline{why}.", m.version),
                )
            } else {
                base(
                    Status::UpdateAvailable,
                    target,
                    ReasonCode::Mandatory,
                    format!("{} is mandatory{why}.", m.version),
                )
            };
        }
    }

    // 7's candidate, needed by 6 to say what is held.
    let Some(candidate) = upgrade(false, None) else {
        let newer_exists = doc.releases.iter().any(|r| {
            &r.version > i.installed
                && &r.version <= recommended
                && doc.permits(i.component, &r.version)
        });
        return if newer_exists {
            base(
                Status::Unknown,
                None,
                ReasonCode::NoArtifact,
                format!(
                    "A newer release exists, but none has a verified {} artifact for {}.",
                    i.component.as_str(),
                    i.platform.os_arch()
                ),
            )
        } else if i.installed > recommended {
            base(
                Status::UpToDate,
                None,
                ReasonCode::Ahead,
                format!("{} is ahead of the recommended {recommended}.", i.installed),
            )
        } else {
            base(
                Status::UpToDate,
                None,
                ReasonCode::UpToDate,
                format!("{} is current.", i.installed),
            )
        };
    };

    // 6. Holds.
    let hold = |code, text: String| base(Status::Hold, Some(candidate.clone()), code, text);
    if i.policy.outside_window {
        return hold(
            ReasonCode::OutsideWindow,
            format!("{} waits for the maintenance window.", candidate.version),
        );
    }
    if i.policy.mode == Mode::Manual {
        return hold(
            ReasonCode::ManualMode,
            format!(
                "{} is available; updates are manual here.",
                candidate.version
            ),
        );
    }
    if let Some(r) = i.rollout.filter(|r| r.version == candidate.version) {
        if r.paused {
            return hold(
                ReasonCode::RolloutPaused,
                format!("The {} rollout is paused.", r.version),
            );
        }
        let pct = r.waves.get(r.wave).copied().unwrap_or(100);
        if !in_cohort(i.target_id, &r.version, pct) {
            return hold(
                ReasonCode::NotInWave,
                format!("{} is rolling out; this one is in a later wave.", r.version),
            );
        }
    }

    // 7.
    let text = format!("{} is available.", candidate.version);
    base(
        Status::UpdateAvailable,
        Some(candidate),
        ReasonCode::NewerRecommended,
        text,
    )
}

/// Whether the installed build can talk to the hub at all. `None` when the
/// caller did not say what it speaks, or the component has no such link.
fn installed_fit(c: Component, speaks: &Speaks, hub: &HubSpeaks) -> Option<Fit> {
    match c {
        Component::Desktop | Component::Android | Component::Ios => {
            let w = speaks.contract_accepts?;
            Some(if w.contains(hub.contract_serves) {
                Fit::Ok
            } else if w.min > hub.contract_serves {
                Fit::Ahead
            } else {
                Fit::Behind
            })
        }
        Component::Agent => {
            let p = speaks.agent_proto?;
            Some(if hub.agent_proto_accepts.contains(p) {
                Fit::Ok
            } else if p > hub.agent_proto_accepts.max {
                Fit::Ahead
            } else {
                Fit::Behind
            })
        }
        Component::Hub => None,
    }
}

/// Whether a release, once installed, can talk to the hub.
fn release_fits(c: &Compatibility, component: Component, hub: &HubSpeaks) -> bool {
    match component {
        Component::Desktop => c.contract.desktop_accepts.contains(hub.contract_serves),
        Component::Android | Component::Ios => c
            .contract
            .mobile_accepts
            .is_some_and(|w| w.contains(hub.contract_serves)),
        Component::Agent => hub.agent_proto_accepts.contains(c.agent_proto.agent_speaks),
        Component::Hub => true,
    }
}

/// Stable per `(target, version)`, different across versions, so the same
/// targets are not always first.
pub fn in_cohort(target_id: &str, version: &Version, pct: u8) -> bool {
    use sha2::Digest as _;
    if pct >= 100 {
        return true;
    }
    let h = sha2::Sha256::digest(format!("{target_id}\n{version}").as_bytes());
    let v = u16::from_be_bytes([h[0], h[1]]) % 100;
    v < u16::from(pct)
}

#[cfg(test)]
mod tests {
    use super::in_cohort;
    use crate::Version;

    #[test]
    fn cohorts_are_stable_and_roughly_proportional() {
        let v = Version::new(0, 3, 4);
        let ids: Vec<String> = (0..2000).map(|n| format!("client:{n}")).collect();
        let in10 = ids.iter().filter(|id| in_cohort(id, &v, 10)).count();
        assert!((120..=280).contains(&in10), "{in10} of 2000 in a 10% wave");
        // Monotonic: whoever is in 10% is in 30%.
        assert!(ids
            .iter()
            .all(|id| !in_cohort(id, &v, 10) || in_cohort(id, &v, 30)));
        assert!(ids.iter().all(|id| in_cohort(id, &v, 100)));
        assert!(ids.iter().all(|id| !in_cohort(id, &v, 0)));
        // A different release draws a different first wave.
        let v2 = Version::new(0, 3, 5);
        assert!(ids
            .iter()
            .any(|id| in_cohort(id, &v, 10) != in_cohort(id, &v2, 10)));
    }
}
