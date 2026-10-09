//! Staged rollouts and the maintenance window (update-channel design §7.4,
//! slice S9).
//!
//! A rollout opens one release of one component to growing waves of its
//! targets: a target is in wave `w` iff `fleet_update::in_cohort(target,
//! version, waves[w])`. `decide` holds everyone outside the open wave
//! (`not_in_wave`) and everyone while the rollout is paused
//! (`rollout_paused`). [`advance`] runs on the decision pusher's beat: after
//! `update.rollout_wave_secs` a wave whose failure ratio stays below the
//! rollout's `halt_failure_ratio` opens the next one; one at or above it
//! pauses the rollout, which `fleet_health` then names (`rollout_paused`).
//!
//! The window is `update.window`, a daily `HH:MM-HH:MM` in UTC. Only an
//! `automatic` component waits for it: an offer a person accepts is theirs
//! to time.

use std::sync::Mutex;

use fleet_update::decide::in_cohort;
use fleet_update::decide::Rollout;
use fleet_update::{Component, Mode, Version};
use serde::Serialize;

use crate::ipc_error::{codes, lock, IpcError};
use crate::service::settings;
use crate::store::{Store, UpdateRolloutRow};

/// What a rollout starts with when the operator names no waves.
pub const DEFAULT_WAVES: &[u8] = &[10, 50, 100];
/// The failure ratio that pauses a rollout when the operator names none.
pub const DEFAULT_HALT_FAILURE_RATIO: f64 = 0.2;
/// At most this many waves.
pub const MAX_WAVES: usize = 10;
/// Phases that count against a wave.
const FAILED_PHASES: &[&str] = &["failed", "rolling_back", "recovered", "rollback_failed"];

/// Whether `now` (unix seconds) falls inside the UTC daily range
/// `[from, until)`, in minutes of the day; a range may run past midnight.
pub fn in_window(from: u16, until: u16, now: i64) -> bool {
    let m = (now.rem_euclid(86_400) / 60) as u16;
    if from < until {
        m >= from && m < until
    } else {
        m >= from || m < until
    }
}

/// Decide rule 6's window: true when `mode` installs by itself and
/// `update.window` is set and does not contain `now`.
pub fn outside_window(store: &Store, mode: Mode, now: i64) -> bool {
    outside(
        &settings::get_string(store, settings::UPDATE_WINDOW),
        mode,
        now,
    )
}

/// [`outside_window`] for a given `window` (an org's own, say).
pub fn outside(window: &str, mode: Mode, now: i64) -> bool {
    if mode != Mode::Automatic {
        return false;
    }
    match settings::parse_time_range(window) {
        Some(Some((from, until))) => !in_window(from, until, now),
        _ => false,
    }
}

/// The component's active rollout as `decide` reads it.
pub fn for_decide(store: &Store, c: Component) -> Result<Option<Rollout>, IpcError> {
    Ok(store
        .update_rollout_active(c.as_str())?
        .and_then(|r| as_rollout(&r)))
}

fn as_rollout(r: &UpdateRolloutRow) -> Option<Rollout> {
    Some(Rollout {
        version: Version::parse(&r.version).ok()?,
        waves: r.waves.clone(),
        wave: r.wave as usize,
        paused: r.paused_at.is_some(),
    })
}

/// Waves must climb strictly, stay within 1–100, and end at 100.
pub fn validate_waves(waves: &[u8]) -> Result<(), IpcError> {
    let bad = |why: &str| {
        Err(IpcError::new(
            codes::E_INVALID,
            format!("waves {waves:?}: {why}"),
        ))
    };
    if waves.is_empty() || waves.len() > MAX_WAVES {
        return bad(&format!("between 1 and {MAX_WAVES} waves"));
    }
    if waves.iter().any(|w| *w == 0 || *w > 100) {
        return bad("each wave is a percent from 1 to 100");
    }
    if waves.windows(2).any(|p| p[0] >= p[1]) {
        return bad("each wave must be larger than the one before");
    }
    if waves.last() != Some(&100) {
        return bad("the last wave must be 100");
    }
    Ok(())
}

/// `update_admin { action: rollout_start }`. `version` must be a release the
/// verified channel lists and does not withdraw; `listed` says whether it is.
pub fn start(
    store: &Mutex<Store>,
    c: Component,
    version: &Version,
    listed: bool,
    waves: Option<Vec<u8>>,
    halt_failure_ratio: Option<f64>,
    now: i64,
) -> Result<UpdateRolloutRow, IpcError> {
    if !listed {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!(
                "{version} is not a release the verified channel offers {}",
                c.as_str()
            ),
        ));
    }
    let waves = waves.unwrap_or_else(|| DEFAULT_WAVES.to_vec());
    validate_waves(&waves)?;
    let ratio = halt_failure_ratio.unwrap_or(DEFAULT_HALT_FAILURE_RATIO);
    if !(0.0..=1.0).contains(&ratio) || ratio.is_nan() {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!("halt_failure_ratio {ratio}: between 0 and 1"),
        ));
    }
    let row =
        lock(store)?.insert_update_rollout(c.as_str(), &version.to_string(), &waves, ratio, now)?;
    super::decisions_may_have_changed();
    Ok(row)
}

fn active(s: &Store, c: Component) -> Result<UpdateRolloutRow, IpcError> {
    s.update_rollout_active(c.as_str())?.ok_or_else(|| {
        IpcError::new(
            codes::E_NOTFOUND,
            format!("{} has no active rollout", c.as_str()),
        )
    })
}

/// `rollout_pause`: hold every target that has not installed it yet.
pub fn pause(
    store: &Mutex<Store>,
    c: Component,
    reason: Option<&str>,
    now: i64,
) -> Result<UpdateRolloutRow, IpcError> {
    let s = lock(store)?;
    let r = active(&s, c)?;
    s.set_update_rollout_paused(
        r.id,
        Some((now, reason.unwrap_or("paused by the operator"))),
        now,
    )?;
    super::decisions_may_have_changed();
    active(&s, c)
}

/// `rollout_resume`: the current wave opens again, its soak clock restarted.
pub fn resume(store: &Mutex<Store>, c: Component, now: i64) -> Result<UpdateRolloutRow, IpcError> {
    let s = lock(store)?;
    let r = active(&s, c)?;
    s.set_update_rollout_paused(r.id, None, now)?;
    super::decisions_may_have_changed();
    active(&s, c)
}

/// `rollout_abort`: end it. Targets that installed it keep it; the rest are
/// offered `recommended` as before the rollout (a pin is the way back).
pub fn abort(store: &Mutex<Store>, c: Component, now: i64) -> Result<UpdateRolloutRow, IpcError> {
    let s = lock(store)?;
    let r = active(&s, c)?;
    s.end_update_rollout(r.id, "aborted", now)?;
    super::decisions_may_have_changed();
    s.update_rollout(r.id)?
        .ok_or_else(|| IpcError::new(codes::E_INTERNAL, "rollout vanished"))
}

/// One wave's outcome so far: targets in the open cohort that run the
/// release, and those whose last report is a failed install.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct WaveTally {
    pub in_wave: u32,
    pub installed: u32,
    pub failed: u32,
}

impl WaveTally {
    /// Failures over attempts; 0 when nobody has tried yet.
    pub fn failure_ratio(&self) -> f64 {
        let tried = self.installed + self.failed;
        if tried == 0 {
            0.0
        } else {
            f64::from(self.failed) / f64::from(tried)
        }
    }
}

pub fn tally(s: &Store, r: &UpdateRolloutRow) -> Result<WaveTally, IpcError> {
    let Ok(version) = Version::parse(&r.version) else {
        return Ok(WaveTally {
            in_wave: 0,
            installed: 0,
            failed: 0,
        });
    };
    let pct = r.waves.get(r.wave as usize).copied().unwrap_or(100);
    let mut t = WaveTally {
        in_wave: 0,
        installed: 0,
        failed: 0,
    };
    for o in s.update_observed_all()? {
        if o.component != r.component || !in_cohort(&o.target, &version, pct) {
            continue;
        }
        t.in_wave += 1;
        if o.version == r.version {
            t.installed += 1;
        } else if FAILED_PHASES.contains(&o.phase.as_str()) {
            t.failed += 1;
        }
    }
    Ok(t)
}

/// What [`advance`] did to one rollout.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Moved {
    pub component: String,
    pub version: String,
    /// `advanced`, `completed` or `halted`.
    pub what: &'static str,
    pub wave: u32,
}

/// Move every running rollout whose wave has soaked: open the next wave,
/// complete it after the last, or pause it when the wave's failure ratio
/// reached its halt ratio.
pub fn advance(store: &Mutex<Store>, now: i64) -> Result<Vec<Moved>, IpcError> {
    let s = lock(store)?;
    let soak = settings::get_secs(&s, settings::UPDATE_ROLLOUT_WAVE_SECS) as i64;
    let mut moved = Vec::new();
    for r in s.update_rollouts(0)? {
        if r.ended_at.is_some() || r.paused_at.is_some() || now - r.wave_started_at < soak {
            continue;
        }
        let t = tally(&s, &r)?;
        let ratio = t.failure_ratio();
        let what = if t.failed > 0 && ratio >= r.halt_failure_ratio {
            let reason = format!(
                "halted: {} of {} installs failed in wave {} ({}%)",
                t.failed,
                t.installed + t.failed,
                r.wave + 1,
                r.waves.get(r.wave as usize).copied().unwrap_or(100)
            );
            s.set_update_rollout_paused(r.id, Some((now, &reason)), now)?;
            "halted"
        } else if (r.wave as usize) + 1 < r.waves.len() {
            s.advance_update_rollout(r.id, r.wave + 1, now)?;
            "advanced"
        } else {
            s.end_update_rollout(r.id, "completed", now)?;
            "completed"
        };
        moved.push(Moved {
            component: r.component.clone(),
            version: r.version.clone(),
            what,
            wave: if what == "advanced" {
                r.wave + 1
            } else {
                r.wave
            },
        });
    }
    Ok(moved)
}
