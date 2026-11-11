//! The limit census (`fleet-hub census limits`): how often every Claude
//! account the fleet reads was at its usage limit at the same time, counted
//! from the usage history the hub already keeps (`account_usage_snapshots`,
//! five weeks).
//!
//! It answers one question before anything is built on the answer: a way to
//! carry a session on with another agent while Claude is out everywhere is
//! worth building only if that happens more than rarely.
//!
//! Local and read-only by construction: it reads the store, adds up and
//! prints. Nothing is recorded and nothing is sent.
//!
//! The rule, at every moment of the window:
//! - An account is counted from its first reading on, and each reading
//!   stands until the next one. The hub reads usage every 5 to 30 minutes
//!   (`service::account_usage`), but only while something keeps the login's
//!   access token fresh: a spare login nothing runs on goes unread after a
//!   few hours (docs/accounts.md), and it is the login with room. So an
//!   account some login uses now (`Store::usage_login_accounts`) keeps its
//!   last reading however old it is.
//! - An account no login uses any more left the fleet at a time the history
//!   does not name: each of its readings stands for [`STANDS_FOR_SECS`] at
//!   most, and after that it is left out, at its limit or not.
//! - A counted account is at a line when its 5-hour or weekly window read at
//!   or past it and has not reset since ([`past_until`]): what a row's
//!   "Paused · limit" goes by at 100% (`service::attention`), and a start's
//!   refusal at `accounts.pause_at` (`service::account_limits`). Past the
//!   reset it has room until a reading says otherwise.
//! - "All at once" is every counted account at the line, with at least one
//!   counted.

use crate::ipc_error::{codes, IpcError};
use crate::service::account_limits::{self, utc_stamp};
use crate::service::account_usage::{
    AccountUsage, FIVE_HOUR_SECS, USAGE_BACKOFF_CAP_SECS, WEEK_SECS,
};
use crate::service::add_account::is_api_key_account;
use crate::store::{Store, UsageAccount, UsageSnapshotRow, USAGE_HISTORY_KEEP_SECS};
use serde::Serialize;
use std::collections::BTreeSet;

/// `days` when none is given: four weeks.
pub const DEFAULT_DAYS: u32 = 28;
/// The longest window: what the history keeps.
pub const MAX_DAYS: u32 = (USAGE_HISTORY_KEEP_SECS / 86_400) as u32;
/// How long a reading stands, when no newer one follows it, for an account
/// no login uses any more.
pub const STANDS_FOR_SECS: i64 = 86_400;
/// How long after a reading the hub counts as having been reading: two polls
/// at their slowest. A longer silence from every account is a stretch the
/// hub did not read usage in (stopped, or nothing ran).
pub const READ_FOR_SECS: i64 = 2 * USAGE_BACKOFF_CAP_SECS;
/// The oldest schema that has the history (`account_usage_snapshots`, 122).
pub const LIMIT_CENSUS_MIN_SCHEMA: i64 = 122;
/// A window that is used up: Claude answers no further prompt on the account.
pub const LIMIT_PCT: f64 = 100.0;
/// The line names, as the JSON spells them.
pub const LIMIT: &str = "limit";
pub const PAUSE_LINE: &str = "pause_line";
/// The stretches a line prints, newest last; the JSON has every one.
const PRINTED_STRETCHES: usize = 10;

/// What the history does not hold, so the census cannot count it.
pub const NOT_COUNTED: &[&str] = &[
    "a login without a usage reading: an API key has a daily spend limit and no usage windows, and an account signed in on macOS hosts only cannot be read (its token is in the Keychain)",
    "use the hub did not see: a login nothing runs on is not read after a few hours, so its last reading stands, at its limit until that window resets and with room after",
    "an account no login uses any more, from a day after each of its readings: the history does not say when it left",
    "a login that could not be used for another reason while its account had room (signed out, host down)",
    "the minutes between a limit being hit and the next reading (readings are 5 to 30 minutes apart)",
    "extra usage: an account with it on keeps working past its limit on pay-as-you-go money, and is counted at its limit all the same",
    "a weekly limit on one model only (Opus, Sonnet): fleet does not pause a row for it, and it is not counted here either",
];

/// What to count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LimitCensusOptions {
    pub days: u32,
    pub now: i64,
}

impl LimitCensusOptions {
    pub fn new(days: Option<u32>, now: i64) -> Result<Self, IpcError> {
        let days = days.unwrap_or(DEFAULT_DAYS);
        if days == 0 || days > MAX_DAYS {
            return Err(IpcError::new(
                codes::E_INVALID,
                format!(
                    "days must be 1-{MAX_DAYS} (the usage history keeps {MAX_DAYS}), got {days}"
                ),
            ));
        }
        Ok(Self { days, now })
    }

    /// The window: the last `days` days, to now.
    pub fn window(&self) -> Stretch {
        Stretch {
            from: self.now - i64::from(self.days) * 86_400,
            until: self.now,
        }
    }
}

/// A stretch of time in unix seconds; `until` is its first second after.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Stretch {
    pub from: i64,
    pub until: i64,
}

impl Stretch {
    pub fn secs(&self) -> i64 {
        self.until - self.from
    }
}

/// How often every counted account was at or past one line at once.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AllAtOnce {
    /// [`LIMIT`] (a window used up) or [`PAUSE_LINE`] (`accounts.pause_at`).
    pub line: &'static str,
    pub at_pct: f64,
    /// Every such stretch, oldest first.
    pub stretches: Vec<Stretch>,
    pub total_secs: i64,
    pub longest_secs: i64,
    /// UTC days with such a moment.
    pub days: u64,
    /// It is so at the end of the window.
    pub to_now: bool,
}

/// One account past one line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AccountPast {
    pub line: &'static str,
    /// Separate stretches.
    pub times: u64,
    pub secs: i64,
}

/// One counted account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AccountCensus {
    pub account_uuid: String,
    pub name: String,
    /// A login uses it now. One that none does is counted for a day after
    /// each reading only.
    pub login: bool,
    /// Past its limit it spends pay-as-you-go money instead of stopping.
    pub has_extra_usage: bool,
    /// Readings taken in the window.
    pub readings: u64,
    pub counted_secs: i64,
    /// Per line of the report, in its order.
    pub past: Vec<AccountPast>,
}

/// The whole census.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LimitCensusReport {
    pub schema_version: i64,
    pub days: u32,
    pub since: i64,
    pub until: i64,
    /// Seconds of the window in which at least one account was counted.
    pub counted_secs: i64,
    /// Seconds of it within [`READ_FOR_SECS`] of a reading: the hub was
    /// reading usage.
    pub read_secs: i64,
    /// The most accounts counted at one moment.
    pub most_at_once: usize,
    pub all_at_once: Vec<AllAtOnce>,
    /// By name.
    pub accounts: Vec<AccountCensus>,
    /// The accounts logins use now that have no reading, by name.
    pub unread_accounts: Vec<String>,
    pub not_counted: Vec<&'static str>,
}

/// When a reading taken at `at` stops being at or past `line`: the latest
/// reset of the windows that read so, as [`Window::live_at`] has it (a window
/// that names no reset holds for its own length). `None` when neither read
/// so.
///
/// [`Window::live_at`]: crate::service::account_usage::Window::live_at
fn past_until(usage: &AccountUsage, at: i64, line: f64) -> Option<i64> {
    [
        (&usage.five_hour, FIVE_HOUR_SECS),
        (&usage.seven_day, WEEK_SECS),
    ]
    .into_iter()
    .filter_map(|(w, len)| {
        let w = w.as_ref()?;
        (w.utilization.is_finite() && w.utilization >= line)
            .then(|| w.resets_at.unwrap_or(at.saturating_add(len)))
    })
    .max()
}

/// Add `from..until`, cut to `window`, to `to`: joined to the last stretch
/// when it begins inside it or where it ends. Stretches come in order.
fn add(to: &mut Vec<Stretch>, from: i64, until: i64, window: Stretch) {
    let (from, until) = (from.max(window.from), until.min(window.until));
    if until <= from {
        return;
    }
    match to.last_mut() {
        Some(last) if from <= last.until => last.until = last.until.max(until),
        _ => to.push(Stretch { from, until }),
    }
}

/// Where one account is counted and where it is past each line.
struct Counted {
    counted: Vec<Stretch>,
    /// Per line, in the lines' order.
    past: Vec<Vec<Stretch>>,
}

/// One account's readings, oldest first, as stretches of `window`. `login`:
/// a login uses the account now, so each reading stands until the next.
fn counted(rows: &[UsageSnapshotRow], lines: &[f64], window: Stretch, login: bool) -> Counted {
    let mut c = Counted {
        counted: Vec::new(),
        past: vec![Vec::new(); lines.len()],
    };
    for (i, r) in rows.iter().enumerate() {
        let at = r.fetched_at;
        let next = rows.get(i + 1).map_or(i64::MAX, |n| n.fetched_at);
        let stands_until = if login {
            next
        } else {
            next.min(at.saturating_add(STANDS_FOR_SECS))
        };
        add(&mut c.counted, at, stands_until, window);
        for (line, past) in lines.iter().zip(&mut c.past) {
            if let Some(until) = past_until(&r.usage, at, *line) {
                add(past, at, until.min(stands_until), window);
            }
        }
    }
    c
}

/// Walk time over every account: `seen(stretch, counted, past)` for each
/// stretch in which the number of accounts counted, and of those past the
/// line, stays the same. `past` stretches lie inside their account's
/// `counted` ones.
fn walk<'a>(
    accounts: impl Iterator<Item = (&'a [Stretch], &'a [Stretch])>,
    mut seen: impl FnMut(Stretch, usize, usize),
) {
    // (when, counted ±1, past ±1)
    let mut turns: Vec<(i64, i64, i64)> = Vec::new();
    for (counted, past) in accounts {
        for s in counted {
            turns.push((s.from, 1, 0));
            turns.push((s.until, -1, 0));
        }
        for s in past {
            turns.push((s.from, 0, 1));
            turns.push((s.until, 0, -1));
        }
    }
    turns.sort_unstable_by_key(|t| t.0);
    let (mut counted, mut past) = (0i64, 0i64);
    let mut i = 0;
    while i < turns.len() {
        let at = turns[i].0;
        while i < turns.len() && turns[i].0 == at {
            counted += turns[i].1;
            past += turns[i].2;
            i += 1;
        }
        if let Some(next) = turns.get(i) {
            seen(
                Stretch {
                    from: at,
                    until: next.0,
                },
                usize::try_from(counted).unwrap_or(0),
                usize::try_from(past).unwrap_or(0),
            );
        }
    }
}

/// The UTC days `stretches` touch.
fn utc_days(stretches: &[Stretch]) -> u64 {
    let days: BTreeSet<i64> = stretches
        .iter()
        .flat_map(|s| s.from.div_euclid(86_400)..=(s.until - 1).div_euclid(86_400))
        .collect();
    days.len() as u64
}

/// Count `rows` (account by account, each oldest first) over `window`.
/// `logins` are the accounts a login uses now. `pause_at` is the fleet's
/// `accounts.pause_at`; at 100 it is the limit itself and is not counted a
/// second time.
pub fn census_of(
    rows: &[UsageSnapshotRow],
    known: &[UsageAccount],
    logins: &[String],
    pause_at: f64,
    opts: &LimitCensusOptions,
    schema_version: i64,
) -> LimitCensusReport {
    let window = opts.window();
    let mut lines = vec![(LIMIT, LIMIT_PCT)];
    if pause_at < LIMIT_PCT {
        lines.push((PAUSE_LINE, pause_at));
    }
    let at: Vec<f64> = lines.iter().map(|l| l.1).collect();

    let per_account: Vec<(&str, u64, Counted)> = rows
        .chunk_by(|a, b| a.account_uuid == b.account_uuid)
        .map(|rows| {
            let readings = rows
                .iter()
                .filter(|r| r.fetched_at >= window.from && r.fetched_at < window.until)
                .count() as u64;
            let uuid = rows[0].account_uuid.as_str();
            let login = logins.iter().any(|l| l == uuid);
            (uuid, readings, counted(rows, &at, window, login))
        })
        .filter(|(_, _, c)| !c.counted.is_empty())
        .collect();

    let (mut counted_secs, mut most_at_once) = (0, 0);
    let none: &[Stretch] = &[];
    walk(
        per_account
            .iter()
            .map(|(_, _, c)| (c.counted.as_slice(), none)),
        |s, counted, _| {
            if counted > 0 {
                counted_secs += s.secs();
            }
            most_at_once = most_at_once.max(counted);
        },
    );
    let mut read_at: Vec<i64> = rows.iter().map(|r| r.fetched_at).collect();
    read_at.sort_unstable();
    let mut read = Vec::new();
    for at in read_at {
        add(&mut read, at, at.saturating_add(READ_FOR_SECS), window);
    }

    let all_at_once = lines
        .iter()
        .enumerate()
        .map(|(i, (line, at_pct))| {
            let mut stretches = Vec::new();
            walk(
                per_account
                    .iter()
                    .map(|(_, _, c)| (c.counted.as_slice(), c.past[i].as_slice())),
                |s, counted, past| {
                    if counted > 0 && past == counted {
                        add(&mut stretches, s.from, s.until, window);
                    }
                },
            );
            AllAtOnce {
                line,
                at_pct: *at_pct,
                total_secs: stretches.iter().map(Stretch::secs).sum(),
                longest_secs: stretches.iter().map(Stretch::secs).max().unwrap_or(0),
                days: utc_days(&stretches),
                to_now: stretches.last().is_some_and(|s| s.until == window.until),
                stretches,
            }
        })
        .collect();

    let name = |uuid: &str| known.iter().find(|k| k.uuid == uuid);
    let mut accounts: Vec<AccountCensus> = per_account
        .iter()
        .map(|(uuid, readings, c)| AccountCensus {
            account_uuid: (*uuid).to_string(),
            name: name(uuid).map_or_else(|| (*uuid).to_string(), |k| k.name.clone()),
            login: logins.iter().any(|l| l == uuid),
            has_extra_usage: name(uuid).is_some_and(|k| k.has_extra_usage),
            readings: *readings,
            counted_secs: c.counted.iter().map(Stretch::secs).sum(),
            past: lines
                .iter()
                .zip(&c.past)
                .map(|((line, _), past)| AccountPast {
                    line,
                    times: past.len() as u64,
                    secs: past.iter().map(Stretch::secs).sum(),
                })
                .collect(),
        })
        .collect();
    accounts.sort_by(|a, b| (&a.name, &a.account_uuid).cmp(&(&b.name, &b.account_uuid)));

    let mut unread_accounts: Vec<String> = logins
        .iter()
        .filter(|l| !rows.iter().any(|r| r.account_uuid == **l))
        .map(|l| {
            let called = name(l).map_or(l.as_str(), |k| k.name.as_str());
            if is_api_key_account(l) {
                format!("{called} (API key)")
            } else {
                called.to_string()
            }
        })
        .collect();
    unread_accounts.sort();

    LimitCensusReport {
        schema_version,
        days: opts.days,
        since: window.from,
        until: window.until,
        counted_secs,
        read_secs: read.iter().map(Stretch::secs).sum(),
        most_at_once,
        all_at_once,
        accounts,
        unread_accounts,
        not_counted: NOT_COUNTED.to_vec(),
    }
}

/// Count the store's usage history.
pub fn limit_census(
    store: &Store,
    opts: &LimitCensusOptions,
) -> Result<LimitCensusReport, IpcError> {
    let schema_version = store.schema_version()?;
    if schema_version < LIMIT_CENSUS_MIN_SCHEMA {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!(
                "this database is at schema {schema_version}; the usage history starts at \
                 {LIMIT_CENSUS_MIN_SCHEMA} (open it once with a current fleet to migrate it)"
            ),
        ));
    }
    Ok(census_of(
        // With each account's last reading from before the window: it still
        // stands as the window begins.
        &store.usage_history_all(opts.window().from)?,
        &store.usage_accounts()?,
        &store.usage_login_accounts()?,
        account_limits::pause_at_pct(store),
        opts,
        schema_version,
    ))
}

/// "45 s", "12 min", "5 h 12 min", "2 d 3 h": the two largest units.
fn span(secs: i64) -> String {
    let (d, h, m) = (secs / 86_400, secs % 86_400 / 3_600, secs % 3_600 / 60);
    match (d, h, m) {
        (0, 0, 0) => format!("{secs} s"),
        (0, 0, m) => format!("{m} min"),
        (0, h, 0) => format!("{h} h"),
        (0, h, m) => format!("{h} h {m} min"),
        (d, 0, _) => format!("{d} d"),
        (d, h, _) => format!("{d} d {h} h"),
    }
}

fn times(n: u64) -> String {
    match n {
        1 => "once".into(),
        n => format!("{n} times"),
    }
}

impl AllAtOnce {
    /// "at their limit (100% of the 5-hour or weekly window)".
    fn words(&self) -> String {
        match self.line {
            LIMIT => format!(
                "at their limit ({:.0}% of the 5-hour or weekly window)",
                self.at_pct
            ),
            _ => format!(
                "past the pause line ({:.0}%, accounts.pause_at)",
                self.at_pct
            ),
        }
    }
}

impl LimitCensusReport {
    /// The census as lines for a terminal.
    pub fn lines(&self) -> Vec<String> {
        let mut v = vec![
            "limit census: how often every Claude account was at its limit at the same time".into(),
            format!(
                "window: {} days to {}; schema {}",
                self.days,
                utc_stamp(self.until),
                self.schema_version
            ),
        ];
        if self.accounts.is_empty() {
            v.push(String::new());
            v.push("no usage reading in this window: nothing to count".into());
        } else {
            v.push(format!(
                "accounts counted: {}, for {} of the window; the hub read usage during {} of it",
                match (self.accounts.len(), self.most_at_once) {
                    (1, _) => "1 (so \"all at once\" is that account alone)".to_string(),
                    (n, most) if most == n => n.to_string(),
                    (n, most) => format!("{n} (at most {most} at one time)"),
                },
                span(self.counted_secs),
                span(self.read_secs)
            ));
            for a in &self.all_at_once {
                v.push(String::new());
                v.push(format!("all {} at once:", a.words()));
                if a.stretches.is_empty() {
                    v.push("  never".into());
                    continue;
                }
                let share = a.total_secs as f64 * 100.0 / self.counted_secs.max(1) as f64;
                v.push(format!(
                    "  {} on {} day{}; {} in total ({} of the time counted); longest {}",
                    times(a.stretches.len() as u64),
                    a.days,
                    if a.days == 1 { "" } else { "s" },
                    span(a.total_secs),
                    if share < 0.1 {
                        "under 0.1%".to_string()
                    } else {
                        format!("{share:.1}%")
                    },
                    span(a.longest_secs)
                ));
                let skipped = a.stretches.len().saturating_sub(PRINTED_STRETCHES);
                if skipped > 0 {
                    v.push(format!("    ({skipped} earlier: --json lists every one)"));
                }
                for (i, s) in a.stretches.iter().enumerate().skip(skipped) {
                    let last = i + 1 == a.stretches.len();
                    v.push(format!(
                        "    {} for {}{}",
                        utc_stamp(s.from),
                        span(s.secs()),
                        if last && a.to_now { ", and still" } else { "" }
                    ));
                }
            }
            v.push(String::new());
            v.push("per account:".into());
            for a in &self.accounts {
                let past: Vec<String> = a
                    .past
                    .iter()
                    .map(|p| {
                        let what = match p.line {
                            LIMIT => "at its limit",
                            _ => "past the pause line",
                        };
                        match p.times {
                            0 => format!("{what} never"),
                            n => format!("{what} {} for {}", times(n), span(p.secs)),
                        }
                    })
                    .collect();
                v.push(format!(
                    "  {}: {} reading{}; {}{}{}",
                    a.name,
                    a.readings,
                    if a.readings == 1 { "" } else { "s" },
                    past.join("; "),
                    if a.has_extra_usage {
                        "; extra usage is on"
                    } else {
                        ""
                    },
                    if a.login {
                        ""
                    } else {
                        "; no login uses it now"
                    }
                ));
            }
        }
        if !self.unread_accounts.is_empty() {
            v.push(String::new());
            v.push(format!(
                "logins with no usage reading: {}",
                self.unread_accounts.join(", ")
            ));
        }
        v.push(String::new());
        v.push("not counted:".into());
        for n in &self.not_counted {
            v.push(format!("  - {n}"));
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::account_usage::{AccountUsageSnapshot, UsageOutcomeKind, Window};
    use crate::service::add_account::API_KEY_ACCOUNT_PREFIX;
    use crate::service::attention::Facts;
    use crate::service::settings;
    use crate::store::HostProfileRow;

    const MIN: i64 = 60;
    const HOUR: i64 = 3_600;
    const DAY: i64 = 86_400;
    /// A midnight UTC (2024-10-04), so a test can say which day a stretch is on.
    const NOW: i64 = 20_000 * DAY;

    fn opts(days: u32) -> LimitCensusOptions {
        LimitCensusOptions::new(Some(days), NOW).unwrap()
    }

    fn window(pct: f64, resets_at: Option<i64>) -> Option<Window> {
        Some(Window {
            utilization: pct,
            resets_at,
        })
    }

    fn row(
        account: &str,
        at: i64,
        five_hour: Option<Window>,
        seven_day: Option<Window>,
    ) -> UsageSnapshotRow {
        UsageSnapshotRow {
            account_uuid: account.into(),
            fetched_at: at,
            usage: AccountUsage {
                five_hour,
                seven_day,
                ..Default::default()
            },
            subscription: None,
            source_host: None,
        }
    }

    /// `account` read at `at`: its 5-hour window at `five`, resetting at
    /// `resets_at`, in a quiet week.
    fn read(account: &str, at: i64, five: f64, resets_at: i64) -> UsageSnapshotRow {
        row(
            account,
            at,
            window(five, Some(resets_at)),
            window(10.0, Some(at + 3 * DAY)),
        )
    }

    /// `account` read on every half hour of `from..until`; `five(at)` is its
    /// 5-hour window then, as (percent, reset).
    fn every_half_hour(
        account: &str,
        from: i64,
        until: i64,
        five: impl Fn(i64) -> (f64, i64),
    ) -> Vec<UsageSnapshotRow> {
        (from..until)
            .step_by(1_800)
            .map(|at| {
                let (pct, resets_at) = five(at);
                read(account, at, pct, resets_at)
            })
            .collect()
    }

    /// A 5-hour window used up from `hit` until it resets at `reset`, and at
    /// 20% the rest of the time.
    fn used_up(hit: i64, reset: i64) -> impl Fn(i64) -> (f64, i64) {
        move |at| {
            if at >= hit && at < reset {
                (100.0, reset)
            } else {
                (20.0, at + HOUR)
            }
        }
    }

    fn quiet(at: i64) -> (f64, i64) {
        (20.0, at + HOUR)
    }

    fn sorted(mut rows: Vec<UsageSnapshotRow>) -> Vec<UsageSnapshotRow> {
        rows.sort_by(|a, b| (&a.account_uuid, a.fetched_at).cmp(&(&b.account_uuid, b.fetched_at)));
        rows
    }

    /// The census of `rows` over the last week, the pause line at 90%;
    /// `logins` are the accounts a login uses now.
    fn census_with(rows: Vec<UsageSnapshotRow>, logins: &[&str]) -> LimitCensusReport {
        let logins: Vec<String> = logins.iter().map(|l| l.to_string()).collect();
        census_of(&sorted(rows), &[], &logins, 90.0, &opts(7), 225)
    }

    /// [`census_with`], every account in `rows` being one a login uses now.
    fn census(rows: Vec<UsageSnapshotRow>) -> LimitCensusReport {
        let logins: BTreeSet<&str> = rows.iter().map(|r| r.account_uuid.as_str()).collect();
        census_with(rows.clone(), &logins.into_iter().collect::<Vec<_>>())
    }

    fn limit(r: &LimitCensusReport) -> &AllAtOnce {
        let a = &r.all_at_once[0];
        assert_eq!((a.line, a.at_pct), (LIMIT, 100.0));
        a
    }

    fn pause_line(r: &LimitCensusReport) -> &AllAtOnce {
        let a = &r.all_at_once[1];
        assert_eq!(a.line, PAUSE_LINE);
        a
    }

    fn stretch(from: i64, until: i64) -> Stretch {
        Stretch { from, until }
    }

    fn account<'a>(r: &'a LimitCensusReport, uuid: &str) -> &'a AccountCensus {
        r.accounts
            .iter()
            .find(|a| a.account_uuid == uuid)
            .unwrap_or_else(|| panic!("{uuid} is not counted: {:?}", r.accounts))
    }

    #[test]
    fn stretches_are_cut_to_the_window_joined_where_they_touch_and_never_empty() {
        let w = stretch(0, 100);
        let mut v = Vec::new();
        add(&mut v, -5, 10, w);
        assert_eq!(v, vec![stretch(0, 10)], "cut at the window's start");
        add(&mut v, 10, 20, w);
        assert_eq!(v, vec![stretch(0, 20)], "joined where the last one ends");
        add(&mut v, 5, 8, w);
        assert_eq!(v, vec![stretch(0, 20)], "one inside the last adds nothing");
        add(&mut v, 30, 30, w);
        add(&mut v, 40, 35, w);
        assert_eq!(v, vec![stretch(0, 20)], "an empty one is no stretch");
        add(&mut v, 21, 200, w);
        add(&mut v, 100, 120, w);
        assert_eq!(
            v,
            vec![stretch(0, 20), stretch(21, 100)],
            "apart from the last, cut at the window's end, and nothing past it"
        );
    }

    #[test]
    fn one_account_at_its_limit_is_all_at_once_from_that_reading_until_its_window_resets() {
        let start = NOW - 2 * DAY;
        // Used up at a reading; it resets ten minutes after one, twenty before the next.
        let (hit, reset) = (start + 3 * HOUR, start + 5 * HOUR + 10 * MIN);
        let r = census(every_half_hour("a", start, NOW, used_up(hit, reset)));
        let l = limit(&r);
        assert_eq!(l.stretches, vec![stretch(hit, reset)]);
        assert_eq!(
            (l.total_secs, l.longest_secs, l.days, l.to_now),
            (2 * HOUR + 10 * MIN, 2 * HOUR + 10 * MIN, 1, false)
        );
        assert_eq!(
            pause_line(&r).stretches,
            l.stretches,
            "at its limit is past the pause line too"
        );
        let a = account(&r, "a");
        assert_eq!((a.readings, a.counted_secs, a.login), (96, 2 * DAY, true));
        assert_eq!(
            a.past,
            vec![
                AccountPast {
                    line: LIMIT,
                    times: 1,
                    secs: 2 * HOUR + 10 * MIN
                },
                AccountPast {
                    line: PAUSE_LINE,
                    times: 1,
                    secs: 2 * HOUR + 10 * MIN
                },
            ]
        );
        assert_eq!(
            (r.counted_secs, r.read_secs, r.most_at_once),
            (2 * DAY, 2 * DAY, 1)
        );
        assert_eq!((r.since, r.until, r.days), (NOW - 7 * DAY, NOW, 7));
    }

    #[test]
    fn two_accounts_are_all_at_once_only_while_both_are_at_their_limit() {
        let t = NOW - 3 * DAY;
        let a = every_half_hour("a", t, NOW, used_up(t + HOUR, t + 5 * HOUR));
        let b = every_half_hour("b", t, NOW, used_up(t + 2 * HOUR, t + 4 * HOUR));
        let r = census([a.clone(), b].concat());
        assert_eq!(
            limit(&r).stretches,
            vec![stretch(t + 2 * HOUR, t + 4 * HOUR)]
        );
        assert_eq!(account(&r, "a").past[0].secs, 4 * HOUR);
        assert_eq!(account(&r, "b").past[0].secs, 2 * HOUR);
        assert_eq!(r.most_at_once, 2);

        let b = every_half_hour("b", t, NOW, quiet);
        let r = census([a, b].concat());
        assert_eq!(limit(&r).stretches, vec![], "b had room all along");
        assert_eq!(
            (limit(&r).total_secs, limit(&r).days, limit(&r).to_now),
            (0, 0, false)
        );
    }

    #[test]
    fn an_account_freed_the_second_the_other_ran_out_is_not_all_at_once() {
        let t = NOW - 3 * DAY;
        let a = every_half_hour("a", t, NOW, used_up(t + HOUR, t + 3 * HOUR));
        let b = every_half_hour("b", t, NOW, used_up(t + 3 * HOUR, t + 5 * HOUR));
        assert_eq!(limit(&census([a, b].concat())).stretches, vec![]);
    }

    #[test]
    fn two_limits_with_room_between_them_are_two_times() {
        let t = NOW - 3 * DAY;
        let twice = |at: i64| {
            let second = t + DAY + 23 * HOUR;
            if at >= second {
                used_up(second, second + 2 * HOUR)(at)
            } else {
                used_up(t, t + HOUR)(at)
            }
        };
        let r = census(every_half_hour("a", t, NOW, twice));
        let l = limit(&r);
        assert_eq!(
            l.stretches,
            vec![
                stretch(t, t + HOUR),
                stretch(t + DAY + 23 * HOUR, t + 2 * DAY + HOUR)
            ]
        );
        assert_eq!((l.total_secs, l.longest_secs), (3 * HOUR, 2 * HOUR));
        assert_eq!(
            l.days, 3,
            "the second one runs over midnight UTC: both days count"
        );
        assert_eq!(account(&r, "a").past[0].times, 2);
    }

    #[test]
    fn the_pause_line_counts_from_its_own_percent_and_at_100_it_is_the_limit_itself() {
        let t = NOW - DAY;
        let rows = every_half_hour("a", t, NOW, |at| {
            if at < t + 2 * HOUR {
                (92.0, t + 2 * HOUR)
            } else {
                quiet(at)
            }
        });
        let at = |pause_at: f64| census_of(&rows, &[], &["a".to_string()], pause_at, &opts(7), 225);
        let r = at(90.0);
        assert_eq!(limit(&r).stretches, vec![]);
        assert_eq!(pause_line(&r).stretches, vec![stretch(t, t + 2 * HOUR)]);
        assert_eq!(pause_line(&r).at_pct, 90.0);
        assert_eq!(
            pause_line(&at(92.0)).stretches.len(),
            1,
            "at the line is past it"
        );
        assert_eq!(pause_line(&at(93.0)).stretches, vec![]);
        assert_eq!(
            at(100.0).all_at_once.len(),
            1,
            "the same line is not counted twice"
        );
        assert_eq!(at(100.0).accounts[0].past.len(), 1);
    }

    /// The spare: read for a day, then nothing ran on it and its access
    /// token went stale. It is the login with room.
    #[test]
    fn a_login_nothing_runs_on_keeps_its_last_reading_and_its_room() {
        let t = NOW - 5 * DAY;
        let busy = |at: i64| {
            let second = t + 3 * DAY;
            if at >= second {
                used_up(second, second + 2 * HOUR)(at)
            } else {
                used_up(t + DAY + 2 * HOUR, t + DAY + 3 * HOUR)(at)
            }
        };
        let b = every_half_hour("b", t, NOW, busy);
        let spare = every_half_hour("a", t, t + DAY, quiet);
        let r = census([spare, b.clone()].concat());
        assert_eq!(
            limit(&r).stretches,
            vec![],
            "two days after its last reading the spare still has its room"
        );
        let a = account(&r, "a");
        assert_eq!((a.readings, a.counted_secs, a.login), (48, 5 * DAY, true));
        assert_eq!((r.counted_secs, r.read_secs), (5 * DAY, 5 * DAY));

        // Last read with its week used up, three days from its reset: it
        // holds until then and has room after, with no reading to say so.
        let spare = vec![row("a", t, None, window(100.0, Some(t + 3 * DAY + HOUR)))];
        let r = census([spare, b].concat());
        assert_eq!(
            limit(&r).stretches,
            vec![
                stretch(t + DAY + 2 * HOUR, t + DAY + 3 * HOUR),
                stretch(t + 3 * DAY, t + 3 * DAY + HOUR)
            ]
        );
        assert_eq!(
            account(&r, "a").past[0],
            AccountPast {
                line: LIMIT,
                times: 1,
                secs: 3 * DAY + HOUR
            }
        );
    }

    #[test]
    fn an_account_no_login_uses_any_more_is_counted_for_a_day_after_each_reading() {
        let t = NOW - 5 * DAY;
        // a is read for a day and then no more; b all along, used up twice:
        // while a's last reading still stands, and after it no longer does.
        let last_a = t + DAY - 30 * MIN;
        let (standing, gone) = (last_a + 2 * HOUR, last_a + DAY + 2 * HOUR);
        let a = every_half_hour("a", t, t + DAY, quiet);
        let b = every_half_hour("b", t, NOW, |at| {
            if at >= gone {
                used_up(gone, gone + HOUR)(at)
            } else {
                used_up(standing, standing + HOUR)(at)
            }
        });
        let r = census_with([a, b].concat(), &["b"]);
        assert_eq!(
            limit(&r).stretches,
            vec![stretch(gone, gone + HOUR)],
            "while a's last reading stands, b alone is not all"
        );
        let a = account(&r, "a");
        assert_eq!(
            (a.counted_secs, a.login),
            (2 * DAY - 30 * MIN, false),
            "its readings, and a day more for the last"
        );
        assert_eq!((r.most_at_once, r.counted_secs), (2, 5 * DAY));

        // At its limit or not: its week is used up for five more days, and
        // it is counted for one.
        let gone = vec![row("a", t, None, window(100.0, Some(t + 5 * DAY)))];
        let r = census_with(gone.clone(), &[]);
        assert_eq!(limit(&r).stretches, vec![stretch(t, t + DAY)]);
        assert_eq!((r.counted_secs, r.read_secs), (DAY, HOUR));
        assert_eq!(
            limit(&census_with(gone, &["a"])).stretches,
            vec![stretch(t, NOW)],
            "a login's holds until its reset"
        );
    }

    #[test]
    fn accounts_counted_one_after_the_other_are_never_two_at_once() {
        let t = NOW - 5 * DAY;
        // No login uses either: a reading stands for a day, and the other
        // account's begins the second it ends, both ways round.
        let r = census_with(
            vec![
                read("a", t, 20.0, t + HOUR),
                read("b", t + DAY, 20.0, t + DAY + HOUR),
                read("a", t + 2 * DAY, 20.0, t + 2 * DAY + HOUR),
            ],
            &[],
        );
        assert_eq!(r.accounts.len(), 2);
        assert_eq!((r.most_at_once, r.counted_secs), (1, 3 * DAY));
    }

    #[test]
    fn a_reading_from_before_the_window_still_stands_inside_it() {
        let s = Store::open_in_memory().unwrap();
        // Read once more, long before, to show which reading is taken.
        s.insert_usage_snapshot(&read("a", NOW - 3 * DAY, 5.0, NOW - 2 * DAY))
            .unwrap();
        s.insert_usage_snapshot(&row(
            "a",
            NOW - DAY - HOUR,
            None,
            window(100.0, Some(NOW - 20 * HOUR)),
        ))
        .unwrap();
        let r = limit_census(&s, &opts(1)).unwrap();
        assert_eq!(
            limit(&r).stretches,
            vec![stretch(NOW - DAY, NOW - 20 * HOUR)]
        );
        let a = account(&r, "a");
        assert_eq!(
            (a.readings, a.counted_secs, a.login),
            (0, 23 * HOUR, false),
            "no reading was taken in the window; the one before it stands until an hour ago"
        );
        assert_eq!((r.counted_secs, r.read_secs), (23 * HOUR, 0));
        assert_eq!(r.schema_version, s.schema_version().unwrap());
    }

    /// Through the store: who is a login now comes from the hosts.
    #[test]
    fn a_spare_login_on_a_host_stands_with_a_reading_from_long_before_the_window() {
        let s = Store::open_in_memory().unwrap();
        s.insert_host("h1", Some("h1")).unwrap();
        s.upsert_account(&crate::store::AccountRow {
            uuid: "main".into(),
            ..Default::default()
        })
        .unwrap();
        s.set_host_account("h1", Some("main")).unwrap();
        let spare = HostProfileRow {
            name: "spare".into(),
            account_uuid: Some("spare".into()),
            email: None,
        };
        s.set_host_profiles("h1", &[spare]).unwrap();
        // The spare was last read ten days before the window, at 30%.
        s.insert_usage_snapshot(&read("spare", NOW - 17 * DAY, 30.0, NOW - 17 * DAY + HOUR))
            .unwrap();
        let hit = NOW - 2 * DAY;
        for r in every_half_hour("main", NOW - 7 * DAY, NOW, used_up(hit, hit + 2 * HOUR)) {
            s.insert_usage_snapshot(&r).unwrap();
        }
        let r = limit_census(&s, &opts(7)).unwrap();
        assert_eq!(limit(&r).stretches, vec![], "the spare had room");
        let a = account(&r, "spare");
        assert_eq!((a.readings, a.counted_secs, a.login), (0, 7 * DAY, true));
        assert_eq!(r.unread_accounts, Vec::<String>::new());

        // The profile is gone from the host: nobody's login, long unread.
        s.set_host_profiles("h1", &[]).unwrap();
        let r = limit_census(&s, &opts(7)).unwrap();
        assert_eq!(limit(&r).stretches, vec![stretch(hit, hit + 2 * HOUR)]);
        assert_eq!(r.accounts.len(), 1, "{:?}", r.accounts);
    }

    #[test]
    fn a_limit_that_lasts_to_now_says_so_and_is_cut_there() {
        let r = census(vec![read("a", NOW - 30 * MIN, 100.0, NOW + 2 * HOUR)]);
        assert_eq!(limit(&r).stretches, vec![stretch(NOW - 30 * MIN, NOW)]);
        assert!(limit(&r).to_now);
        assert_eq!(
            limit(&r).days,
            1,
            "it ends at midnight UTC: the day after is not one of its days"
        );
        assert_eq!((r.counted_secs, r.read_secs), (30 * MIN, 30 * MIN));
        // A reading stamped at now, or after it (another clock), is not in
        // the window.
        for at in [NOW, NOW + MIN] {
            let r = census(vec![read("a", at, 100.0, NOW + 2 * HOUR)]);
            assert_eq!(
                (r.accounts.len(), r.counted_secs, r.read_secs),
                (0, 0, 0),
                "{at}"
            );
        }
        // Nor is it one of the readings an account took in it.
        let r = census(vec![
            read("a", NOW - HOUR, 20.0, NOW),
            read("a", NOW + MIN, 100.0, NOW + 2 * HOUR),
        ]);
        assert_eq!(account(&r, "a").readings, 1);
        assert_eq!(limit(&r).stretches, vec![]);
    }

    #[test]
    fn the_hub_was_reading_for_an_hour_after_each_reading() {
        let t = NOW - 2 * DAY;
        // Read for six hours, nothing for ten, then to now.
        let mut rows = every_half_hour("a", t, t + 6 * HOUR, quiet);
        rows.extend(every_half_hour("a", t + 16 * HOUR, NOW, quiet));
        let r = census(rows);
        assert_eq!(r.counted_secs, 2 * DAY, "the login is counted all along");
        assert_eq!(
            r.read_secs,
            2 * DAY - 10 * HOUR + 30 * MIN,
            "the last reading before the silence is good for an hour"
        );
        assert_eq!(READ_FOR_SECS, HOUR);
    }

    #[test]
    fn a_window_that_names_no_reset_holds_for_its_own_length() {
        let t = NOW - 3 * DAY;
        let r = census(vec![
            row("a", t, window(100.0, None), None),
            read("a", t + 6 * HOUR, 5.0, t + 9 * HOUR),
        ]);
        assert_eq!(limit(&r).stretches, vec![stretch(t, t + 5 * HOUR)]);
        // A week from ten days ago; from three days ago it is not over yet.
        let week = |at: i64| census(vec![row("a", at, None, window(100.0, None))]);
        assert_eq!(
            limit(&week(NOW - 10 * DAY)).stretches,
            vec![stretch(NOW - 7 * DAY, NOW - 3 * DAY)]
        );
        assert_eq!(limit(&week(t)).stretches, vec![stretch(t, NOW)]);
    }

    #[test]
    fn a_newer_reading_replaces_what_the_older_one_said() {
        let t = NOW - 2 * DAY;
        // Used up, three hours from its reset; half an hour on it reads 40%.
        let r = census(vec![
            read("a", t, 100.0, t + 3 * HOUR),
            read("a", t + 30 * MIN, 40.0, t + 3 * HOUR),
        ]);
        assert_eq!(limit(&r).stretches, vec![stretch(t, t + 30 * MIN)]);
        // And the other way: a later reset than the first reading named.
        let r = census(vec![
            read("a", t, 100.0, t + HOUR),
            read("a", t + 30 * MIN, 100.0, t + 2 * HOUR),
            read("a", t + 3 * HOUR, 5.0, t + 8 * HOUR),
        ]);
        assert_eq!(limit(&r).stretches, vec![stretch(t, t + 2 * HOUR)]);
        assert_eq!(account(&r, "a").past[0].times, 1);
    }

    #[test]
    fn an_account_is_held_until_the_last_of_its_used_up_windows_resets() {
        let t = NOW - 2 * DAY;
        let both = |five_resets: i64, week_resets: i64| {
            census(vec![
                row(
                    "a",
                    t,
                    window(100.0, Some(five_resets)),
                    window(100.0, Some(week_resets)),
                ),
                read("a", t + 4 * HOUR, 5.0, t + 9 * HOUR),
            ])
        };
        assert_eq!(
            limit(&both(t + HOUR, t + 3 * HOUR)).stretches,
            vec![stretch(t, t + 3 * HOUR)]
        );
        assert_eq!(
            limit(&both(t + 2 * HOUR, t + HOUR)).stretches,
            vec![stretch(t, t + 2 * HOUR)]
        );
        // A window under the line does not hold it, however late it resets.
        let r = census(vec![
            row(
                "a",
                t,
                window(100.0, Some(t + HOUR)),
                window(60.0, Some(t + 3 * HOUR)),
            ),
            read("a", t + 4 * HOUR, 5.0, t + 9 * HOUR),
        ]);
        assert_eq!(limit(&r).stretches, vec![stretch(t, t + HOUR)]);
    }

    #[test]
    fn a_reading_that_is_not_a_number_or_whose_window_already_reset_is_not_at_the_limit() {
        let t = NOW - 2 * DAY;
        for five in [
            window(f64::NAN, Some(t + HOUR)),
            window(f64::INFINITY, Some(t + HOUR)),
            window(100.0, Some(t)),
            window(100.0, Some(t - HOUR)),
        ] {
            let r = census(vec![row("a", t, five.clone(), None)]);
            assert_eq!(limit(&r).stretches, vec![], "{five:?}");
            assert_eq!(
                account(&r, "a").past[0],
                AccountPast {
                    line: LIMIT,
                    times: 0,
                    secs: 0
                },
                "{five:?}"
            );
            assert_eq!(r.counted_secs, 2 * DAY, "it is counted all the same");
        }
    }

    /// The census against the hub's own two rules, asked minute by minute:
    /// `Facts::from_fleet` for a row's "Paused · limit", `used_pct_of` for
    /// the pause line.
    #[test]
    fn the_count_is_what_the_hub_itself_would_have_said_minute_by_minute() {
        // xorshift: the same readings every run.
        let mut seed = 0x2545_F491_4F6C_DD1D_u64;
        let mut below = move |n: i64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n as u64) as i64
        };
        const PCT: [f64; 7] = [50.0, 89.0, 90.0, 95.0, 100.0, 100.0, 120.0];
        let from = NOW - 3 * DAY;
        let stopped = stretch(from + 30 * HOUR, from + 58 * HOUR);
        // a and b are logins now; no login uses c any more. a was read
        // before the window began, b first some hours into it.
        let names = ["a", "b", "c"];
        let logins = ["a".to_string(), "b".to_string()];
        let mut rows = Vec::new();
        for name in names {
            let mut at = match name {
                "b" => from + (120 + below(4 * 60)) * MIN,
                _ => from - below(26 * 60) * MIN,
            };
            while at < NOW + HOUR {
                let mut win = |len: i64| match below(6) {
                    0 => None,
                    kind => window(
                        PCT[below(7) as usize],
                        match kind {
                            1 => None,
                            2 => Some(at - below(3) * MIN),
                            _ => Some(at + (1 + below(len / MIN)) * MIN),
                        },
                    ),
                };
                let five = win(FIVE_HOUR_SECS);
                rows.push(row(name, at, five, win(2 * DAY)));
                // Every 5 to 30 minutes; now and then nothing for up to 30 hours.
                at += match below(40) {
                    0 => (1 + below(30 * 60)) * MIN,
                    _ => (5 + below(26)) * MIN,
                };
                // The hub was stopped for 28 hours: nobody was read.
                if at >= stopped.from && at < stopped.until {
                    at = stopped.until + below(30) * MIN;
                }
            }
        }
        let r = census_of(&rows, &[], &logins, 90.0, &opts(3), 225);

        let of = |name: &str| -> Vec<&UsageSnapshotRow> {
            rows.iter().filter(|r| r.account_uuid == name).collect()
        };
        let per_account = names.map(of);
        // (counted, at the limit, past the pause line, lapsed, read) at `t`,
        // by the hub's rules.
        let hub_says = |t: i64| {
            let (mut counted, mut limited, mut paused) = (0, 0, 0);
            let (mut lapsed, mut read) = (false, false);
            for (name, rows) in names.iter().zip(&per_account) {
                let newest = rows.partition_point(|r| r.fetched_at <= t);
                let Some(reading) = newest.checked_sub(1).map(|i| rows[i]) else {
                    continue;
                };
                read |= t - reading.fetched_at < READ_FOR_SECS;
                if !logins.iter().any(|l| l == name) && t - reading.fetched_at >= STANDS_FOR_SECS {
                    lapsed = true;
                    continue;
                }
                counted += 1;
                let snap = AccountUsageSnapshot {
                    usage: Some(reading.usage.clone()),
                    fetched_at: Some(reading.fetched_at),
                    status: UsageOutcomeKind::Ok,
                    ..AccountUsageSnapshot::never_fetched(name)
                };
                if Facts::from_fleet(&[], &[snap], t)
                    .limited_accounts
                    .contains_key(*name)
                {
                    limited += 1;
                }
                if account_limits::used_pct_of(&reading.usage, Some(reading.fetched_at), t)
                    .is_some_and(|pct| pct >= 90.0)
                {
                    paused += 1;
                }
            }
            (counted, limited, paused, lapsed, read)
        };
        let (mut counted_secs, mut read_secs, mut all_limited, mut all_paused) = (0, 0, 0, 0);
        let (mut most, mut fewest, mut lapsed_secs) = (0, usize::MAX, 0);
        for t in (from..NOW).step_by(60) {
            let (counted, limited, paused, lapsed, read) = hub_says(t);
            most = most.max(counted);
            fewest = fewest.min(counted);
            read_secs += if read { MIN } else { 0 };
            lapsed_secs += if lapsed { MIN } else { 0 };
            if counted > 0 {
                counted_secs += MIN;
                all_limited += if limited == counted { MIN } else { 0 };
                all_paused += if paused == counted { MIN } else { 0 };
            }
        }
        assert_eq!(limit(&r).total_secs, all_limited);
        assert_eq!(pause_line(&r).total_secs, all_paused);
        assert_eq!(
            (r.counted_secs, r.read_secs, r.most_at_once),
            (counted_secs, read_secs, most)
        );
        // The readings make every case: all at the limit, only past the
        // pause line, neither; two and three accounts counted; the account
        // without a login lapsed; the hub not reading.
        assert!(
            0 < all_limited && all_limited < all_paused && all_paused < counted_secs,
            "{all_limited} {all_paused} {counted_secs}"
        );
        assert_eq!((fewest, most, counted_secs), (2, 3, 3 * DAY));
        assert!(
            0 < lapsed_secs && read_secs < 3 * DAY - 26 * HOUR,
            "{lapsed_secs} {read_secs}"
        );
        assert!(limit(&r).stretches.len() > 3, "{:?}", limit(&r).stretches);
        for a in &r.all_at_once {
            assert!(
                a.stretches.windows(2).all(|w| w[0].until < w[1].from),
                "apart and in order: {:?}",
                a.stretches
            );
            assert_eq!(
                a.longest_secs,
                a.stretches.iter().map(Stretch::secs).max().unwrap()
            );
        }
    }

    #[test]
    fn accounts_go_by_name_and_the_logins_with_no_reading_are_listed() {
        let known = |uuid: &str, name: &str, has_extra_usage: bool| UsageAccount {
            uuid: uuid.into(),
            name: name.into(),
            has_extra_usage,
        };
        let key = format!("{API_KEY_ACCOUNT_PREFIX}0123");
        let known = [
            known("a", "work", true),
            known("b", "home", false),
            known("c", "old@x", false),
            known(&key, "ci", false),
        ];
        // "new" is signed in on a host and was never read; it has no
        // account row either.
        let logins = ["a", "b", "new", key.as_str()].map(String::from);
        let t = NOW - DAY;
        let mut rows = every_half_hour("a", t, NOW, quiet);
        rows.extend(every_half_hour("b", t, NOW, quiet));
        // Read, and since gone from the hosts and from the accounts.
        rows.extend(every_half_hour("z", t, NOW, quiet));
        // Last read nine days ago, and no login uses it: not in the window.
        rows.push(read("c", NOW - 9 * DAY, 100.0, NOW));
        let r = census_of(&sorted(rows), &known, &logins, 90.0, &opts(7), 225);
        let got: Vec<(&str, &str, bool, bool)> = r
            .accounts
            .iter()
            .map(|a| {
                (
                    a.name.as_str(),
                    a.account_uuid.as_str(),
                    a.login,
                    a.has_extra_usage,
                )
            })
            .collect();
        assert_eq!(
            got,
            vec![
                ("home", "b", true, false),
                ("work", "a", true, true),
                ("z", "z", false, false)
            ]
        );
        assert_eq!(r.unread_accounts, vec!["ci (API key)", "new"]);
        assert_eq!(r.not_counted, NOT_COUNTED);
    }

    #[test]
    fn the_pause_line_is_the_fleets_own_setting() {
        let s = Store::open_in_memory().unwrap();
        s.insert_usage_snapshot(&read("a", NOW - HOUR, 85.0, NOW + HOUR))
            .unwrap();
        let r = limit_census(&s, &opts(7)).unwrap();
        assert_eq!(pause_line(&r).at_pct, 90.0);
        assert_eq!(pause_line(&r).stretches, vec![]);
        s.set_setting(settings::ACCOUNTS_PAUSE_AT, "80").unwrap();
        let r = limit_census(&s, &opts(7)).unwrap();
        assert_eq!(pause_line(&r).at_pct, 80.0);
        assert_eq!(pause_line(&r).stretches, vec![stretch(NOW - HOUR, NOW)]);
        assert!(pause_line(&r).to_now);
    }

    #[test]
    fn days_run_from_one_to_what_the_history_keeps() {
        assert_eq!(i64::from(MAX_DAYS) * DAY, USAGE_HISTORY_KEEP_SECS);
        assert!(LimitCensusOptions::new(Some(0), NOW).is_err());
        assert!(LimitCensusOptions::new(Some(MAX_DAYS), NOW).is_ok());
        let e = LimitCensusOptions::new(Some(MAX_DAYS + 1), NOW).unwrap_err();
        assert_eq!(e.code, codes::E_INVALID);
        assert!(e.message.contains("1-35"), "{}", e.message);
        let o = LimitCensusOptions::new(None, NOW).unwrap();
        assert_eq!(o.window(), stretch(NOW - 28 * DAY, NOW));
    }

    #[test]
    fn a_database_from_before_the_history_is_refused_with_its_version_named() {
        let s = Store::open_in_memory().unwrap();
        s.conn_for_test()
            .execute("DELETE FROM schema_version WHERE version >= 122", [])
            .unwrap();
        let e = limit_census(&s, &opts(7)).unwrap_err();
        assert!(e.message.contains("schema 121"), "{}", e.message);
        s.conn_for_test()
            .execute("INSERT INTO schema_version (version) VALUES (122)", [])
            .unwrap();
        assert!(limit_census(&s, &opts(7)).is_ok());
    }

    /// `--db` on a backup of a hub that runs an older release: a database at
    /// the first schema with the history, opened read-only as the command
    /// opens it.
    #[test]
    fn a_database_at_the_first_schema_with_the_history_is_counted_read_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.db");
        let hit = NOW - 2 * DAY;
        {
            let conn = crate::store::testgen::db_file_at(&path, LIMIT_CENSUS_MIN_SCHEMA);
            conn.execute_batch(
                "INSERT INTO accounts (uuid, email, nickname, has_extra_usage)
                   VALUES ('main', 'm@x', 'work', 1);
                 INSERT INTO hosts (alias, account_uuid, claude_profiles)
                   VALUES ('h1', 'main', '[{\"name\":\"spare\",\"account_uuid\":\"spare\"}]');",
            )
            .unwrap();
            let read = |account: &str, at: i64, pct: f64, resets_at: i64| {
                conn.execute(
                    "INSERT INTO account_usage_snapshots
                       (account_uuid, fetched_at, five_hour_pct, five_hour_resets_at)
                     VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params![account, at, pct, resets_at],
                )
                .unwrap();
            };
            read("main", hit, 100.0, hit + 2 * HOUR);
            read("main", hit + 3 * HOUR, 20.0, hit + 8 * HOUR);
            // The spare profile was last read ten days ago.
            read("spare", NOW - 10 * DAY, 30.0, NOW - 10 * DAY + HOUR);
        }
        let s = Store::open_read_only(&path).unwrap();
        let r = limit_census(&s, &opts(7)).unwrap();
        assert_eq!(r.schema_version, LIMIT_CENSUS_MIN_SCHEMA);
        assert_eq!(limit(&r).stretches, vec![], "the spare had room");
        assert_eq!(pause_line(&r).at_pct, 90.0, "the setting's default");
        let got: Vec<(&str, bool, bool, i64)> = r
            .accounts
            .iter()
            .map(|a| (a.name.as_str(), a.login, a.has_extra_usage, a.past[0].secs))
            .collect();
        assert_eq!(
            got,
            vec![("spare", true, false, 0), ("work", true, true, 2 * HOUR)]
        );
    }

    /// Two accounts over the last week: both used up twice, and once more
    /// one used up while the other was past the pause line.
    fn a_week() -> LimitCensusReport {
        let t = NOW - 7 * DAY;
        let day = |n: i64, hour: i64, min: i64| t + n * DAY + hour * HOUR + min * MIN;
        let work = |at: i64| {
            if at >= day(5, 13, 0) {
                used_up(day(5, 13, 0), day(5, 16, 0))(at)
            } else if at >= day(4, 20, 0) && at < day(4, 22, 0) {
                (93.0, day(4, 22, 0))
            } else {
                used_up(day(2, 9, 0), day(2, 11, 10))(at)
            }
        };
        let home = |at: i64| {
            if at >= day(5, 14, 0) {
                used_up(day(5, 14, 0), day(5, 15, 30))(at)
            } else if at >= day(4, 0, 0) {
                used_up(day(4, 19, 30), day(4, 23, 0))(at)
            } else {
                used_up(day(2, 8, 0), day(2, 12, 0))(at)
            }
        };
        let mut rows = every_half_hour("a", t, NOW, work);
        rows.extend(every_half_hour("b", t + DAY, NOW, home));
        let key = format!("{API_KEY_ACCOUNT_PREFIX}1");
        let known = [
            UsageAccount {
                uuid: "a".into(),
                name: "work".into(),
                has_extra_usage: true,
            },
            UsageAccount {
                uuid: "b".into(),
                name: "home".into(),
                has_extra_usage: false,
            },
            UsageAccount {
                uuid: key.clone(),
                name: "ci".into(),
                has_extra_usage: false,
            },
        ];
        let logins = ["a".to_string(), "b".to_string(), key];
        census_of(&rows, &known, &logins, 90.0, &opts(7), 225)
    }

    #[test]
    fn the_census_in_words() {
        let got = a_week().lines();
        let want = "\
limit census: how often every Claude account was at its limit at the same time
window: 7 days to Oct 4 00:00 UTC; schema 225
accounts counted: 2, for 7 d of the window; the hub read usage during 7 d of it

all at their limit (100% of the 5-hour or weekly window) at once:
  2 times on 2 days; 3 h 40 min in total (2.2% of the time counted); longest 2 h 10 min
    Sep 29 09:00 UTC for 2 h 10 min
    Oct 2 14:00 UTC for 1 h 30 min

all past the pause line (90%, accounts.pause_at) at once:
  3 times on 3 days; 5 h 40 min in total (3.4% of the time counted); longest 2 h 10 min
    Sep 29 09:00 UTC for 2 h 10 min
    Oct 1 20:00 UTC for 2 h
    Oct 2 14:00 UTC for 1 h 30 min

per account:
  home: 288 readings; at its limit 3 times for 9 h; past the pause line 3 times for 9 h
  work: 336 readings; at its limit 2 times for 5 h 10 min; past the pause line 3 times for 7 h 10 min; extra usage is on

logins with no usage reading: ci (API key)

not counted:";
        let (head, tail) = got.split_at(got.len() - NOT_COUNTED.len());
        assert_eq!(head.join("\n"), want);
        let listed: Vec<String> = NOT_COUNTED.iter().map(|n| format!("  - {n}")).collect();
        assert_eq!(tail, listed);
    }

    #[test]
    fn the_words_for_nothing_read_one_account_never_and_more_times_than_fit() {
        let lines = census(vec![]).lines();
        assert_eq!(lines[1], "window: 7 days to Oct 4 00:00 UTC; schema 225");
        assert_eq!(
            lines[3],
            "no usage reading in this window: nothing to count"
        );
        assert_eq!(lines[5], "not counted:", "no line is counted: {lines:?}");

        // One account, at its limit since its one reading half an hour ago.
        let lines = census(vec![read("a", NOW - 30 * MIN, 100.0, NOW + HOUR)]).lines();
        assert_eq!(
            lines[2..7],
            [
                "accounts counted: 1 (so \"all at once\" is that account alone), for 30 min of the window; the hub read usage during 30 min of it",
                "",
                "all at their limit (100% of the 5-hour or weekly window) at once:",
                "  once on 1 day; 30 min in total (100.0% of the time counted); longest 30 min",
                "    Oct 3 23:30 UTC for 30 min, and still",
            ]
        );
        assert!(
            lines.contains(
                &"  a: 1 reading; at its limit once for 30 min; past the pause line once for 30 min"
                    .to_string()
            ),
            "{lines:?}"
        );

        // Twice, the second time to now: only that one is still so.
        let lines = census(vec![
            read("a", NOW - DAY, 100.0, NOW - DAY + HOUR),
            read("a", NOW - 30 * MIN, 100.0, NOW + HOUR),
        ])
        .lines();
        assert_eq!(
            lines[6..8],
            [
                "    Oct 3 00:00 UTC for 1 h",
                "    Oct 3 23:30 UTC for 30 min, and still"
            ]
        );

        // One reading two days ago, used up for six hours: a quarter of a
        // day in two days counted, whatever the hour the hub was reading.
        let at = NOW - 2 * DAY;
        let lines = census(vec![read("a", at, 100.0, at + 6 * HOUR)]).lines();
        assert_eq!(
            lines[2],
            "accounts counted: 1 (so \"all at once\" is that account alone), for 2 d of the window; the hub read usage during 1 h of it"
        );
        assert_eq!(
            lines[5],
            "  once on 1 day; 6 h in total (12.5% of the time counted); longest 6 h"
        );

        // Room all along.
        let lines = census(every_half_hour("a", NOW - DAY, NOW, quiet)).lines();
        assert_eq!(lines[5], "  never");
        assert_eq!(lines[8], "  never");
        assert_eq!(
            lines[11],
            "  a: 48 readings; at its limit never; past the pause line never"
        );

        // Fourteen times, one reading each: the ten newest are listed.
        let t = NOW - 7 * DAY;
        let lines = census(every_half_hour("a", t, NOW, |at| {
            if (at - t) % (12 * HOUR) == 0 {
                (100.0, at + 30 * MIN)
            } else {
                quiet(at)
            }
        }))
        .lines();
        assert_eq!(
            lines[5..8],
            [
                "  14 times on 7 days; 7 h in total (4.2% of the time counted); longest 30 min",
                "    (4 earlier: --json lists every one)",
                "    Sep 29 00:00 UTC for 30 min",
            ]
        );
        assert_eq!(lines[16], "    Oct 3 12:00 UTC for 30 min");
        assert_eq!(lines[17], "");

        // Two accounts no login uses, never counted on the same day; a
        // minute at the limit in a week.
        let mut rows = every_half_hour("a", t, t + DAY, quiet);
        rows.push(read("b", NOW - 2 * DAY, 100.0, NOW - 2 * DAY + MIN));
        rows.push(read("b", NOW - 2 * DAY + 30 * MIN, 20.0, NOW));
        let lines = census_with(rows, &[]).lines();
        assert_eq!(
            lines[2],
            "accounts counted: 2 (at most 1 at one time), for 3 d of the window; the hub read usage during 1 d 2 h of it"
        );
        assert!(
            lines[5].contains("1 min in total (under 0.1% of the time counted)"),
            "{}",
            lines[5]
        );
        assert_eq!(
            lines[13],
            "  a: 48 readings; at its limit never; past the pause line never; no login uses it now"
        );
    }

    /// `--json` is read by scripts: every key by its name.
    #[test]
    fn the_json_names_every_number() {
        let v = serde_json::to_value(a_week()).unwrap();
        let keys = |v: &serde_json::Value| -> Vec<String> {
            v.as_object().unwrap().keys().cloned().collect()
        };
        assert_eq!(
            keys(&v),
            [
                "accounts",
                "all_at_once",
                "counted_secs",
                "days",
                "most_at_once",
                "not_counted",
                "read_secs",
                "schema_version",
                "since",
                "unread_accounts",
                "until"
            ]
        );
        assert_eq!(
            keys(&v["all_at_once"][0]),
            [
                "at_pct",
                "days",
                "line",
                "longest_secs",
                "stretches",
                "to_now",
                "total_secs"
            ]
        );
        assert_eq!(v["all_at_once"][0]["line"], "limit");
        assert_eq!(v["all_at_once"][1]["line"], "pause_line");
        assert_eq!(
            v["all_at_once"][0]["stretches"][1],
            serde_json::json!({ "from": NOW - 2 * DAY + 14 * HOUR, "until": NOW - 2 * DAY + 15 * HOUR + 30 * MIN })
        );
        assert_eq!(
            v["accounts"][1],
            serde_json::json!({
                "account_uuid": "a",
                "name": "work",
                "login": true,
                "has_extra_usage": true,
                "readings": 336,
                "counted_secs": 7 * DAY,
                "past": [
                    { "line": "limit", "times": 2, "secs": 5 * HOUR + 10 * MIN },
                    { "line": "pause_line", "times": 3, "secs": 7 * HOUR + 10 * MIN },
                ],
            })
        );
        assert_eq!(v["unread_accounts"], serde_json::json!(["ci (API key)"]));
    }

    #[test]
    fn a_span_reads_in_its_two_largest_units() {
        let cases = [
            (0, "0 s"),
            (59, "59 s"),
            (60, "1 min"),
            (3_599, "59 min"),
            (HOUR, "1 h"),
            (HOUR + 5 * MIN + 59, "1 h 5 min"),
            (DAY - 1, "23 h 59 min"),
            (DAY, "1 d"),
            (DAY + 59 * MIN, "1 d"),
            (2 * DAY + 3 * HOUR + 59 * MIN, "2 d 3 h"),
        ];
        for (secs, words) in cases {
            assert_eq!(span(secs), words);
        }
    }
}
