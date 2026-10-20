//! When a cron routine fires next, beyond its schedule (Control review
//! F16): fleet's own loops back off when nothing changes and wait out a
//! usage limit instead of retrying into it.
//!
//! * **No change** (`routines.backoff_on_no_change`, off by default): each
//!   run in a row that came to `nothing` (`outcome`, step 8.10) doubles the
//!   gap to the next fire, up to [`BACKOFF_MAX_SECS`]; the fire stays on
//!   the routine's schedule (the first scheduled time past the gap). A run
//!   that did work, failed or needs a person ends the streak, and the
//!   schedule is back. A run's `nothing` comes from Jev's reading of it
//!   (`decide::routine_run_outcome`), so without Jev there is no streak.
//! * **A limit** (`routines.pause_at_limit`, on): a fire skipped while its
//!   login's account is at `accounts.pause_at` holds the next one until the
//!   account's window resets ([`resume_at_of`]; a daily spend limit resets
//!   at 00:00 UTC), rather than skipping again at each scheduled time. A
//!   `retry_once` retry of a run that ended at a rate limit waits too.
//!
//! The decisions are PURE; [`next_fire`] reads the facts.

use crate::service::account_usage::{AccountUsage, FIVE_HOUR_SECS, WEEK_SECS};
use crate::service::settings;
use crate::store::{RoutineRow, RoutineRunRow, Store};

/// The longest a backed-off routine waits between fires.
pub const BACKOFF_MAX_SECS: i64 = 6 * 3600;
/// Runs read for a streak.
pub const STREAK_RUNS: i64 = 12;

/// PURE: how many runs in a row, newest first, came to `nothing`. Skipped
/// runs and the one still going are passed over; anything else ends it.
pub fn nothing_streak(runs_newest_first: &[RoutineRunRow]) -> u32 {
    let mut n = 0;
    for r in runs_newest_first {
        match r.state.as_str() {
            "skipped" | "running" => continue,
            _ if r.outcome.as_deref() == Some("nothing") => n += 1,
            _ => break,
        }
    }
    n
}

/// PURE: the gap after `streak` runs with nothing new, from the
/// schedule's own `base`: doubled per run, never past
/// [`BACKOFF_MAX_SECS`] (unless the schedule itself is longer).
pub fn backoff_gap(base: i64, streak: u32) -> i64 {
    if streak == 0 || base <= 0 {
        return base.max(0);
    }
    let cap = BACKOFF_MAX_SECS.max(base);
    base.saturating_mul(1i64 << streak.min(20)).min(cap)
}

/// PURE: the first scheduled time at or after `not_before`, walking the
/// schedule (`next_after(t)` is the first fire strictly after `t`) from
/// `first`. Bounded: a schedule that never gets there answers `None`.
pub fn first_at_or_after(
    first: i64,
    not_before: i64,
    next_after: impl Fn(i64) -> Option<i64>,
) -> Option<i64> {
    let mut t = first;
    for _ in 0..100_000 {
        if t >= not_before {
            return Some(t);
        }
        t = next_after(t)?;
    }
    None
}

/// PURE: when an account over `pause_at` (percent) gets under it again:
/// the latest reset among its live windows at or over the line. `None`
/// when it is under, or no such window says when it resets.
pub fn resume_at_of(usage: &AccountUsage, fetched_at: i64, pause_at: f64, now: i64) -> Option<i64> {
    [
        (&usage.five_hour, FIVE_HOUR_SECS),
        (&usage.seven_day, WEEK_SECS),
    ]
    .into_iter()
    .filter_map(|(w, len)| w.as_ref().map(|w| (w, len)))
    .filter(|(w, len)| w.utilization >= pause_at && w.live_at(Some(fetched_at), *len, now))
    .filter_map(|(w, _)| w.resets_at)
    .max()
}

/// When the login `profile` on `host` gets under `accounts.pause_at` again,
/// if it is over it now; `None` when it is not over, or nobody knows when.
pub fn limit_resume_at(s: &Store, host: &str, profile: Option<&str>, now: i64) -> Option<i64> {
    let acc = crate::service::account_limits::login_account(s, host, profile, now)
        .ok()
        .flatten()
        .filter(|a| a.over)?;
    if acc.daily.is_some() {
        return Some((now.div_euclid(86_400) + 1) * 86_400);
    }
    let pause_at = crate::service::account_limits::pause_at_pct(s);
    let snap = s
        .latest_usage_snapshots()
        .ok()?
        .into_iter()
        .find(|r| r.account_uuid == acc.login.account_uuid)?;
    resume_at_of(&snap.usage, snap.fetched_at, pause_at, now)
}

/// Why the next fire is later than the schedule says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Held {
    pub at: i64,
    pub why: String,
}

/// The next fire of cron routine `r` after its fire at `now` whose run is
/// `run` (`None` when none was recorded), and why it is later than the
/// schedule when it is.
pub fn next_fire(
    s: &Store,
    r: &RoutineRow,
    run: Option<&RoutineRunRow>,
    now: i64,
) -> (Option<i64>, Option<Held>) {
    let next_after = |t: i64| super::next_fire_of(r, t);
    let Some(first) = next_after(now) else {
        return (None, None);
    };
    let mut not_before = first;
    let mut why = None;
    if settings::get_bool(s, settings::ROUTINES_BACKOFF_ON_NO_CHANGE) {
        let runs = s.routine_runs(r.id, STREAK_RUNS).unwrap_or_default();
        let streak = nothing_streak(&runs);
        if streak > 0 {
            let base = next_after(first).map_or(first - now, |second| second - first);
            let gap = backoff_gap(base, streak);
            if now + gap > not_before {
                not_before = now + gap;
                why = Some(format!(
                    "its last {streak} run(s) found nothing new, so it waits {}",
                    super::tick::cap_words(gap)
                ));
            }
        }
    }
    if settings::get_bool(s, settings::ROUTINES_PAUSE_AT_LIMIT)
        && run.is_some_and(|run| run.state == "skipped")
    {
        if let Some(reset) = limit_resume_at(s, &r.host_alias, r.profile.as_deref(), now) {
            if reset > not_before {
                not_before = reset;
                why = Some("its account is at its usage limit until it resets".into());
            }
        }
    }
    match first_at_or_after(first, not_before, next_after) {
        Some(at) if at > first => (Some(at), why.map(|why| Held { at, why })),
        Some(at) => (Some(at), None),
        // A schedule that never reaches the hold: keep the hold itself.
        None => (
            Some(not_before),
            why.map(|why| Held {
                at: not_before,
                why,
            }),
        ),
    }
}

/// Whether a `retry_once` retry of `run` waits: it ended at a rate limit
/// and a fire now would not start either, because its routine's own login
/// is over the line and its fallback cannot take it (review w02-10: the
/// retry goes where [`super::tick::plan`] would send a fire, not back to
/// the run's host).
pub fn retry_waits(s: &Store, r: &RoutineRow, run: &RoutineRunRow, now: i64) -> bool {
    settings::get_bool(s, settings::ROUTINES_PAUSE_AT_LIMIT)
        && run.error_code.as_deref() == Some(super::fix::E_TURN_RATE_LIMIT)
        && crate::service::account_limits::over_limit(s, &r.host_alias, r.profile.as_deref(), now)
            .ok()
            .flatten()
            .is_some()
        && matches!(super::tick::plan(s, r, now), Ok(super::tick::Plan::Skip(_)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::account_usage::Window;

    fn run(state: &str, outcome: Option<&str>) -> RoutineRunRow {
        RoutineRunRow {
            id: 0,
            routine_id: 1,
            trigger: "cron".into(),
            trigger_ref: None,
            state: state.into(),
            reason: None,
            session_id: None,
            cost_micros: 0,
            scheduled_for: None,
            started_at: 0,
            finished_at: None,
            outcome: outcome.map(String::from),
            outcome_source: None,
            error_code: None,
            host_alias: None,
        }
    }

    #[test]
    fn a_streak_counts_nothing_in_a_row_past_skips() {
        let n = |o| run("done", Some(o));
        assert_eq!(nothing_streak(&[]), 0);
        assert_eq!(
            nothing_streak(&[
                run("running", None),
                n("nothing"),
                run("skipped", None),
                n("nothing"),
                n("did_work"),
                n("nothing"),
            ]),
            2
        );
        assert_eq!(
            nothing_streak(&[run("failed", Some("failed")), n("nothing")]),
            0
        );
        assert_eq!(
            nothing_streak(&[run("done", None), n("nothing")]),
            0,
            "no outcome yet"
        );
    }

    #[test]
    fn the_gap_doubles_to_six_hours() {
        let h = 3600;
        assert_eq!(backoff_gap(h, 0), h);
        assert_eq!(backoff_gap(h, 1), 2 * h);
        assert_eq!(backoff_gap(h, 2), 4 * h);
        assert_eq!(backoff_gap(h, 3), 6 * h);
        assert_eq!(backoff_gap(h, 40), 6 * h);
        assert_eq!(
            backoff_gap(24 * h, 3),
            24 * h,
            "a daily routine is never sooner"
        );
        assert_eq!(backoff_gap(0, 3), 0);
    }

    #[test]
    fn the_next_fire_stays_on_the_schedule() {
        let hourly = |t: i64| Some((t / 3600 + 1) * 3600);
        assert_eq!(first_at_or_after(3600, 3600, hourly), Some(3600));
        assert_eq!(
            first_at_or_after(3600, 3 * 3600 + 5, hourly),
            Some(4 * 3600)
        );
        assert_eq!(first_at_or_after(3600, 99, |_| None), Some(3600));
        assert_eq!(first_at_or_after(3600, 7200, |_| None), None);
    }

    #[test]
    fn a_limit_resumes_when_its_window_resets() {
        let now = 1_000_000;
        let w = |u: f64, at: Option<i64>| {
            Some(Window {
                utilization: u,
                resets_at: at,
            })
        };
        let usage = AccountUsage {
            five_hour: w(97.0, Some(now + 600)),
            seven_day: w(40.0, Some(now + 86_400)),
            seven_day_opus: None,
            seven_day_sonnet: None,
        };
        assert_eq!(resume_at_of(&usage, now, 90.0, now), Some(now + 600));
        let both = AccountUsage {
            seven_day: w(95.0, Some(now + 86_400)),
            ..usage.clone()
        };
        assert_eq!(resume_at_of(&both, now, 90.0, now), Some(now + 86_400));
        assert_eq!(resume_at_of(&usage, now, 99.0, now), None, "under the line");
        let reset = AccountUsage {
            five_hour: w(97.0, Some(now - 1)),
            ..usage.clone()
        };
        assert_eq!(resume_at_of(&reset, now, 90.0, now), None, "already reset");
        let unknown = AccountUsage {
            five_hour: w(97.0, None),
            ..usage
        };
        assert_eq!(resume_at_of(&unknown, now, 90.0, now), None);
    }
}
