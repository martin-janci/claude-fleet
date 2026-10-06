//! Org administration phase C: each org's estimated spend, and its budget
//! (`docs/superpowers/specs/2026-10-06-org-administration-design.md`).
//!
//! Spend is `usage_daily_org` (migration 104): the live rows only — a
//! backfill row is history a first read of a transcript booked, not spend in
//! the window. A budget is `budget.org_daily_usd` / `budget.org_monthly_usd`,
//! each org's own value else the fleet's (`settings::get_string_for`); `0`
//! is none. Fleet warns when an org reaches one; it never stops a session.
//!
//! **Who may see it.** An org's spend is a sum over its sessions, other
//! people's private ones among them, so it follows the rule `fleet_health`'s
//! `usage_by_day` already keeps for a person's device: all or nothing, and
//! only a caller that sees every session row there is gets it
//! ([`sees_all_spend`]). A one-person fleet's owner always does.

use crate::service::settings;
use crate::store::Store;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const SECS_PER_DAY: i64 = 86_400;

/// One org's live spend, in micro-USD.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrgSpend {
    /// Today (UTC).
    pub today_micros: i64,
    /// The last 7 UTC days, today included.
    pub week_micros: i64,
    /// This calendar month (UTC).
    pub month_micros: i64,
}

/// Which budget an org has reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Period {
    Daily,
    Monthly,
}

/// An org at or over one of its budgets, as `fleet_health.org_budgets`
/// lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrgBudgetAlert {
    pub org_id: i64,
    pub org: String,
    pub period: Period,
    pub spent_micros: i64,
    pub budget_micros: i64,
}

/// The UTC day number the calendar month of `day` starts on.
pub fn month_start_day(day: i64) -> i64 {
    let dd: i64 = crate::service::usage::day_string(day)[8..]
        .parse()
        .unwrap_or(1);
    day - (dd - 1)
}

/// Every org's live spend at `now`, for the orgs that spent anything in
/// the window.
pub fn spend_by_org(s: &Store, now: i64) -> BTreeMap<i64, OrgSpend> {
    let today = now.div_euclid(SECS_PER_DAY);
    let read = |since: i64| s.org_live_cost_since(since).unwrap_or_default();
    let (day, week, month) = (read(today), read(today - 6), read(month_start_day(today)));
    let mut out: BTreeMap<i64, OrgSpend> = BTreeMap::new();
    for org in day.keys().chain(week.keys()).chain(month.keys()) {
        out.entry(*org).or_insert(OrgSpend {
            today_micros: day.get(org).copied().unwrap_or(0),
            week_micros: week.get(org).copied().unwrap_or(0),
            month_micros: month.get(org).copied().unwrap_or(0),
        });
    }
    out
}

/// Org `org`'s budgets in whole USD, `(daily, monthly)`; `0` is none.
pub fn budgets(s: &Store, org: i64) -> (u64, u64) {
    let read = |key: &str| {
        settings::get_string_for(s, key, Some(org))
            .parse::<u64>()
            .unwrap_or(0)
    };
    (
        read(settings::BUDGET_ORG_DAILY_USD),
        read(settings::BUDGET_ORG_MONTHLY_USD),
    )
}

/// The budgets `spend` has reached, with what was spent against each.
pub fn reached(spend: OrgSpend, (daily, monthly): (u64, u64)) -> Vec<(Period, i64, i64)> {
    let micros = |usd: u64| (usd as i64).saturating_mul(1_000_000);
    let mut out = Vec::new();
    if daily > 0 && spend.today_micros >= micros(daily) {
        out.push((Period::Daily, spend.today_micros, micros(daily)));
    }
    if monthly > 0 && spend.month_micros >= micros(monthly) {
        out.push((Period::Monthly, spend.month_micros, micros(monthly)));
    }
    out
}

/// Every org at or over a budget at `now`, by org then period.
pub fn alerts(s: &Store, now: i64) -> Vec<OrgBudgetAlert> {
    let spend = spend_by_org(s, now);
    let mut out = Vec::new();
    for o in s.list_orgs().unwrap_or_default() {
        let got = spend.get(&o.id).copied().unwrap_or_default();
        for (period, spent_micros, budget_micros) in reached(got, budgets(s, o.id)) {
            out.push(OrgBudgetAlert {
                org_id: o.id,
                org: o.name.clone(),
                period,
                spent_micros,
                budget_micros,
            });
        }
    }
    out
}

/// Whether `view` sees every session row there is: the condition for an
/// org's spend (see the module doc). An unreadable store answers `false`.
pub fn sees_all_spend(s: &Store, view: &crate::service::view_scope::ViewScope) -> bool {
    s.list_all_sessions()
        .map(|rows| rows.iter().all(|r| view.sees_session_row(r).is_visible()))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{UsageDelta, UsageTotals};

    #[test]
    fn a_month_starts_on_its_first_day() {
        let d = crate::service::usage::day_number("2026-10-06").unwrap();
        assert_eq!(
            crate::service::usage::day_string(month_start_day(d)),
            "2026-10-01"
        );
        let first = crate::service::usage::day_number("2026-03-01").unwrap();
        assert_eq!(month_start_day(first), first);
    }

    #[test]
    fn spend_is_per_day_week_and_month_and_budgets_are_reached_at_or_over() {
        let s = Store::open_in_memory().unwrap();
        s.insert_host("a", Some("a")).unwrap();
        let org = s.add_org("Acme", None, false).unwrap().id;
        s.set_host_org("a", Some(org)).unwrap();
        let id = s
            .upsert_session("x", "a", None, None, 1, 1, "running", None)
            .unwrap();
        let today = crate::service::usage::day_number("2026-10-06").unwrap();
        let book = |day: i64, micros: i64| {
            s.apply_usage(
                id,
                "a",
                &UsageDelta {
                    reset: false,
                    totals: UsageTotals {
                        cost_micros: micros,
                        input_tokens: 1,
                        ..Default::default()
                    },
                    model: None,
                    offset: day + micros,
                    source: "s.jsonl".into(),
                    last_msg_id: None,
                    last_msg_usage: None,
                    now: day * SECS_PER_DAY + 10,
                    by_day: Vec::new(),
                    backfill_until: None,
                },
            )
            .unwrap();
        };
        book(today - 40, 9_000_000); // last month: in nothing
        book(today - 5, 2_000_000); // this month and this week
        book(today, 3_000_000);
        let got = spend_by_org(&s, today * SECS_PER_DAY + 100)[&org];
        assert_eq!(
            got,
            OrgSpend {
                today_micros: 3_000_000,
                week_micros: 5_000_000,
                month_micros: 5_000_000,
            }
        );
        assert_eq!(budgets(&s, org), (0, 0));
        assert!(
            alerts(&s, today * SECS_PER_DAY).is_empty(),
            "no budget, no alert"
        );
        settings::set(&s, settings::BUDGET_ORG_DAILY_USD, "3").unwrap();
        settings::set_for_org(
            &s,
            org,
            settings::BUDGET_ORG_MONTHLY_USD,
            Some("10"),
            settings::Actor::Person,
        )
        .unwrap();
        assert_eq!(budgets(&s, org), (3, 10));
        let a = alerts(&s, today * SECS_PER_DAY + 100);
        assert_eq!(a.len(), 1, "{a:?}");
        assert_eq!(
            (a[0].period, a[0].spent_micros, a[0].budget_micros),
            (Period::Daily, 3_000_000, 3_000_000)
        );
        assert_eq!(
            reached(got, (0, 5)),
            vec![(Period::Monthly, 5_000_000, 5_000_000)]
        );
    }
}
