//! An org's "Needs an admin" list (redesign 11.1): derived on every read,
//! never stored, from what the org overview already reads — its budgets, the
//! devices fenced to it and its hosts' unclaimed sessions. Each part follows
//! the gate of what it is made from: budgets only with the spend
//! (`org_spend::sees_all_spend`), devices only for whoever sees them
//! (`AdminView::administers`), unclaimed sessions only for whoever is told
//! the count (`ViewScope::sees_unclaimed_count`).

use crate::service::org_spend::{self as os, OrgSpend, Period};
use serde::{Deserialize, Serialize};

/// The share of a budget, in percent, from which the list warns.
pub const NEAR_BUDGET_PCT: i64 = 80;

/// One thing an org's admin should look at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AdminNeed {
    /// A budget spent to [`NEAR_BUDGET_PCT`] or past it. `pace_day` is the
    /// day of the month a monthly one is reached at this month's pace.
    Budget {
        period: Period,
        spent_micros: i64,
        budget_micros: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pace_day: Option<i64>,
    },
    /// A device fenced to the org that may prompt (`full`) but is not
    /// trusted, so what it types reaches agents marked as untrusted.
    UntrustedDevice { device: String, paired_at: i64 },
    /// Sessions on one of its hosts that nobody has claimed.
    UnclaimedSessions { host: String, count: usize },
}

/// The budgets `spend` has spent [`NEAR_BUDGET_PCT`] of, daily first.
/// `budgets` are whole USD (`0` none); `today` is a UTC day number.
pub fn budget_needs(spend: OrgSpend, (daily, monthly): (u64, u64), today: i64) -> Vec<AdminNeed> {
    let micros = |usd: u64| (usd as i64).saturating_mul(1_000_000);
    let near = |spent: i64, budget: i64| {
        budget > 0 && spent.saturating_mul(100) >= budget.saturating_mul(NEAR_BUDGET_PCT)
    };
    let mut out = Vec::new();
    for (period, spent, budget) in [
        (Period::Daily, spend.today_micros, micros(daily)),
        (Period::Monthly, spend.month_micros, micros(monthly)),
    ] {
        if near(spent, budget) {
            out.push(AdminNeed::Budget {
                period,
                spent_micros: spent,
                budget_micros: budget,
                pace_day: (period == Period::Monthly)
                    .then(|| os::pace_day(today, spent, budget))
                    .flatten(),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(s: &str) -> i64 {
        crate::service::usage::day_number(s).unwrap()
    }

    #[test]
    fn a_budget_is_listed_from_eighty_percent_with_the_day_its_pace_reaches_it() {
        let today = day("2026-10-10");
        let spend = |today_micros, month_micros| OrgSpend {
            today_micros,
            week_micros: 0,
            month_micros,
        };
        assert!(
            budget_needs(spend(0, 0), (0, 0), today).is_empty(),
            "no budget"
        );
        assert!(
            budget_needs(spend(47_000_000, 0), (60, 0), today).is_empty(),
            "78% of the day"
        );
        // $615 of $750 by the 10th: $61.50 a day reaches $750 on the 13th.
        assert_eq!(
            budget_needs(spend(48_000_000, 615_000_000), (60, 750), today),
            vec![
                AdminNeed::Budget {
                    period: Period::Daily,
                    spent_micros: 48_000_000,
                    budget_micros: 60_000_000,
                    pace_day: None,
                },
                AdminNeed::Budget {
                    period: Period::Monthly,
                    spent_micros: 615_000_000,
                    budget_micros: 750_000_000,
                    pace_day: Some(13),
                },
            ]
        );
        // Reached: no pace left to tell.
        assert_eq!(
            budget_needs(spend(0, 750_000_000), (0, 750), today),
            vec![AdminNeed::Budget {
                period: Period::Monthly,
                spent_micros: 750_000_000,
                budget_micros: 750_000_000,
                pace_day: None,
            }]
        );
    }

    #[test]
    fn the_pace_stops_at_the_end_of_the_month() {
        assert_eq!(os::days_in_month(day("2026-02-14")), 28);
        assert_eq!(os::days_in_month(day("2028-02-01")), 29);
        assert_eq!(os::days_in_month(day("2026-10-31")), 31);
        // $10 a day over 30 days of November is $300: a $400 budget holds.
        assert_eq!(
            os::pace_day(day("2026-11-10"), 100_000_000, 400_000_000),
            None
        );
        assert_eq!(
            os::pace_day(day("2026-11-10"), 100_000_000, 300_000_000),
            Some(30)
        );
        assert_eq!(os::pace_day(day("2026-11-10"), 0, 300_000_000), None);
    }
}
